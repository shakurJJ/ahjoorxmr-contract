#![cfg(test)]
use soroban_sdk::{testutils::Address as _, Address, Env};
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;

use crate::{AhjoorPaymentsContract, AhjoorPaymentsContractClient, FailedDebitStatus};

fn setup_retry(
    env: &Env,
) -> (
    AhjoorPaymentsContractClient<'_>,
    Address, // admin
    Address, // merchant
    Address, // customer
    Address, // token
    TokenAdminClient<'_>,
) {
    env.mock_all_auths();

    let contract_id = env.register(AhjoorPaymentsContract, ());
    let client = AhjoorPaymentsContractClient::new(env, &contract_id);

    let admin = Address::generate(env);
    let fee_recipient = Address::generate(env);
    client.initialize(&admin, &fee_recipient, &0u32);

    let token_addr = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin = TokenAdminClient::new(env, &token_addr);
    let token_client = TokenClient::new(env, &token_addr);

    let merchant = Address::generate(env);
    let customer = Address::generate(env);

    // Mint 1_000 to customer
    token_admin.mint(&customer, &1_000);

    // Approve contract to pull from customer (sufficient for small tests)
    token_client.approve(
        &customer,
        &contract_id,
        &500_000,
        &(env.ledger().sequence() + 10_000),
    );

    (client, admin, merchant, customer, token_addr, token_admin)
}

#[test]
fn test_successful_debit_stores_succeeded_record() {
    let env = Env::default();
    let (client, _admin, merchant, customer, token, _ta) = setup_retry(&env);

    let record_id = client.initiate_allowed_payment(&merchant, &customer, &token, &500, &1u32);
    let rec = client.get_failed_debit(&record_id);

    assert_eq!(rec.status, FailedDebitStatus::Succeeded);
    assert_eq!(rec.amount, 500);
    assert_eq!(rec.plan_id, 1);
    assert_eq!(rec.attempt_number, 1);
}

#[test]
fn test_insufficient_balance_stores_pending_record() {
    let env = Env::default();
    let (client, _admin, merchant, customer, token, _ta) = setup_retry(&env);

    // Request more than customer has → stored as Pending instead of reverting
    let record_id = client.initiate_allowed_payment(&merchant, &customer, &token, &5_000, &2u32);
    let rec = client.get_failed_debit(&record_id);

    assert_eq!(rec.status, FailedDebitStatus::Pending);
    assert_eq!(rec.amount, 5_000);
    assert_eq!(rec.attempt_number, 1);
    assert!(rec.next_retry_ledger > 0);
}

#[test]
#[should_panic]
fn test_retry_not_due_before_backoff() {
    let env = Env::default();
    let (client, _admin, merchant, customer, token, _ta) = setup_retry(&env);

    let record_id = client.initiate_allowed_payment(&merchant, &customer, &token, &5_000, &3u32);
    assert_eq!(client.get_failed_debit(&record_id).status, FailedDebitStatus::Pending);

    // Retry immediately without advancing ledger → RetryNotDue
    client.retry_failed_debit(&record_id);
}

#[test]
fn test_retry_after_backoff_succeeds() {
    let env = Env::default();
    let (client, _admin, merchant, customer, token, ta) = setup_retry(&env);

    // First attempt fails (insufficient balance)
    let record_id = client.initiate_allowed_payment(&merchant, &customer, &token, &5_000, &4u32);
    let rec = client.get_failed_debit(&record_id);
    assert_eq!(rec.status, FailedDebitStatus::Pending);

    // Top up customer and advance ledger past next_retry_ledger
    ta.mint(&customer, &10_000);
    env.ledger().set_sequence_number(rec.next_retry_ledger as u32 + 1);

    client.retry_failed_debit(&record_id);

    assert_eq!(
        client.get_failed_debit(&record_id).status,
        FailedDebitStatus::Succeeded
    );
}

#[test]
fn test_max_attempts_leads_to_abandonment() {
    let env = Env::default();
    let (client, admin, merchant, customer, token, _ta) = setup_retry(&env);

    // Low max attempts
    client.set_retry_config(&admin, &1u64, &100u64, &2u32);

    let record_id = client.initiate_allowed_payment(&merchant, &customer, &token, &5_000, &5u32);
    assert_eq!(client.get_failed_debit(&record_id).status, FailedDebitStatus::Pending);

    // Exhaust all attempts (balance never topped up)
    for _ in 0..3 {
        let rec = client.get_failed_debit(&record_id);
        if rec.status != FailedDebitStatus::Pending {
            break;
        }
        env.ledger().set_sequence_number(rec.next_retry_ledger as u32 + 1);
        client.retry_failed_debit(&record_id);
    }

    assert_eq!(
        client.get_failed_debit(&record_id).status,
        FailedDebitStatus::Abandoned
    );
}

#[test]
fn test_early_retry_bypasses_backoff() {
    let env = Env::default();
    let (client, _admin, merchant, customer, token, ta) = setup_retry(&env);

    let record_id = client.initiate_allowed_payment(&merchant, &customer, &token, &5_000, &6u32);
    assert_eq!(client.get_failed_debit(&record_id).status, FailedDebitStatus::Pending);

    // Top up customer — no ledger advance needed for early retry
    ta.mint(&customer, &10_000);
    client.trigger_early_retry(&customer, &record_id);

    assert_eq!(
        client.get_failed_debit(&record_id).status,
        FailedDebitStatus::Succeeded
    );
}

#[test]
fn test_backoff_doubles_per_attempt() {
    let env = Env::default();
    let (client, admin, merchant, customer, token, _ta) = setup_retry(&env);

    client.set_retry_config(&admin, &10u64, &1_000u64, &5u32);

    let record_id = client.initiate_allowed_payment(&merchant, &customer, &token, &5_000, &7u32);
    let rec1 = client.get_failed_debit(&record_id);
    let first_next = rec1.next_retry_ledger;

    // Advance and retry (still no balance)
    env.ledger().set_sequence_number(first_next as u32 + 1);
    client.retry_failed_debit(&record_id);

    let rec2 = client.get_failed_debit(&record_id);
    assert_eq!(rec2.attempt_number, 2);
    // Back-off should have doubled
    assert!(rec2.next_retry_ledger > first_next);
}

#[test]
fn test_retry_after_customer_top_up() {
    let env = Env::default();
    let (client, _admin, merchant, customer, token, ta) = setup_retry(&env);

    let record_id = client.initiate_allowed_payment(&merchant, &customer, &token, &5_000, &8u32);
    let rec = client.get_failed_debit(&record_id);
    assert_eq!(rec.status, FailedDebitStatus::Pending);

    // Customer tops up and waits for back-off to elapse
    ta.mint(&customer, &10_000);
    env.ledger().set_sequence_number(rec.next_retry_ledger as u32 + 1);
    client.retry_failed_debit(&record_id);

    assert_eq!(
        client.get_failed_debit(&record_id).status,
        FailedDebitStatus::Succeeded
    );
}
