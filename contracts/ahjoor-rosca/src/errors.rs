use soroban_sdk::contracterror;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    AlreadyInitialized = 1,
    TokenNotApproved = 2,
    CustomOrderLengthMismatch = 3,
    CustomOrderNonMember = 4,
    AmountMustBePositive = 5,
    RoundDeadlinePassed = 6,
    MemberHasExited = 7,
    NotAMember = 8,
    AlreadyContributed = 9,
    InvalidExchangeRate = 10,
    ExceedsTokenLimit = 11,
    ExceedsRemainingContribution = 12,
    DeadlineNotPassed = 13,
    PenaltyDisabled = 14,
    NotADefaulter = 15,
    CannotChangeMidRound = 16,
    AlreadyAMember = 17,
    NoRewardsToClaim = 18,
    OnlyMembersAllowed = 19,
    ProposalNotFound = 20,
    VotingDeadlinePassed = 21,
    ProposalNotPending = 22,
    AlreadyVoted = 23,
    VotingNotEnded = 24,
    ContractPaused = 25,
    AllMembersSuspended = 26,
    AlreadyPaused = 27,
    NotPaused = 28,
    MemberAlreadyExited = 29,
    ExitRequestPending = 30,
    NoExitRequestFound = 31,
    ExitNotAllowedMidRound = 32,
    /// Contribution rejected because the round deadline has passed.
    ContributionWindowClosed = 33,
    /// Fee basis points exceeds maximum allowed (500 bps = 5%).
    FeeExceedsMaximum = 34,
    /// Max defaults must be at least 1.
    InvalidMaxDefaults = 35,
    /// Maximum members reached.
    GroupFull = 36,
    /// Invalid maximum member count (must be between 1 and 100).
    InvalidMaxMembers = 37,
    /// Delegation already exists for this delegator.
    DelegationAlreadyExists = 38,
    /// No delegation found for this delegator.
    NoDelegationFound = 39,
    /// Delegator cannot vote while delegation is active.
    CannotVoteWithActiveDelegation = 40,
    /// Delegate cannot further sub-delegate.
    CannotSubDelegate = 41,
    /// Invite not found or expired.
    InviteNotFound = 42,
    /// Invite has already been redeemed.
    InviteAlreadyRedeemed = 43,
    /// Invite is for a different address.
    InviteWrongRecipient = 44,
    /// Admin action not found.
    AdminActionNotFound = 45,
    /// Admin action has already been executed.
    AdminActionAlreadyExecuted = 46,
    /// Admin action has expired.
    AdminActionExpired = 47,
    /// Admin has already approved this action.
    AdminAlreadyApproved = 48,
    /// Insufficient approvals for admin action.
    InsufficientApprovals = 49,
    /// Not a co-admin.
    NotACoAdmin = 50,
    /// Emergency payout already requested for this member in this cycle.
    EmergencyPayoutAlreadyRequested = 51,
    /// Emergency payout quorum not met.
    EmergencyPayoutQuorumNotMet = 52,
    /// Emergency payout vote window expired.
    EmergencyPayoutVoteExpired = 53,
    /// Emergency payout already executed for this member in this cycle.
    EmergencyPayoutAlreadyExecuted = 54,
    /// Maximum emergency payouts per cycle reached.
    EmergencyPayoutLimitReached = 55,
    /// Group is already dissolved.
    GroupAlreadyDissolved = 56,
    /// Dissolution vote already in progress.
    DissolutionVoteInProgress = 57,
    /// Dissolution quorum not met.
    DissolutionQuorumNotMet = 58,
    /// Dissolution vote window expired.
    DissolutionVoteExpired = 59,
    /// No funds to distribute during dissolution.
    NoFundsToDistribute = 60,
    /// Invalid emergency payout configuration.
    InvalidEmergencyConfig = 61,
    /// Invalid dissolution configuration.
    InvalidDissolutionConfig = 62,
    /// Only admin is allowed to perform this action.
    OnlyAdminAllowed = 63,
    /// Invalid amount provided.
    InvalidAmount = 64,
    /// Round duration is out of the configured bounds.
    RoundDurationOutOfBounds = 65,
}

/// Extension error codes 51-56 — split from Error because #[contracterror]
/// is bounded by the soroban XDR 50-case limit.
#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtError {
    /// Tier must be at least 1 bps.
    InvalidTier = 51,
    /// Insurance pool balance would go negative.
    InsurancePoolNegative = 52,
    /// Invalid insurance contribution amount.
    InvalidInsuranceContribution = 53,
    /// Member has reached the maximum allowed skips for the current cycle.
    SkipLimitReached = 54,
    /// Member has already requested a skip for this round.
    AlreadySkipped = 55,
    /// Member has zero contribution weight in weighted voting mode.
    InsufficientWeight = 56,
    /// Action requires admin privileges.
    OnlyAdminAllowed = 70,
    /// Invalid amount or index range.
    InvalidAmount = 71,
    /// Emergency payout already requested for this member in this cycle.
    EmergencyPayoutRequested = 57,
    /// Emergency payout quorum not met.
    EmergencyPayoutQuorumNotMet = 58,
    /// Emergency payout vote window expired.
    EmergencyPayoutVoteExpired = 59,
    /// Emergency payout already executed for this member in this cycle.
    EmergencyPayoutAlreadyExecuted = 60,
    /// Maximum emergency payouts per cycle reached.
    EmergencyPayoutLimitReached = 61,
    /// Group is already dissolved.
    GroupAlreadyDissolved = 62,
    /// Dissolution vote already in progress.
    DissolutionVoteInProgress = 63,
    /// Dissolution quorum not met.
    DissolutionQuorumNotMet = 64,
    /// Dissolution vote window expired.
    DissolutionVoteExpired = 65,
    /// No funds to distribute during dissolution.
    NoFundsToDistribute = 66,
    /// Invalid emergency payout configuration.
    InvalidEmergencyConfig = 67,
    /// Invalid dissolution configuration.
    InvalidDissolutionConfig = 68,
    /// Group start time is in the future.
    GroupNotYetActive = 69,
    /// Co-signer already set for this member.
    CoSignerAlreadySet = 72,
    /// No co-signer found for this member.
    NoCoSignerFound = 73,
    /// Co-signer has not accepted the designation.
    CoSignerNotAccepted = 74,
    /// Not the designated co-signer for this member.
    NotTheCoSigner = 75,
    /// Co-signer window has not opened (member has not defaulted).
    CoSignerWindowNotOpen = 76,
    /// Co-signer window has expired.
    CoSignerWindowExpired = 77,
    /// Group is frozen by contract-level admin pending investigation.
    GroupFrozen = 72,
    /// Group is not currently frozen.
    GroupNotFrozen = 73,
    /// Snapshot taken too soon; min_snapshot_interval_ledgers not elapsed (#243).
    SnapshotTooSoon = 72,
}
