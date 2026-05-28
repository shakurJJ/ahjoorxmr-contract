#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, Address, BytesN, Env, Vec,
};

/// Storage TTL Constants
const INSTANCE_LIFETIME_THRESHOLD: u32 = 100_000;
const INSTANCE_BUMP_AMOUNT: u32 = 120_000;

const PERSISTENT_LIFETIME_THRESHOLD: u32 = 100_000;
const PERSISTENT_BUMP_AMOUNT: u32 = 120_000;

#[contracterror]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    NotInitialized = 1,
    AlreadyInitialized = 2,
    Unauthorized = 3,
    TokenAlreadyWhitelisted = 4,
    TokenNotWhitelisted = 5,
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
    /// Persistent: Active suspension per token (Option<TokenSuspension>)
    TokenSuspension(Address),
    /// Persistent: Suspension history per token (Vec<SuspensionRecord>, capped at 10)
    SuspensionHistory(Address),
}

/// #297: Active suspension record.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenSuspension {
    pub expiry_ledger: u32,
    pub reason_hash: soroban_sdk::BytesN<32>,
}

/// #297: Historical suspension entry.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuspensionRecord {
    pub expiry_ledger: u32,
    pub reason_hash: soroban_sdk::BytesN<32>,
    pub lifted_early: bool,
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

        let mut whitelist: Vec<Address> = env
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

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        events::emit_token_delisted(&env, token, admin);
    }

    /// Check if a token is allowed (public view function)
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

        for existing_token in whitelist.iter() {
            if existing_token == token {
                return true;
            }
        }
        false
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

    // ─── #297: Time-Locked Token Suspension ───────────────────────────────────────

    /// Suspend a whitelisted token for `suspend_duration_ledgers` ledgers.
    /// The token is immediately treated as non-whitelisted.
    pub fn suspend_token_timed(
        env: Env,
        admin: Address,
        token: Address,
        suspend_duration_ledgers: u32,
        reason_hash: BytesN<32>,
    ) {
        admin.require_auth();
        Self::require_admin(&env, &admin);
        if suspend_duration_ledgers == 0 {
            panic!("suspend_duration_ledgers must be positive");
        }
        // Token must be whitelisted
        let whitelist: Vec<Address> = env
            .storage().persistent()
            .get(&DataKey::WhitelistedTokens)
            .unwrap_or_else(|| Vec::new(&env));
        let mut found = false;
        for t in whitelist.iter() {
            if t == token { found = true; break; }
        }
        if !found { panic!("Token not whitelisted"); }

        let expiry_ledger = env.ledger().sequence() + suspend_duration_ledgers;
        let suspension = TokenSuspension {
            expiry_ledger,
            reason_hash: reason_hash.clone(),
        };
        let susp_key = DataKey::TokenSuspension(token.clone());
        env.storage().persistent().set(&susp_key, &suspension);
        env.storage().persistent().extend_ttl(
            &susp_key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
        );
        Self::push_suspension_history(&env, &token, expiry_ledger, reason_hash.clone(), false);
        events::emit_token_suspended(&env, token, expiry_ledger, reason_hash);
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Lift an active suspension early; token becomes active immediately.
    pub fn lift_token_suspension(env: Env, admin: Address, token: Address) {
        admin.require_auth();
        Self::require_admin(&env, &admin);
        let susp_key = DataKey::TokenSuspension(token.clone());
        if !env.storage().persistent().has(&susp_key) {
            panic!("No active suspension for this token");
        }
        env.storage().persistent().remove(&susp_key);
        // Mark last history entry as lifted early
        let hist_key = DataKey::SuspensionHistory(token.clone());
        let mut history: Vec<SuspensionRecord> = env
            .storage().persistent().get(&hist_key).unwrap_or_else(|| Vec::new(&env));
        let len = history.len();
        if len > 0 {
            let mut last = history.get(len - 1).unwrap();
            last.lifted_early = true;
            history.set(len - 1, last);
            env.storage().persistent().set(&hist_key, &history);
            env.storage().persistent().extend_ttl(
                &hist_key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
            );
        }
        events::emit_token_suspension_lifted(&env, token, admin, env.ledger().sequence());
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Extend an active suspension by `additional_ledgers`.
    pub fn extend_token_suspension(
        env: Env,
        admin: Address,
        token: Address,
        additional_ledgers: u32,
    ) {
        admin.require_auth();
        Self::require_admin(&env, &admin);
        if additional_ledgers == 0 { panic!("additional_ledgers must be positive"); }
        let susp_key = DataKey::TokenSuspension(token.clone());
        let mut suspension: TokenSuspension = env
            .storage().persistent().get(&susp_key)
            .expect("No active suspension for this token");
        suspension.expiry_ledger += additional_ledgers;
        env.storage().persistent().set(&susp_key, &suspension);
        env.storage().persistent().extend_ttl(
            &susp_key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
        );
        // Update last history entry expiry
        let hist_key = DataKey::SuspensionHistory(token.clone());
        let mut history: Vec<SuspensionRecord> = env
            .storage().persistent().get(&hist_key).unwrap_or_else(|| Vec::new(&env));
        let len = history.len();
        if len > 0 {
            let mut last = history.get(len - 1).unwrap();
            last.expiry_ledger = suspension.expiry_ledger;
            history.set(len - 1, last);
            env.storage().persistent().set(&hist_key, &history);
            env.storage().persistent().extend_ttl(
                &hist_key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
            );
        }
        env.storage().instance().extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the active suspension for a token, if any.
    pub fn get_token_suspension(env: Env, token: Address) -> Option<TokenSuspension> {
        env.storage().persistent().get(&DataKey::TokenSuspension(token))
    }

    /// Get the suspension history (last up to 10 entries) for a token.
    pub fn get_suspension_history(env: Env, token: Address) -> Vec<SuspensionRecord> {
        env.storage().persistent()
            .get(&DataKey::SuspensionHistory(token))
            .unwrap_or_else(|| Vec::new(&env))
    }

    /// Push a new entry to the suspension history, capped at 10.
    fn push_suspension_history(
        env: &Env,
        token: &Address,
        expiry_ledger: u32,
        reason_hash: BytesN<32>,
        lifted_early: bool,
    ) {
        let hist_key = DataKey::SuspensionHistory(token.clone());
        let mut history: Vec<SuspensionRecord> = env
            .storage().persistent().get(&hist_key).unwrap_or_else(|| Vec::new(env));
        // If at cap (10), drop the oldest entry
        if history.len() >= 10 {
            let mut trimmed = Vec::new(env);
            for i in 1..history.len() {
                trimmed.push_back(history.get(i).unwrap());
            }
            history = trimmed;
        }
        history.push_back(SuspensionRecord { expiry_ledger, reason_hash, lifted_early });
        env.storage().persistent().set(&hist_key, &history);
        env.storage().persistent().extend_ttl(
            &hist_key, PERSISTENT_LIFETIME_THRESHOLD, PERSISTENT_BUMP_AMOUNT,
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