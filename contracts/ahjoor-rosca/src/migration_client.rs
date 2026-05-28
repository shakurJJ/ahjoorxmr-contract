use soroban_sdk::{contractclient, Address, Env};
use crate::{MigratedMemberRecord, MigrationRequest};

/// Minimal cross-contract interface used by the destination group contract
/// to interact with the source group contract during member migration.
#[contractclient(name = "RoscaMigrationClient")]
pub trait RoscaMigrationInterface {
    /// Returns the base token address of this group.
    fn get_token(env: Env) -> Address;

    /// Returns the pending outbound migration request for `member`, if any.
    fn get_migration_request(env: Env, member: Address) -> Option<MigrationRequest>;

    /// Called by the destination contract during `execute_migration`.
    /// Removes `member` from the source group, marks their slot Vacant,
    /// and returns their contribution history summary.
    /// Panics if the migration request is not in `BothApproved` state.
    fn finalize_migration_exit(env: Env, member: Address, dest_contract: Address) -> MigratedMemberRecord;
}
