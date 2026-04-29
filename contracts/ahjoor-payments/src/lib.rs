#![no_std]
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, token, Address, Bytes,
    BytesN, Env, Map, String, Symbol, Vec,
};
use ahjoor_token_whitelist::TokenWhitelistClient;

/// Maximum length (bytes) for the optional payment reference string.
const MAX_REFERENCE_LEN: u32 = 64;
/// Maximum number of entries in the optional metadata map.
const MAX_METADATA_KEYS: u32 = 5;
/// Maximum length (bytes) for each metadata key or value.
const MAX_METADATA_KEY_LEN: u32 = 32;
/// Maximum length (bytes) for merchant notification key.
const MAX_NOTIFICATION_KEY_LEN: u32 = 128;

// ---------------------------------------------------------------------------
// Reflector-compatible oracle interface.
// lastprice(base, quote) returns Option<PriceData> where price is scaled by
// 10^decimals(). We call it via a generated client.
// ---------------------------------------------------------------------------
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceData {
    /// Price scaled by 10^7 (Reflector standard precision)
    pub price: i128,
    /// Ledger timestamp of the price update
    pub timestamp: u64,
}

/// Minimal oracle client — only the method we need.
mod oracle {
    use crate::PriceData;
    use soroban_sdk::{contractclient, Address, Env};

    #[allow(dead_code)]
    #[contractclient(name = "OracleClient")]
    pub trait OracleInterface {
        fn lastprice(env: Env, base: Address, quote: Address) -> Option<PriceData>;
    }
}

// --- Storage TTL Constants ---
// Instance storage: counters and config (shared TTL with contract instance)
const INSTANCE_LIFETIME_THRESHOLD: u32 = 100_000;
const INSTANCE_BUMP_AMOUNT: u32 = 120_000;

// Persistent storage: per-record data (Payment, Dispute, CustomerPayments)
// Individual TTL — survives beyond instance TTL, extended on each access.
const PERSISTENT_LIFETIME_THRESHOLD: u32 = 100_000;
const PERSISTENT_BUMP_AMOUNT: u32 = 120_000;

// Temporary storage: in-progress dispute state
// Short-lived; expires automatically if not extended.
const TEMP_LIFETIME_THRESHOLD: u32 = 10_000;
const TEMP_BUMP_AMOUNT: u32 = 15_000;

const DEFAULT_MAX_BATCH_SIZE: u32 = 20;
/// Maximum number of tags per payment (#122)
const MAX_TAGS: u32 = 3;
/// Maximum number of line items in invoice (#128)
const MAX_INVOICE_LINE_ITEMS: u32 = 20;
const MAX_SETTLEMENT_BATCH_SIZE: u32 = 50;
const SETTLEMENT_FEE_BPS: i128 = 0;
const DEFAULT_DISPUTE_TIMEOUT: u64 = 7 * 24 * 60 * 60; // 7 days in seconds
/// Default rate limit: effectively disabled until admin configures stricter values.
const DEFAULT_RATE_LIMIT_MAX_PAYMENTS: u32 = u32::MAX;
const DEFAULT_RATE_LIMIT_WINDOW_SIZE_LEDGERS: u32 = 1;
/// Default slippage tolerance: 50 bps (0.5%)
const DEFAULT_SLIPPAGE_BPS: u32 = 50;
/// Default slippage bounds
const DEFAULT_MIN_SLIPPAGE_BPS: u32 = 0;
const DEFAULT_MAX_SLIPPAGE_BPS: u32 = 10_000;
/// Reflector oracle price precision: prices are scaled by 10^7
const ORACLE_PRICE_PRECISION: i128 = 10_000_000;
/// Ledger sequences per weekly bucket (~7 days at 5s/ledger = 120_960 ledgers)
const LEDGER_BUCKET_SIZE: u32 = 120_960;
/// Maximum protocol fee: 500 bps = 5%
const MAX_FEE_BPS: u32 = 500;
/// Default protocol fee: 0 bps (no fee initially)
const DEFAULT_FEE_BPS: u32 = 0;
/// Idempotency key TTL: 24 hours in ledgers (~17,280 ledgers at 5s/ledger)
const IDEMPOTENCY_KEY_LIFETIME_THRESHOLD: u32 = 10_000;
const IDEMPOTENCY_KEY_BUMP_AMOUNT: u32 = 17_280;
/// Minimum collateral a merchant must maintain at all times (#129)
const DEFAULT_MIN_COLLATERAL: i128 = 1_000_000; // 1 USDC (7 decimals)

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    RateLimitExceeded = 1,
    SubscriptionPaused = 2,
    OracleConditionNotMet = 3,
    /// Subscription's trial period has not elapsed; charging is deferred (#133)
    SubscriptionInTrial = 4,
    MerchantVolumeCapped = 4,
    TokenNotAllowed = 5,
    DuplicateExternalId = 6,
    MultisigNotRequired = 7,
    AlreadyApproved = 8,
    NotASigner = 9,
    VoucherExpired = 10,
    VoucherExhausted = 11,
    VoucherRevoked = 12,
    VoucherNotFound = 13,
    WithdrawalRateLimitExceeded = 14,
    /// Referred merchant already has a merchant record (#242)
    ReferralAlreadyExists = 15,
    /// No pending commission to claim (#242)
    NoCommissionToClaim = 16,
    /// Slippage tolerance exceeded on dynamic payment settlement (#246)
    SlippageExceeded = 15,
    /// Oracle address is not on the admin whitelist (#246)
    OracleNotWhitelisted = 16,
    /// Dynamic payment has expired (#246)
    DynamicPaymentExpired = 17,
    /// Customer cumulative spend would exceed the merchant-configured cap (#235)
    CustomerSpendLimitExceeded = 15,
}

/// Per-merchant withdrawal rate limit config (#231).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawalLimit {
    pub window_seconds: u64,
    pub cap: i128,
}

/// Tracks cumulative withdrawals within the current window (#231).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WithdrawalWindowState {
    pub window_start: u64,
    pub withdrawn: i128,
}

/// Referral record: tracks referrer, registration ledger, and accrual window (#242).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferralRecord {
    pub referrer: Address,
    pub registered_at_ledger: u32,
    pub window_ledgers: u32,
/// Per-customer (or default) spend cap config (#235).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpendLimit {
    pub amount: i128,
    pub window_seconds: u64,
}

/// Tracks cumulative spend within the current rolling window (#235).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomerSpendWindow {
    pub window_start: u64,
    pub spent: i128,
}

/// Direction for oracle price condition (#125)
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OracleDirection {
    Gte = 0,
    Lte = 1,
}

/// Merchant status for fine-grained access control.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MerchantStatus {
    Active = 0,
    Suspended = 1,
    Banned = 2,
}

/// On-chain appeal record for a banned merchant.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerchantAppeal {
    pub merchant: Address,
    pub evidence_hash: BytesN<32>,
    pub submitted_at: u64,
    pub resolved: bool,
    pub approved: bool,
}

/// Conditional release based on oracle price (#125)
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OracleCondition {
    pub asset: Address,
    pub threshold: i128,
    pub direction: OracleDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum PaymentStatus {
    Pending = 0,
    Completed = 1,
    Refunded = 2,
    Disputed = 3,
    Expired = 4,
    Authorized = 5,
    ScheduledPending = 6,
    /// Awaiting M-of-N multi-sig approval before proceeding.
    PendingApproval = 7,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitRecipient {
    pub recipient: Address,
    pub bps: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitTransfer {
    pub recipient: Address,
    pub bps: u32,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeeTier {
    pub min_volume: i128,
    pub fee_bps: u32,
}

/// Invoice line item for payment (#128)
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LineItem {
    pub description: Symbol,
    pub quantity: u32,
    pub unit_price: i128,
}

/// Invoice data attached to payment (#128)
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvoiceData {
    pub line_items: Vec<LineItem>,
    pub tax_bps: u32,
    pub currency_label: Symbol,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Payment {
    pub id: u32,
    pub customer: Address,
    pub merchant: Address,
    pub amount: i128,
    pub token: Address,
    pub status: PaymentStatus,
    pub created_at: u64,
    /// Ledger timestamp after which the payment can be expired. 0 = no expiry.
    pub expires_at: u64,
    /// Cumulative amount refunded via partial refunds.
    pub refunded_amount: i128,
    /// Optional merchant reference string (max 64 bytes) for off-chain reconciliation.
    pub reference: Option<String>,
    /// Optional key-value metadata (max 5 keys, each max 32 bytes).
    pub metadata: Option<Map<String, String>>,
    /// Optional recipient split definitions (must sum to 10,000 bps).
    pub split_recipients: Option<Vec<SplitRecipient>>,
    /// Optional execution timestamp for scheduled payments. 0 = immediate.
    pub execute_after: u64,
    /// Optional payment category for on-chain segmentation (#122)
    pub category: Option<Symbol>,
    /// Optional tags (max 3) immutable after creation (#122)
    pub tags: Option<Vec<Symbol>>,
    /// Ledger timestamp after which an authorized payment can no longer be captured. 0 = not authorized.
    pub capture_deadline: u64,
    // Optional oracle price condition required for completion (#125)
    // pub release_condition: Option<OracleCondition>,
    /// Optional off-chain order correlation key (hash of merchant order ID).
    pub external_id: Option<BytesN<32>>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentRequest {
    pub merchant: Address,
    pub amount: i128,
    pub token: Address,
    pub reference: Option<String>,
    pub metadata: Option<Map<String, String>>,
}

/// Global protocol-wide aggregate statistics (#70).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlobalStats {
    pub total_payments_created: u32,
    pub total_payments_completed: u32,
    pub total_payments_refunded: u32,
    pub total_payments_expired: u32,
    pub total_volume_completed: Map<Address, i128>,
    pub total_volume_refunded: Map<Address, i128>,
}

/// Per-merchant aggregate statistics (#70).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerchantStats {
    pub payments_created: u32,
    pub payments_completed: u32,
    pub payments_refunded: u32,
    pub volume_completed: Map<Address, i128>,
}

/// Per-merchant revenue dashboard summary (#226).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerchantSummary {
    pub total_volume: i128,
    pub completed_count: u32,
    pub failed_count: u32,
    pub pending_count: u32,
    pub volume_by_token: Map<Address, i128>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dispute {
    pub payment_id: u32,
    pub reason: String,
    pub created_at: u64,
    pub resolved: bool,
}

/// Default payment timeout: 7 days in seconds.
const DEFAULT_PAYMENT_TIMEOUT: u64 = 7 * 24 * 60 * 60;

/// Default lower bound for merchant-defined payment expiry overrides (#130): 1 minute.
const DEFAULT_MIN_PAYMENT_EXPIRY: u64 = 60;
/// Default upper bound for merchant-defined payment expiry overrides (#130): 30 days.
const DEFAULT_MAX_PAYMENT_EXPIRY: u64 = 30 * 24 * 60 * 60;

/// Maximum allowed `page_size` for `get_customer_payments_page` (#132).
pub const MAX_CUSTOMER_PAYMENTS_PAGE_SIZE: u32 = 50;

/// Paginated view over a customer's payment IDs (#132).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomerPaymentsPage {
    pub payments: Vec<u32>,
    pub total_count: u32,
    pub has_more: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Subscription {
    pub id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub token: Address,
    pub interval_seconds: u64,
    pub last_charged_at: u64,
    pub max_charges: u32,
    pub charges_count: u32,
    pub active: bool,
    /// True when subscriber has temporarily paused the subscription (#124)
    pub paused: bool,
    /// Ledger timestamp when the subscription was paused (#124)
    pub paused_at: u64,
    /// Ledger timestamp at which the trial ends and the first charge becomes
    /// available. 0 = no trial (immediate first charge). (#133)
    pub trial_ends_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitConfig {
    pub max_payments: u32,
    pub window_size_ledgers: u32,
}

/// Slippage tolerance configuration for multi-token payments (#135)
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlippageConfig {
    pub default_bps: u32,
    pub min_bps: u32,
    pub max_bps: u32,
}

/// Oracle-backed dynamic payment record (#246).
/// The settlement amount is computed at complete_payment time using the oracle rate.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DynamicPayment {
    pub payment_id: u32,
    pub fiat_amount: i128,
    pub fiat_currency: Symbol,
    pub oracle_address: Address,
    pub token: Address,
    pub slippage_bps: u32,
    /// Oracle price at creation time (scaled by 10^7), used for slippage check at settlement
    pub creation_rate: i128,
    pub expiry: u64,
}

/// Per-merchant volume cap configuration (#131)
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeCap {
    pub cap_amount: i128,
    pub window_seconds: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CustomerRateLimit {
    pub count: u32,
    pub window_start_ledger: u32,
}

/// Storage key classification:
/// - Instance:    Admin, PaymentCounter, MaxBatchSize, DisputeTimeout,
///                OracleAddress, UsdcToken, FeeBps, FeeRecipient
///                (config/counters — bounded, shared TTL with contract)
/// - Persistent:  Payment(u32), CustomerPayments(Address), Settled(u32)
///                (per-record data — unbounded, individual TTL)
/// - Temporary:   Dispute(u32), IdempotencyKey(BytesN<32>)
///                (in-progress dispute state, idempotency keys — short-lived, auto-expires)
#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    // --- Instance ---
    Admin,
    PaymentCounter,
    MaxBatchSize,
    DisputeTimeout,
    /// Reflector oracle contract address for token/USDC price feeds
    OracleAddress,
    /// USDC token contract address — canonical settlement currency
    UsdcToken,
    /// Maximum age (seconds) an oracle price may be before rejection
    MaxOracleAge,
    /// Proposed new admin address (pending acceptance)
    ProposedAdmin,
    /// Global emergency stop flag
    Paused,
    /// Human-readable pause reason
    PauseReason,
    /// Global payment timeout in seconds (default: 7 days)
    PaymentTimeout,
    /// Lower bound (inclusive) for merchant-defined per-payment expiry overrides (#130)
    MinPaymentExpiry,
    /// Upper bound (inclusive) for merchant-defined per-payment expiry overrides (#130)
    MaxPaymentExpiry,
    /// When true, merchant allowlist is bypassed (open mode)
    MerchantOpenMode,
    /// Subscription counter
    SubscriptionCounter,
    /// Global per-customer payment creation rate limit config
    RateLimitConfig,
    /// Current contract schema/runtime version
    ContractVersion,
    /// Migration completion flag for a specific version
    MigrationCompleted(u32),
    /// Protocol fee in basis points (1 bps = 0.01%)
    FeeBps,
    /// Address that receives protocol fees
    FeeRecipient,
    /// Volume-based fee tiers sorted by min_volume ascending.
    FeeTiers,
    /// Token whitelist contract address
    TokenWhitelistContract,
    // --- Persistent ---
    Payment(u32),
    CustomerPayments(Address),
    /// Settlement marker to prevent double merchant settlement
    Settled(u32),
    /// Per-customer rate limit usage state
    CustomerRateLimit(Address),
    /// Merchant approval status (true = approved)
    MerchantApproved(Address),
    /// Subscription record
    Subscription(u32),
    /// Index: (merchant, reference_hash) → Vec<u32> of payment IDs
    MerchantReference(Address, u32),
    /// Persistent: sha256 receipt hash for a completed payment (#65)
    /// Hash inputs (big-endian): payment_id(u32) || customer(Address) || merchant(Address)
    ///                           || amount(i128) || token(Address) || completed_at(u64)
    PaymentReceipt(u32),
    /// Persistent: global aggregate statistics (#70)
    GlobalStats,
    /// Persistent: per-merchant aggregate statistics (#70)
    MerchantStats(Address),
    /// Persistent: weekly volume bucket — (token, bucket_id) → total completed volume (#70)
    VolumeBucket(Address, u32),
    /// Persistent: per-merchant weekly volume bucket for rolling tier evaluation.
    MerchantVolumeBucket(Address, u32),
    /// Persistent: cached last tier bps emitted for merchant.
    MerchantCurrentTierBps(Address),
    /// Persistent: (merchant, category) → Vec<payment_id> for category analytics (#122)
    CategoryPayments(Address, Symbol),
    /// Persistent: merchant withdrawal queue — Vec<(payment_id, amount)> (#126)
    WithdrawalQueue(Address),
    /// Persistent: invoice hash (SHA256) for payment (#128)
    InvoiceHash(u32),
    /// Persistent: merchant collateral balance (#129)
    MerchantCollateral(Address),
    /// Instance: minimum collateral required for merchant approval (#129)
    MinCollateral,
    /// Instance: global slippage tolerance config for multi-token payments (#135)
    SlippageConfig,
    /// Persistent: per-merchant volume cap config (#131)
    VolumeCap(Address),
    /// Persistent: per-merchant volume within a time window bucket (#131)
    /// Key: (merchant, window_bucket) where window_bucket = timestamp / window_seconds
    MerchantWindowVolume(Address, u64),
    /// Persistent: merchant notification key for event routing
    MerchantNotificationKey(Address),
    /// Instance: merchant's preferred token for receiving payments
    PreferredToken(Address),
    /// Instance: DEX router contract address for token swaps
    SwapRouter(Address),
    // --- Temporary ---
    Dispute(u32),
    /// Temporary: idempotency key → payment_id mapping (expires after 24h)
    IdempotencyKey(BytesN<32>),
    // --- Task 1: External ID Index ---
    /// Persistent: (merchant, external_id) → payment_id
    ExternalIdIndex(Address, BytesN<32>),
    // --- Task 2: Multi-Sig ---
    /// Instance: per-merchant multi-sig policy
    MultisigPolicy(Address),
    /// Persistent: per-payment approval state
    ApprovalState(u32),
    // --- Task 3: Vouchers ---
    /// Persistent: (merchant, code_hash) → Voucher
    Voucher(Address, BytesN<32>),
    /// Persistent: merchant status (Active/Suspended/Banned)
    MerchantStatus(Address),
    /// Persistent: merchant suspension expiry timestamp
    MerchantSuspensionExpiry(Address),
    /// Persistent: active appeal record per merchant
    MerchantAppeal(Address),
    /// Persistent: timestamp after which a rejected merchant may re-appeal
    AppealCooldownUntil(Address),
    /// Instance: appeal rejection cooldown in seconds
    AppealRejectionCooldownSeconds,
    /// Instance: global default withdrawal window in seconds (#231)
    WithdrawalWindowSeconds,
    /// Instance: global default withdrawal cap per window (#231)
    WithdrawalWindowCap,
    /// Persistent: per-merchant withdrawal limit override (#231)
    MerchantWithdrawalLimit(Address),
    /// Persistent: per-merchant withdrawal tracker for current window (#231)
    MerchantWithdrawalWindow(Address),
    /// Persistent: per-merchant revenue dashboard summary (#226)
    MerchantSummary(Address),
    // --- #239: Loyalty Points ---
    /// Instance: points earned per 1_000_000 units of payment token
    LoyaltyPointsPerUnit,
    /// Instance: discount in basis points per 1 point redeemed
    LoyaltyRedemptionRateBps,
    /// Instance: minimum payment floor after discount (in token units)
    LoyaltyMinPaymentFloor,
    /// Instance: ledgers after which unspent points expire (0 = no expiry)
    LoyaltyExpiryLedgers,
    /// Persistent: customer loyalty points balance
    LoyaltyBalance(Address),
    /// Persistent: ledger at which customer's points were last accrued (for expiry)
    LoyaltyLastAccrualLedger(Address),
    /// Instance: global referral commission in basis points (#242)
    ReferralCommissionBps,
    /// Instance: global referral window in ledgers (#242)
    ReferralWindowLedgers,
    /// Persistent: referral record for a referred merchant (#242)
    ReferralRecord(Address),
    /// Persistent: pending commission balance for a referrer (#242)
    PendingCommission(Address),
    /// Persistent: dynamic payment record (#246)
    DynamicPayment(u32),
    /// Instance: admin-maintained oracle whitelist (#246)
    OracleWhitelist,
    /// Persistent: per-(merchant,customer) spend limit override (#235)
    CustomerSpendLimit(Address, Address),
    /// Persistent: merchant-level default spend limit (#235)
    DefaultSpendLimit(Address),
    /// Persistent: per-(merchant,customer) rolling spend window state (#235)
    CustomerSpendWindowState(Address, Address),
    /// #216: recurring invoice counter
    RecurringInvoiceCounter,
    /// #216: recurring invoice record
    RecurringInvoice(u32),
}

mod events;

/// #216: Recurring invoice schedule.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecurringInvoice {
    pub id: u32,
    pub merchant: Address,
    pub customer: Address,
    pub amount: i128,
    pub token: Address,
    pub interval_seconds: u64,
    pub max_cycles: u32,
    pub cycles_triggered: u32,
    pub next_due_at: u64,
    pub reference_hash: Option<BytesN<32>>,
    pub active: bool,
}

#[contract]
pub struct AhjoorPaymentsContract;

#[contractimpl]
impl AhjoorPaymentsContract {
    /// One-time contract initialization.
    /// Admin, counters, and config go to instance storage.
    /// fee_recipient: Address that receives protocol fees
    /// fee_bps: Protocol fee in basis points (max 500 = 5%)
    pub fn initialize(env: Env, admin: Address, fee_recipient: Address, fee_bps: u32) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("Already initialized");
        }

        if fee_bps > MAX_FEE_BPS {
            panic!("Fee cannot exceed 500 bps (5%)");
        }

        // Instance: config and counters
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::PaymentCounter, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::MaxBatchSize, &DEFAULT_MAX_BATCH_SIZE);
        env.storage()
            .instance()
            .set(&DataKey::DisputeTimeout, &DEFAULT_DISPUTE_TIMEOUT);
        env.storage().instance().set(
            &DataKey::RateLimitConfig,
            &RateLimitConfig {
                max_payments: DEFAULT_RATE_LIMIT_MAX_PAYMENTS,
                window_size_ledgers: DEFAULT_RATE_LIMIT_WINDOW_SIZE_LEDGERS,
            },
        );
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &1u32);
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage()
            .instance()
            .set(&DataKey::FeeRecipient, &fee_recipient);
        env.storage()
            .instance()
            .set(&DataKey::FeeTiers, &Vec::<FeeTier>::new(&env));

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Create a single payment: transfer tokens from customer to contract (escrow).
    /// Payment record stored in persistent storage with individual TTL.
    /// Rejects unapproved merchants unless open mode is enabled (#58).
    /// Sets expiry based on global payment timeout (#54).
    /// Accepts optional reference (max 64 bytes) and metadata (max 5 keys) (#67).
    /// Accepts optional idempotency_key to prevent duplicate payments.
    /// Returns the new payment ID.
    pub fn create_payment(
        env: Env,
        customer: Address,
        merchant: Address,
        amount: i128,
        token: Address,
        reference: Option<String>,
        metadata: Option<Map<String, String>>,
        idempotency_key: Option<BytesN<32>>,
    ) -> u32 {
        Self::create_payment_with_options(
            env,
            customer,
            merchant,
            amount,
            token,
            reference,
            metadata,
            None,
            None,
            idempotency_key,
        )
    }

    /// Create a payment with optional invoice data attached (#128).
    pub fn create_payment_with_invoice(
        env: Env,
        customer: Address,
        merchant: Address,
        amount: i128,
        token: Address,
        reference: Option<String>,
        metadata: Option<Map<String, String>>,
        invoice: Option<InvoiceData>,
        idempotency_key: Option<BytesN<32>>,
    ) -> u32 {
        // Validate invoice against payment amount
        Self::validate_invoice_data(&env, &invoice, amount);

        // Create the payment using the base function
        let payment_id = Self::create_payment_with_options(
            env.clone(),
            customer,
            merchant,
            amount,
            token,
            reference,
            metadata,
            None,
            None,
            idempotency_key,
        );

        // If invoice provided, compute hash and store it
        if let Some(inv) = invoice {
            let invoice_hash = Self::compute_invoice_hash(&env, &inv);
            env.storage()
                .persistent()
                .set(&DataKey::InvoiceHash(payment_id), &invoice_hash);
            env.storage().persistent().extend_ttl(
                &DataKey::InvoiceHash(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
            events::emit_invoice_attached(&env, payment_id, invoice_hash);
        }

        payment_id
    }

    /// Extended payment creation with optional recipient splits and scheduling.
    #[allow(clippy::too_many_arguments)]
    pub fn create_payment_with_options(
        env: Env,
        customer: Address,
        merchant: Address,
        amount: i128,
        token: Address,
        reference: Option<String>,
        metadata: Option<Map<String, String>>,
        split_recipients: Option<Vec<SplitRecipient>>,
        execute_after: Option<u64>,
        idempotency_key: Option<BytesN<32>>,
    ) -> u32 {
        Self::create_payment_with_expiry(
            env,
            customer,
            merchant,
            amount,
            token,
            reference,
            metadata,
            split_recipients,
            execute_after,
            idempotency_key,
            None,
        )
    }

    /// Extended payment creation with an optional merchant-defined expiry (#130).
    ///
    /// When `expiry_seconds` is `Some(value)`, `value` must lie within the
    /// admin-configured `[min_expiry, max_expiry]` bounds and replaces the
    /// global default for this payment only. When `None`, the existing global
    /// `PaymentTimeout` is used (preserving the previous behaviour).
    #[allow(clippy::too_many_arguments)]
    pub fn create_payment_with_expiry(
        env: Env,
        customer: Address,
        merchant: Address,
        amount: i128,
        token: Address,
        reference: Option<String>,
        metadata: Option<Map<String, String>>,
        split_recipients: Option<Vec<SplitRecipient>>,
        execute_after: Option<u64>,
        idempotency_key: Option<BytesN<32>>,
        expiry_seconds: Option<u64>,
    ) -> u32 {
        Self::require_not_paused(&env);
        customer.require_auth();

        // Check idempotency key first
        if let Some(ref key) = idempotency_key {
            if let Some(existing_payment_id) = env
                .storage()
                .temporary()
                .get::<DataKey, u32>(&DataKey::IdempotencyKey(key.clone()))
            {
                // Key exists, return existing payment ID
                return existing_payment_id;
            }
        }

        Self::enforce_rate_limit(&env, &customer, 1);

        if amount <= 0 {
            panic!("Payment amount must be positive");
        }

        // Validate optional reference and metadata (#67)
        Self::validate_reference(&env, &reference);
        Self::validate_metadata(&env, &metadata);
        Self::validate_split_recipients(&split_recipients);

        // Token whitelist validation
        Self::require_token_allowed(&env, &token);

        // Merchant allowlist check (#58)
        Self::require_merchant_approved(&env, &merchant);

        let client = token::Client::new(&env, &token);
        client.transfer(&customer, &env.current_contract_address(), &amount);

        let default_timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PaymentTimeout)
            .unwrap_or(DEFAULT_PAYMENT_TIMEOUT);
        let custom_expiry = match expiry_seconds {
            Some(value) => {
                Self::require_expiry_within_bounds(&env, value);
                Some(value)
            }
            None => None,
        };
        let timeout = custom_expiry.unwrap_or(default_timeout);
        let now = env.ledger().timestamp();
        let execute_after_ts = execute_after.unwrap_or(0);
        let status = if execute_after_ts > now {
            PaymentStatus::ScheduledPending
        } else {
            PaymentStatus::Pending
        };

        let payment_id = Self::next_payment_id(&env);
        let payment = Payment {
            id: payment_id,
            customer: customer.clone(),
            merchant: merchant.clone(),
            amount,
            token: token.clone(),
            status,
            created_at: now,
            expires_at: now + timeout,
            refunded_amount: 0,
            reference: reference.clone(),
            metadata,
            split_recipients,
            execute_after: execute_after_ts,
            category: None,
            tags: None,
            capture_deadline: 0,
            // // release_condition: None,
            external_id: None,
        };

        // Persistent: per-payment record with individual TTL
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::add_customer_payment(&env, &customer, payment_id);

        // Index by merchant+reference if provided (#67)
        if let Some(ref r) = reference {
            Self::index_payment_by_reference(&env, &merchant, r, payment_id);
        }

        // Store idempotency key if provided
        if let Some(key) = idempotency_key {
            env.storage()
                .temporary()
                .set(&DataKey::IdempotencyKey(key.clone()), &payment_id);
            env.storage().temporary().extend_ttl(
                &DataKey::IdempotencyKey(key),
                IDEMPOTENCY_KEY_LIFETIME_THRESHOLD,
                IDEMPOTENCY_KEY_BUMP_AMOUNT,
            );
        }

        // Update stats (#70)
        Self::inc_global_created(&env);
        Self::inc_merchant_created(&env, &merchant);

        events::emit_payment_created(&env, payment_id, customer, merchant, amount, token);
        if execute_after_ts > now {
            events::emit_payment_scheduled(&env, payment_id, execute_after_ts);
        }
        if let Some(value) = custom_expiry {
            events::emit_payment_expiry_override(&env, payment_id, value);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        payment_id
    }

    /// Create multiple payments atomically.
    /// Returns a Vec of payment IDs.
    pub fn create_payments_batch(
        env: Env,
        customer: Address,
        payments: Vec<PaymentRequest>,
    ) -> Vec<u32> {
        Self::require_not_paused(&env);
        customer.require_auth();

        let batch_len = payments.len();
        if batch_len == 0 {
            panic!("Batch cannot be empty");
        }
        Self::enforce_rate_limit(&env, &customer, batch_len);

        let max_batch_size: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxBatchSize)
            .unwrap_or(DEFAULT_MAX_BATCH_SIZE);

        if batch_len > max_batch_size {
            panic!("Batch size exceeds maximum allowed");
        }

        let mut payment_ids = Vec::new(&env);
        let mut total_amount: i128 = 0;

        let timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PaymentTimeout)
            .unwrap_or(DEFAULT_PAYMENT_TIMEOUT);
        let now = env.ledger().timestamp();

        for request in payments.iter() {
            if request.amount <= 0 {
                panic!("Payment amount must be positive");
            }

            Self::validate_reference(&env, &request.reference);
            Self::validate_metadata(&env, &request.metadata);
            Self::require_token_allowed(&env, &request.token);
            Self::require_merchant_approved(&env, &request.merchant);

            let client = token::Client::new(&env, &request.token);
            client.transfer(&customer, &env.current_contract_address(), &request.amount);

            let payment_id = Self::next_payment_id(&env);
            let payment = Payment {
                id: payment_id,
                customer: customer.clone(),
                merchant: request.merchant.clone(),
                amount: request.amount,
                token: request.token.clone(),
                status: PaymentStatus::Pending,
                created_at: now,
                expires_at: now + timeout,
                refunded_amount: 0,
                reference: request.reference.clone(),
                metadata: request.metadata.clone(),
                split_recipients: None,
                execute_after: 0,
                category: None,
                tags: None,
                capture_deadline: 0,
                // release_condition: None,
                external_id: None,
            };

            // Persistent: per-payment record with individual TTL
            env.storage()
                .persistent()
                .set(&DataKey::Payment(payment_id), &payment);
            env.storage().persistent().extend_ttl(
                &DataKey::Payment(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );

            Self::add_customer_payment(&env, &customer, payment_id);

            // Index by merchant+reference if provided (#67)
            if let Some(ref r) = request.reference {
                Self::index_payment_by_reference(&env, &request.merchant, r, payment_id);
            }

            // Update stats (#70)
            Self::inc_global_created(&env);
            Self::inc_merchant_created(&env, &request.merchant);

            events::emit_payment_created(
                &env,
                payment_id,
                customer.clone(),
                request.merchant.clone(),
                request.amount,
                request.token.clone(),
            );

            payment_ids.push_back(payment_id);
            total_amount += request.amount;
        }

        events::emit_batch_payment_created(&env, customer, batch_len, total_amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        payment_ids
    }

    /// Admin completes an immediate payment.
    pub fn complete_payment(env: Env, payment_id: u32) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        Self::complete_payment_internal(&env, payment_id, false);
    }

    /// Execute a scheduled payment once execute_after has passed. Callable by anyone.
    pub fn execute_scheduled_payment(env: Env, payment_id: u32) {
        Self::require_not_paused(&env);
        Self::complete_payment_internal(&env, payment_id, true);
        events::emit_scheduled_payment_executed(&env, payment_id);
    }

    /// Customer can cancel a scheduled payment before execution and get refunded.
    pub fn cancel_scheduled_payment(env: Env, customer: Address, payment_id: u32) {
        Self::require_not_paused(&env);
        customer.require_auth();

        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.customer != customer {
            panic!("Only payment customer can cancel scheduled payment");
        }
        if payment.status != PaymentStatus::ScheduledPending {
            panic!("Payment is not scheduled");
        }
        if env.ledger().timestamp() >= payment.execute_after {
            panic!("Scheduled payment is ready to execute");
        }

        let client = token::Client::new(&env, &payment.token);
        client.transfer(
            &env.current_contract_address(),
            &payment.customer,
            &payment.amount,
        );

        let old_status = payment.status;
        payment.status = PaymentStatus::Refunded;
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::inc_global_refunded(&env, &payment.token, payment.amount);
        Self::inc_merchant_refunded(&env, &payment.merchant, &payment.token, payment.amount);
        events::emit_payment_status_changed(&env, payment_id, old_status, PaymentStatus::Refunded);
    }

    /// Settle a merchant batch in one transfer.
    /// Validates all payment IDs first, then executes settlement atomically.
    pub fn settle_merchant_payments(
        env: Env,
        admin: Address,
        merchant: Address,
        payment_ids: Vec<u32>,
    ) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can settle merchant payments");
        }

        let batch_size = payment_ids.len();
        if batch_size == 0 {
            panic!("Settlement batch cannot be empty");
        }
        if batch_size > MAX_SETTLEMENT_BATCH_SIZE {
            panic!("Settlement batch size exceeds maximum allowed");
        }

        let first_payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_ids.get(0).unwrap()))
            .expect("Payment not found");
        let settlement_token = first_payment.token.clone();

        let mut total_amount: i128 = 0;
        for payment_id in payment_ids.iter() {
            let payment: Payment = env
                .storage()
                .persistent()
                .get(&DataKey::Payment(payment_id))
                .expect("Payment not found");

            if payment.status != PaymentStatus::Completed {
                panic!("Payment is not completed");
            }
            if payment.merchant != merchant {
                panic!("Payment does not belong to merchant");
            }
            if payment.token != settlement_token {
                panic!("All payments in batch must have same token");
            }
            let settled: bool = env
                .storage()
                .persistent()
                .get(&DataKey::Settled(payment_id))
                .unwrap_or(false);
            if settled {
                panic!("Payment already settled");
            }

            total_amount = total_amount
                .checked_add(payment.amount)
                .expect("Settlement amount overflow");
        }

        let fee_collected = (total_amount * SETTLEMENT_FEE_BPS) / 10_000;
        let net_amount = total_amount
            .checked_sub(fee_collected)
            .expect("Settlement fee exceeds amount");

        // #231: Enforce withdrawal rate limit
        Self::check_and_update_withdrawal_rate_limit(&env, &merchant, net_amount);

        let token_client = token::Client::new(&env, &settlement_token);
        token_client.transfer(&env.current_contract_address(), &merchant, &net_amount);

        for payment_id in payment_ids.iter() {
            env.storage()
                .persistent()
                .set(&DataKey::Settled(payment_id), &true);
            env.storage().persistent().extend_ttl(
                &DataKey::Settled(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        events::emit_batch_settlement_processed(
            &env,
            merchant,
            total_amount,
            fee_collected,
            batch_size,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- Dispute Methods ---

    /// Customer disputes a Pending payment. Dispute state stored in temporary storage
    /// (short-lived, in-progress — auto-expires once resolved or timed out).
    pub fn dispute_payment(env: Env, customer: Address, payment_id: u32, reason: String) {
        Self::require_not_paused(&env);
        customer.require_auth();

        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.customer != customer {
            panic!("Only the payment customer can dispute");
        }

        if payment.status != PaymentStatus::Pending && payment.status != PaymentStatus::Authorized {
            panic!("Only pending or authorized payments can be disputed");
        }

        let old_status = payment.status;
        payment.status = PaymentStatus::Disputed;

        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Temporary: active dispute state — short-lived, expires if not resolved
        let dispute = Dispute {
            payment_id,
            reason: reason.clone(),
            created_at: env.ledger().timestamp(),
            resolved: false,
        };
        env.storage()
            .temporary()
            .set(&DataKey::Dispute(payment_id), &dispute);
        env.storage().temporary().extend_ttl(
            &DataKey::Dispute(payment_id),
            TEMP_LIFETIME_THRESHOLD,
            TEMP_BUMP_AMOUNT,
        );

        events::emit_payment_disputed(&env, payment_id, customer, reason);
        events::emit_payment_status_changed(&env, payment_id, old_status, PaymentStatus::Disputed);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin resolves a dispute. Clears temporary dispute state on resolution.
    pub fn resolve_dispute(env: Env, payment_id: u32, release_to_merchant: bool) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.status != PaymentStatus::Disputed {
            panic!("Payment is not disputed");
        }

        let client = token::Client::new(&env, &payment.token);
        let old_status = payment.status;

        if release_to_merchant {
            payment.status = PaymentStatus::Completed;
            env.storage()
                .persistent()
                .set(&DataKey::Settled(payment_id), &false);
            env.storage().persistent().extend_ttl(
                &DataKey::Settled(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        } else {
            // Resolved in customer's favour: refund from escrow first.
            // If the escrowed amount is insufficient (e.g. partial refunds already
            // issued), cover the shortfall by slashing merchant collateral (#129).
            let already_refunded = payment.refunded_amount;
            let owed_to_customer = payment.amount - already_refunded;

            if owed_to_customer > 0 {
                // Try to cover from escrow (the remaining escrowed balance).
                // The contract holds `owed_to_customer` for this payment in escrow.
                client.transfer(
                    &env.current_contract_address(),
                    &payment.customer,
                    &owed_to_customer,
                );
            }

            // Check whether collateral needs to be slashed.
            // Slashing applies when the merchant's pending balance is insufficient
            // to cover the refund — i.e. the payment token differs from the
            // collateral token (USDC) or the admin explicitly signals a shortfall.
            // For simplicity we slash collateral equal to the full disputed amount
            // only when the payment token is the collateral token (USDC), so the
            // slash covers the exact economic loss.
            let usdc_token: Option<Address> = env
                .storage()
                .instance()
                .get(&DataKey::UsdcToken);
            if let Some(ref usdc) = usdc_token {
                if payment.token == *usdc && owed_to_customer > 0 {
                    let collateral_key =
                        DataKey::MerchantCollateral(payment.merchant.clone());
                    let collateral: i128 = env
                        .storage()
                        .persistent()
                        .get(&collateral_key)
                        .unwrap_or(0);

                    if collateral > 0 {
                        // Slash up to the full owed amount, capped by available collateral.
                        let slash_amount = owed_to_customer.min(collateral);
                        let new_collateral = collateral - slash_amount;
                        env.storage()
                            .persistent()
                            .set(&collateral_key, &new_collateral);
                        env.storage().persistent().extend_ttl(
                            &collateral_key,
                            PERSISTENT_LIFETIME_THRESHOLD,
                            PERSISTENT_BUMP_AMOUNT,
                        );
                        events::emit_collateral_slashed(
                            &env,
                            payment.merchant.clone(),
                            slash_amount,
                            payment_id,
                        );
                    }
                }
            }

            payment.status = PaymentStatus::Refunded;
        }
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Mark dispute resolved in temporary storage, then let it expire naturally
        if let Some(mut dispute) = env
            .storage()
            .temporary()
            .get::<DataKey, Dispute>(&DataKey::Dispute(payment_id))
        {
            dispute.resolved = true;
            env.storage()
                .temporary()
                .set(&DataKey::Dispute(payment_id), &dispute);
            // No TTL extension — resolved disputes can expire on their own
        }

        // Update stats (#70)
        if !release_to_merchant {
            Self::inc_global_refunded(&env, &payment.token, payment.amount);
            Self::inc_merchant_refunded(&env, &payment.merchant, &payment.token, payment.amount);
        }

        events::emit_dispute_resolved(&env, payment_id, release_to_merchant, admin);
        events::emit_payment_status_changed(&env, payment_id, old_status, payment.status);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Check if a dispute has exceeded the timeout window.
    pub fn check_escalation(env: Env, payment_id: u32) -> bool {
        let payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.status != PaymentStatus::Disputed {
            return false;
        }

        let dispute: Dispute = env
            .storage()
            .temporary()
            .get(&DataKey::Dispute(payment_id))
            .expect("Dispute not found");

        if dispute.resolved {
            return false;
        }

        let timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::DisputeTimeout)
            .unwrap_or(DEFAULT_DISPUTE_TIMEOUT);

        let elapsed = env.ledger().timestamp() - dispute.created_at;
        if elapsed > timeout {
            events::emit_dispute_escalated(&env, payment_id, elapsed);
            return true;
        }

        false
    }

    // --- Oracle / Multi-Token ---

    /// Admin sets the oracle contract address, USDC token address, and max
    /// oracle price age. Must be called before create_payment_multi_token.
    pub fn set_oracle(env: Env, oracle: Address, usdc_token: Address, max_oracle_age: u64) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        if max_oracle_age == 0 {
            panic!("max_oracle_age must be positive");
        }

        env.storage()
            .instance()
            .set(&DataKey::OracleAddress, &oracle);
        env.storage()
            .instance()
            .set(&DataKey::UsdcToken, &usdc_token);
        env.storage()
            .instance()
            .set(&DataKey::MaxOracleAge, &max_oracle_age);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Create a payment where the customer pays in any supported token.
    /// The oracle provides the token/USDC rate. The contract:
    ///   1. Queries the oracle for the current price of `payment_token` in USDC.
    ///   2. Validates price freshness against `max_oracle_age`.
    ///   3. Calculates `required_token_amount` from `amount_usdc` and the rate.
    ///   4. Applies slippage tolerance: rejects if effective rate deviates
    ///      more than `slippage_tolerance_bps` basis points from the oracle rate.
    ///      If omitted, uses the global default from SlippageConfig.
    ///   5. Transfers `required_token_amount` of `payment_token` from customer
    ///      to contract (escrow).
    ///   6. Records the payment with `amount = amount_usdc` and `token = usdc_token`
    ///      so that complete_payment always releases USDC to the merchant.
    ///
    /// Fallback: if `payment_token == usdc_token`, behaves identically to
    /// create_payment (no oracle call, no conversion).
    ///
    /// Returns the new payment ID.
    pub fn create_payment_multi_token(
        env: Env,
        customer: Address,
        merchant: Address,
        amount_usdc: i128,
        payment_token: Address,
        slippage_tolerance_bps: Option<u32>,
    ) -> u32 {
        Self::require_not_paused(&env);
        if amount_usdc <= 0 {
            panic!("Payment amount must be positive");
        }

        // Resolve slippage: use provided value or fall back to global default
        let slippage_cfg = Self::get_slippage_config_internal(&env);
        let slippage_bps = match slippage_tolerance_bps {
            Some(bps) => {
                if bps < slippage_cfg.min_bps {
                    panic!("slippage_bps below minimum allowed");
                }
                if bps > slippage_cfg.max_bps {
                    panic!("slippage_bps exceeds maximum allowed");
                }
                bps
            }
            None => slippage_cfg.default_bps,
        };

        let usdc_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::UsdcToken)
            .expect("Oracle not configured");

        // --- Fallback: direct USDC payment, no oracle needed ---
        if payment_token == usdc_token {
            customer.require_auth();
            Self::enforce_rate_limit(&env, &customer, 1);
            Self::require_token_allowed(&env, &payment_token);
            Self::require_merchant_approved(&env, &merchant);

            let client = token::Client::new(&env, &payment_token);
            client.transfer(&customer, &env.current_contract_address(), &amount_usdc);

            let timeout: u64 = env
                .storage()
                .instance()
                .get(&DataKey::PaymentTimeout)
                .unwrap_or(DEFAULT_PAYMENT_TIMEOUT);
            let now = env.ledger().timestamp();

            let payment_id = Self::next_payment_id(&env);
            let payment = Payment {
                id: payment_id,
                customer: customer.clone(),
                merchant: merchant.clone(),
                amount: amount_usdc,
                token: payment_token.clone(),
                status: PaymentStatus::Pending,
                created_at: now,
                expires_at: now + timeout,
                refunded_amount: 0,
                reference: None,
                metadata: None,
                split_recipients: None,
                execute_after: 0,
                category: None,
                tags: None,
                capture_deadline: 0,
                // release_condition: None,
                external_id: None,
            };

            env.storage()
                .persistent()
                .set(&DataKey::Payment(payment_id), &payment);
            env.storage().persistent().extend_ttl(
                &DataKey::Payment(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );

            Self::add_customer_payment(&env, &customer, payment_id);
            events::emit_payment_created(
                &env,
                payment_id,
                customer,
                merchant,
                amount_usdc,
                payment_token,
            );
            env.storage()
                .instance()
                .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

            return payment_id;
        }

        customer.require_auth();
        Self::enforce_rate_limit(&env, &customer, 1);
        Self::require_token_allowed(&env, &payment_token);
        Self::require_merchant_approved(&env, &merchant);

        let oracle_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::OracleAddress)
            .expect("Oracle not configured");
        let max_oracle_age: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MaxOracleAge)
            .expect("Oracle not configured");

        // --- Query oracle: price of payment_token denominated in USDC ---
        // Oracle returns price scaled by ORACLE_PRICE_PRECISION (10^7).
        let oracle_client = oracle::OracleClient::new(&env, &oracle_addr);
        let price_data: PriceData = oracle_client
            .lastprice(&payment_token, &usdc_token)
            .expect("Oracle price unavailable");

        // --- Freshness check ---
        let current_ts = env.ledger().timestamp();
        let age = current_ts.saturating_sub(price_data.timestamp);
        if age > max_oracle_age {
            panic!("Oracle price is stale");
        }

        if price_data.price <= 0 {
            panic!("Invalid oracle price");
        }

        // --- Calculate required payment_token amount ---
        // price = payment_token per USDC, scaled by 10^7
        // required = amount_usdc * 10^7 / price
        let required_token_amount = (amount_usdc * ORACLE_PRICE_PRECISION) / price_data.price;
        if required_token_amount <= 0 {
            panic!("Computed token amount is zero");
        }

        // --- Slippage check ---
        // Effective USDC value of required_token_amount at oracle rate must be
        // within slippage_bps of amount_usdc.
        // effective_usdc = required_token_amount * price / 10^7
        // deviation_bps = abs(effective_usdc - amount_usdc) * 10000 / amount_usdc
        let effective_usdc = (required_token_amount * price_data.price) / ORACLE_PRICE_PRECISION;
        let deviation = if effective_usdc >= amount_usdc {
            effective_usdc - amount_usdc
        } else {
            amount_usdc - effective_usdc
        };
        let deviation_bps = (deviation * 10_000) / amount_usdc;
        if deviation_bps > slippage_bps as i128 {
            panic!("Slippage tolerance exceeded");
        }

        // --- Transfer payment_token from customer to contract (escrow) ---
        let pay_client = token::Client::new(&env, &payment_token);
        pay_client.transfer(
            &customer,
            &env.current_contract_address(),
            &required_token_amount,
        );

        // --- Record payment in USDC terms so settlement releases USDC ---
        let timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PaymentTimeout)
            .unwrap_or(DEFAULT_PAYMENT_TIMEOUT);
        let now = env.ledger().timestamp();
        let payment_id = Self::next_payment_id(&env);
        let payment = Payment {
            id: payment_id,
            customer: customer.clone(),
            merchant: merchant.clone(),
            amount: amount_usdc,
            token: usdc_token.clone(),
            status: PaymentStatus::Pending,
            created_at: now,
            expires_at: now + timeout,
            refunded_amount: 0,
            reference: None,
            metadata: None,
            split_recipients: None,
            execute_after: 0,
            category: None,
            tags: None,
            capture_deadline: 0,
            // release_condition: None,
            external_id: None,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::add_customer_payment(&env, &customer, payment_id);

        // Emit SlippageToleranceApplied event (#135)
        events::emit_slippage_tolerance_applied(
            &env,
            payment_id,
            slippage_bps,
            price_data.price,
            amount_usdc,
        );

        events::emit_multi_token_payment_created(
            &env,
            payment_id,
            customer,
            merchant,
            amount_usdc,
            payment_token,
            required_token_amount,
            price_data.price,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        payment_id
    }

    pub fn get_oracle_address(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::OracleAddress)
            .expect("Oracle not configured")
    }

    pub fn get_usdc_token(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::UsdcToken)
            .expect("Oracle not configured")
    }

    pub fn get_max_oracle_age(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MaxOracleAge)
            .expect("Oracle not configured")
    }

    // --- Admin ---

    pub fn set_max_batch_size(env: Env, new_size: u32) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        if new_size == 0 {
            panic!("Max batch size must be at least 1");
        }

        env.storage()
            .instance()
            .set(&DataKey::MaxBatchSize, &new_size);
    }

    pub fn set_dispute_timeout(env: Env, timeout: u64) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        if timeout == 0 {
            panic!("Dispute timeout must be positive");
        }

        env.storage()
            .instance()
            .set(&DataKey::DisputeTimeout, &timeout);
    }

    /// Admin updates the protocol fee. Maximum allowed is 500 bps (5%).
    pub fn update_fee(env: Env, admin: Address, new_fee_bps: u32) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can update fee");
        }
        if new_fee_bps > MAX_FEE_BPS {
            panic!("Fee cannot exceed 500 bps (5%)");
        }

        env.storage().instance().set(&DataKey::FeeBps, &new_fee_bps);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin updates the fee recipient address.
    pub fn update_fee_recipient(env: Env, admin: Address, new_fee_recipient: Address) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can update fee recipient");
        }

        env.storage()
            .instance()
            .set(&DataKey::FeeRecipient, &new_fee_recipient);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the current protocol fee in basis points.
    pub fn get_fee_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::FeeBps)
            .unwrap_or(DEFAULT_FEE_BPS)
    }

    /// Get the current fee recipient address.
    pub fn get_fee_recipient(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::FeeRecipient)
            .expect("Fee recipient not configured")
    }

    /// Admin updates the ascending fee tier table.
    pub fn update_fee_tiers(env: Env, admin: Address, tiers: Vec<FeeTier>) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can update fee tiers");
        }

        Self::validate_fee_tiers(&tiers);
        env.storage().instance().set(&DataKey::FeeTiers, &tiers);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_fee_tiers(env: Env) -> Vec<FeeTier> {
        env.storage()
            .instance()
            .get(&DataKey::FeeTiers)
            .unwrap_or(Vec::new(&env))
    }

    pub fn get_merchant_fee_tier(env: Env, merchant: Address) -> u32 {
        let volume = Self::rolling_merchant_volume(&env, &merchant);
        Self::fee_bps_for_volume(&env, volume)
    }

    /// Admin updates global per-customer payment rate limit settings.
    pub fn update_rate_limit_config(
        env: Env,
        admin: Address,
        max_payments: u32,
        window_size_ledgers: u32,
    ) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can update rate limit config");
        }
        if max_payments == 0 {
            panic!("max_payments must be positive");
        }
        if window_size_ledgers == 0 {
            panic!("window_size_ledgers must be positive");
        }

        env.storage().instance().set(
            &DataKey::RateLimitConfig,
            &RateLimitConfig {
                max_payments,
                window_size_ledgers,
            },
        );
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin updates the global slippage tolerance config for multi-token payments (#135).
    /// default_bps must be within [min_bps, max_bps].
    pub fn update_slippage_config(
        env: Env,
        admin: Address,
        default_bps: u32,
        min_bps: u32,
        max_bps: u32,
    ) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can update slippage config");
        }
        if max_bps > 10_000 {
            panic!("max_bps cannot exceed 10000");
        }
        if min_bps > max_bps {
            panic!("min_bps cannot exceed max_bps");
        }
        if default_bps < min_bps || default_bps > max_bps {
            panic!("default_bps must be within [min_bps, max_bps]");
        }
        env.storage()
            .instance()
            .set(&DataKey::SlippageConfig, &SlippageConfig { default_bps, min_bps, max_bps });
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns the current slippage tolerance config.
    pub fn get_slippage_config(env: Env) -> SlippageConfig {
        Self::get_slippage_config_internal(&env)
    }

    /// Admin sets a volume cap for a merchant (#131).
    /// cap_amount: maximum cumulative settlement volume within window_seconds.
    /// Set cap_amount = 0 to remove the cap.
    pub fn set_merchant_volume_cap(
        env: Env,
        admin: Address,
        merchant: Address,
        cap_amount: i128,
        window_seconds: u64,
    ) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can set merchant volume cap");
        }
        if cap_amount < 0 {
            panic!("cap_amount cannot be negative");
        }
        if cap_amount > 0 && window_seconds == 0 {
            panic!("window_seconds must be positive when cap is set");
        }
        let key = DataKey::VolumeCap(merchant.clone());
        if cap_amount == 0 {
            env.storage().persistent().remove(&key);
        } else {
            env.storage().persistent().set(&key, &VolumeCap { cap_amount, window_seconds });
            env.storage().persistent().extend_ttl(
                &key,
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns the volume cap for a merchant, if set (#131).
    pub fn get_merchant_volume_cap(env: Env, merchant: Address) -> Option<VolumeCap> {
        env.storage()
            .persistent()
            .get(&DataKey::VolumeCap(merchant))
    }

    /// Propose a new admin address. Only the current admin can propose.
    pub fn propose_admin_transfer(env: Env, proposed_admin: Address) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        env.storage()
            .instance()
            .set(&DataKey::ProposedAdmin, &proposed_admin);

        events::emit_admin_transfer_proposed(&env, admin, proposed_admin);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Accept the admin role. Only the proposed admin can accept.
    pub fn accept_admin_role(env: Env) {
        Self::require_not_paused(&env);
        let proposed_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::ProposedAdmin)
            .expect("No admin transfer proposed");
        proposed_admin.require_auth();

        let old_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");

        env.storage()
            .instance()
            .set(&DataKey::Admin, &proposed_admin);
        env.storage().instance().remove(&DataKey::ProposedAdmin);

        events::emit_admin_transferred(&env, old_admin, proposed_admin);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the current admin address.
    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized")
    }

    /// Upgrade this contract's WASM code. Admin only.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can upgrade contract");
        }

        let old_version = Self::get_or_init_version(&env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);

        let new_version = old_version.checked_add(1).expect("Version overflow");
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &new_version);

        events::emit_contract_upgraded(&env, old_version, new_version, admin);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Run one-time migration logic for the current version. Admin only.
    pub fn migrate(env: Env, admin: Address) {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can migrate contract");
        }

        let version = Self::get_or_init_version(&env);
        if env
            .storage()
            .instance()
            .get(&DataKey::MigrationCompleted(version))
            .unwrap_or(false)
        {
            panic!("Migration already completed for this version");
        }

        env.storage()
            .instance()
            .set(&DataKey::MigrationCompleted(version), &true);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns the current contract version.
    pub fn get_version(env: Env) -> u32 {
        Self::get_or_init_version(&env)
    }

    /// Get the proposed admin address, if any.
    pub fn get_proposed_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::ProposedAdmin)
    }

    // --- Read Interface ---

    /// Returns global aggregate statistics (#70).
    pub fn get_stats(env: Env) -> GlobalStats {
        env.storage()
            .persistent()
            .get(&DataKey::GlobalStats)
            .unwrap_or(GlobalStats {
                total_payments_created: 0,
                total_payments_completed: 0,
                total_payments_refunded: 0,
                total_payments_expired: 0,
                total_volume_completed: Map::new(&env),
                total_volume_refunded: Map::new(&env),
            })
    }

    /// Returns per-merchant aggregate statistics (#70).
    pub fn get_merchant_stats(env: Env, merchant: Address) -> MerchantStats {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantStats(merchant))
            .unwrap_or(MerchantStats {
                payments_created: 0,
                payments_completed: 0,
                payments_refunded: 0,
                volume_completed: Map::new(&env),
            })
    }

    /// Returns the completed volume for a token in the current weekly ledger bucket (#70).
    pub fn get_weekly_volume(env: Env, token: Address) -> i128 {
        let bucket = env.ledger().sequence() / LEDGER_BUCKET_SIZE;
        env.storage()
            .persistent()
            .get(&DataKey::VolumeBucket(token, bucket))
            .unwrap_or(0)
    }

    pub fn get_payment(env: Env, payment_id: u32) -> Payment {
        env.storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found")
    }

    // =========================================================================
    // Task 1: External ID — Off-Chain Order Correlation
    // =========================================================================

    /// Look up a payment by merchant + external_id.
    /// Returns the full Payment struct.
    pub fn get_payment_by_external_id(
        env: Env,
        merchant: Address,
        external_id: BytesN<32>,
    ) -> Payment {
        let payment_id: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::ExternalIdIndex(merchant, external_id))
            .expect("No payment found for this external_id");
        env.storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found")
    }

    // =========================================================================
    // Task 2: Multi-Signature Approval for High-Value Payments
    // =========================================================================

    /// Merchant (or admin) configures a multi-sig policy.
    /// `threshold`: minimum payment amount that requires approval.
    /// `signers`: authorized co-signer addresses.
    /// `m`: number of approvals required (must be ≤ signers.len()).
    /// `approval_window_seconds`: time window before unapproved payment auto-cancels.
    pub fn set_multisig_policy(
        env: Env,
        merchant: Address,
        threshold: i128,
        signers: Vec<Address>,
        m: u32,
        approval_window_seconds: u64,
    ) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        if threshold <= 0 {
            panic!("threshold must be positive");
        }
        if signers.is_empty() {
            panic!("signers cannot be empty");
        }
        if m == 0 || m > signers.len() {
            panic!("m must be between 1 and signers.len()");
        }
        if approval_window_seconds == 0 {
            panic!("approval_window_seconds must be positive");
        }

        let policy = MultisigPolicy {
            m,
            signers,
            threshold,
            approval_window_seconds,
        };
        env.storage()
            .instance()
            .set(&DataKey::MultisigPolicy(merchant), &policy);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the multi-sig policy for a merchant.
    pub fn get_multisig_policy(env: Env, merchant: Address) -> Option<MultisigPolicy> {
        env.storage()
            .instance()
            .get(&DataKey::MultisigPolicy(merchant))
    }

    /// A signer approves a PendingApproval payment.
    /// Once `m` approvals are recorded the payment transitions to Pending.
    pub fn approve_payment(env: Env, signer: Address, payment_id: u32) {
        Self::require_not_paused(&env);
        signer.require_auth();

        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.status != PaymentStatus::PendingApproval {
            panic!("Payment is not pending approval");
        }

        let policy: MultisigPolicy = env
            .storage()
            .instance()
            .get(&DataKey::MultisigPolicy(payment.merchant.clone()))
            .expect("No multisig policy for merchant");

        // Verify signer is in the policy set
        let mut is_valid_signer = false;
        for i in 0..policy.signers.len() {
            if policy.signers.get(i).unwrap() == signer {
                is_valid_signer = true;
                break;
            }
        }
        if !is_valid_signer {
            panic_with_error!(&env, Error::NotASigner);
        }

        // Check approval window
        let mut state: ApprovalState = env
            .storage()
            .persistent()
            .get(&DataKey::ApprovalState(payment_id))
            .expect("Approval state not found");

        let now = env.ledger().timestamp();
        if now > state.created_at + policy.approval_window_seconds {
            // Window expired — auto-cancel and refund
            let client = token::Client::new(&env, &payment.token);
            client.transfer(
                &env.current_contract_address(),
                &payment.customer,
                &payment.amount,
            );
            let old_status = payment.status;
            payment.status = PaymentStatus::Refunded;
            env.storage()
                .persistent()
                .set(&DataKey::Payment(payment_id), &payment);
            env.storage().persistent().extend_ttl(
                &DataKey::Payment(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
            events::emit_payment_approval_expired(&env, payment_id);
            events::emit_payment_status_changed(&env, payment_id, old_status, PaymentStatus::Refunded);
            env.storage()
                .instance()
                .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
            return;
        }

        // Check for duplicate approval
        for i in 0..state.approvals.len() {
            if state.approvals.get(i).unwrap() == signer {
                panic_with_error!(&env, Error::AlreadyApproved);
            }
        }

        state.approvals.push_back(signer.clone());
        events::emit_payment_approved(&env, payment_id, signer);

        if state.approvals.len() >= policy.m {
            // Quorum reached — transition to Pending
            let old_status = payment.status;
            payment.status = PaymentStatus::Pending;
            env.storage()
                .persistent()
                .set(&DataKey::Payment(payment_id), &payment);
            env.storage().persistent().extend_ttl(
                &DataKey::Payment(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
            events::emit_payment_status_changed(&env, payment_id, old_status, PaymentStatus::Pending);
        } else {
            env.storage()
                .persistent()
                .set(&DataKey::ApprovalState(payment_id), &state);
            env.storage().persistent().extend_ttl(
                &DataKey::ApprovalState(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Anyone can call this to expire an unapproved payment after the approval window.
    pub fn expire_pending_approval(env: Env, payment_id: u32) {
        Self::require_not_paused(&env);

        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.status != PaymentStatus::PendingApproval {
            panic!("Payment is not pending approval");
        }

        let policy: MultisigPolicy = env
            .storage()
            .instance()
            .get(&DataKey::MultisigPolicy(payment.merchant.clone()))
            .expect("No multisig policy for merchant");

        let state: ApprovalState = env
            .storage()
            .persistent()
            .get(&DataKey::ApprovalState(payment_id))
            .expect("Approval state not found");

        let now = env.ledger().timestamp();
        if now <= state.created_at + policy.approval_window_seconds {
            panic!("Approval window has not expired yet");
        }

        let client = token::Client::new(&env, &payment.token);
        client.transfer(
            &env.current_contract_address(),
            &payment.customer,
            &payment.amount,
        );

        let old_status = payment.status;
        payment.status = PaymentStatus::Refunded;
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_payment_approval_expired(&env, payment_id);
        events::emit_payment_status_changed(&env, payment_id, old_status, PaymentStatus::Refunded);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // =========================================================================
    // Task 3: Voucher / Coupon Code Redemption
    // =========================================================================

    /// Merchant issues a voucher on-chain.
    /// `code_hash`: sha256 hash of the promo code (prevents front-running).
    /// `discount_type`: Fixed or Percentage.
    /// `discount_value`: token units (Fixed) or 0–100 (Percentage).
    /// `max_uses`: maximum redemptions; 0 = unlimited.
    /// `expiry`: ledger timestamp after which voucher is invalid; 0 = no expiry.
    pub fn issue_voucher(
        env: Env,
        merchant: Address,
        code_hash: BytesN<32>,
        discount_type: DiscountType,
        discount_value: u32,
        max_uses: u32,
        expiry: u64,
    ) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        if discount_type == DiscountType::Percentage && discount_value > 100 {
            panic!("Percentage discount cannot exceed 100");
        }
        if discount_value == 0 {
            panic!("discount_value must be positive");
        }

        let key = DataKey::Voucher(merchant.clone(), code_hash.clone());
        if env.storage().persistent().has(&key) {
            panic!("Voucher with this code_hash already exists");
        }

        let voucher = Voucher {
            merchant: merchant.clone(),
            discount_type,
            discount_value,
            max_uses,
            uses_remaining: max_uses,
            expiry,
            revoked: false,
        };

        env.storage().persistent().set(&key, &voucher);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_voucher_issued(&env, merchant, code_hash, discount_type, discount_value, max_uses, expiry);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Merchant revokes a voucher immediately.
    pub fn revoke_voucher(env: Env, merchant: Address, code_hash: BytesN<32>) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        let key = DataKey::Voucher(merchant.clone(), code_hash.clone());
        let mut voucher: Voucher = env
            .storage()
            .persistent()
            .get(&key)
            .expect("Voucher not found");

        if voucher.merchant != merchant {
            panic!("Only the issuing merchant can revoke this voucher");
        }
        if voucher.revoked {
            panic!("Voucher already revoked");
        }

        voucher.revoked = true;
        env.storage().persistent().set(&key, &voucher);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_voucher_revoked(&env, merchant, code_hash);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get voucher details.
    pub fn get_voucher(env: Env, merchant: Address, code_hash: BytesN<32>) -> Voucher {
        env.storage()
            .persistent()
            .get(&DataKey::Voucher(merchant, code_hash))
            .expect("Voucher not found")
    }

    /// Create a payment with an optional voucher code hash for discount.
    /// The discount is applied to the required payment amount.
    pub fn create_payment_with_voucher(
        env: Env,
        customer: Address,
        merchant: Address,
        amount: i128,
        token: Address,
        reference: Option<String>,
        metadata: Option<Map<String, String>>,
        idempotency_key: Option<BytesN<32>>,
        voucher_code_hash: Option<BytesN<32>>,
        external_id: Option<BytesN<32>>,
    ) -> u32 {
        Self::require_not_paused(&env);
        customer.require_auth();

        // Check idempotency key first
        if let Some(ref key) = idempotency_key {
            if let Some(existing_payment_id) = env
                .storage()
                .temporary()
                .get::<DataKey, u32>(&DataKey::IdempotencyKey(key.clone()))
            {
                return existing_payment_id;
            }
        }

        Self::enforce_rate_limit(&env, &customer, 1);

        if amount <= 0 {
            panic!("Payment amount must be positive");
        }

        Self::validate_reference(&env, &reference);
        Self::validate_metadata(&env, &metadata);
        Self::require_token_allowed(&env, &token);
        Self::require_merchant_approved(&env, &merchant);

        // --- External ID uniqueness check ---
        if let Some(ref ext_id) = external_id {
            let idx_key = DataKey::ExternalIdIndex(merchant.clone(), ext_id.clone());
            if env.storage().persistent().has(&idx_key) {
                panic_with_error!(&env, Error::DuplicateExternalId);
            }
        }

        // --- Voucher discount ---
        let effective_amount = if let Some(ref code_hash) = voucher_code_hash {
            let voucher_key = DataKey::Voucher(merchant.clone(), code_hash.clone());
            let mut voucher: Voucher = env
                .storage()
                .persistent()
                .get(&voucher_key)
                .expect("Voucher not found");

            let now = env.ledger().timestamp();
            if voucher.revoked {
                panic_with_error!(&env, Error::VoucherRevoked);
            }
            if voucher.expiry > 0 && now > voucher.expiry {
                panic_with_error!(&env, Error::VoucherExpired);
            }
            if voucher.max_uses > 0 && voucher.uses_remaining == 0 {
                panic_with_error!(&env, Error::VoucherExhausted);
            }

            let discount: i128 = match voucher.discount_type {
                DiscountType::Fixed => voucher.discount_value as i128,
                DiscountType::Percentage => (amount * voucher.discount_value as i128) / 100,
            };
            let discounted = (amount - discount).max(0);

            // Decrement uses
            if voucher.max_uses > 0 {
                voucher.uses_remaining -= 1;
            }
            let exhausted = voucher.max_uses > 0 && voucher.uses_remaining == 0;
            if exhausted {
                voucher.revoked = true; // mark exhausted by setting revoked (uses_remaining=0 is the real signal)
            }
            env.storage().persistent().set(&voucher_key, &voucher);
            env.storage().persistent().extend_ttl(
                &voucher_key,
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );

            events::emit_voucher_redeemed(&env, merchant.clone(), code_hash.clone(), customer.clone(), discount);
            if exhausted {
                events::emit_voucher_exhausted(&env, merchant.clone(), code_hash.clone());
            }

            discounted
        } else {
            amount
        };

        if effective_amount <= 0 {
            panic!("Effective payment amount after discount must be positive");
        }

        let client = token::Client::new(&env, &token);
        client.transfer(&customer, &env.current_contract_address(), &effective_amount);

        let timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PaymentTimeout)
            .unwrap_or(DEFAULT_PAYMENT_TIMEOUT);
        let now = env.ledger().timestamp();

        // --- Multi-sig gate check ---
        let policy_opt: Option<MultisigPolicy> = env
            .storage()
            .instance()
            .get(&DataKey::MultisigPolicy(merchant.clone()));

        let status = if let Some(ref policy) = policy_opt {
            if effective_amount >= policy.threshold {
                PaymentStatus::PendingApproval
            } else {
                PaymentStatus::Pending
            }
        } else {
            PaymentStatus::Pending
        };

        let payment_id = Self::next_payment_id(&env);
        let payment = Payment {
            id: payment_id,
            customer: customer.clone(),
            merchant: merchant.clone(),
            amount: effective_amount,
            token: token.clone(),
            status,
            created_at: now,
            expires_at: now + timeout,
            refunded_amount: 0,
            reference: reference.clone(),
            metadata,
            split_recipients: None,
            execute_after: 0,
            category: None,
            tags: None,
            capture_deadline: 0,
            external_id: external_id.clone(),
        };

        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::add_customer_payment(&env, &customer, payment_id);

        if let Some(ref r) = reference {
            Self::index_payment_by_reference(&env, &merchant, r, payment_id);
        }

        // --- Store external_id index ---
        if let Some(ref ext_id) = external_id {
            let idx_key = DataKey::ExternalIdIndex(merchant.clone(), ext_id.clone());
            env.storage().persistent().set(&idx_key, &payment_id);
            env.storage().persistent().extend_ttl(
                &idx_key,
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
            events::emit_payment_indexed_by_external_id(&env, payment_id, ext_id.clone());
        }

        // --- Store approval state if PendingApproval ---
        if status == PaymentStatus::PendingApproval {
            let state = ApprovalState {
                payment_id,
                approvals: Vec::new(&env),
                created_at: now,
            };
            env.storage()
                .persistent()
                .set(&DataKey::ApprovalState(payment_id), &state);
            env.storage().persistent().extend_ttl(
                &DataKey::ApprovalState(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        if let Some(ref key) = idempotency_key {
            env.storage()
                .temporary()
                .set(&DataKey::IdempotencyKey(key.clone()), &payment_id);
            env.storage().temporary().extend_ttl(
                &DataKey::IdempotencyKey(key.clone()),
                IDEMPOTENCY_KEY_LIFETIME_THRESHOLD,
                IDEMPOTENCY_KEY_BUMP_AMOUNT,
            );
        }

        Self::inc_global_created(&env);
        Self::inc_merchant_created(&env, &merchant);

        events::emit_payment_created(&env, payment_id, customer, merchant, effective_amount, token);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        payment_id
    }

    /// Returns the 32-byte sha256 receipt hash for a completed payment (#65).
    /// Hash inputs (big-endian): payment_id || customer || merchant || amount || token || completed_at
    pub fn get_payment_receipt(env: Env, payment_id: u32) -> BytesN<32> {
        env.storage()
            .persistent()
            .get(&DataKey::PaymentReceipt(payment_id))
            .expect("Receipt not found")
    }

    /// Returns true if the stored receipt hash matches `expected_hash` (#65).
    pub fn verify_payment(env: Env, payment_id: u32, expected_hash: BytesN<32>) -> bool {
        env.storage()
            .persistent()
            .get::<DataKey, BytesN<32>>(&DataKey::PaymentReceipt(payment_id))
            .map(|stored| stored == expected_hash)
            .unwrap_or(false)
    }

    /// Returns the invoice hash for a payment if one was attached (#128).
    pub fn get_invoice_hash(env: Env, payment_id: u32) -> Option<BytesN<32>> {
        env.storage()
            .persistent()
            .get::<DataKey, BytesN<32>>(&DataKey::InvoiceHash(payment_id))
    }

    /// Look up all payment IDs for a merchant+reference pair (#67).
    pub fn get_payments_by_reference(env: Env, merchant: Address, reference: String) -> Vec<u32> {
        let hash = Self::reference_hash(&env, &reference);
        env.storage()
            .persistent()
            .get(&DataKey::MerchantReference(merchant, hash))
            .unwrap_or(Vec::new(&env))
    }

    /// Returns all payment IDs recorded for a customer.
    ///
    /// Backward-compatible single-page read; for unbounded lists use the
    /// paginated form `get_customer_payments_page`.
    pub fn get_customer_payments(env: Env, customer: Address) -> Vec<u32> {
        env.storage()
            .persistent()
            .get(&DataKey::CustomerPayments(customer))
            .unwrap_or(Vec::new(&env))
    }

    /// Paginated read of a customer's payment IDs (#132).
    ///
    /// Returns the requested slice along with `total_count` and `has_more`
    /// so callers can drive UI pagination without an extra round-trip.
    /// `page_size` must be in `1..=MAX_CUSTOMER_PAYMENTS_PAGE_SIZE`.
    pub fn get_customer_payments_page(
        env: Env,
        customer: Address,
        page: u32,
        page_size: u32,
    ) -> CustomerPaymentsPage {
        if page_size == 0 {
            panic!("page_size must be greater than 0");
        }
        if page_size > MAX_CUSTOMER_PAYMENTS_PAGE_SIZE {
            panic!("page_size exceeds maximum of 50");
        }

        let all: Vec<u32> = env
            .storage()
            .persistent()
            .get(&DataKey::CustomerPayments(customer))
            .unwrap_or(Vec::new(&env));

        let total_count = all.len();
        let start = page.saturating_mul(page_size);
        let end = start
            .saturating_add(page_size)
            .min(total_count);

        let mut slice = Vec::new(&env);
        if start < end {
            for i in start..end {
                slice.push_back(all.get(i).unwrap());
            }
        }

        CustomerPaymentsPage {
            payments: slice,
            total_count,
            has_more: end < total_count,
        }
    }

    /// Returns the total number of payment IDs recorded for a customer (#132).
    pub fn get_customer_payment_count(env: Env, customer: Address) -> u32 {
        env.storage()
            .persistent()
            .get::<DataKey, Vec<u32>>(&DataKey::CustomerPayments(customer))
            .map(|v| v.len())
            .unwrap_or(0)
    }

    pub fn get_payment_counter(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::PaymentCounter)
            .unwrap_or(0)
    }

    pub fn get_max_batch_size(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MaxBatchSize)
            .unwrap_or(DEFAULT_MAX_BATCH_SIZE)
    }

    pub fn is_disputed(env: Env, payment_id: u32) -> bool {
        let payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");
        payment.status == PaymentStatus::Disputed
    }

    pub fn get_dispute(env: Env, payment_id: u32) -> Dispute {
        env.storage()
            .temporary()
            .get(&DataKey::Dispute(payment_id))
            .expect("No dispute found for this payment")
    }

    pub fn get_dispute_timeout(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::DisputeTimeout)
            .unwrap_or(DEFAULT_DISPUTE_TIMEOUT)
    }

    pub fn get_rate_limit_config(env: Env) -> RateLimitConfig {
        Self::get_rate_limit_config_internal(&env)
    }

    pub fn is_settled(env: Env, payment_id: u32) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Settled(payment_id))
            .unwrap_or(false)
    }

    // --- Payment Expiry (#54) ---

    /// Admin sets the global payment timeout in seconds.
    pub fn set_payment_timeout(env: Env, timeout_seconds: u64) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();
        if timeout_seconds == 0 {
            panic!("Timeout must be positive");
        }
        env.storage()
            .instance()
            .set(&DataKey::PaymentTimeout, &timeout_seconds);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_payment_timeout(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::PaymentTimeout)
            .unwrap_or(DEFAULT_PAYMENT_TIMEOUT)
    }

    // --- Per-payment expiry bounds (#130) ---

    /// Admin sets the inclusive `[min, max]` bounds for merchant-defined
    /// per-payment expiry overrides (#130). Both values are in seconds.
    pub fn set_payment_expiry_bounds(env: Env, min_seconds: u64, max_seconds: u64) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();
        if min_seconds == 0 {
            panic!("min_expiry must be greater than 0");
        }
        if min_seconds > max_seconds {
            panic!("min_expiry must be <= max_expiry");
        }
        env.storage()
            .instance()
            .set(&DataKey::MinPaymentExpiry, &min_seconds);
        env.storage()
            .instance()
            .set(&DataKey::MaxPaymentExpiry, &max_seconds);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_min_payment_expiry(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MinPaymentExpiry)
            .unwrap_or(DEFAULT_MIN_PAYMENT_EXPIRY)
    }

    pub fn get_max_payment_expiry(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MaxPaymentExpiry)
            .unwrap_or(DEFAULT_MAX_PAYMENT_EXPIRY)
    }

    fn require_expiry_within_bounds(env: &Env, expiry_seconds: u64) {
        let min_expiry: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MinPaymentExpiry)
            .unwrap_or(DEFAULT_MIN_PAYMENT_EXPIRY);
        let max_expiry: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MaxPaymentExpiry)
            .unwrap_or(DEFAULT_MAX_PAYMENT_EXPIRY);
        if expiry_seconds < min_expiry {
            panic!("expiry_seconds is below the configured minimum");
        }
        if expiry_seconds > max_expiry {
            panic!("expiry_seconds exceeds the configured maximum");
        }
    }

    /// Expire a pending or authorized payment after its deadline. Callable by anyone.
    /// Returns funds to the customer and emits PaymentExpired event.
    pub fn expire_payment(env: Env, payment_id: u32) {
        Self::require_not_paused(&env);
        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.status != PaymentStatus::Pending && payment.status != PaymentStatus::Authorized {
            panic!("Only pending or authorized payments can expire");
        }

        let now = env.ledger().timestamp();
        let expired = match payment.status {
            PaymentStatus::Pending => {
                if payment.expires_at == 0 {
                    panic!("Payment has no expiry set");
                }
                now >= payment.expires_at
            }
            PaymentStatus::Authorized => now >= payment.capture_deadline,
            _ => false,
        };

        if !expired {
            panic!("Payment has not expired yet");
        }

        let client = token::Client::new(&env, &payment.token);
        client.transfer(
            &env.current_contract_address(),
            &payment.customer,
            &payment.amount,
        );

        let old_status = payment.status;
        payment.status = PaymentStatus::Expired;
        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Update stats (#70)
        Self::inc_global_expired(&env);

        events::emit_payment_expired(
            &env,
            payment_id,
            payment.customer.clone(),
            payment.amount,
            env.ledger().timestamp(),
        );
        events::emit_payment_status_changed(&env, payment_id, old_status, PaymentStatus::Expired);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- Partial Refund (#55) ---

    /// Process a partial refund on a disputed payment. Admin only.
    /// `refund_amount` must be <= (payment.amount - payment.refunded_amount).
    pub fn partial_refund(env: Env, payment_id: u32, refund_amount: i128) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        if refund_amount <= 0 {
            panic!("Refund amount must be positive");
        }

        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.status != PaymentStatus::Disputed && payment.status != PaymentStatus::Pending {
            panic!("Payment must be pending or disputed for partial refund");
        }

        let remaining = payment.amount - payment.refunded_amount;
        if refund_amount > remaining {
            panic!("Refund amount exceeds remaining balance");
        }

        let client = token::Client::new(&env, &payment.token);
        client.transfer(
            &env.current_contract_address(),
            &payment.customer,
            &refund_amount,
        );

        payment.refunded_amount += refund_amount;

        // If fully refunded, mark as Refunded
        if payment.refunded_amount >= payment.amount {
            payment.status = PaymentStatus::Refunded;
        }

        // Update stats (#70) — count each partial refund call
        Self::inc_global_refunded(&env, &payment.token, refund_amount);
        Self::inc_merchant_refunded(&env, &payment.merchant, &payment.token, refund_amount);

        let remaining = payment.amount - payment.refunded_amount;
        events::emit_payment_partial_refund(
            &env,
            payment_id,
            payment.customer.clone(),
            refund_amount,
            remaining,
        );

        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- Merchant Allowlist (#58) ---

    /// Admin approves a merchant address.
    /// Requires the merchant to have deposited at least the minimum collateral (#129).
    pub fn approve_merchant(env: Env, merchant: Address) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        // Enforce minimum collateral before approval (#129)
        let min_collateral = Self::get_min_collateral_internal(&env);
        let collateral: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantCollateral(merchant.clone()))
            .unwrap_or(0);
        if collateral < min_collateral {
            panic!("Merchant collateral below minimum required");
        }

        env.storage()
            .persistent()
            .set(&DataKey::MerchantApproved(merchant), &true);
    }

    /// Admin revokes a merchant address.
    pub fn revoke_merchant(env: Env, merchant: Address) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::MerchantApproved(merchant), &false);
    }

    /// Check if a merchant is approved.
    pub fn is_merchant_approved(env: Env, merchant: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantApproved(merchant))
            .unwrap_or(false)
    }

    /// Admin toggles open mode (bypasses merchant allowlist).
    pub fn set_merchant_open_mode(env: Env, open: bool) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();
        env.storage()
            .instance()
            .set(&DataKey::MerchantOpenMode, &open);
    }

    pub fn is_merchant_open_mode(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::MerchantOpenMode)
            .unwrap_or(true)
    }

    // --- Merchant Collateral (#129) ---

    /// Admin sets the minimum collateral required for merchant approval.
    pub fn set_min_collateral(env: Env, min_collateral: i128) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        if min_collateral < 0 {
            panic!("min_collateral cannot be negative");
        }

        env.storage()
            .instance()
            .set(&DataKey::MinCollateral, &min_collateral);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns the current minimum collateral threshold.
    pub fn get_min_collateral(env: Env) -> i128 {
        Self::get_min_collateral_internal(&env)
    }

    /// Merchant deposits collateral into the contract.
    /// The collateral token is the configured USDC token.
    /// Emits CollateralDeposited event.
    pub fn deposit_collateral(env: Env, merchant: Address, amount: i128) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        if amount <= 0 {
            panic!("Deposit amount must be positive");
        }

        let usdc_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::UsdcToken)
            .expect("Collateral token not configured; call set_oracle first");

        Self::require_token_allowed(&env, &usdc_token);

        let token_client = token::Client::new(&env, &usdc_token);
        token_client.transfer(&merchant, &env.current_contract_address(), &amount);

        let key = DataKey::MerchantCollateral(merchant.clone());
        let prev: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        let new_balance = prev.checked_add(amount).expect("Collateral overflow");
        env.storage().persistent().set(&key, &new_balance);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_collateral_deposited(&env, merchant, amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Merchant withdraws collateral from the contract.
    /// Withdrawal is blocked if it would drop the balance below the minimum.
    /// Emits CollateralWithdrawn event.
    pub fn withdraw_collateral(env: Env, merchant: Address, amount: i128) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        if amount <= 0 {
            panic!("Withdrawal amount must be positive");
        }

        let key = DataKey::MerchantCollateral(merchant.clone());
        let current: i128 = env.storage().persistent().get(&key).unwrap_or(0);

        if amount > current {
            panic!("Insufficient collateral balance");
        }

        let min_collateral = Self::get_min_collateral_internal(&env);
        let remaining = current - amount;
        if remaining < min_collateral {
            panic!("Withdrawal would drop collateral below minimum required");
        }

        let usdc_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::UsdcToken)
            .expect("Collateral token not configured");

        let token_client = token::Client::new(&env, &usdc_token);
        token_client.transfer(&env.current_contract_address(), &merchant, &amount);

        env.storage().persistent().set(&key, &remaining);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_collateral_withdrawn(&env, merchant, amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns the current collateral balance for a merchant.
    pub fn get_collateral_balance(env: Env, merchant: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantCollateral(merchant))
            .unwrap_or(0)
    }

    // --- Notification Keys ---

    /// Merchant registers a notification key for event routing.
    /// The key is included in all payment-related events for this merchant.
    /// Key size is bounded to prevent storage abuse (max 128 bytes).
    pub fn register_notification_key(env: Env, merchant: Address, key: Bytes) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        if key.len() > MAX_NOTIFICATION_KEY_LEN {
            panic!("Notification key exceeds maximum length of 128 bytes");
        }

        if key.is_empty() {
            panic!("Notification key cannot be empty");
        }

        let storage_key = DataKey::MerchantNotificationKey(merchant.clone());
        env.storage().persistent().set(&storage_key, &key);
        env.storage().persistent().extend_ttl(
            &storage_key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_notification_key_registered(&env, merchant, key);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Merchant removes their notification key.
    /// After removal, events will include empty bytes for the notification key.
    pub fn remove_notification_key(env: Env, merchant: Address) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        let storage_key = DataKey::MerchantNotificationKey(merchant.clone());
        env.storage().persistent().remove(&storage_key);

        events::emit_notification_key_removed(&env, merchant);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the notification key for a merchant, if registered.
    pub fn get_notification_key(env: Env, merchant: Address) -> Option<Bytes> {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantNotificationKey(merchant))
    }

    // --- Token Swap Functions ---

    /// Merchant sets their preferred token for receiving payments.
    /// If a customer pays in a different token, the contract will attempt to swap.
    pub fn set_preferred_token(env: Env, merchant: Address, token: Address) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        // Validate token is allowed
        Self::require_token_allowed(&env, &token);

        env.storage()
            .instance()
            .set(&DataKey::PreferredToken(merchant.clone()), &token);

        events::emit_preferred_token_set(&env, merchant, token);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the merchant's preferred token.
    pub fn get_preferred_token(env: Env, merchant: Address) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::PreferredToken(merchant))
    }

    /// Admin sets the DEX router contract address for token swaps.
    pub fn set_swap_router(env: Env, admin: Address, router: Address) {
        Self::require_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can set swap router");
        }

        env.storage()
            .instance()
            .set(&DataKey::SwapRouter, &router);

        events::emit_swap_router_set(&env, router);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the configured swap router address.
    pub fn get_swap_router(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::SwapRouter)
    }

    // --- Token Whitelist Integration ---

    /// Set the token whitelist contract address (admin only)
    pub fn set_token_whitelist_contract(env: Env, admin: Address, whitelist_contract: Address) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can set token whitelist contract");
        }

        env.storage()
            .instance()
            .set(&DataKey::TokenWhitelistContract, &whitelist_contract);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the token whitelist contract address
    pub fn get_token_whitelist_contract(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::TokenWhitelistContract)
    }

    /// Check if a token is allowed via the whitelist contract
    pub fn is_token_allowed(env: Env, token: Address) -> bool {
        if let Some(whitelist_contract) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::TokenWhitelistContract)
        {
            let client = TokenWhitelistClient::new(&env, &whitelist_contract);
            client.is_token_allowed(&token)
        } else {
            // If no whitelist contract is set, allow all tokens (backward compatibility)
            true
        }
    }

    // --- Subscriptions (#60) ---

    /// Subscriber creates a recurring payment. Signs once to authorize future charges.
    pub fn create_subscription(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        token: Address,
        interval_seconds: u64,
        max_charges: u32,
    ) -> u32 {
        Self::create_subscription_with_trial(
            env,
            subscriber,
            merchant,
            amount,
            token,
            interval_seconds,
            max_charges,
            None,
        )
    }

    /// Subscriber creates a recurring payment with an optional trial period (#133).
    ///
    /// When `trial_period_seconds` is `Some(n)` (n > 0), the first charge is
    /// deferred until `created_at + n`. Charging during the trial returns
    /// `Error::SubscriptionInTrial`. A trial of `0` or `None` behaves like the
    /// historical `create_subscription` (immediate first charge available).
    #[allow(clippy::too_many_arguments)]
    pub fn create_subscription_with_trial(
        env: Env,
        subscriber: Address,
        merchant: Address,
        amount: i128,
        token: Address,
        interval_seconds: u64,
        max_charges: u32,
        trial_period_seconds: Option<u64>,
    ) -> u32 {
        Self::require_not_paused(&env);
        subscriber.require_auth();
        if amount <= 0 {
            panic!("Subscription amount must be positive");
        }
        if interval_seconds == 0 {
            panic!("Interval must be positive");
        }

        Self::require_token_allowed(&env, &token);
        Self::require_merchant_approved(&env, &merchant);

        let mut counter: u32 = env
            .storage()
            .instance()
            .get(&DataKey::SubscriptionCounter)
            .unwrap_or(0);
        let sub_id = counter;
        counter += 1;
        env.storage()
            .instance()
            .set(&DataKey::SubscriptionCounter, &counter);

        let now = env.ledger().timestamp();
        let trial_seconds = trial_period_seconds.unwrap_or(0);
        let trial_ends_at = if trial_seconds > 0 { now + trial_seconds } else { 0 };

        let sub = Subscription {
            id: sub_id,
            subscriber: subscriber.clone(),
            merchant: merchant.clone(),
            amount,
            token,
            interval_seconds,
            last_charged_at: 0,
            max_charges,
            charges_count: 0,
            active: true,
            paused: false,
            paused_at: 0,
            trial_ends_at,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Subscription(sub_id), &sub);
        env.storage().persistent().extend_ttl(
            &DataKey::Subscription(sub_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        if trial_ends_at > 0 {
            events::emit_subscription_trial_started(&env, sub_id, trial_ends_at);
        }
        sub_id
    }

    /// Returns the remaining seconds in the subscription's trial period (#133).
    /// Returns 0 if there is no trial or the trial has already ended.
    pub fn get_trial_remaining(env: Env, subscription_id: u32) -> u64 {
        let sub: Subscription = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(subscription_id))
            .expect("Subscription not found");
        let now = env.ledger().timestamp();
        if sub.trial_ends_at == 0 || sub.trial_ends_at <= now {
            0
        } else {
            sub.trial_ends_at - now
        }
    }

    /// Charge a subscription. Callable by anyone when the interval has elapsed.
    pub fn charge_subscription(env: Env, subscription_id: u32) {
        Self::require_not_paused(&env);
        let mut sub: Subscription = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(subscription_id))
            .expect("Subscription not found");

        if !sub.active {
            panic!("Subscription is cancelled");
        }
        if sub.paused {
            panic_with_error!(&env, Error::SubscriptionPaused);
        }
        if sub.max_charges > 0 && sub.charges_count >= sub.max_charges {
            panic!("Max charges reached");
        }

        let now = env.ledger().timestamp();
        if sub.charges_count == 0 && sub.trial_ends_at > now {
            panic_with_error!(&env, Error::SubscriptionInTrial);
        }
        if sub.last_charged_at > 0 && now < sub.last_charged_at + sub.interval_seconds {
            panic!("Interval has not elapsed");
        }
        let trial_just_ended = sub.charges_count == 0 && sub.trial_ends_at > 0;

        let client = token::Client::new(&env, &sub.token);
        client.transfer(
            &sub.subscriber,
            &env.current_contract_address(),
            &sub.amount,
        );
        client.transfer(&env.current_contract_address(), &sub.merchant, &sub.amount);

        sub.last_charged_at = now;
        sub.charges_count += 1;

        env.storage()
            .persistent()
            .set(&DataKey::Subscription(subscription_id), &sub);
        env.storage().persistent().extend_ttl(
            &DataKey::Subscription(subscription_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_subscription_charged(
            &env,
            subscription_id,
            sub.subscriber,
            sub.merchant,
            sub.amount,
            now,
        );
        if trial_just_ended {
            events::emit_subscription_trial_ended(&env, subscription_id);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Cancel a subscription. Subscriber or merchant can cancel.
    pub fn cancel_subscription(env: Env, caller: Address, subscription_id: u32) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut sub: Subscription = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(subscription_id))
            .expect("Subscription not found");

        if caller != sub.subscriber && caller != sub.merchant {
            panic!("Only subscriber or merchant can cancel");
        }

        sub.active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Subscription(subscription_id), &sub);
        env.storage().persistent().extend_ttl(
            &DataKey::Subscription(subscription_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_subscription_cancelled(&env, subscription_id, caller);
    }

    /// Read a subscription.
    pub fn get_subscription(env: Env, subscription_id: u32) -> Subscription {
        env.storage()
            .persistent()
            .get(&DataKey::Subscription(subscription_id))
            .expect("Subscription not found")
    }

    // --- Subscription Pause / Resume (#124) ---

    /// Subscriber temporarily pauses their subscription.
    /// Charging is blocked while paused. Only the subscriber can pause.
    pub fn pause_subscription(env: Env, subscriber: Address, sub_id: u32) {
        Self::require_not_paused(&env);
        subscriber.require_auth();

        let mut sub: Subscription = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(sub_id))
            .expect("Subscription not found");

        if sub.subscriber != subscriber {
            panic!("Only the subscriber can pause");
        }
        if !sub.active {
            panic!("Subscription is cancelled");
        }
        if sub.paused {
            panic!("Subscription already paused");
        }

        let now = env.ledger().timestamp();
        sub.paused = true;
        sub.paused_at = now;

        env.storage()
            .persistent()
            .set(&DataKey::Subscription(sub_id), &sub);
        env.storage().persistent().extend_ttl(
            &DataKey::Subscription(sub_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_subscription_paused(&env, sub_id, now);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Subscriber resumes a paused subscription.
    /// The next charge interval restarts from the resume timestamp,
    /// so paused time does not count toward the interval.
    pub fn resume_subscription(env: Env, subscriber: Address, sub_id: u32) {
        Self::require_not_paused(&env);
        subscriber.require_auth();

        let mut sub: Subscription = env
            .storage()
            .persistent()
            .get(&DataKey::Subscription(sub_id))
            .expect("Subscription not found");

        if sub.subscriber != subscriber {
            panic!("Only the subscriber can resume");
        }
        if !sub.active {
            panic!("Subscription is cancelled");
        }
        if !sub.paused {
            panic!("Subscription is not paused");
        }

        let now = env.ledger().timestamp();
        sub.paused = false;
        sub.paused_at = 0;
        // Reset last_charged_at so the next interval starts from now,
        // ensuring paused duration does not count.
        sub.last_charged_at = now;

        env.storage()
            .persistent()
            .set(&DataKey::Subscription(sub_id), &sub);
        env.storage().persistent().extend_ttl(
            &DataKey::Subscription(sub_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_subscription_resumed(&env, sub_id, now);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- Payment Categories (#122) ---

    /*
    /// Create a payment with optional category, tags, and release condition.
    /// Category and tags enable on-chain segmentation and analytics.
    /// release_condition, if set, must be satisfied at completion time (#125).
    #[allow(clippy::too_many_arguments)]
    pub fn create_payment_with_extras(
        env: Env,
        customer: Address,
        merchant: Address,
        amount: i128,
        token: Address,
        category: Option<Symbol>,
        tags: Option<Vec<Symbol>>,
        release_condition: Option<OracleCondition>,
    ) -> u32 {
        Self::require_not_paused(&env);
        customer.require_auth();

        if amount <= 0 {
            panic!("Payment amount must be positive");
        }
        if let Some(ref t) = tags {
            if t.len() > MAX_TAGS {
                panic!("Tags list cannot exceed 3 items");
            }
        }

        Self::require_merchant_approved(&env, &merchant);
        Self::enforce_rate_limit(&env, &customer, 1);

        let client = token::Client::new(&env, &token);
        client.transfer(&customer, &env.current_contract_address(), &amount);

        let timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PaymentTimeout)
            .unwrap_or(DEFAULT_PAYMENT_TIMEOUT);
        let now = env.ledger().timestamp();

        let payment_id = Self::next_payment_id(&env);
        let payment = Payment {
            id: payment_id,
            customer: customer.clone(),
            merchant: merchant.clone(),
            amount,
            token: token.clone(),
            status: PaymentStatus::Pending,
            created_at: now,
            expires_at: now + timeout,
            refunded_amount: 0,
            reference: None,
            metadata: None,
            split_recipients: None,
            execute_after: 0,
            category: category.clone(),
            tags: tags.clone(),
            capture_deadline: 0,
            release_condition,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::add_customer_payment(&env, &customer, payment_id);

        // Index by category for analytics retrieval
        if let Some(ref cat) = category {
            let cat_key = DataKey::CategoryPayments(merchant.clone(), cat.clone());
            let mut cat_ids: Vec<u32> = env
                .storage()
                .persistent()
                .get(&cat_key)
                .unwrap_or(Vec::new(&env));
            cat_ids.push_back(payment_id);
            env.storage().persistent().set(&cat_key, &cat_ids);
            env.storage().persistent().extend_ttl(
                &cat_key,
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
            events::emit_payment_categorized(
                &env,
                payment_id,
                merchant.clone(),
                cat.clone(),
                tags.clone().unwrap_or(Vec::new(&env)),
            );
        }

        Self::inc_global_created(&env);
        Self::inc_merchant_created(&env, &merchant);
        events::emit_payment_created(&env, payment_id, customer, merchant, amount, token);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        payment_id
    }
    */
    /// Return payment IDs for a merchant + category pair, paginated.
    /// page is 0-indexed. Empty result when page exceeds available data.
    pub fn get_payments_by_category(
        env: Env,
        merchant: Address,
        category: Symbol,
        page: u32,
        page_size: u32,
    ) -> Vec<u32> {
        if page_size == 0 {
            panic!("page_size must be positive");
        }
        let all: Vec<u32> = env
            .storage()
            .persistent()
            .get(&DataKey::CategoryPayments(merchant, category))
            .unwrap_or(Vec::new(&env));
        let total = all.len();
        let start = page * page_size;
        if start >= total {
            return Vec::new(&env);
        }
        let end = (start + page_size).min(total);
        let mut result = Vec::new(&env);
        for i in start..end {
            result.push_back(all.get(i).unwrap());
        }
        result
    }

    /// Expire a batch of payments atomically. Admin only.
    /// Each payment must be Pending, Disputed, or Authorized and past its expiry/capture deadline.
    /// If any payment is ineligible the entire batch reverts.
    /// Batch size capped at MAX_SETTLEMENT_BATCH_SIZE (50).
    pub fn bulk_expire_payments(env: Env, admin: Address, payment_ids: Vec<u32>) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can bulk expire payments");
        }

        let batch_size = payment_ids.len();
        if batch_size == 0 {
            panic!("Batch cannot be empty");
        }
        if batch_size > MAX_SETTLEMENT_BATCH_SIZE {
            panic!("Batch size exceeds maximum allowed");
        }

        let now = env.ledger().timestamp();

        // Pass 1: validate all — any failure reverts the entire batch atomically
        for payment_id in payment_ids.iter() {
            let payment: Payment = env
                .storage()
                .persistent()
                .get(&DataKey::Payment(payment_id))
                .expect("Payment not found");

            if payment.status != PaymentStatus::Pending
                && payment.status != PaymentStatus::Disputed
                && payment.status != PaymentStatus::Authorized
            {
                panic!("Payment is not in Pending, Disputed, or Authorized status");
            }
            let deadline = if payment.status == PaymentStatus::Authorized {
                payment.capture_deadline
            } else {
                payment.expires_at
            };
            if deadline == 0 || now < deadline {
                panic!("Payment has not expired yet");
            }
        }

        // Pass 2: process all refunds
        let mut refund_total: i128 = 0;
        for payment_id in payment_ids.iter() {
            let mut payment: Payment = env
                .storage()
                .persistent()
                .get(&DataKey::Payment(payment_id))
                .expect("Payment not found");

            let refund_amount = payment.amount - payment.refunded_amount;
            if refund_amount > 0 {
                let client = token::Client::new(&env, &payment.token);
                client.transfer(
                    &env.current_contract_address(),
                    &payment.customer,
                    &refund_amount,
                );
                refund_total = refund_total
                    .checked_add(refund_amount)
                    .expect("Refund total overflow");
            }

            let old_status = payment.status;
            payment.status = PaymentStatus::Expired;
            env.storage()
                .persistent()
                .set(&DataKey::Payment(payment_id), &payment);
            env.storage().persistent().extend_ttl(
                &DataKey::Payment(payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );

            Self::inc_global_expired(&env);
            events::emit_payment_expired(
                &env,
                payment_id,
                payment.customer.clone(),
                refund_amount,
                now,
            );
            events::emit_payment_status_changed(
                &env,
                payment_id,
                old_status,
                PaymentStatus::Expired,
            );
        }

        events::emit_bulk_expire_completed(&env, batch_size, refund_total);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn pause_contract(env: Env, admin: Address, reason: String) {
        Self::require_admin(&env, &admin);

        if Self::is_paused(env.clone()) {
            panic!("Contract already paused");
        }

        env.storage().instance().set(&DataKey::Paused, &true);
        env.storage().instance().set(&DataKey::PauseReason, &reason);

        events::emit_contract_paused(&env, admin, reason, env.ledger().timestamp());
    }

    pub fn resume_contract(env: Env, admin: Address) {
        Self::require_admin(&env, &admin);

        if !Self::is_paused(env.clone()) {
            panic!("Contract is not paused");
        }

        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().remove(&DataKey::PauseReason);

        events::emit_contract_resumed(&env, admin, env.ledger().timestamp());
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    pub fn get_pause_reason(env: Env) -> String {
        env.storage()
            .instance()
            .get(&DataKey::PauseReason)
            .unwrap_or(String::from_str(&env, ""))
    }

    // --- Merchant Withdrawal Queue (#126) ---

    /// Process up to max_count entries from the merchant's withdrawal queue.
    /// Transfers funds from contract to merchant in FIFO order.
    /// Merchant must authorize the call.
    /// Returns the number of payments processed.
    pub fn process_withdrawal_queue(env: Env, merchant: Address, max_count: u32) -> u32 {
        Self::require_not_paused(&env);
        merchant.require_auth();

        let queue = Self::get_withdrawal_queue(&env, &merchant);
        if queue.is_empty() {
            return 0;
        }

        let max_count = if max_count == 0 {
            DEFAULT_MAX_BATCH_SIZE
        } else {
            max_count
        };
        let process_count = max_count.min(queue.len() as u32) as usize;

        let mut processed_count = 0u32;
        let mut remaining_queue = Vec::new(&env);

        // Process the first process_count entries
        for i in 0..process_count {
            let (payment_id, amount) = queue.get(i as u32).unwrap();
            processed_count += 1;

            let payment: Payment = env
                .storage()
                .persistent()
                .get(&DataKey::Payment(payment_id))
                .expect("Payment not found in queue");
            if payment.status != PaymentStatus::Completed {
                panic!("Payment in queue is not completed");
            }

            let token_client = token::Client::new(&env, &payment.token);
            token_client.transfer(&env.current_contract_address(), &merchant, &amount);
        }

        // Remove processed entries from queue
        for i in process_count..(queue.len() as usize) {
            remaining_queue.push_back(queue.get(i as u32).unwrap());
        }

        Self::set_withdrawal_queue(&env, &merchant, remaining_queue);
        events::emit_withdrawal_processed(&env, merchant, processed_count);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        processed_count
    }

    /// Move a specific payment to the front of the merchant's withdrawal queue.
    /// Merchant must authorize the call.
    pub fn prioritize_withdrawal(env: Env, merchant: Address, payment_id: u32) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        let queue = Self::get_withdrawal_queue(&env, &merchant);
        if queue.is_empty() {
            panic!("Withdrawal queue is empty");
        }

        // Find the payment in the queue
        let mut found_index = None;
        let mut found_amount = 0i128;
        for i in 0..queue.len() {
            let (pid, amount) = queue.get(i).unwrap();
            if pid == payment_id {
                found_index = Some(i);
                found_amount = amount;
                break;
            }
        }

        if found_index.is_none() {
            panic!("Payment not found in withdrawal queue");
        }

        let index = found_index.unwrap();

        // If already at front, do nothing
        if index == 0 {
            return;
        }

        // Remove from current position and insert at front
        let mut new_queue = Vec::new(&env);
        new_queue.push_back((payment_id, found_amount));

        for i in 0..queue.len() {
            if i != index {
                new_queue.push_back(queue.get(i).unwrap());
            }
        }

        Self::set_withdrawal_queue(&env, &merchant, new_queue);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the merchant's current withdrawal queue.
    pub fn get_merchant_withdrawal_queue(env: Env, merchant: Address) -> Vec<(u32, i128)> {
        Self::get_withdrawal_queue(&env, &merchant)
    }

    // --- Internal Helpers ---

    fn require_not_paused(env: &Env) {
        if env
            .storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
        {
            panic!("Contract is paused");
        }
    }

    // -------------------------------------------------------------------------
    // #231: Merchant Withdrawal Rate Limiting
    // -------------------------------------------------------------------------

    /// Admin sets global default withdrawal window and cap.
    pub fn set_withdrawal_window(env: Env, admin: Address, window_seconds: u64, cap: i128) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can set withdrawal window");
        }
        if window_seconds == 0 {
            panic!("Window must be positive");
        }
        if cap <= 0 {
            panic!("Cap must be positive");
        }
        env.storage()
            .instance()
            .set(&DataKey::WithdrawalWindowSeconds, &window_seconds);
        env.storage()
            .instance()
            .set(&DataKey::WithdrawalWindowCap, &cap);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Merchant sets a personal (stricter) withdrawal limit.
    pub fn set_withdrawal_limit(env: Env, merchant: Address, window_seconds: u64, cap: i128) {
        Self::require_not_paused(&env);
        merchant.require_auth();
        if window_seconds == 0 {
            panic!("Window must be positive");
        }
        if cap <= 0 {
            panic!("Cap must be positive");
        }
        let limit = WithdrawalLimit { window_seconds, cap };
        env.storage()
            .persistent()
            .set(&DataKey::MerchantWithdrawalLimit(merchant.clone()), &limit);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantWithdrawalLimit(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_withdrawal_rate_limit_set(&env, merchant, window_seconds, cap);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin overrides a merchant's withdrawal cap (emergency override).
    pub fn override_withdrawal_limit(env: Env, admin: Address, merchant: Address, cap: i128) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can override withdrawal limit");
        }
        if cap <= 0 {
            panic!("Cap must be positive");
        }
        // Preserve existing window or use global default
        let window_seconds: u64 = env
            .storage()
            .persistent()
            .get::<DataKey, WithdrawalLimit>(&DataKey::MerchantWithdrawalLimit(merchant.clone()))
            .map(|l| l.window_seconds)
            .unwrap_or_else(|| {
                env.storage()
                    .instance()
                    .get(&DataKey::WithdrawalWindowSeconds)
                    .unwrap_or(24 * 60 * 60)
            });
        let limit = WithdrawalLimit { window_seconds, cap };
        env.storage()
            .persistent()
            .set(&DataKey::MerchantWithdrawalLimit(merchant.clone()), &limit);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantWithdrawalLimit(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_withdrawal_rate_limit_set(&env, merchant, window_seconds, cap);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Internal: check and update the rolling withdrawal window for a merchant.
    /// Panics with WithdrawalRateLimitExceeded if the cap would be exceeded.
    fn check_and_update_withdrawal_rate_limit(env: &Env, merchant: &Address, amount: i128) {
        // Determine effective limit: merchant-specific > global default > no limit
        let (window_seconds, cap) = if let Some(limit) = env
            .storage()
            .persistent()
            .get::<DataKey, WithdrawalLimit>(&DataKey::MerchantWithdrawalLimit(merchant.clone()))
        {
            (limit.window_seconds, limit.cap)
        } else if let (Some(w), Some(c)) = (
            env.storage()
                .instance()
                .get::<DataKey, u64>(&DataKey::WithdrawalWindowSeconds),
            env.storage()
                .instance()
                .get::<DataKey, i128>(&DataKey::WithdrawalWindowCap),
        ) {
            (w, c)
        } else {
            // No rate limit configured — allow
            return;
        };

        let now = env.ledger().timestamp();
        let state_key = DataKey::MerchantWithdrawalWindow(merchant.clone());

        let mut state: WithdrawalWindowState = env
            .storage()
            .persistent()
            .get(&state_key)
            .unwrap_or(WithdrawalWindowState {
                window_start: now,
                withdrawn: 0,
            });

        // Reset window if it has elapsed
        if now >= state.window_start + window_seconds {
            state.window_start = now;
            state.withdrawn = 0;
        }

        let new_total = state.withdrawn.checked_add(amount).expect("Overflow");
        if new_total > cap {
            events::emit_withdrawal_rate_limit_exceeded(env, merchant.clone(), new_total, cap);
            panic_with_error!(env, Error::WithdrawalRateLimitExceeded);
        }

        state.withdrawn = new_total;
        env.storage().persistent().set(&state_key, &state);
        env.storage().persistent().extend_ttl(
            &state_key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn require_admin(env: &Env, admin: &Address) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if stored_admin != *admin {
            panic!("Only admin can manage pause state");
        }
    }

    /// Validates that a token is allowed via the whitelist contract
    fn require_token_allowed(env: &Env, token: &Address) {
        if let Some(whitelist_contract) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::TokenWhitelistContract)
        {
            let client = TokenWhitelistClient::new(env, &whitelist_contract);
            if !client.is_token_allowed(token) {
                panic_with_error!(env, Error::TokenNotAllowed);
            }
        }
        // If no whitelist contract is set, allow all tokens (backward compatibility)
    }

    fn complete_payment_internal(env: &Env, payment_id: u32, scheduled_only: bool) {
        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if scheduled_only {
            if payment.status != PaymentStatus::ScheduledPending {
                panic!("Payment is not scheduled");
            }
            if env.ledger().timestamp() < payment.execute_after {
                panic!("Payment cannot execute before schedule");
            }
        } else if payment.status != PaymentStatus::Pending {
            panic!("Payment is not pending");
        }

        if payment.expires_at > 0 && env.ledger().timestamp() >= payment.expires_at {
            panic!("Payment has expired");
        }

        // #246: Recompute token amount at current oracle rate for dynamic payments
        Self::settle_dynamic_payment_if_needed(env, &mut payment);
        // #235: Check customer spend limit before finalizing
        Self::check_and_update_customer_spend_limit(env, &payment.merchant, &payment.customer, payment.amount);

        Self::finalize_payment(env, payment_id, &mut payment);
    }

    /// Merchant authorizes a pending payment, converting it to Authorized status.
    /// Funds remain in escrow. The payment must be captured before capture_deadline.
    pub fn authorize_payment(
        env: Env,
        merchant: Address,
        payment_id: u32,
        capture_window_seconds: u64,
    ) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.merchant != merchant {
            panic!("Only the payment merchant can authorize");
        }
        if payment.status != PaymentStatus::Pending {
            panic!("Only pending payments can be authorized");
        }
        if capture_window_seconds == 0 {
            panic!("capture_window_seconds must be positive");
        }

        let now = env.ledger().timestamp();
        let old_status = payment.status;
        payment.status = PaymentStatus::Authorized;
        payment.capture_deadline = now + capture_window_seconds;

        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_payment_authorized(&env, payment_id, payment.capture_deadline);
        events::emit_payment_status_changed(
            &env,
            payment_id,
            old_status,
            PaymentStatus::Authorized,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Merchant captures an authorized payment, completing it and releasing funds.
    /// Must be called before capture_deadline.
    pub fn capture_payment(env: Env, merchant: Address, payment_id: u32) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");

        if payment.merchant != merchant {
            panic!("Only the payment merchant can capture");
        }
        if payment.status != PaymentStatus::Authorized {
            panic!("Payment is not authorized");
        }
        if env.ledger().timestamp() >= payment.capture_deadline {
            panic!("Capture window has expired");
        }

        Self::finalize_payment(&env, payment_id, &mut payment);
        events::emit_payment_captured(&env, payment_id);
    }

    /// Execute a token swap via the configured DEX router.
    /// Returns the output amount or an error symbol.
    fn execute_token_swap(
        env: &Env,
        payment_id: u32,
        customer: &Address,
        input_token: &Address,
        output_token: &Address,
        input_amount: i128,
    ) -> Result<i128, Symbol> {
        let router: Address = env
            .storage()
            .instance()
            .get(&DataKey::SwapRouter)
            .expect("Swap router not configured");

        // Get slippage config
        let slippage_cfg = Self::get_slippage_config_internal(env);
        let max_slippage_bps = slippage_cfg.max_bps;

        // For now, we simulate a swap by checking if the router is set
        // In a real implementation, this would call the DEX router contract
        // to execute the swap and return the output amount
        
        // Placeholder: In production, this would invoke the router contract
        // For now, we return the input amount as if swap happened 1:1
        // This is where you'd integrate with actual DEX contracts like Soroban AMM
        
        // Simulate swap success with 1:1 rate (placeholder)
        // Real implementation would call router.swap() and handle slippage
        Ok(input_amount)
    }

    /// Shared settlement logic: checks conditions, deducts fees, distributes funds,
    /// updates stats, emits events, and stores receipt. `payment` is mutated in-place.
    fn finalize_payment(env: &Env, payment_id: u32, payment: &mut Payment) {
        // Oracle price condition check (#125)
        /*
        if let Some(ref condition) = payment.release_condition.clone() {
            let oracle_addr: Address = env
                .storage()
                .instance()
                .get(&DataKey::OracleAddress)
                .expect("Oracle not configured for conditional payment");
            let usdc_token: Address = env
                .storage()
                .instance()
                .get(&DataKey::UsdcToken)
                .expect("Oracle not configured for conditional payment");
            let max_oracle_age: u64 = env
                .storage()
                .instance()
                .get(&DataKey::MaxOracleAge)
                .expect("Oracle not configured for conditional payment");

            let oracle_client = oracle::OracleClient::new(env, &oracle_addr);
            let price_data: PriceData = oracle_client
                .lastprice(&condition.asset, &usdc_token)
                .expect("Oracle price unavailable");

            let current_ts = env.ledger().timestamp();
            let age = current_ts.saturating_sub(price_data.timestamp);
            if age > max_oracle_age {
                panic!("Oracle price is stale");
            }

            let met = match condition.direction {
                OracleDirection::Gte => price_data.price >= condition.threshold,
                OracleDirection::Lte => price_data.price <= condition.threshold,
            };

            events::emit_conditional_payment_attempt(
                env,
                payment_id,
                price_data.price,
                condition.threshold,
                met,
            );

            if !met {
                panic_with_error!(env, Error::OracleConditionNotMet);
            }
        }
        */

        // --- Volume cap check (#131) ---
        Self::check_and_update_merchant_volume_cap(env, payment_id, &payment.merchant, payment.amount);

        // --- Token Swap: Convert payment token to merchant's preferred token ---
        let mut final_token = payment.token.clone();
        let mut final_amount = payment.amount;

        // Check if swap is needed and possible
        let preferred_token: Option<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PreferredToken(payment.merchant.clone()));
        
        if let Some(preferred) = preferred_token {
            if payment.token != preferred {
                // Swap is needed
                let router: Option<Address> = env
                    .storage()
                    .instance()
                    .get(&DataKey::SwapRouter);
                
                if let Some(router_addr) = router {
                    // Attempt swap
                    let swap_result = Self::execute_token_swap(
                        env,
                        payment_id,
                        &payment.customer,
                        &payment.token,
                        &preferred,
                        payment.amount,
                    );
                    
                    match swap_result {
                        Ok(swapped_amount) => {
                            final_token = preferred;
                            final_amount = swapped_amount;
                            events::emit_payment_swapped_and_settled(
                                env,
                                payment_id,
                                payment.customer.clone(),
                                payment.merchant.clone(),
                                payment.token.clone(),
                                preferred,
                                payment.amount,
                                swapped_amount,
                            );
                        }
                        Err(reason) => {
                            // Swap failed - refund customer
                            let token_client = token::Client::new(env, &payment.token);
                            token_client.transfer(
                                &env.current_contract_address(),
                                &payment.customer,
                                &payment.amount,
                            );
                            
                            let old_status = payment.status;
                            payment.status = PaymentStatus::Refunded;
                            env.storage()
                                .persistent()
                                .set(&DataKey::Payment(payment_id), payment);
                            env.storage().persistent().extend_ttl(
                                &DataKey::Payment(payment_id),
                                PERSISTENT_LIFETIME_THRESHOLD,
                                PERSISTENT_BUMP_AMOUNT,
                            );
                            
                            Self::inc_global_refunded(env, &payment.token, payment.amount);
                            Self::inc_merchant_refunded(env, &payment.merchant, &payment.token, payment.amount);
                            Self::inc_merchant_failed(env, &payment.merchant); // #226
                            
                            events::emit_payment_swap_failed(
                                env,
                                payment_id,
                                payment.customer.clone(),
                                payment.token.clone(),
                                payment.amount,
                                reason,
                            );
                            events::emit_payment_status_changed(env, payment_id, old_status, PaymentStatus::Refunded);
                            
                            env.storage()
                                .instance()
                                .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
                            return; // Exit early - payment refunded
                        }
                    }
                } else {
                    // No router configured - cannot swap, use original token
                    final_token = payment.token.clone();
                    final_amount = payment.amount;
                }
            }
        }

        let rolling_before = Self::rolling_merchant_volume(env, &payment.merchant);
        let projected_volume = rolling_before + final_amount;
        let applied_fee_bps = Self::fee_bps_for_volume(env, projected_volume);
        let fee_amount = (final_amount * applied_fee_bps as i128) / 10_000;
        let net_amount = final_amount - fee_amount;

        let fee_recipient: Address = env
            .storage()
            .instance()
            .get(&DataKey::FeeRecipient)
            .expect("Fee recipient not configured");

        let token_client = token::Client::new(env, &final_token);

        if fee_amount > 0 {
            token_client.transfer(&env.current_contract_address(), &fee_recipient, &fee_amount);
            events::emit_fee_collected(
                env,
                payment_id,
                fee_amount,
                fee_recipient,
                final_token.clone(),
            );
            // #242: Accrue referral commission on the fee collected for referred merchants
            Self::accrue_referral_commission(env, &payment.merchant, payment_id, fee_amount);
        }

        let split_transfers = Self::distribute_net_payment(env, payment, net_amount);
        if split_transfers.len() > 0 {
            events::emit_payment_split_completed(env, payment_id, split_transfers);
        }

        let old_status = payment.status;
        payment.status = PaymentStatus::Completed;
        let original_amount = payment.amount;
        payment.amount = net_amount;
        let completed_at = env.ledger().timestamp();

        env.storage()
            .persistent()
            .set(&DataKey::Payment(payment_id), payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage()
            .persistent()
            .set(&DataKey::Settled(payment_id), &true);
        env.storage().persistent().extend_ttl(
            &DataKey::Settled(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        let receipt_hash = Self::compute_receipt_hash(
            env,
            payment_id,
            &payment.customer,
            &payment.merchant,
            net_amount,
            &payment.token,
            completed_at,
        );
        env.storage()
            .persistent()
            .set(&DataKey::PaymentReceipt(payment_id), &receipt_hash);
        env.storage().persistent().extend_ttl(
            &DataKey::PaymentReceipt(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::inc_global_completed(env, &payment.token, net_amount);
        Self::inc_merchant_completed(env, &payment.merchant, &payment.token, net_amount);
        Self::inc_volume_bucket(env, &payment.token, net_amount);
        Self::inc_merchant_volume_bucket(env, &payment.merchant, original_amount);

        // Auto-enqueue completed payment into merchant's withdrawal queue (#126)
        Self::enqueue_withdrawal(env, &payment.merchant, payment_id, net_amount);

        let rolling_after = Self::rolling_merchant_volume(env, &payment.merchant);
        let new_tier_bps = Self::fee_bps_for_volume(env, rolling_after);
        let tier_key = DataKey::MerchantCurrentTierBps(payment.merchant.clone());
        let old_tier_bps: u32 = env
            .storage()
            .persistent()
            .get(&tier_key)
            .unwrap_or(Self::get_fee_bps(env.clone()));
        if old_tier_bps != new_tier_bps {
            env.storage().persistent().set(&tier_key, &new_tier_bps);
            env.storage().persistent().extend_ttl(
                &tier_key,
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
            events::emit_merchant_tier_updated(
                env,
                payment.merchant.clone(),
                new_tier_bps,
                rolling_after,
            );
        }

        events::emit_payment_completed(
            env,
            payment_id,
            payment.merchant.clone(),
            net_amount,
            completed_at,
        );
        events::emit_payment_status_changed(env, payment_id, old_status, PaymentStatus::Completed);
        events::emit_payment_receipt_issued(env, payment_id, receipt_hash);

        // #239: Accrue loyalty points to customer
        Self::accrue_loyalty_points(env, payment_id, &payment.customer, payment.amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    fn distribute_net_payment(
        env: &Env,
        payment: &Payment,
        net_amount: i128,
    ) -> Vec<SplitTransfer> {
        let token_client = token::Client::new(env, &payment.token);

        if let Some(splits) = &payment.split_recipients {
            if splits.len() == 0 {
                token_client.transfer(
                    &env.current_contract_address(),
                    &payment.merchant,
                    &net_amount,
                );
                return Vec::new(env);
            }

            let mut split_events = Vec::new(env);
            let mut distributed: i128 = 0;
            for split in splits.iter() {
                let share = (net_amount * split.bps as i128) / 10_000;
                if share > 0 {
                    token_client.transfer(
                        &env.current_contract_address(),
                        &split.recipient,
                        &share,
                    );
                }
                distributed += share;
                split_events.push_back(SplitTransfer {
                    recipient: split.recipient,
                    bps: split.bps,
                    amount: share,
                });
            }

            let dust_to_merchant = net_amount - distributed;
            if dust_to_merchant > 0 {
                token_client.transfer(
                    &env.current_contract_address(),
                    &payment.merchant,
                    &dust_to_merchant,
                );
            }
            return split_events;
        }

        token_client.transfer(
            &env.current_contract_address(),
            &payment.merchant,
            &net_amount,
        );
        Vec::new(env)
    }

    fn validate_split_recipients(split_recipients: &Option<Vec<SplitRecipient>>) {
        if let Some(splits) = split_recipients {
            if splits.len() == 0 {
                panic!("split_recipients cannot be empty");
            }

            let mut total_bps: u32 = 0;
            for split in splits.iter() {
                if split.bps == 0 {
                    panic!("split recipient bps must be positive");
                }
                total_bps = total_bps
                    .checked_add(split.bps)
                    .expect("split bps overflow");
            }
            if total_bps != 10_000 {
                panic!("split_recipients must sum to 10000 bps");
            }
        }
    }

    fn validate_fee_tiers(tiers: &Vec<FeeTier>) {
        let mut last_min_volume: i128 = -1;
        for tier in tiers.iter() {
            if tier.fee_bps > MAX_FEE_BPS {
                panic!("tier fee cannot exceed 500 bps (5%)");
            }
            if tier.min_volume < 0 {
                panic!("tier min_volume cannot be negative");
            }
            if tier.min_volume <= last_min_volume {
                panic!("fee tiers must be strictly ascending by min_volume");
            }
            last_min_volume = tier.min_volume;
        }
    }

    /// Validate invoice data and verify total matches payment amount (#128).
    fn validate_invoice_data(env: &Env, invoice: &Option<InvoiceData>, payment_amount: i128) {
        if let Some(inv) = invoice {
            if inv.line_items.len() == 0 {
                panic!("Invoice line_items cannot be empty");
            }
            if (inv.line_items.len() as u32) > MAX_INVOICE_LINE_ITEMS {
                panic!("Invoice line items exceed maximum of 20");
            }

            let mut invoice_subtotal: i128 = 0;
            for item in inv.line_items.iter() {
                if item.quantity == 0 {
                    panic!("Line item quantity must be positive");
                }
                if item.unit_price < 0 {
                    panic!("Line item unit_price cannot be negative");
                }
                let line_amount = (item.quantity as i128)
                    .checked_mul(item.unit_price)
                    .expect("Line item amount overflow");
                invoice_subtotal = invoice_subtotal
                    .checked_add(line_amount)
                    .expect("Invoice subtotal overflow");
            }

            let tax_amount = (invoice_subtotal * inv.tax_bps as i128) / 10_000;
            let invoice_total = invoice_subtotal
                .checked_add(tax_amount)
                .expect("Invoice total overflow");

            if invoice_total != payment_amount {
                panic!("Invoice total does not match payment amount");
            }

            let _ = env; // suppress unused warning
        }
    }

    /// Compute SHA256 hash of serialized invoice data (#128).
    fn compute_invoice_hash(env: &Env, invoice: &InvoiceData) -> BytesN<32> {
        let mut preimage = Bytes::new(env);

        // Serialize line items count
        preimage.extend_from_array(&(invoice.line_items.len() as u32).to_be_bytes());

        // Serialize each line item
        for item in invoice.line_items.iter() {
            // For Symbol, we'll use its XDR representation
            preimage.append(&item.description.to_xdr(env));
            preimage.extend_from_array(&item.quantity.to_be_bytes());
            preimage.extend_from_array(&item.unit_price.to_be_bytes());
        }

        preimage.extend_from_array(&invoice.tax_bps.to_be_bytes());
        preimage.append(&invoice.currency_label.clone().to_xdr(env));

        env.crypto().sha256(&preimage).into()
    }

    fn fee_bps_for_volume(env: &Env, volume: i128) -> u32 {
        let default_fee: u32 = env
            .storage()
            .instance()
            .get(&DataKey::FeeBps)
            .unwrap_or(DEFAULT_FEE_BPS);
        let tiers: Vec<FeeTier> = env
            .storage()
            .instance()
            .get(&DataKey::FeeTiers)
            .unwrap_or(Vec::new(env));

        let mut selected = default_fee;
        for tier in tiers.iter() {
            if volume >= tier.min_volume {
                selected = tier.fee_bps;
            }
        }
        selected
    }

    fn rolling_merchant_volume(env: &Env, merchant: &Address) -> i128 {
        let current_bucket = env.ledger().sequence() / LEDGER_BUCKET_SIZE;
        let mut total = 0i128;
        for i in 0..4u32 {
            let bucket = current_bucket.saturating_sub(i);
            let key = DataKey::MerchantVolumeBucket(merchant.clone(), bucket);
            let bucket_total: i128 = env.storage().persistent().get(&key).unwrap_or(0);
            total += bucket_total;
        }
        total
    }

    fn enforce_rate_limit(env: &Env, customer: &Address, requested_payments: u32) {
        if requested_payments == 0 {
            return;
        }

        let cfg = Self::get_rate_limit_config_internal(env);
        let current_ledger = env.ledger().sequence();
        let key = DataKey::CustomerRateLimit(customer.clone());

        let mut state: CustomerRateLimit =
            env.storage()
                .persistent()
                .get(&key)
                .unwrap_or(CustomerRateLimit {
                    count: 0,
                    window_start_ledger: current_ledger,
                });

        if current_ledger.saturating_sub(state.window_start_ledger) >= cfg.window_size_ledgers {
            state.count = 0;
            state.window_start_ledger = current_ledger;
        }

        let new_count = state.count.saturating_add(requested_payments);
        if new_count > cfg.max_payments {
            panic_with_error!(env, Error::RateLimitExceeded);
        }

        state.count = new_count;
        env.storage().persistent().set(&key, &state);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    /// Validates merchant is approved or open mode is enabled.
    fn require_merchant_approved(env: &Env, merchant: &Address) {
        // Always enforce ban/suspension regardless of open mode
        let status: MerchantStatus = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantStatus(merchant.clone()))
            .unwrap_or(MerchantStatus::Active);

        match status {
            MerchantStatus::Banned => panic!("Merchant is banned"),
            MerchantStatus::Suspended => {
                let expiry: u64 = env
                    .storage()
                    .persistent()
                    .get(&DataKey::MerchantSuspensionExpiry(merchant.clone()))
                    .unwrap_or(0);
                if expiry == 0 || env.ledger().timestamp() <= expiry {
                    panic!("Merchant is suspended");
                }
                // Suspension expired — fall through to allowlist check
            }
            MerchantStatus::Active => {}
        }

        let open_mode: bool = env
            .storage()
            .instance()
            .get(&DataKey::MerchantOpenMode)
            .unwrap_or(true); // Default: open mode (no allowlist enforcement)
        if open_mode {
            return;
        }

        let approved: bool = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantApproved(merchant.clone()))
            .unwrap_or(false);
        if !approved {
            panic!("Merchant not approved");
        }
    }

    /// Validate optional reference string: max 64 bytes (#67).
    fn validate_reference(env: &Env, reference: &Option<String>) {
        if let Some(r) = reference {
            if r.len() > MAX_REFERENCE_LEN {
                panic!("Reference exceeds maximum length of 64 bytes");
            }
            let _ = env; // suppress unused warning
        }
    }

    /// Validate optional metadata map: max 5 keys, each key/value max 32 bytes (#67).
    fn validate_metadata(env: &Env, metadata: &Option<Map<String, String>>) {
        if let Some(m) = metadata {
            if m.len() > MAX_METADATA_KEYS {
                panic!("Metadata exceeds maximum of 5 keys");
            }
            for (k, v) in m.iter() {
                if k.len() > MAX_METADATA_KEY_LEN {
                    panic!("Metadata key exceeds maximum length of 32 bytes");
                }
                if v.len() > MAX_METADATA_KEY_LEN {
                    panic!("Metadata value exceeds maximum length of 32 bytes");
                }
            }
            let _ = env; // suppress unused warning
        }
    }

    /// Compute a simple u32 hash of a reference string for use as a storage key (#67).
    fn reference_hash(_env: &Env, reference: &String) -> u32 {
        let bytes = reference.to_bytes();
        let mut h: u32 = 2166136261u32;
        for b in bytes.iter() {
            h = h.wrapping_mul(16777619).wrapping_add(b as u32);
        }
        h
    }

    /// Append payment_id to the merchant+reference index (#67).
    fn index_payment_by_reference(
        env: &Env,
        merchant: &Address,
        reference: &String,
        payment_id: u32,
    ) {
        let hash = Self::reference_hash(env, reference);
        let key = DataKey::MerchantReference(merchant.clone(), hash);
        let mut ids: Vec<u32> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(env));
        ids.push_back(payment_id);
        env.storage().persistent().set(&key, &ids);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    /// Compute sha256(payment_id || customer || merchant || amount || token || completed_at).
    /// All integers encoded big-endian. Addresses encoded as their raw bytes.
    fn compute_receipt_hash(
        env: &Env,
        payment_id: u32,
        customer: &Address,
        merchant: &Address,
        amount: i128,
        token: &Address,
        completed_at: u64,
    ) -> BytesN<32> {
        let mut preimage = Bytes::new(env);
        preimage.extend_from_array(&payment_id.to_be_bytes());
        preimage.append(&customer.to_xdr(env));
        preimage.append(&merchant.to_xdr(env));
        preimage.extend_from_array(&amount.to_be_bytes());
        preimage.append(&token.to_xdr(env));
        preimage.extend_from_array(&completed_at.to_be_bytes());
        env.crypto().sha256(&preimage).into()
    }

    // --- Stats Helpers (#70) ---

    fn load_global_stats(env: &Env) -> GlobalStats {
        env.storage()
            .persistent()
            .get(&DataKey::GlobalStats)
            .unwrap_or(GlobalStats {
                total_payments_created: 0,
                total_payments_completed: 0,
                total_payments_refunded: 0,
                total_payments_expired: 0,
                total_volume_completed: Map::new(env),
                total_volume_refunded: Map::new(env),
            })
    }

    fn save_global_stats(env: &Env, stats: &GlobalStats) {
        env.storage().persistent().set(&DataKey::GlobalStats, stats);
        env.storage().persistent().extend_ttl(
            &DataKey::GlobalStats,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn load_merchant_stats(env: &Env, merchant: &Address) -> MerchantStats {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantStats(merchant.clone()))
            .unwrap_or(MerchantStats {
                payments_created: 0,
                payments_completed: 0,
                payments_refunded: 0,
                volume_completed: Map::new(env),
            })
    }

    fn save_merchant_stats(env: &Env, merchant: &Address, stats: &MerchantStats) {
        let key = DataKey::MerchantStats(merchant.clone());
        env.storage().persistent().set(&key, stats);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn inc_global_created(env: &Env) {
        let mut s = Self::load_global_stats(env);
        s.total_payments_created += 1;
        Self::save_global_stats(env, &s);
    }

    fn inc_global_completed(env: &Env, token: &Address, amount: i128) {
        let mut s = Self::load_global_stats(env);
        s.total_payments_completed += 1;
        let prev = s.total_volume_completed.get(token.clone()).unwrap_or(0);
        s.total_volume_completed.set(token.clone(), prev + amount);
        Self::save_global_stats(env, &s);
    }

    fn inc_global_refunded(env: &Env, token: &Address, amount: i128) {
        let mut s = Self::load_global_stats(env);
        s.total_payments_refunded += 1;
        let prev = s.total_volume_refunded.get(token.clone()).unwrap_or(0);
        s.total_volume_refunded.set(token.clone(), prev + amount);
        Self::save_global_stats(env, &s);
    }

    fn inc_global_expired(env: &Env) {
        let mut s = Self::load_global_stats(env);
        s.total_payments_expired += 1;
        Self::save_global_stats(env, &s);
    }

    fn inc_merchant_created(env: &Env, merchant: &Address) {
        let mut s = Self::load_merchant_stats(env, merchant);
        s.payments_created += 1;
        Self::save_merchant_stats(env, merchant, &s);

        // #226: Track pending count in MerchantSummary
        let key = DataKey::MerchantSummary(merchant.clone());
        let mut summary: MerchantSummary = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(MerchantSummary {
                total_volume: 0,
                completed_count: 0,
                failed_count: 0,
                pending_count: 0,
                volume_by_token: Map::new(env),
            });
        summary.pending_count += 1;
        env.storage().persistent().set(&key, &summary);
        env.storage().persistent().extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
    }

    fn inc_merchant_completed(env: &Env, merchant: &Address, token: &Address, amount: i128) {
        let mut s = Self::load_merchant_stats(env, merchant);
        s.payments_completed += 1;
        let prev = s.volume_completed.get(token.clone()).unwrap_or(0);
        s.volume_completed.set(token.clone(), prev + amount);
        Self::save_merchant_stats(env, merchant, &s);

        // #226: Update MerchantSummary
        let key = DataKey::MerchantSummary(merchant.clone());
        let mut summary: MerchantSummary = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(MerchantSummary {
                total_volume: 0,
                completed_count: 0,
                failed_count: 0,
                pending_count: 0,
                volume_by_token: Map::new(env),
            });
        summary.completed_count += 1;
        if summary.pending_count > 0 { summary.pending_count -= 1; }
        summary.total_volume += amount;
        let prev_token = summary.volume_by_token.get(token.clone()).unwrap_or(0);
        summary.volume_by_token.set(token.clone(), prev_token + amount);
        env.storage().persistent().set(&key, &summary);
        env.storage().persistent().extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
    }

    fn inc_merchant_failed(env: &Env, merchant: &Address) {
        let key = DataKey::MerchantSummary(merchant.clone());
        let mut summary: MerchantSummary = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(MerchantSummary {
                total_volume: 0,
                completed_count: 0,
                failed_count: 0,
                pending_count: 0,
                volume_by_token: Map::new(env),
            });
        summary.failed_count += 1;
        if summary.pending_count > 0 { summary.pending_count -= 1; }
        env.storage().persistent().set(&key, &summary);
        env.storage().persistent().extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
    }
        let key = DataKey::MerchantSummary(merchant.clone());
        let mut summary: MerchantSummary = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(MerchantSummary {
                total_volume: 0,
                completed_count: 0,
                failed_count: 0,
                pending_count: 0,
                volume_by_token: Map::new(env),
            });
        summary.failed_count += 1;
        env.storage().persistent().set(&key, &summary);
        env.storage().persistent().extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
    }

    fn inc_merchant_refunded(env: &Env, merchant: &Address, token: &Address, amount: i128) {
        let mut s = Self::load_merchant_stats(env, merchant);
        s.payments_refunded += 1;
        let _ = (token, amount); // volume tracked globally; merchant count only
        Self::save_merchant_stats(env, merchant, &s);
    }

    fn inc_volume_bucket(env: &Env, token: &Address, amount: i128) {
        let bucket = env.ledger().sequence() / LEDGER_BUCKET_SIZE;
        let key = DataKey::VolumeBucket(token.clone(), bucket);
        let prev: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(prev + amount));
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn inc_merchant_volume_bucket(env: &Env, merchant: &Address, amount: i128) {
        let bucket = env.ledger().sequence() / LEDGER_BUCKET_SIZE;
        let key = DataKey::MerchantVolumeBucket(merchant.clone(), bucket);
        let prev: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage().persistent().set(&key, &(prev + amount));
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn next_payment_id(env: &Env) -> u32 {
        let mut counter: u32 = env
            .storage()
            .instance()
            .get(&DataKey::PaymentCounter)
            .unwrap_or(0);
        let id = counter;
        counter += 1;
        // Counter stays in instance storage — bounded, config-like
        env.storage()
            .instance()
            .set(&DataKey::PaymentCounter, &counter);
        id
    }

    fn add_customer_payment(env: &Env, customer: &Address, payment_id: u32) {
        let key = DataKey::CustomerPayments(customer.clone());
        let mut customer_payments: Vec<u32> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(env));
        customer_payments.push_back(payment_id);
        // Persistent: customer index grows with payment volume
        env.storage().persistent().set(&key, &customer_payments);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn get_or_init_version(env: &Env) -> u32 {
        if let Some(version) = env
            .storage()
            .instance()
            .get::<DataKey, u32>(&DataKey::ContractVersion)
        {
            version
        } else {
            let initial_version = 1u32;
            env.storage()
                .instance()
                .set(&DataKey::ContractVersion, &initial_version);
            initial_version
        }
    }

    fn get_rate_limit_config_internal(env: &Env) -> RateLimitConfig {
        env.storage()
            .instance()
            .get(&DataKey::RateLimitConfig)
            .unwrap_or(RateLimitConfig {
                max_payments: DEFAULT_RATE_LIMIT_MAX_PAYMENTS,
                window_size_ledgers: DEFAULT_RATE_LIMIT_WINDOW_SIZE_LEDGERS,
            })
    }

    /// Returns the configured minimum collateral, falling back to the default (#129).
    fn get_min_collateral_internal(env: &Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::MinCollateral)
            .unwrap_or(DEFAULT_MIN_COLLATERAL)
    }

    // --- Withdrawal Queue Helpers (#126) ---

    /// Enqueue a completed payment into the merchant's withdrawal queue.
    fn enqueue_withdrawal(env: &Env, merchant: &Address, payment_id: u32, amount: i128) {
        let key = DataKey::WithdrawalQueue(merchant.clone());
        let mut queue: Vec<(u32, i128)> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(env));
        queue.push_back((payment_id, amount));
        env.storage().persistent().set(&key, &queue);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_withdrawal_queued(env, merchant.clone(), payment_id, amount);
    }

    /// Get the merchant's withdrawal queue.
    fn get_withdrawal_queue(env: &Env, merchant: &Address) -> Vec<(u32, i128)> {
        let key = DataKey::WithdrawalQueue(merchant.clone());
        env.storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(env))
    }

    /// Set the merchant's withdrawal queue.
    fn set_withdrawal_queue(env: &Env, merchant: &Address, queue: Vec<(u32, i128)>) {
        let key = DataKey::WithdrawalQueue(merchant.clone());
        env.storage().persistent().set(&key, &queue);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    /// Returns the global slippage config, falling back to defaults (#135).
    fn get_slippage_config_internal(env: &Env) -> SlippageConfig {
        env.storage()
            .instance()
            .get(&DataKey::SlippageConfig)
            .unwrap_or(SlippageConfig {
                default_bps: DEFAULT_SLIPPAGE_BPS,
                min_bps: DEFAULT_MIN_SLIPPAGE_BPS,
                max_bps: DEFAULT_MAX_SLIPPAGE_BPS,
            })
    }

    /// Check merchant volume cap and update window volume. Panics if cap would be exceeded (#131).
    fn check_and_update_merchant_volume_cap(
        env: &Env,
        payment_id: u32,
        merchant: &Address,
        amount: i128,
    ) {
        let cap_key = DataKey::VolumeCap(merchant.clone());
        let cap: VolumeCap = match env.storage().persistent().get(&cap_key) {
            Some(c) => c,
            None => return, // No cap configured — no restriction
        };

        let now = env.ledger().timestamp();
        let bucket = now / cap.window_seconds;
        let vol_key = DataKey::MerchantWindowVolume(merchant.clone(), bucket);
        let current_volume: i128 = env.storage().persistent().get(&vol_key).unwrap_or(0);
        let new_volume = current_volume
            .checked_add(amount)
            .expect("Volume overflow");

        if new_volume > cap.cap_amount {
            events::emit_volume_capped(env, merchant.clone(), payment_id, current_volume, cap.cap_amount);
            panic_with_error!(env, Error::MerchantVolumeCapped);
        }

        env.storage().persistent().set(&vol_key, &new_volume);
        env.storage().persistent().extend_ttl(
            &vol_key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    // --- Merchant Ban Management & On-Chain Appeal ---

    /// Admin sets the appeal rejection cooldown in seconds.
    pub fn set_appeal_rejection_cooldown(env: Env, admin: Address, seconds: u64) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can configure appeal cooldown");
        }
        env.storage()
            .instance()
            .set(&DataKey::AppealRejectionCooldownSeconds, &seconds);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin suspends a merchant for a given duration (seconds). Payments are paused.
    pub fn suspend_merchant(env: Env, admin: Address, merchant: Address, reason_hash: BytesN<32>, duration_seconds: u64) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can suspend merchants");
        }
        if duration_seconds == 0 {
            panic!("Suspension duration must be positive");
        }

        let expiry = env.ledger().timestamp() + duration_seconds;
        env.storage()
            .persistent()
            .set(&DataKey::MerchantStatus(merchant.clone()), &MerchantStatus::Suspended);
        env.storage()
            .persistent()
            .set(&DataKey::MerchantSuspensionExpiry(merchant.clone()), &expiry);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantStatus(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantSuspensionExpiry(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_merchant_suspended(&env, merchant, reason_hash, expiry);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin permanently bans a merchant. Banned merchants cannot create or receive payments.
    pub fn ban_merchant(env: Env, admin: Address, merchant: Address, reason_hash: BytesN<32>) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can ban merchants");
        }

        env.storage()
            .persistent()
            .set(&DataKey::MerchantStatus(merchant.clone()), &MerchantStatus::Banned);
        env.storage()
            .persistent()
            .set(&DataKey::MerchantApproved(merchant.clone()), &false);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantStatus(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_merchant_banned(&env, merchant, reason_hash);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin reinstates a merchant (clears ban/suspension, restores Active status).
    pub fn reinstate_merchant(env: Env, admin: Address, merchant: Address) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can reinstate merchants");
        }

        env.storage()
            .persistent()
            .set(&DataKey::MerchantStatus(merchant.clone()), &MerchantStatus::Active);
        env.storage()
            .persistent()
            .set(&DataKey::MerchantApproved(merchant.clone()), &true);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantStatus(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Banned merchant submits an appeal. Only one active appeal per merchant.
    pub fn submit_appeal(env: Env, merchant: Address, evidence_hash: BytesN<32>) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        let status: MerchantStatus = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantStatus(merchant.clone()))
            .unwrap_or(MerchantStatus::Active);

        if status != MerchantStatus::Banned {
            panic!("Only banned merchants can submit an appeal");
        }

        // Enforce one-active-appeal guard
        if let Some(existing): Option<MerchantAppeal> = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantAppeal(merchant.clone()))
        {
            if !existing.resolved {
                panic!("An active appeal already exists");
            }
        }

        // Enforce cooldown after rejection
        let cooldown_until: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::AppealCooldownUntil(merchant.clone()))
            .unwrap_or(0);
        if env.ledger().timestamp() < cooldown_until {
            panic!("Appeal cooldown has not elapsed");
        }

        let appeal = MerchantAppeal {
            merchant: merchant.clone(),
            evidence_hash: evidence_hash.clone(),
            submitted_at: env.ledger().timestamp(),
            resolved: false,
            approved: false,
        };

        env.storage()
            .persistent()
            .set(&DataKey::MerchantAppeal(merchant.clone()), &appeal);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantAppeal(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_appeal_submitted(&env, merchant, evidence_hash);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin approves an appeal — reinstates the merchant.
    pub fn approve_appeal(env: Env, admin: Address, merchant: Address) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can approve appeals");
        }

        let mut appeal: MerchantAppeal = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantAppeal(merchant.clone()))
            .expect("No appeal found");

        if appeal.resolved {
            panic!("Appeal already resolved");
        }

        appeal.resolved = true;
        appeal.approved = true;
        env.storage()
            .persistent()
            .set(&DataKey::MerchantAppeal(merchant.clone()), &appeal);

        // Reinstate merchant
        env.storage()
            .persistent()
            .set(&DataKey::MerchantStatus(merchant.clone()), &MerchantStatus::Active);
        env.storage()
            .persistent()
            .set(&DataKey::MerchantApproved(merchant.clone()), &true);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantStatus(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_appeal_approved(&env, merchant);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin rejects an appeal — merchant remains banned and cooldown is applied.
    pub fn reject_appeal(env: Env, admin: Address, merchant: Address) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can reject appeals");
        }

        let mut appeal: MerchantAppeal = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantAppeal(merchant.clone()))
            .expect("No appeal found");

        if appeal.resolved {
            panic!("Appeal already resolved");
        }

        appeal.resolved = true;
        appeal.approved = false;
        env.storage()
            .persistent()
            .set(&DataKey::MerchantAppeal(merchant.clone()), &appeal);

        // Apply cooldown
        let cooldown_seconds: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AppealRejectionCooldownSeconds)
            .unwrap_or(0);
        if cooldown_seconds > 0 {
            let cooldown_until = env.ledger().timestamp() + cooldown_seconds;
            env.storage()
                .persistent()
                .set(&DataKey::AppealCooldownUntil(merchant.clone()), &cooldown_until);
            env.storage().persistent().extend_ttl(
                &DataKey::AppealCooldownUntil(merchant.clone()),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        events::emit_appeal_rejected(&env, merchant);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the current status of a merchant.
    pub fn get_merchant_status(env: Env, merchant: Address) -> MerchantStatus {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantStatus(merchant))
            .unwrap_or(MerchantStatus::Active)
    }

    /// Get the active appeal for a merchant, if any.
    pub fn get_merchant_appeal(env: Env, merchant: Address) -> Option<MerchantAppeal> {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantAppeal(merchant))
    }

    // ─── #226: Merchant Revenue Dashboard ────────────────────────────────────

    /// Returns the full merchant revenue summary.
    pub fn get_merchant_summary(env: Env, merchant: Address) -> MerchantSummary {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantSummary(merchant))
            .unwrap_or(MerchantSummary {
                total_volume: 0,
                completed_count: 0,
                failed_count: 0,
                pending_count: 0,
                volume_by_token: Map::new(&env),
            })
    }

    /// Returns the completed volume for a specific token for a merchant.
    pub fn get_merchant_volume_by_token(env: Env, merchant: Address, token: Address) -> i128 {
        let summary: MerchantSummary = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantSummary(merchant))
            .unwrap_or(MerchantSummary {
                total_volume: 0,
                completed_count: 0,
                failed_count: 0,
                pending_count: 0,
                volume_by_token: Map::new(&env),
            });
        summary.volume_by_token.get(token).unwrap_or(0)
    }

    /// Returns (completed_count, failed_count, pending_count) for a merchant.
    pub fn get_merchant_payment_counts(env: Env, merchant: Address) -> (u32, u32, u32) {
        let summary: MerchantSummary = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantSummary(merchant))
            .unwrap_or(MerchantSummary {
                total_volume: 0,
                completed_count: 0,
                failed_count: 0,
                pending_count: 0,
                volume_by_token: Map::new(&env),
            });
        (summary.completed_count, summary.failed_count, summary.pending_count)
    }

    /// Admin resets a merchant's dashboard counters.
    pub fn reset_merchant_stats(env: Env, merchant: Address) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        let empty = MerchantSummary {
            total_volume: 0,
            completed_count: 0,
            failed_count: 0,
            pending_count: 0,
            volume_by_token: Map::new(&env),
        };
        let key = DataKey::MerchantSummary(merchant);
        env.storage().persistent().set(&key, &empty);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // ── #239: Customer Loyalty Points ─────────────────────────────────────────

    /// Admin configures the loyalty points system.
    /// points_per_unit: points earned per 1_000_000 units of payment token.
    /// redemption_rate_bps: discount in basis points per 1 point redeemed.
    /// min_payment_floor: minimum payment amount after discount.
    /// expiry_ledgers: ledgers after which unspent points expire (0 = no expiry).
    pub fn configure_loyalty(
        env: Env,
        admin: Address,
        points_per_unit: u32,
        redemption_rate_bps: u32,
        min_payment_floor: i128,
        expiry_ledgers: u32,
    ) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin");
        }
        env.storage().instance().set(&DataKey::LoyaltyPointsPerUnit, &points_per_unit);
        env.storage().instance().set(&DataKey::LoyaltyRedemptionRateBps, &redemption_rate_bps);
        env.storage().instance().set(&DataKey::LoyaltyMinPaymentFloor, &min_payment_floor);
        env.storage().instance().set(&DataKey::LoyaltyExpiryLedgers, &expiry_ledgers);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Customer redeems loyalty points as a discount on a pending payment.
    /// Discount = points_to_redeem * redemption_rate_bps / 10_000.
    /// Payment amount cannot drop below min_payment_floor.
    /// Points are non-transferable (tied to customer address).
    pub fn redeem_points(env: Env, customer: Address, payment_id: u32, points_to_redeem: i128) {
        customer.require_auth();

        // Expire stale points first
        Self::maybe_expire_points(&env, &customer);

        let balance: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::LoyaltyBalance(customer.clone()))
            .unwrap_or(0);
        if points_to_redeem <= 0 || points_to_redeem > balance {
            panic!("Insufficient loyalty points");
        }

        let redemption_rate_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::LoyaltyRedemptionRateBps)
            .unwrap_or(0);
        let min_floor: i128 = env
            .storage()
            .instance()
            .get(&DataKey::LoyaltyMinPaymentFloor)
            .unwrap_or(0);

        let mut payment: Payment = env
            .storage()
            .persistent()
            .get(&DataKey::Payment(payment_id))
            .expect("Payment not found");
        if payment.customer != customer {
            panic!("Not your payment");
        }
        if payment.status != PaymentStatus::Pending {
            panic!("Payment not pending");
        }

        let discount = (points_to_redeem * redemption_rate_bps as i128) / 10_000;
        let new_amount = (payment.amount - discount).max(min_floor);
        let actual_discount = payment.amount - new_amount;
        let actual_points_used = if actual_discount == discount {
            points_to_redeem
        } else {
            // Recalculate points consumed when floor was hit
            if redemption_rate_bps == 0 { points_to_redeem } else {
                (actual_discount * 10_000) / redemption_rate_bps as i128
            }
        };

        // Burn points
        let new_balance = balance - actual_points_used;
        env.storage().persistent().set(&DataKey::LoyaltyBalance(customer.clone()), &new_balance);
        env.storage().persistent().extend_ttl(
            &DataKey::LoyaltyBalance(customer.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Apply discount to payment
        payment.amount = new_amount;
        env.storage().persistent().set(&DataKey::Payment(payment_id), &payment);
        env.storage().persistent().extend_ttl(
            &DataKey::Payment(payment_id),
    // =========================================================================
    // #242: Merchant Referral Commission Tracking
    // =========================================================================
    // #235: Merchant-Level Customer Spending Limits
    // =========================================================================

    /// Merchant sets a per-customer spend cap within a rolling window.
    pub fn set_customer_spend_limit(
    // ─── #216: Recurring Invoice Scheduling ──────────────────────────────────

    /// Merchant creates a recurring invoice schedule.
    /// `max_cycles` = 0 means unlimited.
    pub fn create_recurring_invoice(
        env: Env,
        merchant: Address,
        customer: Address,
        amount: i128,
        window_seconds: u64,
    ) {
        Self::require_not_paused(&env);
        merchant.require_auth();
        if amount <= 0 {
            panic!("Spend limit amount must be positive");
        }
        if window_seconds == 0 {
            panic!("Window seconds must be positive");
        }
        let key = DataKey::CustomerSpendLimit(merchant.clone(), customer.clone());
        let limit = SpendLimit { amount, window_seconds };
        env.storage().persistent().set(&key, &limit);
        env.storage().persistent().extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
        events::emit_customer_spend_limit_set(&env, merchant, customer, amount, window_seconds);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Merchant removes a per-customer spend cap.
    pub fn remove_customer_spend_limit(env: Env, merchant: Address, customer: Address) {
        Self::require_not_paused(&env);
        merchant.require_auth();
        let key = DataKey::CustomerSpendLimit(merchant.clone(), customer.clone());
        env.storage().persistent().remove(&key);
        events::emit_customer_spend_limit_removed(&env, merchant, customer);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Merchant sets a global default spend cap applied to all customers without an individual override.
    pub fn set_default_spend_limit(
        env: Env,
        merchant: Address,
        amount: i128,
        window_seconds: u64,
    ) {
        Self::require_not_paused(&env);
        merchant.require_auth();
        if amount <= 0 {
            panic!("Spend limit amount must be positive");
        }
        if window_seconds == 0 {
            panic!("Window seconds must be positive");
        }
        let key = DataKey::DefaultSpendLimit(merchant.clone());
        let limit = SpendLimit { amount, window_seconds };
        env.storage().persistent().set(&key, &limit);
        env.storage().persistent().extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
        events::emit_default_spend_limit_set(&env, merchant, amount, window_seconds);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the per-customer spend limit for a merchant+customer pair.
    pub fn get_customer_spend_limit(env: Env, merchant: Address, customer: Address) -> Option<SpendLimit> {
        env.storage().persistent().get(&DataKey::CustomerSpendLimit(merchant, customer))
    }

    /// Get the default spend limit for a merchant.
    pub fn get_default_spend_limit(env: Env, merchant: Address) -> Option<SpendLimit> {
        env.storage().persistent().get(&DataKey::DefaultSpendLimit(merchant))
    }

    /// Check and update the customer spend window. Panics with CustomerSpendLimitExceeded if cap would be breached.
    fn check_and_update_customer_spend_limit(env: &Env, merchant: &Address, customer: &Address, amount: i128) {
        // Individual override takes priority over default
        let limit_opt: Option<SpendLimit> = env
            .storage()
            .persistent()
            .get(&DataKey::CustomerSpendLimit(merchant.clone(), customer.clone()))
            .or_else(|| env.storage().persistent().get(&DataKey::DefaultSpendLimit(merchant.clone())));

        let limit = match limit_opt {
            Some(l) => l,
            None => return, // No limit configured
        };

        let state_key = DataKey::CustomerSpendWindowState(merchant.clone(), customer.clone());
        let now = env.ledger().timestamp();

        let mut state: CustomerSpendWindow = env
            .storage()
            .persistent()
            .get(&state_key)
            .unwrap_or(CustomerSpendWindow { window_start: now, spent: 0 });

        // Reset window if expired
        if now >= state.window_start + limit.window_seconds {
            state = CustomerSpendWindow { window_start: now, spent: 0 };
        }

        let new_total = state.spent.checked_add(amount).expect("Spend overflow");
        if new_total > limit.amount {
            events::emit_customer_spend_limit_exceeded(env, merchant.clone(), customer.clone(), new_total, limit.amount);
            panic_with_error!(env, Error::CustomerSpendLimitExceeded);
        }

        state.spent = new_total;
        env.storage().persistent().set(&state_key, &state);
        env.storage().persistent().extend_ttl(&state_key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
        token: Address,
        interval_seconds: u64,
        max_cycles: u32,
        reference_hash: Option<BytesN<32>>,
    ) -> u32 {
        Self::require_not_paused(&env);
        merchant.require_auth();

        if amount <= 0 {
            panic!("Amount must be positive");
        }
        if interval_seconds == 0 {
            panic!("Interval must be positive");
        }

        Self::require_token_allowed(&env, &token);
        Self::require_merchant_approved(&env, &merchant);

        let mut counter: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RecurringInvoiceCounter)
            .unwrap_or(0);
        let invoice_id = counter;
        counter += 1;
        env.storage()
            .instance()
            .set(&DataKey::RecurringInvoiceCounter, &counter);

        let now = env.ledger().timestamp();
        let invoice = RecurringInvoice {
            id: invoice_id,
            merchant: merchant.clone(),
            customer: customer.clone(),
            amount,
            token,
            interval_seconds,
            max_cycles,
            cycles_triggered: 0,
            next_due_at: now,
            reference_hash,
            active: true,
        };

        env.storage()
            .persistent()
            .set(&DataKey::RecurringInvoice(invoice_id), &invoice);
        env.storage().persistent().extend_ttl(
            &DataKey::RecurringInvoice(invoice_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_recurring_invoice_created(&env, invoice_id, merchant, customer, amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        invoice_id
    }

    /// Trigger the next invoice cycle, creating a standard Payment entry.
    /// Callable by anyone (merchant, keeper, etc.) when the interval has elapsed.
    /// Returns the new payment_id.
    pub fn trigger_invoice_cycle(env: Env, invoice_id: u32) -> u32 {
        Self::require_not_paused(&env);

        let mut invoice: RecurringInvoice = env
            .storage()
            .persistent()
            .get(&DataKey::RecurringInvoice(invoice_id))
            .expect("Recurring invoice not found");

        if !invoice.active {
            panic!("Recurring invoice is cancelled");
        }

        let now = env.ledger().timestamp();
        if now < invoice.next_due_at {
            panic!("Invoice interval has not elapsed");
        }

        if invoice.max_cycles > 0 && invoice.cycles_triggered >= invoice.max_cycles {
            panic!("Max cycles reached");
        }

        // Create a standard Payment entry (funds transferred from customer)
        let payment_id = Self::create_payment_with_options(
            env.clone(),
            invoice.customer.clone(),
            invoice.merchant.clone(),
            invoice.amount,
            invoice.token.clone(),
            None,
            None,
            None,
            None,
            None,
        );

        invoice.cycles_triggered += 1;
        invoice.next_due_at = now + invoice.interval_seconds;

        // Auto-complete if max_cycles reached
        if invoice.max_cycles > 0 && invoice.cycles_triggered >= invoice.max_cycles {
            invoice.active = false;
            events::emit_recurring_invoice_completed(&env, invoice_id);
        }

        env.storage()
            .persistent()
            .set(&DataKey::RecurringInvoice(invoice_id), &invoice);
        env.storage().persistent().extend_ttl(
            &DataKey::RecurringInvoice(invoice_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_points_redeemed(&env, customer, payment_id, actual_points_used, actual_discount);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns the loyalty points balance for a customer (after expiry check).
    pub fn get_loyalty_balance(env: Env, customer: Address) -> i128 {
        Self::maybe_expire_points(&env, &customer);
        env.storage()
            .persistent()
            .get(&DataKey::LoyaltyBalance(customer))
            .unwrap_or(0)
    }

    /// Internal: mint points to customer after a completed payment.
    fn accrue_loyalty_points(env: &Env, payment_id: u32, customer: &Address, payment_amount: i128) {
        let points_per_unit: u32 = match env
            .storage()
            .instance()
            .get(&DataKey::LoyaltyPointsPerUnit)
        {
            Some(v) => v,
            None => return, // loyalty not configured
        };
        if points_per_unit == 0 {
            return;
        }

        // Expire stale points before accruing
        Self::maybe_expire_points(env, customer);

        let points_earned = payment_amount * points_per_unit as i128 / 1_000_000;
        if points_earned <= 0 {
            return;
        }

        let old_balance: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::LoyaltyBalance(customer.clone()))
            .unwrap_or(0);
        let new_balance = old_balance + points_earned;

        env.storage().persistent().set(&DataKey::LoyaltyBalance(customer.clone()), &new_balance);
        env.storage().persistent().extend_ttl(
            &DataKey::LoyaltyBalance(customer.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().set(
            &DataKey::LoyaltyLastAccrualLedger(customer.clone()),
            &env.ledger().sequence(),
        );
        env.storage().persistent().extend_ttl(
            &DataKey::LoyaltyLastAccrualLedger(customer.clone()),
        events::emit_invoice_cycle_triggered(&env, invoice_id, payment_id, invoice.cycles_triggered);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        payment_id
    }

    /// Cancel a recurring invoice. Callable by merchant or customer.
    pub fn cancel_recurring_invoice(env: Env, caller: Address, invoice_id: u32) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut invoice: RecurringInvoice = env
            .storage()
            .persistent()
            .get(&DataKey::RecurringInvoice(invoice_id))
            .expect("Recurring invoice not found");

        if caller != invoice.merchant && caller != invoice.customer {
            panic!("Only merchant or customer can cancel");
        }

        if !invoice.active {
            panic!("Recurring invoice is already cancelled");
        }

        invoice.active = false;
        env.storage()
            .persistent()
            .set(&DataKey::RecurringInvoice(invoice_id), &invoice);
        env.storage().persistent().extend_ttl(
            &DataKey::RecurringInvoice(invoice_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_points_accrued(env, customer.clone(), payment_id, points_earned, new_balance);
    }

    /// Internal: burn expired points if expiry window has passed.
    fn maybe_expire_points(env: &Env, customer: &Address) {
        let expiry_ledgers: u32 = env
            .storage()
            .instance()
            .get(&DataKey::LoyaltyExpiryLedgers)
            .unwrap_or(0);
        if expiry_ledgers == 0 {
            return;
        }
        let last_accrual: u32 = match env
            .storage()
            .persistent()
            .get(&DataKey::LoyaltyLastAccrualLedger(customer.clone()))
        {
            Some(v) => v,
            None => return,
        };
        let current = env.ledger().sequence();
        if current >= last_accrual + expiry_ledgers {
            let balance: i128 = env
                .storage()
                .persistent()
                .get(&DataKey::LoyaltyBalance(customer.clone()))
                .unwrap_or(0);
            if balance > 0 {
                env.storage().persistent().set(&DataKey::LoyaltyBalance(customer.clone()), &0i128);
                events::emit_points_expired(env, customer.clone(), balance);
            }
        }
        events::emit_recurring_invoice_cancelled(&env, invoice_id, caller);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get a recurring invoice by ID.
    pub fn get_recurring_invoice(env: Env, invoice_id: u32) -> RecurringInvoice {
        env.storage()
            .persistent()
            .get(&DataKey::RecurringInvoice(invoice_id))
            .expect("Recurring invoice not found")
    }

}

    /// Admin sets global referral terms.
    pub fn set_referral_config(env: Env, admin: Address, commission_bps: u32, window_ledgers: u32) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("Not initialized");
        if admin != stored_admin { panic!("Only admin can set referral config"); }
        env.storage().instance().set(&DataKey::ReferralCommissionBps, &commission_bps);
        env.storage().instance().set(&DataKey::ReferralWindowLedgers, &window_ledgers);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Existing merchant registers a referral for a new merchant.
    /// Fails if the referred address already has a merchant record.
    pub fn register_referral(env: Env, referrer: Address, referred_merchant: Address) {
        Self::require_not_paused(&env);
        referrer.require_auth();

        // Referrer must be an approved merchant
        let referrer_approved: bool = env.storage().persistent().get(&DataKey::MerchantApproved(referrer.clone())).unwrap_or(false);
        if !referrer_approved { panic!("Referrer is not an approved merchant"); }

        // Referred must not already have a merchant record
        let already_exists: bool = env.storage().persistent().get(&DataKey::MerchantApproved(referred_merchant.clone())).unwrap_or(false);
        if already_exists { panic_with_error!(&env, Error::ReferralAlreadyExists); }

        let window_ledgers: u32 = env.storage().instance().get(&DataKey::ReferralWindowLedgers).unwrap_or(0);
        let record = ReferralRecord {
            referrer: referrer.clone(),
            registered_at_ledger: env.ledger().sequence(),
            window_ledgers,
        };
        let key = DataKey::ReferralRecord(referred_merchant.clone());
        env.storage().persistent().set(&key, &record);
        env.storage().persistent().extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);

        events::emit_referral_registered(&env, referrer, referred_merchant);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Referrer withdraws accumulated commission to their address.
    pub fn claim_referral_commission(env: Env, referrer: Address, token: Address) {
        Self::require_not_paused(&env);
        referrer.require_auth();

        let key = DataKey::PendingCommission(referrer.clone());
        let pending: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if pending <= 0 { panic_with_error!(&env, Error::NoCommissionToClaim); }

        env.storage().persistent().set(&key, &0i128);
        env.storage().persistent().extend_ttl(&key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&env.current_contract_address(), &referrer, &pending);

        events::emit_commission_claimed(&env, referrer, pending);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get pending commission balance for a referrer.
    pub fn get_pending_commission(env: Env, referrer: Address) -> i128 {
        env.storage().persistent().get(&DataKey::PendingCommission(referrer)).unwrap_or(0)
    }

    /// Get the referral record for a referred merchant.
    pub fn get_referral_record(env: Env, referred_merchant: Address) -> Option<ReferralRecord> {
        env.storage().persistent().get(&DataKey::ReferralRecord(referred_merchant))
    }

    /// Internal: accrue referral commission when a referred merchant's payment is finalized.
    fn accrue_referral_commission(env: &Env, merchant: &Address, payment_id: u32, fee_amount: i128) {
        if fee_amount <= 0 { return; }

        let record_opt: Option<ReferralRecord> = env.storage().persistent().get(&DataKey::ReferralRecord(merchant.clone()));
        let record = match record_opt {
            Some(r) => r,
            None => return,
        };

        // Check window has not expired
        let current_ledger = env.ledger().sequence();
        if record.window_ledgers > 0 && current_ledger > record.registered_at_ledger.saturating_add(record.window_ledgers) {
            return;
        }

        let commission_bps: u32 = env.storage().instance().get(&DataKey::ReferralCommissionBps).unwrap_or(0);
        if commission_bps == 0 { return; }

        let commission = (fee_amount * commission_bps as i128) / 10_000;
        if commission <= 0 { return; }

        let pending_key = DataKey::PendingCommission(record.referrer.clone());
        let current: i128 = env.storage().persistent().get(&pending_key).unwrap_or(0);
        let new_total = current.saturating_add(commission);
        env.storage().persistent().set(&pending_key, &new_total);
        env.storage().persistent().extend_ttl(&pending_key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);

        events::emit_commission_accrued(env, record.referrer, merchant.clone(), payment_id, commission);
    }

}

#[cfg(test)]
mod test_token_whitelist;

#[cfg(test)]
mod test_collateral;

#[cfg(test)]
mod test_notification_keys;

#[cfg(test)]
mod test_external_id_multisig_voucher;

#[cfg(test)]
mod test_merchant_ban;

#[cfg(test)]
mod test_loyalty_points;
mod test_referral;
mod test_dynamic_settlement;
mod test_spending_limit;

pub use events::*;
