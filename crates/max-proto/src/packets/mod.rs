//! Names for the packets the core uses, over the generated layouts in [`crate::schema`], plus
//! the few packets the desktop client builds without `SerializableMember` (`PING`).

pub mod auth_qr;
pub mod ping;
pub mod session;

pub use auth_qr::{
    AuthByTrack, AuthByTrackResponse, AuthCheck, AuthCheckResponse, CreateQr, CreateQrResponse,
    QrStatus,
};
pub use ping::Ping;
pub use session::{SessionInit, SessionInitResponse, UserAgent};
