use thiserror::Error;

/// Errors of identity values.
#[derive(Debug, Error)]
pub enum IdentityError {
    /// The OS random number generator failed.
    #[error("system random generator: {0}")]
    Random(String),
    /// A stored device id is not 28 bytes of base64.
    #[error("invalid device id: {0}")]
    InvalidDeviceId(String),
}
