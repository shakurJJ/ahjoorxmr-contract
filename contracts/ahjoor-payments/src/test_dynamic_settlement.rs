#![cfg(test)]
use super::*;
use soroban_sdk::token::Client as TokenClient;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{testutils::{Address as _, Ledger}, Address, Env, Symbol};

// ---------------------------------------------------------------------------
// Mock oracle contract for testing
// ---------------------------------------------------------------------------
mod mock_oracle {
    use soroban_sdk::{contract, contractimpl, Address, Env};
    use crate::PriceData;

    #[contract]
    pub struct MockOracle;

    #[contractimpl]
    impl MockOracle {
        pub fn lastprice(_env: Env, _base: Address, _quote: Address) -> Option<PriceData> {
            Some(PriceData {
                price: 10_000_000, // 1.0 scaled by 10^7
                timestamp: 0,
            })
        }
    }
}

fn setup_dynamic<'a>() -> (Env, AhjoorPaymentsContractClient<'a>, Address, Address, Address, Address, TokenClient<'a>, TokenAdminClient<'a>) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorPaymentsContract, ());
    let client = AhjoorPaymentsContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let merchant = Address::generate(&env);
    let token_addr = env.register_stellar_asset_contract_v2(admin.clone()).address();
    let token_client = TokenClient::new(&env, &token_addr);
    let token_admin_client = TokenAdminClient::new(&env, &token_addr);

    // Register mock oracle
    let oracle_addr = env.register(mock_oracle::MockOracle, ());

    client.initialize(&admin, &admin, &0u32);
    client.set_min_collateral(&0i128);
    client.approve_merchant(&merchant);
    client.set_oracle(&oracle_addr, &token_addr, &3600u64);
    client.add_oracle_to_whitelist(&admin, &oracle_addr);

    (env, client, admin, merchant, token_addr, oracle_addr, token_client, token_admin_client)
}

// ---------------------------------------------------------------------------
// Test: successful dynamic settlement
// ---------------------------------------------------------------------------
#[test]
fn test_dynamic_settlement_success() {
    let (env, client, _admin, merchant, token_addr, oracle_addr, _tc, tac) = setup_dynamic();
    let customer = Address::generate(&env);
    tac.mint(&customer, &10_000_000);

    let fiat_currency = Symbol::new(&env, "USD");
    let pid = client.create_dynamic_payment(
        &customer, &merchant, &1_000_000, &fiat_currency, &oracle_addr, &token_addr, &50u32, &0u64,
    );

    client.complete_payment(&pid);
    assert_eq!(client.get_payment(&pid).status, PaymentStatus::Completed);

    let dynamic = client.get_dynamic_payment(&pid);
    assert_eq!(dynamic.fiat_amount, 1_000_000);
}

// ---------------------------------------------------------------------------
// Test: non-whitelisted oracle rejected
// ---------------------------------------------------------------------------
#[test]
fn test_non_whitelisted_oracle_rejected() {
    let (env, client, _admin, merchant, token_addr, _oracle_addr, _tc, tac) = setup_dynamic();
    let customer = Address::generate(&env);
    tac.mint(&customer, &10_000_000);

    let bad_oracle = Address::generate(&env);
    let fiat_currency = Symbol::new(&env, "USD");
    let result = client.try_create_dynamic_payment(
        &customer, &merchant, &1_000_000, &fiat_currency, &bad_oracle, &token_addr, &50u32, &0u64,
    );
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Test: expired dynamic payment rejected at settlement
// ---------------------------------------------------------------------------
#[test]
fn test_expired_dynamic_payment_rejected() {
    let (env, client, _admin, merchant, token_addr, oracle_addr, _tc, tac) = setup_dynamic();
    let customer = Address::generate(&env);
    tac.mint(&customer, &10_000_000);

    let fiat_currency = Symbol::new(&env, "USD");
    let now = env.ledger().timestamp();
    // Set expiry to 100 seconds from now
    let pid = client.create_dynamic_payment(
        &customer, &merchant, &1_000_000, &fiat_currency, &oracle_addr, &token_addr, &50u32, &(now + 100),
    );

    // Advance time past expiry
    env.ledger().with_mut(|l| l.timestamp += 200);

    let result = client.try_complete_payment(&pid);
    assert!(result.is_err());
}
