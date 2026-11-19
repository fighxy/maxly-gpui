//! Transport against a scripted server over an in-memory duplex stream.

// Test helpers fail loudly.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use bytes::BytesMut;
use max_proto::packets::{SessionInitResponse, session::UserAgent};
use max_proto::payload::to_msgpack;
use max_proto::{Cmd, Frame, FrameDecoder, Opcode, PROTOCOL_VERSION, compress_server_payload};
use max_transport::{Incoming, Transport, TransportConfig, TransportError};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

/// Server side of the duplex: reads client frames, writes server frames.
struct Server {
    io: DuplexStream,
    buf: BytesMut,
    dec: FrameDecoder,
}

impl Server {
    async fn next(&mut self) -> Option<Frame> {
        loop {
            if let Some(f) = self.dec.decode(&mut self.buf) {
                return Some(f);
            }
            if self.io.read_buf(&mut self.buf).await.unwrap() == 0 {
                return None;
            }
        }
    }

    /// Answer like the real server: cmd 1, same seq and opcode, lz4 with the ratio flag.
    async fn answer<T: Serialize>(&mut self, to: &Frame, body: &T) {
        let plain = to_msgpack(body).unwrap();
        let (flags, packed) = compress_server_payload(&plain).unwrap();
        self.send(
            Cmd::Response,
            to.header.seq,
            to.header.opcode,
            flags,
            packed,
        )
        .await;
    }

    async fn send(&mut self, cmd: Cmd, seq: u16, opcode: u16, flags: u8, payload: Vec<u8>) {
        let f = Frame::new(PROTOCOL_VERSION, cmd, seq, opcode, flags, payload).unwrap();
        self.io.write_all(&f.encode()).await.unwrap();
    }
}

fn start(
    config: TransportConfig,
) -> (
    Transport,
    tokio::sync::mpsc::UnboundedReceiver<Incoming>,
    Server,
) {
    let (client, server) = tokio::io::duplex(1 << 16);
    let (t, rx) = Transport::spawn(client, config);
    let server = Server {
        io: server,
        buf: BytesMut::new(),
        dec: FrameDecoder::new(),
    };
    (t, rx, server)
}

fn no_ping() -> TransportConfig {
    TransportConfig {
        ping_interval: None,
        ..TransportConfig::default()
    }
}

/// The captured response shape: schema keys (`lang`, `reg-country-code`) and extra ones
/// (`location`, `qr-auth-enabled`) that the client ignores.
#[derive(Serialize)]
struct InitReply {
    location: &'static str,
    #[serde(rename = "qr-auth-enabled")]
    qr: bool,
    lang: bool,
    #[serde(rename = "reg-country-code")]
    reg: Vec<&'static str>,
}

#[tokio::test]
async fn session_init_roundtrip_with_lz4_response() {
    let (t, _rx, mut srv) = start(no_ping());
    let server = tokio::spawn(async move {
        let req = srv.next().await.unwrap();
        assert_eq!(req.header.ver, PROTOCOL_VERSION);
        assert_eq!(
            (req.header.cmd, req.header.seq, req.header.flags),
            (0, 1, 0)
        );
        assert_eq!(req.header.opcode(), Some(Opcode::SessionInit));
        srv.answer(
            &req,
            &InitReply {
                location: "RU",
                qr: true,
                lang: true,
                reg: vec!["AZ", "AM"],
            },
        )
        .await;
        srv
    });
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Init<'a> {
        user_agent: &'a UserAgent,
    }
    let ua = UserAgent {
        device_type: "DESKTOP".into(),
        app_version: "26.41.0".into(),
        os_version: "Windows 11 Version 25H2".into(),
        locale: "ru".into(),
        device_locale: "ru".into(),
        device_name: "PC".into(),
        screen: "2.0x".into(),
        timezone: "Europe/Moscow".into(),
        msi_product_code: "{00000000-0000-0000-0000-000000000000}".into(),
        build_number: 83760,
    };
    let resp: SessionInitResponse = t
        .call(Opcode::SessionInit, &Init { user_agent: &ua })
        .await
        .unwrap();
    assert_eq!(resp.lang, Some(true));
    assert_eq!(resp.reg_country_code.unwrap(), ["AZ", "AM"]);
    server.await.unwrap();
    assert_eq!(t.pending(), 0);
}

#[tokio::test]
async fn responses_out_of_order_reach_their_requests() {
    let (t, _rx, mut srv) = start(no_ping());
    let server = tokio::spawn(async move {
        let a = srv.next().await.unwrap();
        let b = srv.next().await.unwrap();
        srv.answer(&b, &"second").await;
        srv.answer(&a, &"first").await;
    });
    let (ra, rb) = tokio::join!(
        t.request(Opcode::ChatList.as_u16(), to_msgpack(&()).unwrap()),
        t.request(Opcode::ContactList.as_u16(), to_msgpack(&()).unwrap()),
    );
    let (ra, rb) = (ra.unwrap(), rb.unwrap());
    assert_eq!(ra.decode::<String>().unwrap(), "first");
    assert_eq!(rb.decode::<String>().unwrap(), "second");
    assert_ne!(ra.header.seq, rb.header.seq);
    server.await.unwrap();
}

#[tokio::test]
async fn unsolicited_frames_are_pushes() {
    let (_t, mut rx, mut srv) = start(no_ping());
    srv.send(
        Cmd::Request,
        0,
        Opcode::NotificationMessage.as_u16(),
        0,
        to_msgpack(&"hi").unwrap(),
    )
    .await;
    match rx.recv().await.unwrap() {
        Incoming::Push(p) => {
            assert_eq!(p.header.opcode(), Some(Opcode::NotificationMessage));
            assert_eq!(p.decode::<String>().unwrap(), "hi");
        }
        other => panic!("expected a push, got {other:?}"),
    }
}

#[tokio::test]
async fn timeout_frees_the_seq_and_drops_the_late_answer() {
    let cfg = TransportConfig {
        request_timeout: Duration::from_millis(100),
        ..no_ping()
    };
    let (t, mut rx, mut srv) = start(cfg);
    let err = t
        .request(Opcode::ChatInfo.as_u16(), to_msgpack(&()).unwrap())
        .await
        .unwrap_err();
    assert!(
        matches!(err, TransportError::Timeout { seq: 1, .. }),
        "{err:?}"
    );
    assert_eq!(t.pending(), 0);
    // The late answer must not surface as a push nor break the connection.
    let late = srv.next().await.unwrap();
    srv.answer(&late, &"late").await;
    let next = tokio::spawn(async move {
        let f = srv.next().await.unwrap();
        assert_eq!(f.header.seq, 2);
        srv.answer(&f, &"ok").await;
    });
    let ok = t
        .request(Opcode::ChatInfo.as_u16(), to_msgpack(&()).unwrap())
        .await
        .unwrap();
    assert_eq!(ok.decode::<String>().unwrap(), "ok");
    next.await.unwrap();
    // The server task has ended, so a Closed may follow; a Push must not.
    while let Ok(item) = rx.try_recv() {
        assert!(
            !matches!(item, Incoming::Push(_)),
            "late answer surfaced: {item:?}"
        );
    }
}

#[tokio::test]
async fn error_frame_becomes_server_error() {
    let (t, _rx, mut srv) = start(no_ping());
    tokio::spawn(async move {
        let f = srv.next().await.unwrap();
        let body = to_msgpack(
            &[("error", "proto.payload")]
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>(),
        )
        .unwrap();
        srv.send(Cmd::Error, f.header.seq, f.header.opcode, 0, body)
            .await;
    });
    let err = t
        .request(Opcode::MessagingSend.as_u16(), to_msgpack(&()).unwrap())
        .await
        .unwrap_err();
    match err {
        TransportError::Server { opcode, payload } => {
            assert_eq!(opcode, Opcode::MessagingSend.as_u16());
            assert_eq!(
                payload.as_map().unwrap()[0].1.as_str(),
                Some("proto.payload")
            );
        }
        other => panic!("expected a server error, got {other:?}"),
    }
}

#[tokio::test]
async fn opcode_mismatch_is_reported() {
    let (t, _rx, mut srv) = start(no_ping());
    tokio::spawn(async move {
        let f = srv.next().await.unwrap();
        srv.send(
            Cmd::Response,
            f.header.seq,
            Opcode::Profile.as_u16(),
            0,
            Vec::new(),
        )
        .await;
    });
    let err = t
        .request(Opcode::ChatInfo.as_u16(), to_msgpack(&()).unwrap())
        .await
        .unwrap_err();
    assert!(
        matches!(err, TransportError::OpcodeMismatch { .. }),
        "{err:?}"
    );
}

#[tokio::test]
async fn server_close_fails_pending_and_reports_closed() {
    let (t, mut rx, mut srv) = start(no_ping());
    let server = tokio::spawn(async move {
        let _ = srv.next().await.unwrap();
        drop(srv);
    });
    let err = t
        .request(Opcode::ChatList.as_u16(), to_msgpack(&()).unwrap())
        .await
        .unwrap_err();
    assert!(matches!(err, TransportError::Closed), "{err:?}");
    server.await.unwrap();
    assert!(matches!(rx.recv().await.unwrap(), Incoming::Closed(None)));
    assert!(t.is_closed());
    let again = t
        .request(Opcode::ChatList.as_u16(), to_msgpack(&()).unwrap())
        .await
        .unwrap_err();
    assert!(matches!(again, TransportError::Closed));
}

#[tokio::test]
async fn sends_desktop_ping_on_interval() {
    let cfg = TransportConfig {
        ping_interval: Some(Duration::from_millis(50)),
        ..TransportConfig::default()
    };
    let (_t, _rx, mut srv) = start(cfg);
    for expected_seq in 1..=2u16 {
        let f = tokio::time::timeout(Duration::from_secs(5), srv.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(f.header.opcode(), Some(Opcode::Ping));
        assert_eq!(f.header.seq, expected_seq);
        // capture cap1: {"interactive": true}, 14 bytes
        assert_eq!(&f.payload[..], b"\x81\xabinteractive\xc3");
        srv.send(Cmd::Response, f.header.seq, f.header.opcode, 0, Vec::new())
            .await;
    }
}

#[tokio::test]
async fn ping_timeout_closes_the_connection() {
    let cfg = TransportConfig {
        ping_interval: Some(Duration::from_millis(30)),
        request_timeout: Duration::from_millis(60),
        ..TransportConfig::default()
    };
    let (t, mut rx, srv) = start(cfg);
    let _keep = srv; // never answers
    let closed = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(closed, Incoming::Closed(None)));
    assert!(t.is_closed());
}

#[tokio::test]
async fn dropping_every_handle_stops_the_connection() {
    let (t, _rx, mut srv) = start(no_ping());
    drop(t);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), srv.next())
            .await
            .unwrap()
            .is_none()
    );
}
