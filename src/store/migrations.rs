use std::path::Path;

use rusqlite::{Connection, TransactionBehavior};

use super::schema;
use super::StoreError;

pub const SCHEMA_VERSION: u32 = 5;

struct Migration {
    version: u32,
    sql: &'static str,
}

// Append only: a released migration is never edited, and versions stay contiguous from 1.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: schema::V1,
    },
    Migration {
        version: 2,
        sql: schema::V2,
    },
    Migration {
        version: 3,
        sql: schema::V3,
    },
    Migration {
        version: 4,
        sql: schema::V4,
    },
    Migration {
        version: 5,
        sql: schema::V5,
    },
];

pub(super) fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
}

pub(super) fn run(conn: &mut Connection, path: &Path) -> Result<(), StoreError> {
    for migration in MIGRATIONS {
        apply(conn, migration).map_err(|source| StoreError::Migrate {
            path: path.to_owned(),
            version: migration.version,
            source,
        })?;
    }
    Ok(())
}

fn apply(conn: &mut Connection, migration: &Migration) -> rusqlite::Result<()> {
    if user_version(conn)? >= migration.version {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Another process may have migrated between the check above and taking the write lock.
    if user_version(&tx)? >= migration.version {
        return Ok(());
    }
    tx.execute_batch(migration.sql)?;
    tx.pragma_update(None, "user_version", migration.version)?;
    tx.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_contiguous_and_end_at_schema_version() {
        let mut expected = 0_u32;
        for migration in MIGRATIONS {
            expected = expected.saturating_add(1);
            assert_eq!(migration.version, expected);
        }
        assert_eq!(expected, SCHEMA_VERSION);
    }
}
