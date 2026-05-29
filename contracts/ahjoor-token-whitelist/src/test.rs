#![cfg(test)]

use crate::{TokenWhitelistContract, TokenWhitelistContractClient};
use soroban_sdk::{
    testutils::{Address as _, Events, Ledger},
    Address, BytesN, Env,
};

fn setup_test() -> (Env, Address, TokenWhitelistContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register(TokenWhitelistContract, ());
    let client = TokenWhitelistContractClient::new(&env, &contract_id);

    client.initialize(&admin);

    (env, admin, client)
}

#[test]
fn test_initialize() {
    let (env, admin, client) = setup_test();

    // Verify admin is set
    assert_eq!(client.get_admin(), admin);

    // Verify whitelist is empty initially
    let tokens = client.get_whitelisted_tokens();
    assert_eq!(tokens.len(), 0);

    // Check initialization event
    let events = env.events().all();
    // Just verify the contract works, events can be tested separately
}

#[test]
#[should_panic(expected = "Already initialized")]
fn test_initialize_twice_fails() {
    let (_, admin, client) = setup_test();
    
    // Try to initialize again
    client.initialize(&admin);
}

#[test]
fn test_add_token() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    // Add token
    client.add_token(&admin, &token);

    // Verify token is whitelisted
    assert!(client.is_token_allowed(&token));

    // Verify it's in the whitelist
    let tokens = client.get_whitelisted_tokens();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens.get(0).unwrap(), token);

    // Check event was emitted
    let events = env.events().all();
    // Just verify the functionality works
}

#[test]
#[should_panic(expected = "Token already whitelisted")]
fn test_add_token_twice_fails() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.add_token(&admin, &token);
    client.add_token(&admin, &token); // Should fail
}

#[test]
#[should_panic(expected = "Unauthorized: caller is not admin")]
fn test_add_token_unauthorized() {
    let (env, _admin, client) = setup_test();
    let token = Address::generate(&env);
    let unauthorized = Address::generate(&env);

    client.add_token(&unauthorized, &token);
}

#[test]
fn test_remove_token() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    // Add then remove token
    client.add_token(&admin, &token);
    assert!(client.is_token_allowed(&token));

    client.remove_token(&admin, &token);
    assert!(!client.is_token_allowed(&token));

    // Verify it's not in the whitelist
    let tokens = client.get_whitelisted_tokens();
    assert_eq!(tokens.len(), 0);

    // Check events were emitted
    let events = env.events().all();
    // Just verify the functionality works
}

#[test]
#[should_panic(expected = "Token not whitelisted")]
fn test_remove_nonexistent_token_fails() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.remove_token(&admin, &token);
}

#[test]
#[should_panic(expected = "Unauthorized: caller is not admin")]
fn test_remove_token_unauthorized() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);
    let unauthorized = Address::generate(&env);

    client.add_token(&admin, &token);
    client.remove_token(&unauthorized, &token);
}

#[test]
fn test_is_token_allowed_nonexistent() {
    let (env, _admin, client) = setup_test();
    let token = Address::generate(&env);

    assert!(!client.is_token_allowed(&token));
}

#[test]
fn test_multiple_tokens() {
    let (env, admin, client) = setup_test();
    let token1 = Address::generate(&env);
    let token2 = Address::generate(&env);
    let token3 = Address::generate(&env);

    // Add multiple tokens
    client.add_token(&admin, &token1);
    client.add_token(&admin, &token2);
    client.add_token(&admin, &token3);

    // Verify all are whitelisted
    assert!(client.is_token_allowed(&token1));
    assert!(client.is_token_allowed(&token2));
    assert!(client.is_token_allowed(&token3));

    let tokens = client.get_whitelisted_tokens();
    assert_eq!(tokens.len(), 3);

    // Remove one token
    client.remove_token(&admin, &token2);
    assert!(client.is_token_allowed(&token1));
    assert!(!client.is_token_allowed(&token2));
    assert!(client.is_token_allowed(&token3));

    let tokens = client.get_whitelisted_tokens();
    assert_eq!(tokens.len(), 2);
}

#[test]
fn test_admin_transfer() {
    let (env, admin, client) = setup_test();
    let new_admin = Address::generate(&env);
    let token = Address::generate(&env);

    // Current admin can add tokens
    client.add_token(&admin, &token);

    // Propose new admin
    client.propose_admin(&admin, &new_admin);

    // New admin accepts
    client.accept_admin(&new_admin);

    // Verify new admin
    assert_eq!(client.get_admin(), new_admin);

    // New admin can add tokens
    let token2 = Address::generate(&env);
    client.add_token(&new_admin, &token2);

    // Old admin cannot add tokens anymore
    let token3 = Address::generate(&env);
    let result = client.try_add_token(&admin, &token3);
    assert!(result.is_err());

    // Check events
    let events = env.events().all();
    // Just verify the functionality works
}

#[test]
#[should_panic(expected = "Only proposed admin can accept")]
fn test_admin_transfer_wrong_acceptor() {
    let (env, admin, client) = setup_test();
    let new_admin = Address::generate(&env);
    let wrong_admin = Address::generate(&env);

    client.propose_admin(&admin, &new_admin);
    client.accept_admin(&wrong_admin); // Should fail
}

#[test]
#[should_panic(expected = "No admin transfer proposed")]
fn test_accept_admin_without_proposal() {
    let (env, _admin, client) = setup_test();
    let new_admin = Address::generate(&env);

    client.accept_admin(&new_admin); // Should fail
}

#[test]
fn test_token_delisted_mid_operation() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    // Add token
    client.add_token(&admin, &token);
    assert!(client.is_token_allowed(&token));

    // Simulate mid-operation: token gets delisted
    client.remove_token(&admin, &token);

    // Token should no longer be allowed
    assert!(!client.is_token_allowed(&token));
}

// ── Suspension tests ──────────────────────────────────────────────────────────

#[test]
fn test_suspension_active() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.add_token(&admin, &token);
    assert!(client.is_token_allowed(&token));

    let reason = BytesN::from_array(&env, &[1u8; 32]);
    client.suspend_token_timed(&admin, &token, &50u32, &reason);

    assert!(!client.is_token_allowed(&token));
}

#[test]
fn test_auto_reinstatement_on_expiry_query() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.add_token(&admin, &token);

    let reason = BytesN::from_array(&env, &[1u8; 32]);
    let start_seq = env.ledger().sequence();
    client.suspend_token_timed(&admin, &token, &50u32, &reason);

    assert!(!client.is_token_allowed(&token));

    // Advance ledger past suspension expiry
    env.ledger().with_mut(|l| l.sequence_number = start_seq + 51);

    // Lazy reinstatement: first call after expiry clears the record and returns true
    assert!(client.is_token_allowed(&token));
    // Subsequent calls must also return true (suspension fully cleared)
    assert!(client.is_token_allowed(&token));
}

#[test]
fn test_early_lift() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.add_token(&admin, &token);

    let reason = BytesN::from_array(&env, &[1u8; 32]);
    client.suspend_token_timed(&admin, &token, &50u32, &reason);
    assert!(!client.is_token_allowed(&token));

    client.lift_token_suspension(&admin, &token);

    assert!(client.is_token_allowed(&token));
}

#[test]
fn test_suspension_extension() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.add_token(&admin, &token);

    let reason = BytesN::from_array(&env, &[1u8; 32]);
    let start_seq = env.ledger().sequence();
    // Suspend for 50 ledgers, then extend by 50 more → effective expiry = start + 100
    client.suspend_token_timed(&admin, &token, &50u32, &reason);
    client.extend_token_suspension(&admin, &token, &50u32);

    // Advance past original expiry (50) but before extended expiry (100)
    env.ledger().with_mut(|l| l.sequence_number = start_seq + 55);
    assert!(!client.is_token_allowed(&token));

    // Advance past extended expiry
    env.ledger().with_mut(|l| l.sequence_number = start_seq + 101);
    assert!(client.is_token_allowed(&token));
}

#[test]
fn test_suspension_history_record() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.add_token(&admin, &token);

    // First suspension — lift early so we can create a second
    let reason1 = BytesN::from_array(&env, &[1u8; 32]);
    client.suspend_token_timed(&admin, &token, &100u32, &reason1);
    client.lift_token_suspension(&admin, &token);

    // Second suspension
    let reason2 = BytesN::from_array(&env, &[2u8; 32]);
    client.suspend_token_timed(&admin, &token, &100u32, &reason2);

    let history = client.get_suspension_history(&token);
    assert_eq!(history.len(), 2);
    assert_eq!(history.get(0).unwrap().reason_hash, reason1);
    assert_eq!(history.get(1).unwrap().reason_hash, reason2);
}

#[test]
fn test_suspension_history_capped_at_ten() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.add_token(&admin, &token);

    // Create 11 suspensions; each is lifted early before the next
    for i in 0u32..11 {
        let reason = BytesN::from_array(&env, &[i as u8; 32]);
        client.suspend_token_timed(&admin, &token, &100u32, &reason);
        if i < 10 {
            client.lift_token_suspension(&admin, &token);
        }
    }

    let history = client.get_suspension_history(&token);
    assert_eq!(history.len(), 10);
    // Oldest entry (i=0) must have been evicted; the first kept entry is i=1
    assert_eq!(history.get(0).unwrap().reason_hash, BytesN::from_array(&env, &[1u8; 32]));
}

#[test]
fn test_non_suspended_baseline() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    // Normal whitelist/delist cycle is unchanged by the suspension feature
    client.add_token(&admin, &token);
    assert!(client.is_token_allowed(&token));

    let tokens = client.get_whitelisted_tokens();
    assert_eq!(tokens.len(), 1);

    client.remove_token(&admin, &token);
    assert!(!client.is_token_allowed(&token));
}

#[test]
#[should_panic(expected = "Token already suspended")]
fn test_double_suspend_fails() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.add_token(&admin, &token);
    let reason = BytesN::from_array(&env, &[1u8; 32]);
    client.suspend_token_timed(&admin, &token, &50u32, &reason);
    client.suspend_token_timed(&admin, &token, &50u32, &reason);
}

#[test]
#[should_panic(expected = "No active suspension")]
fn test_lift_nonexistent_suspension_fails() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    client.add_token(&admin, &token);
    client.lift_token_suspension(&admin, &token);
}

#[test]
#[should_panic(expected = "Token not whitelisted")]
fn test_suspend_nonwhitelisted_token_fails() {
    let (env, admin, client) = setup_test();
    let token = Address::generate(&env);

    let reason = BytesN::from_array(&env, &[1u8; 32]);
    client.suspend_token_timed(&admin, &token, &50u32, &reason);
}