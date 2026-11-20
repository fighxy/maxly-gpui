//! Connection to the MAX API the way the desktop client keeps it.
//!
//! [`connect`] opens TLS to an [`Endpoint`] (`api2.oneme.ru:443` by default); [`Transport::spawn`]
//! runs any byte stream: it numbers requests (`seq` from 1), matches each response to its
//! request, hands every other frame to the caller as [`Incoming::Push`], and sends `PING`
//! `{interactive}` every 30 s. Session setup (`SESSION_INIT`, login) and reconnect policy belong
//! to the layer above; [`Backoff`] helps with the latter.

mod backoff;
mod error;
mod tls;
mod transport;

pub use backoff::Backoff;
pub use error::TransportError;
pub use tls::{
    Endpoint, ScopedVerifier, connect, http_tls_config, is_max_host, max_roots, tls_config,
    tls_config_for, tls_config_with_roots,
};
pub use transport::{Incoming, Response, Transport, TransportConfig};

/// Result type of this crate.
pub type Result<T, E = TransportError> = std::result::Result<T, E>;
