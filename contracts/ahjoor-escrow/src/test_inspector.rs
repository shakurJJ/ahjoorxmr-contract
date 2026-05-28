#![cfg(test)]
use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, Vec};

use crate::{AhjoorEscrowContract, AhjoorEscrowContractClient, EscrowCreateRequest, EscrowStatus};

fn setup_env() -> (Env, AhjoorEscrowContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register_contract(None, AhjoorEscrowContract);
    let client = AhjoorEscrowContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    client.initialize(&admin);
    (env, client)
}

fn dummy_token(env: &Env, client: &AhjoorEscrowContractClient) -> Address {
    let admin = Address::generate(env);
    let token_id = env.register_stellar_asset_contract_v2(admin.clone()).address();
    client.add_allowed_token(&admin, &token_id);
    token_id
}

fn make_report_hash(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[1u8; 32])
}

#[test]
fn test_three_party_flow_approved() {
    let (env, client) = setup_env();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let inspector = Address::generate(&env);
    let token = dummy_token(&env, &client);

    let deadline = env.ledger().timestamp() + 10_000;
    let request = EscrowCreateRequest {
        seller: seller.clone(),
        arbiter: arbiter.clone(),
        amount: 1000,
        token: token.clone(),
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

    let escrow_id = client.create_escrow_with_inspector(&buyer, &request, &Some(inspector.clone()));

    // Seller marks complete → AwaitingInspection
    client.seller_mark_complete(&seller, &escrow_id);
    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::AwaitingInspection);

    // Buyer cannot release while awaiting inspection
    // (would panic with InspectionPending — tested via should_panic)

    // Inspector approves
    client.submit_inspection_report(&inspector, &escrow_id, &true, &make_report_hash(&env));
    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::InspectionPassed);

    // Report stored on-chain
    let report = client.get_inspector_report(&escrow_id).unwrap();
    assert!(report.approved);

    // Buyer releases
    client.release_escrow(&buyer, &escrow_id);
    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Released);
}

#[test]
fn test_inspection_failed_path() {
    let (env, client) = setup_env();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let inspector = Address::generate(&env);
    let token = dummy_token(&env, &client);

    let deadline = env.ledger().timestamp() + 10_000;
    let request = EscrowCreateRequest {
        seller: seller.clone(),
        arbiter: arbiter.clone(),
        amount: 500,
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

    let escrow_id = client.create_escrow_with_inspector(&buyer, &request, &Some(inspector.clone()));
    client.seller_mark_complete(&seller, &escrow_id);
    client.submit_inspection_report(&inspector, &escrow_id, &false, &make_report_hash(&env));

    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::InspectionFailed);

    let report = client.get_inspector_report(&escrow_id).unwrap();
    assert!(!report.approved);
}

#[test]
#[should_panic(expected = "InspectionPending")]
fn test_release_blocked_during_awaiting_inspection() {
    let (env, client) = setup_env();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let inspector = Address::generate(&env);
    let token = dummy_token(&env, &client);

    let deadline = env.ledger().timestamp() + 10_000;
    let request = EscrowCreateRequest {
        seller: seller.clone(),
        arbiter: arbiter.clone(),
        amount: 500,
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

    let escrow_id = client.create_escrow_with_inspector(&buyer, &request, &Some(inspector.clone()));
    client.seller_mark_complete(&seller, &escrow_id);
    // Should panic
    client.release_escrow(&buyer, &escrow_id);
}

#[test]
fn test_inspector_replacement() {
    let (env, client) = setup_env();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let inspector = Address::generate(&env);
    let new_inspector = Address::generate(&env);
    let token = dummy_token(&env, &client);

    let deadline = env.ledger().timestamp() + 10_000;
    let request = EscrowCreateRequest {
        seller: seller.clone(),
        arbiter: arbiter.clone(),
        amount: 500,
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

    let escrow_id = client.create_escrow_with_inspector(&buyer, &request, &Some(inspector.clone()));
    client.seller_mark_complete(&seller, &escrow_id);

    // Both must sign
    client.replace_inspector(&buyer, &escrow_id, &new_inspector);
    // After only buyer signed, inspector not yet replaced
    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.extensions.inspector, Some(inspector.clone()));

    client.replace_inspector(&seller, &escrow_id, &new_inspector);
    // Now replaced
    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.extensions.inspector, Some(new_inspector.clone()));
    // Status reset to Active
    assert_eq!(escrow.status, EscrowStatus::Active);
}

#[test]
fn test_no_inspector_baseline_unchanged() {
    let (env, client) = setup_env();
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let token = dummy_token(&env, &client);

    let deadline = env.ledger().timestamp() + 10_000;
    let request = EscrowCreateRequest {
        seller: seller.clone(),
        arbiter: arbiter.clone(),
        amount: 300,
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

    let escrow_id = client.create_escrow_with_inspector(&buyer, &request, &None);
    // Buyer can release directly without inspector
    client.release_escrow(&buyer, &escrow_id);
    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Released);
}
