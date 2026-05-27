#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{testutils::Address as _, Address, Env};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct TipSetup<'a> {
    env: Env,
    client: AhjoorPaymentsContractClient<'a>,
    admin: Address,
    token_addr: Address,
    token_client: TokenClient<'a>,
    token_admin: TokenAdminClient<'a>,
}

fn tip_setup<'a>() -> TipSetup<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorPaymentsContract, ());
    let client = AhjoorPaymentsContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let fee_recipient = Address::generate(&env);

    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_client = TokenClient::new(&env, &token_addr);
    let token_admin = TokenAdminClient::new(&env, &token_addr);

    client.initialize(&admin, &fee_recipient, &0);

    TipSetup {
        env,
        client,
        admin,
        token_addr,
        token_client,
        token_admin,
    }
}

// ---------------------------------------------------------------------------
// set_max_tip_bps
// ---------------------------------------------------------------------------

#[test]
fn test_default_max_tip_bps() {
    let s = tip_setup();
    // Default should be 3000 bps (30%)
    assert_eq!(s.client.get_max_tip_bps(), 3_000);
}

#[test]
fn test_set_max_tip_bps() {
    let s = tip_setup();
    s.client.set_max_tip_bps(&s.admin, &1_000);
    assert_eq!(s.client.get_max_tip_bps(), 1_000);
}

#[test]
#[should_panic(expected = "max_tip_bps cannot exceed 10 000")]
fn test_set_max_tip_bps_too_high() {
    let s = tip_setup();
    s.client.set_max_tip_bps(&s.admin, &10_001);
}

// ---------------------------------------------------------------------------
// create_payment_with_tipping — tipping_enabled flag is stored
// ---------------------------------------------------------------------------

#[test]
fn test_create_payment_with_tipping_sets_flag() {
    let s = tip_setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.token_admin.mint(&customer, &1_000);

    let pid = s.client.create_payment_with_tipping(
        &customer,
        &merchant,
        &500,
        &s.token_addr,
        &None,
        &None,
        &None,
    );

    let payment = s.client.get_payment(&pid);
    assert!(payment.tipping_enabled);
}

#[test]
fn test_create_payment_without_tipping_flag_false() {
    let s = tip_setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.token_admin.mint(&customer, &1_000);

    let pid = s.client.create_payment(
        &customer,
        &merchant,
        &500,
        &s.token_addr,
        &None,
        &None,
        &None,
    );

    let payment = s.client.get_payment(&pid);
    assert!(!payment.tipping_enabled);
}

// ---------------------------------------------------------------------------
// complete_payment_with_tip — success path
// ---------------------------------------------------------------------------

#[test]
fn test_zero_tip_completes_like_normal_payment() {
    let s = tip_setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.token_admin.mint(&customer, &1_000);

    // Normal payment (tipping not enabled) with tip_amount = 0 must succeed.
    let pid = s.client.create_payment(
        &customer,
        &merchant,
        &500,
        &s.token_addr,
        &None,
        &None,
        &None,
    );

    s.client.complete_payment_with_tip(&pid, &customer, &0);
    let payment = s.client.get_payment(&pid);
    assert_eq!(payment.status, PaymentStatus::Completed);
}

#[test]
fn test_tip_forwarded_to_merchant() {
    let s = tip_setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    // Customer needs base amount (500) + tip (50)
    s.token_admin.mint(&customer, &550);

    let pid = s.client.create_payment_with_tipping(
        &customer,
        &merchant,
        &500,
        &s.token_addr,
        &None,
        &None,
        &None,
    );

    let before = s.token_client.balance(&merchant);
    s.client.complete_payment_with_tip(&pid, &customer, &50);
    let after = s.token_client.balance(&merchant);

    // Merchant receives base (500) + tip (50) = 550
    assert_eq!(after - before, 550);

    let payment = s.client.get_payment(&pid);
    assert_eq!(payment.status, PaymentStatus::Completed);
}

#[test]
fn test_tip_is_fee_exempt() {
    // Create contract with 10% protocol fee and verify tip doesn't have fee taken.
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(AhjoorPaymentsContract, ());
    let client = AhjoorPaymentsContractClient::new(&env, &contract_id);
    let admin = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = TokenAdminClient::new(&env, &token_addr);
    let token_client = TokenClient::new(&env, &token_addr);

    // 10% protocol fee
    client.initialize(&admin, &fee_recipient, &1_000);

    let customer = Address::generate(&env);
    let merchant = Address::generate(&env);
    token_admin.mint(&customer, &1_100);

    let pid = client.create_payment_with_tipping(
        &customer,
        &merchant,
        &1_000,
        &token_addr,
        &None,
        &None,
        &None,
    );

    let before = token_client.balance(&merchant);
    let before_fee = token_client.balance(&fee_recipient);
    client.complete_payment_with_tip(&pid, &customer, &100);
    let after = token_client.balance(&merchant);
    let after_fee = token_client.balance(&fee_recipient);

    // Base: 1000 - 10% fee (100) = 900 net to merchant
    // Tip: 100 (fee-exempt, goes directly to merchant)
    assert_eq!(after - before, 1_000); // 900 base net + 100 tip
    assert_eq!(after_fee - before_fee, 100); // fee only on base
}

// ---------------------------------------------------------------------------
// complete_payment_with_tip — rejection paths
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "Error(Contract, #18)")]
fn test_tip_on_non_tip_payment_panics() {
    let s = tip_setup();
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.token_admin.mint(&customer, &1_000);

    let pid = s.client.create_payment(
        &customer,
        &merchant,
        &500,
        &s.token_addr,
        &None,
        &None,
        &None,
    );

    // tipping_enabled = false, tip_amount > 0 → TippingNotEnabled
    s.client.complete_payment_with_tip(&pid, &customer, &50);
}

#[test]
#[should_panic(expected = "Error(Contract, #19)")]
fn test_tip_exceeds_max_bps_panics() {
    let s = tip_setup();
    // Max tip = 3000 bps (30%) of base 500 = 150. Provide 200.
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.token_admin.mint(&customer, &700);

    let pid = s.client.create_payment_with_tipping(
        &customer,
        &merchant,
        &500,
        &s.token_addr,
        &None,
        &None,
        &None,
    );

    // 200 > 150 (30% of 500) → TipExceedsMaxBps
    s.client.complete_payment_with_tip(&pid, &customer, &200);
}

#[test]
fn test_tip_at_exact_max_bps_succeeds() {
    let s = tip_setup();
    // 30% of 500 = 150
    let customer = Address::generate(&s.env);
    let merchant = Address::generate(&s.env);
    s.token_admin.mint(&customer, &650);

    let pid = s.client.create_payment_with_tipping(
        &customer,
        &merchant,
        &500,
        &s.token_addr,
        &None,
        &None,
        &None,
    );

    s.client.complete_payment_with_tip(&pid, &customer, &150);
    assert_eq!(
        s.client.get_payment(&pid).status,
        PaymentStatus::Completed
    );
}
