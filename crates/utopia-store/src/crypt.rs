//! Encryption at rest (P2, §15.6, open question 7): the graph file can be a SQLCipher
//! database. The passphrase never touches the file or the sync relay; the app reads it
//! from the macOS keychain or asks for it.

use std::path::Path;

use rusqlite::{Connection, ErrorCode};

use crate::{Result, StoreError};

/// Open a connection, applying the key first (SQLCipher requires it before any read)
/// and checking it can read the schema.
pub(crate) fn open_keyed(path: &Path, key: Option<&str>) -> Result<Connection> {
    if key.is_some_and(str::is_empty) {
        return Err(StoreError::Encrypted);
    }
    let conn = Connection::open(path)?;
    if let Some(k) = key {
        if !encryption_available() {
            return Err(StoreError::Invalid(
                "this build has no encryption at rest (SQLCipher)".into(),
            ));
        }
        conn.pragma_update(None, "key", k)?;
    }
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    }) {
        Ok(_) => Ok(conn),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::NotADatabase => {
            Err(StoreError::Encrypted)
        }
        Err(e) => Err(e.into()),
    }
}

/// Whether this build links SQLCipher.
pub fn encryption_available() -> bool {
    Connection::open_in_memory()
        .and_then(|c| c.query_row("PRAGMA cipher_version", [], |r| r.get::<_, String>(0)))
        .is_ok()
}

/// Whether the file at `path` is an encrypted graph (unreadable without a key).
pub fn is_encrypted(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.len() > 0)
        && matches!(open_keyed(path, None), Err(StoreError::Encrypted))
}

/// Encrypt, re-key or decrypt a graph file that no process has open. `from` is the
/// current passphrase (`None`: plain), `to` the new one (`None`: decrypt). The new file
/// is written beside the old and renamed over it, so a failure leaves the original.
pub fn set_encryption(path: &Path, from: Option<&str>, to: Option<&str>) -> Result<()> {
    if to.is_some_and(str::is_empty) {
        return Err(StoreError::Invalid("the passphrase is empty".into()));
    }
    let conn = open_keyed(path, from)?;
    // Fold the WAL in so the export sees everything.
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let tmp = path.with_extension("utopia-rekey");
    let _ = std::fs::remove_file(&tmp);
    conn.execute(
        "ATTACH DATABASE ?1 AS rekeyed KEY ?2",
        rusqlite::params![tmp.to_string_lossy(), to.unwrap_or("")],
    )?;
    conn.query_row("SELECT sqlcipher_export('rekeyed')", [], |_| Ok(()))?;
    conn.execute_batch(&format!(
        "PRAGMA rekeyed.user_version = {version}; DETACH DATABASE rekeyed;"
    ))?;
    drop(conn);
    for ext in ["utopia-wal", "utopia-shm"] {
        let _ = std::fs::remove_file(path.with_extension(ext));
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}
