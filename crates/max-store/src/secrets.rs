use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use crate::{Result, StoreError};

/// A small protected store for secrets by name.
pub trait SecretStore: Send + Sync {
    /// The secret stored under `name`, if any.
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>>;
    /// Store or replace a secret.
    fn set(&self, name: &str, secret: &[u8]) -> Result<()>;
    /// Remove a secret; removing a missing one is not an error.
    fn delete(&self, name: &str) -> Result<()>;
}

/// The operating system's credential store: on Windows the Credential Manager, whose generic
/// credentials are encrypted with DPAPI for the current user (ADR-009).
#[derive(Debug, Clone)]
pub struct OsSecrets {
    service: String,
}

impl OsSecrets {
    /// Secrets of this application; `service` names it in the credential store.
    pub fn new(service: impl Into<String>) -> OsSecrets {
        OsSecrets {
            service: service.into(),
        }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, name).map_err(|e| StoreError::Secrets(e.to_string()))
    }
}

impl SecretStore for OsSecrets {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match self.entry(name)?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(StoreError::Secrets(e.to_string())),
        }
    }

    fn set(&self, name: &str, secret: &[u8]) -> Result<()> {
        self.entry(name)?
            .set_secret(secret)
            .map_err(|e| StoreError::Secrets(e.to_string()))
    }

    fn delete(&self, name: &str) -> Result<()> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(StoreError::Secrets(e.to_string())),
        }
    }
}

/// Secrets in memory, for tests.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    items: Mutex<HashMap<String, Vec<u8>>>,
}

impl MemorySecrets {
    /// An empty store.
    pub fn new() -> MemorySecrets {
        MemorySecrets::default()
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(items.get(name).cloned())
    }

    fn set(&self, name: &str, secret: &[u8]) -> Result<()> {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.insert(name.to_string(), secret.to_vec());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.remove(name);
        Ok(())
    }
}
