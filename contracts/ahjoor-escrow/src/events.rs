use soroban_sdk::{contractevent, Address, BytesN, Env, String, Symbol};

/// Event: Escrow created
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowCreated {
    pub escrow_id: u32,
    pub buyer: Address,
    pub seller: Address,
    pub arbiter: Address,
    pub amount: i128,
    pub token: Address,
    pub deadline: u64,
}

/// Event: Escrow creation includes a lock window
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowTimeLocked {
    pub escrow_id: u32,
    pub locked_until: u64,
}

/// Event: Batch escrow created summary
#[contractevent]
#[derive(Clone, Debug)]
pub struct BatchEscrowCreated {
    pub count: u32,
    pub first_id: u32,
    pub last_id: u32,
}

/// Event: Escrow released to seller
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowReleased {
    pub escrow_id: u32,
    pub seller: Address,
    pub amount: i128,
}

/// Event: Escrow partially released to seller
#[contractevent]
#[derive(Clone, Debug)]
pub struct PartialReleased {
    pub escrow_id: u32,
    pub released_amount: i128,
    pub remaining_amount: i128,
}

/// Event: Milestone approved and amount released to seller (#136)
#[contractevent]
#[derive(Clone, Debug)]
pub struct MilestoneApproved {
    pub escrow_id: u32,
    pub milestone_index: u32,
    pub amount_released: i128,
}

/// Event: Escrow disputed
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowDisputed {
    pub escrow_id: u32,
    pub disputer: Address,
    pub reason: String,
}

/// Event: Partial dispute raised — undisputed portion released to seller
#[contractevent]
#[derive(Clone, Debug)]
pub struct PartialDisputeRaised {
    pub escrow_id: u32,
    pub dispute_amount: i128,
    pub released_amount: i128,
}

/// Event: Dispute resolved
#[contractevent]
#[derive(Clone, Debug)]
pub struct DisputeResolved {
    pub escrow_id: u32,
    pub release_to_seller: bool,
    pub resolved_by: Address,
}

/// Event: Dispute resolved with a percentage split between buyer and seller
#[contractevent]
#[derive(Clone, Debug)]
pub struct DisputeResolvedSplit {
    pub escrow_id: u32,
    pub buyer_percent: u32,
    pub seller_percent: u32,
    pub buyer_amount: i128,
    pub seller_amount: i128,
    pub resolved_by: Address,
}

/// Event: Dispute escalation threshold reached
#[contractevent]
#[derive(Clone, Debug)]
pub struct DisputeEscalated {
    pub escrow_id: u32,
    pub timeout_seconds: u64,
}

/// Event: Protocol fee paid on dispute resolution
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowProtocolFeePaid {
    pub escrow_id: u32,
    pub fee_amount: i128,
    pub fee_recipient: Address,
}

/// Event: Arbiter fee paid on dispute resolution
#[contractevent]
#[derive(Clone, Debug)]
pub struct ArbiterFeePaid {
    pub escrow_id: u32,
    pub arbiter: Address,
    pub fee_amount: i128,
}

/// Event: Insurance payout claimed
#[contractevent]
#[derive(Clone, Debug)]
pub struct InsuranceClaimed {
    pub escrow_id: u32,
    pub claimant: Address,
    pub amount: i128,
}

/// Event: Oracle-triggered release executed
#[contractevent]
#[derive(Clone, Debug)]
pub struct OracleReleaseTriggered {
    pub escrow_id: u32,
    pub oracle_price: i128,
    pub base: Address,
    pub quote: Address,
    pub comparison: u32,
    pub threshold_price: i128,
}

/// Event: Escrow refunded to buyer
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowRefunded {
    pub escrow_id: u32,
    pub buyer: Address,
    pub amount: i128,
}

/// Event: Contract WASM upgraded
#[contractevent]
#[derive(Clone, Debug)]
pub struct ContractUpgraded {
    pub old_version: u32,
    pub new_version: u32,
    pub by_admin: Address,
}

/// Event: Deadline extension proposed by a participant
#[contractevent]
#[derive(Clone, Debug)]
pub struct DeadlineExtensionProposed {
    pub escrow_id: u32,
    pub proposer: Address,
    pub new_deadline: u64,
    pub proposed_at: u64,
}

/// Event: Deadline updated after counterparty acceptance
#[contractevent]
#[derive(Clone, Debug)]
pub struct DeadlineExtended {
    pub escrow_id: u32,
    pub old_deadline: u64,
    pub new_deadline: u64,
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

/// Event: Token Allowlisted
#[contractevent]
#[derive(Clone, Debug)]
pub struct TokenAllowlisted {
    pub admin: Address,
    pub token: Address,
}

/// Event: Token Removed From Allowlist
#[contractevent]
#[derive(Clone, Debug)]
pub struct TokenRemovedFromAllowlist {
    pub admin: Address,
    pub token: Address,
}

/// Event: Arbiter added to or removed from the pool
#[contractevent]
#[derive(Clone, Debug)]
pub struct ArbiterPoolUpdated {
    pub arbiter: Address,
    pub added: bool,
}

/// Event: Arbiter assigned to an escrow via pool round-robin
#[contractevent]
#[derive(Clone, Debug)]
pub struct ArbiterAssigned {
    pub escrow_id: u32,
    pub arbiter: Address,
}

/// Event: Evidence hash submitted for a dispute
#[contractevent]
#[derive(Clone, Debug)]
pub struct EvidenceSubmitted {
    pub escrow_id: u32,
    pub party: Address,
    pub evidence_hash: BytesN<32>,
}

/// Event: Escrow renewed automatically on release
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowAutoRenewed {
    pub old_escrow_id: u32,
    pub new_escrow_id: u32,
    pub renewals_remaining: u32,
}

/// Event: Buyer role transferred to another address
#[contractevent]
#[derive(Clone, Debug)]
pub struct BuyerRoleTransferred {
    pub escrow_id: u32,
    pub old_buyer: Address,
    pub new_buyer: Address,
}

/// Event: Dispute resolution entered cooling-off state
#[contractevent]
#[derive(Clone, Debug)]
pub struct ResolutionCoolingOff {
    pub escrow_id: u32,
    pub buyer_percent: u32,
    pub arbiter: Address,
    pub cooling_off_ends_at: u64,
}

/// Event: Resolution error flagged by a party during cooling-off
#[contractevent]
#[derive(Clone, Debug)]
pub struct ResolutionFlagged {
    pub escrow_id: u32,
    pub flagger: Address,
    pub reason_hash: BytesN<32>,
}

/// Event: Resolution finalized and funds released
#[contractevent]
#[derive(Clone, Debug)]
pub struct ResolutionFinalized {
    pub escrow_id: u32,
    pub buyer_percent: u32,
    pub finalized_by: Address,
}

// --- Helper Emission Functions ---

pub fn emit_escrow_created(
    e: &Env,
    escrow_id: u32,
    buyer: Address,
    seller: Address,
    arbiter: Address,
    amount: i128,
    token: Address,
    deadline: u64,
) {
    EscrowCreated {
        escrow_id,
        buyer,
        seller,
        arbiter,
        amount,
        token,
        deadline,
    }
    .publish(e);
}

pub fn emit_escrow_time_locked(e: &Env, escrow_id: u32, locked_until: u64) {
    EscrowTimeLocked {
        escrow_id,
        locked_until,
    }
    .publish(e);
}

pub fn emit_batch_escrow_created(e: &Env, count: u32, first_id: u32, last_id: u32) {
    BatchEscrowCreated {
        count,
        first_id,
        last_id,
    }
    .publish(e);
}

pub fn emit_escrow_released(e: &Env, escrow_id: u32, seller: Address, amount: i128) {
    EscrowReleased {
        escrow_id,
        seller,
        amount,
    }
    .publish(e);
}

pub fn emit_partial_released(
    e: &Env,
    escrow_id: u32,
    released_amount: i128,
    remaining_amount: i128,
) {
    PartialReleased {
        escrow_id,
        released_amount,
        remaining_amount,
    }
    .publish(e);
}

pub fn emit_milestone_approved(
    e: &Env,
    escrow_id: u32,
    milestone_index: u32,
    amount_released: i128,
) {
    MilestoneApproved {
        escrow_id,
        milestone_index,
        amount_released,
    }
    .publish(e);
}

pub fn emit_escrow_disputed(e: &Env, escrow_id: u32, disputer: Address, reason: String) {
    EscrowDisputed {
        escrow_id,
        disputer,
        reason,
    }
    .publish(e);
}

pub fn emit_partial_dispute_raised(
    e: &Env,
    escrow_id: u32,
    dispute_amount: i128,
    released_amount: i128,
) {
    PartialDisputeRaised {
        escrow_id,
        dispute_amount,
        released_amount,
    }
    .publish(e);
}

pub fn emit_dispute_resolved(
    e: &Env,
    escrow_id: u32,
    release_to_seller: bool,
    resolved_by: Address,
) {
    DisputeResolved {
        escrow_id,
        release_to_seller,
        resolved_by,
    }
    .publish(e);
}

pub fn emit_dispute_resolved_split(
    e: &Env,
    escrow_id: u32,
    buyer_percent: u32,
    seller_percent: u32,
    buyer_amount: i128,
    seller_amount: i128,
    resolved_by: Address,
) {
    DisputeResolvedSplit {
        escrow_id,
        buyer_percent,
        seller_percent,
        buyer_amount,
        seller_amount,
        resolved_by,
    }
    .publish(e);
}

pub fn emit_dispute_escalated(e: &Env, escrow_id: u32, timeout_seconds: u64) {
    DisputeEscalated {
        escrow_id,
        timeout_seconds,
    }
    .publish(e);
}

pub fn emit_protocol_fee_paid(e: &Env, escrow_id: u32, fee_amount: i128, fee_recipient: Address) {
    EscrowProtocolFeePaid {
        escrow_id,
        fee_amount,
        fee_recipient,
    }
    .publish(e);
}

pub fn emit_arbiter_fee_paid(e: &Env, escrow_id: u32, arbiter: Address, fee_amount: i128) {
    ArbiterFeePaid {
        escrow_id,
        arbiter,
        fee_amount,
    }
    .publish(e);
}

pub fn emit_insurance_claimed(e: &Env, escrow_id: u32, claimant: Address, amount: i128) {
    InsuranceClaimed {
        escrow_id,
        claimant,
        amount,
    }
    .publish(e);
}

pub fn emit_oracle_release_triggered(
    e: &Env,
    escrow_id: u32,
    oracle_price: i128,
    base: Address,
    quote: Address,
    comparison: u32,
    threshold_price: i128,
) {
    OracleReleaseTriggered {
        escrow_id,
        oracle_price,
        base,
        quote,
        comparison,
        threshold_price,
    }
    .publish(e);
}

pub fn emit_escrow_refunded(e: &Env, escrow_id: u32, buyer: Address, amount: i128) {
    EscrowRefunded {
        escrow_id,
        buyer,
        amount,
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
pub fn emit_deadline_extension_proposed(
    e: &Env,
    escrow_id: u32,
    proposer: Address,
    new_deadline: u64,
    proposed_at: u64,
) {
    DeadlineExtensionProposed {
        escrow_id,
        proposer,
        new_deadline,
        proposed_at,
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

pub fn emit_deadline_extended(e: &Env, escrow_id: u32, old_deadline: u64, new_deadline: u64) {
    DeadlineExtended {
        escrow_id,
        old_deadline,
        new_deadline,
    }
    .publish(e);
}

pub fn emit_contract_resumed(e: &Env, admin: Address, timestamp: u64) {
    ContractResumed { admin, timestamp }.publish(e);
}

pub fn emit_token_allowlisted(e: &Env, admin: Address, token: Address) {
    TokenAllowlisted { admin, token }.publish(e);
}

pub fn emit_token_removed_from_allowlist(e: &Env, admin: Address, token: Address) {
    TokenRemovedFromAllowlist { admin, token }.publish(e);
}

/// Event: Escrow template created
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowTemplateCreated {
    pub template_id: u32,
    pub creator: Address,
}

/// Event: Escrow template config updated
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowTemplateUpdated {
    pub template_id: u32,
    pub creator: Address,
}

/// Event: Escrow template deactivated
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowTemplateDeactivated {
    pub template_id: u32,
    pub creator: Address,
}

/// Event: Escrow created from a template
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowCreatedFromTemplate {
    pub escrow_id: u32,
    pub template_id: u32,
}

pub fn emit_escrow_template_created(e: &Env, template_id: u32, creator: Address) {
    EscrowTemplateCreated { template_id, creator }.publish(e);
}

pub fn emit_escrow_template_updated(e: &Env, template_id: u32, creator: Address) {
    EscrowTemplateUpdated { template_id, creator }.publish(e);
}

pub fn emit_escrow_template_deactivated(e: &Env, template_id: u32, creator: Address) {
    EscrowTemplateDeactivated { template_id, creator }.publish(e);
}

pub fn emit_escrow_created_from_template(e: &Env, escrow_id: u32, template_id: u32) {
    EscrowCreatedFromTemplate { escrow_id, template_id }.publish(e);
}

pub fn emit_arbiter_pool_updated(e: &Env, arbiter: Address, added: bool) {
    ArbiterPoolUpdated { arbiter, added }.publish(e);
}

pub fn emit_arbiter_assigned(e: &Env, escrow_id: u32, arbiter: Address) {
    ArbiterAssigned { escrow_id, arbiter }.publish(e);
}

pub fn emit_evidence_submitted(
    e: &Env,
    escrow_id: u32,
    party: Address,
    evidence_hash: BytesN<32>,
) {
    EvidenceSubmitted {
        escrow_id,
        party,
        evidence_hash,
    }
    .publish(e);
}

pub fn emit_escrow_auto_renewed(
    e: &Env,
    old_escrow_id: u32,
    new_escrow_id: u32,
    renewals_remaining: u32,
) {
    EscrowAutoRenewed {
        old_escrow_id,
        new_escrow_id,
        renewals_remaining,
    }
    .publish(e);
}

pub fn emit_buyer_role_transferred(
    e: &Env,
    escrow_id: u32,
    old_buyer: Address,
    new_buyer: Address,
) {
    BuyerRoleTransferred {
        escrow_id,
        old_buyer,
        new_buyer,
    }
    .publish(e);
}

// --- Issue #145: Escrow Metadata ---

/// Event: Escrow metadata hash updated
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowMetadataUpdated {
    pub escrow_id: u32,
    pub new_hash: BytesN<32>,
    pub updated_by: Address,
}

pub fn emit_escrow_metadata_updated(
    e: &Env,
    escrow_id: u32,
    new_hash: BytesN<32>,
    updated_by: Address,
) {
    EscrowMetadataUpdated {
        escrow_id,
        new_hash,
        updated_by,
    }
    .publish(e);
}

// --- Issue #148: Multi-Party Escrow ---

/// Event: Multi-party escrow created
#[contractevent]
#[derive(Clone, Debug)]
pub struct MultiPartyEscrowCreated {
    pub escrow_id: u32,
    pub sellers_count: u32,
}

/// Event: Multi-party escrow released with distributions
#[contractevent]
#[derive(Clone, Debug)]
pub struct MultiPartyEscrowReleased {
    pub escrow_id: u32,
    pub total_amount: i128,
}

pub fn emit_multi_party_escrow_created(e: &Env, escrow_id: u32, sellers_count: u32) {
    MultiPartyEscrowCreated {
        escrow_id,
        sellers_count,
    }
    .publish(e);
}

pub fn emit_multi_party_escrow_released(e: &Env, escrow_id: u32, total_amount: i128) {
    MultiPartyEscrowReleased {
        escrow_id,
        total_amount,
    }
    .publish(e);
}

// --- Issue #150: Seller-Favored Auto-Release on Prolonged Buyer Inactivity ---

/// Event: Inactivity release claimed by seller after buyer inactivity window
#[contractevent]
#[derive(Clone, Debug)]
pub struct InactivityReleaseTriggered {
    pub escrow_id: u32,
    pub seller: Address,
    pub inactivity_seconds: u64,
}

pub fn emit_inactivity_release_triggered(
    e: &Env,
    escrow_id: u32,
    seller: Address,
    inactivity_seconds: u64,
) {
    InactivityReleaseTriggered {
        escrow_id,
        seller,
        inactivity_seconds,
    }
    .publish(e);
}

// --- Issue #151: Dispute Timeout Mechanism ---

/// Event: Dispute timed out and funds released to default winner
#[contractevent]
#[derive(Clone, Debug)]
pub struct DisputeTimedOut {
    pub escrow_id: u32,
    pub arbiter: Address,
    pub default_winner: u32, // 0 = Buyer, 1 = Seller
    pub elapsed_seconds: u64,
}

/// Event: Arbiter timeout penalty applied
#[contractevent]
#[derive(Clone, Debug)]
pub struct ArbiterTimeoutPenaltyApplied {
    pub arbiter: Address,
    pub total_timeouts: u32,
}

pub fn emit_dispute_timed_out(
    e: &Env,
    escrow_id: u32,
    arbiter: Address,
    default_winner: crate::DisputeDefaultWinner,
    elapsed_seconds: u64,
) {
    DisputeTimedOut {
        escrow_id,
        arbiter,
        default_winner: default_winner as u32,
        elapsed_seconds,
    }
    .publish(e);
}

pub fn emit_arbiter_timeout_penalty_applied(e: &Env, arbiter: Address, total_timeouts: u32) {
    ArbiterTimeoutPenaltyApplied {
        arbiter,
        total_timeouts,
    }
    .publish(e);
}

// #215: Time-locked escrow events
pub fn emit_timelocked_escrow_created(e: &Env, escrow_id: u32, unlock_at: u64, beneficiary: Address) {
    e.events().publish((Symbol::new(e, "TLEscrowCreated"),), (escrow_id, unlock_at, beneficiary));
}
pub fn emit_timelocked_funds_claimed(e: &Env, escrow_id: u32, beneficiary: Address, amount: i128) {
    e.events().publish((Symbol::new(e, "TLFundsClaimed"),), (escrow_id, beneficiary, amount));
}
pub fn emit_timelocked_escrow_cancelled(e: &Env, escrow_id: u32, buyer: Address) {
    e.events().publish((Symbol::new(e, "TLEscrowCancelled"),), (escrow_id, buyer));
}

// #229: Mutual Cancellation events
pub fn emit_cancellation_requested(e: &Env, escrow_id: u32, initiator: Address, expires_at: u64) {
    e.events().publish((Symbol::new(e, "CancelRequested"),), (escrow_id, initiator, expires_at));
}
pub fn emit_cancellation_accepted(e: &Env, escrow_id: u32, buyer: Address, amount_returned: i128, penalty: i128) {
    e.events().publish((Symbol::new(e, "CancelAccepted"),), (escrow_id, buyer, amount_returned, penalty));
}
pub fn emit_cancellation_rejected(e: &Env, escrow_id: u32, rejector: Address) {
    e.events().publish((Symbol::new(e, "CancelRejected"),), (escrow_id, rejector));
}
pub fn emit_cancellation_expired(e: &Env, escrow_id: u32) {
    e.events().publish((Symbol::new(e, "CancelExpired"),), (escrow_id,));
// #225: Escrow Top-Up Event

/// Event: Buyer topped up an active escrow with additional funds
#[contractevent]
#[derive(Clone, Debug)]
pub struct EscrowToppedUp {
    pub escrow_id: u32,
    pub added_amount: i128,
    pub new_total: i128,
    pub buyer: Address,
}

pub fn emit_escrow_topped_up(e: &Env, escrow_id: u32, added_amount: i128, new_total: i128, buyer: Address) {
    EscrowToppedUp { escrow_id, added_amount, new_total, buyer }.publish(e);
}

// --- Issue #146: Post-Resolution Rating System ---

/// Event: Rating submitted after escrow completion
#[contractevent]
#[derive(Clone, Debug)]
pub struct RatingSubmitted {
    pub escrow_id: u32,
    pub rater: Address,
    pub ratee: Address,
    pub rating: u32,
    pub comment_hash: Option<BytesN<32>>,
}

pub fn emit_rating_submitted(
    e: &Env,
    escrow_id: u32,
    rater: Address,
    ratee: Address,
    rating: u32,
    comment_hash: Option<BytesN<32>>,
) {
    RatingSubmitted {
        escrow_id,
        rater,
        ratee,
        rating,
        comment_hash,
    }
    .publish(e);
}

// --- Issue #219: Multi-Party Split Release ---

/// Event: Multi-seller escrow created with explicit payee list and shares
#[contractevent]
#[derive(Clone, Debug)]
pub struct MultiSellerEscrowCreated {
    pub escrow_id: u32,
    pub sellers_count: u32,
}

pub fn emit_multi_seller_escrow_created(
    e: &Env,
    escrow_id: u32,
    sellers: soroban_sdk::Vec<(Address, u32)>,
) {
    MultiSellerEscrowCreated {
        escrow_id,
        sellers_count: sellers.len(),
    }
    .publish(e);
}
