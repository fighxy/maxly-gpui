use thiserror::Error;

/// Errors of the client core.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Connection or request failure.
    #[error(transparent)]
    Transport(#[from] max_transport::TransportError),
    /// Local storage failure.
    #[error(transparent)]
    Store(#[from] max_store::StoreError),
    /// Payload encoding failure.
    #[error(transparent)]
    Proto(#[from] max_proto::ProtoError),
    /// The server's answer to a login step has none of the expected outcomes.
    #[error("unexpected login answer: {0}")]
    UnexpectedAuthAnswer(String),
    /// An HTTP request (media download, upload) failed.
    #[error("http: {0}")]
    Http(String),
    /// No saved login token; log in first.
    #[error("not logged in")]
    NotLoggedIn,
    /// The supervisor has stopped.
    #[error("the client has stopped")]
    Stopped,
}
