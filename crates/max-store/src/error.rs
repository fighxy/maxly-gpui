use thiserror::Error;

/// Storage errors.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite or SQLCipher error.
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    /// The key does not open the database (wrong key or not a SQLCipher file).
    #[error("the database key does not match {0}")]
    WrongKey(String),
    /// The database exists but its key is gone from the protected store. The local cache
    /// cannot be read; [`crate::Profile::reset`] starts over (the account is not affected).
    #[error("the key for {0} is missing from the protected store")]
    KeyLost(String),
    /// The protected store failed.
    #[error("protected store: {0}")]
    Secrets(String),
    /// A stored value has an unexpected shape.
    #[error("stored value {key}: {message}")]
    Value {
        /// Key of the value.
        key: String,
        /// What is wrong.
        message: String,
    },
    /// Identity creation failed.
    #[error(transparent)]
    Identity(#[from] max_identity::IdentityError),
    /// File system error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The database was written by a newer version of this program.
    #[error("database schema {found} is newer than supported {supported}")]
    SchemaTooNew {
        /// Version in the file.
        found: i64,
        /// Highest version this build knows.
        supported: i64,
    },
}
