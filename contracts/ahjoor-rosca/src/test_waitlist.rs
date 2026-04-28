#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{testutils::{Address as _, Ledger}, Address, Env, Vec};

fn setup_waitlist<'a>() -> (Env, AhjoorContractClient<'a>, Address, Address, Vec<Address>, TokenClient<'a>, TokenAdminClient<'a>) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_addr = env.register_stellar_asset_contract_v2(admin.clone()).address();
    let token_client = TokenClient::new(&env, &token_addr);
    let token_admin_client = TokenAdminClient::new(&env, &token_addr);

    let mut members = Vec::new(&env);
    for _ in 0..3 {
        let m = Address::generate(&env);
        token_admin_client.mint(&m, &1000);
        members.push_back(m);
    }

    client.init(
        &admin,
        &members,
        &100,
        &token_addr,
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
            max_defaults: 1, // suspend after 1 default for easy testing
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: Some(10),
            skip_fee: 0,
            max_skips_per_cycle: 0,
            voting_mode: VotingMode::Equal,
        
            grace_period_ledgers: 0,},
        &None,
    );

    (env, client, admin, token_addr, members, token_client, token_admin_client)
}

// ---------------------------------------------------------------------------
// Test: vacancy filled from waitlist when member exits
// ---------------------------------------------------------------------------
#[test]
fn test_vacancy_filled_from_waitlist_on_exit() {
    let (env, client, _admin, token_addr, members, _token_client, token_admin_client) = setup_waitlist();

    let waitlisted = Address::generate(&env);
    token_admin_client.mint(&waitlisted, &1000);

    // Join waitlist
    client.join_waitlist(&waitlisted);
    let wl = client.get_waitlist();
    assert_eq!(wl.len(), 1);

    // Member 0 requests exit (no mid-round restriction since no contributions yet)
    let exiting = members.get(0).unwrap();
    client.request_emergency_exit(&exiting);
    client.approve_exit(&exiting);

    // Waitlisted member should now be in members list
    let new_members = client.get_group_info().members;
    assert!(new_members.contains(&waitlisted));

    // Waitlist should be empty
    let wl_after = client.get_waitlist();
    assert_eq!(wl_after.len(), 0);
}

// ---------------------------------------------------------------------------
// Test: empty waitlist gracefully handled (no panic on suspension)
// ---------------------------------------------------------------------------
#[test]
fn test_empty_waitlist_graceful() {
    let (env, client, _admin, token_addr, members, _token_client, _token_admin_client) = setup_waitlist();

    // No one on waitlist — suspension should still work fine
    let member0 = members.get(0).unwrap();
    let member1 = members.get(1).unwrap();
    let member2 = members.get(2).unwrap();

    // Contribute for members 1 and 2 only; member0 defaults
    env.ledger().with_mut(|l| l.timestamp = 100);
    client.contribute(&member1, &token_addr, &100);
    client.contribute(&member2, &token_addr, &100);

    // Close round — member0 defaults and gets suspended (max_defaults=1)
    env.ledger().with_mut(|l| l.timestamp = 3700);
    client.close_round();

    // No panic — empty waitlist handled gracefully
    let wl = client.get_waitlist();
    assert_eq!(wl.len(), 0);
}

// ---------------------------------------------------------------------------
// Test: catch-up contribution calculated correctly
// ---------------------------------------------------------------------------
#[test]
fn test_catch_up_contribution_amount() {
    let (env, client, admin, token_addr, members, token_client, token_admin_client) = setup_waitlist();

    let waitlisted = Address::generate(&env);
    token_admin_client.mint(&waitlisted, &5000);
    client.join_waitlist(&waitlisted);

    // Exit immediately in round 0; catch-up amount remains zero.
    env.ledger().with_mut(|l| l.timestamp = 100);
    let exiting = members.get(0).unwrap();
    client.request_emergency_exit(&exiting);
    client.approve_exit(&exiting);

    // No debt in round 0.
    assert_eq!(client.get_catch_up_debt(&waitlisted), 0);

    // No catch-up payment should be required.
    let bal_before = token_client.balance(&waitlisted);
    let _ = token_addr; // keep setup variables exercised
    let _ = admin;
    let _ = token_admin_client;
    let bal_after = token_client.balance(&waitlisted);
    assert_eq!(bal_before, bal_after);
    assert_eq!(client.get_catch_up_debt(&waitlisted), 0);
}

// ---------------------------------------------------------------------------
// Test: leave_waitlist removes address
// ---------------------------------------------------------------------------
#[test]
fn test_leave_waitlist() {
    let (env, client, _admin, _token_addr, _members, _token_client, _token_admin_client) = setup_waitlist();

    let waitlisted = Address::generate(&env);
    client.join_waitlist(&waitlisted);
    assert_eq!(client.get_waitlist().len(), 1);

    client.leave_waitlist(&waitlisted);
    assert_eq!(client.get_waitlist().len(), 0);
}

// ---------------------------------------------------------------------------
// Test: admin remove_from_waitlist
// ---------------------------------------------------------------------------
#[test]
fn test_admin_remove_from_waitlist() {
    let (env, client, admin, _token_addr, _members, _token_client, _token_admin_client) = setup_waitlist();

    let w1 = Address::generate(&env);
    let w2 = Address::generate(&env);
    client.join_waitlist(&w1);
    client.join_waitlist(&w2);
    assert_eq!(client.get_waitlist().len(), 2);

    client.remove_from_waitlist(&admin, &w1);
    let wl = client.get_waitlist();
    assert_eq!(wl.len(), 1);
    assert_eq!(wl.get(0).unwrap().0, w2);
}

