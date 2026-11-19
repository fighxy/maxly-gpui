//! What the client tells the server about itself in `SESSION_INIT` (ADR-003, ADR-008).
//!
//! * [`ClientVersion`]: the official desktop build we present as (`DESKTOP_26_41_0`).
//! * [`DeviceId`]: 28 random bytes in base64, the shape of the official hardware hash, without
//!   reading any hardware.
//! * [`Identity`]: the per-profile state (device id, device name, launch counter) the caller
//!   stores and loads.
//! * [`system`]: OS version, language and time zone of this machine.
//! * [`session_init`]: the payload, built from the above and [`IdentitySettings`].

mod device_id;
mod device_name;
mod error;
mod identity;
pub mod system;
mod version;

pub use device_id::DeviceId;
pub use device_name::random_device_name;
pub use error::IdentityError;
pub use identity::{
    DeviceNameChoice, Environment, Identity, IdentitySettings, session_init, user_agent,
};
pub use version::{ClientVersion, DESKTOP_26_41_0};

/// Result type of this crate.
pub type Result<T, E = IdentityError> = std::result::Result<T, E>;
