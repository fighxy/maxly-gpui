use std::path::{Path, PathBuf};
use std::sync::Arc;

use max_identity::Identity;
use zeroize::Zeroizing;

use crate::{Database, DbKey, Result, SecretStore, StoreError};

const DB_FILE: &str = "app.db";
const IDENTITY_KEY: &str = "identity";

/// Where profiles live by default: `%LOCALAPPDATA%\maxrs\profiles` on Windows.
pub fn default_profiles_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("maxrs").join("profiles"))
}

/// One local profile: its folder, its encrypted database and its secrets.
pub struct Profile {
    name: String,
    dir: PathBuf,
    db: Database,
    secrets: Arc<dyn SecretStore>,
    identity: Identity,
}

impl Profile {
    /// Open the profile `name` under `root`, creating it on first use. A new profile gets a
    /// random database key (kept only in `secrets`) and a new [`Identity`].
    pub fn open(root: &Path, name: &str, secrets: Arc<dyn SecretStore>) -> Result<Profile> {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join(DB_FILE);
        let key_name = secret_name(name, "db-key");
        let key = match secrets.get(&key_name)? {
            Some(bytes) => DbKey::from_bytes(&Zeroizing::new(bytes))?,
            None if db_path.exists() => {
                return Err(StoreError::KeyLost(db_path.display().to_string()));
            }
            None => {
                let key = DbKey::generate()?;
                secrets.set(&key_name, key.as_bytes())?;
                key
            }
        };
        let db = Database::open(&db_path, &key)?;
        let identity = match db.get::<Identity>(IDENTITY_KEY)? {
            Some(identity) => identity,
            None => {
                let identity = Identity::new_random()?;
                db.put(IDENTITY_KEY, &identity)?;
                identity
            }
        };
        Ok(Profile {
            name: name.to_string(),
            dir,
            db,
            secrets,
            identity,
        })
    }

    /// Delete the profile's database and secrets so the next [`Profile::open`] starts fresh.
    /// Use after [`StoreError::KeyLost`]; the account itself is not affected.
    pub fn reset(root: &Path, name: &str, secrets: &dyn SecretStore) -> Result<()> {
        let dir = root.join(name);
        for file in [DB_FILE, "app.db-wal", "app.db-shm"] {
            match std::fs::remove_file(dir.join(file)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        secrets.delete(&secret_name(name, "db-key"))?;
        secrets.delete(&secret_name(name, "auth-token"))
    }

    /// Profile name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Profile folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The encrypted database.
    pub fn database(&self) -> &Database {
        &self.db
    }

    /// The device identity.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Count a new launch (`clientSessionId`) and store it before it is sent.
    pub fn begin_session(&mut self) -> Result<i64> {
        let id = self.identity.begin_session();
        self.db.put(IDENTITY_KEY, &self.identity)?;
        Ok(id)
    }

    /// A handle to this profile's login token that can live apart from the profile (the
    /// supervisor owns the profile while logout and session closing need the token).
    pub fn token_store(&self) -> TokenStore {
        TokenStore {
            secrets: self.secrets.clone(),
            key: secret_name(&self.name, "auth-token"),
        }
    }

    /// The login token, if logged in.
    pub fn auth_token(&self) -> Result<Option<Zeroizing<String>>> {
        self.token_store().get()
    }

    /// Keep the login token in the protected store.
    pub fn set_auth_token(&self, token: &str) -> Result<()> {
        self.token_store().set(token)
    }

    /// Forget the login token (log out).
    pub fn clear_auth_token(&self) -> Result<()> {
        self.token_store().clear()
    }
}

/// The login token of one profile, in the protected store.
#[derive(Clone)]
pub struct TokenStore {
    secrets: Arc<dyn SecretStore>,
    key: String,
}

impl std::fmt::Debug for TokenStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenStore")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl TokenStore {
    /// The login token, if logged in.
    pub fn get(&self) -> Result<Option<Zeroizing<String>>> {
        match self.secrets.get(&self.key)? {
            None => Ok(None),
            Some(bytes) => {
                let bytes = Zeroizing::new(bytes);
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| StoreError::Secrets("login token is not UTF-8".into()))?;
                Ok(Some(Zeroizing::new(text.to_string())))
            }
        }
    }

    /// Keep a new login token.
    pub fn set(&self, token: &str) -> Result<()> {
        self.secrets.set(&self.key, token.as_bytes())
    }

    /// Forget the login token.
    pub fn clear(&self) -> Result<()> {
        self.secrets.delete(&self.key)
    }
}

fn secret_name(profile: &str, what: &str) -> String {
    format!("{profile}:{what}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySecrets;

    #[test]
    fn creates_once_and_keeps_identity_and_counter() {
        let root = tempfile::tempdir().unwrap();
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
        let device_id = {
            let mut p = Profile::open(root.path(), "default", secrets.clone()).unwrap();
            assert_eq!(p.begin_session().unwrap(), 1);
            p.identity().device_id.clone()
        };
        let mut p = Profile::open(root.path(), "default", secrets.clone()).unwrap();
        assert_eq!(p.identity().device_id, device_id);
        assert_eq!(p.begin_session().unwrap(), 2);
        assert!(secrets.get("default:db-key").unwrap().is_some());
        assert!(!root.path().join("default").join("db-key").exists());
    }

    #[test]
    fn token_lives_in_the_secret_store_only() {
        let root = tempfile::tempdir().unwrap();
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
        let p = Profile::open(root.path(), "default", secrets.clone()).unwrap();
        assert!(p.auth_token().unwrap().is_none());
        p.set_auth_token("tok-123").unwrap();
        assert_eq!(p.auth_token().unwrap().unwrap().as_str(), "tok-123");
        assert_eq!(
            secrets.get("default:auth-token").unwrap().unwrap(),
            b"tok-123"
        );
        p.clear_auth_token().unwrap();
        assert!(p.auth_token().unwrap().is_none());
    }

    #[test]
    fn lost_key_is_reported_and_reset_recovers() {
        let root = tempfile::tempdir().unwrap();
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
        let old_id = Profile::open(root.path(), "p", secrets.clone())
            .unwrap()
            .identity()
            .device_id
            .clone();
        secrets.delete("p:db-key").unwrap();
        let err = Profile::open(root.path(), "p", secrets.clone())
            .err()
            .unwrap();
        assert!(matches!(err, StoreError::KeyLost(_)), "{err:?}");
        Profile::reset(root.path(), "p", secrets.as_ref()).unwrap();
        let p = Profile::open(root.path(), "p", secrets).unwrap();
        assert_ne!(p.identity().device_id, old_id);
    }

    #[test]
    fn profiles_are_separate() {
        let root = tempfile::tempdir().unwrap();
        let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
        let a = Profile::open(root.path(), "a", secrets.clone()).unwrap();
        let b = Profile::open(root.path(), "b", secrets.clone()).unwrap();
        assert_ne!(a.identity().device_id, b.identity().device_id);
        assert_ne!(
            secrets.get("a:db-key").unwrap(),
            secrets.get("b:db-key").unwrap()
        );
    }
}
