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

/// "Russian Trusted Root CA" as bundled by the official client (`network.dll`, Qt resource
/// `:/certs/russiantrustedca.pem`), SHA-256 `D26D2D02...8ECF31`, valid 2022-03-01..2032-02-27.
const RUSSIAN_TRUSTED_ROOT_CA: &[u8] = include_bytes!("../certs/russian_trusted_root_ca.pem");

/// Whether `host` is a MAX API host (`oneme.ru` or a subdomain), which gets [`max_roots`].
pub fn is_max_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "oneme.ru" || host.ends_with(".oneme.ru")
}

/// The roots MAX hosts are checked against: only the Russian Trusted Root CA (ADR-011).
pub fn max_roots() -> Result<RootCertStore> {
    use rustls::pki_types::CertificateDer;
    use rustls::pki_types::pem::PemObject as _;

    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(RUSSIAN_TRUSTED_ROOT_CA) {
        let cert = cert.map_err(|e| {
            TransportError::Tls(rustls::Error::General(format!("bundled root: {e}")))
        })?;
        roots.add(cert)?;
    }
    Ok(roots)
}

/// TLS client config for `host`: [`max_roots`] for MAX hosts, the Mozilla roots otherwise.
pub fn tls_config_for(host: &str) -> Result<Arc<ClientConfig>> {
    if is_max_host(host) {
        tls_config_with_roots(max_roots()?)
    } else {
        tls_config()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_hosts_are_oneme_and_its_subdomains_only() {
        for host in [
            "oneme.ru",
            "api2.oneme.ru",
            "API2.ONEME.RU",
            "api2.oneme.ru.",
        ] {
            assert!(is_max_host(host), "{host}");
        }
        for host in [
            "evil-oneme.ru",
            "oneme.ru.evil.com",
            "max.ru",
            "notoneme.ru",
        ] {
            assert!(!is_max_host(host), "{host}");
        }
    }

    #[test]
    fn bundled_root_parses_to_one_anchor() {
        assert_eq!(max_roots().unwrap().len(), 1);
        assert!(tls_config_for("api2.oneme.ru").is_ok());
        assert!(tls_config_for("example.com").is_ok());
    }
}

/// Checks server certificates against [`max_roots`] for MAX hosts and the Mozilla roots for
/// every other host, so one HTTP client follows ADR-011 for any URL the server hands out.
#[derive(Debug)]
pub struct ScopedVerifier {
    max: Arc<rustls::client::WebPkiServerVerifier>,
    other: Arc<rustls::client::WebPkiServerVerifier>,
}

impl ScopedVerifier {
    /// Both verifiers with the ring provider.
    pub fn new() -> Result<ScopedVerifier> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let build = |roots: RootCertStore| {
            rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(roots),
                provider.clone(),
            )
            .build()
            .map_err(|e| TransportError::Tls(rustls::Error::General(e.to_string())))
        };
        Ok(ScopedVerifier {
            max: build(max_roots()?)?,
            other: build(RootCertStore::from_iter(
                webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
            ))?,
        })
    }

    fn pick(&self, name: &ServerName<'_>) -> &rustls::client::WebPkiServerVerifier {
        match name {
            ServerName::DnsName(dns) if is_max_host(dns.as_ref()) => &self.max,
            _ => &self.other,
        }
    }
}

impl rustls::client::danger::ServerCertVerifier for ScopedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        self.pick(server_name).verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        )
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.other.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.other.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.other.supported_verify_schemes()
    }
}

/// TLS config for an HTTP client that may reach MAX and non-MAX hosts (media, uploads): the
/// certificate check follows [`ScopedVerifier`].
pub fn http_tls_config() -> Result<ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    Ok(ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(ScopedVerifier::new()?))
        .with_no_client_auth())
}

#[cfg(test)]
mod scoped_tests {
    use super::*;

    #[test]
    fn http_config_builds() {
        assert!(http_tls_config().is_ok());
    }
}
