//! Typed payloads of packets whose layout is confirmed by decompilation or a capture.
//! Field order follows the official client, since serde writes struct fields in order.

pub mod auth_qr;
pub mod ping;
pub mod session;

pub use auth_qr::{AuthCheck, AuthCheckResponse, CreateQr, CreateQrResponse, QrStatus};
pub use ping::Ping;
pub use session::{SessionInit, SessionInitResponse, UserAgent};
