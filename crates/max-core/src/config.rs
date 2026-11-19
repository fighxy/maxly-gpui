use std::time::Duration;

use max_identity::{ClientVersion, DESKTOP_26_41_0, IdentitySettings};
use max_transport::{Backoff, Endpoint, TransportConfig};
use serde::{Deserialize, Serialize};

/// How the core connects and presents itself.
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// API server (`api2.oneme.ru:443`).
    pub endpoint: Endpoint,
    /// TCP connect limit.
    pub connect_timeout: Duration,
    /// Request timeout, ping.
    pub transport: TransportConfig,
    /// The official build we present as (ADR-003).
    pub version: ClientVersion,
    /// Device name, time zone and language choices (ADR-008).
    pub identity: IdentitySettings,
    /// Delay between reconnect attempts.
    pub backoff: Backoff,
}

impl Default for CoreConfig {
    fn default() -> CoreConfig {
        CoreConfig {
            endpoint: Endpoint::official(),
            connect_timeout: Duration::from_secs(15),
            transport: TransportConfig::default(),
            version: DESKTOP_26_41_0,
            identity: IdentitySettings::default(),
            backoff: Backoff::default(),
        }
    }
}

/// Sync markers sent in `LOGIN` (`chatsSync`, `contactsSync`, ...). All 0 on a fresh profile,
/// which asks for everything. How the server's answer advances them is not decoded yet, so
/// they stay 0 for now (open question in `rust/MAP.md`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    /// `chatsSync`.
    pub chats: i64,
    /// `contactsSync`.
    pub contacts: i64,
    /// `presenceSync`.
    pub presence: i64,
    /// `callsSync`.
    pub calls: i64,
    /// `draftsSync`.
    pub drafts: i64,
    /// `configHash` of the last received configuration.
    pub config_hash: String,
    /// `lastLogin`.
    pub last_login: i64,
}
