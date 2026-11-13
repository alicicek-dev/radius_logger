use rusqlite::{Connection, Result as SqlResult};
use std::fs;
use std::path::Path;

/// Opens a SQLite connection configured with high-performance WAL + memory-mapped I/O pragmas.
pub fn open_db(db_path: &Path) -> SqlResult<Connection> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            let _ = fs::create_dir_all(parent);
        }
    }
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 30000;
         PRAGMA mmap_size = 536870912;
         PRAGMA cache_size = -64000;
         PRAGMA temp_store = MEMORY;",
    )?;
    Ok(conn)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> SqlResult<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name.eq_ignore_ascii_case(column) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Ensures tables (`AuthLog`, `Devices`), new v5.1+ columns (`SourceFile`, `SourceLine`, `EventHash`),
/// and all performance indexes exist. Safe to run idempotently on both fresh and existing databases.
pub fn ensure_schema_and_migrate(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS AuthLog (
            LogID            INTEGER PRIMARY KEY AUTOINCREMENT,
            EventID          INTEGER,
            TimeCreated      TEXT,
            FullUsername     TEXT,
            Username         TEXT,
            Realm            TEXT,
            ClientIP         TEXT,
            NASIdentifier    TEXT,
            NASIPAddress     TEXT,
            CalledStationID  TEXT,
            CallingStationID TEXT,
            AuthType         TEXT,
            PolicyName       TEXT,
            ReasonCode       INTEGER DEFAULT 0,
            Reason           TEXT,
            Result           TEXT,
            SourceFile       TEXT,
            SourceLine       INTEGER,
            EventHash        TEXT
        );

        CREATE TABLE IF NOT EXISTS Devices (
            DeviceID     INTEGER PRIMARY KEY AUTOINCREMENT,
            MACAddress   TEXT UNIQUE,
            FirstSeen    TEXT,
            LastSeen     TEXT,
            LastUsername TEXT,
            LastResult   TEXT
        );",
    )?;

    if !has_column(conn, "AuthLog", "SourceFile")? {
        conn.execute("ALTER TABLE AuthLog ADD COLUMN SourceFile TEXT", [])?;
    }
    if !has_column(conn, "AuthLog", "SourceLine")? {
        conn.execute("ALTER TABLE AuthLog ADD COLUMN SourceLine INTEGER", [])?;
    }
    if !has_column(conn, "AuthLog", "EventHash")? {
        conn.execute("ALTER TABLE AuthLog ADD COLUMN EventHash TEXT", [])?;
    }

    conn.execute_batch(
        "DROP INDEX IF EXISTS idx_no_dup;
         CREATE UNIQUE INDEX IF NOT EXISTS idx_event_hash ON AuthLog(EventHash) WHERE EventHash IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_time   ON AuthLog(TimeCreated);
         CREATE INDEX IF NOT EXISTS idx_user   ON AuthLog(Username);
         CREATE INDEX IF NOT EXISTS idx_result ON AuthLog(Result);
         CREATE INDEX IF NOT EXISTS idx_realm  ON AuthLog(Realm);
         CREATE INDEX IF NOT EXISTS idx_nas    ON AuthLog(NASIdentifier);
         CREATE INDEX IF NOT EXISTS idx_auth_lookup ON AuthLog(TimeCreated, Result, Username, Realm);
         CREATE INDEX IF NOT EXISTS idx_mac    ON Devices(MACAddress);",
    )?;

    Ok(())
}

/// Resets `AuthLog` and `Devices` tables for a clean backfill.
pub fn reset_tables(conn: &Connection) -> SqlResult<()> {
    conn.execute_batch(
        "DELETE FROM AuthLog;
         DELETE FROM Devices;
         DELETE FROM sqlite_sequence WHERE name IN ('AuthLog', 'Devices');",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_migration_and_reset_idempotent() {
        let dir = std::env::temp_dir().join(format!("eduroam_db_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("schema_test.db");

        let conn = open_db(&db_path).unwrap();
        // First migration
        ensure_schema_and_migrate(&conn).unwrap();
        // Second migration (idempotent)
        ensure_schema_and_migrate(&conn).unwrap();

        conn.execute(
            "INSERT INTO AuthLog (EventID, TimeCreated, Result, EventHash) VALUES (1, '2026-11-07 12:00:00', 'SUCCESS', 'h1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO Devices (MACAddress, FirstSeen, LastSeen, LastUsername, LastResult) VALUES ('AA-BB', '2026-11-07', '2026-11-07', 'u', 'SUCCESS')",
            [],
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM AuthLog", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        reset_tables(&conn).unwrap();
        let count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM AuthLog", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_after, 0);
        let dev_count_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM Devices", [], |r| r.get(0))
            .unwrap();
        assert_eq!(dev_count_after, 0);

        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }
}
