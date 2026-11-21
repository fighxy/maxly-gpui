//! The client core: everything between the wire and the UI.
//!
//! * [`Session`]: one connection after `SESSION_INIT`, with the login flows the desktop
//!   client offers: QR ([`Session::start_qr`]), SMS code ([`Session::request_code`]), the 2FA
//!   password ([`Session::check_password`]), and `LOGIN` with a saved token.
//! * [`messages`]: chat history, sending text, new-message pushes.
//! * [`Supervisor`]: keeps a logged-in session alive: reconnects with backoff, sends
//!   `SESSION_INIT` and `LOGIN` again, and reports [`CoreEvent`]s.
//!
//! Packet layouts are the generated ones from the official client ([`max_proto::schema`]);
//! the string values (`START_AUTH`, `CHECK_CODE`, token kinds `LOGIN` / `REGISTER`) come from
//! `core.dll` (`rust/research/protocol-and-telemetry.md`). No telemetry is sent (ADR-005).

pub mod account;
pub mod assets;
mod auth;
pub mod chats;
mod config;
pub mod contacts;
mod error;
pub mod events;
pub mod media;
pub mod messages;
mod session;
mod supervisor;

pub use auth::{AuthStep, CodeRequest, QrLogin, QrPoll};
pub use config::{CoreConfig, SyncState};
pub use error::CoreError;
pub use session::{LoginResponse, Session};
pub use supervisor::{
    ConnectFuture, Connector, CoreEvent, Stream, Supervisor, SupervisorHandle, TlsConnector,
};

/// Result type of this crate.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;
