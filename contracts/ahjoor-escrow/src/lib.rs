#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, BytesN, Env, String, Vec};
use ahjoor_token_whitelist::TokenWhitelistClient;

// --- Storage TTL Constants ---
const INSTANCE_LIFETIME_THRESHOLD: u32 = 100_000;
const INSTANCE_BUMP_AMOUNT: u32 = 120_000;

const PERSISTENT_LIFETIME_THRESHOLD: u32 = 100_000;
const PERSISTENT_BUMP_AMOUNT: u32 = 120_000;
const DEADLINE_EXTENSION_PROPOSAL_WINDOW: u64 = 24 * 60 * 60;
const MAX_EVIDENCE_ENTRIES_PER_PARTY: u32 = 5;
const DEFAULT_DISPUTE_TIMEOUT_SECONDS: u64 = 7 * 24 * 60 * 60;
const MAX_BATCH_ESCROWS: u32 = 10;
const DEFAULT_MAX_ORACLE_AGE_SECONDS: u64 = 300;
const DEFAULT_INSURANCE_TRIGGER_DAYS: u64 = 7;
const DEFAULT_MAX_TOPUP_MULTIPLIER: u32 = 3;
const DEFAULT_PARTIAL_RELEASE_RESPONSE_DEADLINE: u64 = 86400; // 1 day

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum EscrowStatus {
    Active = 0,
    Released = 1,
    Disputed = 2,
    Resolved = 3,
    Refunded = 4,
    PartiallyReleased = 5,
    PartiallyDisputed = 6,
    /// Arbiter has recorded a verdict; funds held pending cooling-off window.
    CoolingOff = 7,
    /// Mutual cancellation requested; awaiting counterparty response (#229).
    CancellationPending = 8,
    /// #237: Escrow created but awaiting seller collateral deposit before becoming Active.
    AwaitingCollateral = 9,
    /// Seller has proposed a role transfer; buyer has a veto window (#244).
    AwaitingBuyerVetoDecision = 10,
    /// #272: Seller marked work complete; awaiting inspector verdict.
    AwaitingInspection = 11,
    /// #272: Inspector approved; buyer may release.
    InspectionPassed = 12,
    /// #272: Inspector rejected; buyer/seller may negotiate or dispute.
    InspectionFailed = 13,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum DisputeDefaultWinner {
    Buyer = 0,
    Seller = 1,
}

const ORACLE_COMPARISON_LESS_OR_EQUAL: u32 = 0;
const ORACLE_COMPARISON_GREATER_OR_EQUAL: u32 = 1;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseCondition {
    pub base: Address,
    pub quote: Address,
    /// 0 = LessOrEqual, 1 = GreaterOrEqual
    pub comparison: u32,
    pub threshold_price: i128,
}

/// Conditional release based on external contract state (#318)
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConditionalRelease {
    pub oracle_contract: Address,
    pub condition_method: Symbol,
    pub expected_value: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowCreateRequest {
    pub seller: Address,
    pub arbiter: Address,
    pub amount: i128,
    pub token: Address,
    pub deadline: u64,
    pub metadata_hash: Option<BytesN<32>>,
    pub sellers: Vec<(Address, u32)>,
    pub auto_renew: bool,
    pub renewal_count: u32,
    pub buyer_inactivity_secs: u64,
    pub min_lock_until: Option<u64>,
    pub release_base: Option<Address>,
    pub release_quote: Option<Address>,
    pub release_comparison: Option<u32>,
    pub release_threshold_price: Option<i128>,
    pub arbiter_fee_bps: Option<u32>,
    pub dispute_default_winner: Option<u32>, // 0 = Buyer, 1 = Seller
    /// Optional auto-renewal configuration for recurring service agreements.
    pub auto_renew_config: Option<AutoRenewConfig>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Escrow {
    pub id: u32,
    pub buyer: Address,
    pub seller: Address,
    pub arbiter: Address,
    pub amount: i128,
    pub original_amount: i128,
    pub token: Address,
    pub status: EscrowStatus,
    pub created_at: u64,
    pub deadline: u64,
    pub metadata_hash: Option<BytesN<32>>,
    pub sellers: Vec<(Address, u32)>, // (address, bps) — multi-party sellers
    pub extensions: EscrowExtensions,
    pub top_up_history: Vec<TopUpEntry>,
    pub top_up_acknowledged: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowExtensions {
    pub auto_renew: bool,
    pub renewal_count: u32,
    pub renewals_remaining: u32,
    pub dispute_timeout_seconds: Option<u64>,
    /// Seconds of buyer inactivity before seller can claim auto-release; 0 = disabled (#150)
    pub buyer_inactivity_secs: u64,
    /// Earliest ledger timestamp when release/partial release is permitted.
    pub min_lock_until: Option<u64>,
    pub release_base: Option<Address>,
    pub release_quote: Option<Address>,
    pub release_comparison: Option<u32>,
    pub release_threshold_price: Option<i128>,
    /// Optional per-escrow arbiter fee override in basis points.
    pub arbiter_fee_bps: Option<u32>,
    /// Optional per-escrow default winner override for dispute timeout (0 = Buyer, 1 = Seller).
    pub dispute_default_winner: Option<u32>,
    /// #237: Required seller collateral as bps of escrow amount (0 = no collateral).
    pub required_collateral_bps: u32,
    /// #237: Forfeiture bps applied to collateral on buyer-favour dispute resolution.
    pub collateral_forfeit_bps: u32,
    /// #237: Ledger timestamp deadline for seller to deposit collateral (0 = not set).
    pub collateral_deposit_deadline: u64,
    /// #237: Actual collateral amount deposited by seller.
    pub collateral_amount: i128,
    /// #241: Optional pre-committed delivery proof hash set by buyer at creation.
    pub delivery_proof_hash: Option<BytesN<32>>,
    /// #272: Optional inspector address for three-party quality gate.
    pub inspector: Option<Address>,
    /// Auto-renewal config (max_renewals + renewal_interval_ledgers); None = no auto-renewal.
    pub auto_renew_config: Option<AutoRenewConfig>,
    /// Number of renewal cycles completed so far (incremented on each successful renewal).
    pub renewals_completed: u32,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceSubmission {
    pub evidence_hash: BytesN<32>,
    pub evidence_uri_hash: BytesN<32>,
    pub submitted_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dispute {
    pub escrow_id: u32,
    pub reason: String,
    pub created_at: u64,
    pub resolved: bool,
    pub dispute_amount: i128,
    pub timeout_seconds: Option<u64>,
}

// #215: time-locked escrow release data
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeLockData {
    pub unlock_at: u64,
    pub beneficiary: Address,
    pub claimed: bool,
}

/// #229: Mutual cancellation request record.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CancellationRequest {
    pub initiator: Address,
    pub reason_hash: BytesN<32>,
    pub requested_at: u64,
    pub expires_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeadlineProposal {
    pub proposer: Address,
    pub new_deadline: u64,
    pub proposed_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TopUpEntry {
    pub amount: i128,
    pub timestamp: u64,
    pub cumulative_total: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartialReleaseRequest {
    pub request_id: u64,
    pub amount: i128,
    pub justification_hash: BytesN<32>,
    pub created_at: u64,
    pub response_deadline: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowTemplateConfig {
    pub arbiter: Address,
    pub token: Address,
    pub deadline_duration: u64, // seconds from escrow creation
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowTemplate {
    pub id: u32,
    pub creator: Address,
    pub config: EscrowTemplateConfig,
    pub active: bool,
}

/// Status of a milestone in a milestone-based escrow (#136).
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MilestoneStatus {
    Pending = 0,
    Approved = 1,
    Disputed = 2,
}

/// Single milestone in a milestone-based escrow (#136).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub description_hash: BytesN<32>,
    pub amount: i128,
    pub status: MilestoneStatus,
}

const MAX_MILESTONES: u32 = 20;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowBatchConfig {
    pub seller: Address,
    pub arbiter: Address,
    pub amount: i128,
    pub token: Address,
    pub deadline: u64,
    pub metadata_hash: Option<BytesN<32>>,
    pub sellers: Vec<(Address, u32)>,
    pub auto_renew: bool,
    pub renewal_count: u32,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Admin,
    ContractVersion,
    MigrationCompleted(u32),
    Paused,
    PauseReason,
    EscrowCounter,
    Escrow(u32),
    Dispute(u32),
    DeadlineProposal(u32),
    AllowedToken(Address),
    ProtocolFeeBps,
    FeeRecipient,
    TemplateCounter,
    Template(u32),
    ArbiterPool,
    NextArbiterIndex,
    ArbiterNeedsReplacement(u32),
    EscrowMetadata(u32),
    Evidence(u32, Address),
    RenewalAllowance(u32),
    DefaultDisputeTimeout,
    DefaultArbiterFeeBps,
    OracleAddress,
    MaxOracleAge,
    InsurancePool,
    InsuranceToken,
    InsuranceTriggerDays,
    InsuranceAdminConfirmed(u32),
    InsuranceClaimed(u32),
    /// Last timestamp of any buyer action for inactivity tracking (#150)
    LastBuyerAction(u32),
    /// Optional milestone schedule attached to an escrow (#136)
    EscrowMilestones(u32),
    /// Global default winner for dispute timeouts (Buyer or Seller)
    DefaultDisputeWinner,
    /// Timestamp when dispute was raised and arbiter assigned
    DisputeDeadlineStart(u32),
    /// Counter tracking arbiter timeout occurrences per arbiter
    ArbiterTimeoutCount(Address),
    /// Token whitelist contract address
    TokenWhitelistContract,
    /// #215: time-lock metadata per escrow
    TimeLockData(u32),
    /// #229: cancellation request per escrow
    CancellationRequest(u32),
    /// #229: admin-configurable cancellation penalty in basis points
    CancellationPenaltyBps,
    /// #229: admin-configurable cancellation response window in seconds
    CancellationResponseWindow,
    /// #225: max top-up as basis points of original escrow amount (e.g. 5000 = 50%)
    MaxTopUpBps,
    /// #(new): max top-up multiplier (e.g., 3 means max 3x original amount)
    MaxTopupMultiplier,
    /// #(new): partial release response deadline in seconds
    PartialReleaseResponseDeadline,
    /// #(new): pending partial release request per escrow
    PendingPartialRelease(u32),
    /// #(new): partial release request counter per escrow
    PartialReleaseCounter(u32),
    /// #225: cumulative top-up amount per escrow
    EscrowToppedUpAmount(u32),
    /// #237: seller collateral amount locked per escrow
    SellerCollateral(u32),
    /// #241: delivery proof hash submitted by seller (stores proof_hash for event; not the raw proof)
    DeliveryProofSubmitted(u32),
    /// #244: seller transfer proposal per escrow
    SellerTransferProposal(u32),
    /// #244: admin-configurable veto window in ledgers (default: 100)
    SellerTransferVetoWindow,
    /// #146: (ratee) → (total_score: u64, count: u32) for reputation
    RatingScore(Address),
    /// #146: (escrow_id, rater) → bool — prevents double-rating
    RatingSubmitted(u32, Address),
    /// Cooling-off window seconds after arbiter verdict (0 = disabled)
    ResolutionCoolingOffSeconds,
    /// Pending arbiter verdict awaiting cooling-off
    PendingVerdict(u32),
    /// (escrow_id) → (caller, reason_hash) for dispute resolution flag
    ResolutionFlag(u32),
    /// Amendment proposal for an escrow (escrow_id → AmendmentProposal)
    AmendmentProposal(u32),
    /// Amendment proposal nonce counter per escrow (escrow_id → u32)
    AmendmentNonce(u32),
    /// Admin-configurable amendment proposal expiry window in seconds
    AmendmentExpirySeconds,
    /// #272: Inspector report per escrow
    InspectorReport(u32),
    /// #272: Pending inspector replacement
    InspectorReplacement(u32),
    /// Auto-renewal: ordered list of successor escrow IDs for a given original escrow
    RenewalHistory(u32),
    /// Auto-renewal: number of renewals completed for a given escrow chain (keyed by original ID)
    RenewalsCompleted(u32),
    /// Auto-renewal: whether the buyer has cancelled future renewals for this escrow
    AutoRenewalCancelled(u32),
}

/// Overflow storage keys — split from DataKey because #[contracttype] is bounded to 50 variants.
#[derive(Clone)]
#[contracttype]
pub enum DataKey2 {
    /// #317: seller share delegation per escrow (escrow_id, original_seller) → delegate_address
    SellerShareDelegate(u32, Address),
    /// #318: conditional release condition per escrow
    ConditionalReleaseCondition(u32),
    /// #318: buyer waiver signature for conditional release
    BuyerWaiverSigned(u32),
    /// #318: seller waiver signature for conditional release
    SellerWaiverSigned(u32),
/// Overflow storage keys for escrow — DataKey is capped at 52 variants.
#[derive(Clone)]
#[contracttype]
pub enum DataKey2 {
    /// #332: BPS-based milestone states for proportional progressive release
    EscrowMilestonesV2(u32),
}

// ── #332: Milestone BPS Progressive Release ───────────────────────────────────

/// Input type for creating a bps-based milestone escrow.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneInput {
    pub name: String,
    pub release_bps: u32,
    pub description_hash: BytesN<32>,
}

#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MilestoneStateStatus {
    Pending = 0,
    Submitted = 1,
    Approved = 2,
    Rejected = 3,
}

/// On-chain milestone state for a proportional-release escrow.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MilestoneState {
    pub name: String,
    pub release_bps: u32,
    pub description_hash: BytesN<32>,
    pub status: MilestoneStateStatus,
    pub delivery_hash: Option<BytesN<32>>,
    pub rejection_hash: Option<BytesN<32>>,
}

const MAX_PROTOCOL_FEE_BPS: u32 = 200; // 2%
const MAX_ARBITER_FEE_BPS: u32 = 1_000; // 10%
const DEFAULT_RESOLUTION_COOLING_OFF_SECONDS: u64 = 24 * 60 * 60; // 24 hours
const DEFAULT_SELLER_TRANSFER_VETO_WINDOW: u32 = 100; // ledgers
const DEFAULT_AMENDMENT_EXPIRY_SECONDS: u64 = 7 * 24 * 60 * 60; // 7 days

/// Auto-renewal configuration for recurring service agreements.
/// When provided at escrow creation, the escrow will automatically re-fund
/// and restart at the end of each period for up to `max_renewals` cycles.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoRenewConfig {
    /// Maximum number of automatic renewal cycles.
    pub max_renewals: u32,
    /// Duration of each renewal period in ledgers (used to compute new deadline).
    pub renewal_interval_ledgers: u32,
}

/// #244: Pending seller role transfer proposal.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SellerTransferProposal {
    pub original_seller: Address,
    pub new_seller: Address,
    pub veto_deadline: u32, // ledger sequence
}

/// Mutual amendment proposal for post-creation term changes.
/// Both buyer and seller must sign the same proposal for it to take effect.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AmendmentProposal {
    /// Incrementing nonce so each proposal is unique even if terms repeat.
    pub nonce: u32,
    /// Proposer address (buyer or seller).
    pub proposer: Address,
    /// New escrow amount, or None to keep current.
    pub new_amount: Option<i128>,
    /// New deadline (unix timestamp), or None to keep current.
    pub new_deadline: Option<u64>,
    /// New metadata hash, or None to keep current.
    pub new_metadata_hash: Option<BytesN<32>>,
    /// Ledger timestamp when this proposal was created.
    pub proposed_at: u64,
    /// Ledger timestamp after which this proposal expires.
    pub expires_at: u64,
    /// Whether the buyer has signed this proposal.
    pub buyer_signed: bool,
    /// Whether the seller has signed this proposal.
    pub seller_signed: bool,
}

/// #272: Inspector report stored on-chain.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectorReport {
    pub inspector: Address,
    pub approved: bool,
    pub report_hash: BytesN<32>,
    pub submitted_at: u64,
}

/// #272: Pending inspector replacement request.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectorReplacement {
    pub new_inspector: Address,
    pub buyer_signed: bool,
    pub seller_signed: bool,
}

/// Verdict recorded by arbiter during cooling-off period.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingVerdict {
    pub buyer_percent: u32,
    pub arbiter: Address,
    pub recorded_at: u64,
}

mod oracle {
    use crate::PriceData;
    use soroban_sdk::{contractclient, Address, Env};

    #[allow(dead_code)]
    #[contractclient(name = "OracleClient")]
    pub trait OracleInterface {
        fn lastprice(env: Env, base: Address, quote: Address) -> Option<PriceData>;
    }
}

mod events;

#[contract]
pub struct AhjoorEscrowContract;

#[contractimpl]
impl AhjoorEscrowContract {
    /// Initialize upgrade admin and contract versioning state.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("Already initialized");
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &1u32);
        env.storage()
            .instance()
            .set(&DataKey::DefaultDisputeTimeout, &DEFAULT_DISPUTE_TIMEOUT_SECONDS);
        env.storage().instance().set(&DataKey::DefaultArbiterFeeBps, &0u32);
        env.storage().instance().set(&DataKey::InsurancePool, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::InsuranceTriggerDays, &DEFAULT_INSURANCE_TRIGGER_DAYS);
        env.storage()
            .instance()
            .set(&DataKey::MaxOracleAge, &DEFAULT_MAX_ORACLE_AGE_SECONDS);
        env.storage()
            .instance()
            .set(&DataKey::DefaultDisputeWinner, &DisputeDefaultWinner::Buyer);
        env.storage().instance().set(&DataKey::MaxTopupMultiplier, &DEFAULT_MAX_TOPUP_MULTIPLIER);
        env.storage().instance().set(&DataKey::PartialReleaseResponseDeadline, &DEFAULT_PARTIAL_RELEASE_RESPONSE_DEADLINE);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Create a new escrow. Funds are transferred from buyer to contract.
    /// Returns the escrow ID.
    pub fn create_escrow(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        metadata_hash: Option<BytesN<32>>,
        sellers: Vec<(Address, u32)>,
        auto_renew: bool,
        renewal_count: u32,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let request = EscrowCreateRequest {
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers,
            auto_renew,
            renewal_count,
            buyer_inactivity_secs: 0,
            min_lock_until: None,
            release_base: None,
            release_quote: None,
            release_comparison: None,
            release_threshold_price: None,
            arbiter_fee_bps: None,
            dispute_default_winner: None,
            auto_renew_config: None,
        };

        Self::create_escrow_core(&env, &buyer, request)
    }

    /// Create a new escrow with explicit inactivity release configuration.
    pub fn create_escrow_with_inactivity(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        metadata_hash: Option<BytesN<32>>,
        sellers: Vec<(Address, u32)>,
        renewal_count: u32,
        buyer_inactivity_secs: u64,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let auto_renew = renewal_count > 0;

        let request = EscrowCreateRequest {
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers,
            auto_renew,
            renewal_count,
            buyer_inactivity_secs,
            min_lock_until: None,
            release_base: None,
            release_quote: None,
            release_comparison: None,
            release_threshold_price: None,
            arbiter_fee_bps: None,
            dispute_default_winner: None,
            auto_renew_config: None,
        };

        Self::create_escrow_core(&env, &buyer, request)
    }

    /// Create a new escrow with optional lock, oracle condition, and per-escrow arbiter fee.
    pub fn create_escrow_v2(env: Env, buyer: Address, request: EscrowCreateRequest) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        Self::create_escrow_core(&env, &buyer, request)
    }

    /// Create a new escrow with an AutoRenewConfig for recurring service agreements.
    /// Funds are transferred from buyer to contract immediately.
    /// Returns the escrow ID.
    pub fn create_escrow_with_auto_renew(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        metadata_hash: Option<BytesN<32>>,
        auto_renew_config: AutoRenewConfig,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let request = EscrowCreateRequest {
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers: Vec::new(&env),
            auto_renew: true,
            renewal_count: auto_renew_config.max_renewals,
            buyer_inactivity_secs: 0,
            min_lock_until: None,
            release_base: None,
            release_quote: None,
            release_comparison: None,
            release_threshold_price: None,
            arbiter_fee_bps: None,
            dispute_default_winner: None,
            auto_renew_config: Some(auto_renew_config),
        };

        Self::create_escrow_core(&env, &buyer, request)
    }

    /// Create up to 10 escrows in one atomic transaction.
    /// Returns the list of contiguous escrow IDs created.
    pub fn create_escrows_batch(
        env: Env,
        buyer: Address,
        escrow_configs: Vec<EscrowBatchConfig>,
    ) -> Vec<u32> {
        Self::require_not_paused(&env);
        buyer.require_auth();

        if escrow_configs.is_empty() {
            panic!("Batch must contain at least one escrow config");
        }
        if escrow_configs.len() > MAX_BATCH_ESCROWS {
            panic!("Batch size exceeds maximum of 10 escrows");
        }

        let mut created_ids: Vec<u32> = Vec::new(&env);
        let mut first_id: Option<u32> = None;
        let mut last_id: u32 = 0;

        for i in 0..escrow_configs.len() {
            let cfg = escrow_configs.get(i).unwrap();
            let escrow_id = Self::create_escrow_core(
                &env,
                &buyer,
                EscrowCreateRequest {
                    seller: cfg.seller,
                    arbiter: cfg.arbiter,
                    amount: cfg.amount,
                    token: cfg.token,
                    deadline: cfg.deadline,
                    metadata_hash: cfg.metadata_hash,
                    sellers: cfg.sellers,
                    auto_renew: cfg.auto_renew,
                    renewal_count: cfg.renewal_count,
                    buyer_inactivity_secs: 0,
                    min_lock_until: None,
                    release_base: None,
                    release_quote: None,
                    release_comparison: None,
                    release_threshold_price: None,
                    arbiter_fee_bps: None,
                    dispute_default_winner: None,
                    auto_renew_config: None,
                },
            );

            if first_id.is_none() {
                first_id = Some(escrow_id);
            }
            last_id = escrow_id;
            created_ids.push_back(escrow_id);
        }

        events::emit_batch_escrow_created(
            &env,
            escrow_configs.len(),
            first_id.expect("Batch contained no escrows"),
            last_id,
        );

        created_ids
    }

    // ─── #272: Escrow Third-Party Inspector Role ─────────────────────────────

    /// Create an escrow with an optional inspector for quality gate.
    pub fn create_escrow_with_inspector(
        env: Env,
        buyer: Address,
        request: EscrowCreateRequest,
        inspector: Option<Address>,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();
        let escrow_id = Self::create_escrow_core(&env, &buyer, request);
        if let Some(ref insp) = inspector {
            let mut escrow: Escrow = env
                .storage().persistent().get(&DataKey::Escrow(escrow_id)).expect("Escrow not found");
            escrow.extensions.inspector = Some(insp.clone());
            env.storage().persistent().set(&DataKey::Escrow(escrow_id), &escrow);
            env.storage().persistent().extend_ttl(
                &DataKey::Escrow(escrow_id), PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
            );
        }
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        escrow_id
    }

    /// Seller marks work complete. If inspector is set, moves to AwaitingInspection.
    pub fn seller_mark_complete(env: Env, seller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        seller.require_auth();
        let mut escrow: Escrow = env
            .storage().persistent().get(&DataKey::Escrow(escrow_id)).expect("Escrow not found");
        if escrow.seller != seller { panic!("Only seller can mark complete"); }
        if !Self::is_open_escrow_status(escrow.status) { panic!("Escrow is not active"); }
        if escrow.extensions.inspector.is_none() { panic!("No inspector set; use release_escrow directly"); }
        escrow.status = EscrowStatus::AwaitingInspection;
        env.storage().persistent().set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id), PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_seller_marked_complete(&env, escrow_id, seller);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Inspector submits their verdict. Moves status to InspectionPassed or InspectionFailed.
    pub fn submit_inspection_report(
        env: Env,
        inspector: Address,
        escrow_id: u32,
        approved: bool,
        report_hash: BytesN<32>,
    ) {
        Self::require_not_paused(&env);
        inspector.require_auth();
        let mut escrow: Escrow = env
            .storage().persistent().get(&DataKey::Escrow(escrow_id)).expect("Escrow not found");
        if escrow.status != EscrowStatus::AwaitingInspection {
            panic!("Escrow is not awaiting inspection");
        }
        let stored_inspector = escrow.extensions.inspector.clone().expect("No inspector set");
        if inspector != stored_inspector { panic!("Only the assigned inspector can submit a report"); }
        let report = InspectorReport {
            inspector: inspector.clone(),
            approved,
            report_hash: report_hash.clone(),
            submitted_at: env.ledger().timestamp(),
        };
        env.storage().persistent().set(&DataKey::InspectorReport(escrow_id), &report);
        env.storage().persistent().extend_ttl(
            &DataKey::InspectorReport(escrow_id), PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
        );
        escrow.status = if approved { EscrowStatus::InspectionPassed } else { EscrowStatus::InspectionFailed };
        env.storage().persistent().set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id), PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_inspection_report_submitted(&env, escrow_id, inspector, approved, report_hash);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Replace inspector by mutual agreement (both buyer and seller must call).
    pub fn replace_inspector(env: Env, caller: Address, escrow_id: u32, new_inspector: Address) {
        Self::require_not_paused(&env);
        caller.require_auth();
        let escrow: Escrow = env
            .storage().persistent().get(&DataKey::Escrow(escrow_id)).expect("Escrow not found");
        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can propose inspector replacement");
        }
        if escrow.extensions.inspector.is_none() { panic!("No inspector set on this escrow"); }
        let key = DataKey::InspectorReplacement(escrow_id);
        let mut replacement: InspectorReplacement = env
            .storage().persistent().get(&key)
            .unwrap_or(InspectorReplacement {
                new_inspector: new_inspector.clone(),
                buyer_signed: false,
                seller_signed: false,
            });
        if replacement.new_inspector != new_inspector {
            replacement = InspectorReplacement { new_inspector: new_inspector.clone(), buyer_signed: false, seller_signed: false };
        }
        if caller == escrow.buyer { replacement.buyer_signed = true; } else { replacement.seller_signed = true; }
        if replacement.buyer_signed && replacement.seller_signed {
            let mut updated = escrow.clone();
            let old_inspector = updated.extensions.inspector.clone().unwrap();
            updated.extensions.inspector = Some(new_inspector.clone());
            if updated.status == EscrowStatus::AwaitingInspection || updated.status == EscrowStatus::InspectionFailed {
                updated.status = EscrowStatus::Active;
            }
            env.storage().persistent().set(&DataKey::Escrow(escrow_id), &updated);
            env.storage().persistent().extend_ttl(
                &DataKey::Escrow(escrow_id), PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
            );
            env.storage().persistent().remove(&key);
            events::emit_inspector_replaced(&env, escrow_id, old_inspector, new_inspector);
        } else {
            env.storage().persistent().set(&key, &replacement);
            env.storage().persistent().extend_ttl(
                &key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
            );
        }
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the inspector report for an escrow.
    pub fn get_inspector_report(env: Env, escrow_id: u32) -> Option<InspectorReport> {
        env.storage().persistent().get(&DataKey::InspectorReport(escrow_id))
    }

    fn create_escrow_core(env: &Env, buyer: &Address, request: EscrowCreateRequest) -> u32 {
        let EscrowCreateRequest {
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers,
            auto_renew,
            renewal_count,
            buyer_inactivity_secs,
            min_lock_until,
            release_base,
            release_quote,
            release_comparison,
            release_threshold_price,
            arbiter_fee_bps,
            dispute_default_winner,
            auto_renew_config,
        } = request;

        if amount <= 0 {
            panic!("Escrow amount must be positive");
        }

        if deadline <= env.ledger().timestamp() {
            panic!("Deadline must be in the future");
        }

        if let Some(lock_until) = min_lock_until {
            if deadline <= lock_until {
                panic!("Deadline must be after min_lock_until");
            }
        }

        if let Some(fee_bps) = arbiter_fee_bps {
            if fee_bps > MAX_ARBITER_FEE_BPS {
                panic!("Arbiter fee exceeds maximum of 1000 bps");
            }
        }

        let has_any_release_condition = release_base.is_some()
            || release_quote.is_some()
            || release_comparison.is_some()
            || release_threshold_price.is_some();
        let has_full_release_condition = release_base.is_some()
            && release_quote.is_some()
            && release_comparison.is_some()
            && release_threshold_price.is_some();
        if has_any_release_condition && !has_full_release_condition {
            panic!("Incomplete release condition");
        }

        if let Some(threshold) = release_threshold_price {
            if threshold <= 0 {
                panic!("Release condition threshold must be positive");
            }
        }

        if let Some(comparison) = release_comparison {
            if comparison != ORACLE_COMPARISON_LESS_OR_EQUAL
                && comparison != ORACLE_COMPARISON_GREATER_OR_EQUAL
            {
                panic!("Invalid release comparison");
            }
        }

        // Token whitelist validation
        Self::require_token_allowed(&env, &token);

        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }

        // Validate multi-party sellers if provided
        let resolved_sellers: Vec<(Address, u32)> = if sellers.is_empty() {
            // Single-seller mode: wrap seller with 10000 bps
            let mut v = Vec::new(env);
            v.push_back((seller.clone(), 10_000u32));
            v
        } else {
            if sellers.len() > 5 {
                panic!("Maximum 5 sellers allowed");
            }
            let mut total_bps: u32 = 0;
            for i in 0..sellers.len() {
                let (_, bps) = sellers.get(i).unwrap();
                total_bps += bps;
            }
            if total_bps != 10_000 {
                panic!("Seller allocations must sum to 10000 bps");
            }
            sellers
        };

        // Transfer tokens from buyer to contract (escrow)
        let client = token::Client::new(env, &token);
        client.transfer(buyer, &env.current_contract_address(), &amount);

        let escrow_id = Self::next_escrow_id(env);

        // Primary seller is the first in the list (or the passed seller for single-party)
        let primary_seller = if resolved_sellers.len() == 1 {
            resolved_sellers.get(0).unwrap().0
        } else {
            resolved_sellers.get(0).unwrap().0
        };

        let now = env.ledger().timestamp();

        let escrow = Escrow {
            id: escrow_id,
            buyer: buyer.clone(),
            seller: primary_seller.clone(),
            arbiter: arbiter.clone(),
            amount,
            original_amount: amount,
            token: token.clone(),
            status: EscrowStatus::Active,
            created_at: now,
            deadline,
            metadata_hash: metadata_hash.clone(),
            sellers: resolved_sellers.clone(),
            extensions: EscrowExtensions {
                auto_renew,
                renewal_count,
                renewals_remaining: if renewal_count == 0 { 0 } else { renewal_count },
                dispute_timeout_seconds: None,
                buyer_inactivity_secs,
                min_lock_until,
                release_base,
                release_quote,
                release_comparison,
                release_threshold_price,
                arbiter_fee_bps,
                dispute_default_winner,
                required_collateral_bps: 0,
                collateral_forfeit_bps: 0,
                collateral_deposit_deadline: 0,
                collateral_amount: 0,
                delivery_proof_hash: None,
                inspector: None,
                auto_renew_config,
                renewals_completed: 0,
            },
            top_up_history: Vec::new(env),
            top_up_acknowledged: true,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // #150: Initialize LastBuyerAction to creation timestamp
        if buyer_inactivity_secs > 0 {
            env.storage()
                .persistent()
                .set(&DataKey::LastBuyerAction(escrow_id), &now);
            env.storage().persistent().extend_ttl(
                &DataKey::LastBuyerAction(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        // Store metadata separately with timestamp if provided
        if let Some(ref hash) = metadata_hash {
            env.storage().persistent().set(
                &DataKey::EscrowMetadata(escrow_id),
                &(hash.clone(), env.ledger().timestamp()),
            );
            env.storage().persistent().extend_ttl(
                &DataKey::EscrowMetadata(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        events::emit_escrow_created(
            env,
            escrow_id,
            buyer.clone(),
            primary_seller,
            arbiter,
            amount,
            token,
            deadline,
        );

        // Emit multi-party event if more than one seller
        if resolved_sellers.len() > 1 {
            events::emit_multi_party_escrow_created(env, escrow_id, resolved_sellers.len());
        }

        if let Some(lock_until) = escrow.extensions.min_lock_until {
            events::emit_escrow_time_locked(env, escrow_id, lock_until);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        escrow_id
    }

    /// Create a new escrow with an explicit per-escrow dispute timeout override.
    /// Uses renewal_count > 0 to enable auto_renew while staying within Soroban's
    /// 10-parameter contract function limit.
    pub fn create_escrow_w_timeout(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        metadata_hash: Option<BytesN<32>>,
        sellers: Vec<(Address, u32)>,
        renewal_count: u32,
        dispute_timeout_seconds: u64,
    ) -> u32 {
        if dispute_timeout_seconds == 0 {
            panic!("dispute_timeout_seconds must be positive");
        }

        let auto_renew = renewal_count > 0;
        let escrow_id = Self::create_escrow(
            env.clone(),
            buyer,
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers,
            auto_renew,
            renewal_count,
        );

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        escrow.extensions.dispute_timeout_seconds = Some(dispute_timeout_seconds);

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        escrow_id
    }

    /// Release escrowed funds to seller. Can be called by buyer or arbiter.
    pub fn release_escrow(env: Env, caller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        let renewal_source = escrow.clone();

        // #272: Block release if inspection is pending
        if escrow.status == EscrowStatus::AwaitingInspection {
            panic!("InspectionPending: inspector must submit report before release");
        }

        if escrow.status == EscrowStatus::InspectionPassed {
            // Allow release from InspectionPassed — fall through
        } else if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if caller != escrow.buyer && caller != escrow.arbiter {
            panic!("Only buyer or arbiter can release escrow");
        }

        Self::require_unlocked(&env, &escrow);

        // #150: Track buyer activity
        if caller == escrow.buyer {
            Self::update_last_buyer_action(&env, &escrow);
        }

        let total = escrow.amount;

        Self::transfer_to_sellers(&env, &escrow, total, escrow_id);

        // #237: Return seller collateral on buyer-approved release
        let collateral: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::SellerCollateral(escrow_id))
            .unwrap_or(0);
        if collateral > 0 {
            let client = token::Client::new(&env, &escrow.token);
            client.transfer(&env.current_contract_address(), &escrow.seller, &collateral);
            events::emit_collateral_returned(&env, escrow_id, escrow.seller.clone(), collateral);
            env.storage().persistent().remove(&DataKey::SellerCollateral(escrow_id));
        }

        escrow.status = EscrowStatus::Released;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        Self::try_auto_renew(&env, escrow_id, &renewal_source);
    }

    /// Trigger conditional release by verifying oracle condition (#318)
    pub fn trigger_conditional_release(env: Env, caller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        // Get conditional release condition
        let condition: ConditionalRelease = env
            .storage()
            .persistent()
            .get(&DataKey2::ConditionalReleaseCondition(escrow_id))
            .expect("No conditional release set for this escrow");

        // Verify oracle is whitelisted
        let whitelist: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::OracleWhitelist)
            .unwrap_or(Vec::new(&env));
        
        let mut is_whitelisted = false;
        for addr in whitelist.iter() {
            if addr == condition.oracle_contract {
                is_whitelisted = true;
                break;
            }
        }
        if !is_whitelisted {
            panic!("Oracle contract not whitelisted");
        }

        // Call oracle contract to get condition value
        let oracle_client = oracle::OracleClient::new(&env, &condition.oracle_contract);
        let condition_value: i128 = oracle_client.lastprice(&condition.oracle_contract, &condition.oracle_contract)
            .map(|pd| pd.price)
            .unwrap_or(0);

        // Check if condition is met (>= expected_value)
        if condition_value < condition.expected_value {
            panic!("Condition not met");
        }

        // Condition met, proceed with release
        events::emit_conditional_release_triggered(&env, escrow_id, condition.oracle_contract, condition_value);

        // Call release_escrow logic
        Self::release_escrow(env, caller, escrow_id);
    }

    /// Waive conditional release by mutual agreement (#318)
    pub fn waive_release_condition(env: Env, caller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can waive condition");
        }

        // Check if condition exists
        if env
            .storage()
            .persistent()
            .get::<_, ConditionalRelease>(&DataKey2::ConditionalReleaseCondition(escrow_id))
            .is_none()
        {
            panic!("No conditional release set for this escrow");
        }

        // Track signatures
        if caller == escrow.buyer {
            env.storage()
                .persistent()
                .set(&DataKey2::BuyerWaiverSigned(escrow_id), &true);
        } else {
            env.storage()
                .persistent()
                .set(&DataKey2::SellerWaiverSigned(escrow_id), &true);
        }

        // Check if both have signed
        let buyer_signed: bool = env
            .storage()
            .persistent()
            .get(&DataKey2::BuyerWaiverSigned(escrow_id))
            .unwrap_or(false);
        let seller_signed: bool = env
            .storage()
            .persistent()
            .get(&DataKey2::SellerWaiverSigned(escrow_id))
            .unwrap_or(false);

        if buyer_signed && seller_signed {
            // Remove condition
            env.storage()
                .persistent()
                .remove(&DataKey2::ConditionalReleaseCondition(escrow_id));
            env.storage()
                .persistent()
                .remove(&DataKey2::BuyerWaiverSigned(escrow_id));
            env.storage()
                .persistent()
                .remove(&DataKey2::SellerWaiverSigned(escrow_id));

            events::emit_release_condition_waived(&env, escrow_id);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Submit evidence hash anchors for an escrow dispute workflow.
    pub fn submit_evidence(
        env: Env,
        party: Address,
        escrow_id: u32,
        evidence_hash: BytesN<32>,
        evidence_uri_hash: BytesN<32>,
    ) {
        Self::require_not_paused(&env);
        party.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if party != escrow.buyer && party != escrow.seller {
            panic!("Only buyer or seller can submit evidence");
        }

        // #150: Track buyer activity
        if party == escrow.buyer {
            Self::update_last_buyer_action(&env, &escrow);
        }

        let key = DataKey::Evidence(escrow_id, party.clone());
        let mut entries: Vec<EvidenceSubmission> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        if entries.len() >= MAX_EVIDENCE_ENTRIES_PER_PARTY {
            panic!("Maximum evidence entries reached for this party");
        }

        entries.push_back(EvidenceSubmission {
            evidence_hash: evidence_hash.clone(),
            evidence_uri_hash,
            submitted_at: env.ledger().timestamp(),
        });

        env.storage().persistent().set(&key, &entries);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_evidence_submitted(&env, escrow_id, party, evidence_hash);
    }

    /// Returns evidence submissions for buyer and seller in one call.
    pub fn get_evidence(env: Env, escrow_id: u32) -> Vec<(Address, Vec<EvidenceSubmission>)> {
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        let mut all: Vec<(Address, Vec<EvidenceSubmission>)> = Vec::new(&env);

        let buyer_key = DataKey::Evidence(escrow_id, escrow.buyer.clone());
        let buyer_entries: Vec<EvidenceSubmission> = env
            .storage()
            .persistent()
            .get(&buyer_key)
            .unwrap_or(Vec::new(&env));
        if !buyer_entries.is_empty() {
            all.push_back((escrow.buyer.clone(), buyer_entries));
        }

        let seller_key = DataKey::Evidence(escrow_id, escrow.seller.clone());
        let seller_entries: Vec<EvidenceSubmission> = env
            .storage()
            .persistent()
            .get(&seller_key)
            .unwrap_or(Vec::new(&env));
        if !seller_entries.is_empty() {
            all.push_back((escrow.seller, seller_entries));
        }

        all
    }

    /// Buyer pre-approves renewal cycles for a specific escrow chain.
    pub fn set_renewal_allowance(env: Env, buyer: Address, escrow_id: u32, total_renewals: u32) {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if buyer != escrow.buyer {
            panic!("Only buyer can set renewal allowance");
        }

        if !escrow.extensions.auto_renew {
            panic!("Auto-renew is not enabled for this escrow");
        }

        let total_amount = escrow.amount * total_renewals as i128;
        let expiration_ledger = env.ledger().sequence().saturating_add(100_000);
        let token_client = token::Client::new(&env, &escrow.token);
        token_client.approve(
            &buyer,
            &env.current_contract_address(),
            &total_amount,
            &expiration_ledger,
        );

        env.storage()
            .persistent()
            .set(&DataKey::RenewalAllowance(escrow_id), &total_renewals);
        env.storage().persistent().extend_ttl(
            &DataKey::RenewalAllowance(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    /// Buyer can disable auto-renew at any time.
    pub fn cancel_auto_renew(env: Env, buyer: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if buyer != escrow.buyer {
            panic!("Only buyer can cancel auto-renew");
        }

        escrow.extensions.auto_renew = false;
        escrow.extensions.renewals_remaining = 0;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().remove(&DataKey::RenewalAllowance(escrow_id));
    }

    /// Cancel future auto-renewals for an escrow configured with AutoRenewConfig.
    /// Buyer can call this at any time before the current period's release.
    /// After cancellation, no new renewal will be triggered on release.
    pub fn cancel_auto_renewal(env: Env, buyer: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if buyer != escrow.buyer {
            panic!("Only buyer can cancel auto-renewal");
        }

        if escrow.extensions.auto_renew_config.is_none() {
            panic!("No AutoRenewConfig set on this escrow");
        }

        env.storage()
            .persistent()
            .set(&DataKey::AutoRenewalCancelled(escrow_id), &true);
        env.storage().persistent().extend_ttl(
            &DataKey::AutoRenewalCancelled(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_auto_renewal_cancelled(&env, escrow_id);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns the ordered list of successor escrow IDs created by auto-renewals
    /// for the given original escrow ID.
    pub fn get_renewal_history(env: Env, escrow_id: u32) -> Vec<u32> {
        env.storage()
            .persistent()
            .get(&DataKey::RenewalHistory(escrow_id))
            .unwrap_or(Vec::new(&env))
    }

    /// Release part of the escrowed funds to seller. Can be called by buyer or arbiter.
    pub fn partial_release(env: Env, caller: Address, escrow_id: u32, release_amount: i128) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if caller != escrow.buyer && caller != escrow.arbiter {
            panic!("Only buyer or arbiter can release escrow");
        }

        Self::require_unlocked(&env, &escrow);

        if release_amount <= 0 {
            panic!("Release amount must be positive");
        }

        if release_amount > escrow.amount {
            panic!("Release amount exceeds escrow balance");
        }

        let client = token::Client::new(&env, &escrow.token);
        client.transfer(
            &env.current_contract_address(),
            &escrow.seller,
            &release_amount,
        );

        escrow.amount -= release_amount;
        escrow.status = if escrow.amount == 0 {
            EscrowStatus::Released
        } else {
            EscrowStatus::PartiallyReleased
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_partial_released(&env, escrow_id, release_amount, escrow.amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- Milestone-based escrow release (#136) ---

    /// Create a milestone-based escrow.
    ///
    /// `milestones[i].amount` must be positive and the sum of all milestone
    /// amounts must equal `total_amount`. Tokens for `total_amount` are
    /// transferred from `buyer` into the contract up front; each milestone is
    /// released independently via `approve_milestone`. When all milestones are
    /// approved, the escrow transitions automatically to `Released`.
    pub fn create_milestone_escrow(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        token: Address,
        deadline: u64,
        milestones: Vec<Milestone>,
    ) -> u32 {
        buyer.require_auth();

        if milestones.is_empty() {
            panic!("At least one milestone required");
        }
        if milestones.len() > MAX_MILESTONES {
            panic!("Too many milestones");
        }

        let mut total_amount: i128 = 0;
        for i in 0..milestones.len() {
            let m = milestones.get(i).unwrap();
            if m.amount <= 0 {
                panic!("Milestone amount must be positive");
            }
            if m.status != MilestoneStatus::Pending {
                panic!("New milestones must start as Pending");
            }
            total_amount += m.amount;
        }

        let request = EscrowCreateRequest {
            seller,
            arbiter,
            amount: total_amount,
            token,
            deadline,
            metadata_hash: None,
            sellers: Vec::new(&env),
            auto_renew: false,
            renewal_count: 0,
            buyer_inactivity_secs: 0,
            min_lock_until: None,
            release_base: None,
            release_quote: None,
            release_comparison: None,
            release_threshold_price: None,
            arbiter_fee_bps: None,
            dispute_default_winner: None,
        };
        let escrow_id = Self::create_escrow_core(&env, &buyer, request);

        env.storage()
            .persistent()
            .set(&DataKey::EscrowMilestones(escrow_id), &milestones);
        env.storage().persistent().extend_ttl(
            &DataKey::EscrowMilestones(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        escrow_id
    }

    /// Approve a single milestone, releasing its `amount` to the seller.
    ///
    /// Callable by buyer or arbiter. The targeted milestone must be `Pending`.
    /// Disputed milestones do not block other milestones from being approved.
    /// When every milestone reaches a terminal state and at least one was
    /// approved, the escrow auto-transitions to `Released`.
    pub fn approve_milestone(
        env: Env,
        caller: Address,
        escrow_id: u32,
        milestone_index: u32,
    ) {
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status == EscrowStatus::Released
            || escrow.status == EscrowStatus::Refunded
            || escrow.status == EscrowStatus::Resolved
        {
            panic!("Escrow already terminal");
        }

        if caller != escrow.buyer && caller != escrow.arbiter {
            panic!("Only buyer or arbiter can approve milestones");
        }

        let mut milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowMilestones(escrow_id))
            .expect("Escrow has no milestones");

        if milestone_index >= milestones.len() {
            panic!("Milestone index out of range");
        }

        let mut milestone = milestones.get(milestone_index).unwrap();
        if milestone.status != MilestoneStatus::Pending {
            panic!("Milestone not pending");
        }

        let client = token::Client::new(&env, &escrow.token);
        client.transfer(&env.current_contract_address(), &escrow.seller, &milestone.amount);

        let amount_released = milestone.amount;
        milestone.status = MilestoneStatus::Approved;
        milestones.set(milestone_index, milestone);

        env.storage()
            .persistent()
            .set(&DataKey::EscrowMilestones(escrow_id), &milestones);
        env.storage().persistent().extend_ttl(
            &DataKey::EscrowMilestones(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Auto-transition to Released when every milestone is in a terminal
        // state and at least one was approved.
        let mut all_terminal = true;
        let mut any_approved = false;
        for i in 0..milestones.len() {
            let m = milestones.get(i).unwrap();
            match m.status {
                MilestoneStatus::Pending => all_terminal = false,
                MilestoneStatus::Approved => any_approved = true,
                MilestoneStatus::Disputed => {}
            }
        }
        if all_terminal && any_approved && escrow.status == EscrowStatus::Active {
            escrow.status = EscrowStatus::Released;
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(escrow_id), &escrow);
            env.storage().persistent().extend_ttl(
                &DataKey::Escrow(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        events::emit_milestone_approved(&env, escrow_id, milestone_index, amount_released);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Read the current milestone schedule for a milestone-based escrow.
    pub fn get_milestones(env: Env, escrow_id: u32) -> Vec<Milestone> {
        env.storage()
            .persistent()
            .get(&DataKey::EscrowMilestones(escrow_id))
            .expect("Escrow has no milestones")
    }

    /// Dispute an escrow. Can be called by buyer or seller.
    /// Pass `dispute_amount` equal to the full escrow amount for a full dispute,
    /// or less for a partial dispute (undisputed portion is released to seller immediately).
    pub fn dispute_escrow(
        env: Env,
        caller: Address,
        escrow_id: u32,
        reason: String,
        dispute_amount: i128,
    ) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can dispute escrow");
        }

        if dispute_amount <= 0 || dispute_amount > escrow.amount {
            panic!("dispute_amount must be > 0 and <= escrow amount");
        }

        // #150: Track buyer activity
        if caller == escrow.buyer {
            Self::update_last_buyer_action(&env, &escrow);
        }

        let released_amount = escrow.amount - dispute_amount;

        // Release undisputed portion to seller immediately
        if released_amount > 0 {
            let client = token::Client::new(&env, &escrow.token);
            client.transfer(
                &env.current_contract_address(),
                &escrow.seller,
                &released_amount,
            );
        }

        escrow.amount = dispute_amount;
        escrow.status = if released_amount > 0 {
            EscrowStatus::PartiallyDisputed
        } else {
            EscrowStatus::Disputed
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        let dispute = Dispute {
            escrow_id,
            reason: reason.clone(),
            created_at: env.ledger().timestamp(),
            resolved: false,
            dispute_amount,
            timeout_seconds: escrow.extensions.dispute_timeout_seconds,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Dispute(escrow_id), &dispute);
        env.storage().persistent().extend_ttl(
            &DataKey::Dispute(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Record dispute deadline start timestamp for timeout tracking
        let now = env.ledger().timestamp();
        env.storage()
            .persistent()
            .set(&DataKey::DisputeDeadlineStart(escrow_id), &now);
        env.storage().persistent().extend_ttl(
            &DataKey::DisputeDeadlineStart(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        if released_amount > 0 {
            events::emit_partial_dispute_raised(&env, escrow_id, dispute_amount, released_amount);
        } else {
            events::emit_escrow_disputed(&env, escrow_id, caller, reason);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Resolve a dispute. Only arbiter can call this.
    /// `buyer_percent` is 0–100; seller receives the remainder.
    /// Use 100 for full buyer win, 0 for full seller win, or any value in between for a split.
    /// If a cooling-off window is configured, the escrow enters CoolingOff state and funds
    /// are NOT moved until `finalize_resolution` is called after the window expires.
    pub fn resolve_dispute(env: Env, arbiter: Address, escrow_id: u32, buyer_percent: u32) {
        Self::require_not_paused(&env);
        arbiter.require_auth();

        if buyer_percent > 100 {
            panic!("buyer_percent must be between 0 and 100");
        }

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::Disputed
            && escrow.status != EscrowStatus::PartiallyDisputed
        {
            panic!("Escrow is not disputed");
        }

        if arbiter != escrow.arbiter {
            panic!("Only arbiter can resolve dispute");
        }

        let cooling_off_seconds: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ResolutionCoolingOffSeconds)
            .unwrap_or(0);

        if cooling_off_seconds > 0 {
            // Enter cooling-off state — record verdict, do NOT move funds yet
            let verdict = PendingVerdict {
                buyer_percent,
                arbiter: arbiter.clone(),
                recorded_at: env.ledger().timestamp(),
            };
            env.storage()
                .persistent()
                .set(&DataKey::PendingVerdict(escrow_id), &verdict);
            env.storage().persistent().extend_ttl(
                &DataKey::PendingVerdict(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );

            escrow.status = EscrowStatus::CoolingOff;
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(escrow_id), &escrow);
            env.storage().persistent().extend_ttl(
                &DataKey::Escrow(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );

            events::emit_resolution_cooling_off(&env, escrow_id, buyer_percent, arbiter, env.ledger().timestamp() + cooling_off_seconds);

            env.storage()
                .instance()
                .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
            return;
        }

        // No cooling-off configured — execute immediately (legacy path)
        Self::execute_verdict(&env, escrow_id, escrow, buyer_percent, arbiter);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Flag a procedural error during the cooling-off window.
    /// Only the losing party (buyer if seller wins, seller if buyer wins) may call this.
    /// Pauses fund release and escalates to admin queue.
    pub fn flag_resolution_error(env: Env, caller: Address, escrow_id: u32, reason_hash: BytesN<32>) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::CoolingOff {
            panic!("Escrow is not in cooling-off state");
        }

        let verdict: PendingVerdict = env
            .storage()
            .persistent()
            .get(&DataKey::PendingVerdict(escrow_id))
            .expect("No pending verdict");

        // Only buyer or seller may flag
        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can flag a resolution error");
        }

        // Enforce cooling-off window has not expired
        let cooling_off_seconds: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ResolutionCoolingOffSeconds)
            .unwrap_or(0);
        let now = env.ledger().timestamp();
        if now > verdict.recorded_at + cooling_off_seconds {
            panic!("Cooling-off window has expired");
        }

        // Prevent duplicate flags
        if env.storage().persistent().has(&DataKey::ResolutionFlag(escrow_id)) {
            panic!("Resolution already flagged");
        }

        env.storage()
            .persistent()
            .set(&DataKey::ResolutionFlag(escrow_id), &(caller.clone(), reason_hash.clone()));
        env.storage().persistent().extend_ttl(
            &DataKey::ResolutionFlag(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_resolution_flagged(&env, escrow_id, caller, reason_hash);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Finalize a resolution after the cooling-off window has elapsed with no flag.
    /// Anyone can call this once the window expires.
    pub fn finalize_resolution(env: Env, escrow_id: u32) {
        Self::require_not_paused(&env);

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::CoolingOff {
            panic!("Escrow is not in cooling-off state");
        }

        let verdict: PendingVerdict = env
            .storage()
            .persistent()
            .get(&DataKey::PendingVerdict(escrow_id))
            .expect("No pending verdict");

        let cooling_off_seconds: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ResolutionCoolingOffSeconds)
            .unwrap_or(0);

        let now = env.ledger().timestamp();
        if now <= verdict.recorded_at + cooling_off_seconds {
            panic!("Cooling-off window has not elapsed");
        }

        // Ensure no unresolved flag is blocking release
        if env.storage().persistent().has(&DataKey::ResolutionFlag(escrow_id)) {
            panic!("Resolution is flagged; admin must review before finalization");
        }

        let buyer_percent = verdict.buyer_percent;
        let arbiter = verdict.arbiter.clone();

        // Clean up verdict
        env.storage().persistent().remove(&DataKey::PendingVerdict(escrow_id));

        Self::execute_verdict(&env, escrow_id, escrow, buyer_percent, arbiter);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin clears a resolution flag (after review) and allows finalization to proceed.
    pub fn clear_resolution_flag(env: Env, escrow_id: u32) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        admin.require_auth();

        if !env.storage().persistent().has(&DataKey::ResolutionFlag(escrow_id)) {
            panic!("No flag to clear");
        }

        env.storage().persistent().remove(&DataKey::ResolutionFlag(escrow_id));

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin sets the global cooling-off window in seconds (0 = disabled).
    pub fn set_resolution_cooloff_secs(env: Env, admin: Address, seconds: u64) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can configure cooling-off period");
        }

        env.storage()
            .instance()
            .set(&DataKey::ResolutionCoolingOffSeconds, &seconds);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Internal: execute a verdict (transfer funds per buyer_percent split).
    fn execute_verdict(env: &Env, escrow_id: u32, mut escrow: Escrow, buyer_percent: u32, arbiter: Address) {
        let client = token::Client::new(env, &escrow.token);

        let arbiter_fee_bps = Self::effective_arbiter_fee_bps(env, &escrow);
        let arbiter_fee = (escrow.amount * arbiter_fee_bps as i128) / 10_000;

        if arbiter_fee > 0 {
            client.transfer(&env.current_contract_address(), &arbiter, &arbiter_fee);
            events::emit_arbiter_fee_paid(env, escrow_id, arbiter.clone(), arbiter_fee);
        }

        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0);
        let protocol_fee = (escrow.amount * fee_bps as i128) / 10_000;

        if protocol_fee > 0 {
            let fee_recipient: Address = env
                .storage()
                .instance()
                .get(&DataKey::FeeRecipient)
                .expect("FeeRecipient not set");
            client.transfer(
                &env.current_contract_address(),
                &fee_recipient,
                &protocol_fee,
            );
            events::emit_protocol_fee_paid(env, escrow_id, protocol_fee, fee_recipient);
        }

        let distributable = escrow.amount - protocol_fee - arbiter_fee;

        if distributable < 0 {
            panic!("Fee configuration exceeds escrow amount");
        }

        let seller_percent = 100 - buyer_percent;

        let buyer_amount = (distributable * buyer_percent as i128) / 100;
        let seller_amount = distributable - buyer_amount;

        if buyer_amount > 0 {
            client.transfer(
                &env.current_contract_address(),
                &escrow.buyer,
                &buyer_amount,
            );
        }
        if seller_amount > 0 {
            client.transfer(
                &env.current_contract_address(),
                &escrow.seller,
                &seller_amount,
            );
        }

        // #237: Handle seller collateral
        let collateral: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::SellerCollateral(escrow_id))
            .unwrap_or(0);
        if collateral > 0 {
            if buyer_percent > 0 {
                // Buyer-favour: forfeit collateral_forfeit_bps to buyer, return remainder to seller
                let forfeit = (collateral * escrow.extensions.collateral_forfeit_bps as i128) / 10_000;
                let returned = collateral - forfeit;
                if forfeit > 0 {
                    client.transfer(&env.current_contract_address(), &escrow.buyer, &forfeit);
                    events::emit_collateral_forfeited(env, escrow_id, forfeit, escrow.buyer.clone());
                }
                if returned > 0 {
                    client.transfer(&env.current_contract_address(), &escrow.seller, &returned);
                    events::emit_collateral_returned(env, escrow_id, escrow.seller.clone(), returned);
                }
            } else {
                // Seller-favour: return full collateral
                client.transfer(&env.current_contract_address(), &escrow.seller, &collateral);
                events::emit_collateral_returned(env, escrow_id, escrow.seller.clone(), collateral);
            }
            env.storage().persistent().remove(&DataKey::SellerCollateral(escrow_id));
        }

        escrow.status = if buyer_percent == 100 {
            EscrowStatus::Refunded
        } else if buyer_percent == 0 {
            EscrowStatus::Released
        } else {
            EscrowStatus::Resolved
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        if let Some(mut dispute) = env
            .storage()
            .persistent()
            .get::<DataKey, Dispute>(&DataKey::Dispute(escrow_id))
        {
            dispute.resolved = true;
            env.storage()
                .persistent()
                .set(&DataKey::Dispute(escrow_id), &dispute);
        }

        events::emit_dispute_resolved_split(
            env,
            escrow_id,
            buyer_percent,
            seller_percent,
            buyer_amount,
            seller_amount,
            arbiter.clone(),
        );
        let release_to_seller = buyer_percent == 0;
        events::emit_dispute_resolved(env, escrow_id, release_to_seller, arbiter.clone());
        events::emit_resolution_finalized(env, escrow_id, buyer_percent, arbiter);
    }

    /// Returns true when an unresolved dispute has exceeded its timeout window.
    /// Uses per-escrow dispute timeout when set, otherwise falls back to admin default.
    pub fn check_escalation(env: Env, escrow_id: u32) -> bool {
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::Disputed && escrow.status != EscrowStatus::PartiallyDisputed {
            return false;
        }

        let dispute: Dispute = env
            .storage()
            .persistent()
            .get(&DataKey::Dispute(escrow_id))
            .expect("Dispute not found");

        if dispute.resolved {
            return false;
        }

        let default_timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::DefaultDisputeTimeout)
            .unwrap_or(DEFAULT_DISPUTE_TIMEOUT_SECONDS);
        let effective_timeout = dispute.timeout_seconds.unwrap_or(default_timeout);

        let elapsed = env.ledger().timestamp() - dispute.created_at;
        if elapsed > effective_timeout {
            events::emit_dispute_escalated(&env, escrow_id, effective_timeout);
            return true;
        }

        false
    }

    /// Admin updates the default dispute escalation timeout (seconds).
    pub fn update_default_dispute_timeout(env: Env, admin: Address, timeout_seconds: u64) {
        Self::require_admin(&env, &admin);
        if timeout_seconds == 0 {
            panic!("Timeout must be positive");
        }

        env.storage()
            .instance()
            .set(&DataKey::DefaultDisputeTimeout, &timeout_seconds);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_default_dispute_timeout(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::DefaultDisputeTimeout)
            .unwrap_or(DEFAULT_DISPUTE_TIMEOUT_SECONDS)
    }

    /// Set the global default winner for dispute timeouts. Admin only.
    pub fn set_default_dispute_winner(env: Env, admin: Address, winner: DisputeDefaultWinner) {
        Self::require_admin(&env, &admin);
        env.storage()
            .instance()
            .set(&DataKey::DefaultDisputeWinner, &winner);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_default_dispute_winner(env: Env) -> DisputeDefaultWinner {
        env.storage()
            .instance()
            .get(&DataKey::DefaultDisputeWinner)
            .unwrap_or(DisputeDefaultWinner::Buyer)
    }

    /// Admin updates the max top-up multiplier (e.g., 3 = max 3x original amount).
    pub fn update_max_topup_multiplier(env: Env, admin: Address, multiplier: u32) {
        Self::require_admin(&env, &admin);
        if multiplier == 0 {
            panic!("Multiplier must be positive");
        }

        env.storage()
            .instance()
            .set(&DataKey::MaxTopupMultiplier, &multiplier);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_max_topup_multiplier(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MaxTopupMultiplier)
            .unwrap_or(DEFAULT_MAX_TOPUP_MULTIPLIER)
    }

    /// Admin updates the partial release response deadline in seconds
    pub fn update_partial_release_response_deadline(env: Env, admin: Address, deadline: u64) {
        Self::require_admin(&env, &admin);
        if deadline == 0 {
            panic!("Deadline must be positive");
        }

        env.storage()
            .instance()
            .set(&DataKey::PartialReleaseResponseDeadline, &deadline);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_partial_release_response_deadline(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::PartialReleaseResponseDeadline)
            .unwrap_or(DEFAULT_PARTIAL_RELEASE_RESPONSE_DEADLINE)
    }

    /// Enforce dispute timeout: release funds to default winner if arbiter fails to resolve within deadline.
    /// Can be called by anyone after the deadline has passed.
    pub fn enforce_dispute_timeout(env: Env, escrow_id: u32) {
        Self::require_not_paused(&env);

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        // Check if there's a dispute record first
        let dispute: Option<Dispute> = env
            .storage()
            .persistent()
            .get(&DataKey::Dispute(escrow_id));

        let dispute = match dispute {
            Some(d) => d,
            None => panic!("Escrow is not disputed"),
        };

        if dispute.resolved {
            panic!("Dispute already resolved");
        }

        if escrow.status != EscrowStatus::Disputed
            && escrow.status != EscrowStatus::PartiallyDisputed
        {
            panic!("Escrow is not disputed");
        }

        // Get dispute deadline start timestamp
        let deadline_start: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::DisputeDeadlineStart(escrow_id))
            .expect("Dispute deadline start not found");

        // Determine effective timeout
        let default_timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::DefaultDisputeTimeout)
            .unwrap_or(DEFAULT_DISPUTE_TIMEOUT_SECONDS);
        let effective_timeout = dispute.timeout_seconds.unwrap_or(default_timeout);

        // Check if deadline has passed
        let now = env.ledger().timestamp();
        let elapsed = now.saturating_sub(deadline_start);
        if elapsed < effective_timeout {
            panic!("Dispute timeout deadline has not passed yet");
        }

        // Determine default winner
        let default_winner_u32 = escrow
            .extensions
            .dispute_default_winner
            .unwrap_or_else(|| {
                let stored_winner: DisputeDefaultWinner = env
                    .storage()
                    .instance()
                    .get(&DataKey::DefaultDisputeWinner)
                    .unwrap_or(DisputeDefaultWinner::Buyer);
                stored_winner as u32
            });

        let default_winner_enum = match default_winner_u32 {
            0 => DisputeDefaultWinner::Buyer,
            1 => DisputeDefaultWinner::Seller,
            _ => DisputeDefaultWinner::Buyer,
        };

        // Increment arbiter timeout counter
        let arbiter_timeout_count: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::ArbiterTimeoutCount(escrow.arbiter.clone()))
            .unwrap_or(0);
        env.storage().persistent().set(
            &DataKey::ArbiterTimeoutCount(escrow.arbiter.clone()),
            &(arbiter_timeout_count + 1),
        );
        env.storage().persistent().extend_ttl(
            &DataKey::ArbiterTimeoutCount(escrow.arbiter.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Release funds to default winner
        let client = token::Client::new(&env, &escrow.token);
        let release_to_seller = matches!(default_winner_enum, DisputeDefaultWinner::Seller);

        if release_to_seller {
            client.transfer(
                &env.current_contract_address(),
                &escrow.seller,
                &escrow.amount,
            );
            escrow.status = EscrowStatus::Released;
        } else {
            client.transfer(
                &env.current_contract_address(),
                &escrow.buyer,
                &escrow.amount,
            );
            escrow.status = EscrowStatus::Refunded;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Mark dispute as resolved
        let mut resolved_dispute = dispute;
        resolved_dispute.resolved = true;
        env.storage()
            .persistent()
            .set(&DataKey::Dispute(escrow_id), &resolved_dispute);

        events::emit_dispute_timed_out(
            &env,
            escrow_id,
            escrow.arbiter.clone(),
            default_winner_enum,
            elapsed,
        );
        events::emit_arbiter_timeout_penalty_applied(
            &env,
            escrow.arbiter,
            arbiter_timeout_count + 1,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get arbiter timeout count for reputation tracking.
    pub fn get_arbiter_timeout_count(env: Env, arbiter: Address) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::ArbiterTimeoutCount(arbiter))
            .unwrap_or(0)
    }

    /// Set the protocol-wide default arbiter fee (bps). Admin only.
    /// Fee cap is 1000 bps (10%).
    pub fn set_default_arbiter_fee_bps(env: Env, admin: Address, fee_bps: u32) {
        Self::require_admin(&env, &admin);
        if fee_bps > MAX_ARBITER_FEE_BPS {
            panic!("Arbiter fee exceeds maximum of 1000 bps");
        }

        env.storage()
            .instance()
            .set(&DataKey::DefaultArbiterFeeBps, &fee_bps);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_default_arbiter_fee_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::DefaultArbiterFeeBps)
            .unwrap_or(0)
    }

    /// Configure oracle address and max accepted price age (seconds). Admin only.
    pub fn set_oracle(env: Env, admin: Address, oracle: Address, max_oracle_age: u64) {
        Self::require_admin(&env, &admin);
        if max_oracle_age == 0 {
            panic!("max_oracle_age must be positive");
        }

        env.storage().instance().set(&DataKey::OracleAddress, &oracle);
        env.storage()
            .instance()
            .set(&DataKey::MaxOracleAge, &max_oracle_age);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_oracle_address(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::OracleAddress)
            .expect("Oracle not configured")
    }

    pub fn get_max_oracle_age(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MaxOracleAge)
            .unwrap_or(DEFAULT_MAX_ORACLE_AGE_SECONDS)
    }

    /// Configure insurance token and trigger window in days. Admin only.
    pub fn set_insurance_config(env: Env, admin: Address, token: Address, trigger_days: u64) {
        Self::require_admin(&env, &admin);
        if trigger_days == 0 {
            panic!("insurance_trigger_days must be positive");
        }

        // Token whitelist validation
        Self::require_token_allowed(&env, &token);

        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }

        env.storage().instance().set(&DataKey::InsuranceToken, &token);
        env.storage()
            .instance()
            .set(&DataKey::InsuranceTriggerDays, &trigger_days);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_insurance_pool(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::InsurancePool)
            .unwrap_or(0)
    }

    /// Open contribution flow for the insurance pool.
    pub fn contribute_to_insurance(env: Env, contributor: Address, amount: i128) {
        Self::require_not_paused(&env);
        contributor.require_auth();

        if amount <= 0 {
            panic!("Insurance contribution must be positive");
        }

        let token: Address = env
            .storage()
            .instance()
            .get(&DataKey::InsuranceToken)
            .expect("Insurance token not configured");

        let client = token::Client::new(&env, &token);
        client.transfer(&contributor, &env.current_contract_address(), &amount);

        let current_pool: i128 = env
            .storage()
            .instance()
            .get(&DataKey::InsurancePool)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::InsurancePool, &(current_pool + amount));
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin confirms or clears inactivity confirmation for a disputed escrow.
    pub fn confirm_insurance_inactivity(
        env: Env,
        admin: Address,
        escrow_id: u32,
        confirmed: bool,
    ) {
        Self::require_admin(&env, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::InsuranceAdminConfirmed(escrow_id), &confirmed);
        env.storage().persistent().extend_ttl(
            &DataKey::InsuranceAdminConfirmed(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    /// Claim insurance for unresolved disputes after admin-confirmed inactivity.
    /// The claim amount is capped at 50% of the disputed amount and pool balance.
    pub fn claim_insurance(env: Env, claimant: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        claimant.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if claimant != escrow.buyer && claimant != escrow.seller {
            panic!("Only buyer or seller can claim insurance");
        }

        if escrow.status != EscrowStatus::Disputed
            && escrow.status != EscrowStatus::PartiallyDisputed
        {
            panic!("Escrow is not disputed");
        }

        let dispute: Dispute = env
            .storage()
            .persistent()
            .get(&DataKey::Dispute(escrow_id))
            .expect("Dispute not found");
        if dispute.resolved {
            panic!("Dispute already resolved");
        }

        if env
            .storage()
            .persistent()
            .get::<DataKey, bool>(&DataKey::InsuranceClaimed(escrow_id))
            .unwrap_or(false)
        {
            panic!("Insurance already claimed");
        }

        let confirmed: bool = env
            .storage()
            .persistent()
            .get(&DataKey::InsuranceAdminConfirmed(escrow_id))
            .unwrap_or(false);
        if !confirmed {
            panic!("Admin confirmation required");
        }

        let trigger_days: u64 = env
            .storage()
            .instance()
            .get(&DataKey::InsuranceTriggerDays)
            .unwrap_or(DEFAULT_INSURANCE_TRIGGER_DAYS);
        let trigger_seconds = trigger_days.saturating_mul(24 * 60 * 60);
        let elapsed = env.ledger().timestamp().saturating_sub(dispute.created_at);
        if elapsed < trigger_seconds {
            panic!("Insurance trigger period not reached");
        }

        let insurance_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::InsuranceToken)
            .expect("Insurance token not configured");
        if insurance_token != escrow.token {
            panic!("Escrow token not covered by insurance pool");
        }

        let max_claim = escrow.amount / 2;
        let pool_balance: i128 = env
            .storage()
            .instance()
            .get(&DataKey::InsurancePool)
            .unwrap_or(0);
        let claim_amount = if pool_balance < max_claim {
            pool_balance
        } else {
            max_claim
        };

        if claim_amount <= 0 {
            panic!("Insurance pool has insufficient balance");
        }

        let token_client = token::Client::new(&env, &insurance_token);
        token_client.transfer(&env.current_contract_address(), &claimant, &claim_amount);

        env.storage()
            .instance()
            .set(&DataKey::InsurancePool, &(pool_balance - claim_amount));
        env.storage()
            .persistent()
            .set(&DataKey::InsuranceClaimed(escrow_id), &true);
        env.storage().persistent().extend_ttl(
            &DataKey::InsuranceClaimed(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_insurance_claimed(&env, escrow_id, claimant, claim_amount);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Set protocol fee in basis points and fee recipient. Admin only.
    /// Max fee is 200 bps (2%).
    pub fn update_protocol_fee(env: Env, admin: Address, fee_bps: u32, fee_recipient: Address) {
        Self::require_admin(&env, &admin);
        if fee_bps > MAX_PROTOCOL_FEE_BPS {
            panic!("Fee exceeds maximum of 200 bps");
        }
        env.storage()
            .instance()
            .set(&DataKey::ProtocolFeeBps, &fee_bps);
        env.storage()
            .instance()
            .set(&DataKey::FeeRecipient, &fee_recipient);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get current protocol fee bps and fee recipient.
    pub fn get_protocol_fee(env: Env) -> (u32, Option<Address>) {
        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0);
        let fee_recipient: Option<Address> = env.storage().instance().get(&DataKey::FeeRecipient);
        (fee_bps, fee_recipient)
    }

    /// Anyone may trigger release when the escrow's oracle condition is met.
    pub fn check_and_release_escrow(env: Env, escrow_id: u32) {
        Self::require_not_paused(&env);

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        let base = escrow
            .extensions
            .release_base
            .clone()
            .expect("No release condition set");
        let quote = escrow
            .extensions
            .release_quote
            .clone()
            .expect("No release condition set");
        let comparison = escrow
            .extensions
            .release_comparison
            .expect("No release condition set");
        let threshold_price = escrow
            .extensions
            .release_threshold_price
            .expect("No release condition set");

        let price_data = Self::get_oracle_price(&env, &base, &quote);
        let condition_met = match comparison {
            ORACLE_COMPARISON_LESS_OR_EQUAL => price_data.price <= threshold_price,
            ORACLE_COMPARISON_GREATER_OR_EQUAL => price_data.price >= threshold_price,
            _ => panic!("Invalid release comparison"),
        };

        if !condition_met {
            panic!("Release condition not met");
        }

        Self::transfer_to_sellers(&env, &escrow, escrow.amount, escrow_id);
        escrow.status = EscrowStatus::Released;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_oracle_release_triggered(
            &env,
            escrow_id,
            price_data.price,
            base,
            quote,
            comparison,
            threshold_price,
        );
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Auto-release expired escrow (past deadline, undisputed). Can be called by buyer.
    pub fn auto_release_expired(env: Env, escrow_id: u32) {
        Self::require_not_paused(&env);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if env.ledger().timestamp() <= escrow.deadline {
            panic!("Escrow has not expired yet");
        }

        Self::require_unlocked(&env, &escrow);

        escrow.buyer.require_auth();

        let client = token::Client::new(&env, &escrow.token);
        client.transfer(
            &env.current_contract_address(),
            &escrow.buyer,
            &escrow.amount,
        );

        escrow.status = EscrowStatus::Refunded;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_refunded(&env, escrow_id, escrow.buyer, escrow.amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Propose a new deadline for an active escrow.
    /// Only buyer or seller may propose and the proposal requires counterparty acceptance.
    pub fn propose_deadline_extension(
        env: Env,
        caller: Address,
        escrow_id: u32,
        new_deadline: u64,
    ) {
        caller.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can propose deadline extension");
        }

        if escrow.status == EscrowStatus::Disputed || Self::is_terminal_escrow_status(escrow.status)
        {
            panic!("Cannot extend deadline while escrow is disputed");
        }

        if new_deadline <= escrow.deadline {
            panic!("New deadline must be greater than current deadline");
        }

        let proposal = DeadlineProposal {
            proposer: caller.clone(),
            new_deadline,
            proposed_at: env.ledger().timestamp(),
        };

        env.storage()
            .persistent()
            .set(&DataKey::DeadlineProposal(escrow_id), &proposal);
        env.storage().persistent().extend_ttl(
            &DataKey::DeadlineProposal(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_deadline_extension_proposed(
            &env,
            escrow_id,
            caller,
            proposal.new_deadline,
            proposal.proposed_at,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Accept a pending deadline extension proposed by the counterparty.
    pub fn accept_deadline_extension(env: Env, caller: Address, escrow_id: u32) {
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can accept deadline extension");
        }

        if escrow.status == EscrowStatus::Disputed || Self::is_terminal_escrow_status(escrow.status)
        {
            panic!("Cannot extend deadline while escrow is disputed");
        }

        let proposal: DeadlineProposal = env
            .storage()
            .persistent()
            .get(&DataKey::DeadlineProposal(escrow_id))
            .expect("No deadline extension proposal found");

        if caller == proposal.proposer {
            panic!("Proposer cannot accept their own deadline extension");
        }

        let now = env.ledger().timestamp();
        if now > proposal.proposed_at + DEADLINE_EXTENSION_PROPOSAL_WINDOW {
            env.storage()
                .persistent()
                .remove(&DataKey::DeadlineProposal(escrow_id));
            panic!("Deadline extension proposal has expired");
        }

        if proposal.new_deadline <= escrow.deadline {
            env.storage()
                .persistent()
                .remove(&DataKey::DeadlineProposal(escrow_id));
            panic!("New deadline must be greater than current deadline");
        }

        let old_deadline = escrow.deadline;
        escrow.deadline = proposal.new_deadline;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage()
            .persistent()
            .remove(&DataKey::DeadlineProposal(escrow_id));

        events::emit_deadline_extended(&env, escrow_id, old_deadline, escrow.deadline);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get escrow details
    pub fn get_escrow(env: Env, escrow_id: u32) -> Escrow {
        env.storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found")
    }

    /// Get list of sellers for an escrow (#317)
    pub fn get_escrow_sellers(env: Env, escrow_id: u32) -> Vec<(Address, u32)> {
        let escrow: Escrow = env
    /// Buyer tops up an active or awaiting inspection escrow
    pub fn top_up_escrow(env: Env, buyer: Address, escrow_id: u32, additional_amount: i128) {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        escrow.sellers
    }

    /// Delegate seller's share to another address before release (#317)
    pub fn delegate_escrow_share(
        env: Env,
        seller: Address,
        escrow_id: u32,
        delegate: Address,

        if buyer != escrow.buyer {
            panic!("Only buyer can top up escrow");
        }

        // Check status is Active or AwaitingInspection
        if escrow.status != EscrowStatus::Active && escrow.status != EscrowStatus::AwaitingInspection {
            panic!("Escrow is not active or awaiting inspection");
        }

        if additional_amount <= 0 {
            panic!("Additional amount must be positive");
        }

        let max_topup_multiplier: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxTopupMultiplier)
            .unwrap_or(DEFAULT_MAX_TOPUP_MULTIPLIER);
        let max_total = escrow.original_amount * (max_topup_multiplier as i128);
        let new_total = escrow.amount + additional_amount;

        if new_total > max_total {
            panic!("Top-up limit exceeded");
        }

        // Transfer tokens
        let token_client = token::Client::new(&env, &escrow.token);
        token_client.transfer(&buyer, &env.current_contract_address(), &additional_amount);

        // Update escrow
        escrow.amount = new_total;
        let topup_entry = TopUpEntry {
            amount: additional_amount,
            timestamp: env.ledger().timestamp(),
            cumulative_total: new_total,
        };
        escrow.top_up_history.push_back(topup_entry);
        escrow.top_up_acknowledged = false;

        // Save and extend TTL
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Emit event
        events::emit_escrow_topped_up(&env, escrow_id, buyer, additional_amount, new_total);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Seller acknowledges the top-up
    pub fn acknowledge_topup(env: Env, seller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        seller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if seller != escrow.seller {
            panic!("Only seller can acknowledge top-up");
        }

        escrow.top_up_acknowledged = true;

        // Save and extend TTL
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Emit event
        events::emit_top_up_acknowledged(&env, escrow_id, seller, escrow.amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Seller requests partial release of escrowed funds
    pub fn request_partial_release(
        env: Env,
        seller: Address,
        escrow_id: u32,
        amount: i128,
        justification_hash: BytesN<32>,
    ) {
        Self::require_not_paused(&env);
        seller.require_auth();

        if seller == delegate {
            panic!("Delegate must be different from seller");
        }

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if seller != escrow.seller {
            panic!("Only seller can request partial release");
        }

        if escrow.status != EscrowStatus::Active {
            panic!("Partial release only allowed on active escrow");
        }

        if amount <= 0 {
            panic!("Partial release amount must be positive");
        }

        if amount > escrow.amount {
            panic!("Partial release amount cannot exceed escrow amount");
        }

        // Check if there's already a pending request
        if env.storage().persistent().has(&DataKey::PendingPartialRelease(escrow_id)) {
            panic!("Request already pending");
        }

        // Get next request ID
        let mut request_counter: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::PartialReleaseCounter(escrow_id))
            .unwrap_or(0);
        request_counter += 1;
        env.storage()
            .persistent()
            .set(&DataKey::PartialReleaseCounter(escrow_id), &request_counter);

        // Get response deadline
        let response_deadline_seconds: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PartialReleaseResponseDeadline)
            .unwrap_or(DEFAULT_PARTIAL_RELEASE_RESPONSE_DEADLINE);
        let now = env.ledger().timestamp();
        let response_deadline = now + response_deadline_seconds;

        // Create request
        let request = PartialReleaseRequest {
            request_id: request_counter,
            amount,
            justification_hash,
            created_at: now,
            response_deadline,
        };

        // Store request
        env.storage()
            .persistent()
            .set(&DataKey::PendingPartialRelease(escrow_id), &request);
        env.storage().persistent().extend_ttl(
            &DataKey::PendingPartialRelease(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().extend_ttl(
            &DataKey::PartialReleaseCounter(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Emit event
        events::emit_partial_release_requested(&env, escrow_id, request_counter, seller, amount);
    }

    /// Buyer approves partial release request
    pub fn approve_partial_release(env: Env, buyer: Address, escrow_id: u32, request_id: u64) {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if buyer != escrow.buyer {
            panic!("Only buyer can approve partial release");
        }

        // Get pending request
        let request: PartialReleaseRequest = env
            .storage()
            .persistent()
            .get(&DataKey::PendingPartialRelease(escrow_id))
            .expect("No pending partial release request");

        if request.request_id != request_id {
            panic!("Invalid request ID");
        }

        // Transfer funds
        Self::transfer_to_sellers(&env, &escrow, request.amount, escrow_id);

        // Reduce escrow amount
        escrow.amount -= request.amount;

        // Save escrow
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Remove pending request
        env.storage()
            .persistent()
            .remove(&DataKey::PendingPartialRelease(escrow_id));

        // Emit event
        events::emit_partial_release_approved(&env, escrow_id, request_id, request.amount);
    }

    /// Buyer rejects partial release request
    pub fn reject_partial_release(
        env: Env,
        buyer: Address,
        escrow_id: u32,
        request_id: u64,
        reason_hash: BytesN<32>,
    ) {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        // Verify seller is part of this escrow
        let mut is_seller = false;
        for (addr, _) in escrow.sellers.iter() {
            if addr == seller {
                is_seller = true;
                break;
            }
        }
        if !is_seller {
            panic!("Seller is not part of this escrow");
        }

        // Only allow delegation before release
        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Can only delegate before escrow is released");
        }

        env.storage()
            .persistent()
            .set(&DataKey2::SellerShareDelegate(escrow_id, seller.clone()), &delegate);
        env.storage().persistent().extend_ttl(
            &DataKey2::SellerShareDelegate(escrow_id, seller.clone()),
        if buyer != escrow.buyer {
            panic!("Only buyer can reject partial release");
        }

        // Get pending request
        let request: PartialReleaseRequest = env
            .storage()
            .persistent()
            .get(&DataKey::PendingPartialRelease(escrow_id))
            .expect("No pending partial release request");

        if request.request_id != request_id {
            panic!("Invalid request ID");
        }

        // Remove pending request
        env.storage()
            .persistent()
            .remove(&DataKey::PendingPartialRelease(escrow_id));

        // Emit event
        events::emit_partial_release_rejected(&env, escrow_id, request_id);
    }

    /// Seller escalates to dispute if buyer doesn't respond within deadline
    pub fn escalate_partial_release_to_dispute(env: Env, seller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        seller.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if seller != escrow.seller {
            panic!("Only seller can escalate partial release");
        }

        // Get pending request
        let request: PartialReleaseRequest = env
            .storage()
            .persistent()
            .get(&DataKey::PendingPartialRelease(escrow_id))
            .expect("No pending partial release request");

        // Check if deadline passed
        let now = env.ledger().timestamp();
        if now < request.response_deadline {
            panic!("Response deadline not yet passed");
        }

        // Remove pending request
        env.storage()
            .persistent()
            .remove(&DataKey::PendingPartialRelease(escrow_id));

        // Escalate to full dispute
        let mut escrow_mut = escrow;
        escrow_mut.status = EscrowStatus::Disputed;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow_mut);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_seller_share_delegated(&env, escrow_id, seller, delegate);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        // Create a dispute record (using existing Dispute struct)
        let dispute = Dispute {
            escrow_id,
            reason: String::from_str(&env, "Partial release escalation"),
            created_at: now,
            resolved: false,
            dispute_amount: escrow_mut.amount,
            timeout_seconds: None,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Dispute(escrow_id), &dispute);
        env.storage().persistent().extend_ttl(
            &DataKey::Dispute(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    /// Transfer buyer role in an active escrow to a new address.
    pub fn transfer_buyer_role(
        env: Env,
        current_buyer: Address,
        escrow_id: u32,
        new_buyer: Address,
    ) {
        Self::require_not_paused(&env);
        current_buyer.require_auth();

        if current_buyer == new_buyer {
            panic!("New buyer must be different from current buyer");
        }

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::Active {
            panic!("Buyer transfer only allowed for active escrows");
        }

        if escrow.buyer != current_buyer {
            panic!("Only current buyer can transfer buyer role");
        }

        let old_buyer = escrow.buyer.clone();
        escrow.buyer = new_buyer.clone();

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_buyer_role_transferred(&env, escrow_id, old_buyer, new_buyer);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Update metadata hash for an escrow. Requires auth from buyer or seller.
    pub fn update_metadata(
        env: Env,
        caller: Address,
        escrow_id: u32,
        new_hash: BytesN<32>,
    ) {
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can update metadata");
        }

        // #150: Track buyer activity
        if caller == escrow.buyer {
            Self::update_last_buyer_action(&env, &escrow);
        }

        escrow.metadata_hash = Some(new_hash.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);

        env.storage().persistent().set(
            &DataKey::EscrowMetadata(escrow_id),
            &(new_hash.clone(), env.ledger().timestamp()),
        );
        env.storage().persistent().extend_ttl(
            &DataKey::EscrowMetadata(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_metadata_updated(&env, escrow_id, new_hash, caller);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the latest metadata hash for an escrow.
    pub fn get_metadata_hash(env: Env, escrow_id: u32) -> Option<BytesN<32>> {
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        escrow.metadata_hash
    }

    /// Get dispute details
    pub fn get_dispute(env: Env, escrow_id: u32) -> Dispute {
        env.storage()
            .persistent()
            .get(&DataKey::Dispute(escrow_id))
            .expect("No dispute found for this escrow")
    }

    /// Get escrow counter
    pub fn get_escrow_counter(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::EscrowCounter)
            .unwrap_or(0)
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

    // ─── #215: Time-Locked Escrow Release ────────────────────────────────────

    pub fn create_timelocked_escrow(
        env: Env,
        buyer: Address,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        unlock_at: u64,
        beneficiary: Address,
        metadata_hash: Option<BytesN<32>>,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();
        if unlock_at <= env.ledger().timestamp() { panic!("unlock_at must be in the future"); }
        let request = EscrowCreateRequest {
            seller: beneficiary.clone(),
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers: Vec::new(&env),
            auto_renew: false,
            renewal_count: 0,
            buyer_inactivity_secs: 0,
            min_lock_until: Some(unlock_at),
            release_base: None,
            release_quote: None,
            release_comparison: None,
            release_threshold_price: None,
            arbiter_fee_bps: None,
            dispute_default_winner: None,
        };
        let escrow_id = Self::create_escrow_core(&env, &buyer, request);
        let lock_data = TimeLockData { unlock_at, beneficiary: beneficiary.clone(), claimed: false };
        env.storage().persistent().set(&DataKey::TimeLockData(escrow_id), &lock_data);
        env.storage().persistent().extend_ttl(&DataKey::TimeLockData(escrow_id), PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
        events::emit_timelocked_escrow_created(&env, escrow_id, unlock_at, beneficiary);
        escrow_id
    }

    pub fn claim_timelocked(env: Env, beneficiary: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        beneficiary.require_auth();
        let mut lock_data: TimeLockData = env.storage().persistent().get(&DataKey::TimeLockData(escrow_id)).expect("Not a time-locked escrow");
        if lock_data.claimed { panic!("Already claimed"); }
        if beneficiary != lock_data.beneficiary { panic!("Only beneficiary can claim"); }
        if env.ledger().timestamp() < lock_data.unlock_at { panic!("Unlock time has not passed"); }
        let escrow: Escrow = env.storage().persistent().get(&DataKey::Escrow(escrow_id)).expect("Escrow not found");
        if escrow.status != EscrowStatus::Active { panic!("Escrow not active"); }
        let client = token::Client::new(&env, &escrow.token);
        client.transfer(&env.current_contract_address(), &beneficiary, &escrow.amount);
        let mut e = escrow.clone();
        e.status = EscrowStatus::Released;
        env.storage().persistent().set(&DataKey::Escrow(escrow_id), &e);
        env.storage().persistent().extend_ttl(&DataKey::Escrow(escrow_id), PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
        lock_data.claimed = true;
        env.storage().persistent().set(&DataKey::TimeLockData(escrow_id), &lock_data);
        events::emit_timelocked_funds_claimed(&env, escrow_id, beneficiary, escrow.amount);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn cancel_timelocked(env: Env, buyer: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        buyer.require_auth();
        let lock_data: TimeLockData = env.storage().persistent().get(&DataKey::TimeLockData(escrow_id)).expect("Not a time-locked escrow");
        if lock_data.claimed { panic!("Already claimed"); }
        if env.ledger().timestamp() >= lock_data.unlock_at { panic!("Past unlock time; use claim_timelocked"); }
        let mut escrow: Escrow = env.storage().persistent().get(&DataKey::Escrow(escrow_id)).expect("Escrow not found");
        if escrow.buyer != buyer { panic!("Only buyer can cancel"); }
        if escrow.status == EscrowStatus::Disputed || escrow.status == EscrowStatus::PartiallyDisputed { panic!("Dispute active"); }
        if escrow.status != EscrowStatus::Active { panic!("Escrow not active"); }
        let client = token::Client::new(&env, &escrow.token);
        client.transfer(&env.current_contract_address(), &buyer, &escrow.amount);
        escrow.status = EscrowStatus::Refunded;
        env.storage().persistent().set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(&DataKey::Escrow(escrow_id), PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
        events::emit_timelocked_escrow_cancelled(&env, escrow_id, buyer);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_timelock_data(env: Env, escrow_id: u32) -> Option<TimeLockData> {
        env.storage().persistent().get(&DataKey::TimeLockData(escrow_id))
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
        Self::require_or_bootstrap_admin(&env, &admin);

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

    /// Add a token to the allowlist. Admin only.
    pub fn add_allowed_token(env: Env, admin: Address, token: Address) {
        Self::require_admin(&env, &admin);
        env.storage()
            .instance()
            .set(&DataKey::AllowedToken(token.clone()), &true);
        events::emit_token_allowlisted(&env, admin, token);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Remove a token from the allowlist. Admin only.
    pub fn remove_allowed_token(env: Env, admin: Address, token: Address) {
        Self::require_admin(&env, &admin);
        env.storage()
            .instance()
            .remove(&DataKey::AllowedToken(token.clone()));
        events::emit_token_removed_from_allowlist(&env, admin, token);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Create a reusable escrow template. Returns the template ID.
    pub fn create_escrow_template(
        env: Env,
        creator: Address,
        config: EscrowTemplateConfig,
    ) -> u32 {
        creator.require_auth();

        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(config.token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }
        if config.deadline_duration == 0 {
            panic!("deadline_duration must be positive");
        }

        let mut counter: u32 = env
            .storage()
            .instance()
            .get(&DataKey::TemplateCounter)
            .unwrap_or(0);
        let template_id = counter;
        counter += 1;
        env.storage()
            .instance()
            .set(&DataKey::TemplateCounter, &counter);

        let template = EscrowTemplate {
            id: template_id,
            creator: creator.clone(),
            config,
            active: true,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Template(template_id), &template);
        env.storage().persistent().extend_ttl(
            &DataKey::Template(template_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_template_created(&env, template_id, creator);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        template_id
    }

    /// Create an escrow from an existing template. Any caller may use any active template.
    pub fn create_escrow_from_template(
        env: Env,
        buyer: Address,
        seller: Address,
        template_id: u32,
        amount: i128,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let template: EscrowTemplate = env
            .storage()
            .persistent()
            .get(&DataKey::Template(template_id))
            .expect("Template not found");

        if !template.active {
            panic!("Template is deactivated");
        }
        if amount <= 0 {
            panic!("Escrow amount must be positive");
        }

        let deadline = env.ledger().timestamp() + template.config.deadline_duration;

        let client = token::Client::new(&env, &template.config.token);
        client.transfer(&buyer, &env.current_contract_address(), &amount);

        let escrow_id = Self::next_escrow_id(&env);
        let mut single_seller = Vec::new(&env);
        single_seller.push_back((seller.clone(), 10_000u32));
        let escrow = Escrow {
            id: escrow_id,
            buyer: buyer.clone(),
            seller: seller.clone(),
            arbiter: template.config.arbiter.clone(),
            amount,
            original_amount: amount,
            token: template.config.token.clone(),
            status: EscrowStatus::Active,
            created_at: env.ledger().timestamp(),
            deadline,
            metadata_hash: None,
            sellers: single_seller,
            extensions: EscrowExtensions {
                auto_renew: false,
                renewal_count: 0,
                renewals_remaining: 0,
                dispute_timeout_seconds: None,
                buyer_inactivity_secs: 0,
                min_lock_until: None,
                release_base: None,
                release_quote: None,
                release_comparison: None,
                release_threshold_price: None,
                arbiter_fee_bps: None,
                dispute_default_winner: None,
                required_collateral_bps: 0,
                collateral_forfeit_bps: 0,
                collateral_deposit_deadline: 0,
                collateral_amount: 0,
                delivery_proof_hash: None,
                inspector: None,
                auto_renew_config: None,
                renewals_completed: 0,
            },
            top_up_history: Vec::new(&env),
            top_up_acknowledged: true,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_created(
            &env,
            escrow_id,
            buyer,
            seller,
            template.config.arbiter,
            amount,
            template.config.token,
            deadline,
        );
        events::emit_escrow_created_from_template(&env, escrow_id, template_id);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        escrow_id
    }
    /// Add an arbiter to the pool. Admin only.
    pub fn add_arbiter(env: Env, admin: Address, arbiter: Address) {
        Self::require_admin(&env, &admin);
        let mut pool: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ArbiterPool)
            .unwrap_or(Vec::new(&env));
        for i in 0..pool.len() {
            if pool.get(i).unwrap() == arbiter {
                panic!("Arbiter already in pool");
            }
        }
        pool.push_back(arbiter.clone());
        env.storage()
            .instance()
            .set(&DataKey::ArbiterPool, &pool);
        events::emit_arbiter_pool_updated(&env, arbiter, true);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Remove an arbiter from the pool. Admin only.
    /// Active escrows with this arbiter are flagged via ArbiterNeedsReplacement.
    pub fn remove_arbiter(env: Env, admin: Address, arbiter: Address, escrow_ids: Vec<u32>) {
        Self::require_admin(&env, &admin);
        let pool: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ArbiterPool)
            .expect("Arbiter pool is empty");
        let mut found = false;
        let mut new_pool: Vec<Address> = Vec::new(&env);
        for i in 0..pool.len() {
            let a = pool.get(i).unwrap();
            if a == arbiter {
                found = true;
            } else {
                new_pool.push_back(a);
            }
        }
        if !found {
            panic!("Arbiter not in pool");
        }
        // Reset index if it would go out of bounds
        let next_idx: u32 = env
            .storage()
            .instance()
            .get(&DataKey::NextArbiterIndex)
            .unwrap_or(0);
        if new_pool.is_empty() || next_idx >= new_pool.len() {
            env.storage()
                .instance()
                .set(&DataKey::NextArbiterIndex, &0u32);
        }
        env.storage()
            .instance()
            .set(&DataKey::ArbiterPool, &new_pool);
        // Flag active escrows that used this arbiter
        for i in 0..escrow_ids.len() {
            let eid = escrow_ids.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(eid))
            {
                if escrow.arbiter == arbiter && Self::is_open_escrow_status(escrow.status) {
                    env.storage()
                        .persistent()
                        .set(&DataKey::ArbiterNeedsReplacement(eid), &true);
                }
            }
        }
        events::emit_arbiter_pool_updated(&env, arbiter, false);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Create an escrow with the next arbiter from the pool (round-robin).
    pub fn create_escrow_with_pool_arbiter(
        env: Env,
        buyer: Address,
        seller: Address,
        amount: i128,
        token: Address,
        deadline: u64,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        if amount <= 0 {
            panic!("Escrow amount must be positive");
        }
        if deadline <= env.ledger().timestamp() {
            panic!("Deadline must be in the future");
        }
        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }

        let pool: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ArbiterPool)
            .unwrap_or(Vec::new(&env));
        if pool.is_empty() {
            panic!("Arbiter pool is empty");
        }

        let idx: u32 = env
            .storage()
            .instance()
            .get(&DataKey::NextArbiterIndex)
            .unwrap_or(0);
        let arbiter = pool.get(idx % pool.len()).unwrap();
        let next_idx = (idx + 1) % pool.len();
        env.storage()
            .instance()
            .set(&DataKey::NextArbiterIndex, &next_idx);

        let client = token::Client::new(&env, &token);
        client.transfer(&buyer, &env.current_contract_address(), &amount);

        let escrow_id = Self::next_escrow_id(&env);
        let mut single_seller = Vec::new(&env);
        single_seller.push_back((seller.clone(), 10_000u32));
        let escrow = Escrow {
            id: escrow_id,
            buyer: buyer.clone(),
            seller: seller.clone(),
            arbiter: arbiter.clone(),
            amount,
            token: token.clone(),
            status: EscrowStatus::Active,
            created_at: env.ledger().timestamp(),
            deadline,
            metadata_hash: None,
            sellers: single_seller,
            extensions: EscrowExtensions {
                auto_renew: false,
                renewal_count: 0,
                renewals_remaining: 0,
                dispute_timeout_seconds: None,
                buyer_inactivity_secs: 0,
                min_lock_until: None,
                release_base: None,
                release_quote: None,
                release_comparison: None,
                release_threshold_price: None,
                arbiter_fee_bps: None,
                dispute_default_winner: None,
                required_collateral_bps: 0,
                collateral_forfeit_bps: 0,
                collateral_deposit_deadline: 0,
                collateral_amount: 0,
                delivery_proof_hash: None,
                inspector: None,
                auto_renew_config: None,
                renewals_completed: 0,
            },
            top_up_history: Vec::new(&env),
            top_up_acknowledged: true,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_created(
            &env, escrow_id, buyer, seller, arbiter.clone(), amount, token, deadline,
        );
        events::emit_arbiter_assigned(&env, escrow_id, arbiter);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        escrow_id
    }

    /// Update a template's config. Only the template creator can call this.
    pub fn update_escrow_template(
        env: Env,
        creator: Address,
        template_id: u32,
        new_config: EscrowTemplateConfig,
    ) {
        creator.require_auth();

        let mut template: EscrowTemplate = env
            .storage()
            .persistent()
            .get(&DataKey::Template(template_id))
            .expect("Template not found");

        if template.creator != creator {
            panic!("Only template creator can update");
        }
        if !template.active {
            panic!("Template is deactivated");
        }

        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(new_config.token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }
        if new_config.deadline_duration == 0 {
            panic!("deadline_duration must be positive");
        }

        template.config = new_config;
        env.storage()
            .persistent()
            .set(&DataKey::Template(template_id), &template);
        env.storage().persistent().extend_ttl(
            &DataKey::Template(template_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_template_updated(&env, template_id, creator);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Deactivate a template. Only the template creator can call this.
    pub fn deactivate_escrow_template(env: Env, creator: Address, template_id: u32) {
        creator.require_auth();

        let mut template: EscrowTemplate = env
            .storage()
            .persistent()
            .get(&DataKey::Template(template_id))
            .expect("Template not found");

        if template.creator != creator {
            panic!("Only template creator can deactivate");
        }
        if !template.active {
            panic!("Template already deactivated");
        }

        template.active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Template(template_id), &template);

        events::emit_escrow_template_deactivated(&env, template_id, creator);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get template details.
    pub fn get_escrow_template(env: Env, template_id: u32) -> EscrowTemplate {
        env.storage()
            .persistent()
            .get(&DataKey::Template(template_id))
            .expect("Template not found")
    }

    // -------------------------------------------------------------------------
    // #150: Seller-Favored Auto-Release on Prolonged Buyer Inactivity
    // -------------------------------------------------------------------------

    /// Claim an inactivity-based release to seller.
    /// Callable only by the escrow's seller after `buyer_inactivity_secs`
    /// have elapsed without any buyer action. Disabled when the field is 0.
    pub fn claim_inactivity_release(env: Env, seller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        seller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.extensions.buyer_inactivity_secs == 0 {
            panic!("Inactivity release is not enabled for this escrow");
        }

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if seller != escrow.seller {
            panic!("Only the escrow seller can claim inactivity release");
        }

        let last_action: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::LastBuyerAction(escrow_id))
            .unwrap_or(escrow.created_at);

        let now = env.ledger().timestamp();
        let inactivity_seconds = now.saturating_sub(last_action);

        if inactivity_seconds < escrow.extensions.buyer_inactivity_secs {
            panic!("Buyer inactivity window has not elapsed");
        }

        let client = token::Client::new(&env, &escrow.token);
        client.transfer(
            &env.current_contract_address(),
            &escrow.seller,
            &escrow.amount,
        );

        escrow.status = EscrowStatus::Released;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_inactivity_release_triggered(&env, escrow_id, seller, inactivity_seconds);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // -------------------------------------------------------------------------
    // #229: Escrow Mutual Cancellation with Configurable Penalty
    // -------------------------------------------------------------------------

    /// Admin sets the cancellation penalty in basis points (applied to initiator's share).
    pub fn set_cancellation_penalty_bps(env: Env, admin: Address, penalty_bps: u32) {
        Self::require_not_paused(&env);
        Self::require_or_bootstrap_admin(&env, &admin);
        if penalty_bps > 10_000 {
            panic!("Penalty cannot exceed 10000 bps");
        }
        env.storage()
            .instance()
            .set(&DataKey::CancellationPenaltyBps, &penalty_bps);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin sets the response window in seconds for cancellation requests.
    pub fn set_cancellation_response_window(env: Env, admin: Address, window_seconds: u64) {
        Self::require_not_paused(&env);
        Self::require_or_bootstrap_admin(&env, &admin);
        if window_seconds == 0 {
            panic!("Response window must be positive");
        }
        env.storage()
            .instance()
            .set(&DataKey::CancellationResponseWindow, &window_seconds);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Either party (buyer or seller) requests mutual cancellation of an active escrow.
    pub fn request_cancellation(env: Env, caller: Address, escrow_id: u32, reason_hash: BytesN<32>) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can request cancellation");
        }

        let window: u64 = env
            .storage()
            .instance()
            .get(&DataKey::CancellationResponseWindow)
            .unwrap_or(7 * 24 * 60 * 60); // default 7 days

        let now = env.ledger().timestamp();
        let expires_at = now + window;

        let request = CancellationRequest {
            initiator: caller.clone(),
            reason_hash,
            requested_at: now,
            expires_at,
        };

        env.storage()
            .persistent()
            .set(&DataKey::CancellationRequest(escrow_id), &request);
        env.storage().persistent().extend_ttl(
            &DataKey::CancellationRequest(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        escrow.status = EscrowStatus::CancellationPending;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_cancellation_requested(&env, escrow_id, caller, expires_at);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// The counterparty accepts the cancellation request.
    /// Full escrow amount minus penalty (applied to initiator) is returned to buyer.
    /// Penalty goes to fee collector.
    pub fn accept_cancellation(env: Env, caller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::CancellationPending {
            panic!("No pending cancellation for this escrow");
        }

        let request: CancellationRequest = env
            .storage()
            .persistent()
            .get(&DataKey::CancellationRequest(escrow_id))
            .expect("Cancellation request not found");

        let now = env.ledger().timestamp();

        // Check if request has expired — auto-restore Active
        if now > request.expires_at {
            escrow.status = EscrowStatus::Active;
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(escrow_id), &escrow);
            env.storage().persistent().extend_ttl(
                &DataKey::Escrow(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
            env.storage()
                .persistent()
                .remove(&DataKey::CancellationRequest(escrow_id));
            events::emit_cancellation_expired(&env, escrow_id);
            return;
        }

        // Caller must be the counterparty (not the initiator)
        if caller == request.initiator {
            panic!("Initiator cannot accept their own cancellation request");
        }
        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can accept cancellation");
        }

        let penalty_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CancellationPenaltyBps)
            .unwrap_or(0);

        let penalty_amount = if penalty_bps > 0 {
            (escrow.amount as u128 * penalty_bps as u128 / 10_000) as i128
        } else {
            0
        };

        let return_amount = escrow.amount - penalty_amount;

        let token_client = token::Client::new(&env, &escrow.token);

        // Return funds to buyer
        if return_amount > 0 {
            token_client.transfer(
                &env.current_contract_address(),
                &escrow.buyer,
                &return_amount,
            );
        }

        // Send penalty to fee collector if configured
        if penalty_amount > 0 {
            if let Some(fee_recipient) = env
                .storage()
                .instance()
                .get::<DataKey, Address>(&DataKey::FeeRecipient)
            {
                token_client.transfer(
                    &env.current_contract_address(),
                    &fee_recipient,
                    &penalty_amount,
                );
            } else {
                // No fee recipient — return penalty to buyer too
                token_client.transfer(
                    &env.current_contract_address(),
                    &escrow.buyer,
                    &penalty_amount,
                );
            }
        }

        escrow.status = EscrowStatus::Refunded;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage()
            .persistent()
            .remove(&DataKey::CancellationRequest(escrow_id));

        events::emit_cancellation_accepted(&env, escrow_id, escrow.buyer, return_amount, penalty_amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// The counterparty rejects the cancellation request; escrow resumes Active state.
    pub fn reject_cancellation(env: Env, caller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::CancellationPending {
            panic!("No pending cancellation for this escrow");
        }

        let request: CancellationRequest = env
            .storage()
            .persistent()
            .get(&DataKey::CancellationRequest(escrow_id))
            .expect("Cancellation request not found");

        if caller == request.initiator {
            panic!("Initiator cannot reject their own cancellation request");
        }
        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can reject cancellation");
        }

        escrow.status = EscrowStatus::Active;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage()
            .persistent()
            .remove(&DataKey::CancellationRequest(escrow_id));

        events::emit_cancellation_rejected(&env, escrow_id, caller);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Expire a stale cancellation request and restore Active state.
    /// Callable by anyone after the response window has elapsed.
    pub fn expire_cancellation(env: Env, escrow_id: u32) {
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::CancellationPending {
            panic!("No pending cancellation for this escrow");
        }

        let request: CancellationRequest = env
            .storage()
            .persistent()
            .get(&DataKey::CancellationRequest(escrow_id))
            .expect("Cancellation request not found");

        if env.ledger().timestamp() <= request.expires_at {
            panic!("Response window has not elapsed");
        }

        escrow.status = EscrowStatus::Active;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage()
            .persistent()
            .remove(&DataKey::CancellationRequest(escrow_id));

        events::emit_cancellation_expired(&env, escrow_id);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
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

    fn require_or_bootstrap_admin(env: &Env, admin: &Address) {
        admin.require_auth();
        if let Some(stored_admin) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::Admin)
        {
            if stored_admin != *admin {
                panic!("Only admin can pause contract");
            }
        } else {
            env.storage().instance().set(&DataKey::Admin, admin);
        }
    }

    fn require_admin(env: &Env, admin: &Address) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if stored_admin != *admin {
            panic!("Only admin can resume contract");
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

    fn require_unlocked(env: &Env, escrow: &Escrow) {
        if let Some(lock_until) = escrow.extensions.min_lock_until {
            if env.ledger().timestamp() < lock_until {
                panic!("EscrowStillLocked");
            }
        }
    }

    fn transfer_to_sellers(env: &Env, escrow: &Escrow, total: i128, escrow_id: u32) {
        let client = token::Client::new(env, &escrow.token);
        if escrow.sellers.len() <= 1 {
            client.transfer(&env.current_contract_address(), &escrow.seller, &total);
            events::emit_escrow_released(env, escrow_id, escrow.seller.clone(), total);
            return;
        }

        // Multi-seller distribution (#317)
        let mut distributions: Vec<(Address, i128)> = Vec::new(env);
        let mut distributed: i128 = 0;
        
        for i in 1..escrow.sellers.len() {
            let (addr, bps) = escrow.sellers.get(i).unwrap();
            let share = (total * bps as i128) / 10_000;
            if share > 0 {
                // Check for delegation (#317)
                let recipient = env
                    .storage()
                    .persistent()
                    .get::<_, Address>(&DataKey2::SellerShareDelegate(escrow_id, addr.clone()))
                    .unwrap_or(addr.clone());
                
                client.transfer(&env.current_contract_address(), &recipient, &share);
                distributions.push_back((recipient, share));
            }
            distributed += share;
        }

        let first_share = total - distributed;
        if first_share > 0 {
            let (first_addr, _) = escrow.sellers.get(0).unwrap();
            // Check for delegation (#317)
            let recipient = env
                .storage()
                .persistent()
                .get::<_, Address>(&DataKey2::SellerShareDelegate(escrow_id, first_addr.clone()))
                .unwrap_or(first_addr.clone());
            
            client.transfer(&env.current_contract_address(), &recipient, &first_share);
            distributions.push_back((recipient, first_share));
        }
        
        events::emit_multi_seller_escrow_released(env, escrow_id, distributions);
    }

    fn get_oracle_price(env: &Env, base: &Address, quote: &Address) -> PriceData {
        let oracle_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::OracleAddress)
            .expect("Oracle not configured");
        let max_oracle_age: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MaxOracleAge)
            .unwrap_or(DEFAULT_MAX_ORACLE_AGE_SECONDS);

        let oracle_client = oracle::OracleClient::new(env, &oracle_addr);
        let price_data = oracle_client
            .lastprice(base, quote)
            .expect("Oracle price unavailable");

        let age = env.ledger().timestamp().saturating_sub(price_data.timestamp);
        if age > max_oracle_age {
            panic!("Oracle price is stale");
        }
        if price_data.price <= 0 {
            panic!("Invalid oracle price");
        }

        price_data
    }

    fn effective_arbiter_fee_bps(env: &Env, escrow: &Escrow) -> u32 {
        escrow.extensions.arbiter_fee_bps.unwrap_or(
            env.storage()
                .instance()
                .get(&DataKey::DefaultArbiterFeeBps)
                .unwrap_or(0),
        )
    }

    fn is_open_escrow_status(status: EscrowStatus) -> bool {
        matches!(
            status,
            EscrowStatus::Active | EscrowStatus::PartiallyReleased | EscrowStatus::InspectionPassed
        )
    }

    fn is_terminal_escrow_status(status: EscrowStatus) -> bool {
        matches!(
            status,
            EscrowStatus::Released | EscrowStatus::Resolved | EscrowStatus::Refunded
        )
    }

    fn next_escrow_id(env: &Env) -> u32 {
        let mut counter: u32 = env
            .storage()
            .instance()
            .get(&DataKey::EscrowCounter)
            .unwrap_or(0);
        let id = counter;
        counter += 1;
        env.storage()
            .instance()
            .set(&DataKey::EscrowCounter, &counter);
        id
    }

    fn try_auto_renew(env: &Env, old_escrow_id: u32, source: &Escrow) {
        // ── New AutoRenewConfig path ──────────────────────────────────────────
        if let Some(ref cfg) = source.extensions.auto_renew_config {
            // Check if buyer has cancelled renewals for this escrow chain
            let cancelled: bool = env
                .storage()
                .persistent()
                .get(&DataKey::AutoRenewalCancelled(old_escrow_id))
                .unwrap_or(false);
            if cancelled {
                return;
            }

            let renewals_completed = source.extensions.renewals_completed;

            // Enforce max_renewals cap
            if renewals_completed >= cfg.max_renewals {
                return;
            }

            let renewal_index = renewals_completed + 1;

            // Attempt to pull funds from buyer's pre-approved allowance
            let token_client = token::Client::new(env, &source.token);
            let transfer_result = token_client.try_transfer_from(
                &env.current_contract_address(),
                &source.buyer,
                &env.current_contract_address(),
                &source.amount,
            );

            if transfer_result.is_err() {
                // Insufficient allowance — emit RenewalFailed and return gracefully
                events::emit_renewal_failed(
                    env,
                    old_escrow_id,
                    renewal_index,
                    soroban_sdk::String::from_str(env, "InsufficientAllowance"),
                );
                return;
            }

            // Compute new deadline using renewal_interval_ledgers converted to seconds
            // (Soroban ledger ≈ 5 s; we store interval in ledgers but deadline is unix timestamp)
            let interval_secs = (cfg.renewal_interval_ledgers as u64) * 5;
            let now = env.ledger().timestamp();
            let new_deadline = now + interval_secs;

            let new_escrow_id = Self::next_escrow_id(env);

            let renewed = Escrow {
                id: new_escrow_id,
                buyer: source.buyer.clone(),
                seller: source.seller.clone(),
                arbiter: source.arbiter.clone(),
                amount: source.amount,
                token: source.token.clone(),
                status: EscrowStatus::Active,
                created_at: now,
                deadline: new_deadline,
                metadata_hash: source.metadata_hash.clone(),
                sellers: source.sellers.clone(),
                extensions: EscrowExtensions {
                    auto_renew: source.extensions.auto_renew,
                    renewal_count: source.extensions.renewal_count,
                    renewals_remaining: source.extensions.renewals_remaining.saturating_sub(1),
                    dispute_timeout_seconds: source.extensions.dispute_timeout_seconds,
                    buyer_inactivity_secs: source.extensions.buyer_inactivity_secs,
                    min_lock_until: source.extensions.min_lock_until,
                    release_base: source.extensions.release_base.clone(),
                    release_quote: source.extensions.release_quote.clone(),
                    release_comparison: source.extensions.release_comparison,
                    release_threshold_price: source.extensions.release_threshold_price,
                    arbiter_fee_bps: source.extensions.arbiter_fee_bps,
                    dispute_default_winner: source.extensions.dispute_default_winner,
                    required_collateral_bps: source.extensions.required_collateral_bps,
                    collateral_forfeit_bps: source.extensions.collateral_forfeit_bps,
                    collateral_deposit_deadline: 0,
                    collateral_amount: 0,
                    delivery_proof_hash: source.extensions.delivery_proof_hash.clone(),
                    inspector: source.extensions.inspector.clone(),
                    auto_renew_config: Some(cfg.clone()),
                    renewals_completed: renewal_index,
                },
            };

            env.storage()
                .persistent()
                .set(&DataKey::Escrow(new_escrow_id), &renewed);
            env.storage().persistent().extend_ttl(
                &DataKey::Escrow(new_escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );

            // #150: Initialize LastBuyerAction for renewed escrow
            if source.extensions.buyer_inactivity_secs > 0 {
                env.storage()
                    .persistent()
                    .set(&DataKey::LastBuyerAction(new_escrow_id), &now);
                env.storage().persistent().extend_ttl(
                    &DataKey::LastBuyerAction(new_escrow_id),
                    PERSISTENT_LIFETIME_THRESHOLD,
                    PERSISTENT_BUMP_AMOUNT,
                );
            }

            // Append new_escrow_id to the renewal history of the original escrow
            // The "original" escrow is tracked by walking back: we store history keyed
            // by old_escrow_id so callers can call get_renewal_history(original_id).
            let history_key = DataKey::RenewalHistory(old_escrow_id);
            let mut history: Vec<u32> = env
                .storage()
                .persistent()
                .get(&history_key)
                .unwrap_or(Vec::new(env));
            history.push_back(new_escrow_id);
            env.storage().persistent().set(&history_key, &history);
            env.storage().persistent().extend_ttl(
                &history_key,
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );

            events::emit_escrow_auto_renewed_v2(env, old_escrow_id, new_escrow_id, renewal_index);
            return;
        }

        // ── Legacy auto_renew path (backward-compatible) ─────────────────────
        if !source.extensions.auto_renew {
            return;
        }

        if source.extensions.renewal_count != 0 && source.extensions.renewals_remaining == 0 {
            return;
        }

        let allowance: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::RenewalAllowance(old_escrow_id))
            .unwrap_or(0);
        if allowance == 0 {
            panic!("Insufficient renewal allowance");
        }

        let duration = source.deadline.saturating_sub(source.created_at);
        if duration == 0 {
            panic!("Escrow renewal duration must be positive");
        }

        let client = token::Client::new(env, &source.token);
        client.transfer_from(
            &env.current_contract_address(),
            &source.buyer,
            &env.current_contract_address(),
            &source.amount,
        );

        let new_escrow_id = Self::next_escrow_id(env);
        let now = env.ledger().timestamp();
        let renewals_remaining = if source.extensions.renewal_count == 0 {
            0
        } else {
            source.extensions.renewals_remaining - 1
        };

        let renewed = Escrow {
            id: new_escrow_id,
            buyer: source.buyer.clone(),
            seller: source.seller.clone(),
            arbiter: source.arbiter.clone(),
            amount: source.amount,
            token: source.token.clone(),
            status: EscrowStatus::Active,
            created_at: now,
            deadline: now + duration,
            metadata_hash: source.metadata_hash.clone(),
            sellers: source.sellers.clone(),
            extensions: EscrowExtensions {
                auto_renew: source.extensions.auto_renew,
                renewal_count: source.extensions.renewal_count,
                renewals_remaining,
                dispute_timeout_seconds: source.extensions.dispute_timeout_seconds,
                buyer_inactivity_secs: source.extensions.buyer_inactivity_secs,
                min_lock_until: source.extensions.min_lock_until,
                release_base: source.extensions.release_base.clone(),
                release_quote: source.extensions.release_quote.clone(),
                release_comparison: source.extensions.release_comparison,
                release_threshold_price: source.extensions.release_threshold_price,
                arbiter_fee_bps: source.extensions.arbiter_fee_bps,
                dispute_default_winner: source.extensions.dispute_default_winner,
                required_collateral_bps: source.extensions.required_collateral_bps,
                collateral_forfeit_bps: source.extensions.collateral_forfeit_bps,
                collateral_deposit_deadline: 0,
                collateral_amount: 0,
                delivery_proof_hash: source.extensions.delivery_proof_hash.clone(),
                inspector: source.extensions.inspector.clone(),
                auto_renew_config: None,
                renewals_completed: 0,
            },
        };

        // #150: Initialize LastBuyerAction for renewed escrow
        if source.extensions.buyer_inactivity_secs > 0 {
            env.storage()
                .persistent()
                .set(&DataKey::LastBuyerAction(new_escrow_id), &now);
            env.storage().persistent().extend_ttl(
                &DataKey::LastBuyerAction(new_escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(new_escrow_id), &renewed);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(new_escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        let remaining_allowance = allowance - 1;
        env.storage()
            .persistent()
            .set(&DataKey::RenewalAllowance(new_escrow_id), &remaining_allowance);
        env.storage().persistent().extend_ttl(
            &DataKey::RenewalAllowance(new_escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_auto_renewed(env, old_escrow_id, new_escrow_id, renewals_remaining);
    }

    /// Update the LastBuyerAction timestamp for inactivity tracking (#150).
    /// Only records when inactivity release is enabled for the escrow.
    fn update_last_buyer_action(env: &Env, escrow: &Escrow) {
        if escrow.extensions.buyer_inactivity_secs == 0 {
            return;
        }
        let now = env.ledger().timestamp();
        env.storage()
            .persistent()
            .set(&DataKey::LastBuyerAction(escrow.id), &now);
        env.storage().persistent().extend_ttl(
            &DataKey::LastBuyerAction(escrow.id),
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

    // ─── #225: Escrow Buyer Deposit Top-Up ───────────────────────────────────

    /// Admin sets the maximum cumulative top-up as basis points of the original amount.
    /// Default: 5000 bps = 50%.
    pub fn set_max_top_up_bps(env: Env, admin: Address, max_top_up_bps: u32) {
        Self::require_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can set max top-up bps");
        }
        env.storage().instance().set(&DataKey::MaxTopUpBps, &max_top_up_bps);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Buyer tops up an active or pending-release escrow with additional funds.
    /// Rejected if escrow is Disputed, Released, or Cancelled.
    /// Total cumulative top-ups are capped at `max_top_up_bps` of the original amount.
    pub fn top_up_escrow(env: Env, buyer: Address, escrow_id: u32, amount: i128) {
        Self::require_not_paused(&env);
        buyer.require_auth();

        if amount <= 0 {
            panic!("Top-up amount must be positive");
        }

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.buyer != buyer {
            panic!("Only the buyer can top up this escrow");
        }

        // Only Active or PartiallyReleased states are allowed
        match escrow.status {
            EscrowStatus::Active | EscrowStatus::PartiallyReleased => {}
            _ => panic!("Escrow is not in a top-up eligible state"),
        }

        // Enforce max_top_up_bps cap
        let max_top_up_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxTopUpBps)
            .unwrap_or(5_000); // default 50%
        let original_amount = escrow.amount;
        let already_topped_up: i128 = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowToppedUpAmount(escrow_id))
            .unwrap_or(0);
        let max_top_up = (original_amount * max_top_up_bps as i128) / 10_000;
        if already_topped_up + amount > max_top_up {
            panic!("Top-up would exceed maximum allowed top-up amount");
        }

        // Transfer funds from buyer to contract
        let token_client = token::Client::new(&env, &escrow.token);
        token_client.transfer(&buyer, &env.current_contract_address(), &amount);

        // Update escrow amount and cumulative top-up tracker
        escrow.amount += amount;
        let new_total = escrow.amount;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        let new_topped_up = already_topped_up + amount;
        env.storage()
            .persistent()
            .set(&DataKey::EscrowToppedUpAmount(escrow_id), &new_topped_up);
        env.storage().persistent().extend_ttl(
            &DataKey::EscrowToppedUpAmount(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_topped_up(&env, escrow_id, amount, new_total, buyer);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // ─── #219: Multi-Party Split Release ─────────────────────────────────────

    /// Create an escrow with explicit multi-party seller splits.
    /// `sellers` is a list of (address, bps) where bps must sum to 10,000.
    /// On release, each seller receives their proportional share of the net amount.
    /// Emits `MultiSellerEscrowCreated` in addition to the standard escrow events.
    pub fn create_multi_seller_escrow(
        env: Env,
        buyer: Address,
        sellers: Vec<(Address, u32)>,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        metadata_hash: Option<BytesN<32>>,
        required_collateral_bps: u32,
        collateral_forfeit_bps: u32,
        collateral_deposit_window: u64,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();
        if collateral_forfeit_bps > 10_000 {
            panic!("collateral_forfeit_bps cannot exceed 10000");
        }
        if sellers.is_empty() {
            panic!("At least one seller required");
        }
        let primary_seller = sellers.get(0).unwrap().0;
        let escrow_id = Self::create_escrow_core(
            &env,
            &buyer,
            EscrowCreateRequest {
                seller: primary_seller,
                arbiter,
                amount,
                token,
                deadline,
                metadata_hash,
                sellers,
                auto_renew: false,
                renewal_count: 0,
                buyer_inactivity_secs: 0,
                min_lock_until: None,
                release_base: None,
                release_quote: None,
                release_comparison: None,
                release_threshold_price: None,
                arbiter_fee_bps: None,
                dispute_default_winner: None,
            },
        );
        if required_collateral_bps > 0 {
            let mut escrow: Escrow = env
                .storage()
                .persistent()
                .get(&DataKey::Escrow(escrow_id))
                .expect("Escrow not found");
            let collateral_amount = (amount as u128 * required_collateral_bps as u128 / 10_000) as i128;
            let deposit_deadline = env.ledger().timestamp() + collateral_deposit_window;
            escrow.status = EscrowStatus::AwaitingCollateral;
            escrow.extensions.required_collateral_bps = required_collateral_bps;
            escrow.extensions.collateral_forfeit_bps = collateral_forfeit_bps;
            escrow.extensions.collateral_deposit_deadline = deposit_deadline;
            escrow.extensions.collateral_amount = collateral_amount;
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(escrow_id), &escrow);
            env.storage().persistent().extend_ttl(
                &DataKey::Escrow(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        escrow_id
    }

    /// Seller deposits the required collateral; escrow transitions from AwaitingCollateral to Active.
    pub fn deposit_collateral(env: Env, seller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        seller.require_auth();
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        if escrow.seller != seller {
            panic!("Only seller can deposit collateral");
        }
        if escrow.status != EscrowStatus::AwaitingCollateral {
            panic!("Escrow is not awaiting collateral");
        }
        let now = env.ledger().timestamp();
        if now > escrow.extensions.collateral_deposit_deadline {
            panic!("Collateral deposit window has expired");
        }
        let collateral = escrow.extensions.collateral_amount;
        let client = token::Client::new(&env, &escrow.token);
        client.transfer(&seller, &env.current_contract_address(), &collateral);
        env.storage()
            .persistent()
            .set(&DataKey::SellerCollateral(escrow_id), &collateral);
        env.storage().persistent().extend_ttl(
            &DataKey::SellerCollateral(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        escrow.status = EscrowStatus::Active;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_collateral_deposited(&env, escrow_id, seller, collateral);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // ─── #146: Post-Resolution Rating System ─────────────────────────────────

    /// Submit a 1-5 star rating for the counterparty after escrow completion.
    /// Callable by buyer (rating seller) or seller (rating buyer).
    /// Only allowed once per escrow per rater; escrow must be Released or Resolved.
    pub fn submit_rating(
        env: Env,
        rater: Address,
        escrow_id: u32,
        rating: u32,
        comment_hash: Option<BytesN<32>>,
    ) {
        rater.require_auth();
        if rating < 1 || rating > 5 {
            panic!("Rating must be between 1 and 5");
        }
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        if escrow.status != EscrowStatus::Released && escrow.status != EscrowStatus::Resolved {
            panic!("Rating only allowed after escrow is Released or Resolved");
        }
        let ratee = if rater == escrow.buyer {
            escrow.seller.clone()
        } else if rater == escrow.seller {
            escrow.buyer.clone()
        } else {
            panic!("Only buyer or seller can submit a rating");
        };
        let rating_key = DataKey::RatingSubmitted(escrow_id, rater.clone());
        if env.storage().persistent().has(&rating_key) {
            panic!("Rating already submitted for this escrow");
        }
        env.storage().persistent().set(&rating_key, &true);
        env.storage().persistent().extend_ttl(
            &rating_key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        let score_key = DataKey::RatingScore(ratee.clone());
        let (total_score, count): (u64, u32) = env
            .storage()
            .persistent()
            .get(&score_key)
            .unwrap_or((0u64, 0u32));
        let new_total = total_score + rating as u64;
        let new_count = count + 1;
        env.storage()
            .persistent()
            .set(&score_key, &(new_total, new_count));
        env.storage().persistent().extend_ttl(
            &score_key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_rating_submitted(&env, escrow_id, rater, ratee, rating, comment_hash);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Seller submits a raw proof. If sha256(proof) matches the stored delivery_proof_hash,
    /// the escrow is automatically released to the seller.
    pub fn submit_delivery_proof(env: Env, seller: Address, escrow_id: u32, proof: BytesN<32>) {
        Self::require_not_paused(&env);
        seller.require_auth();
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        if escrow.seller != seller {
            panic!("Only seller can submit delivery proof");
        }
        if escrow.status == EscrowStatus::Disputed {
            panic!("Proof submission locked: escrow is under dispute");
        }
        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }
        let expected_hash = escrow
            .extensions
            .delivery_proof_hash
            .clone()
            .expect("No delivery proof hash set on this escrow");
        let proof_bytes = soroban_sdk::Bytes::from(proof.clone());
        let computed: BytesN<32> = env.crypto().sha256(&proof_bytes).into();
        if computed != expected_hash {
            panic!("InvalidDeliveryProof");
        }
        let total = escrow.amount;
        Self::transfer_to_sellers(&env, &escrow, total, escrow_id);
        escrow.status = EscrowStatus::Released;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_delivery_proof_submitted(&env, escrow_id, seller, computed, true);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Buyer explicitly approves the seller transfer, finalising it immediately.
    pub fn approve_seller_transfer(env: Env, buyer: Address, escrow_id: u32) {
        buyer.require_auth();
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        if escrow.buyer != buyer {
            panic!("Only the buyer can approve");
        }
        if escrow.status != EscrowStatus::AwaitingBuyerVetoDecision {
            panic!("No pending seller transfer");
        }
        let proposal: SellerTransferProposal = env
            .storage()
            .persistent()
            .get(&DataKey::SellerTransferProposal(escrow_id))
            .expect("Proposal not found");
        escrow.seller = proposal.new_seller.clone();
        escrow.status = EscrowStatus::Active;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_seller_transfer_approved(&env, escrow_id, proposal.new_seller);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Anyone can call this after the veto window expires to finalise the transfer.
    pub fn expire_seller_transfer_veto(env: Env, escrow_id: u32) {
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        if escrow.status != EscrowStatus::AwaitingBuyerVetoDecision {
            panic!("No pending seller transfer");
        }
        let proposal: SellerTransferProposal = env
            .storage()
            .persistent()
            .get(&DataKey::SellerTransferProposal(escrow_id))
            .expect("Proposal not found");
        if env.ledger().sequence() <= proposal.veto_deadline {
            panic!("Veto window has not expired yet");
        }
        escrow.seller = proposal.new_seller.clone();
        escrow.status = EscrowStatus::Active;
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_seller_transfer_expired_approved(&env, escrow_id, proposal.new_seller);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns (avg_score_x100, total_ratings) for an address.
    /// avg_score_x100 = (total_score * 100) / count, or 0 if no ratings.
    pub fn get_reputation(env: Env, address: Address) -> (u32, u32) {
        let score_key = DataKey::RatingScore(address);
        let (total_score, count): (u64, u32) = env
            .storage()
            .persistent()
            .get(&score_key)
            .unwrap_or((0u64, 0u32));
        if count == 0 {
            return (0, 0);
        }
        let avg_x100 = ((total_score * 100) / count as u64) as u32;
        (avg_x100, count)
    }

    // ── #332: Milestone BPS Progressive Release ───────────────────────────────

    /// Create an escrow divided into named milestones with proportional BPS payouts.
    /// `milestones[i].release_bps` values must sum to exactly 10 000.
    /// The full `amount` is transferred from buyer into the contract up front.
    pub fn create_bps_milestone_escrow(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        milestones: Vec<MilestoneInput>,
    ) -> u32 {
        buyer.require_auth();

        if milestones.is_empty() {
            panic!("At least one milestone required");
        }
        if milestones.len() > MAX_MILESTONES {
            panic!("Too many milestones");
        }

        // Validate release_bps sum == 10 000
        let mut total_bps: u32 = 0;
        for i in 0..milestones.len() {
            let m = milestones.get(i).unwrap();
            if m.release_bps == 0 {
                panic!("release_bps must be positive for each milestone");
            }
            total_bps = total_bps.saturating_add(m.release_bps);
        }
        if total_bps != 10_000 {
            panic!("Milestone release_bps must sum to 10 000");
        }
        if amount <= 0 {
            panic!("Escrow amount must be positive");
        }

        let request = EscrowCreateRequest {
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash: None,
            sellers: Vec::new(&env),
            auto_renew: false,
            renewal_count: 0,
            buyer_inactivity_secs: 0,
            min_lock_until: None,
            release_base: None,
            release_quote: None,
            release_comparison: None,
            release_threshold_price: None,
            arbiter_fee_bps: None,
            dispute_default_winner: None,
        };
        let escrow_id = Self::create_escrow_core(&env, &buyer, request);

        // Convert MilestoneInput → MilestoneState (all Pending, no hashes yet)
        let mut states: Vec<MilestoneState> = Vec::new(&env);
        for i in 0..milestones.len() {
            let m = milestones.get(i).unwrap();
            states.push_back(MilestoneState {
                name: m.name,
                release_bps: m.release_bps,
                description_hash: m.description_hash,
                status: MilestoneStateStatus::Pending,
                delivery_hash: None,
                rejection_hash: None,
            });
        }

        env.storage()
            .persistent()
            .set(&DataKey2::EscrowMilestonesV2(escrow_id), &states);
        env.storage().persistent().extend_ttl(
            &DataKey2::EscrowMilestonesV2(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
        escrow_id
    }

    /// Seller marks a milestone as delivered by submitting a delivery hash.
    /// The milestone must be in `Pending` or `Rejected` state.
    pub fn submit_milestone(
        env: Env,
        seller: Address,
        escrow_id: u32,
        milestone_index: u32,
        delivery_hash: BytesN<32>,
    ) {
        seller.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        if escrow.seller != seller {
            panic!("Only the escrow seller may submit milestones");
        }
        if escrow.status != EscrowStatus::Active {
            panic!("Escrow is not active");
        }

        let mut states: Vec<MilestoneState> = env
            .storage()
            .persistent()
            .get(&DataKey2::EscrowMilestonesV2(escrow_id))
            .expect("No BPS milestones for this escrow");

        if milestone_index >= states.len() {
            panic!("Milestone index out of range");
        }
        let mut state = states.get(milestone_index).unwrap();
        if state.status != MilestoneStateStatus::Pending
            && state.status != MilestoneStateStatus::Rejected
        {
            panic!("Milestone must be Pending or Rejected to submit");
        }

        state.status = MilestoneStateStatus::Submitted;
        state.delivery_hash = Some(delivery_hash.clone());
        state.rejection_hash = None;
        states.set(milestone_index, state);

        env.storage()
            .persistent()
            .set(&DataKey2::EscrowMilestonesV2(escrow_id), &states);
        env.storage().persistent().extend_ttl(
            &DataKey2::EscrowMilestonesV2(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_milestone_submitted(&env, escrow_id, milestone_index, delivery_hash);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Buyer (or arbiter) approves a submitted milestone, releasing its
    /// proportional share (`amount * release_bps / 10_000`) to the seller.
    /// The final milestone releases any rounding remainder.
    pub fn approve_proportional_milestone(
        env: Env,
        caller: Address,
        escrow_id: u32,
        milestone_index: u32,
    ) {
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        if caller != escrow.buyer && caller != escrow.arbiter {
            panic!("Only buyer or arbiter can approve milestones");
        }
        if escrow.status != EscrowStatus::Active {
            panic!("Escrow is not active");
        }

        let mut states: Vec<MilestoneState> = env
            .storage()
            .persistent()
            .get(&DataKey2::EscrowMilestonesV2(escrow_id))
            .expect("No BPS milestones for this escrow");

        if milestone_index >= states.len() {
            panic!("Milestone index out of range");
        }
        let mut state = states.get(milestone_index).unwrap();
        if state.status != MilestoneStateStatus::Submitted {
            panic!("Milestone must be Submitted before approval");
        }

        // Calculate release amount using bps; final milestone gets remainder
        let is_last = milestone_index == states.len() - 1;
        let amount_released = if is_last {
            // Sum already-released amounts to compute remainder
            let mut already_released: i128 = 0;
            for i in 0..states.len() - 1 {
                let s = states.get(i).unwrap();
                if s.status == MilestoneStateStatus::Approved {
                    already_released += escrow.amount * s.release_bps as i128 / 10_000;
                }
            }
            escrow.amount - already_released
        } else {
            escrow.amount * state.release_bps as i128 / 10_000
        };

        let token_client = token::Client::new(&env, &escrow.token);
        token_client.transfer(
            &env.current_contract_address(),
            &escrow.seller,
            &amount_released,
        );

        state.status = MilestoneStateStatus::Approved;
        states.set(milestone_index, state);

        env.storage()
            .persistent()
            .set(&DataKey2::EscrowMilestonesV2(escrow_id), &states);
        env.storage().persistent().extend_ttl(
            &DataKey2::EscrowMilestonesV2(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Auto-transition escrow to Released when all milestones are terminal
        let mut all_terminal = true;
        for i in 0..states.len() {
            let s = states.get(i).unwrap();
            if s.status == MilestoneStateStatus::Pending
                || s.status == MilestoneStateStatus::Submitted
            {
                all_terminal = false;
                break;
            }
        }
        if all_terminal {
            escrow.status = EscrowStatus::Released;
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(escrow_id), &escrow);
            env.storage().persistent().extend_ttl(
                &DataKey::Escrow(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        events::emit_milestone_approved(&env, escrow_id, milestone_index, amount_released);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Buyer rejects a submitted milestone, returning it to `Pending` state.
    /// The seller can re-submit after addressing the buyer's concerns.
    pub fn reject_milestone(
        env: Env,
        buyer: Address,
        escrow_id: u32,
        milestone_index: u32,
        rejection_hash: BytesN<32>,
    ) {
        buyer.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        if escrow.buyer != buyer {
            panic!("Only the escrow buyer may reject milestones");
        }
        if escrow.status != EscrowStatus::Active {
            panic!("Escrow is not active");
        }

        let mut states: Vec<MilestoneState> = env
            .storage()
            .persistent()
            .get(&DataKey2::EscrowMilestonesV2(escrow_id))
            .expect("No BPS milestones for this escrow");

        if milestone_index >= states.len() {
            panic!("Milestone index out of range");
        }
        let mut state = states.get(milestone_index).unwrap();
        if state.status != MilestoneStateStatus::Submitted {
            panic!("Only a Submitted milestone can be rejected");
        }

        state.status = MilestoneStateStatus::Rejected;
        state.rejection_hash = Some(rejection_hash);
        states.set(milestone_index, state);

        env.storage()
            .persistent()
            .set(&DataKey2::EscrowMilestonesV2(escrow_id), &states);
        env.storage().persistent().extend_ttl(
            &DataKey2::EscrowMilestonesV2(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_milestone_rejected(&env, escrow_id, milestone_index);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Read the BPS milestone state schedule for an escrow.
    pub fn get_bps_milestones(env: Env, escrow_id: u32) -> Vec<MilestoneState> {
        env.storage()
            .persistent()
            .get(&DataKey2::EscrowMilestonesV2(escrow_id))
            .expect("No BPS milestones for this escrow")
    }

}

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_dispute_timeout;

#[cfg(test)]
mod test_token_whitelist;

#[cfg(test)]
mod test_cooling_off;

#[cfg(test)]
mod test_seller_veto;

#[cfg(test)]
mod test_inspector;
#[cfg(test)]
mod test_milestone_bps;
