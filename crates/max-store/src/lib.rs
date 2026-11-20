//! Local storage of a profile (ADR-009).
//!
//! * [`Database`]: one SQLCipher file, opened with a raw 256-bit key; a key-value table for
//!   now, more tables as the core needs them (migrations by `PRAGMA user_version`).
//! * [`SecretStore`]: where the database key and the login token live. [`OsSecrets`] is the
//!   Windows Credential Manager (entries encrypted with DPAPI for the current user);
//!   [`MemorySecrets`] is for tests.
//! * [`Profile`]: ties both together and keeps the [`max_identity::Identity`].
//!
//! Building needs `OPENSSL_SRC_PERL` pointing to a full Perl (SQLCipher builds OpenSSL from
//! source) and a short target directory path; see `BUILDING.md`.

mod database;
mod error;
mod profile;
mod secrets;

pub use database::{Database, DbKey};
pub use error::StoreError;
pub use profile::{Profile, TokenStore, default_profiles_dir};
pub use secrets::{MemorySecrets, OsSecrets, SecretStore};

/// Result type of this crate.
pub type Result<T, E = StoreError> = std::result::Result<T, E>;
