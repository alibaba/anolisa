//! Checksummed SQLite schema migrations for Gateway Task storage.

use rusqlite::{params, Connection, OptionalExtension, Transaction};

use super::StoreError;

mod migration_v1;
mod migration_v2;
mod migration_v3;
mod migration_v4;
mod migration_v5;
mod migration_v6;
mod migration_v7;
mod migration_v8;
mod migration_v9;

pub(super) const CURRENT_SCHEMA_VERSION: u32 = 9;

struct Migration {
    version: u32,
    checksum: &'static str,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    migration_v1::MIGRATION,
    migration_v2::MIGRATION,
    migration_v3::MIGRATION,
    migration_v4::MIGRATION,
    migration_v5::MIGRATION,
    migration_v6::MIGRATION,
    migration_v7::MIGRATION,
    migration_v8::MIGRATION,
    migration_v9::MIGRATION,
];

include!("schema/operations.rs");

pub(super) fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    migrate_through(connection, CURRENT_SCHEMA_VERSION)
}

pub(super) fn preflight_existing(connection: &Connection) -> Result<(), StoreError> {
    validate_schema_history(connection, CURRENT_SCHEMA_VERSION)?;
    validate_integrity(connection)
}

#[cfg(test)]
pub(super) fn migrate_to_for_test(
    connection: &mut Connection,
    target_version: u32,
) -> Result<(), StoreError> {
    migrate_through(connection, target_version)
}

#[cfg(test)]
mod tests;
