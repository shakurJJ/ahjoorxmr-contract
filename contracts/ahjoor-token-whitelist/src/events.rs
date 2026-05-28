use soroban_sdk::{contractevent, Address, BytesN, Env};

/// Event: Contract initialized
#[contractevent]
#[derive(Clone, Debug)]
pub struct ContractInitialized {
    pub admin: Address,
}

/// Event: Token added to whitelist
#[contractevent]
#[derive(Clone, Debug)]
pub struct TokenWhitelisted {
    pub token: Address,
    pub admin: Address,
}

/// Event: Token removed from whitelist
#[contractevent]
#[derive(Clone, Debug)]
pub struct TokenDelisted {
    pub token: Address,
    pub admin: Address,
}

/// Event: Admin transfer proposed
#[contractevent]
#[derive(Clone, Debug)]
pub struct AdminTransferProposed {
    pub current_admin: Address,
    pub proposed_admin: Address,
}

/// Event: Admin transfer completed
#[contractevent]
#[derive(Clone, Debug)]
pub struct AdminTransferred {
    pub old_admin: Address,
    pub new_admin: Address,
}

// --- Helper Emission Functions ---

pub fn emit_contract_initialized(e: &Env, admin: Address) {
    ContractInitialized { admin }.publish(e);
}

pub fn emit_token_whitelisted(e: &Env, token: Address, admin: Address) {
    TokenWhitelisted { token, admin }.publish(e);
}

pub fn emit_token_delisted(e: &Env, token: Address, admin: Address) {
    TokenDelisted { token, admin }.publish(e);
}

pub fn emit_admin_transfer_proposed(e: &Env, current_admin: Address, proposed_admin: Address) {
    AdminTransferProposed {
        current_admin,
        proposed_admin,
    }
    .publish(e);
}

pub fn emit_admin_transferred(e: &Env, old_admin: Address, new_admin: Address) {
    AdminTransferred {
        old_admin,
        new_admin,
    }
    .publish(e);
}

// --- Issue #297: Time-Locked Token Suspension ---

pub fn emit_token_suspended(e: &Env, token: Address, expiry_ledger: u32, reason_hash: BytesN<32>) {
    e.events().publish(
        (soroban_sdk::Symbol::new(e, "TokenSuspended"),),
        (token, expiry_ledger, reason_hash),
    );
}

pub fn emit_token_suspension_lifted(e: &Env, token: Address, lifted_by: Address, ledger: u32) {
    e.events().publish(
        (soroban_sdk::Symbol::new(e, "TokenSuspensionLifted"),),
        (token, lifted_by, ledger),
    );
}

pub fn emit_token_auto_reinstated(e: &Env, token: Address, ledger: u32) {
    e.events().publish(
        (soroban_sdk::Symbol::new(e, "TokenAutoReinstated"),),
        (token, ledger),
    );
}