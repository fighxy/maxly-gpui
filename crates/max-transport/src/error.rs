use max_proto::ProtoError;
use thiserror::Error;

/// Errors of a connection or a single request.
#[derive(Debug, Error)]
pub enum TransportError {
    /// Socket error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// TLS setup error.
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),
    /// The host is not a valid TLS server name.
    #[error("invalid server name: {0}")]
    ServerName(String),
    /// TCP connect did not finish in time.
    #[error("connect to {0} timed out")]
    ConnectTimeout(String),
    /// Frame or payload error.
    #[error(transparent)]
    Proto(#[from] ProtoError),
    /// No response within the request timeout.
    #[error("request opcode {opcode} seq {seq} timed out")]
    Timeout {
        /// Request opcode.
        opcode: u16,
        /// Request seq.
        seq: u16,
    },
    /// The connection is closed; the request was not answered.
    #[error("connection closed")]
    Closed,
    /// The server answered with an error frame (`cmd` 3).
    #[error("server error for opcode {opcode}: {payload}")]
    Server {
        /// Request opcode.
        opcode: u16,
        /// Error payload as sent by the server.
        payload: rmpv::Value,
    },
    /// The response to a seq carries another opcode than the request.
    #[error("response opcode {got} does not match request opcode {expected}")]
    OpcodeMismatch {
        /// Request opcode.
        expected: u16,
        /// Response opcode.
        got: u16,
    },
    /// All 65535 seq values are waiting for responses.
    #[error("no free seq: too many requests in flight")]
    SeqExhausted,
}
