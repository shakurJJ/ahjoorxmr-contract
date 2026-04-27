use crate::{PaymentStatus, SplitTransfer};
use soroban_sdk::{contractevent, Address, BytesN, Env, String, Symbol, Vec};

/// Event: Payment receipt issued on completion (#65)
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentReceiptIssued {
    pub payment_id: u32,
    pub receipt_hash: BytesN<32>,
}

/// Event: Protocol fee collected on payment completion
#[contractevent]
#[derive(Clone, Debug)]
pub struct FeeCollected {
    pub payment_id: u32,
    pub fee_amount: i128,
    pub fee_recipient: Address,
    pub token: Address,
}

/// Event: Payment split distribution completed
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentSplitCompleted {
    pub payment_id: u32,
    pub splits: Vec<SplitTransfer>,
}

/// Event: Scheduled payment created
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentScheduled {
    pub payment_id: u32,
    pub execute_after: u64,
}

/// Event: Scheduled payment executed
#[contractevent]
#[derive(Clone, Debug)]
pub struct ScheduledPaymentExecuted {
    pub payment_id: u32,
}

/// Event: Merchant fee tier updated based on rolling 30-day volume
#[contractevent]
#[derive(Clone, Debug)]
pub struct MerchantTierUpdated {
    pub merchant: Address,
    pub new_tier_bps: u32,
    pub volume: i128,
}

/// Event: Multi-token payment created (customer paid in non-USDC token)
#[contractevent]
#[derive(Clone, Debug)]
pub struct MultiTokenPaymentCreated {
    pub payment_id: u32,
    pub customer: Address,
    pub merchant: Address,
    pub amount_usdc: i128,
    pub payment_token: Address,
    pub token_amount: i128,
    /// Oracle price used (scaled by 10^7)
    pub oracle_price: i128,
}

/// Event: Individual payment created
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentCreated {
    pub payment_id: u32,
    pub customer: Address,
    pub merchant: Address,
    pub amount: i128,
    pub token: Address,
    pub notification_key: soroban_sdk::Bytes,
}

/// Event: Batch payment operation completed
#[contractevent]
#[derive(Clone, Debug)]
pub struct BatchPaymentCreated {
    pub customer: Address,
    pub payment_count: u32,
    pub total_amount: i128,
}

/// Event: Payment status changed
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentStatusChanged {
    pub payment_id: u32,
    pub old_status: PaymentStatus,
    pub new_status: PaymentStatus,
}

/// Event: Payment completed (released from escrow to merchant)
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentCompleted {
    pub payment_id: u32,
    pub merchant: Address,
    pub amount: i128,
    pub completed_at: u64,
    pub notification_key: soroban_sdk::Bytes,
}

/// Event: Payment expired — funds returned to customer
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentExpired {
    pub payment_id: u32,
    pub customer: Address,
    pub amount: i128,
    pub expired_at: u64,
    pub notification_key: soroban_sdk::Bytes,
}

/// Event: Payment authorized by merchant — funds held in escrow (#127)
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentAuthorized {
    pub payment_id: u32,
    pub capture_deadline: u64,
}

/// Event: Payment created with a merchant-defined expiry override (#130)
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentExpiryOverride {
    pub payment_id: u32,
    pub expiry_seconds: u64,
}

/// Event: Authorized payment captured by merchant — funds released (#127)
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentCaptured {
    pub payment_id: u32,
}

/// Event: Partial refund issued on a pending/disputed payment
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentPartialRefund {
    pub payment_id: u32,
    pub customer: Address,
    pub refund_amount: i128,
    pub remaining: i128,
}

/// Event: Subscription charged
#[contractevent]
#[derive(Clone, Debug)]
pub struct SubscriptionCharged {
    pub subscription_id: u32,
    pub subscriber: Address,
    pub merchant: Address,
    pub amount: i128,
    pub charged_at: u64,
}

/// Event: Subscription cancelled
#[contractevent]
#[derive(Clone, Debug)]
pub struct SubscriptionCancelled {
    pub subscription_id: u32,
    pub cancelled_by: Address,
}

/// Event: Subscription created with a trial period (#133)
#[contractevent]
#[derive(Clone, Debug)]
pub struct SubscriptionTrialStarted {
    pub subscription_id: u32,
    pub trial_ends_at: u64,
}

/// Event: Subscription trial ended on first successful charge (#133)
#[contractevent]
#[derive(Clone, Debug)]
pub struct SubscriptionTrialEnded {
    pub subscription_id: u32,
}

/// Event: Merchant settlement batch processed.
#[contractevent]
#[derive(Clone, Debug)]
pub struct BatchSettlementProcessed {
    pub merchant: Address,
    pub total_amount: i128,
    pub fee_collected: i128,
    pub payment_count: u32,
}

/// Event: Payment disputed by customer
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentDisputed {
    pub payment_id: u32,
    pub customer: Address,
    pub reason: String,
    pub notification_key: soroban_sdk::Bytes,
}

/// Event: Dispute resolved by admin
#[contractevent]
#[derive(Clone, Debug)]
pub struct DisputeResolved {
    pub payment_id: u32,
    pub release_to_merchant: bool,
    pub resolved_by: Address,
}

/// Event: Dispute auto-escalated after timeout
#[contractevent]
#[derive(Clone, Debug)]
pub struct DisputeEscalated {
    pub payment_id: u32,
    pub elapsed_seconds: u64,
}

/// Event: Admin transfer proposed
#[contractevent]
#[derive(Clone, Debug)]
pub struct AdminTransferProposed {
    pub current_admin: Address,
    pub proposed_admin: Address,
}

/// Event: Admin transfer accepted
#[contractevent]
#[derive(Clone, Debug)]
pub struct AdminTransferred {
    pub old_admin: Address,
    pub new_admin: Address,
}

/// Event: Contract WASM upgraded
#[contractevent]
#[derive(Clone, Debug)]
pub struct ContractUpgraded {
    pub old_version: u32,
    pub new_version: u32,
    pub by_admin: Address,
}

/// Event: Contract paused
#[contractevent]
#[derive(Clone, Debug)]
pub struct ContractPaused {
    pub admin: Address,
    pub reason: String,
    pub timestamp: u64,
}

/// Event: Contract resumed
#[contractevent]
#[derive(Clone, Debug)]
pub struct ContractResumed {
    pub admin: Address,
    pub timestamp: u64,
}

/// Event: Payment queued for merchant withdrawal (#126)
#[contractevent]
#[derive(Clone, Debug)]
pub struct WithdrawalQueued {
    pub merchant: Address,
    pub payment_id: u32,
    pub amount: i128,
}

/// Event: Merchant withdrawal queue processed (#126)
#[contractevent]
#[derive(Clone, Debug)]
pub struct WithdrawalProcessed {
    pub merchant: Address,
    pub total: u32,
}

/// Event: Invoice attached to payment (#128)
#[contractevent]
#[derive(Clone, Debug)]
pub struct InvoiceAttached {
    pub payment_id: u32,
    pub invoice_hash: BytesN<32>,
}

// --- Helper Emission Functions ---

pub fn emit_payment_created(
    e: &Env,
    payment_id: u32,
    customer: Address,
    merchant: Address,
    amount: i128,
    token: Address,
) {
    // Get notification key for the merchant
    let notification_key = e
        .storage()
        .persistent()
        .get(&crate::DataKey::MerchantNotificationKey(merchant.clone()))
        .unwrap_or(soroban_sdk::Bytes::new(e));

    PaymentCreated {
        payment_id,
        customer,
        merchant,
        amount,
        token,
        notification_key,
    }
    .publish(e);
}

pub fn emit_batch_payment_created(
    e: &Env,
    customer: Address,
    payment_count: u32,
    total_amount: i128,
) {
    BatchPaymentCreated {
        customer,
        payment_count,
        total_amount,
    }
    .publish(e);
}

pub fn emit_payment_status_changed(
    e: &Env,
    payment_id: u32,
    old_status: PaymentStatus,
    new_status: PaymentStatus,
) {
    PaymentStatusChanged {
        payment_id,
        old_status,
        new_status,
    }
    .publish(e);
}

pub fn emit_payment_completed(
    e: &Env,
    payment_id: u32,
    merchant: Address,
    amount: i128,
    completed_at: u64,
) {
    // Get notification key for the merchant
    let notification_key = e
        .storage()
        .persistent()
        .get(&crate::DataKey::MerchantNotificationKey(merchant.clone()))
        .unwrap_or(soroban_sdk::Bytes::new(e));

    PaymentCompleted {
        payment_id,
        merchant,
        amount,
        completed_at,
        notification_key,
    }
    .publish(e);
}

pub fn emit_payment_expired(
    e: &Env,
    payment_id: u32,
    customer: Address,
    amount: i128,
    expired_at: u64,
) {
    // Get the payment to find the merchant for notification key lookup
    let payment: crate::Payment = e
        .storage()
        .persistent()
        .get(&crate::DataKey::Payment(payment_id))
        .expect("Payment not found");

    // Get notification key for the merchant
    let notification_key = e
        .storage()
        .persistent()
        .get(&crate::DataKey::MerchantNotificationKey(payment.merchant))
        .unwrap_or(soroban_sdk::Bytes::new(e));

    PaymentExpired {
        payment_id,
        customer,
        amount,
        expired_at,
        notification_key,
    }
    .publish(e);
}

pub fn emit_payment_authorized(e: &Env, payment_id: u32, capture_deadline: u64) {
    PaymentAuthorized {
        payment_id,
        capture_deadline,
    }
    .publish(e);
}

pub fn emit_payment_expiry_override(e: &Env, payment_id: u32, expiry_seconds: u64) {
    PaymentExpiryOverride {
        payment_id,
        expiry_seconds,
    }
    .publish(e);
}

pub fn emit_payment_captured(e: &Env, payment_id: u32) {
    PaymentCaptured { payment_id }.publish(e);
}

pub fn emit_payment_partial_refund(
    e: &Env,
    payment_id: u32,
    customer: Address,
    refund_amount: i128,
    remaining: i128,
) {
    PaymentPartialRefund {
        payment_id,
        customer,
        refund_amount,
        remaining,
    }
    .publish(e);
}

pub fn emit_subscription_charged(
    e: &Env,
    subscription_id: u32,
    subscriber: Address,
    merchant: Address,
    amount: i128,
    charged_at: u64,
) {
    SubscriptionCharged {
        subscription_id,
        subscriber,
        merchant,
        amount,
        charged_at,
    }
    .publish(e);
}

pub fn emit_subscription_cancelled(e: &Env, subscription_id: u32, cancelled_by: Address) {
    SubscriptionCancelled {
        subscription_id,
        cancelled_by,
    }
    .publish(e);
}

pub fn emit_subscription_trial_started(e: &Env, subscription_id: u32, trial_ends_at: u64) {
    SubscriptionTrialStarted {
        subscription_id,
        trial_ends_at,
    }
    .publish(e);
}

pub fn emit_subscription_trial_ended(e: &Env, subscription_id: u32) {
    SubscriptionTrialEnded { subscription_id }.publish(e);
}

pub fn emit_batch_settlement_processed(
    e: &Env,
    merchant: Address,
    total_amount: i128,
    fee_collected: i128,
    payment_count: u32,
) {
    BatchSettlementProcessed {
        merchant,
        total_amount,
        fee_collected,
        payment_count,
    }
    .publish(e);
}

pub fn emit_payment_disputed(e: &Env, payment_id: u32, customer: Address, reason: String) {
    // Get the payment to find the merchant for notification key lookup
    let payment: crate::Payment = e
        .storage()
        .persistent()
        .get(&crate::DataKey::Payment(payment_id))
        .expect("Payment not found");

    // Get notification key for the merchant
    let notification_key = e
        .storage()
        .persistent()
        .get(&crate::DataKey::MerchantNotificationKey(payment.merchant))
        .unwrap_or(soroban_sdk::Bytes::new(e));

    PaymentDisputed {
        payment_id,
        customer,
        reason,
        notification_key,
    }
    .publish(e);
}

pub fn emit_dispute_resolved(
    e: &Env,
    payment_id: u32,
    release_to_merchant: bool,
    resolved_by: Address,
) {
    DisputeResolved {
        payment_id,
        release_to_merchant,
        resolved_by,
    }
    .publish(e);
}

pub fn emit_dispute_escalated(e: &Env, payment_id: u32, elapsed_seconds: u64) {
    DisputeEscalated {
        payment_id,
        elapsed_seconds,
    }
    .publish(e);
}

pub fn emit_admin_transfer_proposed(e: &Env, current_admin: Address, proposed_admin: Address) {
    AdminTransferProposed {
        current_admin,
        proposed_admin,
    }
    .publish(e);
}

pub fn emit_admin_transferred(e: &Env, old_admin: Address, new_admin: Address) {
    AdminTransferred {
        old_admin,
        new_admin,
    }
    .publish(e);
}

pub fn emit_contract_upgraded(e: &Env, old_version: u32, new_version: u32, by_admin: Address) {
    ContractUpgraded {
        old_version,
        new_version,
        by_admin,
    }
    .publish(e);
}

pub fn emit_contract_paused(e: &Env, admin: Address, reason: String, timestamp: u64) {
    ContractPaused {
        admin,
        reason,
        timestamp,
    }
    .publish(e);
}

pub fn emit_contract_resumed(e: &Env, admin: Address, timestamp: u64) {
    ContractResumed { admin, timestamp }.publish(e);
}

pub fn emit_payment_receipt_issued(e: &Env, payment_id: u32, receipt_hash: BytesN<32>) {
    PaymentReceiptIssued {
        payment_id,
        receipt_hash,
    }
    .publish(e);
}

pub fn emit_fee_collected(
    e: &Env,
    payment_id: u32,
    fee_amount: i128,
    fee_recipient: Address,
    token: Address,
) {
    FeeCollected {
        payment_id,
        fee_amount,
        fee_recipient,
        token,
    }
    .publish(e);
}

pub fn emit_payment_split_completed(e: &Env, payment_id: u32, splits: Vec<SplitTransfer>) {
    PaymentSplitCompleted { payment_id, splits }.publish(e);
}

pub fn emit_payment_scheduled(e: &Env, payment_id: u32, execute_after: u64) {
    PaymentScheduled {
        payment_id,
        execute_after,
    }
    .publish(e);
}

pub fn emit_scheduled_payment_executed(e: &Env, payment_id: u32) {
    ScheduledPaymentExecuted { payment_id }.publish(e);
}

pub fn emit_merchant_tier_updated(e: &Env, merchant: Address, new_tier_bps: u32, volume: i128) {
    MerchantTierUpdated {
        merchant,
        new_tier_bps,
        volume,
    }
    .publish(e);
}

#[allow(clippy::too_many_arguments)]
/// Event: Payment tagged with a category (#122)
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentCategorized {
    pub payment_id: u32,
    pub merchant: Address,
    pub category: Symbol,
    pub tags: Vec<Symbol>,
}

/// Event: Bulk expire batch completed (#123)
#[contractevent]
#[derive(Clone, Debug)]
pub struct BulkExpireCompleted {
    pub expired_count: u32,
    pub refund_total: i128,
}

/// Event: Subscription paused by subscriber (#124)
#[contractevent]
#[derive(Clone, Debug)]
pub struct SubscriptionPaused {
    pub sub_id: u32,
    pub paused_at: u64,
}

/// Event: Subscription resumed by subscriber (#124)
#[contractevent]
#[derive(Clone, Debug)]
pub struct SubscriptionResumed {
    pub sub_id: u32,
    pub resumed_at: u64,
}

/// Event: Conditional payment completion attempt (#125)
#[contractevent]
#[derive(Clone, Debug)]
pub struct ConditionalPaymentAttempt {
    pub payment_id: u32,
    pub oracle_price: i128,
    pub threshold: i128,
    pub met: bool,
}

pub fn emit_payment_categorized(
    e: &Env,
    payment_id: u32,
    merchant: Address,
    category: Symbol,
    tags: Vec<Symbol>,
) {
    PaymentCategorized {
        payment_id,
        merchant,
        category,
        tags,
    }
    .publish(e);
}

pub fn emit_bulk_expire_completed(e: &Env, expired_count: u32, refund_total: i128) {
    BulkExpireCompleted {
        expired_count,
        refund_total,
    }
    .publish(e);
}

pub fn emit_subscription_paused(e: &Env, sub_id: u32, paused_at: u64) {
    SubscriptionPaused { sub_id, paused_at }.publish(e);
}

pub fn emit_subscription_resumed(e: &Env, sub_id: u32, resumed_at: u64) {
    SubscriptionResumed { sub_id, resumed_at }.publish(e);
}

pub fn emit_conditional_payment_attempt(
    e: &Env,
    payment_id: u32,
    oracle_price: i128,
    threshold: i128,
    met: bool,
) {
    ConditionalPaymentAttempt {
        payment_id,
        oracle_price,
        threshold,
        met,
    }
    .publish(e);
}

pub fn emit_multi_token_payment_created(
    e: &Env,
    payment_id: u32,
    customer: Address,
    merchant: Address,
    amount_usdc: i128,
    payment_token: Address,
    token_amount: i128,
    oracle_price: i128,
) {
    MultiTokenPaymentCreated {
        payment_id,
        customer,
        merchant,
        amount_usdc,
        payment_token,
        token_amount,
        oracle_price,
    }
    .publish(e);
}

pub fn emit_withdrawal_queued(e: &Env, merchant: Address, payment_id: u32, amount: i128) {
    WithdrawalQueued {
        merchant,
        payment_id,
        amount,
    }
    .publish(e);
}

pub fn emit_withdrawal_processed(e: &Env, merchant: Address, total: u32) {
    WithdrawalProcessed { merchant, total }.publish(e);
}

pub fn emit_invoice_attached(e: &Env, payment_id: u32, invoice_hash: BytesN<32>) {
    InvoiceAttached {
        payment_id,
        invoice_hash,
    }
    .publish(e);
}

// --- Collateral Events (#129) ---

/// Event: Merchant deposited collateral
#[contractevent]
#[derive(Clone, Debug)]
pub struct CollateralDeposited {
    pub merchant: Address,
    pub amount: i128,
}

/// Event: Merchant withdrew collateral
#[contractevent]
#[derive(Clone, Debug)]
pub struct CollateralWithdrawn {
    pub merchant: Address,
    pub amount: i128,
}

/// Event: Merchant collateral slashed due to dispute resolved for customer
#[contractevent]
#[derive(Clone, Debug)]
pub struct CollateralSlashed {
    pub merchant: Address,
    pub amount: i128,
    pub payment_id: u32,
}

pub fn emit_collateral_deposited(e: &Env, merchant: Address, amount: i128) {
    CollateralDeposited { merchant, amount }.publish(e);
}

pub fn emit_collateral_withdrawn(e: &Env, merchant: Address, amount: i128) {
    CollateralWithdrawn { merchant, amount }.publish(e);
}

pub fn emit_collateral_slashed(e: &Env, merchant: Address, amount: i128, payment_id: u32) {
    CollateralSlashed {
        merchant,
        amount,
        payment_id,
    }
    .publish(e);
}

// --- Slippage Tolerance Event (#135) ---

/// Event: Slippage tolerance applied to a multi-token payment
#[contractevent]
#[derive(Clone, Debug)]
pub struct SlippageToleranceApplied {
    pub payment_id: u32,
    pub tolerance_bps: u32,
    pub oracle_price: i128,
    pub settled_amount: i128,
}

pub fn emit_slippage_tolerance_applied(
    e: &Env,
    payment_id: u32,
    tolerance_bps: u32,
    oracle_price: i128,
    settled_amount: i128,
) {
    SlippageToleranceApplied {
        payment_id,
        tolerance_bps,
        oracle_price,
        settled_amount,
    }
    .publish(e);
}

// --- Volume Cap Event (#131) ---

/// Event: Merchant payment rejected due to volume cap
#[contractevent]
#[derive(Clone, Debug)]
pub struct VolumeCapped {
    pub merchant: Address,
    pub payment_id: u32,
    pub current_volume: i128,
    pub cap: i128,
}

pub fn emit_volume_capped(
    e: &Env,
    merchant: Address,
    payment_id: u32,
    current_volume: i128,
    cap: i128,
) {
    VolumeCapped {
        merchant,
        payment_id,
        current_volume,
        cap,
    }
    .publish(e);
}

// --- Notification Key Events ---

/// Event: Merchant registered a notification key
#[contractevent]
#[derive(Clone, Debug)]
pub struct NotificationKeyRegistered {
    pub merchant: Address,
    pub key: soroban_sdk::Bytes,
}

/// Event: Merchant removed their notification key
#[contractevent]
#[derive(Clone, Debug)]
pub struct NotificationKeyRemoved {
    pub merchant: Address,
}

pub fn emit_notification_key_registered(e: &Env, merchant: Address, key: soroban_sdk::Bytes) {
    NotificationKeyRegistered { merchant, key }.publish(e);
}

pub fn emit_notification_key_removed(e: &Env, merchant: Address) {
    NotificationKeyRemoved { merchant }.publish(e);
}

// --- Token Swap Events ---

/// Event: Payment swapped and settled
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentSwappedAndSettled {
    pub payment_id: u32,
    pub customer: Address,
    pub merchant: Address,
    pub input_token: Address,
    pub output_token: Address,
    pub input_amount: i128,
    pub output_amount: i128,
}

/// Event: Payment swap failed
#[contractevent]
#[derive(Clone, Debug)]
pub struct PaymentSwapFailed {
    pub payment_id: u32,
    pub customer: Address,
    pub token: Address,
    pub amount: i128,
    pub reason: Symbol,
}

/// Event: Merchant preferred token set
#[contractevent]
#[derive(Clone, Debug)]
pub struct PreferredTokenSet {
    pub merchant: Address,
    pub token: Address,
}

/// Event: Swap router set
#[contractevent]
#[derive(Clone, Debug)]
pub struct SwapRouterSet {
    pub router: Address,
}

// --- Helper Emission Functions ---

pub fn emit_payment_swapped_and_settled(
    e: &Env,
    payment_id: u32,
    customer: Address,
    merchant: Address,
    input_token: Address,
    output_token: Address,
    input_amount: i128,
    output_amount: i128,
) {
    PaymentSwappedAndSettled {
        payment_id,
        customer,
        merchant,
        input_token,
        output_token,
        input_amount,
        output_amount,
    }
    .publish(e);
}

pub fn emit_payment_swap_failed(
    e: &Env,
    payment_id: u32,
    customer: Address,
    token: Address,
    amount: i128,
    reason: Symbol,
) {
    PaymentSwapFailed {
        payment_id,
        customer,
        token,
        amount,
        reason,
    }
    .publish(e);
}

pub fn emit_preferred_token_set(e: &Env, merchant: Address, token: Address) {
    PreferredTokenSet { merchant, token }.publish(e);
}

pub fn emit_swap_router_set(e: &Env, router: Address) {
    SwapRouterSet { router }.publish(e);
}
