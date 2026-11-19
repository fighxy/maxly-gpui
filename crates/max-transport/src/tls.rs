use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, RootCertStore};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::{Result, TransportError};

/// A server to connect to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    /// Host name, also used for TLS server name verification.
    pub host: String,
    /// TCP port.
    pub port: u16,
}

impl Endpoint {
    /// Any host and port.
    pub fn new(host: impl Into<String>, port: u16) -> Endpoint {
        Endpoint {
            host: host.into(),
            port,
        }
    }

    /// The API server of the desktop client (`config.dll`: `ssl_url2`, `ssl_port`).
    pub fn official() -> Endpoint {
        Endpoint::new("api2.oneme.ru", 443)
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// TLS client config with the Mozilla root store (`webpki-roots`) and rustls' safe defaults
/// (TLS 1.2 and 1.3).
pub fn tls_config() -> Result<Arc<ClientConfig>> {
    let roots = RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    tls_config_with_roots(roots)
}

/// TLS client config that trusts only `roots` (tests, pinning).
pub fn tls_config_with_roots(roots: RootCertStore) -> Result<Arc<ClientConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Open TCP and TLS to `endpoint`. The TCP connect is bounded by `connect_timeout`.
pub async fn connect(
    endpoint: &Endpoint,
    tls: Arc<ClientConfig>,
    connect_timeout: Duration,
) -> Result<TlsStream<TcpStream>> {
    let name = ServerName::try_from(endpoint.host.clone())
        .map_err(|_| TransportError::ServerName(endpoint.host.clone()))?;
    let tcp = tokio::time::timeout(
        connect_timeout,
        TcpStream::connect((endpoint.host.as_str(), endpoint.port)),
    )
    .await
    .map_err(|_| TransportError::ConnectTimeout(endpoint.to_string()))??;
    tcp.set_nodelay(true)?;
    Ok(TlsConnector::from(tls).connect(name, tcp).await?)
}
