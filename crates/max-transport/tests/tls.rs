//! Real TLS over loopback: a local server with a self-signed certificate for `localhost`.

// Test helpers fail loudly.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use max_proto::payload::to_msgpack;
use max_proto::{Cmd, Frame, FrameDecoder, Opcode, PROTOCOL_VERSION};
use max_transport::{
    Endpoint, Transport, TransportConfig, TransportError, connect, tls_config,
    tls_config_with_roots,
};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

struct Pki {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

fn self_signed() -> Pki {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    Pki {
        cert: ck.cert.der().clone(),
        key: PrivatePkcs8KeyDer::from(ck.signing_key.serialize_der()).into(),
    }
}

async fn echo_server(pki: Pki) -> u16 {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![pki.cert], pki.key)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut buf = BytesMut::new();
                let mut dec = FrameDecoder::new();
                loop {
                    while let Some(f) = dec.decode(&mut buf) {
                        let reply = Frame::new(
                            PROTOCOL_VERSION,
                            Cmd::Response,
                            f.header.seq,
                            f.header.opcode,
                            0,
                            f.payload,
                        )
                        .unwrap();
                        tls.write_all(&reply.encode()).await.unwrap();
                    }
                    if tls.read_buf(&mut buf).await.unwrap_or(0) == 0 {
                        return;
                    }
                }
            });
        }
    });
    port
}

#[tokio::test]
async fn request_over_tls_with_a_trusted_root() {
    let pki = self_signed();
    let mut roots = RootCertStore::empty();
    roots.add(pki.cert.clone()).unwrap();
    let port = echo_server(pki).await;

    let stream = connect(
        &Endpoint::new("localhost", port),
        tls_config_with_roots(roots).unwrap(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let (t, _rx) = Transport::spawn(
        stream,
        TransportConfig {
            ping_interval: None,
            ..TransportConfig::default()
        },
    );
    let resp = t
        .request(Opcode::Ping.as_u16(), to_msgpack(&"over tls").unwrap())
        .await
        .unwrap();
    assert_eq!(resp.decode::<String>().unwrap(), "over tls");
}

#[tokio::test]
async fn untrusted_certificate_is_refused() {
    let port = echo_server(self_signed()).await;
    let err = connect(
        &Endpoint::new("localhost", port),
        tls_config().unwrap(),
        Duration::from_secs(5),
    )
    .await
    .unwrap_err();
    // rustls reports the bad certificate as an io error from the handshake.
    assert!(matches!(err, TransportError::Io(_)), "{err:?}");
}

#[test]
fn official_endpoint() {
    assert_eq!(Endpoint::official().to_string(), "api2.oneme.ru:443");
}
