use soroban_sdk::{contracttype, Address, BytesN, Map, String, Vec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum PayoutStrategy {
    RoundRobin = 0,
    AdminAssigned = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum DistributionType {
    Equal = 0,
    Proportional = 1,
    Weighted = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum VotingMode {
    Equal = 0,
    WeightedByContributions = 1,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoscaConfig {
    pub strategy: PayoutStrategy,
    pub custom_order: Option<Vec<Address>>,
    pub penalty_amount: i128,
    pub exit_penalty_bps: u32,
    pub collective_goal: Option<i128>,
    pub member_goals: Option<Map<Address, i128>>,
    /// Protocol fee in basis points (e.g., 100 = 1%, 500 = 5%). Max 500 bps.
    pub fee_bps: u32,
    /// Address that receives protocol fees
    pub fee_recipient: Option<Address>,
    /// Number of consecutive missed rounds before suspension (default: 3)
    pub max_defaults: u32,
    /// Additional ledgers (time units) before penalties are applied after deadline.
    pub grace_period_ledgers: u32,
    pub use_timestamp_schedule: bool,
    pub round_duration_seconds: u64,
    pub max_members: Option<u32>,
    pub skip_fee: i128,
    pub max_skips_per_cycle: u32,
    pub voting_mode: VotingMode,
    /// Late fee in basis points applied to contributions during the grace period.
    /// Collected from the late contributor and distributed to on-time members.
    /// 0 = no late fee (grace period is free). Max 1000 bps (10%).
    pub late_fee_bps: u32,
    /// Grace period duration in seconds (timestamp-based schedule).
    /// Used when use_timestamp_schedule = true. 0 = no grace period.
    pub grace_period_seconds: u64,
    /// Enable the slot auction mechanism for this group.
    /// When true, an auction opens at the start of each new cycle.
    pub auction_enabled: bool,
    /// Number of ledger timestamps (seconds) the bidding window stays open.
    /// Ignored when auction_enabled = false.
    pub auction_window_ledgers: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupInfo {
    pub members: Vec<Address>,
    pub contribution_amount: i128,
    pub token: Address,
    pub current_round: u32,
    pub total_rounds: u32,
    pub paid_members: Vec<Address>,
    pub next_recipient: Address,
    /// Timestamp (seconds) by which all contributions for the current round must be received.
    pub round_deadline: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayoutRecord {
    pub recipient: Address,
    pub amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExitRequest {
    pub member: Address,
    pub rounds_contributed: u32,
    /// Computed dynamically in `approve_exit` from rounds_contributed, payout history, and
    /// exit_penalty_bps; not stored at request time.
    pub refund_amount: i128,
    pub approved: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberStatus {
    pub is_member: bool,
    pub is_suspended: bool,
    pub is_exited: bool,
    pub contributions_this_round: i128,
    pub has_paid_this_round: bool,
    pub default_count: u32,
    pub lifetime_contributions: i128,
    pub claimable_rewards: i128,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum ProposalType {
    PenaltyAppeal = 0,
    RuleChange = 1,
    MemberRemoval = 2,
    MaxMembersUpdate = 3,
    Reinstatement = 4, // #218
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum ProposalStatus {
    Pending = 0,
    Approved = 1,
    Rejected = 2,
    Executed = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Proposal {
    pub id: u32,
    pub proposal_type: ProposalType,
    pub creator: Address,
    pub description: String,
    pub target_member: Address,
    pub votes_for: i128,
    pub votes_against: i128,
    pub created_at: u64,
    pub deadline: u64,
    pub status: ProposalStatus,
    pub execution_data: Option<i128>,
    pub required_quorum: u32, // bps (e.g. 5100 = 51%)
}

/// Storage key classification:
///
/// INSTANCE (config + active round state — bounded, shared TTL):
///   Admin, Members, PayoutOrder, Strategy, ContributionAmt, Token,
///   CurrentRound, PaidMembers, RoundDuration, RoundDeadline, Defaulters,
///   PenaltyAmount, DefaultCount, SuspendedMembers, ApprovedTokens,
///   RewardPool, TotalParticipations, MemberParticipation, ClaimedRewards,
///   RewardWeights, RewardDistType, ExitedMembers, ExitPenaltyBps,
///   IsPaused, PauseReason, PauseTimestamp, CollectiveGoal, TotalCollected,
///   MemberGoals, MemberCollected, MilestonesReached, ExchangeRates,
///   TokenLimits, ProposalCounter, Proposals, ProposalVotes,
///   VotingDeadline, QuorumPercentage, MemberContributions
///
/// PERSISTENT (unbounded growth — individual TTL per key):
///   RoundHistory — appended every round; must outlive instance TTL
///
/// TEMPORARY (short-lived in-progress state — auto-expires):\
///   ExitRequests — pending admin approval; no long-term retention needed
#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    // --- Instance ---
    Admin,                   // Address
    Members,                 // Vec<Address>
    PayoutOrder,             // Vec<Address>
    Strategy,                // PayoutStrategy
    ContributionAmt,         // i128
    Token,                   // Address
    CurrentRound,            // u32
    PaidMembers,             // Vec<Address>
    RoundDuration,           // u64
    RoundDeadline,           // u64
    Defaulters,              // Vec<Address>
    PenaltyAmount,           // i128
    DefaultCount,            // Map<Address, u32>
    SuspendedMembers,        // Vec<Address>
    ApprovedTokens,          // Vec<Address>
    RewardPool,              // i128
    TotalParticipations,     // u32
    MemberParticipation,     // Map<Address, u32>
    ClaimedRewards,          // Map<Address, i128>
    RewardWeights,           // Map<Address, u32>
    RewardDistType,          // DistributionType
    ExitedMembers,           // Vec<Address>
    ExitPenaltyBps,          // u32 (basis points, e.g. 1000 = 10%)
    Paused,                  // bool (global pause alias)
    IsPaused,                // bool
    PauseReason,             // String
    PauseTimestamp,          // u64
    CollectiveGoal,          // i128
    TotalCollected,          // i128
    MemberGoals,             // Map<Address, i128>
    MemberCollected,         // Map<Address, i128>
    MilestonesReached,       // Vec<u32> (e.g. 25, 50, 75, 100)
    ExchangeRates,           // Map<Address, i128>
    TokenLimits,             // Map<Address, i128>
    ProposalCounter,         // u32
    Proposals,               // Map<u32, Proposal>
    ProposalVotes,           // Map<u32, Map<Address, bool>>
    VotingDeadline,          // u64
    QuorumPercentage,        // u32 (e.g., 51 for 51%)
    MemberContributions,     // Map<Address, i128> cumulative per round
    ProposedAdmin,           // Address — proposed new admin (pending acceptance)
    ContractVersion,         // u32
    FeeBps,                  // u32 — protocol fee in basis points
    FeeRecipient,            // Address — receives protocol fees
    MaxDefaults,             // u32 — suspension threshold
    UseTimestampSchedule,    // bool
    RoundDurationSeconds,    // u64
    RoundDeadlineTimestamp,  // u64
    MaxMembers,              // u32
    MemberTiers,             // Map<Address, u32>
}

/// Overflow key enum — DataKey is capped at 50 variants by the soroban XDR limit.
/// Less-frequently-used instance keys go here.
#[derive(Clone)]
#[contracttype]
pub enum DataKey2 {
    InsurancePool,           // i128
    InsuranceContributionBps, // u32
    SkipFee,                 // i128
    MaxSkipsPerCycle,        // u32
    SkipRequests,            // Map<(Address, u32), bool>
    MemberSkips,             // Map<(Address, u32), u32>
    QuorumConfig,            // Map<ProposalType, u32>
    VotingMode,              // VotingMode
    ReinvestPreference,      // Map<Address, bool>
    ExitRequests,            // Map<Address, ExitRequest>
    /// Token whitelist contract address
    TokenWhitelistContract,  // Address
    // Audit Trail
    CycleRecords,            // Map<u32, CycleRecord> — per-cycle audit trail
    CycleRecordRetentionWindow, // u32 — number of cycles to retain in persistent storage
    ArchivedCycleRecords,    // Map<u32, CycleRecord> — archived records in temporary storage
    CycleStartTimestamps,    // Map<u32, u64> — track when each cycle started
    // Emergency Payout
    EmergencyPayoutConfig,   // EmergencyPayoutConfig
    EmergencyPayoutRequests, // Map<(u32, Address), EmergencyPayoutRequest> — (round, requester)
    EmergencyPayoutVotes,    // Map<(u32, Address, Address), bool> — (round, requester, voter)
    EmergencyPayoutCount,    // Map<u32, u32> — (cycle, count) — emergency payouts per cycle
    EmergencyPayoutApproved, // Map<(u32, Address), bool> — (round, requester) — track approved emergency payouts per cycle
    // Group Dissolution
    GroupStatus,             // GroupStatus
    DissolutionConfig,       // DissolutionConfig
    DissolutionVotes,        // Map<(u32, Address), bool> — (round, voter)
    DissolutionVoteCount,    // Map<u32, i128> — (round, votes_for)
    DissolutionDeadline,     // Map<u32, u64> — (round, deadline)
    // #213: Payout Slot Swap
    SlotSwapCounter,
    SlotSwaps,               // Map<u32, SlotSwap>
    SlotSwapRequiresAdmin,   // bool
    SlotSwapExpirySeconds,   // u64
    // #214: Insurance Coverage
    InsuranceCoverageMode,   // InsuranceCoverageMode
    InsuranceClaims,         // Map<u32, Vec<InsuranceClaim>>
    // #218: Reinstatement
    ReinstatementFee,        // i128
    PendingReinstatementFee, // Vec<Address>
    ActiveReinstatementProposal, // Map<Address, u32>
    // Waitlist (#219)
    Waitlist,                // Vec<(Address, u64)> — (address, joined_at)
    CatchUpDebt,             // Map<Address, i128> — catch-up contributions owed
    // #230: Group Merge
    MergeProposalCounter,    // u32
    MergeProposals,          // Map<u32, MergeProposal>
    GroupMergedInto,         // u32 — target group_id this group was merged into
    // #224: Cycle Completion Bonus
    CycleBonusAmount,        // i128 — bonus per qualifying member per cycle
    // #227: Round Duration Update
    PendingRoundDuration,    // u64 — new duration to apply at next round start
    MinRoundDuration,        // u64 — lower bound for round duration
    MaxRoundDuration,        // u64 — upper bound for round duration
    // Waitlist (#219)
    StartAt,                 // u64
    GroupActivationEmitted,  // bool
    GracePeriodLedgers,      // u32
    PendingPenalties,        // Map<Address, u32> (member -> round)
    LastRoundDeadline,       // u64
    // #240: Co-Signer Guarantee
    CoSigners,               // Map<Address, CoSignerRecord> — member → co-signer record
    CoSignerWindowLedgers,   // u32 — grace period ledgers before penalty applied
}

/// Overflow key enum — DataKey2 is capped at 50 variants by the soroban XDR limit.
#[derive(Clone)]
#[contracttype]
pub enum DataKey3 {
    CoSignerWindowStart,     // Map<Address, u32> — member → ledger when window opened (#240)
    ProxyAuthorizations,     // Map<(u32, Address), ProxyAuthorization> — (group_id, member)
    IsFrozen,                // bool — group is frozen by contract-level admin (#236)
    // #267: Tiered Contribution Levels
    GroupTiers,              // Vec<Tier> — named tier definitions
    MemberTierIndex,         // Map<Address, u32> — member → tier_id
    PendingTierChange,       // Map<Address, u32> — queued tier changes for next cycle
    // #269: On-Chain Member Credit Score
    ScoreWeights,            // ScoreWeights — admin-configurable scoring formula weights
    MinCreditScore,          // i128 — minimum score required to join this group
    // Slot Auction
    AuctionEnabled,          // bool — auction feature flag
    AuctionWindowLedgers,    // u64 — bidding window duration in seconds
    AuctionOpenUntil,        // u64 — timestamp when current auction window closes (0 = no open auction)
    AuctionBids,             // Vec<SlotBid> — bids placed in the current auction
    AuctionRound,            // u32 — the round for which the current auction was opened
    // Cross-Group Migration
    MigrationRequests,       // Map<Address, MigrationRequest> — member → pending outbound migration
    IncomingMigrations,      // Map<Address, IncomingMigration> — member → pending inbound migration
    MigratedMembers,         // Map<Address, MigratedMemberRecord> — member → migration annotation
    VacantSlots,             // Vec<u32> — slot indices freed by migrated-out members
    /// #314: Group treasury configuration
    TreasuryConfig,          // TreasuryConfig
    /// #314: Group treasury balance
    TreasuryBalance,         // i128
    /// #314: Treasury round proposals per round
    TreasuryRoundProposal(u32), // (round_index) → TreasuryRoundProposal
    /// #314: Treasury round votes per member
    TreasuryRoundVotes(u32, Address), // (round_index, member) → bool
    // #330: Contribution Delegation
    ContribDelegations,      // Map<Address, ContribDelegationRecord> — member → delegation
    // #331: Group Split
    SplitProposalCounter,    // u32
    SplitProposals,          // Map<u32, SplitProposal>
    SplitConfirmationWindow, // u32 — ledgers members have to confirm
}

// ── #330: Contribution Delegation ────────────────────────────────────────────

/// Delegation record granting a proxy the right to act for a member.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContribDelegationRecord {
    pub proxy: Address,
    pub expiry_ledger: u64,
}

// ── #331: Group Split ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum SplitProposalStatus {
    Pending = 0,
    Executed = 1,
    Expired = 2,
}

/// Proposal to divide one ROSCA group into two independent sub-groups.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitProposal {
    pub id: u32,
    pub group_a_members: Vec<Address>,
    pub group_b_members: Vec<Address>,
    pub split_reason_hash: BytesN<32>,
    pub confirmations: Vec<Address>,
    pub status: SplitProposalStatus,
    pub created_at_ledger: u32,
    pub expiry_ledger: u32,
}

/// Persistent storage keys — kept separate because DataKey was hitting
/// the 64-variant limit enforced by the `#[contracttype]` macro.
#[derive(Clone)]
#[contracttype]
pub enum PersistentKey {
    RoundHistory,              // Vec<PayoutRecord> — grows every round
    ReputationScores,          // Map<Address, i128> — cumulative member reliability score
    FreezeLog,                 // Vec<FreezeRecord> — append-only freeze audit log
    SnapshotLog,               // Vec<GroupSnapshot> — append-only snapshot log (#243)
    LastSnapshotLedger,        // u32 — last snapshot ledger for spam guard (#243)
    MinSnapshotIntervalLedgers, // u32 — min interval between snapshots (#243)
    MemberCreditScores,        // Map<Address, MemberScore> — per-member credit score (#269)
}

/// Record of a single freeze/unfreeze cycle for a group.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreezeRecord {
    pub frozen_at_ledger: u32,
    pub frozen_by: Address,
    pub reason_hash: BytesN<32>,
    pub unfrozen_at_ledger: Option<u32>,
    pub resolution_hash: Option<BytesN<32>>,
}

/// On-chain group state snapshot for immutable audit (#243).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupSnapshot {
    pub snapshot_id: u32,
    pub taken_at_ledger: u32,
    pub taken_by: Address,
    pub round_number: u32,
    pub pooled_balance: i128,
    pub member_statuses: Vec<MemberStatus>,
    pub payout_order: Vec<Address>,
    pub state_hash: BytesN<32>,
}

// #240: Co-Signer Guarantee

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum CoSignerStatus {
    Pending = 0,   // set by member, not yet accepted
    Active = 1,    // accepted by co-signer
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoSignerRecord {
    pub co_signer: Address,
    pub status: CoSignerStatus,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProxyAuthorization {
    pub proxy: Address,
    pub max_rounds: u32,
    pub used_rounds: u32,
}

// ── Audit Trail ────────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContributionEntry {
    pub member: Address,
    pub amount: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CycleRecord {
    pub cycle_number: u32,
    pub total_pool_amount: i128,
    pub payout_recipient: Address,
    pub payout_amount: i128,
    pub contributions: Vec<ContributionEntry>,
    pub defaulters: Vec<Address>,
    pub skippers: Vec<Address>,
    pub penalties_collected: i128,
    pub fee_collected: i128,
    pub insurance_drawn: i128,
    pub cycle_start_timestamp: u64,
    pub cycle_end_timestamp: u64,
}

// --- Emergency Payout Types ---

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmergencyPayoutRequest {
    pub requester: Address,
    pub reason_hash: BytesN<32>,
    pub created_at: u64,
    pub deadline: u64,
    pub votes_for: i128,
    pub votes_against: i128,
    pub executed: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmergencyPayoutConfig {
    pub emergency_quorum_bps: u32,      // e.g., 6667 = 66.67%
    pub vote_window_seconds: u64,       // how long voting lasts
    pub max_emergency_per_cycle: u32,   // max emergency payouts per cycle
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum GroupStatus {
    Active = 0,
    Dissolved = 1,
    /// Group was merged into another group; all further interactions are rejected.
    Merged = 2,
    /// Group was split into two sub-groups; no further operations permitted.
    Split = 3,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DissolutionConfig {
    pub dissolution_quorum_bps: u32,    // e.g., 7500 = 75%
    pub vote_window_seconds: u64,
}

/// #230: Merge proposal between two ROSCA groups.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeProposal {
    pub id: u32,
    pub group_a_admin: Address,
    pub group_b_id: u32,
    pub proposed_at: u64,
    pub accepted: bool,
}

// #213: Payout Slot Swap
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum SlotSwapStatus {
    Pending = 0,
    Accepted = 1,
    Rejected = 2,
    Executed = 3,
    Expired = 4,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlotSwap {
    pub id: u32,
    pub initiator: Address,
    pub counterparty: Address,
    pub round_a: u32,
    pub round_b: u32,
    pub status: SlotSwapStatus,
    pub created_at: u64,
    pub expiry_at: u64,
    pub admin_approved: bool,
}

// #214: Insurance Coverage Mode & Claims
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum InsuranceCoverageMode {
    None = 0,
    Partial = 1,
    Full = 2,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InsuranceClaim {
    pub round: u32,
    pub defaulter: Address,
    pub amount_covered: i128,
}

// ── #267: Tiered Contribution Levels ──────────────────────────────────────────

/// A contribution tier definition — name, fixed contribution amount, and payout weight.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tier {
    pub name: soroban_sdk::Symbol,
    pub contribution_amount: i128,
    pub payout_weight: u32,
}

// ── #269: On-Chain Member Credit Score ────────────────────────────────────────

/// Accumulated contribution-behaviour record for a member (#269).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemberScore {
    pub on_time_contributions: u32,
    pub late_contributions: u32,
    pub defaults: u32,
    pub early_exits: u32,
    pub groups_completed: u32,
    /// Computed numeric score derived from the above counters and ScoreWeights.
    pub score: i128,
}

/// Admin-configurable weights used to compute the credit score (#269).
/// Positive weights increase score; negative weights decrease it.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScoreWeights {
    pub on_time_weight: i128,
    pub late_weight: i128,
    pub default_weight: i128,
    pub exit_weight: i128,
    pub completion_weight: i128,
}

// ── Slot Auction (#slot-auction) ──────────────────────────────────────────────

/// A single bid placed during a slot auction.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SlotBid {
    /// The member who placed this bid.
    pub bidder: Address,
    /// The payout-order slot index the bidder wants to move into.
    pub desired_slot: u32,
    /// Amount of base token deposited as the bid.
    pub amount: i128,
    /// Ledger timestamp at which the bid was placed (used for tie-breaking).
    pub placed_at: u64,
}

// ── Cross-Group Member Migration ───────────────────────────────────────────────

/// Approval state for a pending cross-group migration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum MigrationApprovalState {
    /// Neither admin has approved yet.
    Pending = 0,
    /// Source admin approved; waiting for destination admin.
    SourceApproved = 1,
    /// Destination admin approved; waiting for source admin.
    DestApproved = 2,
    /// Both admins approved — ready to execute.
    BothApproved = 3,
    /// Migration has been executed.
    Executed = 4,
}

/// A pending cross-group migration request stored on the **source** contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationRequest {
    /// The member who wants to migrate.
    pub member: Address,
    /// Address of the destination group contract.
    pub to_group: Address,
    /// Slot index in the destination group's payout order.
    pub target_slot: u32,
    /// Approval state.
    pub state: MigrationApprovalState,
    /// Timestamp when the request was created.
    pub created_at: u64,
}

/// Contribution history summary carried from the source group to the destination.
/// Stored on the **destination** contract as a `MigratedMember` annotation.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigratedMemberRecord {
    /// Address of the source group contract.
    pub from_group: Address,
    /// Number of rounds the member fully completed in the source group.
    pub rounds_completed: u32,
    /// Number of on-time (full, non-late) contributions in the source group.
    pub on_time_count: u32,
    /// Slot index assigned in this (destination) group.
    pub slot_index: u32,
    /// Timestamp when the migration was executed.
    pub migrated_at: u64,
}

/// Incoming migration approval stored on the **destination** contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IncomingMigration {
    /// The member being migrated in.
    pub member: Address,
    /// Address of the source group contract.
    pub from_group: Address,
    /// Slot index to insert the member at.
    pub target_slot: u32,
    /// Whether the destination admin has approved.
    pub dest_approved: bool,
}

/// Group treasury configuration (#314)
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreasuryConfig {
    pub treasury_admin: Address,
    pub enabled: bool,
}

/// Treasury round proposal (#314)
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TreasuryRoundProposal {
    pub round_index: u32,
    pub purpose_hash: BytesN<32>,
    pub proposed_at: u64,
    pub votes_for: i128,
    pub votes_against: i128,
    pub confirmed: bool,
}
