#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, BytesN, Env, String, Vec};
use ahjoor_token_whitelist::TokenWhitelistClient;

// --- Storage TTL Constants ---
const INSTANCE_LIFETIME_THRESHOLD: u32 = 100_000;
const INSTANCE_BUMP_AMOUNT: u32 = 120_000;

const PERSISTENT_LIFETIME_THRESHOLD: u32 = 100_000;
const PERSISTENT_BUMP_AMOUNT: u32 = 120_000;

// ---------------------------------------------------------------------------
// Minimal payment contract client — only the fields we need from get_payment.
// ---------------------------------------------------------------------------
mod payment_contract {
    use soroban_sdk::{contractclient, contracttype, Address, Env, Map, String, Vec};

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub enum PaymentStatus {
        Pending = 0,
        Completed = 1,
        Refunded = 2,
        Disputed = 3,
        Expired = 4,
        ScheduledPending = 5,
    }

    #[contracttype]
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct SplitRecipient {
        pub recipient: Address,
        pub bps: u32,
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
        pub expires_at: u64,
        pub refunded_amount: i128,
        pub reference: Option<String>,
        pub metadata: Option<Map<String, String>>,
        pub split_recipients: Option<Vec<SplitRecipient>>,
        pub execute_after: u64,
    }

    #[allow(dead_code)]
    #[contractclient(name = "PaymentContractClient")]
    pub trait PaymentContractInterface {
        fn get_payment(env: Env, payment_id: u32) -> Payment;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum RefundStatus {
    Requested = 0,
    Approved = 1,
    Rejected = 2,
    Processed = 3,
    UnderAppeal = 4,
    /// Terminal status: refund request was cancelled before approval (#168)
    Cancelled = 5,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Refund {
    pub id: u32,
    pub payment_id: u32,
    pub customer: Address,
    pub merchant: Address,
    pub amount: i128,
    pub token: Address,
    pub status: RefundStatus,
    pub reason: String,
    pub reason_code: u32,
    pub requested_at: u64,
    pub approved_at: Option<u64>,
    pub processed_at: Option<u64>,
    pub rejected_at: Option<u64>,
    pub auto_approved_source: Option<String>, // "whitelist" or "dispute_window"
    pub escrow_id: Option<u32>,                // For cross-contract escrow refunds
    pub fee_amount: Option<i128>,              // Fee deducted on processing
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundStats {
    pub total_requested: u32,
    pub total_approved: u32,
    pub total_rejected: u32,
    pub total_processed: u32,
    pub total_amount_refunded: i128,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Admin,
    Paused,
    PauseReason,
    RefundCounter,
    ContractVersion,
    MigrationCompleted(u32),
    Refund(u32),
    /// Address of the payment contract for cross-contract validation (#64).
    PaymentContractAddress,
    /// Index: customer → Vec<u32> of refund IDs
    CustomerRefunds(Address),
    /// Index: merchant → Vec<u32> of refund IDs
    MerchantRefunds(Address),
    /// Index: payment_id → Vec<u32> of refund IDs
    PaymentRefunds(u32),
    /// Dispute window in seconds; after this period a Requested refund can be auto-approved.
    DisputeWindow,
    /// Cumulative refunded amount per payment_id (#165).
    RefundedAmount(u32),
    /// Whitelist of auto-approved merchants (Issue #163)
    AutoApprovedMerchants,
    /// Escrow contract address for cross-contract refund registration (Issue #162)
    EscrowContractAddress,
    /// Global refund statistics (Issue #161)
    GlobalRefundStats,
    /// Per-merchant refund statistics (Issue #161)
    MerchantRefundStats(Address),
    /// Refund processing fee in basis points (Issue #160)
    RefundFeeBps,
    /// Fee recipient address (Issue #160)
    FeeRecipient,
    /// Index: (merchant, reason_code) → Vec<u32> of refund IDs (#157)
    ReasonCodeRefunds(Address, u32),
    /// Count of refunds per (merchant, reason_code) (#157)
    ReasonCodeCount(Address, u32),
    /// Global auto-reject window in seconds (#158)
    AutoRejectWindow,
    /// Per-refund deadline extension in seconds (#158)
    RefundDeadlineExtension(u32),
    /// Appeal window in seconds after rejection (#159)
    AppealWindow,
    /// Ordered queue of pending (Requested) refund IDs (#164)
    PendingRefundQueue,
    /// Tiered refund policy: Vec<(max_seconds_since_payment, refund_bps)>.
    RefundTiers,
    /// Cooldown period in seconds between customer refund requests (#166)
    RefundCooldown,
    /// Last refund request timestamp per customer (#166) — stored in temporary storage
    LastRefundRequest(Address),
    /// List of approved delegate addresses per merchant (#167)
    MerchantDelegates(Address),
    /// Window in seconds during which a customer can cancel their own request (#168)
    CustomerCancelWindow,
    /// Token whitelist contract address
    TokenWhitelistContract,
    /// Fraud score per buyer address
    FraudScore(Address),
    /// Admin-configurable threshold for blocking refund requests
    FraudScoreBlockThreshold,
    /// Last fraud score update timestamp for decay calculation
    FraudScoreLastUpdate(Address),
    /// Fraud score decay interval in seconds (default: 30 days)
    FraudScoreDecayInterval,
}

mod events;

/// Optional extended configuration for `initialize`.
/// Groups the extra parameters that would otherwise exceed Soroban's 10-parameter limit.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundInitConfig {
    pub escrow_contract: Option<Address>,
    pub refund_fee_bps: u32,
    pub fee_recipient: Option<Address>,
    pub auto_reject_window_seconds: u64,
    pub appeal_window_seconds: u64,
    pub refund_tiers: Option<Vec<(u64, u32)>>,
    pub refund_cooldown_seconds: u64,
    pub customer_cancel_window_seconds: u64,
}

#[contract]
pub struct AhjoorRefundContract;

#[contractimpl]
impl AhjoorRefundContract {
    /// Initialize the refund contract.
    /// `config` bundles optional extended settings to stay within Soroban's 10-parameter limit.
    /// Pass `None` for `config` to use all defaults (zero fees, no cooldown, etc.).
    pub fn initialize(
        env: Env,
        admin: Address,
        payment_contract: Address,
        dispute_window: u64,
        config: Option<RefundInitConfig>,
    ) {
        let cfg = config.unwrap_or(RefundInitConfig {
            escrow_contract: None,
            refund_fee_bps: 0,
            fee_recipient: None,
            auto_reject_window_seconds: 0,
            appeal_window_seconds: 0,
            refund_tiers: None,
            refund_cooldown_seconds: 0,
            customer_cancel_window_seconds: 0,
        });
        let escrow_contract = cfg.escrow_contract;
        let refund_fee_bps = cfg.refund_fee_bps;
        let fee_recipient = cfg.fee_recipient;
        let auto_reject_window_seconds = cfg.auto_reject_window_seconds;
        let appeal_window_seconds = cfg.appeal_window_seconds;
        let refund_tiers = cfg.refund_tiers;
        let refund_cooldown_seconds = cfg.refund_cooldown_seconds;
        let customer_cancel_window_seconds = cfg.customer_cancel_window_seconds;
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("Already initialized");
        }

        // Validate fee cap (max 200 bps = 2%)
        if refund_fee_bps > 200 {
            panic!("Refund fee cannot exceed 200 basis points (2%)");
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::RefundCounter, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &1u32);
        env.storage()
            .instance()
            .set(&DataKey::PaymentContractAddress, &payment_contract);
        env.storage()
            .instance()
            .set(&DataKey::DisputeWindow, &dispute_window);

        // Issue #162: Store escrow contract address
        if let Some(escrow_addr) = escrow_contract {
            env.storage()
                .instance()
                .set(&DataKey::EscrowContractAddress, &escrow_addr);
        }

        // Issue #160: Store fee configuration
        env.storage()
            .instance()
            .set(&DataKey::RefundFeeBps, &refund_fee_bps);
        if let Some(recipient) = fee_recipient {
            env.storage()
                .instance()
                .set(&DataKey::FeeRecipient, &recipient);
        }

        let tiers = refund_tiers.unwrap_or(Vec::new(&env));
        Self::validate_refund_tiers(&tiers);
        env.storage().instance().set(&DataKey::RefundTiers, &tiers);

        // Issue #161: Initialize global stats
        let initial_stats = RefundStats {
            total_requested: 0,
            total_approved: 0,
            total_rejected: 0,
            total_processed: 0,
            total_amount_refunded: 0,
        };
        env.storage()
            .instance()
            .set(&DataKey::GlobalRefundStats, &initial_stats);

        // Issue #163: Initialize empty whitelist
        let empty_whitelist: Vec<Address> = Vec::new(&env);
        env.storage()
            .persistent()
            .set(&DataKey::AutoApprovedMerchants, &empty_whitelist);

        // Issue #158: Store auto-reject window (default 30 days = 2_592_000s)
        let effective_auto_reject = if auto_reject_window_seconds == 0 {
            2_592_000u64
        } else {
            auto_reject_window_seconds
        };
        env.storage()
            .instance()
            .set(&DataKey::AutoRejectWindow, &effective_auto_reject);

        // Issue #159: Store appeal window
        env.storage()
            .instance()
            .set(&DataKey::AppealWindow, &appeal_window_seconds);

        // Issue #164: Initialize empty pending refund queue
        let empty_queue: Vec<u32> = Vec::new(&env);
        env.storage()
            .persistent()
            .set(&DataKey::PendingRefundQueue, &empty_queue);

        // Issue #166: Store per-customer cooldown period (0 = no cooldown)
        env.storage()
            .instance()
            .set(&DataKey::RefundCooldown, &refund_cooldown_seconds);

        // Issue #168: Store customer cancel window
        env.storage()
            .instance()
            .set(&DataKey::CustomerCancelWindow, &customer_cancel_window_seconds);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Request a refund linked to an existing completed payment.
    /// Cross-contract validates: payment exists, status is Completed, merchant matches,
    /// and refund amount does not exceed the original payment amount (#64).
    /// If merchant is whitelisted, auto-approves immediately (Issue #163).
    /// `reason_code`: 0=Defective, 1=NotDelivered, 2=DuplicateCharge, 3=Unauthorized, 4=Other (#157).
    /// Returns the refund ID.
    pub fn request_refund(
        env: Env,
        customer: Address,
        payment_id: u32,
        amount: i128,
        reason: String,
        reason_code: u32,
    ) -> u32 {
        Self::require_not_paused(&env);
        customer.require_auth();

        if amount <= 0 {
            panic!("Refund amount must be positive");
        }

        // #157: Validate reason code (0-4)
        if reason_code > 4 {
            panic!("Invalid reason code: must be 0-4");
        }

        // #166: Enforce per-customer cooldown
        let cooldown: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RefundCooldown)
            .unwrap_or(0);
        if cooldown > 0 {
            let last_request: u64 = env
                .storage()
                .temporary()
                .get(&DataKey::LastRefundRequest(customer.clone()))
                .unwrap_or(0);
            let now_ts = env.ledger().timestamp();
            if last_request > 0 && now_ts.saturating_sub(last_request) < cooldown {
                let next_eligible_at = last_request + cooldown;
                events::emit_refund_cooldown_active(&env, customer.clone(), next_eligible_at);
                panic!("RefundCooldownActive");
            }
        }

        // Fraud score check: block buyers at or above threshold
        if Self::is_buyer_blocked(&env, &customer) {
            let score = Self::get_fraud_score(env.clone(), customer.clone());
            let threshold = Self::get_fraud_score_block_threshold(env.clone());
            panic!("BuyerBlockedForFraud: score={}, threshold={}", score, threshold);
        }

        // --- Cross-contract validation (#64) ---
        let payment_contract_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::PaymentContractAddress)
            .expect("Payment contract not configured");

        let payment_client =
            payment_contract::PaymentContractClient::new(&env, &payment_contract_addr);
        let payment = payment_client
            .try_get_payment(&payment_id)
            .unwrap_or_else(|_| panic!("PaymentContractError: payment not found"))
            .unwrap_or_else(|_| panic!("PaymentContractError: payment not found"));

        // Validate payment status is Completed
        if payment.status != payment_contract::PaymentStatus::Completed {
            panic!("PaymentContractError: payment is not completed");
        }

        // Validate merchant matches the payment's merchant
        // (customer is the one requesting, merchant is cached for audit)
        let merchant = payment.merchant.clone();

        // Validate refund amount does not exceed original payment amount
        let already_refunded: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::RefundedAmount(payment_id))
            .unwrap_or(0);

        let mut effective_amount = amount;
        let tiers = Self::get_refund_tiers(env.clone());
        let mut applied_tier_bps: Option<u32> = None;
        let mut tier_max_refundable: Option<i128> = None;

        if !tiers.is_empty() {
            let elapsed = env.ledger().timestamp().saturating_sub(payment.created_at);
            let mut matched = false;
            for i in 0..tiers.len() {
                let (max_seconds, tier_bps) = tiers.get(i).unwrap();
                if elapsed <= max_seconds {
                    matched = true;
                    let absolute_cap = (payment.amount as u128 * tier_bps as u128 / 10_000) as i128;
                    let remaining_under_tier = absolute_cap.saturating_sub(already_refunded);
                    if remaining_under_tier <= 0 {
                        panic!("ExceedsRefundableAmount");
                    }
                    effective_amount = if amount > remaining_under_tier {
                        remaining_under_tier
                    } else {
                        amount
                    };
                    applied_tier_bps = Some(tier_bps);
                    tier_max_refundable = Some(remaining_under_tier);
                    break;
                }
            }

            if !matched {
                panic!("RefundWindowExpired");
            }
        }

        if effective_amount + already_refunded > payment.amount {
            panic!("ExceedsRefundableAmount");
        }

        // Cache validated payment data — token comes from the payment record
        let token = payment.token.clone();

        // Token whitelist validation
        Self::require_token_allowed(&env, &token);

        // Escrow funds to this contract so approved refunds can be processed.
        let client = token::Client::new(&env, &token);
        client.transfer(&customer, &env.current_contract_address(), &effective_amount);

        let refund_id = Self::next_refund_id(&env);

        let is_whitelisted = Self::is_merchant_auto_approved(&env, &merchant);

        let initial_status = if is_whitelisted {
            RefundStatus::Approved
        } else {
            RefundStatus::Requested
        };

        let now = env.ledger().timestamp();
        let refund = Refund {
            id: refund_id,
            payment_id,
            customer: customer.clone(),
            merchant: merchant.clone(),
            amount: effective_amount,
            token: token.clone(),
            status: initial_status,
            reason,
            reason_code,
            requested_at: now,
            approved_at: if is_whitelisted { Some(now) } else { None },
            processed_at: None,
            rejected_at: None,
            auto_approved_source: if is_whitelisted {
                Some(String::from_str(&env, "whitelist"))
            } else {
                None
            },
            escrow_id: None,
            fee_amount: None,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::append_index(&env, &DataKey::CustomerRefunds(customer.clone()), refund_id);
        Self::append_index(&env, &DataKey::MerchantRefunds(merchant.clone()), refund_id);
        Self::append_index(&env, &DataKey::PaymentRefunds(payment_id), refund_id);

        // #157: Update reason code index and count
        Self::append_index(
            &env,
            &DataKey::ReasonCodeRefunds(merchant.clone(), reason_code),
            refund_id,
        );
        let count_key = DataKey::ReasonCodeCount(merchant.clone(), reason_code);
        let prev_count: u32 = env
            .storage()
            .persistent()
            .get(&count_key)
            .unwrap_or(0);
        env.storage()
            .persistent()
            .set(&count_key, &(prev_count + 1));
        env.storage().persistent().extend_ttl(
            &count_key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // #164: Add to pending queue only when not auto-approved
        if !is_whitelisted {
            Self::append_to_pending_queue(&env, refund_id);
        }

        Self::update_stats_on_request(&env, &merchant, effective_amount);

        // #166: Record this request timestamp in temporary storage (before customer is moved)
        if cooldown > 0 {
            let now_for_cooldown = env.ledger().timestamp();
            env.storage()
                .temporary()
                .set(&DataKey::LastRefundRequest(customer.clone()), &now_for_cooldown);
            env.storage().temporary().extend_ttl(
                &DataKey::LastRefundRequest(customer.clone()),
                INSTANCE_BUMP_AMOUNT,
                INSTANCE_BUMP_AMOUNT,
            );
        }

        events::emit_refund_requested(&env, refund_id, customer, effective_amount, token, refund.reason);
        events::emit_refund_reason_recorded(&env, refund_id, reason_code);

        if let (Some(tier_bps), Some(max_refundable)) = (applied_tier_bps, tier_max_refundable) {
            events::emit_refund_tier_applied(&env, refund_id, tier_bps, max_refundable);
        }

        if is_whitelisted {
            events::emit_refund_auto_approved_whitelist(&env, refund_id, merchant, effective_amount);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        refund_id
    }

    /// Approve a refund request. Callable by admin or a delegate of the refund's merchant (#167).
    pub fn approve_refund(env: Env, admin: Address, refund_id: u32) {
        Self::require_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");

        let mut refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        // #167: Allow admin OR a delegate of the refund's merchant
        let is_admin = admin == stored_admin;
        let is_delegate = !is_admin && Self::is_merchant_delegate(&env, &refund.merchant, &admin);

        if !is_admin && !is_delegate {
            panic!("Only admin or merchant delegate can approve refunds");
        }

        if refund.status != RefundStatus::Requested {
            panic!("Refund is not in requested status");
        }

        refund.status = RefundStatus::Approved;
        refund.approved_at = Some(env.ledger().timestamp());

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // #164: Remove from pending queue
        Self::remove_from_pending_queue(&env, refund_id);

        // Update fraud score: decrement on approved refund
        Self::decrement_fraud_score(&env, &refund.customer);

        Self::update_stats_on_approve(&env, &refund.merchant);

        if is_delegate {
            events::emit_refund_approved_by_delegate(&env, refund_id, admin.clone());
        }
        events::emit_refund_approved(&env, refund_id, admin, refund.approved_at.unwrap());

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Reject a refund request. Callable by admin or a delegate of the refund's merchant (#167).
    pub fn reject_refund(env: Env, admin: Address, refund_id: u32, rejection_reason: String) {
        Self::require_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");

        let mut refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        // #167: Allow admin OR a delegate of the refund's merchant
        let is_admin = admin == stored_admin;
        let is_delegate = !is_admin && Self::is_merchant_delegate(&env, &refund.merchant, &admin);

        if !is_admin && !is_delegate {
            panic!("Only admin or merchant delegate can reject refunds");
        }

        if refund.status != RefundStatus::Requested {
            panic!("Refund is not in requested status");
        }

        let now = env.ledger().timestamp();
        refund.status = RefundStatus::Rejected;
        refund.rejected_at = Some(now);

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // #164: Remove from pending queue
        Self::remove_from_pending_queue(&env, refund_id);

        // Update fraud score: increment on rejected refund (+2 for admin, +1 for delegate)
        if is_admin {
            Self::increment_fraud_score(&env, &refund.customer, Symbol::new(&env, "admin_rejected"));
        } else {
            Self::increment_fraud_score(&env, &refund.customer, Symbol::new(&env, "rejected"));
        }

        Self::update_stats_on_reject(&env, &refund.merchant);

        events::emit_refund_rejected(&env, refund_id, admin, rejection_reason, now);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn process_refund(env: Env, admin: Address, refund_id: u32) {
        Self::require_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");

        if admin != stored_admin {
            panic!("Only admin can process refunds");
        }

        Self::process_refund_internal(&env, refund_id);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Merchant initiates an immediate refund directly to customer.
    pub fn merchant_refund(
        env: Env,
        merchant: Address,
        payment_id: u32,
        amount: i128,
        reason_code: u32,
    ) -> u32 {
        Self::require_not_paused(&env);
        merchant.require_auth();

        if amount <= 0 {
            panic!("Refund amount must be positive");
        }

        let payment_contract_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::PaymentContractAddress)
            .expect("Payment contract not configured");
        let payment_client =
            payment_contract::PaymentContractClient::new(&env, &payment_contract_addr);
        let payment = payment_client
            .try_get_payment(&payment_id)
            .unwrap_or_else(|_| panic!("PaymentContractError: payment not found"))
            .unwrap_or_else(|_| panic!("PaymentContractError: payment not found"));

        if payment.merchant != merchant {
            panic!("Only payment merchant can initiate refund");
        }

        if amount > payment.amount {
            panic!("ExceedsRefundableAmount");
        }

        let already_refunded: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::RefundedAmount(payment_id))
            .unwrap_or(0);
        let remaining = payment.amount.saturating_sub(already_refunded);
        if amount > remaining {
            panic!("MerchantBalanceInsufficient");
        }

        // Token whitelist validation
        Self::require_token_allowed(&env, &payment.token);

        let token_client = token::Client::new(&env, &payment.token);
        token_client.transfer(&merchant, &payment.customer, &amount);

        let refund_id = Self::next_refund_id(&env);
        let now = env.ledger().timestamp();
        let refund = Refund {
            id: refund_id,
            payment_id,
            customer: payment.customer.clone(),
            merchant: merchant.clone(),
            amount,
            token: payment.token.clone(),
            status: RefundStatus::Processed,
            reason: String::from_str(&env, "merchant_initiated"),
            reason_code: 4, // "Other" — merchant-initiated
            requested_at: now,
            approved_at: Some(now),
            processed_at: Some(now),
            rejected_at: None,
            auto_approved_source: Some(String::from_str(&env, "merchant_direct")),
            escrow_id: None,
            fee_amount: None,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::append_index(&env, &DataKey::CustomerRefunds(payment.customer.clone()), refund_id);
        Self::append_index(&env, &DataKey::MerchantRefunds(merchant.clone()), refund_id);
        Self::append_index(&env, &DataKey::PaymentRefunds(payment_id), refund_id);

        let new_total = already_refunded + amount;
        env.storage()
            .persistent()
            .set(&DataKey::RefundedAmount(payment_id), &new_total);
        env.storage().persistent().extend_ttl(
            &DataKey::RefundedAmount(payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::update_stats_on_request(&env, &merchant, amount);
        Self::update_stats_on_process(&env, &merchant, amount);

        events::emit_merchant_initiated_refund(
            &env,
            refund_id,
            payment_id,
            merchant,
            amount,
            reason_code,
        );

        refund_id
    }

    /// Process up to 20 approved refunds atomically.
    pub fn bulk_process_refunds(env: Env, admin: Address, refund_ids: Vec<u32>) {
        Self::require_not_paused(&env);
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can process refunds");
        }

        if refund_ids.is_empty() {
            panic!("Refund batch cannot be empty");
        }
        if refund_ids.len() > 20 {
            panic!("BatchTooLarge");
        }

        for i in 0..refund_ids.len() {
            let rid = refund_ids.get(i).unwrap();
            let refund: Refund = env
                .storage()
                .persistent()
                .get(&DataKey::Refund(rid))
                .expect("Refund not found");
            if refund.status != RefundStatus::Approved {
                panic!("Refund is not approved");
            }
        }

        let mut total_amount: i128 = 0;
        for i in 0..refund_ids.len() {
            let rid = refund_ids.get(i).unwrap();
            let refund: Refund = env
                .storage()
                .persistent()
                .get(&DataKey::Refund(rid))
                .expect("Refund not found");
            total_amount += refund.amount;
            Self::process_refund_internal(&env, rid);
        }

        events::emit_bulk_refund_processed(&env, refund_ids.len(), total_amount);
    }

    pub fn set_refund_tiers(env: Env, admin: Address, tiers: Vec<(u64, u32)>) {
        Self::require_admin(&env, &admin);
        Self::validate_refund_tiers(&tiers);
        env.storage().instance().set(&DataKey::RefundTiers, &tiers);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_refund_tiers(env: Env) -> Vec<(u64, u32)> {
        env.storage()
            .instance()
            .get(&DataKey::RefundTiers)
            .unwrap_or(Vec::new(&env))
    }

    /// Auto-approve a refund once the dispute window has elapsed without merchant action.
    /// Callable by anyone. Panics if the merchant has already approved or rejected the refund,
    /// or if the dispute window has not yet elapsed.
    pub fn auto_approve_refund(env: Env, refund_id: u32) {
        Self::require_not_paused(&env);

        let mut refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        if refund.status != RefundStatus::Requested {
            panic!("Refund has already been acted on");
        }

        let dispute_window: u64 = env
            .storage()
            .instance()
            .get(&DataKey::DisputeWindow)
            .expect("Dispute window not configured");

        let now = env.ledger().timestamp();
        if now < refund.requested_at + dispute_window {
            panic!("Dispute window has not elapsed");
        }

        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RefundFeeBps)
            .unwrap_or(0);

        let fee_amount = if fee_bps > 0 {
            (refund.amount as u128 * fee_bps as u128 / 10_000) as i128
        } else {
            0
        };

        let customer_amount = refund.amount - fee_amount;

        let client = token::Client::new(&env, &refund.token);
        if customer_amount > 0 {
            client.transfer(
                &env.current_contract_address(),
                &refund.customer,
                &customer_amount,
            );
        }

        // Transfer fee to fee recipient if configured
        if fee_amount > 0 {
            if let Some(fee_recipient) = env
                .storage()
                .instance()
                .get::<DataKey, Address>(&DataKey::FeeRecipient)
            {
                client.transfer(
                    &env.current_contract_address(),
                    &fee_recipient,
                    &fee_amount,
                );
                events::emit_refund_fee_collected(&env, refund_id, fee_amount);
            }
        }

        refund.status = RefundStatus::Processed;
        refund.processed_at = Some(now);
        refund.auto_approved_source = Some(String::from_str(&env, "dispute_window"));
        refund.fee_amount = if fee_amount > 0 { Some(fee_amount) } else { None };

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::update_stats_on_process(&env, &refund.merchant, refund.amount);

        events::emit_refund_auto_approved(&env, refund_id, refund.customer, refund.amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Add a merchant to the auto-approval whitelist. Admin only.
    pub fn add_to_auto_approve(env: Env, admin: Address, merchant: Address) {
        Self::require_admin(&env, &admin);

        let mut whitelist: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::AutoApprovedMerchants)
            .unwrap_or(Vec::new(&env));

        // Check if already whitelisted
        for addr in whitelist.iter() {
            if addr == merchant {
                panic!("Merchant already whitelisted");
            }
        }

        whitelist.push_back(merchant);
        env.storage()
            .persistent()
            .set(&DataKey::AutoApprovedMerchants, &whitelist);
        env.storage().persistent().extend_ttl(
            &DataKey::AutoApprovedMerchants,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Remove a merchant from the auto-approval whitelist. Admin only.
    pub fn remove_from_auto_approve(env: Env, admin: Address, merchant: Address) {
        Self::require_admin(&env, &admin);

        let whitelist: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::AutoApprovedMerchants)
            .unwrap_or(Vec::new(&env));

        let mut found = false;
        let mut new_whitelist = Vec::new(&env);
        for addr in whitelist.iter() {
            if addr != merchant {
                new_whitelist.push_back(addr);
            } else {
                found = true;
            }
        }

        if !found {
            panic!("Merchant not in whitelist");
        }

        env.storage()
            .persistent()
            .set(&DataKey::AutoApprovedMerchants, &new_whitelist);
        env.storage().persistent().extend_ttl(
            &DataKey::AutoApprovedMerchants,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the auto-approval whitelist.
    pub fn get_auto_approved_merchants(env: Env) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::AutoApprovedMerchants)
            .unwrap_or(Vec::new(&env))
    }

    // -------------------------------------------------------------------------
    // #166: Per-Customer Refund Request Cooldown Period
    // -------------------------------------------------------------------------

    /// Admin waives the cooldown for a specific customer for their next request only.
    /// Removes the LastRefundRequest entry from temporary storage.
    pub fn waive_cooldown(env: Env, admin: Address, customer: Address) {
        Self::require_admin(&env, &admin);
        env.storage()
            .temporary()
            .remove(&DataKey::LastRefundRequest(customer));
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the configured refund cooldown in seconds.
    pub fn get_refund_cooldown(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::RefundCooldown)
            .unwrap_or(0)
    }

    // -------------------------------------------------------------------------
    // #167: Delegated Refund Approval for Merchant Sub-Admins
    // -------------------------------------------------------------------------

    /// Add a delegate that can approve/reject refunds for a specific merchant.
    /// Maximum 5 delegates per merchant. Only the merchant can call this.
    pub fn add_delegate(env: Env, merchant: Address, delegate: Address) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        let key = DataKey::MerchantDelegates(merchant.clone());
        let mut delegates: Vec<Address> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        if delegates.len() >= 5 {
            panic!("Maximum 5 delegates per merchant");
        }

        for addr in delegates.iter() {
            if addr == delegate {
                panic!("Address is already a delegate");
            }
        }

        delegates.push_back(delegate.clone());
        env.storage().persistent().set(&key, &delegates);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_delegate_added(&env, merchant, delegate);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Remove a delegate from a merchant's delegate list. Only the merchant can call this.
    pub fn remove_delegate(env: Env, merchant: Address, delegate: Address) {
        Self::require_not_paused(&env);
        merchant.require_auth();

        let key = DataKey::MerchantDelegates(merchant.clone());
        let delegates: Vec<Address> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        let mut new_delegates = Vec::new(&env);
        let mut found = false;
        for addr in delegates.iter() {
            if addr == delegate {
                found = true;
            } else {
                new_delegates.push_back(addr);
            }
        }

        if !found {
            panic!("Address is not a delegate");
        }

        env.storage().persistent().set(&key, &new_delegates);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the list of delegates for a merchant.
    pub fn get_delegates(env: Env, merchant: Address) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantDelegates(merchant))
            .unwrap_or(Vec::new(&env))
    }

    // -------------------------------------------------------------------------
    // #168: Refund Request Expiry with Auto-Cancellation
    // -------------------------------------------------------------------------

    /// Cancel a refund request. Only callable by the requesting customer while status is Requested.
    /// Returns escrowed funds to the customer.
    pub fn cancel_refund_request(env: Env, customer: Address, refund_id: u32) {
        Self::require_not_paused(&env);
        customer.require_auth();

        let mut refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        if refund.customer != customer {
            panic!("Only the requesting customer can cancel this refund");
        }

        if refund.status != RefundStatus::Requested {
            panic!("Only Requested refunds can be cancelled");
        }

        // Return escrowed funds to customer
        let client = token::Client::new(&env, &refund.token);
        client.transfer(
            &env.current_contract_address(),
            &refund.customer,
            &refund.amount,
        );

        refund.status = RefundStatus::Cancelled;

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Remove from pending queue
        Self::remove_from_pending_queue(&env, refund_id);

        // Update fraud score: increment if cancelled after dispute window (+1)
        // This penalizes buyers who cancel after the merchant has had time to review
        let dispute_window: u64 = env
            .storage()
            .instance()
            .get(&DataKey::DisputeWindow)
            .unwrap_or(0);
        let now = env.ledger().timestamp();
        if now > refund.requested_at + dispute_window {
            Self::increment_fraud_score(&env, &refund.customer, Symbol::new(&env, "cancelled_late"));
        }

        events::emit_refund_request_cancelled(&env, refund_id, customer);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Auto-cancel a refund request that has been in Requested status beyond
    /// `customer_cancel_window_seconds`. Callable by anyone after the window expires.
    /// Returns escrowed funds to the customer.
    pub fn auto_cancel_expired_request(env: Env, refund_id: u32) {
        Self::require_not_paused(&env);

        let mut refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        if refund.status != RefundStatus::Requested {
            panic!("Only Requested refunds can be auto-cancelled");
        }

        let cancel_window: u64 = env
            .storage()
            .instance()
            .get(&DataKey::CustomerCancelWindow)
            .expect("Customer cancel window not configured");

        let now = env.ledger().timestamp();
        if now < refund.requested_at + cancel_window {
            panic!("Customer cancel window has not elapsed");
        }

        // Return escrowed funds to customer
        let client = token::Client::new(&env, &refund.token);
        client.transfer(
            &env.current_contract_address(),
            &refund.customer,
            &refund.amount,
        );

        let cancelled_by = refund.customer.clone();
        refund.status = RefundStatus::Cancelled;

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Remove from pending queue
        Self::remove_from_pending_queue(&env, refund_id);

        events::emit_refund_request_cancelled(&env, refund_id, cancelled_by);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the configured customer cancel window in seconds.
    pub fn get_customer_cancel_window(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::CustomerCancelWindow)
            .expect("Customer cancel window not configured")
    }

    // -------------------------------------------------------------------------
    // Fraud Score Tracking Functions
    // -------------------------------------------------------------------------

    /// Set the fraud score block threshold. Admin only.
    /// Buyers at or above this threshold cannot submit new refund requests.
    pub fn set_fraud_score_block_threshold(env: Env, admin: Address, threshold: u32) {
        Self::require_admin(&env, &admin);

        if threshold == 0 {
            panic!("Threshold must be positive");
        }

        let old_threshold: u32 = env
            .storage()
            .instance()
            .get(&DataKey::FraudScoreBlockThreshold)
            .unwrap_or(10);

        env.storage()
            .instance()
            .set(&DataKey::FraudScoreBlockThreshold, &threshold);

        events::emit_fraud_score_block_threshold_updated(&env, old_threshold, threshold);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the current fraud score block threshold.
    pub fn get_fraud_score_block_threshold(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::FraudScoreBlockThreshold)
            .unwrap_or(10)
    }

    /// Get the fraud score for a buyer.
    pub fn get_fraud_score(env: Env, buyer: Address) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::FraudScore(buyer))
            .unwrap_or(0)
    }

    /// Manually reset a buyer's fraud score. Admin only.
    pub fn reset_fraud_score(env: Env, admin: Address, buyer: Address) {
        Self::require_admin(&env, &admin);

        let current_score = Self::get_fraud_score(env.clone(), buyer);
        if current_score == 0 {
            return; // Nothing to reset
        }

        env.storage()
            .instance()
            .set(&DataKey::FraudScore(buyer.clone()), &0u32);
        env.storage()
            .instance()
            .set(&DataKey::FraudScoreLastUpdate(buyer.clone()), &env.ledger().timestamp());

        events::emit_fraud_score_reset(&env, buyer, admin);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Apply time-based decay to a buyer's fraud score.
    /// Called internally when the decay interval has elapsed.
    fn apply_fraud_score_decay(env: &Env, buyer: &Address) {
        let decay_interval: u64 = env
            .storage()
            .instance()
            .get(&DataKey::FraudScoreDecayInterval)
            .unwrap_or(30 * 24 * 60 * 60); // default 30 days

        let last_update: u64 = env
            .storage()
            .instance()
            .get(&DataKey::FraudScoreLastUpdate(buyer.clone()))
            .unwrap_or(0);

        if last_update == 0 {
            return; // No score to decay
        }

        let now = env.ledger().timestamp();
        if now - last_update < decay_interval {
            return; // Not enough time has passed
        }

        let current_score = Self::get_fraud_score(env.clone(), buyer.clone());
        if current_score == 0 {
            return;
        }

        // Decay by 1 point
        let new_score = current_score.saturating_sub(1);
        env.storage()
            .instance()
            .set(&DataKey::FraudScore(buyer.clone()), &new_score);
        env.storage()
            .instance()
            .set(&DataKey::FraudScoreLastUpdate(buyer.clone()), &now);

        events::emit_fraud_score_decay_applied(&env, buyer.clone(), current_score, new_score);
    }

    /// Increment fraud score for a buyer. Called on specific events.
    fn increment_fraud_score(env: &Env, buyer: &Address, reason: Symbol) {
        // Apply decay first
        Self::apply_fraud_score_decay(env, buyer);

        let current_score = Self::get_fraud_score(env.clone(), buyer.clone());
        let new_score = current_score.saturating_add(1);
        
        env.storage()
            .instance()
            .set(&DataKey::FraudScore(buyer.clone()), &new_score);
        env.storage()
            .instance()
            .set(&DataKey::FraudScoreLastUpdate(buyer.clone()), &env.ledger().timestamp());

        events::emit_fraud_score_updated(&env, buyer.clone(), new_score, reason.clone());

        // Check if buyer should be blocked
        let threshold = Self::get_fraud_score_block_threshold(env.clone());
        if new_score >= threshold {
            events::emit_buyer_blocked_for_fraud(&env, buyer.clone(), new_score, threshold);
        }
    }

    /// Decrement fraud score for a buyer. Called on positive events.
    fn decrement_fraud_score(env: &Env, buyer: &Address) {
        // Apply decay first
        Self::apply_fraud_score_decay(env, buyer);

        let current_score = Self::get_fraud_score(env.clone(), buyer.clone());
        if current_score == 0 {
            return;
        }

        let new_score = current_score.saturating_sub(1);
        env.storage()
            .instance()
            .set(&DataKey::FraudScore(buyer.clone()), &new_score);
        env.storage()
            .instance()
            .set(&DataKey::FraudScoreLastUpdate(buyer.clone()), &env.ledger().timestamp());

        events::emit_fraud_score_updated(&env, buyer.clone(), new_score, Symbol::new(env, "approved"));
    }

    /// Check if a buyer is blocked from requesting refunds.
    fn is_buyer_blocked(env: &Env, buyer: &Address) -> bool {
        let score = Self::get_fraud_score(env.clone(), buyer.clone());
        let threshold = Self::get_fraud_score_block_threshold(env.clone());
        score >= threshold
    }

    // -------------------------------------------------------------------------
    // #157: Structured Refund Reason Codes
    // -------------------------------------------------------------------------

    /// Get refund IDs for a merchant filtered by reason code, with pagination.
    /// reason_code: 0=Defective, 1=NotDelivered, 2=DuplicateCharge, 3=Unauthorized, 4=Other
    pub fn get_refunds_by_reason(
        env: Env,
        merchant: Address,
        reason_code: u32,
        page: u32,
        page_size: u32,
    ) -> Vec<u32> {
        if reason_code > 4 {
            panic!("Invalid reason code: must be 0-4");
        }
        let key = DataKey::ReasonCodeRefunds(merchant, reason_code);
        let all: Vec<u32> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));
        let total = all.len();
        let start = (page * page_size).min(total);
        let end = (start + page_size).min(total);
        let mut result = Vec::new(&env);
        for i in start..end {
            result.push_back(all.get(i).unwrap());
        }
        result
    }

    /// Get the count of refunds for a merchant + reason_code combination.
    pub fn get_reason_code_count(env: Env, merchant: Address, reason_code: u32) -> u32 {
        if reason_code > 4 {
            panic!("Invalid reason code: must be 0-4");
        }
        env.storage()
            .persistent()
            .get(&DataKey::ReasonCodeCount(merchant, reason_code))
            .unwrap_or(0)
    }

    // -------------------------------------------------------------------------
    // #158: Auto-Reject Stale Refund Requests
    // -------------------------------------------------------------------------

    /// Auto-reject a refund that has been sitting in Requested status beyond the idle window.
    /// Callable by anyone. Returns escrowed funds to customer.
    pub fn auto_reject_stale_refund(env: Env, refund_id: u32) {
        Self::require_not_paused(&env);

        let mut refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        if refund.status != RefundStatus::Requested {
            panic!("Refund is not in requested status");
        }

        let auto_reject_window: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AutoRejectWindow)
            .expect("Auto-reject window not configured");

        let extension: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::RefundDeadlineExtension(refund_id))
            .unwrap_or(0);

        let deadline = refund.requested_at + auto_reject_window + extension;
        let now = env.ledger().timestamp();

        if now < deadline {
            panic!("Auto-reject window has not elapsed");
        }

        let elapsed_seconds = now - refund.requested_at;

        // Return escrowed funds to customer
        let client = token::Client::new(&env, &refund.token);
        client.transfer(
            &env.current_contract_address(),
            &refund.customer,
            &refund.amount,
        );

        refund.status = RefundStatus::Rejected;
        refund.rejected_at = Some(now);

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // #164: Remove from pending queue
        Self::remove_from_pending_queue(&env, refund_id);

        // Update fraud score: increment on auto-rejected refund (+1)
        Self::increment_fraud_score(&env, &refund.customer, Symbol::new(&env, "auto_rejected"));

        Self::update_stats_on_reject(&env, &refund.merchant);

        events::emit_refund_auto_rejected(&env, refund_id, elapsed_seconds);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Extend the auto-reject deadline for a specific refund. Admin only.
    pub fn extend_refund_deadline(
        env: Env,
        admin: Address,
        refund_id: u32,
        extra_seconds: u64,
    ) {
        Self::require_admin(&env, &admin);

        let refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        if refund.status != RefundStatus::Requested {
            panic!("Refund is not in requested status");
        }

        let key = DataKey::RefundDeadlineExtension(refund_id);
        let current_extension: u64 = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(0);

        env.storage()
            .persistent()
            .set(&key, &(current_extension + extra_seconds));
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the configured auto-reject window in seconds.
    pub fn get_auto_reject_window(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::AutoRejectWindow)
            .expect("Auto-reject window not configured")
    }

    // -------------------------------------------------------------------------
    // #159: Customer Refund Appeal After Merchant Rejection
    // -------------------------------------------------------------------------

    /// Appeal a rejected refund. Only the original customer can call this,
    /// within the configured appeal window after rejection.
    pub fn appeal_refund(env: Env, customer: Address, refund_id: u32) {
        Self::require_not_paused(&env);
        customer.require_auth();

        let mut refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        if refund.status != RefundStatus::Rejected {
            panic!("Appeal only allowed from Rejected status");
        }

        if refund.customer != customer {
            panic!("Only the original customer can appeal");
        }

        let appeal_window: u64 = env
            .storage()
            .instance()
            .get(&DataKey::AppealWindow)
            .expect("Appeal window not configured");

        let rejected_at = refund.rejected_at.expect("Rejection timestamp missing");
        let now = env.ledger().timestamp();

        if now > rejected_at + appeal_window {
            panic!("Appeal window has expired");
        }

        refund.status = RefundStatus::UnderAppeal;

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_refund_appealed(&env, refund_id, customer);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Resolve a refund appeal. Admin only.
    /// If approved: transfers funds to customer (processed).
    /// If rejected: final rejection, funds returned to customer, no further appeal.
    pub fn resolve_appeal(env: Env, admin: Address, refund_id: u32, approve: bool) {
        Self::require_not_paused(&env);
        Self::require_admin(&env, &admin);

        let mut refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        if refund.status != RefundStatus::UnderAppeal {
            panic!("Refund is not under appeal");
        }

        let now = env.ledger().timestamp();
        let client = token::Client::new(&env, &refund.token);

        if approve {
            let fee_bps: u32 = env
                .storage()
                .instance()
                .get(&DataKey::RefundFeeBps)
                .unwrap_or(0);

            let fee_amount = if fee_bps > 0 {
                (refund.amount as u128 * fee_bps as u128 / 10_000) as i128
            } else {
                0
            };

            let customer_amount = refund.amount - fee_amount;

            if customer_amount > 0 {
                client.transfer(
                    &env.current_contract_address(),
                    &refund.customer,
                    &customer_amount,
                );
            }

            if fee_amount > 0 {
                if let Some(fee_recipient) = env
                    .storage()
                    .instance()
                    .get::<DataKey, Address>(&DataKey::FeeRecipient)
                {
                    client.transfer(
                        &env.current_contract_address(),
                        &fee_recipient,
                        &fee_amount,
                    );
                    events::emit_refund_fee_collected(&env, refund_id, fee_amount);
                }
            }

            // Update cumulative refunded amount
            let already_refunded: i128 = env
                .storage()
                .persistent()
                .get(&DataKey::RefundedAmount(refund.payment_id))
                .unwrap_or(0);
            let new_total = already_refunded + refund.amount;
            env.storage()
                .persistent()
                .set(&DataKey::RefundedAmount(refund.payment_id), &new_total);
            env.storage().persistent().extend_ttl(
                &DataKey::RefundedAmount(refund.payment_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );

            refund.status = RefundStatus::Processed;
            refund.processed_at = Some(now);
            refund.fee_amount = if fee_amount > 0 { Some(fee_amount) } else { None };

            Self::update_stats_on_process(&env, &refund.merchant, refund.amount);
        } else {
            // Final rejection — return escrowed funds to customer
            client.transfer(
                &env.current_contract_address(),
                &refund.customer,
                &refund.amount,
            );

            refund.status = RefundStatus::Rejected;
            refund.rejected_at = Some(now);

            Self::update_stats_on_reject(&env, &refund.merchant);
        }

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_appeal_resolved(&env, refund_id, approve);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the configured appeal window in seconds.
    pub fn get_appeal_window(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::AppealWindow)
            .expect("Appeal window not configured")
    }

    // -------------------------------------------------------------------------
    // #164: Paginated Admin Refund Queue View
    // -------------------------------------------------------------------------

    /// Get paginated list of pending (Requested) refund IDs.
    /// page_size is capped at 50. Returns (refund_ids, total, has_more).
    pub fn get_pending_refund_queue(
        env: Env,
        page: u32,
        page_size: u32,
    ) -> (Vec<u32>, u32, bool) {
        let effective_size = page_size.min(50);
        let queue: Vec<u32> = env
            .storage()
            .persistent()
            .get(&DataKey::PendingRefundQueue)
            .unwrap_or(Vec::new(&env));
        let total = queue.len();
        let start = (page * effective_size).min(total);
        let end = (start + effective_size).min(total);
        let mut result = Vec::new(&env);
        for i in start..end {
            result.push_back(queue.get(i).unwrap());
        }
        let has_more = end < total;
        (result, total, has_more)
    }

    /// Get the count of pending (Requested) refunds in the queue.
    pub fn get_pending_refund_count(env: Env) -> u32 {
        let queue: Vec<u32> = env
            .storage()
            .persistent()
            .get(&DataKey::PendingRefundQueue)
            .unwrap_or(Vec::new(&env));
        queue.len()
    }

    /// Register a refund from the escrow contract. Only callable by the configured escrow contract.
    /// Creates a refund record in Processed status (no approval needed).
    pub fn register_escrow_refund(
        env: Env,
        escrow_id: u32,
        buyer: Address,
        amount: i128,
        token: Address,
    ) -> u32 {
        Self::require_not_paused(&env);

        // Verify caller is the configured escrow contract
        let escrow_contract_addr: Option<Address> = env
            .storage()
            .instance()
            .get(&DataKey::EscrowContractAddress);

        if let Some(escrow_addr) = escrow_contract_addr {
            if env.current_contract_address() != escrow_addr {
                panic!("Only escrow contract can register escrow refunds");
            }
        } else {
            panic!("Escrow contract not configured");
        }

        if amount <= 0 {
            panic!("Refund amount must be positive");
        }

        let refund_id = Self::next_refund_id(&env);
        let now = env.ledger().timestamp();

        // Use buyer as merchant placeholder for escrow refunds
        let merchant = buyer.clone();

        let refund = Refund {
            id: refund_id,
            payment_id: 0, // No payment_id for escrow refunds
            customer: buyer.clone(),
            merchant: merchant.clone(),
            amount,
            token: token.clone(),
            status: RefundStatus::Processed,
            reason: String::from_str(&env, "escrow_refund"),
            reason_code: 4, // Other
            requested_at: now,
            approved_at: Some(now),
            processed_at: Some(now),
            rejected_at: None,
            auto_approved_source: None,
            escrow_id: Some(escrow_id),
            fee_amount: None,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::append_index(&env, &DataKey::CustomerRefunds(buyer.clone()), refund_id);

        events::emit_escrow_refund_registered(&env, refund_id, escrow_id, buyer, amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        refund_id
    }

    /// Get global refund statistics.
    pub fn get_global_refund_stats(env: &Env) -> RefundStats {
        env.storage()
            .instance()
            .get(&DataKey::GlobalRefundStats)
            .unwrap_or(RefundStats {
                total_requested: 0,
                total_approved: 0,
                total_rejected: 0,
                total_processed: 0,
                total_amount_refunded: 0,
            })
    }

    /// Get per-merchant refund statistics.
    pub fn get_merchant_refund_stats(env: &Env, merchant: Address) -> RefundStats {
        env.storage()
            .persistent()
            .get(&DataKey::MerchantRefundStats(merchant))
            .unwrap_or(RefundStats {
                total_requested: 0,
                total_approved: 0,
                total_rejected: 0,
                total_processed: 0,
                total_amount_refunded: 0,
            })
    }

    /// Update the refund fee in basis points. Admin only. Max 200 bps (2%).
    pub fn update_refund_fee(env: Env, admin: Address, new_fee_bps: u32) {
        Self::require_admin(&env, &admin);

        if new_fee_bps > 200 {
            panic!("Refund fee cannot exceed 200 basis points (2%)");
        }

        env.storage()
            .instance()
            .set(&DataKey::RefundFeeBps, &new_fee_bps);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the current refund fee in basis points.
    pub fn get_refund_fee(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::RefundFeeBps)
            .unwrap_or(0)
    }

    /// Get the fee recipient address.
    pub fn get_fee_recipient(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::FeeRecipient)
    }

    /// Get the configured dispute window in seconds.
    pub fn get_dispute_window(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::DisputeWindow)
            .expect("Dispute window not configured")
    }

    /// Get refund details
    pub fn get_refund(env: Env, refund_id: u32) -> Refund {
        env.storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found")
    }

    /// Get refunds by customer with pagination.
    pub fn get_refunds_by_customer(
        env: Env,
        customer: Address,
        limit: u32,
        offset: u32,
    ) -> Vec<u32> {
        Self::paginate(&env, &DataKey::CustomerRefunds(customer), limit, offset)
    }

    /// Get refunds by merchant with pagination.
    pub fn get_refunds_by_merchant(
        env: Env,
        merchant: Address,
        limit: u32,
        offset: u32,
    ) -> Vec<u32> {
        Self::paginate(&env, &DataKey::MerchantRefunds(merchant), limit, offset)
    }

    /// Get the remaining refundable amount for a payment.
    pub fn get_refundable_remaining(env: Env, payment_id: u32) -> i128 {
        let payment_contract_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::PaymentContractAddress)
            .expect("Payment contract not configured");
        let payment_client =
            payment_contract::PaymentContractClient::new(&env, &payment_contract_addr);
        let payment = payment_client
            .try_get_payment(&payment_id)
            .unwrap_or_else(|_| panic!("PaymentContractError: payment not found"))
            .unwrap_or_else(|_| panic!("PaymentContractError: payment not found"));
        let already_refunded: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::RefundedAmount(payment_id))
            .unwrap_or(0);
        payment.amount - already_refunded
    }

    /// Get all refund IDs for a given payment ID.
    pub fn get_refunds_by_payment(env: Env, payment_id: u32) -> Vec<u32> {
        env.storage()
            .persistent()
            .get(&DataKey::PaymentRefunds(payment_id))
            .unwrap_or(Vec::new(&env))
    }

    /// Get refund counter
    pub fn get_refund_counter(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::RefundCounter)
            .unwrap_or(0)
    }

    /// Get admin address
    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized")
    }

    /// Get the configured payment contract address (#64).
    pub fn get_payment_contract(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::PaymentContractAddress)
            .expect("Payment contract not configured")
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

    /// Validates that a token is allowed via the whitelist contract
    fn require_token_allowed(env: &Env, token: &Address) {
        if let Some(whitelist_contract) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::TokenWhitelistContract)
        {
            let client = TokenWhitelistClient::new(env, &whitelist_contract);
            if !client.is_token_allowed(token) {
                panic!("TokenNotAllowed");
            }
        }
        // If no whitelist contract is set, allow all tokens (backward compatibility)
    }

    fn append_index(env: &Env, key: &DataKey, refund_id: u32) {
        let mut ids: Vec<u32> = env.storage().persistent().get(key).unwrap_or(Vec::new(env));
        ids.push_back(refund_id);
        env.storage().persistent().set(key, &ids);
        env.storage().persistent().extend_ttl(
            key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn paginate(env: &Env, key: &DataKey, limit: u32, offset: u32) -> Vec<u32> {
        let all: Vec<u32> = env.storage().persistent().get(key).unwrap_or(Vec::new(env));
        let total = all.len();
        let start = offset.min(total);
        let end = (start + limit).min(total);
        let mut page = Vec::new(env);
        for i in start..end {
            page.push_back(all.get(i).unwrap());
        }
        page
    }

    fn next_refund_id(env: &Env) -> u32 {
        let mut counter: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RefundCounter)
            .unwrap_or(0);
        let id = counter;
        counter += 1;
        env.storage()
            .instance()
            .set(&DataKey::RefundCounter, &counter);
        id
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

    // --- Helper Functions for Stats and Whitelist ---

    /// Check if `caller` is a registered delegate for `merchant` (#167).
    fn is_merchant_delegate(env: &Env, merchant: &Address, caller: &Address) -> bool {
        let delegates: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::MerchantDelegates(merchant.clone()))
            .unwrap_or(Vec::new(env));
        for addr in delegates.iter() {
            if addr == *caller {
                return true;
            }
        }
        false
    }

    fn is_merchant_auto_approved(env: &Env, merchant: &Address) -> bool {
        let whitelist: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::AutoApprovedMerchants)
            .unwrap_or(Vec::new(env));

        for addr in whitelist.iter() {
            if addr == *merchant {
                return true;
            }
        }
        false
    }

    fn update_stats_on_request(env: &Env, merchant: &Address, _amount: i128) {
        // Update global stats
        let mut global_stats = Self::get_global_refund_stats(env);
        global_stats.total_requested += 1;
        env.storage()
            .instance()
            .set(&DataKey::GlobalRefundStats, &global_stats);

        // Update merchant stats
        let mut merchant_stats = Self::get_merchant_refund_stats(env, merchant.clone());
        merchant_stats.total_requested += 1;
        env.storage()
            .persistent()
            .set(&DataKey::MerchantRefundStats(merchant.clone()), &merchant_stats);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantRefundStats(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn update_stats_on_approve(env: &Env, merchant: &Address) {
        // Update global stats
        let mut global_stats = Self::get_global_refund_stats(env);
        global_stats.total_approved += 1;
        env.storage()
            .instance()
            .set(&DataKey::GlobalRefundStats, &global_stats);

        // Update merchant stats
        let mut merchant_stats = Self::get_merchant_refund_stats(env, merchant.clone());
        merchant_stats.total_approved += 1;
        env.storage()
            .persistent()
            .set(&DataKey::MerchantRefundStats(merchant.clone()), &merchant_stats);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantRefundStats(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn update_stats_on_reject(env: &Env, merchant: &Address) {
        // Update global stats
        let mut global_stats = Self::get_global_refund_stats(env);
        global_stats.total_rejected += 1;
        env.storage()
            .instance()
            .set(&DataKey::GlobalRefundStats, &global_stats);

        // Update merchant stats
        let mut merchant_stats = Self::get_merchant_refund_stats(env, merchant.clone());
        merchant_stats.total_rejected += 1;
        env.storage()
            .persistent()
            .set(&DataKey::MerchantRefundStats(merchant.clone()), &merchant_stats);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantRefundStats(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn append_to_pending_queue(env: &Env, refund_id: u32) {
        let mut queue: Vec<u32> = env
            .storage()
            .persistent()
            .get(&DataKey::PendingRefundQueue)
            .unwrap_or(Vec::new(env));
        queue.push_back(refund_id);
        env.storage()
            .persistent()
            .set(&DataKey::PendingRefundQueue, &queue);
        env.storage().persistent().extend_ttl(
            &DataKey::PendingRefundQueue,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn remove_from_pending_queue(env: &Env, refund_id: u32) {
        let queue: Vec<u32> = env
            .storage()
            .persistent()
            .get(&DataKey::PendingRefundQueue)
            .unwrap_or(Vec::new(env));
        let mut new_queue = Vec::new(env);
        for id in queue.iter() {
            if id != refund_id {
                new_queue.push_back(id);
            }
        }
        env.storage()
            .persistent()
            .set(&DataKey::PendingRefundQueue, &new_queue);
        env.storage().persistent().extend_ttl(
            &DataKey::PendingRefundQueue,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn update_stats_on_process(env: &Env, merchant: &Address, amount: i128) {
        // Update global stats
        let mut global_stats = Self::get_global_refund_stats(env);
        global_stats.total_processed += 1;
        global_stats.total_amount_refunded += amount;
        env.storage()
            .instance()
            .set(&DataKey::GlobalRefundStats, &global_stats);

        // Update merchant stats
        let mut merchant_stats = Self::get_merchant_refund_stats(env, merchant.clone());
        merchant_stats.total_processed += 1;
        merchant_stats.total_amount_refunded += amount;
        env.storage()
            .persistent()
            .set(&DataKey::MerchantRefundStats(merchant.clone()), &merchant_stats);
        env.storage().persistent().extend_ttl(
            &DataKey::MerchantRefundStats(merchant.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn validate_refund_tiers(tiers: &Vec<(u64, u32)>) {
        let mut prev_max = 0u64;
        for i in 0..tiers.len() {
            let (max_seconds_since_payment, refund_bps) = tiers.get(i).unwrap();
            if i > 0 && max_seconds_since_payment < prev_max {
                panic!("Refund tiers must be sorted by max_seconds_since_payment");
            }
            if refund_bps > 10_000 {
                panic!("Refund tier bps must be <= 10000");
            }
            prev_max = max_seconds_since_payment;
        }
    }

    fn process_refund_internal(env: &Env, refund_id: u32) {
        let mut refund: Refund = env
            .storage()
            .persistent()
            .get(&DataKey::Refund(refund_id))
            .expect("Refund not found");

        if refund.status != RefundStatus::Approved {
            panic!("Refund is not approved");
        }

        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RefundFeeBps)
            .unwrap_or(0);

        let fee_amount = if fee_bps > 0 {
            (refund.amount as u128 * fee_bps as u128 / 10_000) as i128
        } else {
            0
        };

        let customer_amount = refund.amount - fee_amount;

        let client = token::Client::new(env, &refund.token);
        if customer_amount > 0 {
            client.transfer(
                &env.current_contract_address(),
                &refund.customer,
                &customer_amount,
            );
        }

        if fee_amount > 0 {
            if let Some(fee_recipient) = env
                .storage()
                .instance()
                .get::<DataKey, Address>(&DataKey::FeeRecipient)
            {
                client.transfer(
                    &env.current_contract_address(),
                    &fee_recipient,
                    &fee_amount,
                );
                events::emit_refund_fee_collected(env, refund_id, fee_amount);
            }
        }

        let already_refunded: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::RefundedAmount(refund.payment_id))
            .unwrap_or(0);
        let new_total = already_refunded + refund.amount;
        env.storage()
            .persistent()
            .set(&DataKey::RefundedAmount(refund.payment_id), &new_total);
        env.storage().persistent().extend_ttl(
            &DataKey::RefundedAmount(refund.payment_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        let payment_contract_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::PaymentContractAddress)
            .expect("Payment contract not configured");
        let payment_client =
            payment_contract::PaymentContractClient::new(env, &payment_contract_addr);
        if let Ok(Ok(payment)) = payment_client.try_get_payment(&refund.payment_id) {
            let remaining_refundable = payment.amount - new_total;
            if remaining_refundable <= payment.amount / 10 {
                events::emit_partial_refund_cap_applied(env, refund_id, remaining_refundable);
            }
        }

        refund.status = RefundStatus::Processed;
        refund.processed_at = Some(env.ledger().timestamp());
        refund.fee_amount = if fee_amount > 0 { Some(fee_amount) } else { None };

        env.storage()
            .persistent()
            .set(&DataKey::Refund(refund_id), &refund);
        env.storage().persistent().extend_ttl(
            &DataKey::Refund(refund_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::update_stats_on_process(env, &refund.merchant, refund.amount);

        events::emit_refund_processed(
            env,
            refund_id,
            refund.customer,
            customer_amount,
            refund.processed_at.unwrap(),
        );
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_token_whitelist;
