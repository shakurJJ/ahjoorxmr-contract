#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{testutils::{Address as _, Ledger}, Address, Env, Vec};

fn setup_veto<'a>() -> (Env, AhjoorEscrowContractClient<'a>, Address, Address, Address, Address, Address, TokenClient<'a>, TokenAdminClient<'a>) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorEscrowContract, ());
    let client = AhjoorEscrowContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let buyer = Address::generate(&env);
    let seller = Address::generate(&env);
    let arbiter = Address::generate(&env);
    let token_addr = env.register_stellar_asset_contract_v2(admin.clone()).address();
    let token_client = TokenClient::new(&env, &token_addr);
    let token_admin_client = TokenAdminClient::new(&env, &token_addr);

    client.initialize(&admin);
    client.add_allowed_token(&admin, &token_addr);
    token_admin_client.mint(&buyer, &1000);

    let deadline = env.ledger().timestamp() + 100_000;
    client.create_escrow(
        &buyer, &seller, &arbiter, &500, &token_addr, &deadline,
        &None, &Vec::new(&env), &false, &0u32,
    );

    (env, client, admin, buyer, seller, arbiter, token_addr, token_client, token_admin_client)
}

#[test]
fn test_buyer_veto_refunds_buyer() {
    let (env, client, admin, buyer, seller, _arbiter, _token_addr, token_client, _) = setup_veto();
    let escrow_id = 0u32;
    let new_seller = Address::generate(&env);

    let buyer_bal_before = token_client.balance(&buyer);

    client.transfer_seller_role(&seller, &escrow_id, &new_seller);

    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::AwaitingBuyerVetoDecision);

    client.veto_seller_transfer(&buyer, &escrow_id);

    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Refunded);
    assert_eq!(token_client.balance(&buyer), buyer_bal_before + 500);
}

#[test]
fn test_buyer_approval_finalises_transfer() {
    let (env, client, _admin, buyer, seller, _arbiter, _token_addr, _token_client, _) = setup_veto();
    let escrow_id = 0u32;
    let new_seller = Address::generate(&env);

    client.transfer_seller_role(&seller, &escrow_id, &new_seller);
    client.approve_seller_transfer(&buyer, &escrow_id);

    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Active);
    assert_eq!(escrow.seller, new_seller);
}

#[test]
fn test_window_expiry_auto_approves() {
    let (env, client, admin, _buyer, seller, _arbiter, _token_addr, _token_client, _) = setup_veto();
    let escrow_id = 0u32;
    let new_seller = Address::generate(&env);

    // Set short window
    client.set_seller_transfer_veto_window(&admin, &10u32);
    client.transfer_seller_role(&seller, &escrow_id, &new_seller);

    // Advance past veto window
    env.ledger().with_mut(|l| l.sequence_number += 20);

    client.finalize_seller_transfer_if_expired(&escrow_id);

    let escrow = client.get_escrow(&escrow_id);
    assert_eq!(escrow.status, EscrowStatus::Active);
    assert_eq!(escrow.seller, new_seller);
}

#[test]
fn test_veto_window_configurable() {
    let (env, client, admin, _buyer, _seller, _arbiter, _token_addr, _token_client, _) = setup_veto();
    // Just verify admin can set the window without panic
    client.set_seller_transfer_veto_window(&admin, &200u32);
}

#[test]
fn test_only_seller_can_initiate_transfer() {
    let (env, client, _admin, buyer, _seller, _arbiter, _token_addr, _token_client, _) = setup_veto();
    let escrow_id = 0u32;
    let new_seller = Address::generate(&env);

    // Buyer tries to initiate — should fail
    let result = client.try_transfer_seller_role(&buyer, &escrow_id, &new_seller);
    assert!(result.is_err());
}
