#![no_std]
use soroban_sdk::{contract, contractimpl, contracttype, token, Address, BytesN, Env, String, Vec};

// --- Storage TTL Constants ---
const INSTANCE_LIFETIME_THRESHOLD: u32 = 100_000;
const INSTANCE_BUMP_AMOUNT: u32 = 120_000;

const PERSISTENT_LIFETIME_THRESHOLD: u32 = 100_000;
const PERSISTENT_BUMP_AMOUNT: u32 = 120_000;
const DEADLINE_EXTENSION_PROPOSAL_WINDOW: u64 = 24 * 60 * 60;
const MAX_EVIDENCE_ENTRIES_PER_PARTY: u32 = 5;
const DEFAULT_DISPUTE_TIMEOUT_SECONDS: u64 = 7 * 24 * 60 * 60;
const MAX_BATCH_ESCROWS: u32 = 10;
const DEFAULT_MAX_ORACLE_AGE_SECONDS: u64 = 300;
const DEFAULT_INSURANCE_TRIGGER_DAYS: u64 = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[contracttype]
pub enum EscrowStatus {
    Active = 0,
    Released = 1,
    Disputed = 2,
    Resolved = 3,
    Refunded = 4,
    PartiallyReleased = 5,
    PartiallyDisputed = 6,
}

const ORACLE_COMPARISON_LESS_OR_EQUAL: u32 = 0;
const ORACLE_COMPARISON_GREATER_OR_EQUAL: u32 = 1;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseCondition {
    pub base: Address,
    pub quote: Address,
    /// 0 = LessOrEqual, 1 = GreaterOrEqual
    pub comparison: u32,
    pub threshold_price: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceData {
    pub price: i128,
    pub timestamp: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowCreateRequest {
    pub seller: Address,
    pub arbiter: Address,
    pub amount: i128,
    pub token: Address,
    pub deadline: u64,
    pub metadata_hash: Option<BytesN<32>>,
    pub sellers: Vec<(Address, u32)>,
    pub auto_renew: bool,
    pub renewal_count: u32,
    pub buyer_inactivity_secs: u64,
    pub min_lock_until: Option<u64>,
    pub release_base: Option<Address>,
    pub release_quote: Option<Address>,
    pub release_comparison: Option<u32>,
    pub release_threshold_price: Option<i128>,
    pub arbiter_fee_bps: Option<u32>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Escrow {
    pub id: u32,
    pub buyer: Address,
    pub seller: Address,
    pub arbiter: Address,
    pub amount: i128,
    pub token: Address,
    pub status: EscrowStatus,
    pub created_at: u64,
    pub deadline: u64,
    pub metadata_hash: Option<BytesN<32>>,
    pub sellers: Vec<(Address, u32)>, // (address, bps) — multi-party sellers
    pub extensions: EscrowExtensions,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowExtensions {
    pub auto_renew: bool,
    pub renewal_count: u32,
    pub renewals_remaining: u32,
    pub dispute_timeout_seconds: Option<u64>,
    /// Seconds of buyer inactivity before seller can claim auto-release; 0 = disabled (#150)
    pub buyer_inactivity_secs: u64,
    /// Earliest ledger timestamp when release/partial release is permitted.
    pub min_lock_until: Option<u64>,
    pub release_base: Option<Address>,
    pub release_quote: Option<Address>,
    pub release_comparison: Option<u32>,
    pub release_threshold_price: Option<i128>,
    /// Optional per-escrow arbiter fee override in basis points.
    pub arbiter_fee_bps: Option<u32>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceSubmission {
    pub evidence_hash: BytesN<32>,
    pub evidence_uri_hash: BytesN<32>,
    pub submitted_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Dispute {
    pub escrow_id: u32,
    pub reason: String,
    pub created_at: u64,
    pub resolved: bool,
    pub dispute_amount: i128,
    pub timeout_seconds: Option<u64>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeadlineProposal {
    pub proposer: Address,
    pub new_deadline: u64,
    pub proposed_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowTemplateConfig {
    pub arbiter: Address,
    pub token: Address,
    pub deadline_duration: u64, // seconds from escrow creation
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowTemplate {
    pub id: u32,
    pub creator: Address,
    pub config: EscrowTemplateConfig,
    pub active: bool,
}

/// Status of a milestone in a milestone-based escrow (#136).
#[contracttype]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MilestoneStatus {
    Pending = 0,
    Approved = 1,
    Disputed = 2,
}

/// Single milestone in a milestone-based escrow (#136).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Milestone {
    pub description_hash: BytesN<32>,
    pub amount: i128,
    pub status: MilestoneStatus,
}

const MAX_MILESTONES: u32 = 20;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowBatchConfig {
    pub seller: Address,
    pub arbiter: Address,
    pub amount: i128,
    pub token: Address,
    pub deadline: u64,
    pub metadata_hash: Option<BytesN<32>>,
    pub sellers: Vec<(Address, u32)>,
    pub auto_renew: bool,
    pub renewal_count: u32,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Admin,
    ContractVersion,
    MigrationCompleted(u32),
    Paused,
    PauseReason,
    EscrowCounter,
    Escrow(u32),
    Dispute(u32),
    DeadlineProposal(u32),
    AllowedToken(Address),
    ProtocolFeeBps,
    FeeRecipient,
    TemplateCounter,
    Template(u32),
    ArbiterPool,
    NextArbiterIndex,
    ArbiterNeedsReplacement(u32),
    EscrowMetadata(u32),
    Evidence(u32, Address),
    RenewalAllowance(u32),
    DefaultDisputeTimeout,
    DefaultArbiterFeeBps,
    OracleAddress,
    MaxOracleAge,
    InsurancePool,
    InsuranceToken,
    InsuranceTriggerDays,
    InsuranceAdminConfirmed(u32),
    InsuranceClaimed(u32),
    /// Last timestamp of any buyer action for inactivity tracking (#150)
    LastBuyerAction(u32),
    /// Optional milestone schedule attached to an escrow (#136)
    EscrowMilestones(u32),
}

const MAX_PROTOCOL_FEE_BPS: u32 = 200; // 2%
const MAX_ARBITER_FEE_BPS: u32 = 1_000; // 10%

mod oracle {
    use crate::PriceData;
    use soroban_sdk::{contractclient, Address, Env};

    #[allow(dead_code)]
    #[contractclient(name = "OracleClient")]
    pub trait OracleInterface {
        fn lastprice(env: Env, base: Address, quote: Address) -> Option<PriceData>;
    }
}

mod events;

#[contract]
pub struct AhjoorEscrowContract;

#[contractimpl]
impl AhjoorEscrowContract {
    /// Initialize upgrade admin and contract versioning state.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("Already initialized");
        }

        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &1u32);
        env.storage()
            .instance()
            .set(&DataKey::DefaultDisputeTimeout, &DEFAULT_DISPUTE_TIMEOUT_SECONDS);
        env.storage().instance().set(&DataKey::DefaultArbiterFeeBps, &0u32);
        env.storage().instance().set(&DataKey::InsurancePool, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::InsuranceTriggerDays, &DEFAULT_INSURANCE_TRIGGER_DAYS);
        env.storage()
            .instance()
            .set(&DataKey::MaxOracleAge, &DEFAULT_MAX_ORACLE_AGE_SECONDS);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Create a new escrow. Funds are transferred from buyer to contract.
    /// Returns the escrow ID.
    pub fn create_escrow(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        metadata_hash: Option<BytesN<32>>,
        sellers: Vec<(Address, u32)>,
        auto_renew: bool,
        renewal_count: u32,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let request = EscrowCreateRequest {
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers,
            auto_renew,
            renewal_count,
            buyer_inactivity_secs: 0,
            min_lock_until: None,
            release_base: None,
            release_quote: None,
            release_comparison: None,
            release_threshold_price: None,
            arbiter_fee_bps: None,
        };

        Self::create_escrow_core(&env, &buyer, request)
    }

    /// Create a new escrow with explicit inactivity release configuration.
    pub fn create_escrow_with_inactivity(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        metadata_hash: Option<BytesN<32>>,
        sellers: Vec<(Address, u32)>,
        renewal_count: u32,
        buyer_inactivity_secs: u64,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let auto_renew = renewal_count > 0;

        let request = EscrowCreateRequest {
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers,
            auto_renew,
            renewal_count,
            buyer_inactivity_secs,
            min_lock_until: None,
            release_base: None,
            release_quote: None,
            release_comparison: None,
            release_threshold_price: None,
            arbiter_fee_bps: None,
        };

        Self::create_escrow_core(&env, &buyer, request)
    }

    /// Create a new escrow with optional lock, oracle condition, and per-escrow arbiter fee.
    pub fn create_escrow_v2(env: Env, buyer: Address, request: EscrowCreateRequest) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        Self::create_escrow_core(&env, &buyer, request)
    }

    /// Create up to 10 escrows in one atomic transaction.
    /// Returns the list of contiguous escrow IDs created.
    pub fn create_escrows_batch(
        env: Env,
        buyer: Address,
        escrow_configs: Vec<EscrowBatchConfig>,
    ) -> Vec<u32> {
        Self::require_not_paused(&env);
        buyer.require_auth();

        if escrow_configs.is_empty() {
            panic!("Batch must contain at least one escrow config");
        }
        if escrow_configs.len() > MAX_BATCH_ESCROWS {
            panic!("Batch size exceeds maximum of 10 escrows");
        }

        let mut created_ids: Vec<u32> = Vec::new(&env);
        let mut first_id: Option<u32> = None;
        let mut last_id: u32 = 0;

        for i in 0..escrow_configs.len() {
            let cfg = escrow_configs.get(i).unwrap();
            let escrow_id = Self::create_escrow_core(
                &env,
                &buyer,
                EscrowCreateRequest {
                    seller: cfg.seller,
                    arbiter: cfg.arbiter,
                    amount: cfg.amount,
                    token: cfg.token,
                    deadline: cfg.deadline,
                    metadata_hash: cfg.metadata_hash,
                    sellers: cfg.sellers,
                    auto_renew: cfg.auto_renew,
                    renewal_count: cfg.renewal_count,
                    buyer_inactivity_secs: 0,
                    min_lock_until: None,
                    release_base: None,
                    release_quote: None,
                    release_comparison: None,
                    release_threshold_price: None,
                    arbiter_fee_bps: None,
                },
            );

            if first_id.is_none() {
                first_id = Some(escrow_id);
            }
            last_id = escrow_id;
            created_ids.push_back(escrow_id);
        }

        events::emit_batch_escrow_created(
            &env,
            escrow_configs.len(),
            first_id.expect("Batch contained no escrows"),
            last_id,
        );

        created_ids
    }

    fn create_escrow_core(env: &Env, buyer: &Address, request: EscrowCreateRequest) -> u32 {
        let EscrowCreateRequest {
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers,
            auto_renew,
            renewal_count,
            buyer_inactivity_secs,
            min_lock_until,
            release_base,
            release_quote,
            release_comparison,
            release_threshold_price,
            arbiter_fee_bps,
        } = request;

        if amount <= 0 {
            panic!("Escrow amount must be positive");
        }

        if deadline <= env.ledger().timestamp() {
            panic!("Deadline must be in the future");
        }

        if let Some(lock_until) = min_lock_until {
            if deadline <= lock_until {
                panic!("Deadline must be after min_lock_until");
            }
        }

        if let Some(fee_bps) = arbiter_fee_bps {
            if fee_bps > MAX_ARBITER_FEE_BPS {
                panic!("Arbiter fee exceeds maximum of 1000 bps");
            }
        }

        let has_any_release_condition = release_base.is_some()
            || release_quote.is_some()
            || release_comparison.is_some()
            || release_threshold_price.is_some();
        let has_full_release_condition = release_base.is_some()
            && release_quote.is_some()
            && release_comparison.is_some()
            && release_threshold_price.is_some();
        if has_any_release_condition && !has_full_release_condition {
            panic!("Incomplete release condition");
        }

        if let Some(threshold) = release_threshold_price {
            if threshold <= 0 {
                panic!("Release condition threshold must be positive");
            }
        }

        if let Some(comparison) = release_comparison {
            if comparison != ORACLE_COMPARISON_LESS_OR_EQUAL
                && comparison != ORACLE_COMPARISON_GREATER_OR_EQUAL
            {
                panic!("Invalid release comparison");
            }
        }

        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }

        // Validate multi-party sellers if provided
        let resolved_sellers: Vec<(Address, u32)> = if sellers.is_empty() {
            // Single-seller mode: wrap seller with 10000 bps
            let mut v = Vec::new(env);
            v.push_back((seller.clone(), 10_000u32));
            v
        } else {
            if sellers.len() > 5 {
                panic!("Maximum 5 sellers allowed");
            }
            let mut total_bps: u32 = 0;
            for i in 0..sellers.len() {
                let (_, bps) = sellers.get(i).unwrap();
                total_bps += bps;
            }
            if total_bps != 10_000 {
                panic!("Seller allocations must sum to 10000 bps");
            }
            sellers
        };

        // Transfer tokens from buyer to contract (escrow)
        let client = token::Client::new(env, &token);
        client.transfer(buyer, &env.current_contract_address(), &amount);

        let escrow_id = Self::next_escrow_id(env);

        // Primary seller is the first in the list (or the passed seller for single-party)
        let primary_seller = if resolved_sellers.len() == 1 {
            resolved_sellers.get(0).unwrap().0
        } else {
            resolved_sellers.get(0).unwrap().0
        };

        let now = env.ledger().timestamp();

        let escrow = Escrow {
            id: escrow_id,
            buyer: buyer.clone(),
            seller: primary_seller.clone(),
            arbiter: arbiter.clone(),
            amount,
            token: token.clone(),
            status: EscrowStatus::Active,
            created_at: now,
            deadline,
            metadata_hash: metadata_hash.clone(),
            sellers: resolved_sellers.clone(),
            extensions: EscrowExtensions {
                auto_renew,
                renewal_count,
                renewals_remaining: if renewal_count == 0 { 0 } else { renewal_count },
                dispute_timeout_seconds: None,
                buyer_inactivity_secs,
                min_lock_until,
                release_base,
                release_quote,
                release_comparison,
                release_threshold_price,
                arbiter_fee_bps,
            },
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // #150: Initialize LastBuyerAction to creation timestamp
        if buyer_inactivity_secs > 0 {
            env.storage()
                .persistent()
                .set(&DataKey::LastBuyerAction(escrow_id), &now);
            env.storage().persistent().extend_ttl(
                &DataKey::LastBuyerAction(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        // Store metadata separately with timestamp if provided
        if let Some(ref hash) = metadata_hash {
            env.storage().persistent().set(
                &DataKey::EscrowMetadata(escrow_id),
                &(hash.clone(), env.ledger().timestamp()),
            );
            env.storage().persistent().extend_ttl(
                &DataKey::EscrowMetadata(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        events::emit_escrow_created(
            env,
            escrow_id,
            buyer.clone(),
            primary_seller,
            arbiter,
            amount,
            token,
            deadline,
        );

        // Emit multi-party event if more than one seller
        if resolved_sellers.len() > 1 {
            events::emit_multi_party_escrow_created(env, escrow_id, resolved_sellers.len());
        }

        if let Some(lock_until) = escrow.extensions.min_lock_until {
            events::emit_escrow_time_locked(env, escrow_id, lock_until);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        escrow_id
    }

    /// Create a new escrow with an explicit per-escrow dispute timeout override.
    /// Uses renewal_count > 0 to enable auto_renew while staying within Soroban's
    /// 10-parameter contract function limit.
    pub fn create_escrow_w_timeout(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        amount: i128,
        token: Address,
        deadline: u64,
        metadata_hash: Option<BytesN<32>>,
        sellers: Vec<(Address, u32)>,
        renewal_count: u32,
        dispute_timeout_seconds: u64,
    ) -> u32 {
        if dispute_timeout_seconds == 0 {
            panic!("dispute_timeout_seconds must be positive");
        }

        let auto_renew = renewal_count > 0;
        let escrow_id = Self::create_escrow(
            env.clone(),
            buyer,
            seller,
            arbiter,
            amount,
            token,
            deadline,
            metadata_hash,
            sellers,
            auto_renew,
            renewal_count,
        );

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        escrow.extensions.dispute_timeout_seconds = Some(dispute_timeout_seconds);

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        escrow_id
    }

    /// Release escrowed funds to seller. Can be called by buyer or arbiter.
    pub fn release_escrow(env: Env, caller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        let renewal_source = escrow.clone();

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if caller != escrow.buyer && caller != escrow.arbiter {
            panic!("Only buyer or arbiter can release escrow");
        }

        Self::require_unlocked(&env, &escrow);

        // #150: Track buyer activity
        if caller == escrow.buyer {
            Self::update_last_buyer_action(&env, &escrow);
        }

        let total = escrow.amount;

        Self::transfer_to_sellers(&env, &escrow, total, escrow_id);

        escrow.status = EscrowStatus::Released;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        Self::try_auto_renew(&env, escrow_id, &renewal_source);
    }

    /// Submit evidence hash anchors for an escrow dispute workflow.
    pub fn submit_evidence(
        env: Env,
        party: Address,
        escrow_id: u32,
        evidence_hash: BytesN<32>,
        evidence_uri_hash: BytesN<32>,
    ) {
        Self::require_not_paused(&env);
        party.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if party != escrow.buyer && party != escrow.seller {
            panic!("Only buyer or seller can submit evidence");
        }

        // #150: Track buyer activity
        if party == escrow.buyer {
            Self::update_last_buyer_action(&env, &escrow);
        }

        let key = DataKey::Evidence(escrow_id, party.clone());
        let mut entries: Vec<EvidenceSubmission> = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or(Vec::new(&env));

        if entries.len() >= MAX_EVIDENCE_ENTRIES_PER_PARTY {
            panic!("Maximum evidence entries reached for this party");
        }

        entries.push_back(EvidenceSubmission {
            evidence_hash: evidence_hash.clone(),
            evidence_uri_hash,
            submitted_at: env.ledger().timestamp(),
        });

        env.storage().persistent().set(&key, &entries);
        env.storage().persistent().extend_ttl(
            &key,
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_evidence_submitted(&env, escrow_id, party, evidence_hash);
    }

    /// Returns evidence submissions for buyer and seller in one call.
    pub fn get_evidence(env: Env, escrow_id: u32) -> Vec<(Address, Vec<EvidenceSubmission>)> {
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        let mut all: Vec<(Address, Vec<EvidenceSubmission>)> = Vec::new(&env);

        let buyer_key = DataKey::Evidence(escrow_id, escrow.buyer.clone());
        let buyer_entries: Vec<EvidenceSubmission> = env
            .storage()
            .persistent()
            .get(&buyer_key)
            .unwrap_or(Vec::new(&env));
        if !buyer_entries.is_empty() {
            all.push_back((escrow.buyer.clone(), buyer_entries));
        }

        let seller_key = DataKey::Evidence(escrow_id, escrow.seller.clone());
        let seller_entries: Vec<EvidenceSubmission> = env
            .storage()
            .persistent()
            .get(&seller_key)
            .unwrap_or(Vec::new(&env));
        if !seller_entries.is_empty() {
            all.push_back((escrow.seller, seller_entries));
        }

        all
    }

    /// Buyer pre-approves renewal cycles for a specific escrow chain.
    pub fn set_renewal_allowance(env: Env, buyer: Address, escrow_id: u32, total_renewals: u32) {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if buyer != escrow.buyer {
            panic!("Only buyer can set renewal allowance");
        }

        if !escrow.extensions.auto_renew {
            panic!("Auto-renew is not enabled for this escrow");
        }

        let total_amount = escrow.amount * total_renewals as i128;
        let expiration_ledger = env.ledger().sequence().saturating_add(100_000);
        let token_client = token::Client::new(&env, &escrow.token);
        token_client.approve(
            &buyer,
            &env.current_contract_address(),
            &total_amount,
            &expiration_ledger,
        );

        env.storage()
            .persistent()
            .set(&DataKey::RenewalAllowance(escrow_id), &total_renewals);
        env.storage().persistent().extend_ttl(
            &DataKey::RenewalAllowance(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    /// Buyer can disable auto-renew at any time.
    pub fn cancel_auto_renew(env: Env, buyer: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if buyer != escrow.buyer {
            panic!("Only buyer can cancel auto-renew");
        }

        escrow.extensions.auto_renew = false;
        escrow.extensions.renewals_remaining = 0;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().remove(&DataKey::RenewalAllowance(escrow_id));
    }

    /// Release part of the escrowed funds to seller. Can be called by buyer or arbiter.
    pub fn partial_release(env: Env, caller: Address, escrow_id: u32, release_amount: i128) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if caller != escrow.buyer && caller != escrow.arbiter {
            panic!("Only buyer or arbiter can release escrow");
        }

        Self::require_unlocked(&env, &escrow);

        if release_amount <= 0 {
            panic!("Release amount must be positive");
        }

        if release_amount > escrow.amount {
            panic!("Release amount exceeds escrow balance");
        }

        let client = token::Client::new(&env, &escrow.token);
        client.transfer(
            &env.current_contract_address(),
            &escrow.seller,
            &release_amount,
        );

        escrow.amount -= release_amount;
        escrow.status = if escrow.amount == 0 {
            EscrowStatus::Released
        } else {
            EscrowStatus::PartiallyReleased
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_partial_released(&env, escrow_id, release_amount, escrow.amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- Milestone-based escrow release (#136) ---

    /// Create a milestone-based escrow.
    ///
    /// `milestones[i].amount` must be positive and the sum of all milestone
    /// amounts must equal `total_amount`. Tokens for `total_amount` are
    /// transferred from `buyer` into the contract up front; each milestone is
    /// released independently via `approve_milestone`. When all milestones are
    /// approved, the escrow transitions automatically to `Released`.
    pub fn create_milestone_escrow(
        env: Env,
        buyer: Address,
        seller: Address,
        arbiter: Address,
        token: Address,
        deadline: u64,
        milestones: Vec<Milestone>,
    ) -> u32 {
        buyer.require_auth();

        if milestones.is_empty() {
            panic!("At least one milestone required");
        }
        if milestones.len() > MAX_MILESTONES {
            panic!("Too many milestones");
        }

        let mut total_amount: i128 = 0;
        for i in 0..milestones.len() {
            let m = milestones.get(i).unwrap();
            if m.amount <= 0 {
                panic!("Milestone amount must be positive");
            }
            if m.status != MilestoneStatus::Pending {
                panic!("New milestones must start as Pending");
            }
            total_amount += m.amount;
        }

        let request = EscrowCreateRequest {
            seller,
            arbiter,
            amount: total_amount,
            token,
            deadline,
            metadata_hash: None,
            sellers: Vec::new(&env),
            auto_renew: false,
            renewal_count: 0,
            buyer_inactivity_secs: 0,
            min_lock_until: None,
            release_base: None,
            release_quote: None,
            release_comparison: None,
            release_threshold_price: None,
            arbiter_fee_bps: None,
        };
        let escrow_id = Self::create_escrow_core(&env, &buyer, request);

        env.storage()
            .persistent()
            .set(&DataKey::EscrowMilestones(escrow_id), &milestones);
        env.storage().persistent().extend_ttl(
            &DataKey::EscrowMilestones(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        escrow_id
    }

    /// Approve a single milestone, releasing its `amount` to the seller.
    ///
    /// Callable by buyer or arbiter. The targeted milestone must be `Pending`.
    /// Disputed milestones do not block other milestones from being approved.
    /// When every milestone reaches a terminal state and at least one was
    /// approved, the escrow auto-transitions to `Released`.
    pub fn approve_milestone(
        env: Env,
        caller: Address,
        escrow_id: u32,
        milestone_index: u32,
    ) {
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status == EscrowStatus::Released
            || escrow.status == EscrowStatus::Refunded
            || escrow.status == EscrowStatus::Resolved
        {
            panic!("Escrow already terminal");
        }

        if caller != escrow.buyer && caller != escrow.arbiter {
            panic!("Only buyer or arbiter can approve milestones");
        }

        let mut milestones: Vec<Milestone> = env
            .storage()
            .persistent()
            .get(&DataKey::EscrowMilestones(escrow_id))
            .expect("Escrow has no milestones");

        if milestone_index >= milestones.len() {
            panic!("Milestone index out of range");
        }

        let mut milestone = milestones.get(milestone_index).unwrap();
        if milestone.status != MilestoneStatus::Pending {
            panic!("Milestone not pending");
        }

        let client = token::Client::new(&env, &escrow.token);
        client.transfer(&env.current_contract_address(), &escrow.seller, &milestone.amount);

        let amount_released = milestone.amount;
        milestone.status = MilestoneStatus::Approved;
        milestones.set(milestone_index, milestone);

        env.storage()
            .persistent()
            .set(&DataKey::EscrowMilestones(escrow_id), &milestones);
        env.storage().persistent().extend_ttl(
            &DataKey::EscrowMilestones(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Auto-transition to Released when every milestone is in a terminal
        // state and at least one was approved.
        let mut all_terminal = true;
        let mut any_approved = false;
        for i in 0..milestones.len() {
            let m = milestones.get(i).unwrap();
            match m.status {
                MilestoneStatus::Pending => all_terminal = false,
                MilestoneStatus::Approved => any_approved = true,
                MilestoneStatus::Disputed => {}
            }
        }
        if all_terminal && any_approved && escrow.status == EscrowStatus::Active {
            escrow.status = EscrowStatus::Released;
            env.storage()
                .persistent()
                .set(&DataKey::Escrow(escrow_id), &escrow);
            env.storage().persistent().extend_ttl(
                &DataKey::Escrow(escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        events::emit_milestone_approved(&env, escrow_id, milestone_index, amount_released);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Read the current milestone schedule for a milestone-based escrow.
    pub fn get_milestones(env: Env, escrow_id: u32) -> Vec<Milestone> {
        env.storage()
            .persistent()
            .get(&DataKey::EscrowMilestones(escrow_id))
            .expect("Escrow has no milestones")
    }

    /// Dispute an escrow. Can be called by buyer or seller.
    /// Pass `dispute_amount` equal to the full escrow amount for a full dispute,
    /// or less for a partial dispute (undisputed portion is released to seller immediately).
    pub fn dispute_escrow(
        env: Env,
        caller: Address,
        escrow_id: u32,
        reason: String,
        dispute_amount: i128,
    ) {
        Self::require_not_paused(&env);
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can dispute escrow");
        }

        if dispute_amount <= 0 || dispute_amount > escrow.amount {
            panic!("dispute_amount must be > 0 and <= escrow amount");
        }

        // #150: Track buyer activity
        if caller == escrow.buyer {
            Self::update_last_buyer_action(&env, &escrow);
        }

        let released_amount = escrow.amount - dispute_amount;

        // Release undisputed portion to seller immediately
        if released_amount > 0 {
            let client = token::Client::new(&env, &escrow.token);
            client.transfer(
                &env.current_contract_address(),
                &escrow.seller,
                &released_amount,
            );
        }

        escrow.amount = dispute_amount;
        escrow.status = if released_amount > 0 {
            EscrowStatus::PartiallyDisputed
        } else {
            EscrowStatus::Disputed
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        let dispute = Dispute {
            escrow_id,
            reason: reason.clone(),
            created_at: env.ledger().timestamp(),
            resolved: false,
            dispute_amount,
            timeout_seconds: escrow.extensions.dispute_timeout_seconds,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Dispute(escrow_id), &dispute);
        env.storage().persistent().extend_ttl(
            &DataKey::Dispute(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        if released_amount > 0 {
            events::emit_partial_dispute_raised(&env, escrow_id, dispute_amount, released_amount);
        } else {
            events::emit_escrow_disputed(&env, escrow_id, caller, reason);
        }

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Resolve a dispute. Only arbiter can call this.
    pub fn resolve_dispute(env: Env, arbiter: Address, escrow_id: u32, release_to_seller: bool) {
        Self::require_not_paused(&env);
        arbiter.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::Disputed
            && escrow.status != EscrowStatus::PartiallyDisputed
        {
            panic!("Escrow is not disputed");
        }

        if arbiter != escrow.arbiter {
            panic!("Only arbiter can resolve dispute");
        }

        let client = token::Client::new(&env, &escrow.token);

        let arbiter_fee_bps = Self::effective_arbiter_fee_bps(&env, &escrow);
        let arbiter_fee = (escrow.amount * arbiter_fee_bps as i128) / 10_000;

        if arbiter_fee > 0 {
            client.transfer(&env.current_contract_address(), &arbiter, &arbiter_fee);
            events::emit_arbiter_fee_paid(&env, escrow_id, arbiter.clone(), arbiter_fee);
        }

        // Compute and deduct protocol fee
        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0);
        let protocol_fee = (escrow.amount * fee_bps as i128) / 10_000;

        if protocol_fee > 0 {
            let fee_recipient: Address = env
                .storage()
                .instance()
                .get(&DataKey::FeeRecipient)
                .expect("FeeRecipient not set");
            client.transfer(
                &env.current_contract_address(),
                &fee_recipient,
                &protocol_fee,
            );
            events::emit_protocol_fee_paid(&env, escrow_id, protocol_fee, fee_recipient);
        }

        let winner_amount = escrow.amount - protocol_fee - arbiter_fee;

        if winner_amount < 0 {
            panic!("Fee configuration exceeds escrow amount");
        }

        if release_to_seller {
            client.transfer(
                &env.current_contract_address(),
                &escrow.seller,
                &winner_amount,
            );
            escrow.status = EscrowStatus::Released;
        } else {
            client.transfer(
                &env.current_contract_address(),
                &escrow.buyer,
                &winner_amount,
            );
            escrow.status = EscrowStatus::Refunded;
        }

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        // Mark dispute as resolved
        if let Some(mut dispute) = env
            .storage()
            .persistent()
            .get::<DataKey, Dispute>(&DataKey::Dispute(escrow_id))
        {
            dispute.resolved = true;
            env.storage()
                .persistent()
                .set(&DataKey::Dispute(escrow_id), &dispute);
        }

        events::emit_dispute_resolved(&env, escrow_id, release_to_seller, arbiter);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns true when an unresolved dispute has exceeded its timeout window.
    /// Uses per-escrow dispute timeout when set, otherwise falls back to admin default.
    pub fn check_escalation(env: Env, escrow_id: u32) -> bool {
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::Disputed && escrow.status != EscrowStatus::PartiallyDisputed {
            return false;
        }

        let dispute: Dispute = env
            .storage()
            .persistent()
            .get(&DataKey::Dispute(escrow_id))
            .expect("Dispute not found");

        if dispute.resolved {
            return false;
        }

        let default_timeout: u64 = env
            .storage()
            .instance()
            .get(&DataKey::DefaultDisputeTimeout)
            .unwrap_or(DEFAULT_DISPUTE_TIMEOUT_SECONDS);
        let effective_timeout = dispute.timeout_seconds.unwrap_or(default_timeout);

        let elapsed = env.ledger().timestamp() - dispute.created_at;
        if elapsed > effective_timeout {
            events::emit_dispute_escalated(&env, escrow_id, effective_timeout);
            return true;
        }

        false
    }

    /// Admin updates the default dispute escalation timeout (seconds).
    pub fn update_default_dispute_timeout(env: Env, admin: Address, timeout_seconds: u64) {
        Self::require_admin(&env, &admin);
        if timeout_seconds == 0 {
            panic!("Timeout must be positive");
        }

        env.storage()
            .instance()
            .set(&DataKey::DefaultDisputeTimeout, &timeout_seconds);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_default_dispute_timeout(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::DefaultDisputeTimeout)
            .unwrap_or(DEFAULT_DISPUTE_TIMEOUT_SECONDS)
    }

    /// Set the protocol-wide default arbiter fee (bps). Admin only.
    /// Fee cap is 1000 bps (10%).
    pub fn set_default_arbiter_fee_bps(env: Env, admin: Address, fee_bps: u32) {
        Self::require_admin(&env, &admin);
        if fee_bps > MAX_ARBITER_FEE_BPS {
            panic!("Arbiter fee exceeds maximum of 1000 bps");
        }

        env.storage()
            .instance()
            .set(&DataKey::DefaultArbiterFeeBps, &fee_bps);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_default_arbiter_fee_bps(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::DefaultArbiterFeeBps)
            .unwrap_or(0)
    }

    /// Configure oracle address and max accepted price age (seconds). Admin only.
    pub fn set_oracle(env: Env, admin: Address, oracle: Address, max_oracle_age: u64) {
        Self::require_admin(&env, &admin);
        if max_oracle_age == 0 {
            panic!("max_oracle_age must be positive");
        }

        env.storage().instance().set(&DataKey::OracleAddress, &oracle);
        env.storage()
            .instance()
            .set(&DataKey::MaxOracleAge, &max_oracle_age);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_oracle_address(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::OracleAddress)
            .expect("Oracle not configured")
    }

    pub fn get_max_oracle_age(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MaxOracleAge)
            .unwrap_or(DEFAULT_MAX_ORACLE_AGE_SECONDS)
    }

    /// Configure insurance token and trigger window in days. Admin only.
    pub fn set_insurance_config(env: Env, admin: Address, token: Address, trigger_days: u64) {
        Self::require_admin(&env, &admin);
        if trigger_days == 0 {
            panic!("insurance_trigger_days must be positive");
        }

        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }

        env.storage().instance().set(&DataKey::InsuranceToken, &token);
        env.storage()
            .instance()
            .set(&DataKey::InsuranceTriggerDays, &trigger_days);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    pub fn get_insurance_pool(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::InsurancePool)
            .unwrap_or(0)
    }

    /// Open contribution flow for the insurance pool.
    pub fn contribute_to_insurance(env: Env, contributor: Address, amount: i128) {
        Self::require_not_paused(&env);
        contributor.require_auth();

        if amount <= 0 {
            panic!("Insurance contribution must be positive");
        }

        let token: Address = env
            .storage()
            .instance()
            .get(&DataKey::InsuranceToken)
            .expect("Insurance token not configured");

        let client = token::Client::new(&env, &token);
        client.transfer(&contributor, &env.current_contract_address(), &amount);

        let current_pool: i128 = env
            .storage()
            .instance()
            .get(&DataKey::InsurancePool)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::InsurancePool, &(current_pool + amount));
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Admin confirms or clears inactivity confirmation for a disputed escrow.
    pub fn confirm_insurance_inactivity(
        env: Env,
        admin: Address,
        escrow_id: u32,
        confirmed: bool,
    ) {
        Self::require_admin(&env, &admin);
        env.storage()
            .persistent()
            .set(&DataKey::InsuranceAdminConfirmed(escrow_id), &confirmed);
        env.storage().persistent().extend_ttl(
            &DataKey::InsuranceAdminConfirmed(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    /// Claim insurance for unresolved disputes after admin-confirmed inactivity.
    /// The claim amount is capped at 50% of the disputed amount and pool balance.
    pub fn claim_insurance(env: Env, claimant: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        claimant.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if claimant != escrow.buyer && claimant != escrow.seller {
            panic!("Only buyer or seller can claim insurance");
        }

        if escrow.status != EscrowStatus::Disputed
            && escrow.status != EscrowStatus::PartiallyDisputed
        {
            panic!("Escrow is not disputed");
        }

        let dispute: Dispute = env
            .storage()
            .persistent()
            .get(&DataKey::Dispute(escrow_id))
            .expect("Dispute not found");
        if dispute.resolved {
            panic!("Dispute already resolved");
        }

        if env
            .storage()
            .persistent()
            .get::<DataKey, bool>(&DataKey::InsuranceClaimed(escrow_id))
            .unwrap_or(false)
        {
            panic!("Insurance already claimed");
        }

        let confirmed: bool = env
            .storage()
            .persistent()
            .get(&DataKey::InsuranceAdminConfirmed(escrow_id))
            .unwrap_or(false);
        if !confirmed {
            panic!("Admin confirmation required");
        }

        let trigger_days: u64 = env
            .storage()
            .instance()
            .get(&DataKey::InsuranceTriggerDays)
            .unwrap_or(DEFAULT_INSURANCE_TRIGGER_DAYS);
        let trigger_seconds = trigger_days.saturating_mul(24 * 60 * 60);
        let elapsed = env.ledger().timestamp().saturating_sub(dispute.created_at);
        if elapsed < trigger_seconds {
            panic!("Insurance trigger period not reached");
        }

        let insurance_token: Address = env
            .storage()
            .instance()
            .get(&DataKey::InsuranceToken)
            .expect("Insurance token not configured");
        if insurance_token != escrow.token {
            panic!("Escrow token not covered by insurance pool");
        }

        let max_claim = escrow.amount / 2;
        let pool_balance: i128 = env
            .storage()
            .instance()
            .get(&DataKey::InsurancePool)
            .unwrap_or(0);
        let claim_amount = if pool_balance < max_claim {
            pool_balance
        } else {
            max_claim
        };

        if claim_amount <= 0 {
            panic!("Insurance pool has insufficient balance");
        }

        let token_client = token::Client::new(&env, &insurance_token);
        token_client.transfer(&env.current_contract_address(), &claimant, &claim_amount);

        env.storage()
            .instance()
            .set(&DataKey::InsurancePool, &(pool_balance - claim_amount));
        env.storage()
            .persistent()
            .set(&DataKey::InsuranceClaimed(escrow_id), &true);
        env.storage().persistent().extend_ttl(
            &DataKey::InsuranceClaimed(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_insurance_claimed(&env, escrow_id, claimant, claim_amount);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Set protocol fee in basis points and fee recipient. Admin only.
    /// Max fee is 200 bps (2%).
    pub fn update_protocol_fee(env: Env, admin: Address, fee_bps: u32, fee_recipient: Address) {
        Self::require_admin(&env, &admin);
        if fee_bps > MAX_PROTOCOL_FEE_BPS {
            panic!("Fee exceeds maximum of 200 bps");
        }
        env.storage()
            .instance()
            .set(&DataKey::ProtocolFeeBps, &fee_bps);
        env.storage()
            .instance()
            .set(&DataKey::FeeRecipient, &fee_recipient);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get current protocol fee bps and fee recipient.
    pub fn get_protocol_fee(env: Env) -> (u32, Option<Address>) {
        let fee_bps: u32 = env
            .storage()
            .instance()
            .get(&DataKey::ProtocolFeeBps)
            .unwrap_or(0);
        let fee_recipient: Option<Address> = env.storage().instance().get(&DataKey::FeeRecipient);
        (fee_bps, fee_recipient)
    }

    /// Anyone may trigger release when the escrow's oracle condition is met.
    pub fn check_and_release_escrow(env: Env, escrow_id: u32) {
        Self::require_not_paused(&env);

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        let base = escrow
            .extensions
            .release_base
            .clone()
            .expect("No release condition set");
        let quote = escrow
            .extensions
            .release_quote
            .clone()
            .expect("No release condition set");
        let comparison = escrow
            .extensions
            .release_comparison
            .expect("No release condition set");
        let threshold_price = escrow
            .extensions
            .release_threshold_price
            .expect("No release condition set");

        let price_data = Self::get_oracle_price(&env, &base, &quote);
        let condition_met = match comparison {
            ORACLE_COMPARISON_LESS_OR_EQUAL => price_data.price <= threshold_price,
            ORACLE_COMPARISON_GREATER_OR_EQUAL => price_data.price >= threshold_price,
            _ => panic!("Invalid release comparison"),
        };

        if !condition_met {
            panic!("Release condition not met");
        }

        Self::transfer_to_sellers(&env, &escrow, escrow.amount, escrow_id);
        escrow.status = EscrowStatus::Released;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_oracle_release_triggered(
            &env,
            escrow_id,
            price_data.price,
            base,
            quote,
            comparison,
            threshold_price,
        );
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Auto-release expired escrow (past deadline, undisputed). Can be called by buyer.
    pub fn auto_release_expired(env: Env, escrow_id: u32) {
        Self::require_not_paused(&env);
        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if env.ledger().timestamp() <= escrow.deadline {
            panic!("Escrow has not expired yet");
        }

        Self::require_unlocked(&env, &escrow);

        escrow.buyer.require_auth();

        let client = token::Client::new(&env, &escrow.token);
        client.transfer(
            &env.current_contract_address(),
            &escrow.buyer,
            &escrow.amount,
        );

        escrow.status = EscrowStatus::Refunded;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_refunded(&env, escrow_id, escrow.buyer, escrow.amount);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Propose a new deadline for an active escrow.
    /// Only buyer or seller may propose and the proposal requires counterparty acceptance.
    pub fn propose_deadline_extension(
        env: Env,
        caller: Address,
        escrow_id: u32,
        new_deadline: u64,
    ) {
        caller.require_auth();

        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can propose deadline extension");
        }

        if escrow.status == EscrowStatus::Disputed || Self::is_terminal_escrow_status(escrow.status)
        {
            panic!("Cannot extend deadline while escrow is disputed");
        }

        if new_deadline <= escrow.deadline {
            panic!("New deadline must be greater than current deadline");
        }

        let proposal = DeadlineProposal {
            proposer: caller.clone(),
            new_deadline,
            proposed_at: env.ledger().timestamp(),
        };

        env.storage()
            .persistent()
            .set(&DataKey::DeadlineProposal(escrow_id), &proposal);
        env.storage().persistent().extend_ttl(
            &DataKey::DeadlineProposal(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_deadline_extension_proposed(
            &env,
            escrow_id,
            caller,
            proposal.new_deadline,
            proposal.proposed_at,
        );

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Accept a pending deadline extension proposed by the counterparty.
    pub fn accept_deadline_extension(env: Env, caller: Address, escrow_id: u32) {
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can accept deadline extension");
        }

        if escrow.status == EscrowStatus::Disputed || Self::is_terminal_escrow_status(escrow.status)
        {
            panic!("Cannot extend deadline while escrow is disputed");
        }

        let proposal: DeadlineProposal = env
            .storage()
            .persistent()
            .get(&DataKey::DeadlineProposal(escrow_id))
            .expect("No deadline extension proposal found");

        if caller == proposal.proposer {
            panic!("Proposer cannot accept their own deadline extension");
        }

        let now = env.ledger().timestamp();
        if now > proposal.proposed_at + DEADLINE_EXTENSION_PROPOSAL_WINDOW {
            env.storage()
                .persistent()
                .remove(&DataKey::DeadlineProposal(escrow_id));
            panic!("Deadline extension proposal has expired");
        }

        if proposal.new_deadline <= escrow.deadline {
            env.storage()
                .persistent()
                .remove(&DataKey::DeadlineProposal(escrow_id));
            panic!("New deadline must be greater than current deadline");
        }

        let old_deadline = escrow.deadline;
        escrow.deadline = proposal.new_deadline;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage()
            .persistent()
            .remove(&DataKey::DeadlineProposal(escrow_id));

        events::emit_deadline_extended(&env, escrow_id, old_deadline, escrow.deadline);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get escrow details
    pub fn get_escrow(env: Env, escrow_id: u32) -> Escrow {
        env.storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found")
    }

    /// Transfer buyer role in an active escrow to a new address.
    pub fn transfer_buyer_role(
        env: Env,
        current_buyer: Address,
        escrow_id: u32,
        new_buyer: Address,
    ) {
        Self::require_not_paused(&env);
        current_buyer.require_auth();

        if current_buyer == new_buyer {
            panic!("New buyer must be different from current buyer");
        }

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.status != EscrowStatus::Active {
            panic!("Buyer transfer only allowed for active escrows");
        }

        if escrow.buyer != current_buyer {
            panic!("Only current buyer can transfer buyer role");
        }

        let old_buyer = escrow.buyer.clone();
        escrow.buyer = new_buyer.clone();

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_buyer_role_transferred(&env, escrow_id, old_buyer, new_buyer);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Update metadata hash for an escrow. Requires auth from buyer or seller.
    pub fn update_metadata(
        env: Env,
        caller: Address,
        escrow_id: u32,
        new_hash: BytesN<32>,
    ) {
        caller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if caller != escrow.buyer && caller != escrow.seller {
            panic!("Only buyer or seller can update metadata");
        }

        // #150: Track buyer activity
        if caller == escrow.buyer {
            Self::update_last_buyer_action(&env, &escrow);
        }

        escrow.metadata_hash = Some(new_hash.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);

        env.storage().persistent().set(
            &DataKey::EscrowMetadata(escrow_id),
            &(new_hash.clone(), env.ledger().timestamp()),
        );
        env.storage().persistent().extend_ttl(
            &DataKey::EscrowMetadata(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_metadata_updated(&env, escrow_id, new_hash, caller);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get the latest metadata hash for an escrow.
    pub fn get_metadata_hash(env: Env, escrow_id: u32) -> Option<BytesN<32>> {
        let escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");
        escrow.metadata_hash
    }

    /// Get dispute details
    pub fn get_dispute(env: Env, escrow_id: u32) -> Dispute {
        env.storage()
            .persistent()
            .get(&DataKey::Dispute(escrow_id))
            .expect("No dispute found for this escrow")
    }

    /// Get escrow counter
    pub fn get_escrow_counter(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::EscrowCounter)
            .unwrap_or(0)
    }

    /// Upgrade this contract's WASM code. Admin only.
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: BytesN<32>) {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can upgrade contract");
        }

        let old_version = Self::get_or_init_version(&env);
        env.deployer().update_current_contract_wasm(new_wasm_hash);

        let new_version = old_version.checked_add(1).expect("Version overflow");
        env.storage()
            .instance()
            .set(&DataKey::ContractVersion, &new_version);

        events::emit_contract_upgraded(&env, old_version, new_version, admin);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Run one-time migration logic for the current version. Admin only.
    pub fn migrate(env: Env, admin: Address) {
        admin.require_auth();

        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Not initialized");
        if admin != stored_admin {
            panic!("Only admin can migrate contract");
        }

        let version = Self::get_or_init_version(&env);
        if env
            .storage()
            .instance()
            .get(&DataKey::MigrationCompleted(version))
            .unwrap_or(false)
        {
            panic!("Migration already completed for this version");
        }

        env.storage()
            .instance()
            .set(&DataKey::MigrationCompleted(version), &true);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Returns the current contract version.
    pub fn get_version(env: Env) -> u32 {
        Self::get_or_init_version(&env)
    }

    pub fn pause_contract(env: Env, admin: Address, reason: String) {
        Self::require_or_bootstrap_admin(&env, &admin);

        if Self::is_paused(env.clone()) {
            panic!("Contract already paused");
        }

        env.storage().instance().set(&DataKey::Paused, &true);
        env.storage().instance().set(&DataKey::PauseReason, &reason);

        events::emit_contract_paused(&env, admin, reason, env.ledger().timestamp());
    }

    pub fn resume_contract(env: Env, admin: Address) {
        Self::require_admin(&env, &admin);

        if !Self::is_paused(env.clone()) {
            panic!("Contract is not paused");
        }

        env.storage().instance().set(&DataKey::Paused, &false);
        env.storage().instance().remove(&DataKey::PauseReason);

        events::emit_contract_resumed(&env, admin, env.ledger().timestamp());
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    pub fn get_pause_reason(env: Env) -> String {
        env.storage()
            .instance()
            .get(&DataKey::PauseReason)
            .unwrap_or(String::from_str(&env, ""))
    }

    /// Add a token to the allowlist. Admin only.
    pub fn add_allowed_token(env: Env, admin: Address, token: Address) {
        Self::require_admin(&env, &admin);
        env.storage()
            .instance()
            .set(&DataKey::AllowedToken(token.clone()), &true);
        events::emit_token_allowlisted(&env, admin, token);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Remove a token from the allowlist. Admin only.
    pub fn remove_allowed_token(env: Env, admin: Address, token: Address) {
        Self::require_admin(&env, &admin);
        env.storage()
            .instance()
            .remove(&DataKey::AllowedToken(token.clone()));
        events::emit_token_removed_from_allowlist(&env, admin, token);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Create a reusable escrow template. Returns the template ID.
    pub fn create_escrow_template(
        env: Env,
        creator: Address,
        config: EscrowTemplateConfig,
    ) -> u32 {
        creator.require_auth();

        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(config.token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }
        if config.deadline_duration == 0 {
            panic!("deadline_duration must be positive");
        }

        let mut counter: u32 = env
            .storage()
            .instance()
            .get(&DataKey::TemplateCounter)
            .unwrap_or(0);
        let template_id = counter;
        counter += 1;
        env.storage()
            .instance()
            .set(&DataKey::TemplateCounter, &counter);

        let template = EscrowTemplate {
            id: template_id,
            creator: creator.clone(),
            config,
            active: true,
        };
        env.storage()
            .persistent()
            .set(&DataKey::Template(template_id), &template);
        env.storage().persistent().extend_ttl(
            &DataKey::Template(template_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_template_created(&env, template_id, creator);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        template_id
    }

    /// Create an escrow from an existing template. Any caller may use any active template.
    pub fn create_escrow_from_template(
        env: Env,
        buyer: Address,
        seller: Address,
        template_id: u32,
        amount: i128,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        let template: EscrowTemplate = env
            .storage()
            .persistent()
            .get(&DataKey::Template(template_id))
            .expect("Template not found");

        if !template.active {
            panic!("Template is deactivated");
        }
        if amount <= 0 {
            panic!("Escrow amount must be positive");
        }

        let deadline = env.ledger().timestamp() + template.config.deadline_duration;

        let client = token::Client::new(&env, &template.config.token);
        client.transfer(&buyer, &env.current_contract_address(), &amount);

        let escrow_id = Self::next_escrow_id(&env);
        let mut single_seller = Vec::new(&env);
        single_seller.push_back((seller.clone(), 10_000u32));
        let escrow = Escrow {
            id: escrow_id,
            buyer: buyer.clone(),
            seller: seller.clone(),
            arbiter: template.config.arbiter.clone(),
            amount,
            token: template.config.token.clone(),
            status: EscrowStatus::Active,
            created_at: env.ledger().timestamp(),
            deadline,
            metadata_hash: None,
            sellers: single_seller,
            extensions: EscrowExtensions {
                auto_renew: false,
                renewal_count: 0,
                renewals_remaining: 0,
                dispute_timeout_seconds: None,
                buyer_inactivity_secs: 0,
                min_lock_until: None,
                release_base: None,
                release_quote: None,
                release_comparison: None,
                release_threshold_price: None,
                arbiter_fee_bps: None,
            },
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_created(
            &env,
            escrow_id,
            buyer,
            seller,
            template.config.arbiter,
            amount,
            template.config.token,
            deadline,
        );
        events::emit_escrow_created_from_template(&env, escrow_id, template_id);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        escrow_id
    }
    /// Add an arbiter to the pool. Admin only.
    pub fn add_arbiter(env: Env, admin: Address, arbiter: Address) {
        Self::require_admin(&env, &admin);
        let mut pool: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ArbiterPool)
            .unwrap_or(Vec::new(&env));
        for i in 0..pool.len() {
            if pool.get(i).unwrap() == arbiter {
                panic!("Arbiter already in pool");
            }
        }
        pool.push_back(arbiter.clone());
        env.storage()
            .instance()
            .set(&DataKey::ArbiterPool, &pool);
        events::emit_arbiter_pool_updated(&env, arbiter, true);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Remove an arbiter from the pool. Admin only.
    /// Active escrows with this arbiter are flagged via ArbiterNeedsReplacement.
    pub fn remove_arbiter(env: Env, admin: Address, arbiter: Address, escrow_ids: Vec<u32>) {
        Self::require_admin(&env, &admin);
        let pool: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ArbiterPool)
            .expect("Arbiter pool is empty");
        let mut found = false;
        let mut new_pool: Vec<Address> = Vec::new(&env);
        for i in 0..pool.len() {
            let a = pool.get(i).unwrap();
            if a == arbiter {
                found = true;
            } else {
                new_pool.push_back(a);
            }
        }
        if !found {
            panic!("Arbiter not in pool");
        }
        // Reset index if it would go out of bounds
        let next_idx: u32 = env
            .storage()
            .instance()
            .get(&DataKey::NextArbiterIndex)
            .unwrap_or(0);
        if new_pool.is_empty() || next_idx >= new_pool.len() {
            env.storage()
                .instance()
                .set(&DataKey::NextArbiterIndex, &0u32);
        }
        env.storage()
            .instance()
            .set(&DataKey::ArbiterPool, &new_pool);
        // Flag active escrows that used this arbiter
        for i in 0..escrow_ids.len() {
            let eid = escrow_ids.get(i).unwrap();
            if let Some(escrow) = env
                .storage()
                .persistent()
                .get::<DataKey, Escrow>(&DataKey::Escrow(eid))
            {
                if escrow.arbiter == arbiter && Self::is_open_escrow_status(escrow.status) {
                    env.storage()
                        .persistent()
                        .set(&DataKey::ArbiterNeedsReplacement(eid), &true);
                }
            }
        }
        events::emit_arbiter_pool_updated(&env, arbiter, false);
        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Create an escrow with the next arbiter from the pool (round-robin).
    pub fn create_escrow_with_pool_arbiter(
        env: Env,
        buyer: Address,
        seller: Address,
        amount: i128,
        token: Address,
        deadline: u64,
    ) -> u32 {
        Self::require_not_paused(&env);
        buyer.require_auth();

        if amount <= 0 {
            panic!("Escrow amount must be positive");
        }
        if deadline <= env.ledger().timestamp() {
            panic!("Deadline must be in the future");
        }
        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }

        let pool: Vec<Address> = env
            .storage()
            .instance()
            .get(&DataKey::ArbiterPool)
            .unwrap_or(Vec::new(&env));
        if pool.is_empty() {
            panic!("Arbiter pool is empty");
        }

        let idx: u32 = env
            .storage()
            .instance()
            .get(&DataKey::NextArbiterIndex)
            .unwrap_or(0);
        let arbiter = pool.get(idx % pool.len()).unwrap();
        let next_idx = (idx + 1) % pool.len();
        env.storage()
            .instance()
            .set(&DataKey::NextArbiterIndex, &next_idx);

        let client = token::Client::new(&env, &token);
        client.transfer(&buyer, &env.current_contract_address(), &amount);

        let escrow_id = Self::next_escrow_id(&env);
        let mut single_seller = Vec::new(&env);
        single_seller.push_back((seller.clone(), 10_000u32));
        let escrow = Escrow {
            id: escrow_id,
            buyer: buyer.clone(),
            seller: seller.clone(),
            arbiter: arbiter.clone(),
            amount,
            token: token.clone(),
            status: EscrowStatus::Active,
            created_at: env.ledger().timestamp(),
            deadline,
            metadata_hash: None,
            sellers: single_seller,
            extensions: EscrowExtensions {
                auto_renew: false,
                renewal_count: 0,
                renewals_remaining: 0,
                dispute_timeout_seconds: None,
                buyer_inactivity_secs: 0,
                min_lock_until: None,
                release_base: None,
                release_quote: None,
                release_comparison: None,
                release_threshold_price: None,
                arbiter_fee_bps: None,
            },
        };

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_created(
            &env, escrow_id, buyer, seller, arbiter.clone(), amount, token, deadline,
        );
        events::emit_arbiter_assigned(&env, escrow_id, arbiter);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);

        escrow_id
    }

    /// Update a template's config. Only the template creator can call this.
    pub fn update_escrow_template(
        env: Env,
        creator: Address,
        template_id: u32,
        new_config: EscrowTemplateConfig,
    ) {
        creator.require_auth();

        let mut template: EscrowTemplate = env
            .storage()
            .persistent()
            .get(&DataKey::Template(template_id))
            .expect("Template not found");

        if template.creator != creator {
            panic!("Only template creator can update");
        }
        if !template.active {
            panic!("Template is deactivated");
        }

        let is_allowed = env
            .storage()
            .instance()
            .get(&DataKey::AllowedToken(new_config.token.clone()))
            .unwrap_or(false);
        if !is_allowed {
            panic!("TokenNotAllowed");
        }
        if new_config.deadline_duration == 0 {
            panic!("deadline_duration must be positive");
        }

        template.config = new_config;
        env.storage()
            .persistent()
            .set(&DataKey::Template(template_id), &template);
        env.storage().persistent().extend_ttl(
            &DataKey::Template(template_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_template_updated(&env, template_id, creator);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Deactivate a template. Only the template creator can call this.
    pub fn deactivate_escrow_template(env: Env, creator: Address, template_id: u32) {
        creator.require_auth();

        let mut template: EscrowTemplate = env
            .storage()
            .persistent()
            .get(&DataKey::Template(template_id))
            .expect("Template not found");

        if template.creator != creator {
            panic!("Only template creator can deactivate");
        }
        if !template.active {
            panic!("Template already deactivated");
        }

        template.active = false;
        env.storage()
            .persistent()
            .set(&DataKey::Template(template_id), &template);

        events::emit_escrow_template_deactivated(&env, template_id, creator);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    /// Get template details.
    pub fn get_escrow_template(env: Env, template_id: u32) -> EscrowTemplate {
        env.storage()
            .persistent()
            .get(&DataKey::Template(template_id))
            .expect("Template not found")
    }

    // -------------------------------------------------------------------------
    // #150: Seller-Favored Auto-Release on Prolonged Buyer Inactivity
    // -------------------------------------------------------------------------

    /// Claim an inactivity-based release to seller.
    /// Callable only by the escrow's seller after `buyer_inactivity_secs`
    /// have elapsed without any buyer action. Disabled when the field is 0.
    pub fn claim_inactivity_release(env: Env, seller: Address, escrow_id: u32) {
        Self::require_not_paused(&env);
        seller.require_auth();

        let mut escrow: Escrow = env
            .storage()
            .persistent()
            .get(&DataKey::Escrow(escrow_id))
            .expect("Escrow not found");

        if escrow.extensions.buyer_inactivity_secs == 0 {
            panic!("Inactivity release is not enabled for this escrow");
        }

        if !Self::is_open_escrow_status(escrow.status) {
            panic!("Escrow is not active");
        }

        if seller != escrow.seller {
            panic!("Only the escrow seller can claim inactivity release");
        }

        let last_action: u64 = env
            .storage()
            .persistent()
            .get(&DataKey::LastBuyerAction(escrow_id))
            .unwrap_or(escrow.created_at);

        let now = env.ledger().timestamp();
        let inactivity_seconds = now.saturating_sub(last_action);

        if inactivity_seconds < escrow.extensions.buyer_inactivity_secs {
            panic!("Buyer inactivity window has not elapsed");
        }

        let client = token::Client::new(&env, &escrow.token);
        client.transfer(
            &env.current_contract_address(),
            &escrow.seller,
            &escrow.amount,
        );

        escrow.status = EscrowStatus::Released;

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(escrow_id), &escrow);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_inactivity_release_triggered(&env, escrow_id, seller, inactivity_seconds);

        env.storage()
            .instance()
            .extend_ttl(INSTANCE_LIFETIME_THRESHOLD, INSTANCE_BUMP_AMOUNT);
    }

    // --- Internal Helpers ---

    fn require_not_paused(env: &Env) {
        if env
            .storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
        {
            panic!("Contract is paused");
        }
    }

    fn require_or_bootstrap_admin(env: &Env, admin: &Address) {
        admin.require_auth();
        if let Some(stored_admin) = env
            .storage()
            .instance()
            .get::<DataKey, Address>(&DataKey::Admin)
        {
            if stored_admin != *admin {
                panic!("Only admin can pause contract");
            }
        } else {
            env.storage().instance().set(&DataKey::Admin, admin);
        }
    }

    fn require_admin(env: &Env, admin: &Address) {
        admin.require_auth();
        let stored_admin: Address = env
            .storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set");
        if stored_admin != *admin {
            panic!("Only admin can resume contract");
        }
    }

    fn require_unlocked(env: &Env, escrow: &Escrow) {
        if let Some(lock_until) = escrow.extensions.min_lock_until {
            if env.ledger().timestamp() < lock_until {
                panic!("EscrowStillLocked");
            }
        }
    }

    fn transfer_to_sellers(env: &Env, escrow: &Escrow, total: i128, escrow_id: u32) {
        let client = token::Client::new(env, &escrow.token);
        if escrow.sellers.len() <= 1 {
            client.transfer(&env.current_contract_address(), &escrow.seller, &total);
            events::emit_escrow_released(env, escrow_id, escrow.seller.clone(), total);
            return;
        }

        let mut distributed: i128 = 0;
        for i in 1..escrow.sellers.len() {
            let (addr, bps) = escrow.sellers.get(i).unwrap();
            let share = (total * bps as i128) / 10_000;
            if share > 0 {
                client.transfer(&env.current_contract_address(), &addr, &share);
            }
            distributed += share;
        }

        let first_share = total - distributed;
        if first_share > 0 {
            let (first_addr, _) = escrow.sellers.get(0).unwrap();
            client.transfer(&env.current_contract_address(), &first_addr, &first_share);
        }
        events::emit_multi_party_escrow_released(env, escrow_id, total);
    }

    fn get_oracle_price(env: &Env, base: &Address, quote: &Address) -> PriceData {
        let oracle_addr: Address = env
            .storage()
            .instance()
            .get(&DataKey::OracleAddress)
            .expect("Oracle not configured");
        let max_oracle_age: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MaxOracleAge)
            .unwrap_or(DEFAULT_MAX_ORACLE_AGE_SECONDS);

        let oracle_client = oracle::OracleClient::new(env, &oracle_addr);
        let price_data = oracle_client
            .lastprice(base, quote)
            .expect("Oracle price unavailable");

        let age = env.ledger().timestamp().saturating_sub(price_data.timestamp);
        if age > max_oracle_age {
            panic!("Oracle price is stale");
        }
        if price_data.price <= 0 {
            panic!("Invalid oracle price");
        }

        price_data
    }

    fn effective_arbiter_fee_bps(env: &Env, escrow: &Escrow) -> u32 {
        escrow.extensions.arbiter_fee_bps.unwrap_or(
            env.storage()
                .instance()
                .get(&DataKey::DefaultArbiterFeeBps)
                .unwrap_or(0),
        )
    }

    fn is_open_escrow_status(status: EscrowStatus) -> bool {
        matches!(
            status,
            EscrowStatus::Active | EscrowStatus::PartiallyReleased
        )
    }

    fn is_terminal_escrow_status(status: EscrowStatus) -> bool {
        matches!(
            status,
            EscrowStatus::Released | EscrowStatus::Resolved | EscrowStatus::Refunded
        )
    }

    fn next_escrow_id(env: &Env) -> u32 {
        let mut counter: u32 = env
            .storage()
            .instance()
            .get(&DataKey::EscrowCounter)
            .unwrap_or(0);
        let id = counter;
        counter += 1;
        env.storage()
            .instance()
            .set(&DataKey::EscrowCounter, &counter);
        id
    }

    fn try_auto_renew(env: &Env, old_escrow_id: u32, source: &Escrow) {
        if !source.extensions.auto_renew {
            return;
        }

        if source.extensions.renewal_count != 0 && source.extensions.renewals_remaining == 0 {
            return;
        }

        let allowance: u32 = env
            .storage()
            .persistent()
            .get(&DataKey::RenewalAllowance(old_escrow_id))
            .unwrap_or(0);
        if allowance == 0 {
            panic!("Insufficient renewal allowance");
        }

        let duration = source.deadline.saturating_sub(source.created_at);
        if duration == 0 {
            panic!("Escrow renewal duration must be positive");
        }

        let client = token::Client::new(env, &source.token);
        client.transfer_from(
            &env.current_contract_address(),
            &source.buyer,
            &env.current_contract_address(),
            &source.amount,
        );

        let new_escrow_id = Self::next_escrow_id(env);
        let now = env.ledger().timestamp();
        let renewals_remaining = if source.extensions.renewal_count == 0 {
            0
        } else {
            source.extensions.renewals_remaining - 1
        };

        let renewed = Escrow {
            id: new_escrow_id,
            buyer: source.buyer.clone(),
            seller: source.seller.clone(),
            arbiter: source.arbiter.clone(),
            amount: source.amount,
            token: source.token.clone(),
            status: EscrowStatus::Active,
            created_at: now,
            deadline: now + duration,
            metadata_hash: source.metadata_hash.clone(),
            sellers: source.sellers.clone(),
            extensions: EscrowExtensions {
                auto_renew: source.extensions.auto_renew,
                renewal_count: source.extensions.renewal_count,
                renewals_remaining,
                dispute_timeout_seconds: source.extensions.dispute_timeout_seconds,
                buyer_inactivity_secs: source.extensions.buyer_inactivity_secs,
                min_lock_until: source.extensions.min_lock_until,
                release_base: source.extensions.release_base.clone(),
                release_quote: source.extensions.release_quote.clone(),
                release_comparison: source.extensions.release_comparison,
                release_threshold_price: source.extensions.release_threshold_price,
                arbiter_fee_bps: source.extensions.arbiter_fee_bps,
            },
        };

        // #150: Initialize LastBuyerAction for renewed escrow
        if source.extensions.buyer_inactivity_secs > 0 {
            env.storage()
                .persistent()
                .set(&DataKey::LastBuyerAction(new_escrow_id), &now);
            env.storage().persistent().extend_ttl(
                &DataKey::LastBuyerAction(new_escrow_id),
                PERSISTENT_LIFETIME_THRESHOLD,
                PERSISTENT_BUMP_AMOUNT,
            );
        }

        env.storage()
            .persistent()
            .set(&DataKey::Escrow(new_escrow_id), &renewed);
        env.storage().persistent().extend_ttl(
            &DataKey::Escrow(new_escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        let remaining_allowance = allowance - 1;
        env.storage()
            .persistent()
            .set(&DataKey::RenewalAllowance(new_escrow_id), &remaining_allowance);
        env.storage().persistent().extend_ttl(
            &DataKey::RenewalAllowance(new_escrow_id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );

        events::emit_escrow_auto_renewed(env, old_escrow_id, new_escrow_id, renewals_remaining);
    }

    /// Update the LastBuyerAction timestamp for inactivity tracking (#150).
    /// Only records when inactivity release is enabled for the escrow.
    fn update_last_buyer_action(env: &Env, escrow: &Escrow) {
        if escrow.extensions.buyer_inactivity_secs == 0 {
            return;
        }
        let now = env.ledger().timestamp();
        env.storage()
            .persistent()
            .set(&DataKey::LastBuyerAction(escrow.id), &now);
        env.storage().persistent().extend_ttl(
            &DataKey::LastBuyerAction(escrow.id),
            PERSISTENT_LIFETIME_THRESHOLD,
            PERSISTENT_BUMP_AMOUNT,
        );
    }

    fn get_or_init_version(env: &Env) -> u32 {
        if let Some(version) = env
            .storage()
            .instance()
            .get::<DataKey, u32>(&DataKey::ContractVersion)
        {
            version
        } else {
            let initial_version = 1u32;
            env.storage()
                .instance()
                .set(&DataKey::ContractVersion, &initial_version);
            initial_version
        }
    }
}

#[cfg(test)]
mod test;
