// SPDX-License-Identifier: Apache-2.0

//! Append-only schema migrations and the transactional runner.
//!
//! A [`Migration`] is `{ version, name, up }`; `up` is a `&'static str` batch of DDL. The list is
//! append-only and its index order matches its version order. Each migration runs in its own
//! transaction: a failure rolls back the DDL, the `migrations` row, and the `meta.schema_version`
//! bump together.

use rusqlite::{Connection, OptionalExtension, params};

use crate::{
    schema::{CURRENT_SCHEMA_VERSION, V1},
    store::StoreError,
};

/// One numbered, named schema migration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Migration {
    /// Monotonic schema version this migration produces.
    pub(crate) version: u32,
    /// Human-readable name recorded in the `migrations` table.
    pub(crate) name: &'static str,
    /// DDL batch that performs the migration.
    pub(crate) up: &'static str,
}

/// The production migration list, ordered by version.
pub(crate) const MIGRATIONS: &[Migration] =
    &[Migration { version: 1, name: "initial schema", up: V1 }];

/// Applies every migration that the database has not recorded yet.
///
/// A fresh database (no `meta` table) starts by running the first migration in `migrations`, then
/// continues with any later pending versions. An existing database below the current version
/// applies each pending migration in its own transaction; a database newer than
/// [`CURRENT_SCHEMA_VERSION`] is rejected.
///
/// # Errors
///
/// Returns [`StoreError::NewerDatabase`] when the database is newer than this build, and
/// [`StoreError::Migration`] when a migration fails (the failing transaction is rolled back).
pub(crate) fn apply_migrations(
    conn: &Connection,
    migrations: &[Migration],
) -> Result<(), StoreError> {
    let current = if let Some(found) = schema_version(conn)? {
        if found > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::NewerDatabase { found, supported: CURRENT_SCHEMA_VERSION });
        }
        found
    } else {
        let initial = migrations
            .first()
            .ok_or(StoreError::Migration { version: 0, name: "no migrations defined" })?;
        apply_one(conn, initial)?;
        initial.version
    };

    for migration in migrations.iter().filter(|migration| migration.version > current) {
        apply_one(conn, migration)?;
    }
    Ok(())
}

/// Runs one migration, its bookkeeping row, and the version bump in a single transaction.
fn apply_one(conn: &Connection, migration: &Migration) -> Result<(), StoreError> {
    let failed = || StoreError::Migration { version: migration.version, name: migration.name };
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(migration.up).map_err(|_| failed())?;
    tx.execute(
        "INSERT INTO migrations (version, name, applied_at) VALUES (?1, ?2, datetime('now'))",
        params![migration.version, migration.name],
    )
    .map_err(|_| failed())?;
    tx.execute(
        "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [migration.version.to_string()],
    )
    .map_err(|_| failed())?;
    tx.commit().map_err(|_| failed())?;
    Ok(())
}

/// Reads `meta.schema_version`; `None` means the `meta` table does not exist (a fresh database).
fn schema_version(conn: &Connection) -> Result<Option<u32>, StoreError> {
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
        [],
        |row| row.get(0),
    )?;
    if !has_meta {
        return Ok(None);
    }

    let value: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0))
        .optional()?;
    let Some(value) = value else {
        return Err(StoreError::Migration { version: 0, name: "missing meta.schema_version" });
    };
    value
        .parse::<u32>()
        .map(Some)
        .map_err(|_| StoreError::Migration { version: 0, name: "unreadable meta.schema_version" })
}

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, OptionalExtension as _};

    use super::{MIGRATIONS, Migration, apply_migrations};
    use crate::{schema::CURRENT_SCHEMA_VERSION, store::StoreError};

    fn connection() -> Connection {
        Connection::open_in_memory().expect("open in-memory database")
    }

    fn version(conn: &Connection) -> Option<String> {
        conn.query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| row.get(0))
            .optional()
            .expect("read schema version")
    }

    fn applied(conn: &Connection) -> i64 {
        conn.query_row("SELECT count(*) FROM migrations", [], |row| row.get(0))
            .expect("count migrations")
    }

    #[test]
    fn production_list_ends_at_the_current_version() {
        assert_eq!(
            MIGRATIONS.last().map(|migration| migration.version),
            Some(CURRENT_SCHEMA_VERSION)
        );
    }

    #[test]
    fn fresh_database_stamps_version_one() {
        let conn = connection();
        apply_migrations(&conn, MIGRATIONS).expect("apply migrations");
        assert_eq!(version(&conn).as_deref(), Some("1"));
        assert_eq!(applied(&conn), 1);
    }

    #[test]
    fn reapplying_is_idempotent() {
        let conn = connection();
        apply_migrations(&conn, MIGRATIONS).expect("first apply");
        apply_migrations(&conn, MIGRATIONS).expect("second apply");
        assert_eq!(version(&conn).as_deref(), Some("1"));
        assert_eq!(applied(&conn), 1);
    }

    #[test]
    fn failing_migration_rolls_back() {
        let conn = connection();
        apply_migrations(&conn, MIGRATIONS).expect("apply schema v1");

        let broken = Migration { version: 2, name: "broken migration", up: "THIS IS NOT SQL" };
        let mut list = MIGRATIONS.to_vec();
        list.push(broken);
        let error = apply_migrations(&conn, &list).expect_err("migration must fail");

        assert!(
            matches!(error, StoreError::Migration { version: 2, .. }),
            "expected a migration error, got: {error}"
        );
        assert!(!error.to_string().contains("THIS IS NOT SQL"), "error must not leak DDL");
        assert_eq!(version(&conn).as_deref(), Some("1"));
        assert_eq!(applied(&conn), 1);
    }

    #[test]
    fn newer_database_is_rejected() {
        let conn = connection();
        apply_migrations(&conn, MIGRATIONS).expect("apply schema v1");
        conn.execute("UPDATE meta SET value = '2' WHERE key = 'schema_version'", [])
            .expect("bump stored version");

        let error = apply_migrations(&conn, MIGRATIONS).expect_err("newer database must fail");
        assert!(matches!(
            error,
            StoreError::NewerDatabase { found: 2, supported: CURRENT_SCHEMA_VERSION }
        ));
    }
}
