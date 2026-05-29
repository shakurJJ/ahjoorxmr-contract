#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Vec,
};

/// Storage TTL Constants
const INSTANCE_LIFETIME_THRESHOLD: u32 = 100_000;
const INSTANCE_BUMP_AMOUNT: u32 = 120_000;

const PERSISTENT_LIFETIME_THRESHOLD: u32 = 100_000;
const PERSISTENT_BUMP_AMOUNT: u32 = 120_000;

const SUSPENSION_HISTORY_LIMIT: u32 = 10;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    TokenAlreadyWhitelisted = 4,
    TokenNotWhitelisted = 5,
    TokenAlreadySuspended = 6,
    TokenNotSuspended = 7,
}

#[contracttype]
#[derive(Clone)]
pub struct SuspensionRecord {
    pub expiry_ledger: u32,
    pub reason_hash: BytesN<32>,
}

#[contracttype]
#[derive(Clone)]
pub struct SuspensionHistoryEntry {
    pub start_ledger: u32,
    pub expiry_ledger: u32,
    pub reason_hash: BytesN<32>,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Instance: Admin address
    Admin,
    /// Instance: Proposed new admin address (pending acceptance)
    ProposedAdmin,
    /// Persistent: Vec of whitelisted token addresses
    WhitelistedTokens,
    /// Persistent: Active suspension record per token
    SuspensionRecord(Address),
    /// Persistent: Suspension history per token (last 10 entries)
    SuspensionHistory(Address),
}

mod events;
mod client;

pub use client::TokenWhitelistClient;

#[contract]
pub struct TokenWhitelistContract;

#[contractimpl]
impl TokenWhitelistContract {
    /// Initialize the contract with an admin address
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("Already initialized");
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        
        // Initialize empty whitelist
        let empty_vec: Vec<Address> = Vec::new(&env);
        env.storage()
            .persistent()
            .set(&DataKey::WhitelistedTokens, &empty_vec);
        env.storage().persistent().extend_ttl(
            &DataKey::WhitelistedTokens,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        events::emit_contract_initialized(&env, admin);
    }

    /// Add a token to the whitelist (admin only)
    pub fn add_token(env: Env, admin: Address, token: Address) {
        admin.require_auth();
        Self::require_admin(&env, &admin);

        let mut whitelist: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedTokens)
            .unwrap_or_else(|| Vec::new(&env));

        // Check if token already exists
        for existing_token in whitelist.iter() {
            if existing_token == token {
                panic!("Token already whitelisted");
            }
        }

        whitelist.push_back(token.clone());
        env.storage()
            .persistent()
            .set(&DataKey::WhitelistedTokens, &whitelist);
        env.storage().persistent().extend_ttl(
            &DataKey::WhitelistedTokens,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        events::emit_token_whitelisted(&env, token, admin);
    }

    /// Remove a token from the whitelist (admin only)
    pub fn remove_token(env: Env, admin: Address, token: Address) {
        admin.require_auth();
        Self::require_admin(&env, &admin);

        let whitelist: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedTokens)
            .unwrap_or_else(|| Vec::new(&env));

        // Find and remove the token
        let mut found = false;
        let mut new_whitelist = Vec::new(&env);
        for existing_token in whitelist.iter() {
            if existing_token == token {
                found = true;
            } else {
                new_whitelist.push_back(existing_token);
            }
        }

        if !found {
            panic!("Token not whitelisted");
        }

        env.storage()
            .persistent()
            .set(&DataKey::WhitelistedTokens, &new_whitelist);
        env.storage().persistent().extend_ttl(
            &DataKey::WhitelistedTokens,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Clean up any suspension record for the removed token
        if env.storage().persistent().has(&DataKey::SuspensionRecord(token.clone())) {
            env.storage().persistent().remove(&DataKey::SuspensionRecord(token.clone()));
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        events::emit_token_delisted(&env, token, admin);
    }

    /// Check if a token is allowed (public view function)
    /// Returns false during active suspension; lazily clears and reinstates after expiry
    pub fn is_token_allowed(env: Env, token: Address) -> bool {
        // #297: Lazy suspension check
        let susp_key = DataKey::TokenSuspension(token.clone());
        if let Some(suspension) = env
            .storage()
            .persistent()
            .get::<DataKey, TokenSuspension>(&susp_key)
        {
            if env.ledger().sequence() < suspension.expiry_ledger {
                // Still suspended
                return false;
            } else {
                // Expired — lazy reinstatement: clear suspension record
                env.storage().persistent().remove(&susp_key);
                events::emit_token_auto_reinstated(&env, token.clone(), env.ledger().sequence());
            }
        }

        let whitelist: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedTokens)
            .unwrap_or_else(|| Vec::new(&env));

        env.storage().persistent().extend_ttl(
            &DataKey::WhitelistedTokens,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        let mut in_whitelist = false;
        for existing_token in whitelist.iter() {
            if existing_token == token {
                in_whitelist = true;
                break;
            }
        }

        if !in_whitelist {
            return false;
        }

        // Check for an active or expired suspension record
        let maybe_record: Option<SuspensionRecord> = env
            .storage()
            .persistent()
            .get(&DataKey::SuspensionRecord(token.clone()));

        if let Some(record) = maybe_record {
            let current_ledger = env.ledger().sequence();
            if current_ledger < record.expiry_ledger {
                // Suspension is still active
                return false;
            }
            // Suspension expired — lazy reinstatement: clear record and emit event
            env.storage()
                .persistent()
                .remove(&DataKey::SuspensionRecord(token.clone()));
            events::emit_token_auto_reinstated(&env, token, current_ledger);
        }

        true
    }

    /// Get all whitelisted tokens (view function)
    pub fn get_whitelisted_tokens(env: Env) -> Vec<Address> {
        let whitelist: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedTokens)
            .unwrap_or_else(|| Vec::new(&env));

        env.storage().persistent().extend_ttl(
            &DataKey::WhitelistedTokens,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        whitelist
    }

    /// Get the current admin address
    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Contract not initialized")
    }

    /// Propose a new admin (current admin only)
    pub fn propose_admin(env: Env, current_admin: Address, new_admin: Address) {
        current_admin.require_auth();
        Self::require_admin(&env, &current_admin);

        env.storage()
            .instance()
            .set(&DataKey::ProposedAdmin, &new_admin);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        events::emit_admin_transfer_proposed(&env, current_admin, new_admin);
    }

    /// Accept admin transfer (proposed admin only)
    pub fn accept_admin(env: Env, new_admin: Address) {
        new_admin.require_auth();

        let proposed_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::ProposedAdmin)
            .expect("No admin transfer proposed");

        if new_admin != proposed_admin {
            panic!("Only proposed admin can accept");
        }

        let old_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Contract not initialized");

        env.storage().instance().set(&DataKey::Admin, &new_admin);
        env.storage().instance().remove(&DataKey::ProposedAdmin);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        events::emit_admin_transferred(&env, old_admin, new_admin);
    }

    /// Temporarily suspend a whitelisted token for a given number of ledgers (admin only)
    pub fn suspend_token_timed(
        env: Env,
        admin: Address,
        token: Address,
        suspend_duration_ledgers: u32,
        reason_hash: BytesN<32>,
    ) {
        admin.require_auth();
        Self::require_admin(&env, &admin);

        let whitelist: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::WhitelistedTokens)
            .unwrap_or_else(|| Vec::new(&env));

        let mut in_whitelist = false;
        for t in whitelist.iter() {
            if t == token {
                in_whitelist = true;
                break;
            }
        }
        if !in_whitelist {
            panic!("Token not whitelisted");
        }

        let current_ledger = env.ledger().sequence();

        // Reject if an active suspension already exists
        let maybe_existing: Option<SuspensionRecord> = env
            .storage()
            .persistent()
            .get(&DataKey::SuspensionRecord(token.clone()));
        if let Some(existing) = maybe_existing {
            if current_ledger < existing.expiry_ledger {
                panic!("Token already suspended");
            }
        }

        let expiry_ledger = current_ledger + suspend_duration_ledgers;

        env.storage().persistent().set(
            &DataKey::SuspensionRecord(token.clone()),
            &SuspensionRecord {
                expiry_ledger,
                reason_hash: reason_hash.clone(),
            },
        );
        env.storage().persistent().extend_ttl(
            &DataKey::SuspensionRecord(token.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        Self::add_to_suspension_history(
            &env,
            &token,
            current_ledger,
            expiry_ledger,
            reason_hash.clone(),
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        events::emit_token_suspended(&env, token, expiry_ledger, reason_hash);
    }

    /// Lift an active suspension immediately (admin only)
    pub fn lift_token_suspension(env: Env, admin: Address, token: Address) {
        admin.require_auth();
        Self::require_admin(&env, &admin);

        let maybe_record: Option<SuspensionRecord> = env
            .storage()
            .persistent()
            .get(&DataKey::SuspensionRecord(token.clone()));

        let record = match maybe_record {
            Some(r) => r,
            None => panic!("No active suspension"),
        };

        let current_ledger = env.ledger().sequence();
        if current_ledger >= record.expiry_ledger {
            panic!("No active suspension");
        }

        env.storage()
            .persistent()
            .remove(&DataKey::SuspensionRecord(token.clone()));

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        events::emit_token_suspension_lifted(&env, token, admin, current_ledger);
    }

    /// Extend an active suspension by additional ledgers (admin only)
    pub fn extend_token_suspension(
        env: Env,
        admin: Address,
        token: Address,
        additional_ledgers: u32,
    ) {
        admin.require_auth();
        Self::require_admin(&env, &admin);

        let maybe_record: Option<SuspensionRecord> = env
            .storage()
            .persistent()
            .get(&DataKey::SuspensionRecord(token.clone()));

        let record = match maybe_record {
            Some(r) => r,
            None => panic!("No active suspension"),
        };

        let current_ledger = env.ledger().sequence();
        if current_ledger >= record.expiry_ledger {
            panic!("No active suspension");
        }

        env.storage().persistent().set(
            &DataKey::SuspensionRecord(token.clone()),
            &SuspensionRecord {
                expiry_ledger: record.expiry_ledger + additional_ledgers,
                reason_hash: record.reason_hash,
            },
        );
        env.storage().persistent().extend_ttl(
            &DataKey::SuspensionRecord(token.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get suspension history for a token (up to last 10 entries)
    pub fn get_suspension_history(env: Env, token: Address) -> Vec<SuspensionHistoryEntry> {
        env.storage()
            .persistent()
            .get(&DataKey::SuspensionHistory(token))
            .unwrap_or_else(|| Vec::new(&env))
    }

    fn add_to_suspension_history(
        env: &Env,
        token: &Address,
        start_ledger: u32,
        expiry_ledger: u32,
        reason_hash: BytesN<32>,
    ) {
        let mut history: Vec<SuspensionHistoryEntry> = env
            .storage()
            .persistent()
            .get(&DataKey::SuspensionHistory(token.clone()))
            .unwrap_or_else(|| Vec::new(env));

        history.push_back(SuspensionHistoryEntry {
            start_ledger,
            expiry_ledger,
            reason_hash,
        });

        // Keep only the last SUSPENSION_HISTORY_LIMIT entries
        if history.len() > SUSPENSION_HISTORY_LIMIT {
            let start_idx = history.len() - SUSPENSION_HISTORY_LIMIT;
            let mut trimmed: Vec<SuspensionHistoryEntry> = Vec::new(env);
            for i in start_idx..history.len() {
                trimmed.push_back(history.get(i).unwrap());
            }
            history = trimmed;
        }

        env.storage()
            .persistent()
            .set(&DataKey::SuspensionHistory(token.clone()), &history);
        env.storage().persistent().extend_ttl(
            &DataKey::SuspensionHistory(token.clone()),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    /// Internal helper to check admin authorization
    fn require_admin(env: &Env, caller: &Address) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Contract not initialized");

        if caller != &admin {
            panic!("Unauthorized: caller is not admin");
        }
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod test_suspension;