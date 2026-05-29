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

/// Event: Token temporarily suspended
#[contractevent]
#[derive(Clone, Debug)]
pub struct TokenSuspended {
    pub token: Address,
    pub expiry_ledger: u32,
    pub reason_hash: BytesN<32>,
}

/// Event: Token suspension lifted early by admin
#[contractevent]
#[derive(Clone, Debug)]
pub struct TokenSuspensionLifted {
    pub token: Address,
    pub lifted_by: Address,
    pub ledger: u32,
}

/// Event: Token automatically reinstated after suspension expiry
#[contractevent]
#[derive(Clone, Debug)]
pub struct TokenAutoReinstated {
    pub token: Address,
    pub ledger: u32,
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

pub fn emit_token_suspended(e: &Env, token: Address, expiry_ledger: u32, reason_hash: BytesN<32>) {
    TokenSuspended {
        token,
        expiry_ledger,
        reason_hash,
    }
    .publish(e);
}

pub fn emit_token_suspension_lifted(e: &Env, token: Address, lifted_by: Address, ledger: u32) {
    TokenSuspensionLifted {
        token,
        lifted_by,
        ledger,
    }
    .publish(e);
}

pub fn emit_token_auto_reinstated(e: &Env, token: Address, ledger: u32) {
    e.events().publish(
        (soroban_sdk::Symbol::new(e, "TokenAutoReinstated"),),
        (token, ledger),
    );
}

#[contractevent]
#[derive(Clone, Debug)]
pub struct TokenQuotaSet {
    pub token: Address,
    pub max_volume_per_period: i128,
    pub period_ledgers: u32,
}

pub fn emit_token_quota_set(e: &Env, token: Address, max_volume_per_period: i128, period_ledgers: u32) {
    TokenQuotaSet {
        token,
        max_volume_per_period,
        period_ledgers,
    }
    .publish(e);
}

#[contractevent]
#[derive(Clone, Debug)]
pub struct TokenQuotaExceeded {
    pub token: Address,
    pub attempted_amount: i128,
    pub period_volume: i128,
}

pub fn emit_token_quota_exceeded(e: &Env, token: Address, attempted_amount: i128, period_volume: i128) {
    TokenQuotaExceeded {
        token,
        attempted_amount,
        period_volume,
    }
    .publish(e);
    TokenAutoReinstated { token, ledger }.publish(e);
}