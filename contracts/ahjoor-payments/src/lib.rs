#![no_std]
use soroban_sdk::xdr::ToXdr;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, panic_with_error, token, Address, Bytes,
    BytesN, Env, Map, String, Symbol, Vec,
};

/// Maximum length (bytes) for the optional payment reference string.
const MAX_REFERENCE_LEN: u32 = 64;
/// Maximum number of entries in the optional metadata map.
const MAX_METADATA_KEYS: u32 = 5;
/// Maximum length (bytes) for each metadata key or value.
const MAX_METADATA_KEY_LEN: u32 = 32;

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
}

/// Direction for oracle price condition (#125)
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OracleDirection {
    Gte = 0,
    Lte = 1,
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
    /// Optional oracle price condition required for completion (#125)
    pub release_condition: Option<OracleCondition>,
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
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RateLimitConfig {
    pub max_payments: u32,
    pub window_size_ledgers: u32,
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
    // --- Temporary ---
    Dispute(u32),
    /// Temporary: idempotency key → payment_id mapping (expires after 24h)
    IdempotencyKey(BytesN<32>),
}

mod events;

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

        // Merchant allowlist check (#58)
        Self::require_merchant_approved(&env, &merchant);

        let client = token::Client::new(&env, &token);
        client.transfer(&customer, &env.current_contract_address(), &amount);

        let timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PaymentTimeout)
            .unwrap_or(DEFAULT_PAYMENT_TIMEOUT);
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
            release_condition: None,
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
                release_condition: None,
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
    ///      more than `slippage_bps` basis points from the oracle rate.
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
        slippage_bps: u32,
    ) -> u32 {
        Self::require_not_paused(&env);
        if amount_usdc <= 0 {
            panic!("Payment amount must be positive");
        }
        if slippage_bps > 10_000 {
            panic!("slippage_bps cannot exceed 10000");
        }

        let usdc_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::UsdcToken)
            .expect("Oracle not configured");

        // --- Fallback: direct USDC payment, no oracle needed ---
        if payment_token == usdc_token {
            customer.require_auth();
            Self::enforce_rate_limit(&env, &customer, 1);
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
                release_condition: None,
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
            release_condition: None,
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

    pub fn get_customer_payments(env: Env, customer: Address) -> Vec<u32> {
        env.storage()
            .persistent()
            .get(&DataKey::CustomerPayments(customer))
            .unwrap_or(Vec::new(&env))
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
        Self::require_not_paused(&env);
        subscriber.require_auth();
        if amount <= 0 {
            panic!("Subscription amount must be positive");
        }
        if interval_seconds == 0 {
            panic!("Interval must be positive");
        }

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

        let sub = Subscription {
            id: sub_id,
            subscriber,
            merchant,
            amount,
            token,
            interval_seconds,
            last_charged_at: 0,
            max_charges,
            charges_count: 0,
            active: true,
            paused: false,
            paused_at: 0,
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
        sub_id
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
        if sub.last_charged_at > 0 && now < sub.last_charged_at + sub.interval_seconds {
            panic!("Interval has not elapsed");
        }

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

    /// Shared settlement logic: checks conditions, deducts fees, distributes funds,
    /// updates stats, emits events, and stores receipt. `payment` is mutated in-place.
    fn finalize_payment(env: &Env, payment_id: u32, payment: &mut Payment) {
        // Oracle price condition check (#125)
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

        let rolling_before = Self::rolling_merchant_volume(env, &payment.merchant);
        let projected_volume = rolling_before + payment.amount;
        let applied_fee_bps = Self::fee_bps_for_volume(env, projected_volume);
        let fee_amount = (payment.amount * applied_fee_bps as i128) / 10_000;
        let net_amount = payment.amount - fee_amount;

        let fee_recipient: Address = env
            .storage()
            .instance()
            .get(&DataKey::FeeRecipient)
            .expect("Fee recipient not configured");

        let token_client = token::Client::new(env, &payment.token);

        if fee_amount > 0 {
            token_client.transfer(&env.current_contract_address(), &fee_recipient, &fee_amount);
            events::emit_fee_collected(
                env,
                payment_id,
                fee_amount,
                fee_recipient,
                payment.token.clone(),
            );
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
    }

    fn inc_merchant_completed(env: &Env, merchant: &Address, token: &Address, amount: i128) {
        let mut s = Self::load_merchant_stats(env, merchant);
        s.payments_completed += 1;
        let prev = s.volume_completed.get(token.clone()).unwrap_or(0);
        s.volume_completed.set(token.clone(), prev + amount);
        Self::save_merchant_stats(env, merchant, &s);
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
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_collateral;

pub use events::*;
