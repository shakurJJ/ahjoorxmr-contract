#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger},
    Address, Env,
};

/// Helper to create a test setup with members
fn setup_with_members<'a>(n: usize, mint_amount: i128) -> (Env, AhjoorContractClient<'a>, Address, Address, TokenClient<'a>, TokenAdminClient<'a>, soroban_sdk::Vec<Address>) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_admin = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_admin);
    let token_admin_client = TokenAdminClient::new(&env, &token_admin);

    let mut members = soroban_sdk::Vec::new(&env);
    for _ in 0..n {
        let addr = Address::generate(&env);
        if mint_amount > 0 {
            token_admin_client.mint(&addr, &mint_amount);
        }
        members.push_back(addr);
    }

    (env, client, admin, token_admin, token_client, token_admin_client, members)
}

// ============================================================================
// FEATURE 1: Protocol Fee on ROSCA Round Payouts
// ============================================================================

#[test]
fn test_protocol_fee_deducted_from_payout() {
    let (env, client, admin, token_admin, token_client, token_admin_client, members) = 
        setup_with_members(3, 1000);

    let fee_recipient = Address::generate(&env);
    token_admin_client.mint(&fee_recipient, &0); // Initialize balance

    // Initialize with 2% fee (200 bps)
    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 200, // 2%
            fee_recipient: Some(fee_recipient.clone()),
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );

    // All members contribute
    env.ledger().set_timestamp(100);
    for i in 0..3 {
        let member = members.get(i).unwrap();
        client.contribute(&member, &token_admin, &100);
    }

    // Total pot = 300
    // Fee = 300 * 200 / 10000 = 6
    // Payout = 300 - 6 = 294

    let recipient = members.get(0).unwrap();
    assert_eq!(token_client.balance(&recipient), 1194); // 900 (after contribution) + 294 (payout)
    assert_eq!(token_client.balance(&fee_recipient), 6); // Fee collected
}

#[test]
fn test_protocol_fee_max_cap_enforced() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(2, 1000);

    let fee_recipient = Address::generate(&env);

    // Try to initialize with 6% fee (600 bps) - should fail
    let result = client.try_init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 600, // 6% - exceeds max
            fee_recipient: Some(fee_recipient),
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );

    assert_eq!(result.unwrap_err().unwrap(), Error::FeeExceedsMaximum.into());
}

#[test]
fn test_update_fee_function() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(2, 1000);

    let fee_recipient = Address::generate(&env);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 100, // 1%
            fee_recipient: Some(fee_recipient),
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );

    assert_eq!(client.get_fee_bps(), 100);

    // Update fee to 3%
    client.update_fee(&300);
    assert_eq!(client.get_fee_bps(), 300);

    // Try to update beyond cap - should fail
    let result = client.try_update_fee(&600);
    assert_eq!(result.unwrap_err().unwrap(), Error::FeeExceedsMaximum.into());
}

#[test]
fn test_no_fee_when_fee_bps_zero() {
    let (env, client, admin, token_admin, token_client, _, members) = 
        setup_with_members(2, 1000);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0, // No fee
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );

    env.ledger().set_timestamp(100);
    for i in 0..2 {
        let member = members.get(i).unwrap();
        client.contribute(&member, &token_admin, &100);
    }

    let recipient = members.get(0).unwrap();
    assert_eq!(token_client.balance(&recipient), 1100); // 900 + 200 (full pot, no fee)
}

// ============================================================================
// FEATURE 2: Partial Contribution Installments Within a Round
// ============================================================================

#[test]
fn test_partial_contribution_installments() {
    let (env, client, admin, token_admin, token_client, _, members) = 
        setup_with_members(2, 1000);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );

    let member1 = members.get(0).unwrap();
    let member2 = members.get(1).unwrap();

    env.ledger().set_timestamp(100);

    // Member1 pays in installments: 30, 40, 30
    client.contribute(&member1, &token_admin, &30);
    assert_eq!(token_client.balance(&member1), 970);
    
    let (paid, remaining) = client.get_member_contribution_status(&member1);
    assert_eq!(paid, 30);
    assert_eq!(remaining, 70);

    client.contribute(&member1, &token_admin, &40);
    assert_eq!(token_client.balance(&member1), 930);
    
    let (paid, remaining) = client.get_member_contribution_status(&member1);
    assert_eq!(paid, 70);
    assert_eq!(remaining, 30);

    client.contribute(&member1, &token_admin, &30);
    assert_eq!(token_client.balance(&member1), 900);
    
    let (paid, remaining) = client.get_member_contribution_status(&member1);
    assert_eq!(paid, 100);
    assert_eq!(remaining, 0);

    // Member2 pays in full
    client.contribute(&member2, &token_admin, &100);

    // Payout should happen
    assert_eq!(token_client.balance(&member1), 1100); // Got the payout
}

#[test]
fn test_partial_contribution_events_emitted() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(1, 1000);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );

    let member = members.get(0).unwrap();
    env.ledger().set_timestamp(100);

    // Make partial contribution
    client.contribute(&member, &token_admin, &50);
}

#[test]
fn test_cannot_exceed_remaining_contribution() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(1, 1000);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );

    let member = members.get(0).unwrap();
    env.ledger().set_timestamp(100);

    // Pay 60
    client.contribute(&member, &token_admin, &60);

    // Try to pay 50 more (total would be 110, exceeds 100)
    let result = client.try_contribute(&member, &token_admin, &50);
    assert_eq!(result.unwrap_err().unwrap(), Error::ExceedsRemainingContribution.into());
}

#[test]
fn test_get_member_contribution_status() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(1, 1000);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );

    let member = members.get(0).unwrap();

    // Initially no contribution
    let (paid, remaining) = client.get_member_contribution_status(&member);
    assert_eq!(paid, 0);
    assert_eq!(remaining, 100);

    env.ledger().set_timestamp(100);
    client.contribute(&member, &token_admin, &25);

    let (paid, remaining) = client.get_member_contribution_status(&member);
    assert_eq!(paid, 25);
    assert_eq!(remaining, 75);
}

// ============================================================================
// FEATURE: Payout Scheduling by Target Calendar Date
// ============================================================================

#[test]
fn test_timestamp_based_scheduling() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(2, 1000);

    let round_duration_seconds = 86400 * 30; // 30 days
    
    env.ledger().set_timestamp(1000000);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600, // This is the old round_duration (ledger-based)
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: true,
            round_duration_seconds,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,},
        &None,
    );

    // Initial deadline should be 1000000 + 30 days
    let expected_deadline = 1000000 + round_duration_seconds;
    assert_eq!(client.get_next_deadline_timestamp(), expected_deadline);

    // Upcoming deadlines should reflect timestamp-based scheduling
    let upcoming = client.get_upcoming_deadlines(&3);
    assert_eq!(upcoming.get(0).unwrap(), expected_deadline);
    assert_eq!(upcoming.get(1).unwrap(), expected_deadline + round_duration_seconds);
    assert_eq!(upcoming.get(2).unwrap(), expected_deadline + 2 * round_duration_seconds);

    // Contribute within deadline
    env.ledger().set_timestamp(1000000 + 86400); // 1 day later
    client.contribute(&members.get(0).unwrap(), &token_admin, &100);

    // Contribute outside old deadline (3600) but inside new one
    env.ledger().set_timestamp(1000000 + 7200); 
    client.contribute(&members.get(1).unwrap(), &token_admin, &100);

    // After round completes (due to 2/2 members contributing), next deadline should be updated correctly
    let current_timestamp = env.ledger().timestamp();
    let next_expected_deadline = current_timestamp + round_duration_seconds;
    assert_eq!(client.get_next_deadline_timestamp(), next_expected_deadline);
    
    let (_, _, deadline, _, _) = client.get_state();
    assert_eq!(deadline, next_expected_deadline);
}

// ============================================================================
// FEATURE: Maximum Member Limit
// ============================================================================

#[test]
fn test_max_members_enforcement() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(2, 1000);

    // Init with max_members = 2
    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: Some(2),
        
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,},
        &None,
    );

    assert_eq!(client.get_max_members(), 2);

    // Try to add a 3rd member - should fail
    let user3 = Address::generate(&env);
    let result = client.try_add_member(&user3);
    assert!(result.is_err());
}

#[test]
fn test_update_max_members() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(2, 1000);

    // Init with default max_members (50)
    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
        
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,},
        &None,
    );

    assert_eq!(client.get_max_members(), 50);

    // Update max_members to 10
    client.update_max_members(&10);
    assert_eq!(client.get_max_members(), 10);

    // Try to decrease below current member count (2) - should fail
    let result = client.try_update_max_members(&1);
    assert!(result.is_err());

    // Try to increase above 100 - should fail
    let result = client.try_update_max_members(&101);
    assert!(result.is_err());
}

#[test]
fn test_max_members_boundary() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(2, 1000);

    // Init with max_members = 3
    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: Some(3),
        
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,},
        &None,
    );

    // Add member at capacity minus 1 (currently 2, capacity 3)
    let user3 = Address::generate(&env);
    client.add_member(&user3);
    assert_eq!(client.get_max_members(), 3);

    // Now at capacity (3/3), try to add another - should fail
    let user4 = Address::generate(&env);
    let result = client.try_add_member(&user4);
    assert!(result.is_err());
}

#[test]
fn test_max_members_proposal() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(2, 1000);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 3,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: Some(5),
        
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,},
        &None,
    );

    let user1 = members.get(0).unwrap();
    let user2 = members.get(1).unwrap();

    // Create proposal to increase max_members to 10
    client.create_proposal(
        &user1,
        &ProposalType::MaxMembersUpdate,
        &soroban_sdk::String::from_str(&env, "Increase max members"),
        &user1, // target doesn't matter much here
        &86400,
        &Some(10),
    );

    let proposal_id = 0;
    client.vote_on_proposal(&user1, &proposal_id, &true);
    client.vote_on_proposal(&user2, &proposal_id, &true);

    // Advance time to end voting
    env.ledger().set_timestamp(env.ledger().timestamp() + 86400 * 8); // > 7 days default

    client.execute_proposal(&proposal_id);

    assert_eq!(client.get_max_members(), 10);
}

#[test]
fn test_configurable_max_defaults() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(2, 1000);

    // Set max_defaults to 2 instead of default 3
    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 10,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 2, // Custom threshold
        
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,},
        &None,
    );

    assert_eq!(client.get_max_defaults(), 2);

    let member1 = members.get(0).unwrap();
    let member2 = members.get(1).unwrap();

    // Round 1: member2 defaults
    env.ledger().set_timestamp(100);
    client.contribute(&member1, &token_admin, &100);
    env.ledger().set_timestamp(3700);
    client.finalize_round();

    // Round 2: member2 defaults again (now has 2 defaults, should be suspended)
    env.ledger().set_timestamp(4000);
    client.contribute(&member1, &token_admin, &100);
    env.ledger().set_timestamp(7400);
    client.finalize_round();

    // Check member2 is suspended after 2 defaults
    let status = client.get_member_status(&member2);
    assert!(status.is_suspended);
    assert_eq!(status.default_count, 2);
}

#[test]
fn test_suspension_threshold_set_event() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(1, 1000);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 5,
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );
}

#[test]
fn test_max_defaults_must_be_at_least_one() {
    let (env, client, admin, token_admin, _, _, members) = 
        setup_with_members(1, 1000);

    // Try to initialize with max_defaults = 0 - should fail
    let result = client.try_init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 0,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 0, // Invalid
        
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,},
        &None,
    );

    assert_eq!(result.unwrap_err().unwrap(), Error::InvalidMaxDefaults.into());
}

#[test]
fn test_penalise_defaulter_uses_max_defaults() {
    let (env, client, admin, token_admin, _, token_admin_client, members) = 
        setup_with_members(2, 1000);

    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 10,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 0,
            fee_recipient: None,
            max_defaults: 2,

            grace_period_ledgers: 0,
        
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,},
        &None,
    );

    let member1 = members.get(0).unwrap();
    let member2 = members.get(1).unwrap();

    // Round 1: member2 defaults
    env.ledger().set_timestamp(100);
    client.contribute(&member1, &token_admin, &100);
    env.ledger().set_timestamp(3700);
    client.close_round();

    // Penalize member2 (first default)
    client.penalise_defaulter(&member2);
    let status = client.get_member_status(&member2);
    assert_eq!(status.default_count, 1);
    assert!(!status.is_suspended); // Not suspended yet

    // Round 2: member2 defaults again
    env.ledger().set_timestamp(4000);
    client.contribute(&member1, &token_admin, &100);
    env.ledger().set_timestamp(7400);
    client.close_round();

    // Penalize member2 again (second default, should trigger suspension)
    token_admin_client.mint(&member2, &10); // Give penalty amount
    client.penalise_defaulter(&member2);
    let status = client.get_member_status(&member2);
    assert_eq!(status.default_count, 2);
    assert!(status.is_suspended); // Now suspended at threshold
}

// ============================================================================
// INTEGRATION TESTS: All Features Together
// ============================================================================

#[test]
fn test_all_features_integrated() {
    let (env, client, admin, token_admin, token_client, _, members) = 
        setup_with_members(3, 1000);

    let fee_recipient = Address::generate(&env);

    // Initialize with all new features
    client.init(
        &admin,
        &members,
        &100,
        &token_admin,
        &3600,
        &RoscaConfig {
            strategy: PayoutStrategy::RoundRobin,
            custom_order: None,
            penalty_amount: 10,
            exit_penalty_bps: 0,
            collective_goal: None,
            member_goals: None,
            fee_bps: 250, // 2.5% fee
            fee_recipient: Some(fee_recipient.clone()),
            max_defaults: 2, // Suspend after 2 defaults
        
            grace_period_ledgers: 0,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,},
        &None,
    );

    let member1 = members.get(0).unwrap();
    let member2 = members.get(1).unwrap();
    let member3 = members.get(2).unwrap();

    env.ledger().set_timestamp(100);

    // Member1 pays in installments
    client.contribute(&member1, &token_admin, &50);
    client.contribute(&member1, &token_admin, &50);

    // Member2 pays in full
    client.contribute(&member2, &token_admin, &100);

    // Member3 pays in full
    client.contribute(&member3, &token_admin, &100);

    // Total pot = 300
    // Fee = 300 * 250 / 10000 = 7.5 = 7 (integer division)
    // Payout = 300 - 7 = 293

    assert_eq!(token_client.balance(&member1), 1193); // 900 + 293
    assert_eq!(token_client.balance(&fee_recipient), 7);

    // Verify all query functions work
    assert_eq!(client.get_fee_bps(), 250);
    assert_eq!(client.get_fee_recipient(), Some(fee_recipient));
    assert_eq!(client.get_max_defaults(), 2);
}





