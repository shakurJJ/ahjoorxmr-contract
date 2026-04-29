use soroban_sdk::{contractevent, Address, Env, String};

/// Event: Refund reason code recorded (#157)
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundReasonRecorded {
    pub refund_id: u32,
    pub reason_code: u32,
}

/// Event: Refund auto-rejected after idle window (#158)
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundAutoRejected {
    pub refund_id: u32,
    pub elapsed_seconds: u64,
}

/// Event: Customer appealed a rejected refund (#159)
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundAppealed {
    pub refund_id: u32,
    pub customer: Address,
}

/// Event: Admin resolved a refund appeal (#159)
#[contractevent]
#[derive(Clone, Debug)]
pub struct AppealResolved {
    pub refund_id: u32,
    pub approved: bool,
}

/// Event: Refund requested
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundRequested {
    pub refund_id: u32,
    pub customer: Address,
    pub amount: i128,
    pub token: Address,
    pub reason: String,
}

/// Event: Refund approved
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundApproved {
    pub refund_id: u32,
    pub approved_by: Address,
    pub approved_at: u64,
}

/// Event: Refund rejected
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundRejected {
    pub refund_id: u32,
    pub rejected_by: Address,
    pub rejection_reason: String,
    pub rejected_at: u64,
}

/// Event: Refund processed (tokens transferred)
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundProcessed {
    pub refund_id: u32,
    pub customer: Address,
    pub amount: i128,
    pub processed_at: u64,
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

/// Event: Merchant made a counter-offer on a refund request
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundCounterOffered {
    pub refund_id: u32,
    pub merchant: Address,
    pub counter_amount: i128,
    pub expires_at: u64,
}

/// Event: Customer accepted the counter-offer
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundCounterAccepted {
    pub refund_id: u32,
    pub customer: Address,
    pub amount: i128,
}

/// Event: Customer rejected the counter-offer (escalated to admin)
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundCounterRejected {
    pub refund_id: u32,
    pub customer: Address,
}

/// Event: Refund auto-approved after dispute window elapsed without merchant response
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundAutoApproved {
    pub refund_id: u32,
    pub customer: Address,
    pub amount: i128,
}

/// Event: Refund auto-approved via whitelist
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundAutoApprovedWhitelist {
    pub refund_id: u32,
    pub merchant: Address,
    pub amount: i128,
}

/// Event: Escrow refund registered
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowRefundRegistered {
    pub refund_id: u32,
    pub escrow_id: u32,
    pub buyer: Address,
    pub amount: i128,
}

/// Event: Refund fee collected
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundFeeCollected {
    pub refund_id: u32,
    pub fee_amount: i128,
}

/// Event: Partial refund cap threshold reached
#[contractevent]
#[derive(Clone, Debug)]
pub struct PartialRefundCapApplied {
    pub refund_id: u32,
    pub remaining_refundable: i128,
}

/// Event: Tier applied to a refund request
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundTierApplied {
    pub refund_id: u32,
    pub tier_bps: u32,
    pub max_refundable: i128,
}

/// Event: Merchant initiated immediate refund
#[contractevent]
#[derive(Clone, Debug)]
pub struct MerchantInitiatedRefund {
    pub refund_id: u32,
    pub payment_id: u32,
    pub merchant: Address,
    pub amount: i128,
    pub reason_code: u32,
}

/// Event: Bulk approved refunds processed
#[contractevent]
#[derive(Clone, Debug)]
pub struct BulkRefundProcessed {
    pub count: u32,
    pub total_amount: i128,
}

// --- Issue #166: Per-Customer Refund Request Cooldown Period ---

/// Event: Customer attempted a refund request within the cooldown window
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundCooldownActive {
    pub customer: Address,
    pub next_eligible_at: u64,
}

// --- Issue #167: Delegated Refund Approval for Merchant Sub-Admins ---

/// Event: A delegate was added for a merchant
#[contractevent]
#[derive(Clone, Debug)]
pub struct DelegateAdded {
    pub merchant: Address,
    pub delegate: Address,
}

/// Event: A refund was approved by a merchant delegate
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundApprovedByDelegate {
    pub refund_id: u32,
    pub delegate: Address,
}

// --- Issue #168: Refund Request Expiry with Auto-Cancellation ---

/// Event: A refund request was cancelled (by customer or auto-cancelled)
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundRequestCancelled {
    pub refund_id: u32,
    pub cancelled_by: Address,
}

// --- Helper Emission Functions ---

pub fn emit_refund_requested(
    e: &Env,
    refund_id: u32,
    customer: Address,
    amount: i128,
    token: Address,
    reason: String,
) {
    RefundRequested {
        refund_id,
        customer,
        amount,
        token,
        reason,
    }
    .publish(e);
}

pub fn emit_refund_approved(e: &Env, refund_id: u32, approved_by: Address, approved_at: u64) {
    RefundApproved {
        refund_id,
        approved_by,
        approved_at,
    }
    .publish(e);
}

pub fn emit_refund_rejected(
    e: &Env,
    refund_id: u32,
    rejected_by: Address,
    rejection_reason: String,
    rejected_at: u64,
) {
    RefundRejected {
        refund_id,
        rejected_by,
        rejection_reason,
        rejected_at,
    }
    .publish(e);
}

pub fn emit_refund_processed(
    e: &Env,
    refund_id: u32,
    customer: Address,
    amount: i128,
    processed_at: u64,
) {
    RefundProcessed {
        refund_id,
        customer,
        amount,
        processed_at,
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

pub fn emit_refund_auto_approved(e: &Env, refund_id: u32, customer: Address, amount: i128) {
    RefundAutoApproved {
        refund_id,
        customer,
        amount,
    }
    .publish(e);
}

pub fn emit_partial_refund_cap_applied(e: &Env, refund_id: u32, remaining_refundable: i128) {
    PartialRefundCapApplied {
        refund_id,
        remaining_refundable,
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

pub fn emit_refund_auto_approved_whitelist(
    e: &Env,
    refund_id: u32,
    merchant: Address,
    amount: i128,
) {
    RefundAutoApprovedWhitelist {
        refund_id,
        merchant,
        amount,
    }
    .publish(e);
}

pub fn emit_escrow_refund_registered(
    e: &Env,
    refund_id: u32,
    escrow_id: u32,
    buyer: Address,
    amount: i128,
) {
    EscrowRefundRegistered {
        refund_id,
        escrow_id,
        buyer,
        amount,
    }
    .publish(e);
}

pub fn emit_refund_fee_collected(e: &Env, refund_id: u32, fee_amount: i128) {
    RefundFeeCollected {
        refund_id,
        fee_amount,
    }
    .publish(e);
}

pub fn emit_refund_reason_recorded(e: &Env, refund_id: u32, reason_code: u32) {
    RefundReasonRecorded {
        refund_id,
        reason_code,
    }
    .publish(e);
}

pub fn emit_refund_auto_rejected(e: &Env, refund_id: u32, elapsed_seconds: u64) {
    RefundAutoRejected {
        refund_id,
        elapsed_seconds,
    }
    .publish(e);
}

pub fn emit_refund_tier_applied(
    e: &Env,
    refund_id: u32,
    tier_bps: u32,
    max_refundable: i128,
) {
    RefundTierApplied {
        refund_id,
        tier_bps,
        max_refundable,
    }
    .publish(e);
}

pub fn emit_merchant_initiated_refund(
    e: &Env,
    refund_id: u32,
    payment_id: u32,
    merchant: Address,
    amount: i128,
    reason_code: u32,
) {
    MerchantInitiatedRefund {
        refund_id,
        payment_id,
        merchant,
        amount,
        reason_code,
    }
    .publish(e);
}

pub fn emit_appeal_resolved(e: &Env, refund_id: u32, approved: bool) {
    AppealResolved {
        refund_id,
        approved,
    }
    .publish(e);
}

pub fn emit_bulk_refund_processed(e: &Env, count: u32, total_amount: i128) {
    BulkRefundProcessed { count, total_amount }.publish(e);
}

pub fn emit_refund_appealed(e: &Env, refund_id: u32, customer: Address) {
    RefundAppealed { refund_id, customer }.publish(e);
}

// --- #166 ---

pub fn emit_refund_cooldown_active(e: &Env, customer: Address, next_eligible_at: u64) {
    RefundCooldownActive {
        customer,
        next_eligible_at,
    }
    .publish(e);
}

// --- #167 ---

pub fn emit_delegate_added(e: &Env, merchant: Address, delegate: Address) {
    DelegateAdded { merchant, delegate }.publish(e);
}

pub fn emit_refund_approved_by_delegate(e: &Env, refund_id: u32, delegate: Address) {
    RefundApprovedByDelegate { refund_id, delegate }.publish(e);
}

// --- #168 ---

pub fn emit_refund_request_cancelled(e: &Env, refund_id: u32, cancelled_by: Address) {
    RefundRequestCancelled {
        refund_id,
        cancelled_by,
    }
    .publish(e);
}

// --- Fraud Score Events ---

/// Event: Fraud score updated for a buyer
#[contractevent]
#[derive(Clone, Debug)]
pub struct FraudScoreUpdated {
    pub buyer: Address,
    pub new_score: u32,
    pub reason: Symbol,
}

/// Event: Buyer blocked for exceeding fraud score threshold
#[contractevent]
#[derive(Clone, Debug)]
pub struct BuyerBlockedForFraud {
    pub buyer: Address,
    pub score: u32,
    pub threshold: u32,
}

/// Event: Buyer's fraud score manually reset by admin
#[contractevent]
#[derive(Clone, Debug)]
pub struct FraudScoreReset {
    pub buyer: Address,
    pub reset_by: Address,
}

/// Event: Fraud score decay applied
#[contractevent]
#[derive(Clone, Debug)]
pub struct FraudScoreDecayApplied {
    pub buyer: Address,
    pub old_score: u32,
    pub new_score: u32,
}

/// Event: Fraud score block threshold updated
#[contractevent]
#[derive(Clone, Debug)]
pub struct FraudScoreBlockThresholdUpdated {
    pub old_threshold: u32,
    pub new_threshold: u32,
}

// --- Helper Emission Functions ---

pub fn emit_fraud_score_updated(e: &Env, buyer: Address, new_score: u32, reason: Symbol) {
    FraudScoreUpdated { buyer, new_score, reason }.publish(e);
}

pub fn emit_buyer_blocked_for_fraud(e: &Env, buyer: Address, score: u32, threshold: u32) {
    BuyerBlockedForFraud { buyer, score, threshold }.publish(e);
}

pub fn emit_fraud_score_reset(e: &Env, buyer: Address, reset_by: Address) {
    FraudScoreReset { buyer, reset_by }.publish(e);
}

pub fn emit_fraud_score_decay_applied(e: &Env, buyer: Address, old_score: u32, new_score: u32) {
    FraudScoreDecayApplied { buyer, old_score, new_score }.publish(e);
}

pub fn emit_fraud_score_block_threshold_updated(e: &Env, old_threshold: u32, new_threshold: u32) {
    FraudScoreBlockThresholdUpdated { old_threshold, new_threshold }.publish(e);
}

pub fn emit_refund_counter_offered(e: &Env, refund_id: u32, merchant: Address, counter_amount: i128, expires_at: u64) {
    RefundCounterOffered { refund_id, merchant, counter_amount, expires_at }.publish(e);
}

pub fn emit_refund_counter_accepted(e: &Env, refund_id: u32, customer: Address, amount: i128) {
    RefundCounterAccepted { refund_id, customer, amount }.publish(e);
}

pub fn emit_refund_counter_rejected(e: &Env, refund_id: u32, customer: Address) {
    RefundCounterRejected { refund_id, customer }.publish(e);
}

// --- Issue #228: Refund Merchant Auto-Approval Threshold ---

/// Event: Merchant set their auto-approval threshold
#[contractevent]
#[derive(Clone, Debug)]
pub struct AutoApproveThresholdSet {
    pub merchant: Address,
    pub amount: i128,
}

/// Event: Refund auto-approved because amount <= merchant threshold
#[contractevent]
#[derive(Clone, Debug)]
pub struct RefundAutoApprovedByThreshold {
    pub refund_id: u32,
    pub amount: i128,
}

pub fn emit_auto_approve_threshold_set(e: &Env, merchant: Address, amount: i128) {
    AutoApproveThresholdSet { merchant, amount }.publish(e);
}

pub fn emit_refund_auto_approved_by_threshold(e: &Env, refund_id: u32, amount: i128) {
    RefundAutoApprovedByThreshold { refund_id, amount }.publish(e);
}
