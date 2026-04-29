#![no_std]
use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, Address, Bytes, BytesN, Env, Map, String, Symbol, Vec,
};
use ahjoor_token_whitelist::TokenWhitelistClient;

// Instance storage: config, counters, and active round state (bounded, shared TTL)
const INSTANCE_LIFETIME_THRESHOLD: u32 = 100_000;
const INSTANCE_BUMP_AMOUNT: u32 = 120_000;

// Persistent storage: RoundHistory (grows by one record per round — unbounded)
// Each write extends its own TTL independently of the instance.
const PERSISTENT_LIFETIME_THRESHOLD: u32 = 100_000;
const PERSISTENT_BUMP_AMOUNT: u32 = 120_000;

// Temporary storage: ExitRequests (in-progress, pending admin approval — short-lived)
// Auto-expires if not acted upon; no long-term retention needed.
const TEMP_LIFETIME_THRESHOLD: u32 = 10_000;
const TEMP_BUMP_AMOUNT: u32 = 15_000;

pub mod types;
pub use types::*;

mod errors;
mod events;
mod internals;
mod audit_trail;
mod test_tiers;
mod test_weighted_voting;
mod test_reinvest;
mod test_token_whitelist;

use crate::errors::{Error, ExtError};

#[contract]
pub struct AhjoorContract;

#[contractimpl]
impl AhjoorContract {
    pub fn init(
        env: Env,
        admin: Address,
        members: Vec<Address>,
        contribution_amount: i128,
        token: Address,
        round_duration: u64,
        config: RoscaConfig,
        start_at: Option<u64>,
    ) {
        if env.storage().instance().has(&DataKey::Members) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }

        // Validate fee_bps: max 500 bps (5%)
        if config.fee_bps > 500 {
            panic_with_error!(&env, Error::FeeExceedsMaximum);
        }

        // Validate max_defaults: must be at least 1
        if config.max_defaults < 1 {
            panic_with_error!(&env, Error::InvalidMaxDefaults);
        }

        // Validate max_members: 1 <= max_members <= 100
        let max_members = config.max_members.unwrap_or(50);
        if max_members < 1 || max_members > 100 {
            panic_with_error!(&env, Error::InvalidMaxMembers);
        }
        if (members.len() as u32) > max_members {
            panic_with_error!(&env, Error::GroupFull);
        }

        let approved_tokens: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ApprovedTokens)
            .unwrap_or(Vec::new(&env));

        if !approved_tokens.is_empty() && !approved_tokens.contains(&token) {
            panic_with_error!(&env, Error::TokenNotApproved);
        }

        // Token whitelist validation for base token
        Self::require_token_allowed(&env, &token);

        let resolved_order = match config.strategy {
            PayoutStrategy::RoundRobin => members.clone(),
            PayoutStrategy::AdminAssigned => {
                let order = config
                    .custom_order
                    .expect("AdminAssigned strategy requires a custom order");
                if order.len() != members.len() {
                    panic_with_error!(&env, Error::CustomOrderLengthMismatch);
                }
                for member in order.iter() {
                    if !members.contains(&member) {
                        panic_with_error!(&env, Error::CustomOrderNonMember);
                    }
                }
                order
            }
        };

        let now = env.ledger().timestamp();
        let resolved_start_at = start_at.unwrap_or(now);
        let deadline = resolved_start_at + round_duration;
        let member_count = members.len();

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &1u32);
        env.storage().instance().set(&DataKey::Members, &members);
        env.storage()
            .instance()
            .set(&DataKey::PayoutOrder, &resolved_order);
        env.storage()
            .instance()
            .set(&DataKey::Strategy, &config.strategy);
        env.storage()
            .instance()
            .set(&DataKey::ContributionAmt, &contribution_amount);
        env.storage().instance().set(&DataKey::Token, &token);

        // Auto-approve the base token
        let mut approved_tokens: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ApprovedTokens)
            .unwrap_or(Vec::new(&env));
        if !approved_tokens.contains(&token) {
            approved_tokens.push_back(token.clone());
            env.storage()
                .instance()
                .set(&DataKey::ApprovedTokens, &approved_tokens);
        }

        env.storage().instance().set(&DataKey::CurrentRound, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::PaidMembers, &Vec::<Address>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::RoundDuration, &round_duration);
        env.storage()
            .instance()
            .set(&DataKey::RoundDeadline, &deadline);
        env.storage()
            .instance()
            .set(&DataKey2::LastRoundDeadline, &deadline);
        env.storage()
            .instance()
            .set(&DataKey2::StartAt, &resolved_start_at);
        env.storage()
            .instance()
            .set(&DataKey2::GroupActivationEmitted, &false);
        env.storage()
            .instance()
            .set(&DataKey::Defaulters, &Vec::<Address>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::PenaltyAmount, &config.penalty_amount);
        env.storage()
            .instance()
            .set(&DataKey::DefaultCount, &Map::<Address, u32>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::SuspendedMembers, &Vec::<Address>::new(&env));
        // Persistent: RoundHistory grows by one record per round — must not share instance TTL
        env.storage()
            .persistent()
            .set(&PersistentKey::RoundHistory, &Vec::<PayoutRecord>::new(&env));
        env.storage().persistent().extend_ttl(
            &PersistentKey::RoundHistory,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage()
            .persistent()
            .set(&PersistentKey::ReputationScores, &Map::<Address, i128>::new(&env));
        env.storage().persistent().extend_ttl(
            &PersistentKey::ReputationScores,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().instance().set(&DataKey::RewardPool, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::TotalParticipations, &0u32);
        env.storage().instance().set(
            &DataKey::MemberParticipation,
            &Map::<Address, u32>::new(&env),
        );
        env.storage()
            .instance()
            .set(&DataKey::ClaimedRewards, &Map::<Address, i128>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::RewardWeights, &Map::<Address, u32>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::RewardDistType, &DistributionType::Equal);

        env.storage()
            .instance()
            .set(&DataKey::ExitPenaltyBps, &config.exit_penalty_bps);
        // Temporary: ExitRequests are short-lived pending-admin state — auto-expire when unused
        env.storage().temporary().set(
            &DataKey2::ExitRequests,
            &Map::<Address, ExitRequest>::new(&env),
        );
        env.storage().temporary().extend_ttl(
            &DataKey2::ExitRequests,
            TEMP_LIFETIME_THRESHOLD,
            TEMP_BUMP_AMOUNT,
        );
        env.storage()
            .instance()
            .set(&DataKey::ExitedMembers, &Vec::<Address>::new(&env));
        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::IsPaused, &false);
        env.storage().instance().set(
            &DataKey::MemberContributions,
            &Map::<Address, i128>::new(&env),
        );

        // Protocol Fee Configuration
        env.storage()
            .instance()
            .set(&DataKey::FeeBps, &config.fee_bps);
        if let Some(recipient) = config.fee_recipient {
            env.storage()
                .instance()
                .set(&DataKey::FeeRecipient, &recipient);
        }

        // Suspension Threshold Configuration
        env.storage()
            .instance()
            .set(&DataKey::MaxDefaults, &config.max_defaults);
        events::emit_suspension_threshold_set(&env, config.max_defaults);
        env.storage()
            .instance()
            .set(&DataKey2::GracePeriodLedgers, &config.grace_period_ledgers);
        env.storage()
            .instance()
            .set(&DataKey2::PendingPenalties, &Map::<Address, u32>::new(&env));

        env.storage()
            .instance()
            .set(&DataKey::MaxMembers, &max_members);

        // Timestamp-based Payout Scheduling
        env.storage()
            .instance()
            .set(&DataKey::UseTimestampSchedule, &config.use_timestamp_schedule);
        env.storage()
            .instance()
            .set(&DataKey::RoundDurationSeconds, &config.round_duration_seconds);

        if config.use_timestamp_schedule {
            let timestamp_deadline = resolved_start_at + config.round_duration_seconds;
            env.storage()
                .instance()
                .set(&DataKey::RoundDeadlineTimestamp, &timestamp_deadline);
            events::emit_round_deadline_timestamp_set(&env, 0, timestamp_deadline);
        }

        // Savings Goal Initialization
        if let Some(goal) = config.collective_goal {
            env.storage()
                .instance()
                .set(&DataKey::CollectiveGoal, &goal);
        }
        if let Some(goals) = config.member_goals {
            env.storage().instance().set(&DataKey::MemberGoals, &goals);
        }
        env.storage()
            .instance()
            .set(&DataKey::TotalCollected, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::MemberCollected, &Map::<Address, i128>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::MilestonesReached, &Vec::<u32>::new(&env));

        // Skip Functionality Initialization
        env.storage()
            .instance()
            .set(&DataKey2::SkipFee, &config.skip_fee);
        env.storage()
            .instance()
            .set(&DataKey2::MaxSkipsPerCycle, &config.max_skips_per_cycle);
        env.storage()
            .instance()
            .set(&DataKey2::SkipRequests, &Map::<(Address, u32), bool>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey2::MemberSkips, &Map::<(Address, u32), u32>::new(&env));

        // Voting Mode Initialization
        env.storage()
            .instance()
            .set(&DataKey2::VotingMode, &config.voting_mode);

        // Reinvestment Initialization
        env.storage()
            .instance()
            .set(&DataKey2::ReinvestPreference, &Map::<Address, bool>::new(&env));

        // Governance Initialization
        env.storage()
            .instance()
            .set(&DataKey::ProposalCounter, &0u32);
        env.storage()
            .instance()
            .set(&DataKey::Proposals, &Map::<u32, Proposal>::new(&env));
        env.storage().instance().set(
            &DataKey::ProposalVotes,
            &Map::<u32, Map<Address, bool>>::new(&env),
        );
        env.storage()
            .instance()
            .set(&DataKey::VotingDeadline, &0u64);
        env.storage()
            .instance()
            .set(&DataKey::QuorumPercentage, &51u32);

        events::emit_rosc_init(&env, member_count as u32, contribution_amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Propose a new admin address. Only the current admin can propose.
    pub fn propose_admin_transfer(env: Env, proposed_admin: Address) {
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

    /// Returns the configured group start timestamp.
    pub fn get_start_time(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey2::StartAt)
            .unwrap_or(env.ledger().timestamp())
    }

    /// Returns true when group contributions can begin.
    pub fn is_active(env: Env) -> bool {
        let start_at = Self::get_start_time(env.clone());
        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        env.ledger().timestamp() >= start_at && group_status == GroupStatus::Active
    }

    /// Cancel a pending (not-yet-active) group and refund deposited rewards to admin.
    pub fn cancel_pending_group(env: Env, admin: Address) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can cancel pending group");
        }

        let start_at = Self::get_start_time(env.clone());
        if env.ledger().timestamp() >= start_at {
            panic_with_error!(&env, ExtError::GroupAlreadyDissolved);
        }

        let reward_pool: i128 = env
            .storage()
            .instance()
            .get(&DataKey::RewardPool)
            .unwrap_or(0);
        if reward_pool > 0 {
            let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
            let client = token::Client::new(&env, &token_addr);
            client.transfer(&env.current_contract_address(), &admin, &reward_pool);
            env.storage().instance().set(&DataKey::RewardPool, &0i128);
        }

        env.storage()
            .instance()
            .set(&DataKey2::GroupStatus, &GroupStatus::Dissolved);
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

        // Migration logic would go here
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
        internals::check_not_paused(&env);
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
            .set(&DataKey2::TokenWhitelistContract, &whitelist_contract);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the token whitelist contract address
    pub fn get_token_whitelist_contract(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey2::TokenWhitelistContract)
    }

    /// Check if a token is allowed via the whitelist contract
    pub fn is_token_allowed(env: Env, token: Address) -> bool {
        if let Some(whitelist_contract) = env
            .storage()
            .instance()
            .get::<DataKey2, Address>(&DataKey2::TokenWhitelistContract)
        {
            let client = TokenWhitelistClient::new(&env, &whitelist_contract);
            client.is_token_allowed(&token)
        } else {
            // If no whitelist contract is set, allow all tokens (backward compatibility)
            true
        }
    }

    /// Set the contribution tier for a member. Tier changes take effect in the next round.
    pub fn set_member_tier(env: Env, admin: Address, member: Address, tier_bps: u32) {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can set member tier");
        }

        if tier_bps == 0 {
            panic_with_error!(&env, ExtError::InvalidTier);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&member) {
            panic_with_error!(&env, Error::NotAMember);
        }

        let mut tiers: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey::MemberTiers)
            .unwrap_or(Map::new(&env));

        tiers.set(member.clone(), tier_bps);
        env.storage().instance().set(&DataKey::MemberTiers, &tiers);

        events::emit_member_tier_set(&env, member, tier_bps);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn contribute_to_insurance(env: Env, contributor: Address, token: Address, amount: i128) {
        internals::check_not_paused(&env);
        contributor.require_auth();

        if amount <= 0 {
            panic_with_error!(&env, ExtError::InvalidInsuranceContribution);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&contributor) {
            panic_with_error!(&env, Error::NotAMember);
        }

        let exited_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env));
        if exited_members.contains(&contributor) {
            panic_with_error!(&env, Error::MemberHasExited);
        }

        let approved_tokens: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ApprovedTokens)
            .unwrap_or(Vec::new(&env));
        if !approved_tokens.contains(&token) {
            panic_with_error!(&env, Error::TokenNotApproved);
        }

        // Token whitelist validation
        Self::require_token_allowed(&env, &token);

        let client = token::Client::new(&env, &token);
        client.transfer(
            &contributor,
            &env.current_contract_address(),
            &amount,
        );

        let mut insurance_pool: i128 = env
            .storage()
            .instance()
            .get(&DataKey2::InsurancePool)
            .unwrap_or(0);
        insurance_pool += amount;
        env.storage()
            .instance()
            .set(&DataKey2::InsurancePool, &insurance_pool);

        events::emit_insurance_top_up(&env, contributor, amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_insurance_pool(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey2::InsurancePool)
            .unwrap_or(0)
    }

    /// Get the proposed admin address, if any.
    pub fn get_proposed_admin(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::ProposedAdmin)
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

    /// Validates that a token is allowed via the whitelist contract
    fn require_token_allowed(env: &Env, token: &Address) {
        if let Some(whitelist_contract) = env
            .storage()
            .instance()
            .get::<DataKey2, Address>(&DataKey2::TokenWhitelistContract)
        {
            let client = TokenWhitelistClient::new(env, &whitelist_contract);
            if !client.is_token_allowed(token) {
                panic_with_error!(env, Error::TokenNotApproved);
            }
        }
        // If no whitelist contract is set, allow all tokens (backward compatibility)
    }

    pub fn contribute(env: Env, contributor: Address, token: Address, amount: i128) {
        internals::check_not_paused(&env);
        internals::check_not_frozen(&env);
        contributor.require_auth();

        let start_at = Self::get_start_time(env.clone());
        if env.ledger().timestamp() < start_at {
            panic_with_error!(&env, ExtError::GroupNotYetActive);
        }
        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        if group_status == GroupStatus::Dissolved {
            panic_with_error!(&env, ExtError::GroupAlreadyDissolved);
        }

        if amount <= 0 {
            panic_with_error!(&env, Error::AmountMustBePositive);
        }

        let use_timestamp: bool = env
            .storage()
            .instance()
            .get(&DataKey::UseTimestampSchedule)
            .unwrap_or(false);

        let deadline: u64 = if use_timestamp {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadlineTimestamp)
                .expect("Timestamp deadline not set")
        } else {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadline)
                .expect("Deadline not set")
        };

        if env.ledger().timestamp() > deadline {
            panic_with_error!(&env, Error::ContributionWindowClosed);
        }

        let exited_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env));
        if exited_members.contains(&contributor) {
            panic_with_error!(&env, Error::MemberHasExited);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&contributor) {
            panic_with_error!(&env, Error::NotAMember);
        }

        let activation_emitted: bool = env
            .storage()
            .instance()
            .get(&DataKey2::GroupActivationEmitted)
            .unwrap_or(false);

        let mut paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .expect("Not initialized");
        if paid_members.contains(&contributor) {
            panic_with_error!(&env, Error::AlreadyContributed);
        }

        // #218: collect reinstatement fee before first contribution after reinstatement
        {
            let mut pending: Vec<Address> = env.storage().instance().get(&DataKey2::PendingReinstatementFee).unwrap_or(Vec::new(&env));
            if pending.contains(&contributor) {
                let fee: i128 = env.storage().instance().get(&DataKey2::ReinstatementFee).unwrap_or(0);
                if fee > 0 {
                    let fee_token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
                    let fee_client = token::Client::new(&env, &fee_token);
                    fee_client.transfer(&contributor, &env.current_contract_address(), &fee);
                    events::emit_reinstatement_fee_collected(&env, contributor.clone(), fee);
                }
                let mut new_pending: Vec<Address> = Vec::new(&env);
                for m in pending.iter() { if m != contributor { new_pending.push_back(m); } }
                env.storage().instance().set(&DataKey2::PendingReinstatementFee, &new_pending);
            }
        }

        // Validate token
        let approved_tokens: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ApprovedTokens)
            .unwrap_or(Vec::new(&env));
        if !approved_tokens.contains(&token) {
            panic_with_error!(&env, Error::TokenNotApproved);
        }

        // Token whitelist validation
        Self::require_token_allowed(&env, &token);

        let base_token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let base_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ContributionAmt)
            .unwrap();

        // Calculate member-specific required amount based on tier
        let tiers: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey::MemberTiers)
            .unwrap_or(Map::new(&env));
        let tier_bps = tiers.get(contributor.clone()).unwrap_or(10_000); // Default to 1x (10000 bps)
        let member_required_amount = (base_amount * tier_bps as i128) / 10_000;

        let amount_to_transfer = if token == base_token {
            amount  // For base token, transfer the exact amount specified
        } else {
            let rates: Map<Address, i128> = env
                .storage()
                .instance()
                .get(&DataKey::ExchangeRates)
                .unwrap_or(Map::new(&env));
            let rate = rates.get(token.clone()).expect("Exchange rate not set");
            if rate <= 0 {
                panic_with_error!(&env, Error::InvalidExchangeRate);
            }
            // Valuation logic: RequiredAmount = (Amount * 10^7) / Rate
            // Rate is expected to be in 10^7 precision (e.g., 1.5 * 10^7 = 15,000,000)
            (amount * 10_000_000) / rate
        };

        // Check token-specific limits
        let limits: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::TokenLimits)
            .unwrap_or(Map::new(&env));
        if let Some(limit) = limits.get(token.clone()) {
            if amount_to_transfer > limit {
                panic_with_error!(&env, Error::ExceedsTokenLimit);
            }
        }

        // Calculate insurance auto-deduction if configured
        let insurance_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey2::InsuranceContributionBps)
            .unwrap_or(0);
        let insurance_deduction = if insurance_bps > 0 {
            (amount_to_transfer * insurance_bps as i128) / 10_000
        } else {
            0
        };
        let total_transfer_amount = amount_to_transfer + insurance_deduction;

        let client = token::Client::new(&env, &token);
        client.transfer(
            &contributor,
            &env.current_contract_address(),
            &total_transfer_amount,
        );

        // Update insurance pool if auto-deduction was applied
        if insurance_deduction > 0 {
            let mut insurance_pool: i128 = env
                .storage()
                .instance()
                .get(&DataKey2::InsurancePool)
                .unwrap_or(0);
            insurance_pool += insurance_deduction;
            env.storage()
                .instance()
                .set(&DataKey2::InsurancePool, &insurance_pool);
            events::emit_insurance_top_up(&env, contributor.clone(), insurance_deduction);
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        // Load (and update) cumulative contributions for this round
        let mut member_contributions: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::MemberContributions)
            .unwrap_or(Map::new(&env));
        let already_paid: i128 = member_contributions.get(contributor.clone()).unwrap_or(0);
        let remaining = member_required_amount - already_paid;

        if amount > remaining {
            panic_with_error!(&env, Error::ExceedsRemainingContribution);
        }

        let new_total = already_paid + amount;
        member_contributions.set(contributor.clone(), new_total);
        env.storage()
            .instance()
            .set(&DataKey::MemberContributions, &member_contributions);

        events::emit_contrib(
            &env,
            contributor.clone(),
            current_round,
            token,
            amount_to_transfer,
        );

        // Emit partial contribution event if not yet fully paid
        let remaining_after = member_required_amount - new_total;
        if remaining_after > 0 {
            events::emit_partial_contribution(
                &env,
                contributor.clone(),
                current_round,
                amount,
                remaining_after,
            );
        }

        // Only mark as fully paid (and track participation) when target is reached
        if new_total == member_required_amount {
            Self::apply_reputation_delta(&env, contributor.clone(), 10, "on_time_full");
            paid_members.push_back(contributor.clone());
            env.storage()
                .instance()
                .set(&DataKey::PaidMembers, &paid_members);

            // Track reward participation
            let mut total_participations: u32 = env
                .storage()
                .instance()
                .get(&DataKey::TotalParticipations)
                .unwrap_or(0);
            let mut member_participation: Map<Address, u32> = env
                .storage()
                .instance()
                .get(&DataKey::MemberParticipation)
                .unwrap_or(Map::new(&env));

            let current_participation = member_participation.get(contributor.clone()).unwrap_or(0);
            member_participation.set(contributor.clone(), current_participation + 1);
            total_participations += 1;

            env.storage()
                .instance()
                .set(&DataKey::TotalParticipations, &total_participations);
            env.storage()
                .instance()
                .set(&DataKey::MemberParticipation, &member_participation);

            // Only trigger payout when all members have fully contributed
            if new_total == member_required_amount && paid_members.len() == members.len() {
                internals::complete_round_payout(&env, &paid_members);

                // Emit auto-close event if enabled
                let auto_close_enabled: bool = env
                    .storage()
                    .temporary()
                    .get(&Symbol::new(&env, "auto_close_enabled"))
                    .unwrap_or(false);
                if auto_close_enabled {
                    let current_round: u32 = env
                        .storage()
                        .instance()
                        .get(&DataKey::CurrentRound)
                        .unwrap_or(0);
                    events::emit_round_auto_closed_early(&env, current_round, env.ledger().timestamp());
                }
            }

            // Savings Goal Progress Tracking
            let mut total_collected: i128 = env
                .storage()
                .instance()
                .get(&DataKey::TotalCollected)
                .unwrap_or(0);
            total_collected += amount;
            env.storage()
                .instance()
                .set(&DataKey::TotalCollected, &total_collected);

            let mut member_collected: Map<Address, i128> = env
                .storage()
                .instance()
                .get(&DataKey::MemberCollected)
                .unwrap_or(Map::new(&env));
            let m_collected = member_collected.get(contributor.clone()).unwrap_or(0) + amount;
            member_collected.set(contributor.clone(), m_collected);
            env.storage()
                .instance()
                .set(&DataKey::MemberCollected, &member_collected);

            // Milestone Detection
            if let Some(collective_goal) = env
                .storage()
                .instance()
                .get::<_, i128>(&DataKey::CollectiveGoal)
            {
                let mut milestones_reached: Vec<u32> = env
                    .storage()
                    .instance()
                    .get(&DataKey::MilestonesReached)
                    .unwrap_or(Vec::new(&env));

                let progress_bps = (total_collected * 10000i128) / collective_goal;
                let thresholds: [u32; 4] = [2500u32, 5000u32, 7500u32, 10000u32];
                let milestone_names: [u32; 4] = [25u32, 50u32, 75u32, 100u32];

                for i in 0..4 {
                    let threshold = thresholds[i];
                    let milestone = milestone_names[i];
                    if progress_bps >= threshold as i128 && !milestones_reached.contains(&milestone)
                    {
                        milestones_reached.push_back(milestone);
                        events::emit_milestone(&env, milestone, total_collected);
                    }
                }
                env.storage()
                    .instance()
                    .set(&DataKey::MilestonesReached, &milestones_reached);
            }
        }

        if !activation_emitted {
            events::emit_group_activated(&env, start_at);
            env.storage()
                .instance()
                .set(&DataKey2::GroupActivationEmitted, &true);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn request_skip(env: Env, member: Address, round: u32) {
        internals::check_not_paused(&env);
        member.require_auth();

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        if round < current_round {
            panic_with_error!(&env, Error::RoundDeadlinePassed);
        }

        let use_timestamp: bool = env
            .storage()
            .instance()
            .get(&DataKey::UseTimestampSchedule)
            .unwrap_or(false);

        let deadline: u64 = if use_timestamp {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadlineTimestamp)
                .expect("Timestamp deadline not set")
        } else {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadline)
                .expect("Deadline not set")
        };

        // Only allow skip for current round if before deadline
        if round == current_round && env.ledger().timestamp() > deadline {
            panic_with_error!(&env, Error::ContributionWindowClosed);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&member) {
            panic_with_error!(&env, Error::NotAMember);
        }

        let mut skip_requests: Map<(Address, u32), bool> = env
            .storage()
            .instance()
            .get(&DataKey2::SkipRequests)
            .unwrap_or(Map::new(&env));

        if skip_requests.get((member.clone(), round)).unwrap_or(false) {
            panic_with_error!(&env, ExtError::AlreadySkipped);
        }

        // Check if already contributed this round
        if round == current_round {
            let paid_members: Vec<Address> = env
                .storage()
                .instance()
                .get(&DataKey::PaidMembers)
                .expect("Not initialized");
            if paid_members.contains(&member) {
                panic_with_error!(&env, Error::AlreadyContributed);
            }
        }

        let payout_order: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PayoutOrder)
            .expect("Not initialized");
        let cycle_index = round / (payout_order.len() as u32);
        let max_skips: u32 = env
            .storage()
            .instance()
            .get(&DataKey2::MaxSkipsPerCycle)
            .unwrap_or(0);

        let mut member_skips: Map<(Address, u32), u32> = env
            .storage()
            .instance()
            .get(&DataKey2::MemberSkips)
            .unwrap_or(Map::new(&env));

        let current_skips = member_skips.get((member.clone(), cycle_index)).unwrap_or(0);
        if current_skips >= max_skips {
            panic_with_error!(&env, ExtError::SkipLimitReached);
        }

        let skip_fee: i128 = env
            .storage()
            .instance()
            .get(&DataKey2::SkipFee)
            .unwrap_or(0);

        if skip_fee > 0 {
            let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
            let client = token::Client::new(&env, &token_addr);
            client.transfer(&member, &env.current_contract_address(), &skip_fee);
        }

        skip_requests.set((member.clone(), round), true);
        member_skips.set((member.clone(), cycle_index), current_skips + 1);

        env.storage()
            .instance()
            .set(&DataKey2::SkipRequests, &skip_requests);
        env.storage()
            .instance()
            .set(&DataKey2::MemberSkips, &member_skips);

        events::emit_round_skip_requested(&env, member, round, skip_fee);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn close_round(env: Env) {
        internals::check_not_paused(&env);
        internals::check_not_frozen(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        let use_timestamp: bool = env
            .storage()
            .instance()
            .get(&DataKey::UseTimestampSchedule)
            .unwrap_or(false);

        let deadline: u64 = if use_timestamp {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadlineTimestamp)
                .expect("Timestamp deadline not set")
        } else {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadline)
                .unwrap()
        };
        if env.ledger().timestamp() <= deadline {
            panic_with_error!(&env, Error::DeadlineNotPassed);
        }

        let members: Vec<Address> = env.storage().instance().get(&DataKey::Members).unwrap();
        let paid_members: Vec<Address> =
            env.storage().instance().get(&DataKey::PaidMembers).unwrap();
        let exited_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env));

        let skip_requests: Map<(Address, u32), bool> = env
            .storage()
            .instance()
            .get(&DataKey2::SkipRequests)
            .unwrap_or(Map::new(&env));

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap();

        let mut defaulters = Vec::new(&env);
        for member in members.iter() {
            let has_skipped = skip_requests.get((member.clone(), current_round)).unwrap_or(false);
            if !paid_members.contains(&member) && !exited_members.contains(&member) && !has_skipped {
                defaulters.push_back(member);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::Defaulters, &defaulters);

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap();
        events::emit_closed(&env, current_round, defaulters);
        env.storage()
            .instance()
            .set(&DataKey2::LastRoundDeadline, &deadline);

        internals::reset_round_state(&env, current_round);
    }

    /// Finalize a round once its deadline has passed.
    ///
    /// Unlike `close_round` (which only resets state), this function also:
    /// - Identifies non-contributors as delinquent and increments their default count
    /// - Suspends members after 3 consecutive missed rounds
    /// - Executes the payout with whatever funds have been collected
    ///
    /// Admin only. Panics with `DeadlineNotPassed` if called before the deadline.
    pub fn finalize_round(env: Env) {
        internals::check_not_paused(&env);
        internals::check_not_frozen(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();
        Self::process_pending_penalties(&env);

        let use_timestamp: bool = env
            .storage()
            .instance()
            .get(&DataKey::UseTimestampSchedule)
            .unwrap_or(false);

        let deadline: u64 = if use_timestamp {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadlineTimestamp)
                .expect("Timestamp deadline not set")
        } else {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadline)
                .unwrap()
        };
        if env.ledger().timestamp() <= deadline {
            panic_with_error!(&env, Error::DeadlineNotPassed);
        }

        let members: Vec<Address> = env.storage().instance().get(&DataKey::Members).unwrap();
        let paid_members: Vec<Address> =
            env.storage().instance().get(&DataKey::PaidMembers).unwrap();
        let exited_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env));

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);
        let penalty_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::PenaltyAmount)
            .unwrap_or(0);

        let mut default_count: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey::DefaultCount)
            .unwrap_or(Map::new(&env));
        let mut suspended_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::SuspendedMembers)
            .unwrap_or(Vec::new(&env));

        let skip_requests: Map<(Address, u32), bool> = env
            .storage()
            .instance()
            .get(&DataKey2::SkipRequests)
            .unwrap_or(Map::new(&env));

        // Identify defaulters (non-contributors, non-exited, non-skippers)
        let mut defaulters: Vec<Address> = Vec::new(&env);
        for member in members.iter() {
            let has_skipped = skip_requests.get((member.clone(), current_round)).unwrap_or(false);
            if !paid_members.contains(&member) && !exited_members.contains(&member) && !has_skipped {
                defaulters.push_back(member.clone());
            }
        }

        env.storage()
            .instance()
            .set(&DataKey::Defaulters, &defaulters);

        events::emit_round_finalized(&env, current_round, defaulters.clone());
        env.storage()
            .instance()
            .set(&DataKey2::LastRoundDeadline, &deadline);

        // Execute payout BEFORE applying new suspensions so the recipient selection
        // uses the pre-round suspension state (newly delinquent members don't affect
        // this round's payout).
        internals::complete_round_payout(&env, &paid_members);

        // Apply default tracking and suspensions after the payout
        let max_defaults: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxDefaults)
            .unwrap_or(3);

        // #240: co-signer window config
        let co_signer_window: u32 = env
            .storage()
            .instance()
            .get(&DataKey2::CoSignerWindowLedgers)
            .unwrap_or(0);
        let co_signers: Map<Address, CoSignerRecord> = env
            .storage()
            .instance()
            .get(&DataKey2::CoSigners)
            .unwrap_or(Map::new(&env));
        let mut window_starts: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey2::CoSignerWindowStart)
            .unwrap_or(Map::new(&env));

        for member in defaulters.iter() {
            // #240: if member has an active co-signer and window > 0, open grace period
            // instead of immediately applying the penalty
            if co_signer_window > 0 {
                if let Some(record) = co_signers.get(member.clone()) {
                    if record.status == CoSignerStatus::Active {
                        // Open window if not already open
                        if window_starts.get(member.clone()).is_none() {
                            window_starts.set(member.clone(), env.ledger().sequence());
                            env.storage()
                                .instance()
                                .set(&DataKey2::CoSignerWindowStart, &window_starts);
                            // Skip penalty this round — co-signer has a window to act
                            continue;
                        }
                        // Window already open — check if expired
                        let start = window_starts.get(member.clone()).unwrap();
                        if env.ledger().sequence() < start + co_signer_window {
                            // Still within window — skip penalty
                            continue;
                        }
                        // Window expired — clear it and fall through to penalty
                        window_starts.remove(member.clone());
                        env.storage()
                            .instance()
                            .set(&DataKey2::CoSignerWindowStart, &window_starts);
                        events::emit_co_signer_window_expired(&env, 0, member.clone());
                    }
                }
            }

            let count = default_count.get(member.clone()).unwrap_or(0) + 1;
            default_count.set(member.clone(), count);

            events::emit_defaulted(&env, member.clone(), current_round, penalty_amount, count);

            // Suspend after reaching max_defaults consecutive missed rounds
            if count >= max_defaults && !suspended_members.contains(&member) {
                suspended_members.push_back(member.clone());
                events::emit_suspended(&env, member.clone(), count);
                Self::try_promote_from_waitlist(&env, &member);
            }
        }

        env.storage()
            .instance()
            .set(&DataKey::DefaultCount, &default_count);
        env.storage()
            .instance()
            .set(&DataKey::SuspendedMembers, &suspended_members);

        // ── #224: Cycle completion bonus ──────────────────────────────────────
        // A cycle ends when (current_round + 1) is a multiple of payout_order.len().
        let payout_order: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PayoutOrder)
            .unwrap_or(Vec::new(&env));
        let cycle_len = payout_order.len() as u32;
        if cycle_len > 0 && (current_round + 1) % cycle_len == 0 {
            let bonus_amount: i128 = env
                .storage()
                .instance()
                .get(&DataKey2::CycleBonusAmount)
                .unwrap_or(0);
            if bonus_amount > 0 {
                let cycle_number = (current_round + 1) / cycle_len;
                let cycle_start = (cycle_number - 1) * cycle_len;
                let mut qualifying: Vec<Address> = Vec::new(&env);
                for member in members.iter() {
                    if exited_members.contains(&member) { continue; }
                    let defaults = default_count.get(member.clone()).unwrap_or(0);
                    let mut had_skip = false;
                    for r in cycle_start..=(current_round) {
                        if skip_requests.get((member.clone(), r)).unwrap_or(false) {
                            had_skip = true;
                            break;
                        }
                    }
                    if defaults == 0 && !had_skip {
                        qualifying.push_back(member);
                    }
                }
                let q_count = qualifying.len() as i128;
                if q_count > 0 {
                    let total_needed = bonus_amount * q_count;
                    let mut reward_pool: i128 = env
                        .storage()
                        .instance()
                        .get(&DataKey::RewardPool)
                        .unwrap_or(0);
                    let actual_bonus = if reward_pool >= total_needed {
                        bonus_amount
                    } else if reward_pool > 0 {
                        let prorated = reward_pool / q_count;
                        let shortfall = total_needed - reward_pool;
                        events::emit_cycle_bonus_prorated(&env, cycle_number, shortfall);
                        prorated
                    } else {
                        0
                    };
                    if actual_bonus > 0 {
                        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
                        let token_client = token::Client::new(&env, &token_addr);
                        for member in qualifying.iter() {
                            token_client.transfer(
                                &env.current_contract_address(),
                                &member,
                                &actual_bonus,
                            );
                            reward_pool -= actual_bonus;
                            events::emit_cycle_bonus_paid(&env, member, actual_bonus, cycle_number);
                        }
                        env.storage().instance().set(&DataKey::RewardPool, &reward_pool);
                    }
                }
            }
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // ─── #224: Cycle Completion Bonus ────────────────────────────────────────

    /// Admin sets the per-member cycle completion bonus drawn from the reward pool.
    pub fn set_cycle_bonus(env: Env, admin: Address, amount: i128) {
        internals::check_not_paused(&env);
        admin.require_auth();
        let a: Address = env.storage().instance().get(&DataKey::Admin).expect("No admin");
        if admin != a { panic_with_error!(&env, Error::OnlyAdminAllowed); }
        if amount < 0 { panic_with_error!(&env, Error::AmountMustBePositive); }
        env.storage().instance().set(&DataKey2::CycleBonusAmount, &amount);
        events::emit_cycle_bonus_configured(&env, amount);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns the configured cycle bonus amount (0 if not set).
    pub fn get_cycle_bonus(env: Env) -> i128 {
        env.storage().instance().get(&DataKey2::CycleBonusAmount).unwrap_or(0)
    }

    // ─── #227: Round Duration Update ─────────────────────────────────────────

    /// Admin schedules a round duration change that takes effect from the next round.
    /// `new_duration_seconds` must be within [min_round_duration, max_round_duration].
    pub fn update_round_duration(env: Env, admin: Address, new_duration_seconds: u64) {
        internals::check_not_paused(&env);
        admin.require_auth();
        let a: Address = env.storage().instance().get(&DataKey::Admin).expect("No admin");
        if admin != a { panic_with_error!(&env, Error::OnlyAdminAllowed); }

        let min_dur: u64 = env.storage().instance().get(&DataKey2::MinRoundDuration).unwrap_or(60);
        let max_dur: u64 = env.storage().instance().get(&DataKey2::MaxRoundDuration).unwrap_or(u64::MAX);
        if new_duration_seconds < min_dur || new_duration_seconds > max_dur {
            panic_with_error!(&env, Error::RoundDurationOutOfBounds);
        }

        let old_duration: u64 = env.storage().instance().get(&DataKey::RoundDuration).unwrap_or(0);
        let current_round: u32 = env.storage().instance().get(&DataKey::CurrentRound).unwrap_or(0);

        env.storage().instance().set(&DataKey2::PendingRoundDuration, &new_duration_seconds);
        events::emit_round_duration_update_scheduled(&env, old_duration, new_duration_seconds, current_round + 1);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin configures the min/max bounds for round duration.
    pub fn set_round_duration_bounds(env: Env, admin: Address, min_seconds: u64, max_seconds: u64) {
        internals::check_not_paused(&env);
        admin.require_auth();
        let a: Address = env.storage().instance().get(&DataKey::Admin).expect("No admin");
        if admin != a { panic_with_error!(&env, Error::OnlyAdminAllowed); }
        if min_seconds == 0 || min_seconds > max_seconds { panic_with_error!(&env, Error::InvalidAmount); }
        env.storage().instance().set(&DataKey2::MinRoundDuration, &min_seconds);
        env.storage().instance().set(&DataKey2::MaxRoundDuration, &max_seconds);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();
        Self::process_pending_penalties(&env);

        let penalty_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::PenaltyAmount)
            .unwrap_or(0);
        if penalty_amount == 0 {
            panic_with_error!(&env, Error::PenaltyDisabled);
        }

        let defaulters: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Defaulters)
            .unwrap_or(Vec::new(&env));
        if !defaulters.contains(&member) {
            panic_with_error!(&env, Error::NotADefaulter);
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);
        let round_deadline: u64 = env
            .storage()
            .instance()
            .get(&DataKey2::LastRoundDeadline)
            .or(env.storage().instance().get(&DataKey::RoundDeadline))
            .unwrap_or(0);
        let grace_period_ledgers: u32 = env
            .storage()
            .instance()
            .get(&DataKey2::GracePeriodLedgers)
            .unwrap_or(0);
        let grace_expires_at = round_deadline.saturating_add(grace_period_ledgers as u64);
        let current_ledger = env.ledger().timestamp();
        if current_ledger <= grace_expires_at {
            let mut pending_penalties: Map<Address, u32> = env
                .storage()
                .instance()
                .get(&DataKey2::PendingPenalties)
                .unwrap_or(Map::new(&env));
            pending_penalties.set(member.clone(), current_round);
            env.storage()
                .instance()
                .set(&DataKey2::PendingPenalties, &pending_penalties);
            events::emit_grace_period_warning(
                &env,
                member,
                current_round,
                grace_expires_at,
            );
            return;
        }

        let mut pending_penalties: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey2::PendingPenalties)
            .unwrap_or(Map::new(&env));
        pending_penalties.remove(member.clone());
        env.storage()
            .instance()
            .set(&DataKey2::PendingPenalties, &pending_penalties);

        Self::apply_penalty(&env, member, penalty_amount, current_round);
    }

    fn process_pending_penalties(env: &Env) {
        let mut pending_penalties: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey2::PendingPenalties)
            .unwrap_or(Map::new(env));
        if pending_penalties.len() == 0 {
            return;
        }

        let penalty_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::PenaltyAmount)
            .unwrap_or(0);
        if penalty_amount == 0 {
            pending_penalties = Map::new(env);
            env.storage()
                .instance()
                .set(&DataKey2::PendingPenalties, &pending_penalties);
            return;
        }

        let grace_period_ledgers: u32 = env
            .storage()
            .instance()
            .get(&DataKey2::GracePeriodLedgers)
            .unwrap_or(0);
        let round_deadline: u64 = env
            .storage()
            .instance()
            .get(&DataKey2::LastRoundDeadline)
            .or(env.storage().instance().get(&DataKey::RoundDeadline))
            .unwrap_or(0);
        let grace_expires_at = round_deadline.saturating_add(grace_period_ledgers as u64);
        let current_ledger = env.ledger().timestamp();
        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        let mut still_pending: Map<Address, u32> = Map::new(env);
        for (member, pending_round) in pending_penalties.iter() {
            if current_ledger > grace_expires_at || current_round > pending_round {
                Self::apply_penalty(env, member, penalty_amount, current_round);
            } else {
                still_pending.set(member, pending_round);
            }
        }

        env.storage()
            .instance()
            .set(&DataKey2::PendingPenalties, &still_pending);
    }

    fn apply_penalty(env: &Env, member: Address, penalty_amount: i128, round: u32) {
        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(env, &token_addr);

        member.require_auth();
        client.transfer(&member, &env.current_contract_address(), &penalty_amount);

        let mut default_count: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey::DefaultCount)
            .unwrap_or(Map::new(env));
        let current_defaults = default_count.get(member.clone()).unwrap_or(0);
        let new_default_count = current_defaults + 1;
        default_count.set(member.clone(), new_default_count);
        env.storage()
            .instance()
            .set(&DataKey::DefaultCount, &default_count);

        events::emit_defaulted(
            env,
            member.clone(),
            round,
            penalty_amount,
            new_default_count,
        );
        // Confirmed default is applied here (not when merely pending).
        Self::apply_reputation_delta(env, member.clone(), -20, "defaulted");
        // Late-but-paid: member settled after defaulting.
        Self::apply_reputation_delta(env, member.clone(), 5, "late_paid");

        let max_defaults: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxDefaults)
            .unwrap_or(3);

        if new_default_count >= max_defaults {
            let mut suspended_members: Vec<Address> = env
                .storage()
                .instance()
                .get(&DataKey::SuspendedMembers)
                .unwrap_or(Vec::new(env));
            if !suspended_members.contains(&member) {
                suspended_members.push_back(member.clone());
                env.storage()
                    .instance()
                    .set(&DataKey::SuspendedMembers, &suspended_members);
                events::emit_suspended(env, member.clone(), new_default_count);
                Self::try_promote_from_waitlist(env, &member);
            }
        }
    }

    fn apply_reputation_delta(env: &Env, member: Address, delta: i128, reason: &str) {
        let mut scores: Map<Address, i128> = env
            .storage()
            .persistent()
            .get(&PersistentKey::ReputationScores)
            .unwrap_or(Map::new(env));
        let old_score = scores.get(member.clone()).unwrap_or(0);
        let mut new_score = old_score + delta;
        if new_score < 0 {
            new_score = 0;
        }
        scores.set(member.clone(), new_score);
        env.storage()
            .persistent()
            .set(&PersistentKey::ReputationScores, &scores);
        env.storage().persistent().extend_ttl(
            &PersistentKey::ReputationScores,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        events::emit_reputation_updated(
            env,
            member,
            old_score,
            new_score,
            Symbol::new(env, reason),
        );
    }

    pub fn get_reputation_score(env: Env, member: Address) -> i128 {
        let scores: Map<Address, i128> = env
            .storage()
            .persistent()
            .get(&PersistentKey::ReputationScores)
            .unwrap_or(Map::new(&env));
        env.storage().persistent().extend_ttl(
            &PersistentKey::ReputationScores,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        scores.get(member).unwrap_or(0)
    }

    pub fn get_group_avg_reputation(env: Env) -> i128 {
        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .unwrap_or(Vec::new(&env));
        if members.is_empty() {
            return 0;
        }
        let scores: Map<Address, i128> = env
            .storage()
            .persistent()
            .get(&PersistentKey::ReputationScores)
            .unwrap_or(Map::new(&env));
        let mut total = 0i128;
        for member in members.iter() {
            total += scores.get(member).unwrap_or(0);
        }
        env.storage().persistent().extend_ttl(
            &PersistentKey::ReputationScores,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        total / (members.len() as i128)
    }

    pub fn add_member(env: Env, new_member: Address) {
        internals::check_not_paused(&env);
        internals::check_not_frozen(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        // Reject mid-round: paid_members must be empty
        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        if !paid_members.is_empty() {
            panic_with_error!(&env, Error::CannotChangeMidRound);
        }

        let mut members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");

        let max_members: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxMembers)
            .unwrap_or(50);

        if (members.len() as u32) >= max_members {
            panic_with_error!(&env, Error::GroupFull);
        }

        if members.contains(&new_member) {
            panic_with_error!(&env, Error::AlreadyAMember);
        }
        members.push_back(new_member.clone());
        env.storage().instance().set(&DataKey::Members, &members);

        // Recalculate payout order: append new member to the end
        let mut payout_order: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PayoutOrder)
            .expect("Payout order not set");
        payout_order.push_back(new_member.clone());
        env.storage()
            .instance()
            .set(&DataKey::PayoutOrder, &payout_order);

        events::emit_mem_add(&env, new_member, members.len() as u32);
    }

    pub fn remove_member(env: Env, member: Address) {
        internals::check_not_paused(&env);
        internals::check_not_frozen(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        // Reject mid-round: paid_members must be empty
        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        if !paid_members.is_empty() {
            panic_with_error!(&env, Error::CannotChangeMidRound);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&member) {
            panic_with_error!(&env, Error::NotAMember);
        }

        // Remove from members list
        let mut new_members: Vec<Address> = Vec::new(&env);
        for m in members.iter() {
            if m != member {
                new_members.push_back(m);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::Members, &new_members);

        // Recalculate payout order: filter out the member
        let old_order: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PayoutOrder)
            .expect("Payout order not set");
        let mut new_order: Vec<Address> = Vec::new(&env);
        for m in old_order.iter() {
            if m != member {
                new_order.push_back(m);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::PayoutOrder, &new_order);

        events::emit_mem_rmv(&env, member, new_members.len() as u32);
    }

    pub fn add_approved_token(env: Env, token: Address) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        let mut approved_tokens: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ApprovedTokens)
            .unwrap_or(Vec::new(&env));

        if !approved_tokens.contains(&token) {
            approved_tokens.push_back(token.clone());
            env.storage()
                .instance()
                .set(&DataKey::ApprovedTokens, &approved_tokens);
            events::emit_tok_add(&env, token);
        }
    }

    pub fn remove_approved_token(env: Env, token: Address) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        let approved_tokens: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ApprovedTokens)
            .unwrap_or(Vec::new(&env));

        if approved_tokens.contains(&token) {
            let mut new_approved_tokens: Vec<Address> = Vec::new(&env);
            for t in approved_tokens.iter() {
                if t != token {
                    new_approved_tokens.push_back(t);
                }
            }
            env.storage()
                .instance()
                .set(&DataKey::ApprovedTokens, &new_approved_tokens);
            events::emit_tok_rmv(&env, token);
        }
    }

    pub fn set_exchange_rate(env: Env, token: Address, rate: i128) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        let mut rates: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::ExchangeRates)
            .unwrap_or(Map::new(&env));

        rates.set(token.clone(), rate);
        env.storage()
            .instance()
            .set(&DataKey::ExchangeRates, &rates);
        events::emit_rate_set(&env, token, rate);
    }

    pub fn set_token_limit(env: Env, token: Address, limit: i128) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        let mut limits: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::TokenLimits)
            .unwrap_or(Map::new(&env));

        limits.set(token.clone(), limit);
        env.storage().instance().set(&DataKey::TokenLimits, &limits);
        events::emit_lim_set(&env, token, limit);
    }

    pub fn bump_storage(env: Env) {
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn deposit_rewards(env: Env, depositor: Address, amount: i128) {
        internals::check_not_paused(&env);
        depositor.require_auth();

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);

        client.transfer(&depositor, &env.current_contract_address(), &amount);

        let mut reward_pool: i128 = env
            .storage()
            .instance()
            .get(&DataKey::RewardPool)
            .unwrap_or(0);
        reward_pool += amount;
        env.storage()
            .instance()
            .set(&DataKey::RewardPool, &reward_pool);

        events::emit_rew_dep(&env, depositor, amount);
    }

    pub fn set_reward_dist_params(
        env: Env,
        dist_type: DistributionType,
        weights: Option<Map<Address, u32>>,
    ) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        env.storage()
            .instance()
            .set(&DataKey::RewardDistType, &dist_type);

        if let Some(w) = weights {
            env.storage().instance().set(&DataKey::RewardWeights, &w);
        }

        events::emit_rew_cfg(&env, dist_type);
    }

    pub fn claim_rewards(env: Env, member: Address) {
        internals::check_not_paused(&env);
        member.require_auth();

        let claimable = Self::get_claimable_reward(env.clone(), member.clone());
        if claimable <= 0 {
            panic_with_error!(&env, Error::NoRewardsToClaim);
        }

        let mut claimed_rewards: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::ClaimedRewards)
            .unwrap_or(Map::new(&env));
        let total_claimed = claimed_rewards.get(member.clone()).unwrap_or(0);
        claimed_rewards.set(member.clone(), total_claimed + claimable);
        env.storage()
            .instance()
            .set(&DataKey::ClaimedRewards, &claimed_rewards);

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);

        client.transfer(&env.current_contract_address(), &member, &claimable);

        events::emit_rew_clm(&env, member, claimable);
    }

    pub fn get_claimable_reward(env: Env, member: Address) -> i128 {
        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&member) {
            return 0;
        }

        let reward_pool: i128 = env
            .storage()
            .instance()
            .get(&DataKey::RewardPool)
            .unwrap_or(0);
        if reward_pool == 0 {
            return 0;
        }

        let dist_type: DistributionType = env
            .storage()
            .instance()
            .get(&DataKey::RewardDistType)
            .unwrap_or(DistributionType::Equal);

        let share = match dist_type {
            DistributionType::Equal => reward_pool / (members.len() as i128),
            DistributionType::Proportional => {
                let total_participations: u32 = env
                    .storage()
                    .instance()
                    .get(&DataKey::TotalParticipations)
                    .unwrap_or(0);
                if total_participations == 0 {
                    0
                } else {
                    let member_participation: Map<Address, u32> = env
                        .storage()
                        .instance()
                        .get(&DataKey::MemberParticipation)
                        .unwrap_or(Map::new(&env));
                    let count = member_participation.get(member.clone()).unwrap_or(0);
                    (reward_pool * (count as i128)) / (total_participations as i128)
                }
            }
            DistributionType::Weighted => {
                let weights: Map<Address, u32> = env
                    .storage()
                    .instance()
                    .get(&DataKey::RewardWeights)
                    .unwrap_or(Map::new(&env));
                let total_weight: u32 = {
                    let mut sum = 0u32;
                    for w in weights.values().iter() {
                        sum += w;
                    }
                    sum
                };
                if total_weight == 0 {
                    reward_pool / (members.len() as i128) // Fallback to equal
                } else {
                    let weight = weights.get(member.clone()).unwrap_or(0);
                    (reward_pool * (weight as i128)) / (total_weight as i128)
                }
            }
        };

        let claimed_rewards: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::ClaimedRewards)
            .unwrap_or(Map::new(&env));
        let already_claimed = claimed_rewards.get(member).unwrap_or(0);

        share - already_claimed
    }

    // --- GOVERNANCE FUNCTIONS ---

    pub fn create_proposal(
        env: Env,
        creator: Address,
        proposal_type: ProposalType,
        description: soroban_sdk::String,
        target_member: Address,
        voting_duration: u64,
        execution_data: Option<i128>,
    ) {
        internals::check_not_paused(&env);
        creator.require_auth();

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&creator) {
            panic_with_error!(&env, Error::OnlyMembersAllowed);
        }

        let current_time = env.ledger().timestamp();
        let deadline = current_time + voting_duration;

        let mut proposal_counter: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ProposalCounter)
            .unwrap_or(0);
        let proposal_id = proposal_counter;
        proposal_counter += 1;

        let quorum_config: Map<ProposalType, u32> = env
            .storage()
            .instance()
            .get(&DataKey2::QuorumConfig)
            .unwrap_or(Map::new(&env));

        let required_quorum = if let Some(q) = quorum_config.get(proposal_type) {
            q
        } else {
            let global_q: u32 = env
                .storage()
                .instance()
                .get(&DataKey::QuorumPercentage)
                .unwrap_or(51);
            global_q * 100 // Convert % to bps
        };

        let proposal = Proposal {
            id: proposal_id,
            proposal_type,
            creator: creator.clone(),
            description,
            target_member: target_member.clone(),
            votes_for: 0,
            votes_against: 0,
            created_at: current_time,
            deadline,
            status: ProposalStatus::Pending,
            execution_data,
            required_quorum,
        };

        let mut proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&DataKey::Proposals)
            .unwrap_or(Map::new(&env));
        proposals.set(proposal_id, proposal.clone());
        env.storage()
            .instance()
            .set(&DataKey::Proposals, &proposals);

        let mut proposal_votes: Map<u32, Map<Address, bool>> = env
            .storage()
            .instance()
            .get(&DataKey::ProposalVotes)
            .unwrap_or(Map::new(&env));
        proposal_votes.set(proposal_id, Map::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::ProposalVotes, &proposal_votes);

        env.storage()
            .instance()
            .set(&DataKey::ProposalCounter, &proposal_counter);

        events::emit_prop_new(
            &env,
            proposal_id,
            creator,
            target_member,
            current_time,
            deadline,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_member_voting_weight(env: Env, member: Address) -> i128 {
        let voting_mode: VotingMode = env
            .storage()
            .instance()
            .get(&DataKey2::VotingMode)
            .unwrap_or(VotingMode::Equal);

        match voting_mode {
            VotingMode::Equal => 1i128,
            VotingMode::WeightedByContributions => {
                let contributions: Map<Address, i128> = env
                    .storage()
                    .instance()
                    .get(&DataKey::MemberContributions)
                    .unwrap_or(Map::new(&env));
                contributions.get(member).unwrap_or(0)
            }
        }
    }

    /// Set a member's preference for auto-reinvesting payouts into the next round.
    /// Preference can be toggled anytime before the current round's contribution deadline.
    pub fn set_reinvest_preference(env: Env, member: Address, reinvest: bool) {
        internals::check_not_paused(&env);
        member.require_auth();

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&member) {
            panic_with_error!(&env, Error::NotAMember);
        }

        let use_timestamp: bool = env
            .storage()
            .instance()
            .get(&DataKey::UseTimestampSchedule)
            .unwrap_or(false);

        let deadline: u64 = if use_timestamp {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadlineTimestamp)
                .expect("Timestamp deadline not set")
        } else {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadline)
                .expect("Deadline not set")
        };

        if env.ledger().timestamp() > deadline {
            panic_with_error!(&env, Error::ContributionWindowClosed);
        }

        let mut preferences: Map<Address, bool> = env
            .storage()
            .instance()
            .get(&DataKey2::ReinvestPreference)
            .unwrap_or(Map::new(&env));

        preferences.set(member, reinvest);
        env.storage()
            .instance()
            .set(&DataKey2::ReinvestPreference, &preferences);

        env.storage()
             .instance()
             .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
     }

    pub fn get_reinvest_preference(env: Env, member: Address) -> bool {
        let preferences: Map<Address, bool> = env
            .storage()
            .instance()
            .get(&DataKey2::ReinvestPreference)
            .unwrap_or(Map::new(&env));
        preferences.get(member).unwrap_or(false)
    }

     pub fn vote_on_proposal(env: Env, voter: Address, proposal_id: u32, vote_for: bool) {
        internals::check_not_paused(&env);
        voter.require_auth();

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&voter) {
            panic_with_error!(&env, Error::OnlyMembersAllowed);
        }

        // Check if voter has an active delegation
        let delegations: Map<Address, Address> = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "vote_delegations"))
            .unwrap_or(Map::new(&env));
        if delegations.contains_key(voter.clone()) {
            panic_with_error!(&env, Error::CannotVoteWithActiveDelegation);
        }

        let mut proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&DataKey::Proposals)
            .unwrap_or(Map::new(&env));
        if !proposals.contains_key(proposal_id) {
            panic_with_error!(&env, Error::ProposalNotFound);
        }

        let mut proposal = proposals.get(proposal_id).unwrap();
        let current_time = env.ledger().timestamp();
        if current_time > proposal.deadline {
            panic_with_error!(&env, Error::VotingDeadlinePassed);
        }
        if proposal.status != ProposalStatus::Pending {
            panic_with_error!(&env, Error::ProposalNotPending);
        }

        let mut proposal_votes: Map<u32, Map<Address, bool>> = env
            .storage()
            .instance()
            .get(&DataKey::ProposalVotes)
            .unwrap_or(Map::new(&env));
        let mut votes = proposal_votes.get(proposal_id).unwrap_or(Map::new(&env));

        if votes.contains_key(voter.clone()) {
            panic_with_error!(&env, Error::AlreadyVoted);
        }

        votes.set(voter.clone(), vote_for);
        proposal_votes.set(proposal_id, votes.clone()); // cloned for delegation loop

        let voter_weight = Self::get_member_voting_weight(env.clone(), voter.clone());
        if voter_weight == 0 {
            let voting_mode: VotingMode = env.storage().instance().get(&DataKey2::VotingMode).unwrap_or(VotingMode::Equal);
            if voting_mode == VotingMode::WeightedByContributions {
                panic_with_error!(&env, ExtError::InsufficientWeight);
            }
        }

        if vote_for {
            proposal.votes_for += voter_weight;
        } else {
            proposal.votes_against += voter_weight;
        }

        // Count votes from delegators
        let mut delegator_votes_for = 0i128;
        let mut delegator_votes_against = 0i128;
        for (delegator, delegate) in delegations.iter() {
            if delegate == voter {
                // This voter is a delegate; check if delegator hasn't voted yet
                let delegator_voted = votes.contains_key(delegator.clone());
                if !delegator_voted {
                    let delegator_weight = Self::get_member_voting_weight(env.clone(), delegator.clone());
                    if vote_for {
                        delegator_votes_for += delegator_weight;
                    } else {
                        delegator_votes_against += delegator_weight;
                    }
                    // Mark delegator as voted
                    votes.set(delegator.clone(), vote_for);
                }
            }
        }

        proposal.votes_for += delegator_votes_for;
        proposal.votes_against += delegator_votes_against;

        proposal_votes.set(proposal_id, votes);
        proposals.set(proposal_id, proposal);
        env.storage()
            .instance()
            .set(&DataKey::Proposals, &proposals);
        env.storage()
            .instance()
            .set(&DataKey::ProposalVotes, &proposal_votes);

        let voting_mode: VotingMode = env
            .storage()
            .instance()
            .get(&DataKey2::VotingMode)
            .unwrap_or(VotingMode::Equal);
        
        if voting_mode == VotingMode::WeightedByContributions {
            events::emit_weighted_vote_cast(&env, voter, proposal_id, voter_weight);
        } else {
            events::emit_voted(&env, proposal_id, voter, vote_for);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn execute_proposal(env: Env, proposal_id: u32) {
        internals::check_not_paused(&env);

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");

        let mut proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&DataKey::Proposals)
            .unwrap_or(Map::new(&env));
        if !proposals.contains_key(proposal_id) {
            panic_with_error!(&env, Error::ProposalNotFound);
        }

        let mut proposal = proposals.get(proposal_id).unwrap();
        let current_time = env.ledger().timestamp();

        if proposal.status != ProposalStatus::Pending {
            panic_with_error!(&env, Error::ProposalNotPending);
        }

        if current_time <= proposal.deadline {
            panic_with_error!(&env, Error::VotingNotEnded);
        }

        let voting_mode: VotingMode = env
            .storage()
            .instance()
            .get(&DataKey2::VotingMode)
            .unwrap_or(VotingMode::Equal);

        let total_votes = proposal.votes_for + proposal.votes_against;

        let total_possible_votes = match voting_mode {
            VotingMode::Equal => members.len() as i128,
            VotingMode::WeightedByContributions => {
                let contributions: Map<Address, i128> = env
                    .storage()
                    .instance()
                    .get(&DataKey::MemberContributions)
                    .unwrap_or(Map::new(&env));
                let mut total = 0i128;
                for member in members.iter() {
                    total += contributions.get(member).unwrap_or(0);
                }
                total
            }
        };

        let required_votes = ((total_possible_votes * proposal.required_quorum as i128) + 9999) / 10000;

        if total_votes < required_votes {
            proposal.status = ProposalStatus::Rejected;
            proposals.set(proposal_id, proposal.clone());
            env.storage()
                .instance()
                .set(&DataKey::Proposals, &proposals);
            events::emit_prop_rej(
                &env,
                proposal_id,
                Symbol::new(&env, "insufficient_quorum"),
                total_votes,
                required_votes,
            );
            return;
        }

        if proposal.votes_for <= proposal.votes_against {
            proposal.status = ProposalStatus::Rejected;
            proposals.set(proposal_id, proposal.clone());
            env.storage()
                .instance()
                .set(&DataKey::Proposals, &proposals);
            events::emit_prop_rej(
                &env,
                proposal_id,
                Symbol::new(&env, "votes_failed"),
                proposal.votes_for,
                proposal.votes_against,
            );
            return;
        }

        proposal.status = ProposalStatus::Approved;

        match proposal.proposal_type {
            ProposalType::PenaltyAppeal => {
                internals::execute_penalty_appeal(&env, &proposal.target_member);
            }
            ProposalType::RuleChange => {
                internals::execute_rule_change(&env, proposal.execution_data);
            }
            ProposalType::MemberRemoval => {
                internals::execute_member_removal(&env, &proposal.target_member);
            }
            ProposalType::MaxMembersUpdate => {
                internals::execute_max_members_update(&env, proposal.execution_data);
            }
            // #218: lift suspension, reset defaults, re-append to payout order
            ProposalType::Reinstatement => {
                let target = proposal.target_member.clone();
                let mut suspended: Vec<Address> = env.storage().instance().get(&DataKey::SuspendedMembers).unwrap_or(Vec::new(&env));
                let mut ns: Vec<Address> = Vec::new(&env);
                for m in suspended.iter() { if m != target { ns.push_back(m); } }
                env.storage().instance().set(&DataKey::SuspendedMembers, &ns);
                let mut dc: Map<Address, u32> = env.storage().instance().get(&DataKey::DefaultCount).unwrap_or(Map::new(&env));
                dc.set(target.clone(), 0);
                env.storage().instance().set(&DataKey::DefaultCount, &dc);
                let mut po: Vec<Address> = env.storage().instance().get(&DataKey::PayoutOrder).unwrap_or(Vec::new(&env));
                if !po.contains(&target) { po.push_back(target.clone()); env.storage().instance().set(&DataKey::PayoutOrder, &po); }
                let fee: i128 = env.storage().instance().get(&DataKey2::ReinstatementFee).unwrap_or(0);
                if fee > 0 {
                    let mut pf: Vec<Address> = env.storage().instance().get(&DataKey2::PendingReinstatementFee).unwrap_or(Vec::new(&env));
                    if !pf.contains(&target) { pf.push_back(target.clone()); env.storage().instance().set(&DataKey2::PendingReinstatementFee, &pf); }
                }
                let mut am: Map<Address, u32> = env.storage().instance().get(&DataKey2::ActiveReinstatementProposal).unwrap_or(Map::new(&env));
                am.remove(target.clone());
                env.storage().instance().set(&DataKey2::ActiveReinstatementProposal, &am);
                events::emit_reinstatement_approved(&env, target);
            }
        }

        proposal.status = ProposalStatus::Executed;
        proposals.set(proposal_id, proposal.clone());
        env.storage()
            .instance()
            .set(&DataKey::Proposals, &proposals);

        events::emit_prop_exec(
            &env,
            proposal_id,
            proposal.proposal_type as u32,
            proposal.target_member.clone(),
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn set_quorum_per_type(
        env: Env,
        admin: Address,
        proposal_type: ProposalType,
        quorum_bps: u32,
    ) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can set quorum per type");
        }

        if quorum_bps < 100 || quorum_bps > 10000 {
            panic!("Quorum must be between 1% and 100%");
        }

        let mut quorum_config: Map<ProposalType, u32> = env
            .storage()
            .instance()
            .get(&DataKey2::QuorumConfig)
            .unwrap_or(Map::new(&env));

        quorum_config.set(proposal_type, quorum_bps);
        env.storage()
            .instance()
            .set(&DataKey2::QuorumConfig, &quorum_config);

        events::emit_quorum_config_updated(&env, proposal_type, quorum_bps);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- EMERGENCY PAYOUT FUNCTIONS ---

    /// Configure emergency payout settings. Admin only.
    pub fn set_emergency_payout_config(
        env: Env,
        admin: Address,
        emergency_quorum_bps: u32,
        vote_window_seconds: u64,
        max_emergency_per_cycle: u32,
    ) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can set emergency payout config");
        }

        if emergency_quorum_bps < 1000 || emergency_quorum_bps > 10000 {
            panic_with_error!(&env, ExtError::InvalidEmergencyConfig);
        }
        if vote_window_seconds == 0 {
            panic_with_error!(&env, ExtError::InvalidEmergencyConfig);
        }
        if max_emergency_per_cycle == 0 {
            panic_with_error!(&env, ExtError::InvalidEmergencyConfig);
        }

        let config = EmergencyPayoutConfig {
            emergency_quorum_bps,
            vote_window_seconds,
            max_emergency_per_cycle,
        };
        env.storage()
            .instance()
            .set(&DataKey2::EmergencyPayoutConfig, &config);

        events::emit_emergency_payout_config_updated(
            &env,
            emergency_quorum_bps,
            vote_window_seconds,
            max_emergency_per_cycle,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Request an emergency payout. Member must be in good standing.
    pub fn request_emergency_payout(env: Env, member: Address, reason_hash: BytesN<32>) {
        internals::check_not_paused(&env);
        member.require_auth();

        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        if group_status == GroupStatus::Dissolved {
            panic_with_error!(&env, ExtError::GroupAlreadyDissolved);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&member) {
            panic_with_error!(&env, Error::NotAMember);
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        // Check if already requested
        let requests: Map<(u32, Address), EmergencyPayoutRequest> = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutRequests)
            .unwrap_or(Map::new(&env));
        if requests.contains_key((current_round, member.clone())) {
            panic_with_error!(&env, ExtError::EmergencyPayoutRequested);
        }

        // Check if already executed in this cycle
        let approved: Map<(u32, Address), bool> = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutApproved)
            .unwrap_or(Map::new(&env));
        let payout_order: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PayoutOrder)
            .expect("Not initialized");
        let cycle_index = current_round / (payout_order.len() as u32);
        if approved.get((cycle_index, member.clone())).unwrap_or(false) {
            panic_with_error!(&env, ExtError::EmergencyPayoutAlreadyExecuted);
        }

        // Check max emergency payouts per cycle
        let emergency_count: Map<u32, u32> = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutCount)
            .unwrap_or(Map::new(&env));
        let current_count = emergency_count.get(cycle_index).unwrap_or(0);
        let config: EmergencyPayoutConfig = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutConfig)
            .unwrap_or(EmergencyPayoutConfig {
                emergency_quorum_bps: 6667, // default 66.67%
                vote_window_seconds: 7 * 24 * 60 * 60, // default 7 days
                max_emergency_per_cycle: 1,
            });
        if current_count >= config.max_emergency_per_cycle {
            panic_with_error!(&env, ExtError::EmergencyPayoutLimitReached);
        }

        let now = env.ledger().timestamp();
        let deadline = now + config.vote_window_seconds;

        let request = EmergencyPayoutRequest {
            requester: member.clone(),
            reason_hash: reason_hash.clone(),
            created_at: now,
            deadline,
            votes_for: 0,
            votes_against: 0,
            executed: false,
        };

        let mut new_requests = requests;
        new_requests.set((current_round, member.clone()), request);
        env.storage()
            .instance()
            .set(&DataKey2::EmergencyPayoutRequests, &new_requests);

        events::emit_emergency_payout_requested(&env, member, current_round, reason_hash, deadline);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Vote on an emergency payout request.
    pub fn vote_emergency_payout(env: Env, voter: Address, requester: Address, approve: bool) {
        internals::check_not_paused(&env);
        voter.require_auth();

        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        if group_status == GroupStatus::Dissolved {
            panic_with_error!(&env, ExtError::GroupAlreadyDissolved);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&voter) {
            panic_with_error!(&env, Error::OnlyMembersAllowed);
        }
        if voter == requester {
            panic!("Cannot vote on your own emergency payout request");
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        let mut requests: Map<(u32, Address), EmergencyPayoutRequest> = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutRequests)
            .unwrap_or(Map::new(&env));
        if !requests.contains_key((current_round, requester.clone())) {
            panic!("Emergency payout request not found");
        }

        let mut request = requests.get((current_round, requester.clone())).unwrap();
        if request.executed {
            panic!("Emergency payout already executed");
        }

        let now = env.ledger().timestamp();
        if now > request.deadline {
            panic_with_error!(&env, ExtError::EmergencyPayoutVoteExpired);
        }

        // Check if voter already voted
        let votes: Map<(u32, Address, Address), bool> = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutVotes)
            .unwrap_or(Map::new(&env));
        if votes.get((current_round, requester.clone(), voter.clone())).unwrap_or(false) {
            panic!("Already voted on this emergency payout request");
        }

        // Record vote
        let mut new_votes = votes;
        new_votes.set((current_round, requester.clone(), voter.clone()), true);
        env.storage()
            .instance()
            .set(&DataKey2::EmergencyPayoutVotes, &new_votes);

        // Update vote counts
        let voter_weight = Self::get_member_voting_weight(env.clone(), voter.clone());
        if approve {
            request.votes_for += voter_weight;
        } else {
            request.votes_against += voter_weight;
        }
        requests.set((current_round, requester.clone()), request.clone());
        env.storage()
            .instance()
            .set(&DataKey2::EmergencyPayoutRequests, &requests);

        events::emit_emergency_payout_vote_cast(
            &env,
            requester.clone(),
            current_round,
            voter,
            approve,
            request.votes_for,
            request.votes_against,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Execute an approved emergency payout.
    pub fn execute_emergency_payout(env: Env, requester: Address) {
        internals::check_not_paused(&env);

        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        if group_status == GroupStatus::Dissolved {
            panic_with_error!(&env, ExtError::GroupAlreadyDissolved);
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        let mut requests: Map<(u32, Address), EmergencyPayoutRequest> = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutRequests)
            .unwrap_or(Map::new(&env));
        if !requests.contains_key((current_round, requester.clone())) {
            panic!("Emergency payout request not found");
        }

        let mut request = requests.get((current_round, requester.clone())).unwrap();
        if request.executed {
            panic_with_error!(&env, ExtError::EmergencyPayoutAlreadyExecuted);
        }

        let now = env.ledger().timestamp();
        if now > request.deadline {
            panic_with_error!(&env, ExtError::EmergencyPayoutVoteExpired);
        }

        // Check quorum
        let config: EmergencyPayoutConfig = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutConfig)
            .unwrap_or(EmergencyPayoutConfig {
                emergency_quorum_bps: 6667,
                vote_window_seconds: 7 * 24 * 60 * 60,
                max_emergency_per_cycle: 1,
            });

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        let voting_mode: VotingMode = env
            .storage()
            .instance()
            .get(&DataKey2::VotingMode)
            .unwrap_or(VotingMode::Equal);

        let total_possible_votes = match voting_mode {
            VotingMode::Equal => members.len() as i128,
            VotingMode::WeightedByContributions => {
                let contributions: Map<Address, i128> = env
                    .storage()
                    .instance()
                    .get(&DataKey::MemberContributions)
                    .unwrap_or(Map::new(&env));
                let mut total = 0i128;
                for member in members.iter() {
                    total += contributions.get(member).unwrap_or(0);
                }
                total
            }
        };

        let required_votes = ((total_possible_votes * config.emergency_quorum_bps as i128) + 9999) / 10000;
        let total_votes = request.votes_for + request.votes_against;

        if total_votes < required_votes {
            panic_with_error!(&env, ExtError::EmergencyPayoutQuorumNotMet);
        }

        if request.votes_for <= request.votes_against {
            events::emit_emergency_payout_rejected(
                &env,
                requester.clone(),
                current_round,
                Symbol::new(&env, "votes_failed"),
            );
            return;
        }

        // Execute the emergency payout
        request.executed = true;
        requests.set((current_round, requester.clone()), request);
        env.storage()
            .instance()
            .set(&DataKey2::EmergencyPayoutRequests, &requests);

        // Mark as approved for this cycle
        let payout_order: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PayoutOrder)
            .expect("Not initialized");
        let cycle_index = current_round / (payout_order.len() as u32);
        let mut approved: Map<(u32, Address), bool> = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutApproved)
            .unwrap_or(Map::new(&env));
        approved.set((cycle_index, requester.clone()), true);
        env.storage()
            .instance()
            .set(&DataKey2::EmergencyPayoutApproved, &approved);

        // Increment emergency count for this cycle
        let mut emergency_count: Map<u32, u32> = env
            .storage()
            .instance()
            .get(&DataKey2::EmergencyPayoutCount)
            .unwrap_or(Map::new(&env));
        let current_count = emergency_count.get(cycle_index).unwrap_or(0);
        emergency_count.set(cycle_index, current_count + 1);
        env.storage()
            .instance()
            .set(&DataKey2::EmergencyPayoutCount, &emergency_count);

        // Calculate payout amount (full contribution amount)
        let contribution_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ContributionAmt)
            .unwrap_or(0);
        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();

        // Transfer funds to requester
        let client = token::Client::new(&env, &token_addr);
        client.transfer(&env.current_contract_address(), &requester, &contribution_amount);

        // Mark requester as paid for this round
        let mut paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .expect("Not initialized");
        if !paid_members.contains(&requester) {
            paid_members.push_back(requester.clone());
            env.storage()
                .instance()
                .set(&DataKey::PaidMembers, &paid_members);
        }

        events::emit_emergency_payout_executed(&env, requester.clone(), current_round, contribution_amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- GROUP DISSOLUTION FUNCTIONS ---

    /// Configure dissolution settings. Admin only.
    pub fn set_dissolution_config(
        env: Env,
        admin: Address,
        dissolution_quorum_bps: u32,
        vote_window_seconds: u64,
    ) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can set dissolution config");
        }

        if dissolution_quorum_bps < 1000 || dissolution_quorum_bps > 10000 {
            panic_with_error!(&env, ExtError::InvalidDissolutionConfig);
        }
        if vote_window_seconds == 0 {
            panic_with_error!(&env, ExtError::InvalidDissolutionConfig);
        }

        let config = DissolutionConfig {
            dissolution_quorum_bps,
            vote_window_seconds,
        };
        env.storage()
            .instance()
            .set(&DataKey2::DissolutionConfig, &config);

        events::emit_dissolution_config_updated(&env, dissolution_quorum_bps, vote_window_seconds);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin initiates group dissolution.
    pub fn dissolve_group(env: Env, admin: Address, reason_hash: BytesN<32>) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can dissolve group");
        }

        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        if group_status == GroupStatus::Dissolved {
            panic_with_error!(&env, ExtError::GroupAlreadyDissolved);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");

        // Calculate total pool
        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);
        let total_pool = client.balance(&env.current_contract_address());

        if total_pool <= 0 {
            panic_with_error!(&env, ExtError::NoFundsToDistribute);
        }

        // Get member contributions for pro-rata distribution
        let member_collected: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::MemberCollected)
            .unwrap_or(Map::new(&env));

        // Mark group as dissolved
        env.storage()
            .instance()
            .set(&DataKey2::GroupStatus, &GroupStatus::Dissolved);

        // Distribute funds pro-rata
        let mut total_contributions: i128 = 0;
        for member in members.iter() {
            total_contributions += member_collected.get(member.clone()).unwrap_or(0);
        }

        if total_contributions > 0 {
            for member in members.iter() {
                let contribution = member_collected.get(member.clone()).unwrap_or(0);
                let share = (contribution * total_pool) / total_contributions;
                if share > 0 {
                    client.transfer(&env.current_contract_address(), &member, &share);
                    events::emit_member_refunded(&env, member.clone(), share, contribution, total_pool);
                }
            }
        }

        // Handle rounding dust - send to fee recipient or first member
        let remaining = client.balance(&env.current_contract_address());
        if remaining > 0 {
            if let Some(fee_recipient) = env
                .storage()
                .instance()
                .get::<DataKey, Address>(&DataKey::FeeRecipient)
            {
                client.transfer(&env.current_contract_address(), &fee_recipient, &remaining);
            } else if let Some(first_member) = members.get(0) {
                client.transfer(&env.current_contract_address(), &first_member, &remaining);
            }
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        events::emit_group_dissolved(&env, current_round, reason_hash, total_pool, members.len() as u32);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Start a dissolution vote (member-initiated).
    pub fn start_dissolution_vote(env: Env, member: Address) {
        internals::check_not_paused(&env);
        member.require_auth();

        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        if group_status == GroupStatus::Dissolved {
            panic_with_error!(&env, ExtError::GroupAlreadyDissolved);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&member) {
            panic_with_error!(&env, Error::NotAMember);
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        // Check if vote already in progress
        let dissolution_deadlines: Map<u32, u64> = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionDeadline)
            .unwrap_or(Map::new(&env));
        let deadline: u64 = dissolution_deadlines.get(current_round).unwrap_or(0);
        if deadline > 0 && env.ledger().timestamp() < deadline {
            panic_with_error!(&env, ExtError::DissolutionVoteInProgress);
        }

        let config: DissolutionConfig = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionConfig)
            .unwrap_or(DissolutionConfig {
                dissolution_quorum_bps: 7500, // default 75%
                vote_window_seconds: 14 * 24 * 60 * 60, // default 14 days
            });

        let new_deadline = env.ledger().timestamp() + config.vote_window_seconds;
        let mut new_deadlines: Map<u32, u64> = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionDeadline)
            .unwrap_or(Map::new(&env));
        new_deadlines.set(current_round, new_deadline);
        env.storage()
            .instance()
            .set(&DataKey2::DissolutionDeadline, &new_deadlines);

        let mut vote_counts: Map<u32, i128> = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionVoteCount)
            .unwrap_or(Map::new(&env));
        vote_counts.set(current_round, 0);
        env.storage()
            .instance()
            .set(&DataKey2::DissolutionVoteCount, &vote_counts);

        events::emit_dissolution_vote_started(&env, current_round, new_deadline);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Vote on dissolution.
    pub fn vote_dissolve_group(env: Env, voter: Address, approve: bool) {
        internals::check_not_paused(&env);
        voter.require_auth();

        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        if group_status == GroupStatus::Dissolved {
            panic_with_error!(&env, ExtError::GroupAlreadyDissolved);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&voter) {
            panic_with_error!(&env, Error::OnlyMembersAllowed);
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        let dissolution_deadlines: Map<u32, u64> = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionDeadline)
            .unwrap_or(Map::new(&env));
        let deadline: u64 = dissolution_deadlines.get(current_round).unwrap_or(0);
        if deadline == 0 {
            panic!("No dissolution vote in progress");
        }

        if env.ledger().timestamp() > deadline {
            panic_with_error!(&env, ExtError::DissolutionVoteExpired);
        }

        // Check if already voted
        let votes: Map<(u32, Address), bool> = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionVotes)
            .unwrap_or(Map::new(&env));
        if votes.get((current_round, voter.clone())).unwrap_or(false) {
            panic!("Already voted on dissolution");
        }

        // Record vote
        let mut new_votes = votes;
        new_votes.set((current_round, voter.clone()), true);
        env.storage()
            .instance()
            .set(&DataKey2::DissolutionVotes, &new_votes);

        // Update vote count
        let voter_weight = Self::get_member_voting_weight(env.clone(), voter.clone());
        let mut vote_counts: Map<u32, i128> = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionVoteCount)
            .unwrap_or(Map::new(&env));
        let mut votes_for: i128 = vote_counts.get(current_round).unwrap_or(0);
        if approve {
            votes_for += voter_weight;
        }
        vote_counts.set(current_round, votes_for);
        env.storage()
            .instance()
            .set(&DataKey2::DissolutionVoteCount, &vote_counts);

        events::emit_dissolution_vote_cast(&env, current_round, voter, approve, votes_for);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Execute dissolution if quorum is met.
    pub fn execute_dissolution(env: Env) {
        internals::check_not_paused(&env);

        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        if group_status == GroupStatus::Dissolved {
            panic_with_error!(&env, ExtError::GroupAlreadyDissolved);
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        let dissolution_deadlines: Map<u32, u64> = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionDeadline)
            .unwrap_or(Map::new(&env));
        let deadline: u64 = dissolution_deadlines.get(current_round).unwrap_or(0);
        if deadline == 0 {
            panic!("No dissolution vote in progress");
        }

        if env.ledger().timestamp() <= deadline {
            panic!("Voting period not ended");
        }

        let config: DissolutionConfig = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionConfig)
            .unwrap_or(DissolutionConfig {
                dissolution_quorum_bps: 7500,
                vote_window_seconds: 14 * 24 * 60 * 60,
            });

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        let voting_mode: VotingMode = env
            .storage()
            .instance()
            .get(&DataKey2::VotingMode)
            .unwrap_or(VotingMode::Equal);

        let total_possible_votes = match voting_mode {
            VotingMode::Equal => members.len() as i128,
            VotingMode::WeightedByContributions => {
                let contributions: Map<Address, i128> = env
                    .storage()
                    .instance()
                    .get(&DataKey::MemberContributions)
                    .unwrap_or(Map::new(&env));
                let mut total = 0i128;
                for member in members.iter() {
                    total += contributions.get(member).unwrap_or(0);
                }
                total
            }
        };

        let vote_counts: Map<u32, i128> = env
            .storage()
            .instance()
            .get(&DataKey2::DissolutionVoteCount)
            .unwrap_or(Map::new(&env));
        let votes_for: i128 = vote_counts.get(current_round).unwrap_or(0);

        let required_votes = ((total_possible_votes * config.dissolution_quorum_bps as i128) + 9999) / 10000;

        if votes_for < required_votes {
            panic_with_error!(&env, ExtError::DissolutionQuorumNotMet);
        }

        events::emit_dissolution_quorum_reached(&env, current_round, votes_for);

        // Execute dissolution with empty reason hash
        let reason_hash = BytesN::<32>::from_array(&env, &[0u8; 32]);
        Self::dissolve_group(
            env.clone(),
            env.storage()
                .instance()
                .get(&DataKey::Admin)
                .expect("Not initialized"),
            reason_hash,
        );
    }

    // --- READ INTERFACE ---

    pub fn get_group_info(env: Env) -> GroupInfo {
        let members: Vec<Address> = env.storage().instance().get(&DataKey::Members).unwrap();
        let payout_order: Vec<Address> =
            env.storage().instance().get(&DataKey::PayoutOrder).unwrap();
        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        let recipient_idx = (current_round % payout_order.len()) as u32;
        let next_recipient = payout_order.get(recipient_idx).unwrap();

        GroupInfo {
            members,
            contribution_amount: env
                .storage()
                .instance()
                .get(&DataKey::ContributionAmt)
                .unwrap_or(0),
            token: env.storage().instance().get(&DataKey::Token).unwrap(),
            current_round,
            total_rounds: payout_order.len(),
            paid_members: env
                .storage()
                .instance()
                .get(&DataKey::PaidMembers)
                .unwrap_or(Vec::new(&env)),
            next_recipient,
            round_deadline: {
                let use_timestamp: bool = env
                    .storage()
                    .instance()
                    .get(&DataKey::UseTimestampSchedule)
                    .unwrap_or(false);
                if use_timestamp {
                    env.storage()
                        .instance()
                        .get(&DataKey::RoundDeadlineTimestamp)
                        .unwrap_or(0)
                } else {
                    env.storage()
                        .instance()
                        .get(&DataKey::RoundDeadline)
                        .unwrap_or(0)
                }
            },
        }
    }

    pub fn get_member_status(env: Env, member: Address) -> MemberStatus {
        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .unwrap_or(Vec::new(&env));
        let is_member = members.contains(&member);

        let suspended_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::SuspendedMembers)
            .unwrap_or(Vec::new(&env));
        let is_suspended = suspended_members.contains(&member);

        let exited_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env));
        let is_exited = exited_members.contains(&member);

        let member_contributions: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::MemberContributions)
            .unwrap_or(Map::new(&env));
        let contributions_this_round = member_contributions.get(member.clone()).unwrap_or(0);

        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        let has_paid_this_round = paid_members.contains(&member);

        let default_count_map: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey::DefaultCount)
            .unwrap_or(Map::new(&env));
        let default_count = default_count_map.get(member.clone()).unwrap_or(0);

        let member_collected: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::MemberCollected)
            .unwrap_or(Map::new(&env));
        let lifetime_contributions = member_collected.get(member.clone()).unwrap_or(0);

        let claimable_rewards = Self::get_claimable_reward(env.clone(), member.clone());

        MemberStatus {
            is_member,
            is_suspended,
            is_exited,
            contributions_this_round,
            has_paid_this_round,
            default_count,
            lifetime_contributions,
            claimable_rewards,
        }
    }

    /// Returns `(amount_contributed_so_far, amount_remaining)` for `member`
    /// in the current round.
    pub fn get_member_contribution_status(env: Env, member: Address) -> (i128, i128) {
        let target: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ContributionAmt)
            .unwrap_or(0);
        let member_contributions: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::MemberContributions)
            .unwrap_or(Map::new(&env));
        let contributed = member_contributions.get(member).unwrap_or(0);
        let remaining = target - contributed;
        (contributed, remaining)
    }

    pub fn get_round_history(env: Env) -> Vec<PayoutRecord> {
        env.storage()
            .persistent()
            .get(&PersistentKey::RoundHistory)
            .unwrap_or(Vec::new(&env))
    }

    pub fn get_state(env: Env) -> (u32, Vec<Address>, u64, PayoutStrategy, Address) {
        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);
        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));

        let use_timestamp: bool = env
            .storage()
            .instance()
            .get(&DataKey::UseTimestampSchedule)
            .unwrap_or(false);

        let deadline: u64 = if use_timestamp {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadlineTimestamp)
                .unwrap_or(0)
        } else {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadline)
                .unwrap_or(0)
        };

        let strategy: PayoutStrategy = env
            .storage()
            .instance()
            .get(&DataKey::Strategy)
            .unwrap_or(PayoutStrategy::RoundRobin);
        let token: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        (current_round, paid_members, deadline, strategy, token)
    }

    pub fn emit_deadline_reminder(env: Env, interval: Symbol) {
        internals::check_not_paused(&env);

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);
        let use_timestamp: bool = env
            .storage()
            .instance()
            .get(&DataKey::UseTimestampSchedule)
            .unwrap_or(false);

        let deadline: u64 = if use_timestamp {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadlineTimestamp)
                .unwrap_or(0)
        } else {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadline)
                .unwrap_or(0)
        };
        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        let exited_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env));

        let current_time = env.ledger().timestamp();
        let time_remaining = if deadline > current_time {
            deadline - current_time
        } else {
            0
        };

        let mut non_contributors = Vec::new(&env);
        for member in members.iter() {
            if !paid_members.contains(&member) && !exited_members.contains(&member) {
                non_contributors.push_back(member);
            }
        }

        events::emit_reminder(
            &env,
            current_round,
            time_remaining,
            non_contributors,
            interval,
        );
    }

    pub fn get_upcoming_deadlines(env: Env, count: u32) -> Map<u32, u64> {
        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);
        let use_timestamp: bool = env
            .storage()
            .instance()
            .get(&DataKey::UseTimestampSchedule)
            .unwrap_or(false);

        let current_deadline: u64 = if use_timestamp {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadlineTimestamp)
                .unwrap_or(0)
        } else {
            env.storage()
                .instance()
                .get(&DataKey::RoundDeadline)
                .unwrap_or(0)
        };

        let round_duration: u64 = if use_timestamp {
            env.storage()
                .instance()
                .get(&DataKey::RoundDurationSeconds)
                .unwrap_or(0)
        } else {
            env.storage()
                .instance()
                .get(&DataKey::RoundDuration)
                .unwrap_or(0)
        };

        let mut deadlines = Map::new(&env);
        for i in 0..count {
            let round = current_round + i;
            let deadline = if i == 0 {
                current_deadline
            } else {
                current_deadline + (i as u64 * round_duration)
            };
            deadlines.set(round, deadline);
        }
        deadlines
    }

    pub fn get_next_deadline_timestamp(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::RoundDeadlineTimestamp)
            .unwrap_or(0)
    }

    pub fn get_savings_progress(env: Env, member: Option<Address>) -> (i128, i128, i128, i128) {
        let total_collected = env
            .storage()
            .instance()
            .get(&DataKey::TotalCollected)
            .unwrap_or(0);
        let collective_goal = env
            .storage()
            .instance()
            .get(&DataKey::CollectiveGoal)
            .unwrap_or(0);

        let (member_collected, member_goal) = if let Some(m) = member {
            let m_collected = env
                .storage()
                .instance()
                .get::<_, Map<Address, i128>>(&DataKey::MemberCollected)
                .unwrap_or(Map::new(&env))
                .get(m.clone())
                .unwrap_or(0);
            let m_goal = env
                .storage()
                .instance()
                .get::<_, Map<Address, i128>>(&DataKey::MemberGoals)
                .unwrap_or(Map::new(&env))
                .get(m)
                .unwrap_or(0);
            (m_collected, m_goal)
        } else {
            (0, 0)
        };

        (
            total_collected,
            collective_goal,
            member_collected,
            member_goal,
        )
    }

    pub fn get_exchange_rates(env: Env) -> Map<Address, i128> {
        env.storage()
            .instance()
            .get(&DataKey::ExchangeRates)
            .unwrap_or(Map::new(&env))
    }

    pub fn get_token_limits(env: Env) -> Map<Address, i128> {
        env.storage()
            .instance()
            .get(&DataKey::TokenLimits)
            .unwrap_or(Map::new(&env))
    }

    pub fn get_approved_tokens(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&DataKey::ApprovedTokens)
            .unwrap_or(Vec::new(&env))
    }

    pub fn get_proposal(env: Env, proposal_id: u32) -> Option<Proposal> {
        let proposals: Map<u32, Proposal> = env
            .storage()
            .instance()
            .get(&DataKey::Proposals)
            .unwrap_or(Map::new(&env));
        proposals.get(proposal_id)
    }

    pub fn get_proposal_counter(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::ProposalCounter)
            .unwrap_or(0)
    }

    pub fn get_member_vote(env: Env, proposal_id: u32, member: Address) -> bool {
        let proposal_votes: Map<u32, Map<Address, bool>> = env
            .storage()
            .instance()
            .get(&DataKey::ProposalVotes)
            .unwrap_or(Map::new(&env));
        if let Some(votes) = proposal_votes.get(proposal_id) {
            votes.get(member).unwrap_or(false)
        } else {
            false
        }
    }

    pub fn get_quorum_percentage(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::QuorumPercentage)
            .unwrap_or(51)
    }

    /// Update the protocol fee configuration. Admin only.
    /// Fee is capped at 500 bps (5%).
    pub fn update_fee(env: Env, new_fee_bps: u32) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        if new_fee_bps > 500 {
            panic_with_error!(&env, Error::FeeExceedsMaximum);
        }

        env.storage()
            .instance()
            .set(&DataKey::FeeBps, &new_fee_bps);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the current protocol fee in basis points.
    pub fn get_fee_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::FeeBps)
            .unwrap_or(0)
    }

    /// Get the protocol fee recipient address.
    pub fn get_fee_recipient(env: Env) -> Option<Address> {
        env.storage()
            .instance()
            .get(&DataKey::FeeRecipient)
    }

    /// Get the maximum number of consecutive defaults before suspension.
    pub fn get_max_defaults(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MaxDefaults)
            .unwrap_or(3)
    }

    /// Update the maximum member limit. Admin-only.
    /// Cannot decrease below current member count.
    /// new_max must be between 1 and 100.
    pub fn update_max_members(env: Env, new_max: u32) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        if new_max < 1 || new_max > 100 {
            panic_with_error!(&env, Error::InvalidMaxMembers);
        }

        let current_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .unwrap_or(Vec::new(&env));

        if new_max < current_members.len() as u32 {
            panic_with_error!(&env, Error::InvalidMaxMembers);
        }

        let old_max: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxMembers)
            .unwrap_or(50);

        env.storage()
            .instance()
            .set(&DataKey::MaxMembers, &new_max);

        events::emit_max_members_upd(&env, old_max, new_max);
    }

    /// Get the current maximum member limit.
    pub fn get_max_members(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::MaxMembers)
            .unwrap_or(50)
    }

    // --- EMERGENCY EXIT ---

    pub fn pause_group(env: Env, reason: soroban_sdk::String) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        if Self::is_paused(env.clone()) {
            panic_with_error!(&env, Error::AlreadyPaused);
        }

        env.storage().instance().set(&DataKey::Paused, &true);
        env.storage().instance().set(&DataKey::IsPaused, &true);
        env.storage().instance().set(&DataKey::PauseReason, &reason);
        env.storage()
            .instance()
            .set(&DataKey::PauseTimestamp, &env.ledger().timestamp());

        events::emit_paused(&env, reason);
    }

    pub fn resume_group(env: Env, reason: soroban_sdk::String) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        if !Self::is_paused(env.clone()) {
            panic_with_error!(&env, Error::NotPaused);
        }

        let pause_timestamp: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PauseTimestamp)
            .unwrap();
        let current_timestamp = env.ledger().timestamp();
        let pause_duration = current_timestamp - pause_timestamp;

        // Extend the round deadline
        let current_deadline: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RoundDeadline)
            .unwrap_or(0);
        if current_deadline > 0 {
            env.storage().instance().set(
                &DataKey::RoundDeadline,
                &(current_deadline + pause_duration),
            );
        }

        // Extend the timestamp-based deadline if enabled
        let current_timestamp_deadline: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RoundDeadlineTimestamp)
            .unwrap_or(0);
        if current_timestamp_deadline > 0 {
            let next_deadline = current_timestamp_deadline + pause_duration;
            env.storage().instance().set(
                &DataKey::RoundDeadlineTimestamp,
                &next_deadline,
            );
            let current_round: u32 = env
                .storage()
                .instance()
                .get(&DataKey::CurrentRound)
                .unwrap_or(0);
            events::emit_round_deadline_timestamp_set(&env, current_round, next_deadline);
        }

        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().set(&DataKey::IsPaused, &false);

        // Clean up Reason and Timestamp to save storage space
        env.storage().instance().remove(&DataKey::PauseReason);
        env.storage().instance().remove(&DataKey::PauseTimestamp);

        events::emit_resumed(&env, reason);
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .or(env.storage().instance().get(&DataKey::IsPaused))
            .unwrap_or(false)
    }

    pub fn get_pause_reason(env: Env) -> soroban_sdk::String {
        env.storage()
            .instance()
            .get(&DataKey::PauseReason)
            .unwrap_or(soroban_sdk::String::from_str(&env, ""))
    }

    pub fn pause_contract(env: Env, admin: Address, reason: soroban_sdk::String) {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            panic!("Only admin can pause contract");
        }
        admin.require_auth();
        Self::pause_group(env, reason);
    }

    pub fn resume_contract(env: Env, admin: Address) {
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            panic!("Only admin can resume contract");
        }
        admin.require_auth();
        Self::resume_group(env.clone(), soroban_sdk::String::from_str(&env, "Resumed"));
    }

    pub fn get_pause_info(env: Env) -> (bool, soroban_sdk::String, u64) {
        let is_paused = Self::is_paused(env.clone());
        let reason: soroban_sdk::String = env
            .storage()
            .instance()
            .get(&DataKey::PauseReason)
            .unwrap_or(soroban_sdk::String::from_str(&env, ""));
        let timestamp: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PauseTimestamp)
            .unwrap_or(0);
        (is_paused, reason, timestamp)
    }

    pub fn request_emergency_exit(env: Env, member: Address) {
        internals::check_not_paused(&env);
        member.require_auth();

        let exited_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env));
        if exited_members.contains(&member) {
            panic_with_error!(&env, Error::MemberAlreadyExited);
        }

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&member) {
            panic_with_error!(&env, Error::NotAMember);
        }

        // Prevent exit mid-round
        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        if !paid_members.is_empty() {
            panic_with_error!(&env, Error::ExitNotAllowedMidRound);
        }

        // Check no existing pending request
        let mut requests: Map<Address, ExitRequest> = env
            .storage()
            .temporary()
            .get(&DataKey2::ExitRequests)
            .unwrap_or(Map::new(&env));
        if requests.contains_key(member.clone()) {
            panic_with_error!(&env, Error::ExitRequestPending);
        }

        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);

        // penalty_amount and refund_amount are computed dynamically in approve_exit
        // based on round history and current exit_penalty_bps — not pre-calculated here.
        let request = ExitRequest {
            member: member.clone(),
            rounds_contributed: current_round,
            refund_amount: 0,
            approved: false,
        };
        requests.set(member.clone(), request);
        env.storage()
            .temporary()
            .set(&DataKey2::ExitRequests, &requests);
        env.storage().temporary().extend_ttl(
            &DataKey2::ExitRequests,
            TEMP_LIFETIME_THRESHOLD,
            TEMP_BUMP_AMOUNT,
        );

        events::emit_exit_req(&env, member.clone(), current_round);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn approve_exit(env: Env, member: Address) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        let mut requests: Map<Address, ExitRequest> = env
            .storage()
            .temporary()
            .get(&DataKey2::ExitRequests)
            .unwrap_or(Map::new(&env));
        if !requests.contains_key(member.clone()) {
            panic_with_error!(&env, Error::NoExitRequestFound);
        }
        let request = requests.get(member.clone()).unwrap();

        // Compute penalty and refund dynamically based on current state.
        // This ensures members who already received a payout round are penalized on net balance.
        let contribution_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ContributionAmt)
            .unwrap_or(0);
        let exit_penalty_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ExitPenaltyBps)
            .unwrap_or(0);

        let contributed_total = contribution_amount * (request.rounds_contributed as i128);

        // Sum payouts the member has received from round history
        let history: Vec<PayoutRecord> = env
            .storage()
            .persistent()
            .get(&PersistentKey::RoundHistory)
            .unwrap_or(Vec::new(&env));
        let mut received_payout = 0i128;
        for record in history.iter() {
            if record.recipient == member {
                received_payout += record.amount;
            }
        }

        let penalty = contributed_total * (exit_penalty_bps as i128) / 10_000;
        let net = contributed_total - received_payout - penalty;
        let refund_amount = if net > 0 { net } else { 0 };

        if refund_amount > 0 {
            let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
            let client = token::Client::new(&env, &token_addr);
            client.transfer(&env.current_contract_address(), &member, &refund_amount);
        }

        // Remove from Members list
        let old_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .unwrap_or(Vec::new(&env));
        let mut new_members: Vec<Address> = Vec::new(&env);
        for m in old_members.iter() {
            if m != member {
                new_members.push_back(m);
            }
        }
        env.storage()
            .instance()
            .set(&DataKey::Members, &new_members);

        // Add to ExitedMembers
        let mut exited_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env));
        exited_members.push_back(member.clone());
        env.storage()
            .instance()
            .set(&DataKey::ExitedMembers, &exited_members);

        requests.remove(member.clone());
        env.storage()
            .temporary()
            .set(&DataKey2::ExitRequests, &requests);

        events::emit_exit_ok(&env, member.clone(), refund_amount);

        // Auto-promote from waitlist to fill the vacancy
        Self::try_promote_from_waitlist(&env, &member);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn reject_exit(env: Env, member: Address) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        let mut requests: Map<Address, ExitRequest> = env
            .storage()
            .temporary()
            .get(&DataKey2::ExitRequests)
            .unwrap_or(Map::new(&env));
        if !requests.contains_key(member.clone()) {
            panic_with_error!(&env, Error::NoExitRequestFound);
        }

        requests.remove(member.clone());
        env.storage()
            .temporary()
            .set(&DataKey2::ExitRequests, &requests);

        events::emit_exit_no(&env, member.clone());

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_exit_requests(env: Env) -> Map<Address, ExitRequest> {
        env.storage()
            .temporary()
            .get(&DataKey2::ExitRequests)
            .unwrap_or(Map::new(&env))
    }

    pub fn get_exited_members(env: Env) -> Vec<Address> {
        env.storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env))
    }

    // --- FEATURE 1: DELEGATED VOTING FOR GOVERNANCE PROPOSALS ---

    /// Delegate voting power to another member
    pub fn delegate_vote(env: Env, delegator: Address, delegate: Address) {
        internals::check_not_paused(&env);
        delegator.require_auth();

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&delegator) {
            panic_with_error!(&env, Error::NotAMember);
        }
        if !members.contains(&delegate) {
            panic_with_error!(&env, Error::NotAMember);
        }

        // Check for sub-delegation: delegate cannot already be delegating
        let delegations: Map<Address, Address> = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "vote_delegations"))
            .unwrap_or(Map::new(&env));
        if delegations.contains_key(delegate.clone()) {
            panic_with_error!(&env, Error::CannotSubDelegate);
        }

        let mut new_delegations = delegations.clone();
        new_delegations.set(delegator.clone(), delegate.clone());
        env.storage()
            .temporary()
            .set(&Symbol::new(&env, "vote_delegations"), &new_delegations);
        env.storage().temporary().extend_ttl(
            &Symbol::new(&env, "vote_delegations"),
            TEMP_LIFETIME_THRESHOLD,
            TEMP_BUMP_AMOUNT,
        );

        events::emit_vote_delegated(&env, delegator, delegate);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Revoke voting delegation
    pub fn revoke_delegation(env: Env, delegator: Address) {
        internals::check_not_paused(&env);
        delegator.require_auth();

        let mut delegations: Map<Address, Address> = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "vote_delegations"))
            .unwrap_or(Map::new(&env));

        if !delegations.contains_key(delegator.clone()) {
            panic_with_error!(&env, Error::NoDelegationFound);
        }

        delegations.remove(delegator.clone());
        env.storage()
            .temporary()
            .set(&Symbol::new(&env, "vote_delegations"), &delegations);

        events::emit_delegation_revoked(&env, delegator);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the delegate for a delegator (if any)
    pub fn get_vote_delegation(env: Env, delegator: Address) -> Option<Address> {
        let delegations: Map<Address, Address> = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "vote_delegations"))
            .unwrap_or(Map::new(&env));
        delegations.get(delegator)
    }

    // --- FEATURE 2: AUTO-CLOSE ROUND WHEN ALL MEMBERS HAVE CONTRIBUTED ---

    /// Enable or disable auto-close on full contribution
    pub fn set_auto_close_enabled(env: Env, enabled: bool) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        env.storage()
            .temporary()
            .set(&Symbol::new(&env, "auto_close_enabled"), &enabled);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Check if auto-close is enabled
    pub fn is_auto_close_enabled(env: Env) -> bool {
        env.storage()
            .temporary()
            .get(&Symbol::new(&env, "auto_close_enabled"))
            .unwrap_or(false)
    }

    // --- FEATURE 3: INVITATION-BASED MEMBER JOINING WITH INVITE CODES ---

    /// Generate an invite for a new member (admin only)
    pub fn generate_invite(env: Env, invitee: Address) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if members.contains(&invitee) {
            panic_with_error!(&env, Error::AlreadyAMember);
        }

        let mut approved_invitees: Vec<Address> = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "approved_invitees"))
            .unwrap_or(Vec::new(&env));

        if !approved_invitees.contains(&invitee) {
            approved_invitees.push_back(invitee.clone());
            env.storage()
                .temporary()
                .set(&Symbol::new(&env, "approved_invitees"), &approved_invitees);
            env.storage().temporary().extend_ttl(
                &Symbol::new(&env, "approved_invitees"),
                TEMP_LIFETIME_THRESHOLD,
                TEMP_BUMP_AMOUNT,
            );
        }

        events::emit_invite_generated(&env, invitee, env.ledger().timestamp());

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Join the group using an invite (invitee only)
    pub fn join_with_invite(env: Env, invitee: Address) {
        internals::check_not_paused(&env);
        invitee.require_auth();

        let mut approved_invitees: Vec<Address> = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "approved_invitees"))
            .unwrap_or(Vec::new(&env));

        if !approved_invitees.contains(&invitee) {
            panic_with_error!(&env, Error::InviteNotFound);
        }

        // Remove from approved list
        let mut new_approved: Vec<Address> = Vec::new(&env);
        for addr in approved_invitees.iter() {
            if addr != invitee {
                new_approved.push_back(addr);
            }
        }
        env.storage()
            .temporary()
            .set(&Symbol::new(&env, "approved_invitees"), &new_approved);

        // Add member
        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        if !paid_members.is_empty() {
            panic_with_error!(&env, Error::CannotChangeMidRound);
        }

        let mut members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");

        let max_members: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxMembers)
            .unwrap_or(50);

        if (members.len() as u32) >= max_members {
            panic_with_error!(&env, Error::GroupFull);
        }

        if members.contains(&invitee) {
            panic_with_error!(&env, Error::AlreadyAMember);
        }

        members.push_back(invitee.clone());
        env.storage().instance().set(&DataKey::Members, &members);

        let mut payout_order: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PayoutOrder)
            .expect("Payout order not set");
        payout_order.push_back(invitee.clone());
        env.storage()
            .instance()
            .set(&DataKey::PayoutOrder, &payout_order);

        events::emit_invite_redeemed(&env, invitee.clone());
        events::emit_mem_add(&env, invitee, members.len() as u32);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- FEATURE 4: ADMIN MULTI-SIG AUTHORIZATION FOR CRITICAL OPERATIONS ---

    /// Initialize multi-sig configuration (admin only)
    pub fn init_multisig(env: Env, co_admins: Vec<Address>, threshold: u32) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        if threshold < 1 || threshold > (co_admins.len() as u32 + 1) {
            panic!("Invalid multisig threshold");
        }

        env.storage()
            .temporary()
            .set(&Symbol::new(&env, "co_admins"), &co_admins);
        env.storage()
            .temporary()
            .set(&Symbol::new(&env, "multisig_threshold"), &threshold);
        env.storage().temporary().extend_ttl(
            &Symbol::new(&env, "co_admins"),
            TEMP_LIFETIME_THRESHOLD,
            TEMP_BUMP_AMOUNT,
        );
        env.storage().temporary().extend_ttl(
            &Symbol::new(&env, "multisig_threshold"),
            TEMP_LIFETIME_THRESHOLD,
            TEMP_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Propose a critical admin action (remove member, penalize, update fee)
    pub fn propose_admin_action(
        env: Env,
        action_type: u32, // 0: RemoveMember, 1: PenaliseDefaulter, 2: UpdateFee
        target_member: Option<Address>,
        payload: Option<i128>,
    ) {
        internals::check_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        admin.require_auth();

        let threshold: u32 = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "multisig_threshold"))
            .unwrap_or(1);

        // If threshold is 1, execute immediately (single admin)
        if threshold == 1 {
            match action_type {
                0 => {
                    // RemoveMember
                    if let Some(member) = target_member {
                        Self::remove_member(env.clone(), member);
                    }
                }
                1 => {
                    // PenaliseDefaulter
                    if let Some(member) = target_member {
                        Self::penalise_defaulter(env.clone(), member);
                    }
                }
                2 => {
                    // UpdateFee
                    if let Some(fee_bps) = payload {
                        Self::update_fee(env.clone(), fee_bps as u32);
                    }
                }
                _ => panic!("Invalid action type"),
            }
            return;
        }

        // Multi-sig required: emit event for co-admins to approve
        events::emit_admin_action_proposed(
            &env,
            0, // action_id not used in simplified version
            Symbol::new(&env, match action_type {
                0 => "RemoveMember",
                1 => "PenaliseDefaulter",
                2 => "UpdateFee",
                _ => "Unknown",
            }),
            admin,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Approve a pending admin action (co-admin only)
    pub fn approve_admin_action(env: Env, approver: Address, action_type: u32, target_member: Option<Address>, payload: Option<i128>) {
        internals::check_not_paused(&env);
        approver.require_auth();

        let co_admins: Vec<Address> = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "co_admins"))
            .unwrap_or(Vec::new(&env));
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");

        if !co_admins.contains(&approver) && approver != admin {
            panic_with_error!(&env, Error::NotACoAdmin);
        }

        // Execute the action
        match action_type {
            0 => {
                // RemoveMember
                if let Some(member) = target_member {
                    Self::remove_member(env.clone(), member);
                }
            }
            1 => {
                // PenaliseDefaulter
                if let Some(member) = target_member {
                    Self::penalise_defaulter(env.clone(), member);
                }
            }
            2 => {
                // UpdateFee
                if let Some(fee_bps) = payload {
                    Self::update_fee(env.clone(), fee_bps as u32);
                }
            }
            _ => panic!("Invalid action type"),
        }

        events::emit_admin_action_executed(
            &env,
            0,
            Symbol::new(&env, match action_type {
                0 => "RemoveMember",
                1 => "PenaliseDefaulter",
                2 => "UpdateFee",
                _ => "Unknown",
            }),
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // ─── #213: Payout Slot Swap ───────────────────────────────────────────────

    pub fn set_slot_swap_config(env: Env, admin: Address, requires_admin: bool, expiry_seconds: u64) {
        admin.require_auth();
        internals::check_not_paused(&env);
        let a: Address = env.storage().instance().get(&DataKey::Admin).expect("No admin");
        if admin != a { panic_with_error!(&env, ExtError::OnlyAdminAllowed); }
        env.storage().instance().set(&DataKey2::SlotSwapRequiresAdmin, &requires_admin);
        env.storage().instance().set(&DataKey2::SlotSwapExpirySeconds, &expiry_seconds);
    }

    pub fn request_slot_swap(env: Env, initiator: Address, round_a: u32, round_b: u32, counterparty: Address) -> u32 {
        initiator.require_auth();
        internals::check_not_paused(&env);
        let members: Vec<Address> = env.storage().instance().get(&DataKey::Members).expect("Not init");
        if !members.contains(&initiator) || !members.contains(&counterparty) { panic_with_error!(&env, Error::OnlyMembersAllowed); }
        let current_round: u32 = env.storage().instance().get(&DataKey::CurrentRound).unwrap_or(0);
        let payout_order: Vec<Address> = env.storage().instance().get(&DataKey::PayoutOrder).unwrap();
        let order_len = payout_order.len() as u32;
        if round_a >= order_len || round_b >= order_len || round_a <= current_round || round_b <= current_round { panic_with_error!(&env, ExtError::InvalidAmount); }
        if payout_order.get(round_a).unwrap() != initiator || payout_order.get(round_b).unwrap() != counterparty { panic_with_error!(&env, Error::OnlyMembersAllowed); }
        let expiry: u64 = env.storage().instance().get(&DataKey2::SlotSwapExpirySeconds).unwrap_or(86_400);
        let now = env.ledger().timestamp();
        let swap_id: u32 = env.storage().instance().get(&DataKey2::SlotSwapCounter).unwrap_or(0) + 1;
        env.storage().instance().set(&DataKey2::SlotSwapCounter, &swap_id);
        let swap = SlotSwap { id: swap_id, initiator: initiator.clone(), counterparty: counterparty.clone(), round_a, round_b, status: SlotSwapStatus::Pending, created_at: now, expiry_at: now + expiry, admin_approved: false };
        let mut swaps: Map<u32, SlotSwap> = env.storage().instance().get(&DataKey2::SlotSwaps).unwrap_or(Map::new(&env));
        swaps.set(swap_id, swap);
        env.storage().instance().set(&DataKey2::SlotSwaps, &swaps);
        events::emit_slot_swap_requested(&env, swap_id, initiator, counterparty, round_a, round_b);
        swap_id
    }

    pub fn accept_slot_swap(env: Env, counterparty: Address, swap_id: u32) {
        counterparty.require_auth();
        internals::check_not_paused(&env);
        let mut swaps: Map<u32, SlotSwap> = env.storage().instance().get(&DataKey2::SlotSwaps).unwrap_or(Map::new(&env));
        let mut swap = swaps.get(swap_id).expect("Swap not found");
        if swap.counterparty != counterparty { panic_with_error!(&env, Error::OnlyMembersAllowed); }
        if swap.status != SlotSwapStatus::Pending { panic_with_error!(&env, Error::ProposalNotPending); }
        if env.ledger().timestamp() > swap.expiry_at {
            swap.status = SlotSwapStatus::Expired;
            swaps.set(swap_id, swap);
            env.storage().instance().set(&DataKey2::SlotSwaps, &swaps);
            events::emit_slot_swap_expired(&env, swap_id);
            return;
        }
        swap.status = SlotSwapStatus::Accepted;
        swaps.set(swap_id, swap.clone());
        env.storage().instance().set(&DataKey2::SlotSwaps, &swaps);
        events::emit_slot_swap_accepted(&env, swap_id, counterparty);
        let requires_admin: bool = env.storage().instance().get(&DataKey2::SlotSwapRequiresAdmin).unwrap_or(false);
        if !requires_admin { Self::execute_slot_swap_inner(&env, swap_id); }
    }

    pub fn reject_slot_swap(env: Env, counterparty: Address, swap_id: u32) {
        counterparty.require_auth();
        let mut swaps: Map<u32, SlotSwap> = env.storage().instance().get(&DataKey2::SlotSwaps).unwrap_or(Map::new(&env));
        let mut swap = swaps.get(swap_id).expect("Swap not found");
        if swap.counterparty != counterparty { panic_with_error!(&env, Error::OnlyMembersAllowed); }
        if swap.status != SlotSwapStatus::Pending { panic_with_error!(&env, Error::ProposalNotPending); }
        swap.status = SlotSwapStatus::Rejected;
        swaps.set(swap_id, swap);
        env.storage().instance().set(&DataKey2::SlotSwaps, &swaps);
        events::emit_slot_swap_rejected(&env, swap_id, counterparty);
    }

    pub fn approve_slot_swap(env: Env, admin: Address, swap_id: u32) {
        admin.require_auth();
        let a: Address = env.storage().instance().get(&DataKey::Admin).expect("No admin");
        if admin != a { panic_with_error!(&env, ExtError::OnlyAdminAllowed); }
        let mut swaps: Map<u32, SlotSwap> = env.storage().instance().get(&DataKey2::SlotSwaps).unwrap_or(Map::new(&env));
        let swap = swaps.get(swap_id).expect("Swap not found");
        if swap.status != SlotSwapStatus::Accepted { panic_with_error!(&env, Error::ProposalNotPending); }
        Self::execute_slot_swap_inner(&env, swap_id);
    }

    fn execute_slot_swap_inner(env: &Env, swap_id: u32) {
        let mut swaps: Map<u32, SlotSwap> = env.storage().instance().get(&DataKey2::SlotSwaps).unwrap_or(Map::new(env));
        let mut swap = swaps.get(swap_id).unwrap();
        let mut payout_order: Vec<Address> = env.storage().instance().get(&DataKey::PayoutOrder).unwrap();
        let addr_a = payout_order.get(swap.round_a).unwrap();
        let addr_b = payout_order.get(swap.round_b).unwrap();
        let mut new_order: Vec<Address> = Vec::new(env);
        for (i, addr) in payout_order.iter().enumerate() {
            if i as u32 == swap.round_a { new_order.push_back(addr_b.clone()); }
            else if i as u32 == swap.round_b { new_order.push_back(addr_a.clone()); }
            else { new_order.push_back(addr); }
        }
        env.storage().instance().set(&DataKey::PayoutOrder, &new_order);
        swap.status = SlotSwapStatus::Executed;
        swaps.set(swap_id, swap.clone());
        env.storage().instance().set(&DataKey2::SlotSwaps, &swaps);
        events::emit_slot_swap_executed(env, swap_id, swap.round_a, swap.round_b);
    }

    // ─── #214: Insurance Coverage Mode ───────────────────────────────────────

    pub fn set_insurance_coverage_mode(env: Env, admin: Address, mode: InsuranceCoverageMode) {
        admin.require_auth();
        internals::check_not_paused(&env);
        let a: Address = env.storage().instance().get(&DataKey::Admin).expect("No admin");
        if admin != a { panic_with_error!(&env, ExtError::OnlyAdminAllowed); }
        env.storage().instance().set(&DataKey2::InsuranceCoverageMode, &mode);
        events::emit_insurance_coverage_mode_set(&env, mode as u32);
    }

    pub fn get_insurance_claims(env: Env, round: u32) -> Vec<InsuranceClaim> {
        let claims: Map<u32, Vec<InsuranceClaim>> = env.storage().instance().get(&DataKey2::InsuranceClaims).unwrap_or(Map::new(&env));
        claims.get(round).unwrap_or(Vec::new(&env))
    }

    // ─── #218: Suspended Member Reinstatement ────────────────────────────────

    pub fn set_reinstatement_fee(env: Env, admin: Address, fee: i128) {
        admin.require_auth();
        internals::check_not_paused(&env);
        let a: Address = env.storage().instance().get(&DataKey::Admin).expect("No admin");
        if admin != a { panic_with_error!(&env, ExtError::OnlyAdminAllowed); }
        if fee < 0 { panic_with_error!(&env, Error::AmountMustBePositive); }
        env.storage().instance().set(&DataKey2::ReinstatementFee, &fee);
    }

    pub fn request_reinstatement(env: Env, member: Address, reason_hash: BytesN<32>) -> u32 {
        member.require_auth();
        internals::check_not_paused(&env);
        let suspended: Vec<Address> = env.storage().instance().get(&DataKey::SuspendedMembers).unwrap_or(Vec::new(&env));
        if !suspended.contains(&member) { panic_with_error!(&env, Error::NotAMember); }
        let am: Map<Address, u32> = env.storage().instance().get(&DataKey2::ActiveReinstatementProposal).unwrap_or(Map::new(&env));
        if am.contains_key(member.clone()) { panic_with_error!(&env, Error::AlreadyContributed); }
        let quorum_config: Map<ProposalType, u32> = env.storage().instance().get(&DataKey2::QuorumConfig).unwrap_or(Map::new(&env));
        let required_quorum = quorum_config.get(ProposalType::Reinstatement).unwrap_or(5_100);
        let now = env.ledger().timestamp();
        let mut proposals: Map<u32, Proposal> = env.storage().instance().get(&DataKey::Proposals).unwrap_or(Map::new(&env));
        let proposal_id: u32 = env.storage().instance().get(&DataKey::ProposalCounter).unwrap_or(0) + 1;
        env.storage().instance().set(&DataKey::ProposalCounter, &proposal_id);
        let proposal = Proposal {
            id: proposal_id,
            proposal_type: ProposalType::Reinstatement,
            creator: member.clone(),
            description: String::from_str(&env, "Reinstatement request"),
            target_member: member.clone(),
            votes_for: 0,
            votes_against: 0,
            created_at: now,
            deadline: now + 604_800,
            status: ProposalStatus::Pending,
            execution_data: None,
            required_quorum,
        };
        proposals.set(proposal_id, proposal);
        env.storage().instance().set(&DataKey::Proposals, &proposals);
        let mut active = am;
        active.set(member.clone(), proposal_id);
        env.storage().instance().set(&DataKey2::ActiveReinstatementProposal, &active);
        events::emit_reinstatement_requested(&env, member, proposal_id);
        proposal_id
    }

    /// Get multisig configuration
    pub fn get_multisig_config(env: Env) -> (Vec<Address>, u32) {
        let co_admins: Vec<Address> = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "co_admins"))
            .unwrap_or(Vec::new(&env));
        let threshold: u32 = env
            .storage()
            .temporary()
            .get(&Symbol::new(&env, "multisig_threshold"))
            .unwrap_or(1);
        (co_admins, threshold)
    }

    // --- Waitlist Functions ---

    /// Join the waitlist for this ROSCA group.
    /// Caller is added to the end of the waitlist in registration order.
    pub fn join_waitlist(env: Env, caller: Address) {
        internals::check_not_paused(&env);
        caller.require_auth();

        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if members.contains(&caller) {
            panic_with_error!(&env, Error::AlreadyAMember);
        }

        let exited_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ExitedMembers)
            .unwrap_or(Vec::new(&env));
        if exited_members.contains(&caller) {
            panic_with_error!(&env, Error::MemberHasExited);
        }

        let mut waitlist: Vec<(Address, u64)> = env
            .storage()
            .instance()
            .get(&DataKey2::Waitlist)
            .unwrap_or(Vec::new(&env));

        // Check not already on waitlist
        for i in 0..waitlist.len() {
            let (addr, _) = waitlist.get(i).unwrap();
            if addr == caller {
                panic!("Already on waitlist");
            }
        }

        waitlist.push_back((caller.clone(), env.ledger().timestamp()));
        env.storage().instance().set(&DataKey2::Waitlist, &waitlist);

        events::emit_waitlist_updated(&env, caller, true, waitlist.len() as u32);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Leave the waitlist voluntarily.
    pub fn leave_waitlist(env: Env, caller: Address) {
        internals::check_not_paused(&env);
        caller.require_auth();

        let waitlist: Vec<(Address, u64)> = env
            .storage()
            .instance()
            .get(&DataKey2::Waitlist)
            .unwrap_or(Vec::new(&env));

        let mut new_waitlist: Vec<(Address, u64)> = Vec::new(&env);
        let mut found = false;
        for i in 0..waitlist.len() {
            let entry = waitlist.get(i).unwrap();
            if entry.0 == caller {
                found = true;
            } else {
                new_waitlist.push_back(entry);
            }
        }
        if !found {
            panic!("Not on waitlist");
        }

        env.storage().instance().set(&DataKey2::Waitlist, &new_waitlist);
        events::emit_waitlist_updated(&env, caller, false, new_waitlist.len() as u32);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin removes an address from the waitlist.
    pub fn remove_from_waitlist(env: Env, admin: Address, target: Address) {
        internals::check_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            panic!("Only admin can remove from waitlist");
        }

        let waitlist: Vec<(Address, u64)> = env
            .storage()
            .instance()
            .get(&DataKey2::Waitlist)
            .unwrap_or(Vec::new(&env));

        let mut new_waitlist: Vec<(Address, u64)> = Vec::new(&env);
        let mut found = false;
        for i in 0..waitlist.len() {
            let entry = waitlist.get(i).unwrap();
            if entry.0 == target {
                found = true;
            } else {
                new_waitlist.push_back(entry);
            }
        }
        if !found {
            panic!("Address not on waitlist");
        }

        env.storage().instance().set(&DataKey2::Waitlist, &new_waitlist);
        events::emit_waitlist_updated(&env, target, false, new_waitlist.len() as u32);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the current waitlist as a Vec of (address, joined_at) pairs.
    pub fn get_waitlist(env: Env) -> Vec<(Address, u64)> {
        env.storage()
            .instance()
            .get(&DataKey2::Waitlist)
            .unwrap_or(Vec::new(&env))
    }

    /// Internal: promote the first waitlisted address to fill a vacancy left by `vacated_by`.
    /// Records the catch-up contribution debt; the new member must call `pay_catch_up_contribution`.
    fn try_promote_from_waitlist(env: &Env, vacated_by: &Address) {
        let waitlist: Vec<(Address, u64)> = env
            .storage()
            .instance()
            .get(&DataKey2::Waitlist)
            .unwrap_or(Vec::new(&env));

        if waitlist.is_empty() {
            return;
        }

        let (new_member, _) = waitlist.get(0).unwrap();

        // Remove from waitlist
        let mut new_waitlist: Vec<(Address, u64)> = Vec::new(&env);
        for i in 1..waitlist.len() {
            new_waitlist.push_back(waitlist.get(i).unwrap());
        }
        env.storage().instance().set(&DataKey2::Waitlist, &new_waitlist);

        // Add to members
        let mut members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        members.push_back(new_member.clone());
        env.storage().instance().set(&DataKey::Members, &members);

        // Add to payout order at the end
        let mut payout_order: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PayoutOrder)
            .expect("Payout order not set");
        payout_order.push_back(new_member.clone());
        env.storage().instance().set(&DataKey::PayoutOrder, &payout_order);

        // Calculate catch-up contribution: rounds already elapsed × contribution_amount
        let current_round: u32 = env
            .storage()
            .instance()
            .get(&DataKey::CurrentRound)
            .unwrap_or(0);
        let contribution_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ContributionAmt)
            .unwrap_or(0);
        let catch_up_amount = (current_round as i128) * contribution_amount;

        // Collect catch-up immediately (new_member must have authorized this call chain)
        if catch_up_amount > 0 {
            let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
            let client = token::Client::new(env, &token_addr);
            client.transfer(&new_member, &env.current_contract_address(), &catch_up_amount);
        }

        events::emit_member_enrolled_from_waitlist(
            env,
            new_member.clone(),
            vacated_by.clone(),
            current_round,
            catch_up_amount,
        );
    }

    /// New member pays their catch-up contribution after being promoted from the waitlist.
    pub fn pay_catch_up_contribution(env: Env, member: Address) {
        internals::check_not_paused(&env);
        member.require_auth();

        let mut debts: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey2::CatchUpDebt)
            .unwrap_or(Map::new(&env));

        let amount = debts.get(member.clone()).unwrap_or(0);
        if amount == 0 {
            panic!("No catch-up contribution owed");
        }

        let token_addr: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let client = token::Client::new(&env, &token_addr);
        client.transfer(&member, &env.current_contract_address(), &amount);

        debts.remove(member.clone());
        env.storage().instance().set(&DataKey2::CatchUpDebt, &debts);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the catch-up contribution owed by a member.
    pub fn get_catch_up_debt(env: Env, member: Address) -> i128 {
        let debts: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey2::CatchUpDebt)
            .unwrap_or(Map::new(&env));
        debts.get(member).unwrap_or(0)
    }

    // ─── #230: ROSCA Group Merge ──────────────────────────────────────────────

    /// Admin of this group (Group A) proposes a merge with Group B.
    /// `group_b_id` is an external identifier for the other group.
    /// Returns the merge proposal ID.
    pub fn propose_merge(env: Env, admin: Address, group_b_id: u32) -> u32 {
        internals::check_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            panic_with_error!(&env, Error::OnlyAdminAllowed);
        }

        // Cannot merge a dissolved or already-merged group
        let group_status: GroupStatus = env
            .storage()
            .instance()
            .get(&DataKey2::GroupStatus)
            .unwrap_or(GroupStatus::Active);
        if group_status != GroupStatus::Active {
            panic!("Group is not active");
        }

        // Merges are only permitted between rounds (PaidMembers must be empty)
        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        if !paid_members.is_empty() {
            panic!("Merge only permitted between rounds");
        }

        let proposal_id: u32 = env
            .storage()
            .instance()
            .get(&DataKey2::MergeProposalCounter)
            .unwrap_or(0) + 1;
        env.storage()
            .instance()
            .set(&DataKey2::MergeProposalCounter, &proposal_id);

        let proposal = MergeProposal {
            id: proposal_id,
            group_a_admin: admin.clone(),
            group_b_id,
            proposed_at: env.ledger().timestamp(),
            accepted: false,
        };

        let mut proposals: Map<u32, MergeProposal> = env
            .storage()
            .instance()
            .get(&DataKey2::MergeProposals)
            .unwrap_or(Map::new(&env));
        proposals.set(proposal_id, proposal);
        env.storage()
            .instance()
            .set(&DataKey2::MergeProposals, &proposals);

        events::emit_merge_proposed(&env, proposal_id, admin, group_b_id);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        proposal_id
    }

    /// Admin accepts a merge proposal and executes the merge.
    /// `new_members` is the list of Group B's members to append to this group's payout order.
    /// `group_b_balance` is the amount of tokens transferred from Group B (caller must have
    /// already transferred the tokens to this contract before calling).
    pub fn accept_merge(
        env: Env,
        admin: Address,
        merge_proposal_id: u32,
        new_members: Vec<Address>,
    ) {
        internals::check_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if admin != stored_admin {
            panic_with_error!(&env, Error::OnlyAdminAllowed);
        }

        let mut proposals: Map<u32, MergeProposal> = env
            .storage()
            .instance()
            .get(&DataKey2::MergeProposals)
            .unwrap_or(Map::new(&env));
        let mut proposal = proposals.get(merge_proposal_id).expect("Merge proposal not found");

        if proposal.accepted {
            panic!("Merge proposal already accepted");
        }

        // Merges are only permitted between rounds
        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        if !paid_members.is_empty() {
            panic!("Merge only permitted between rounds");
        }

        let max_members: u32 = env
            .storage()
            .instance()
            .get(&DataKey::MaxMembers)
            .unwrap_or(50);

        let mut members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");

        let combined_count = members.len() as u32 + new_members.len() as u32;
        if combined_count > max_members {
            panic!("Combined member count exceeds max_members");
        }

        let mut payout_order: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PayoutOrder)
            .expect("Not initialized");

        // Append Group B's members after Group A's remaining members
        for m in new_members.iter() {
            if !members.contains(&m) {
                members.push_back(m.clone());
                payout_order.push_back(m.clone());
            }
        }

        env.storage().instance().set(&DataKey::Members, &members);
        env.storage().instance().set(&DataKey::PayoutOrder, &payout_order);

        // Mark Group B as merged
        env.storage()
            .instance()
            .set(&DataKey2::GroupMergedInto, &proposal.group_b_id);

        proposal.accepted = true;
        proposals.set(merge_proposal_id, proposal.clone());
        env.storage()
            .instance()
            .set(&DataKey2::MergeProposals, &proposals);

        events::emit_merge_accepted(&env, merge_proposal_id);
        events::emit_merge_completed(&env, merge_proposal_id, new_members.len() as u32);
        events::emit_group_marked_merged(&env, proposal.group_b_id);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get a merge proposal by ID.
    pub fn get_merge_proposal(env: Env, proposal_id: u32) -> MergeProposal {
        let proposals: Map<u32, MergeProposal> = env
            .storage()
            .instance()
            .get(&DataKey2::MergeProposals)
            .unwrap_or(Map::new(&env));
        proposals.get(proposal_id).expect("Merge proposal not found")
    }

    // ── #236: Group Activity Freeze ────────────────────────────────────────────

    /// Contract-level admin freezes all group activity pending investigation.
    /// All mutating operations (contribute, close_round, finalize_round,
    /// add_member, remove_member) are blocked while frozen.
    pub fn freeze_group(env: Env, admin: Address, group_id: u32, reason_hash: BytesN<32>) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic_with_error!(&env, ExtError::OnlyAdminAllowed);
        }
        env.storage()
            .instance()
            .set(&DataKey2::CoSignerWindowLedgers, &window_ledgers);

        let is_frozen: bool = env
            .storage()
            .instance()
            .get(&DataKey2::IsFrozen)
            .unwrap_or(false);
        if is_frozen {
            panic_with_error!(&env, ExtError::GroupFrozen);
        }

        env.storage().instance().set(&DataKey2::IsFrozen, &true);

        // Append to immutable freeze log in persistent storage.
        let mut log: Vec<FreezeRecord> = env
            .storage()
            .persistent()
            .get(&PersistentKey::FreezeLog)
            .unwrap_or(Vec::new(&env));
        log.push_back(FreezeRecord {
            frozen_at_ledger: env.ledger().sequence(),
            frozen_by: admin.clone(),
            reason_hash: reason_hash.clone(),
            unfrozen_at_ledger: None,
            resolution_hash: None,
        });
        env.storage().persistent().set(&PersistentKey::FreezeLog, &log);
        env.storage().persistent().extend_ttl(
            &PersistentKey::FreezeLog,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_group_frozen(&env, group_id, reason_hash, env.ledger().sequence());
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Member designates a co-signer guarantor. Co-signer must call accept_co_signer to activate.
    pub fn set_co_signer(env: Env, member: Address, group_id: u32, co_signer: Address) {
        member.require_auth();
        let members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::Members)
            .expect("Not initialized");
        if !members.contains(&member) {
            panic_with_error!(&env, Error::NotAMember);
        }

        let mut co_signers: Map<Address, CoSignerRecord> = env
            .storage()
            .instance()
            .get(&DataKey2::CoSigners)
            .unwrap_or(Map::new(&env));
        if co_signers.contains_key(member.clone()) {
            panic_with_error!(&env, ExtError::CoSignerAlreadySet);
        }

        co_signers.set(member.clone(), CoSignerRecord {
            co_signer: co_signer.clone(),
            status: CoSignerStatus::Pending,
        });
        env.storage().instance().set(&DataKey2::CoSigners, &co_signers);

        events::emit_co_signer_set(&env, group_id, member, co_signer);
    /// Contract-level admin unfreezes the group, logging the resolution on-chain.
    pub fn unfreeze_group(env: Env, admin: Address, group_id: u32, resolution_hash: BytesN<32>) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic_with_error!(&env, ExtError::OnlyAdminAllowed);
        }

        let is_frozen: bool = env
            .storage()
            .instance()
            .get(&DataKey2::IsFrozen)
            .unwrap_or(false);
        if !is_frozen {
            panic_with_error!(&env, ExtError::GroupNotFrozen);
        }

        env.storage().instance().set(&DataKey2::IsFrozen, &false);

        // Update the last freeze record with unfreeze info.
        let mut log: Vec<FreezeRecord> = env
            .storage()
            .persistent()
            .get(&PersistentKey::FreezeLog)
            .unwrap_or(Vec::new(&env));
        let last_idx = log.len() - 1;
        let mut record = log.get(last_idx).unwrap();
        record.unfrozen_at_ledger = Some(env.ledger().sequence());
        record.resolution_hash = Some(resolution_hash.clone());
        log.set(last_idx, record);
        env.storage().persistent().set(&PersistentKey::FreezeLog, &log);
        env.storage().persistent().extend_ttl(
            &PersistentKey::FreezeLog,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_group_unfrozen(&env, group_id, resolution_hash, env.ledger().sequence());
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Co-signer accepts the guarantee designation, activating it.
    pub fn accept_co_signer(env: Env, co_signer: Address, group_id: u32, member: Address) {
        co_signer.require_auth();

        let mut co_signers: Map<Address, CoSignerRecord> = env
            .storage()
            .instance()
            .get(&DataKey2::CoSigners)
            .unwrap_or(Map::new(&env));
        let mut record = co_signers.get(member.clone()).unwrap_or_else(|| {
            panic_with_error!(&env, ExtError::NoCoSignerFound)
        });
        if record.co_signer != co_signer {
            panic_with_error!(&env, ExtError::NotTheCoSigner);
        }
        record.status = CoSignerStatus::Active;
        co_signers.set(member.clone(), record);
        env.storage().instance().set(&DataKey2::CoSigners, &co_signers);

        events::emit_co_signer_accepted(&env, group_id, member, co_signer);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Co-signer pays on behalf of a defaulting member during the grace window.
    /// The contribution is recorded as the member's own.
    pub fn co_signer_contribute(
        env: Env,
        co_signer: Address,
        group_id: u32,
        member: Address,
        token: Address,
        amount: i128,
    ) {
        co_signer.require_auth();

        let co_signers: Map<Address, CoSignerRecord> = env
            .storage()
            .instance()
            .get(&DataKey2::CoSigners)
            .unwrap_or(Map::new(&env));
        let record = co_signers.get(member.clone()).unwrap_or_else(|| {
            panic_with_error!(&env, ExtError::NoCoSignerFound)
        });
        if record.co_signer != co_signer {
            panic_with_error!(&env, ExtError::NotTheCoSigner);
        }
        if record.status != CoSignerStatus::Active {
            panic_with_error!(&env, ExtError::CoSignerNotAccepted);
        }

        // Verify window is open
        let window_starts: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey2::CoSignerWindowStart)
            .unwrap_or(Map::new(&env));
        let start = window_starts.get(member.clone()).unwrap_or_else(|| {
            panic_with_error!(&env, ExtError::CoSignerWindowNotOpen)
        });
        let co_signer_window: u32 = env
            .storage()
            .instance()
            .get(&DataKey2::CoSignerWindowLedgers)
            .unwrap_or(0);
        if env.ledger().sequence() >= start + co_signer_window {
            panic_with_error!(&env, ExtError::CoSignerWindowExpired);
        }

        // Transfer from co-signer to contract on behalf of member
        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&co_signer, &env.current_contract_address(), &amount);

        // Record contribution under member's name
        let mut paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        if !paid_members.contains(&member) {
            paid_members.push_back(member.clone());
        }
        env.storage().instance().set(&DataKey::PaidMembers, &paid_members);

        // Clear the window
        let mut window_starts_mut: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey2::CoSignerWindowStart)
            .unwrap_or(Map::new(&env));
        window_starts_mut.remove(member.clone());
        env.storage()
            .instance()
            .set(&DataKey2::CoSignerWindowStart, &window_starts_mut);

        events::emit_co_signer_contributed(&env, group_id, member, co_signer, amount);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Member removes their co-signer designation (only between rounds).
    pub fn remove_co_signer(env: Env, member: Address, group_id: u32) {
        member.require_auth();

        // Only allowed between rounds (paid_members must be empty)
        let paid_members: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::PaidMembers)
            .unwrap_or(Vec::new(&env));
        if !paid_members.is_empty() {
            panic_with_error!(&env, Error::CannotChangeMidRound);
        }

        let mut co_signers: Map<Address, CoSignerRecord> = env
            .storage()
            .instance()
            .get(&DataKey2::CoSigners)
            .unwrap_or(Map::new(&env));
        if !co_signers.contains_key(member.clone()) {
            panic_with_error!(&env, ExtError::NoCoSignerFound);
        }
        co_signers.remove(member.clone());
        env.storage().instance().set(&DataKey2::CoSigners, &co_signers);

        let _ = group_id; // used in event
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }
    /// Returns the freeze log (read-only, available even when frozen).
    pub fn get_freeze_log(env: Env) -> Vec<FreezeRecord> {
        env.storage()
            .persistent()
            .get(&PersistentKey::FreezeLog)
            .unwrap_or(Vec::new(&env))
    }
    // =========================================================================
    // #243: On-Chain Group State Snapshot for Immutable Audit
    // =========================================================================

    /// Admin sets the minimum ledger interval between snapshots (spam guard).
    pub fn set_min_snapshot_interval(env: Env, admin: Address, interval_ledgers: u32) {
        internals::check_not_paused(&env);
        admin.require_auth();
        let stored_admin: Address = env.storage().instance().get(&DataKey::Admin).expect("Not initialized");
        if admin != stored_admin { panic!("Only admin can set snapshot interval"); }

        env.storage().persistent().set(&PersistentKey::MinSnapshotIntervalLedgers, &interval_ledgers);
        env.storage().persistent().extend_ttl(&PersistentKey::MinSnapshotIntervalLedgers, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Any member or admin takes a snapshot of the current group state.
    /// Appends to the append-only SnapshotLog in persistent storage.
    pub fn take_snapshot(env: Env, caller: Address) -> u32 {
        internals::check_not_paused(&env);
        caller.require_auth();

        // Caller must be a member or admin
        let admin: Address = env.storage().instance().get(&DataKey::Admin).expect("Not initialized");
        let members: Vec<Address> = env.storage().instance().get(&DataKey::Members).unwrap_or(Vec::new(&env));
        if caller != admin && !members.contains(&caller) {
            panic_with_error!(&env, Error::OnlyMembersAllowed);
        }

        // Spam guard
        let current_ledger = env.ledger().sequence();
        let last_ledger: u32 = env.storage().persistent().get(&PersistentKey::LastSnapshotLedger).unwrap_or(0);
        let min_interval: u32 = env.storage().persistent().get(&PersistentKey::MinSnapshotIntervalLedgers).unwrap_or(0);
        if min_interval > 0 && current_ledger < last_ledger.saturating_add(min_interval) {
            panic_with_error!(&env, ExtError::SnapshotTooSoon);
        }

        // Collect current state
        let current_round: u32 = env.storage().instance().get(&DataKey::CurrentRound).unwrap_or(0);
        let payout_order: Vec<Address> = env.storage().instance().get(&DataKey::PayoutOrder).unwrap_or(Vec::new(&env));

        // Compute pooled balance: sum of member contributions this round
        let member_contributions: Map<Address, i128> = env.storage().instance().get(&DataKey::MemberContributions).unwrap_or(Map::new(&env));
        let mut pooled_balance: i128 = 0;
        for (_, amt) in member_contributions.iter() {
            pooled_balance = pooled_balance.saturating_add(amt);
        }

        // Collect member statuses
        let mut member_statuses: Vec<MemberStatus> = Vec::new(&env);
        for member in members.iter() {
            member_statuses.push_back(Self::get_member_status(env.clone(), member));
        }

        // Compute state_hash: sha256 of round_number || pooled_balance || payout_order XDR
        let mut preimage = soroban_sdk::Bytes::new(&env);
        preimage.extend_from_array(&current_round.to_be_bytes());
        preimage.extend_from_array(&pooled_balance.to_be_bytes());
        for addr in payout_order.iter() {
            preimage.append(&addr.to_xdr(&env));
        }
        let state_hash: BytesN<32> = env.crypto().sha256(&preimage).into();

        // Load existing snapshot log and append
        let mut log: Vec<GroupSnapshot> = env.storage().persistent().get(&PersistentKey::SnapshotLog).unwrap_or(Vec::new(&env));
        let snapshot_id = log.len() as u32;

        let snapshot = GroupSnapshot {
            snapshot_id,
            taken_at_ledger: current_ledger,
            taken_by: caller.clone(),
            round_number: current_round,
            pooled_balance,
            member_statuses,
            payout_order,
            state_hash: state_hash.clone(),
        };

        log.push_back(snapshot);
        env.storage().persistent().set(&PersistentKey::SnapshotLog, &log);
        env.storage().persistent().extend_ttl(&PersistentKey::SnapshotLog, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);

        // Update last snapshot ledger
        env.storage().persistent().set(&PersistentKey::LastSnapshotLedger, &current_ledger);
        env.storage().persistent().extend_ttl(&PersistentKey::LastSnapshotLedger, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT);

        events::emit_snapshot_taken(&env, snapshot_id, caller, state_hash);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        snapshot_id
    }

    /// Returns a specific snapshot by ID.
    pub fn get_snapshot(env: Env, snapshot_id: u32) -> GroupSnapshot {
        let log: Vec<GroupSnapshot> = env.storage().persistent().get(&PersistentKey::SnapshotLog).unwrap_or(Vec::new(&env));
        log.get(snapshot_id).expect("Snapshot not found")
    }

    /// Returns the total number of snapshots taken.
    pub fn get_snapshot_count(env: Env) -> u32 {
        let log: Vec<GroupSnapshot> = env.storage().persistent().get(&PersistentKey::SnapshotLog).unwrap_or(Vec::new(&env));
        log.len() as u32
    }

}

mod test;
mod test_new_features;
mod test_skip;
mod test_quorum;
mod test_waitlist;
mod test_cosigner_guarantee;
mod test_group_freeze;
mod test_snapshot;
pub use events::*;
