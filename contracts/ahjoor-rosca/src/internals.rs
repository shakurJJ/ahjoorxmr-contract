use crate::{errors::{Error, ExtError}, events, audit_trail, ContributionEntry, DataKey, DataKey2, PersistentKey, PayoutRecord, types::{InsuranceClaim, InsuranceCoverageMode}};
use soroban_sdk::{panic_with_error, token, Address, Env, Map, Vec};

const PERSISTENT_LIFETIME_THRESHOLD: u32 = 100_000;
const PERSISTENT_BUMP_AMOUNT: u32 = 120_000;

/// Panics if the contract is currently paused.
pub(crate) fn check_not_paused(env: &Env) {
    let is_paused: bool = env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .or(env.storage().instance().get(&DataKey::IsPaused))
        .unwrap_or(false);
    if is_paused {
        panic_with_error!(env, Error::ContractPaused);
    }
}

/// Panics if the group is currently frozen by the contract-level admin.
pub(crate) fn check_not_frozen(env: &Env) {
    let is_frozen: bool = env
        .storage()
        .instance()
        .get(&DataKey2::IsFrozen)
        .unwrap_or(false);
    if is_frozen {
        panic_with_error!(env, ExtError::GroupFrozen);
    }
}

/// Pays out the current round's pot to the next eligible recipient, records
/// the payout in history, and resets the round state for the next round.
pub(crate) fn complete_round_payout(env: &Env, _paid_members: &Vec<Address>) {
    let current_round: u32 = env
        .storage()
        .instance()
        .get(&DataKey::CurrentRound)
        .unwrap();
    let payout_order: Vec<Address> = env.storage().instance().get(&DataKey::PayoutOrder).unwrap();
    let suspended_members: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::SuspendedMembers)
        .unwrap_or(Vec::new(env));
    let exited_members: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::ExitedMembers)
        .unwrap_or(Vec::new(env));

    let skip_requests: Map<(Address, u32), bool> = env
        .storage()
        .instance()
        .get(&DataKey2::SkipRequests)
        .unwrap_or(Map::new(env));

    let mut recipient_idx = (current_round % payout_order.len()) as u32;
    let mut attempts = 0;
    while attempts < payout_order.len() {
        let potential_recipient = payout_order.get(recipient_idx).unwrap();
        let has_skipped = skip_requests.get((potential_recipient.clone(), current_round)).unwrap_or(false);
        if !suspended_members.contains(&potential_recipient)
            && !exited_members.contains(&potential_recipient)
            && !has_skipped
        {
            break;
        }
        recipient_idx = (recipient_idx + 1) % payout_order.len();
        attempts += 1;
    }

    if attempts >= payout_order.len() {
        panic_with_error!(env, Error::AllMembersSuspended);
    }

    let payout_recipient = payout_order.get(recipient_idx).unwrap();
    let preferences: Map<Address, bool> = env
        .storage()
        .instance()
        .get(&DataKey2::ReinvestPreference)
        .unwrap_or(Map::new(env));
    let should_reinvest = preferences.get(payout_recipient.clone()).unwrap_or(false);

    let reward_pool: i128 = env
        .storage()
        .instance()
        .get(&DataKey::RewardPool)
        .unwrap_or(0);
    let base_token: Address = env.storage().instance().get(&DataKey::Token).unwrap();

    let approved_tokens: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::ApprovedTokens)
        .unwrap_or(Vec::new(env));

    // Get protocol fee configuration
    let fee_bps: u32 = env
        .storage()
        .instance()
        .get(&DataKey::FeeBps)
        .unwrap_or(0);
    let fee_recipient_opt: Option<Address> = env
        .storage()
        .instance()
        .get(&DataKey::FeeRecipient);

    let mut total_payout_history_amt = 0i128;
    let mut reinvested_amount = 0i128;

    // Calculate expected pot based on member tiers and check for shortfall
    let base_amount: i128 = env
        .storage()
        .instance()
        .get(&DataKey::ContributionAmt)
        .unwrap_or(0);
    let tiers: Map<Address, u32> = env
        .storage()
        .instance()
        .get(&DataKey::MemberTiers)
        .unwrap_or(Map::new(env));
    let member_contributions: Map<Address, i128> = env
        .storage()
        .instance()
        .get(&DataKey::MemberContributions)
        .unwrap_or(Map::new(env));

    let mut expected_pot: i128 = 0;
    for member in _paid_members.iter() {
        let tier_bps = tiers.get(member.clone()).unwrap_or(10_000);
        let member_expected = (base_amount * tier_bps as i128) / 10_000;
        expected_pot += member_expected;
    }

    // Calculate actual pot (excluding reward_pool for base token)
    let mut actual_pot: i128 = 0;
    for token_addr in approved_tokens.iter() {
        let client = token::Client::new(env, &token_addr);
        let mut balance = client.balance(&env.current_contract_address());

        if token_addr == base_token {
            balance -= reward_pool;
            actual_pot = balance;
        }
    }

    // #214: Draw from insurance pool based on InsuranceCoverageMode
    let mut insurance_pool: i128 = env
        .storage()
        .instance()
        .get(&DataKey2::InsurancePool)
        .unwrap_or(0);
    let shortfall = expected_pot - actual_pot;
    let coverage_mode: InsuranceCoverageMode = env
        .storage()
        .instance()
        .get(&DataKey2::InsuranceCoverageMode)
        .unwrap_or(InsuranceCoverageMode::Partial);

    if shortfall > 0 && coverage_mode != InsuranceCoverageMode::None {
        let draw_amount = match coverage_mode {
            InsuranceCoverageMode::None => 0,
            InsuranceCoverageMode::Partial => {
                if insurance_pool >= shortfall { shortfall } else { insurance_pool }
            }
            InsuranceCoverageMode::Full => {
                if insurance_pool >= shortfall {
                    shortfall
                } else {
                    events::emit_insurance_pool_exhausted(env, current_round, shortfall - insurance_pool);
                    0
                }
            }
        };
        if draw_amount > 0 {
            insurance_pool -= draw_amount;
            env.storage().instance().set(&DataKey2::InsurancePool, &insurance_pool);
            events::emit_insurance_paid_out(env, current_round, draw_amount, insurance_pool);
            events::emit_insurance_claim_executed(env, current_round, payout_recipient.clone(), draw_amount);
            let mut claims: Map<u32, Vec<InsuranceClaim>> = env
                .storage()
                .instance()
                .get(&DataKey2::InsuranceClaims)
                .unwrap_or(Map::new(env));
            let mut round_claims: Vec<InsuranceClaim> = claims.get(current_round).unwrap_or(Vec::new(env));
            round_claims.push_back(InsuranceClaim { round: current_round, defaulter: payout_recipient.clone(), amount_covered: draw_amount });
            claims.set(current_round, round_claims);
            env.storage().instance().set(&DataKey2::InsuranceClaims, &claims);
            actual_pot += draw_amount;
        }
    } else if shortfall > 0 && insurance_pool == 0 && coverage_mode != InsuranceCoverageMode::None {
        events::emit_insurance_pool_exhausted(env, current_round, shortfall);
    }

    for token_addr in approved_tokens.iter() {
        let client = token::Client::new(env, &token_addr);
        let mut balance = client.balance(&env.current_contract_address());

        if token_addr == base_token {
            balance -= reward_pool;
            total_payout_history_amt = balance;
        }

        if balance > 0 {
            // Calculate protocol fee
            let fee_amount = if fee_bps > 0 && fee_recipient_opt.is_some() {
                (balance * (fee_bps as i128)) / 10_000
            } else {
                0
            };

            let payout_amount = balance - fee_amount;

            if should_reinvest && token_addr == base_token {
                reinvested_amount = payout_amount;
                events::emit_payout_reinvested(env, payout_recipient.clone(), current_round, payout_amount);
            } else if payout_amount > 0 {
                // Transfer payout to recipient
                client.transfer(&env.current_contract_address(), &payout_recipient, &payout_amount);
            }

            // Transfer fee to fee recipient
            if fee_amount > 0 {
                if let Some(fee_recipient) = fee_recipient_opt.clone() {
                    client.transfer(&env.current_contract_address(), &fee_recipient, &fee_amount);
                    
                    // Emit fee collected event (only for base token to avoid duplicates)
                    if token_addr == base_token {
                        events::emit_fee_collected(env, current_round, fee_amount, fee_recipient);
                    }
                }
            }
        }
    }

    // Persistent: RoundHistory — append new record and extend its individual TTL
    let mut history: Vec<PayoutRecord> = env
        .storage()
        .persistent()
        .get(&PersistentKey::RoundHistory)
        .unwrap_or(Vec::new(env));
    history.push_back(PayoutRecord {
        recipient: payout_recipient.clone(),
        amount: total_payout_history_amt,
    });
    env.storage()
        .persistent()
        .set(&PersistentKey::RoundHistory, &history);
    env.storage().persistent().extend_ttl(
        &PersistentKey::RoundHistory,
        PERSISTENT_LIFETIME_THRESHOLD,
        PERSISTENT_BUMP_AMOUNT,
    );

    events::emit_rd_done(
        env,
        current_round,
        payout_recipient.clone(),
        total_payout_history_amt,
    );
    reset_round_state(env, current_round);

    // Apply reinvestment to the next round's contributions
    if should_reinvest && reinvested_amount > 0 {
        let mut next_contributions: Map<Address, i128> = env
            .storage()
            .instance()
            .get(&DataKey::MemberContributions)
            .unwrap_or(Map::new(env));
        
        next_contributions.set(payout_recipient.clone(), reinvested_amount);
        env.storage()
            .instance()
            .set(&DataKey::MemberContributions, &next_contributions);
        
        // Check if this reinvestment fulfills the next round's requirement
        let base_amount: i128 = env
            .storage()
            .instance()
            .get(&DataKey::ContributionAmt)
            .unwrap_or(0);
        let tiers: Map<Address, u32> = env
            .storage()
            .instance()
            .get(&DataKey::MemberTiers)
            .unwrap_or(Map::new(env));
        let tier_bps = tiers.get(payout_recipient.clone()).unwrap_or(10_000);
        let member_required = (base_amount * tier_bps as i128) / 10_000;

        if reinvested_amount >= member_required {
            let mut next_paid_members: Vec<Address> = env
                .storage()
                .instance()
                .get(&DataKey::PaidMembers)
                .unwrap_or(Vec::new(env));
            if !next_paid_members.contains(&payout_recipient) {
                next_paid_members.push_back(payout_recipient.clone());
                env.storage()
                    .instance()
                    .set(&DataKey::PaidMembers, &next_paid_members);
            }

            // Track reward participation for the next round
            let mut total_participations: u32 = env
                .storage()
                .instance()
                .get(&DataKey::TotalParticipations)
                .unwrap_or(0);
            let mut member_participation: Map<Address, u32> = env
                .storage()
                .instance()
                .get(&DataKey::MemberParticipation)
                .unwrap_or(Map::new(env));

            let current_participation = member_participation.get(payout_recipient.clone()).unwrap_or(0);
            member_participation.set(payout_recipient.clone(), current_participation + 1);
            total_participations += 1;

            env.storage()
                .instance()
                .set(&DataKey::TotalParticipations, &total_participations);
            env.storage()
                .instance()
                .set(&DataKey::MemberParticipation, &member_participation);
        }
    }
}

/// Advances the round counter, clears paid-members and per-round contributions,
/// and sets a new deadline.
pub(crate) fn reset_round_state(env: &Env, current_round: u32) {
    // #227: Apply pending round duration if one was scheduled
    let pending_duration: Option<u64> = env.storage().instance().get(&DataKey2::PendingRoundDuration);
    let duration: u64 = if let Some(pending) = pending_duration {
        env.storage().instance().set(&DataKey::RoundDuration, &pending);
        env.storage().instance().remove(&DataKey2::PendingRoundDuration);
        // Also update RoundDurationSeconds for timestamp-based scheduling
        env.storage().instance().set(&DataKey::RoundDurationSeconds, &pending);
        events::emit_round_duration_applied(env, current_round + 1, pending);
        pending
    } else {
        env.storage().instance().get(&DataKey::RoundDuration).unwrap()
    };
    env.storage()
        .instance()
        .set(&DataKey::CurrentRound, &(current_round + 1));
    env.storage()
        .instance()
        .set(&DataKey::PaidMembers, &Vec::<Address>::new(env));
    env.storage().instance().set(
        &DataKey::MemberContributions,
        &Map::<Address, i128>::new(env),
    );
    env.storage().instance().set(
        &DataKey::RoundDeadline,
        &(env.ledger().timestamp() + duration),
    );

    // Update timestamp-based deadline if enabled
    let use_timestamp: bool = env
        .storage()
        .instance()
        .get(&DataKey::UseTimestampSchedule)
        .unwrap_or(false);

    if use_timestamp {
        let duration_seconds: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RoundDurationSeconds)
            .unwrap_or(0);
        let next_timestamp_deadline = env.ledger().timestamp() + duration_seconds;
        env.storage()
            .instance()
            .set(&DataKey::RoundDeadlineTimestamp, &next_timestamp_deadline);
        events::emit_round_deadline_timestamp_set(env, current_round + 1, next_timestamp_deadline);
    }

    events::emit_reset(env, current_round);
}

/// Resets a member's default count and removes them from the suspended list.
pub(crate) fn execute_penalty_appeal(env: &Env, member: &Address) {
    let mut default_count: Map<Address, u32> = env
        .storage()
        .instance()
        .get(&DataKey::DefaultCount)
        .unwrap_or(Map::new(env));

    default_count.set(member.clone(), 0);
    env.storage()
        .instance()
        .set(&DataKey::DefaultCount, &default_count);

    let suspended_members: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::SuspendedMembers)
        .unwrap_or(Vec::new(env));
    let mut new_suspended = Vec::new(env);
    for m in suspended_members.iter() {
        if m != *member {
            new_suspended.push_back(m);
        }
    }
    env.storage()
        .instance()
        .set(&DataKey::SuspendedMembers, &new_suspended);

    events::emit_appeal_ok(env, member.clone());
}

/// Updates the quorum percentage if the value is within [1, 100].
pub(crate) fn execute_rule_change(env: &Env, new_quorum: Option<i128>) {
    if let Some(quorum) = new_quorum {
        if quorum >= 1 && quorum <= 100 {
            env.storage()
                .instance()
                .set(&DataKey::QuorumPercentage, &(quorum as u32));
            events::emit_rule_upd(env, quorum);
        }
    }
}

/// Updates the maximum member limit if the value is within [1, 100] and >= current count.
pub(crate) fn execute_max_members_update(env: &Env, new_max_val: Option<i128>) {
    if let Some(new_max_i128) = new_max_val {
        let new_max = new_max_i128 as u32;
        if new_max >= 1 && new_max <= 100 {
            let current_members: Vec<Address> = env
                .storage()
                .instance()
                .get(&DataKey::Members)
                .unwrap_or(Vec::new(env));

            if new_max >= current_members.len() as u32 {
                let old_max: u32 = env
                    .storage()
                    .instance()
                    .get(&DataKey::MaxMembers)
                    .unwrap_or(50);

                env.storage()
                    .instance()
                    .set(&DataKey::MaxMembers, &new_max);

                events::emit_max_members_upd(env, old_max, new_max);
            }
        }
    }
}

/// Removes a member from both the members list and the payout order.
pub(crate) fn execute_member_removal(env: &Env, member: &Address) {
    let old_members: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::Members)
        .unwrap_or(Vec::new(env));
    let mut new_members: Vec<Address> = Vec::new(env);
    for m in old_members.iter() {
        if m != *member {
            new_members.push_back(m);
        }
    }
    env.storage()
        .instance()
        .set(&DataKey::Members, &new_members);

    let old_order: Vec<Address> = env
        .storage()
        .instance()
        .get(&DataKey::PayoutOrder)
        .unwrap_or(Vec::new(env));
    let mut new_order: Vec<Address> = Vec::new(env);
    for m in old_order.iter() {
        if m != *member {
            new_order.push_back(m);
        }
    }
    env.storage()
        .instance()
        .set(&DataKey::PayoutOrder, &new_order);

    events::emit_mem_del(env, member.clone());
}
