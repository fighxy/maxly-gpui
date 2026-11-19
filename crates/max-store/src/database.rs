use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{Result, StoreError};

/// A raw 256-bit SQLCipher key. Wiped from memory when dropped.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct DbKey([u8; 32]);

impl DbKey {
    /// A new random key from the OS random generator.
    pub fn generate() -> Result<DbKey> {
        let mut key = [0u8; 32];
        getrandom::fill(&mut key).map_err(|e| StoreError::Secrets(format!("random key: {e}")))?;
        Ok(DbKey(key))
    }

    /// A key from its stored bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<DbKey> {
        let key: [u8; 32] = bytes.try_into().map_err(|_| {
            StoreError::Secrets(format!("database key has {} bytes, not 32", bytes.len()))
        })?;
        Ok(DbKey(key))
    }

    /// The bytes to store.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// SQLCipher raw key syntax `x'..64 hex..'`: used as is, no key derivation.
    fn pragma_value(&self) -> String {
        let mut s = String::with_capacity(67);
        s.push_str("x'");
        for b in self.0 {
            let _ = write!(s, "{b:02x}");
        }
        s.push('\'');
        s
    }
}

impl std::fmt::Debug for DbKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DbKey(..)")
    }
}

/// Schema versions; each entry upgrades from the previous `user_version`.
const MIGRATIONS: &[&str] = &[
    // 1: key-value store for small profile values (identity, settings).
    "CREATE TABLE kv (key TEXT PRIMARY KEY NOT NULL, value BLOB NOT NULL) STRICT;",
];

/// An open SQLCipher database.
pub struct Database {
    conn: Connection,
    path: PathBuf,
}

impl Database {
    /// Open or create the database at `path` with `key`. Fails with
    /// [`StoreError::WrongKey`] when the key does not match an existing file.
    pub fn open(path: &Path, key: &DbKey) -> Result<Database> {
        let conn = Connection::open(path)?;
        // SQLCipher logs decryption failures to stderr by default; errors reach the caller.
        conn.pragma_update(None, "cipher_log_level", "NONE")?;
        let mut value = key.pragma_value();
        let keyed = conn.pragma_update(None, "key", &value);
        value.zeroize();
        keyed?;
        // The first read proves the key: a wrong key reads as "file is not a database".
        if let Err(e) = conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
            r.get::<_, i64>(0)
        }) {
            return Err(match e {
                rusqlite::Error::SqliteFailure(f, _)
                    if f.code == rusqlite::ErrorCode::NotADatabase =>
                {
                    StoreError::WrongKey(path.display().to_string())
                }
                other => other.into(),
            });
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut db = Database {
            conn,
            path: path.to_path_buf(),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<()> {
        let current: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        let supported = MIGRATIONS.len() as i64;
        if current > supported {
            return Err(StoreError::SchemaTooNew {
                found: current,
                supported,
            });
        }
        for (i, sql) in MIGRATIONS
            .iter()
            .enumerate()
            .skip(usize::try_from(current).unwrap_or(0))
        {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", i as i64 + 1)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Schema version of the open file.
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    /// The SQLCipher library version.
    pub fn cipher_version(&self) -> Result<String> {
        Ok(self
            .conn
            .query_row("PRAGMA cipher_version", [], |r| r.get(0))?)
    }

    /// Path of the file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read a value stored with [`Database::put`].
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let raw: Option<Vec<u8>> = self
            .conn
            .query_row("SELECT value FROM kv WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()?;
        raw.map(|bytes| {
            serde_json::from_slice(&bytes).map_err(|e| StoreError::Value {
                key: key.to_string(),
                message: e.to_string(),
            })
        })
        .transpose()
    }

    /// Store a value as JSON under `key`, replacing any previous one.
    pub fn put<T: Serialize + ?Sized>(&self, key: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|e| StoreError::Value {
            key: key.to_string(),
            message: e.to_string(),
        })?;
        self.conn.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, bytes],
        )?;
        Ok(())
    }

    /// Remove a value; removing a missing one is not an error.
    pub fn remove(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM kv WHERE key = ?1", params![key])?;
        Ok(())
    }

    /// The connection, for crates that add their own tables.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.db");
        (dir, path)
    }

    #[test]
    fn stores_and_reopens() {
        let (_dir, path) = temp_db();
        let key = DbKey::generate().unwrap();
        {
            let db = Database::open(&path, &key).unwrap();
            assert!(db.cipher_version().unwrap().starts_with('4'));
            db.put("answer", &42u32).unwrap();
            db.put("answer", &43u32).unwrap();
        }
        let db = Database::open(&path, &key).unwrap();
        assert_eq!(db.get::<u32>("answer").unwrap(), Some(43));
        assert_eq!(db.get::<u32>("missing").unwrap(), None);
        db.remove("answer").unwrap();
        assert_eq!(db.get::<u32>("answer").unwrap(), None);
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i64);
    }

    #[test]
    fn wrong_key_is_refused() {
        let (_dir, path) = temp_db();
        Database::open(&path, &DbKey::generate().unwrap())
            .unwrap()
            .put("x", &1)
            .unwrap();
        let err = Database::open(&path, &DbKey::generate().unwrap())
            .err()
            .unwrap();
        assert!(matches!(err, StoreError::WrongKey(_)), "{err:?}");
    }

    #[test]
    fn file_is_not_plain_sqlite() {
        let (_dir, path) = temp_db();
        let db = Database::open(&path, &DbKey::generate().unwrap()).unwrap();
        db.put("secret", "visible?").unwrap();
        drop(db);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.starts_with(b"SQLite format 3\0"));
        assert!(!bytes.windows(8).any(|w| w == b"visible?"));
    }

    #[test]
    fn newer_schema_is_refused() {
        let (_dir, path) = temp_db();
        let key = DbKey::generate().unwrap();
        let db = Database::open(&path, &key).unwrap();
        db.connection()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        drop(db);
        let err = Database::open(&path, &key).err().unwrap();
        assert!(
            matches!(err, StoreError::SchemaTooNew { found: 99, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn key_roundtrip_and_debug_hides_it() {
        let key = DbKey::generate().unwrap();
        assert_eq!(DbKey::from_bytes(key.as_bytes()).unwrap(), key);
        assert!(DbKey::from_bytes(&[0u8; 16]).is_err());
        assert_eq!(format!("{key:?}"), "DbKey(..)");
        assert_eq!(key.pragma_value().len(), 67);
    }
}
