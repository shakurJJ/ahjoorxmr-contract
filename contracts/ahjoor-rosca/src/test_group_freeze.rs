#![cfg(test)]
use super::*;
use soroban_sdk::token::StellarAssetClient as TokenAdminClient;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    Address, BytesN, Env,
};

fn setup_freeze_test<'a>() -> (Env, AhjoorContractClient<'a>, Address, Address, soroban_sdk::Vec<Address>) {
    let env = Env::default();
    env.mock_all_auths();

    let contract_id = env.register(AhjoorContract, ());
    let client = AhjoorContractClient::new(&env, &contract_id);

    let admin = Address::generate(&env);
    let token_admin = env
        .register_stellar_asset_contract_v2(admin.clone())
        .address();
    let token_admin_client = TokenAdminClient::new(&env, &token_admin);

    let mut members = soroban_sdk::Vec::new(&env);
    for _ in 0..3 {
        let addr = Address::generate(&env);
        token_admin_client.mint(&addr, &10_000);
        members.push_back(addr);
    }

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
            max_defaults: 3,
            use_timestamp_schedule: false,
            round_duration_seconds: 0,
            max_members: None,
            skip_fee: 0,
            max_skips_per_cycle: 1,
            voting_mode: VotingMode::Equal,
        },
        &None,
    );

    env.ledger().set_timestamp(100);

    (env, client, admin, token_admin, members)
}

fn reason_hash(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[1u8; 32])
}

fn resolution_hash(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[2u8; 32])
}

#[test]
fn test_freeze_blocks_contribute() {
    let (env, client, admin, token_admin, members) = setup_freeze_test();
    let member = members.get(0).unwrap();

    client.freeze_group(&admin, &0, &reason_hash(&env));

    let result = client.try_contribute(&member, &token_admin, &100);
    assert!(result.is_err());
}

#[test]
fn test_freeze_blocks_close_round() {
    let (env, client, admin, _token_admin, _members) = setup_freeze_test();

    client.freeze_group(&admin, &0, &reason_hash(&env));

    // Advance past deadline
    env.ledger().set_timestamp(100_000);
    let result = client.try_close_round();
    assert!(result.is_err());
}

#[test]
fn test_freeze_blocks_add_member() {
    let (env, client, admin, _token_admin, _members) = setup_freeze_test();
    let new_member = Address::generate(&env);

    client.freeze_group(&admin, &0, &reason_hash(&env));

    let result = client.try_add_member(&new_member);
    assert!(result.is_err());
}

#[test]
fn test_freeze_blocks_remove_member() {
    let (env, client, admin, _token_admin, members) = setup_freeze_test();
    let member = members.get(0).unwrap();

    client.freeze_group(&admin, &0, &reason_hash(&env));

    let result = client.try_remove_member(&member);
    assert!(result.is_err());
}

#[test]
fn test_read_queries_succeed_during_freeze() {
    let (env, client, admin, _token_admin, members) = setup_freeze_test();
    let member = members.get(0).unwrap();

    client.freeze_group(&admin, &0, &reason_hash(&env));

    // Read-only queries must not panic
    let _info = client.get_group_info();
    let _status = client.get_member_status(&member);
}

#[test]
fn test_unfreeze_restores_operations() {
    let (env, client, admin, token_admin, members) = setup_freeze_test();
    let member = members.get(0).unwrap();

    client.freeze_group(&admin, &0, &reason_hash(&env));
    client.unfreeze_group(&admin, &0, &resolution_hash(&env));

    // Contribute should succeed after unfreeze
    client.contribute(&member, &token_admin, &100);
}

#[test]
fn test_non_admin_cannot_freeze() {
    let (env, client, _admin, _token_admin, members) = setup_freeze_test();
    let non_admin = members.get(0).unwrap();

    let result = client.try_freeze_group(&non_admin, &0, &reason_hash(&env));
    assert!(result.is_err());
}

#[test]
fn test_freeze_log_appended() {
    let (env, client, admin, _token_admin, _members) = setup_freeze_test();

    client.freeze_group(&admin, &0, &reason_hash(&env));
    client.unfreeze_group(&admin, &0, &resolution_hash(&env));

    let log = client.get_freeze_log();
    assert_eq!(log.len(), 1);
    let record = log.get(0).unwrap();
    assert_eq!(record.reason_hash, reason_hash(&env));
    assert!(record.unfrozen_at_ledger.is_some());
    assert_eq!(record.resolution_hash, Some(resolution_hash(&env)));
}
