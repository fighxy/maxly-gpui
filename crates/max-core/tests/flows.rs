//! Login flows and reconnects against a scripted server over in-memory streams.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use max_core::{
    AuthStep, ConnectFuture, Connector, CoreConfig, CoreEvent, QrPoll, Session, Stream, Supervisor,
    SyncState,
};
use max_identity::Environment;
use max_proto::payload::{decode_value, to_msgpack};
use max_proto::{Cmd, Frame, FrameDecoder, Opcode, PROTOCOL_VERSION, compress_server_payload};
use max_store::{MemorySecrets, Profile, SecretStore};
use max_transport::{Backoff, TransportConfig};
use rmpv::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::sync::{Mutex, mpsc};

fn map(entries: &[(&str, Value)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(k, v)| (Value::from(*k), v.clone()))
            .collect(),
    )
}

fn get<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.as_map()
        .and_then(|m| m.iter().find(|(k, _)| k.as_str() == Some(key)))
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("no key {key} in {v}"))
}

struct Server {
    io: DuplexStream,
    buf: BytesMut,
    dec: FrameDecoder,
}

impl Server {
    fn new(io: DuplexStream) -> Server {
        Server {
            io,
            buf: BytesMut::new(),
            dec: FrameDecoder::new(),
        }
    }

    /// Next client request: (opcode, seq, decoded payload).
    async fn next(&mut self) -> Option<(Opcode, u16, Value)> {
        loop {
            if let Some(f) = self.dec.decode(&mut self.buf) {
                assert_eq!(f.header.ver, PROTOCOL_VERSION);
                assert_eq!(f.header.flags, 0, "the client never compresses");
                let op = f.header.opcode().expect("known opcode");
                return Some((op, f.header.seq, decode_value(&f.payload).unwrap()));
            }
            if self.io.read_buf(&mut self.buf).await.unwrap() == 0 {
                return None;
            }
        }
    }

    async fn expect(&mut self, op: Opcode) -> (u16, Value) {
        let (got, seq, body) = self.next().await.expect("a request");
        assert_eq!(got, op, "unexpected request {body}");
        (seq, body)
    }

    async fn reply(&mut self, op: Opcode, seq: u16, body: &Value) {
        let (flags, packed) = compress_server_payload(&to_msgpack(body).unwrap()).unwrap();
        let f = Frame::new(
            PROTOCOL_VERSION,
            Cmd::Response,
            seq,
            op.as_u16(),
            flags,
            packed,
        )
        .unwrap();
        self.io.write_all(&f.encode()).await.unwrap();
    }

    /// Answer SESSION_INIT and return its payload.
    async fn session_init(&mut self) -> Value {
        let (seq, body) = self.expect(Opcode::SessionInit).await;
        self.reply(
            Opcode::SessionInit,
            seq,
            &map(&[("lang", true.into()), ("location", "RU".into())]),
        )
        .await;
        body
    }

    /// Answer LOGIN with a minimal sync and return its payload.
    async fn login(&mut self, user_id: i64) -> Value {
        let (seq, body) = self.expect(Opcode::Login).await;
        let contact = map(&[("id", user_id.into()), ("updateTime", 1i64.into())]);
        let reply = map(&[
            ("profile", map(&[("contact", contact)])),
            ("chats", Value::Array(vec![])),
            ("contacts", Value::Array(vec![])),
            ("config", map(&[("hash", "h1".into())])),
        ]);
        self.reply(Opcode::Login, seq, &reply).await;
        body
    }
}

fn env() -> Environment {
    Environment {
        os_version: "Windows 11 Version 25H2".into(),
        system_language: "ru".into(),
        timezone: "Europe/Moscow".into(),
        computer_name: Some("REAL-NAME".into()),
    }
}

fn config() -> CoreConfig {
    CoreConfig {
        transport: TransportConfig {
            ping_interval: None,
            request_timeout: Duration::from_secs(5),
            ..TransportConfig::default()
        },
        backoff: Backoff::new(Duration::from_millis(10), Duration::from_millis(50)),
        ..CoreConfig::default()
    }
}

fn profile(root: &tempfile::TempDir, secrets: &Arc<dyn SecretStore>) -> Profile {
    Profile::open(root.path(), "default", secrets.clone()).unwrap()
}

#[tokio::test]
async fn session_init_sends_the_desktop_identity_and_counts_launches() {
    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let mut p = profile(&root, &secrets);
    let device_id = p.identity().device_id.to_string();
    for launch in 1..=2i64 {
        let (client, server) = tokio::io::duplex(1 << 16);
        let srv = tokio::spawn(async move {
            let mut s = Server::new(server);
            s.session_init().await
        });
        let session = Session::start(client, &config(), &mut p, &env())
            .await
            .unwrap();
        assert_eq!(session.init_response().lang, Some(true));
        let body = srv.await.unwrap();
        let ua = get(&body, "userAgent");
        assert_eq!(get(ua, "deviceType").as_str(), Some("DESKTOP"));
        assert_eq!(get(ua, "buildNumber").as_i64(), Some(83760));
        assert!(
            get(ua, "deviceName")
                .as_str()
                .unwrap()
                .starts_with("DESKTOP-"),
            "random name, not REAL-NAME"
        );
        assert_eq!(get(&body, "deviceId").as_str(), Some(device_id.as_str()));
        assert_eq!(get(&body, "clientSessionId").as_i64(), Some(launch));
    }
}

#[tokio::test]
async fn qr_login_then_login_stores_the_token() {
    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let mut p = profile(&root, &secrets);
    let (client, server) = tokio::io::duplex(1 << 16);
    let srv = tokio::spawn(async move {
        let mut s = Server::new(server);
        s.session_init().await;
        let (seq, _) = s.expect(Opcode::CreateQr).await;
        let qr = map(&[
            ("trackId", "track-1".into()),
            ("qrLink", "https://max.ru/qr/x".into()),
            ("pollingInterval", 5000i64.into()),
        ]);
        s.reply(Opcode::CreateQr, seq, &qr).await;
        let (seq, body) = s.expect(Opcode::AuthCheck).await;
        assert_eq!(get(&body, "trackId").as_str(), Some("track-1"));
        s.reply(
            Opcode::AuthCheck,
            seq,
            &map(&[("status", map(&[("expiresAt", 99i64.into())]))]),
        )
        .await;
        let (seq, _) = s.expect(Opcode::AuthCheck).await;
        s.reply(
            Opcode::AuthCheck,
            seq,
            &map(&[("status", map(&[("loginAvailable", true.into())]))]),
        )
        .await;
        let (seq, body) = s.expect(Opcode::AuthByTrack).await;
        assert_eq!(get(&body, "trackId").as_str(), Some("track-1"));
        s.reply(
            Opcode::AuthByTrack,
            seq,
            &map(&[(
                "tokenAttrs",
                map(&[("LOGIN", map(&[("token", "tok-qr".into())]))]),
            )]),
        )
        .await;
        s.login(7).await
    });

    let session = Session::start(client, &config(), &mut p, &env())
        .await
        .unwrap();
    let qr = session.start_qr().await.unwrap();
    assert_eq!(
        (qr.track_id.as_str(), qr.polling_interval_ms),
        ("track-1", 5000)
    );
    assert_eq!(
        session.poll_qr(&qr).await.unwrap(),
        QrPoll::Waiting {
            expires_at: Some(99)
        }
    );
    assert_eq!(session.poll_qr(&qr).await.unwrap(), QrPoll::Ready);
    let step = session.finish_qr(&qr).await.unwrap();
    let login = session
        .complete(&p, &step, &SyncState::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(login.profile.contact.id, 7);
    assert_eq!(login.config.hash, "h1");
    assert_eq!(p.auth_token().unwrap().unwrap().as_str(), "tok-qr");

    let body = srv.await.unwrap();
    assert_eq!(get(&body, "token").as_str(), Some("tok-qr"));
    assert_eq!(get(&body, "interactive").as_bool(), Some(true));
    assert_eq!(get(&body, "chatsSync").as_i64(), Some(0));
}

#[tokio::test]
async fn sms_code_then_2fa_password() {
    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let mut p = profile(&root, &secrets);
    let (client, server) = tokio::io::duplex(1 << 16);
    let srv = tokio::spawn(async move {
        let mut s = Server::new(server);
        s.session_init().await;
        let (seq, body) = s.expect(Opcode::AuthRequest).await;
        assert_eq!(get(&body, "phone").as_str(), Some("+70000000000"));
        assert_eq!(get(&body, "type").as_str(), Some("START_AUTH"));
        let r = map(&[
            ("token", "t-sms".into()),
            ("codeLength", 6i64.into()),
            ("altActionDuration", 60i64.into()),
        ]);
        s.reply(Opcode::AuthRequest, seq, &r).await;
        let (seq, body) = s.expect(Opcode::Auth).await;
        assert_eq!(get(&body, "token").as_str(), Some("t-sms"));
        assert_eq!(get(&body, "verifyCode").as_str(), Some("123456"));
        assert_eq!(get(&body, "authTokenType").as_str(), Some("CHECK_CODE"));
        let r = map(&[
            ("tokenAttrs", map(&[])),
            ("passwordChallenge", map(&[("trackId", "tr-2fa".into())])),
        ]);
        s.reply(Opcode::Auth, seq, &r).await;
        let (seq, body) = s.expect(Opcode::LoginCheckPassword).await;
        assert_eq!(get(&body, "password").as_str(), Some("secret"));
        assert_eq!(get(&body, "trackId").as_str(), Some("tr-2fa"));
        s.reply(
            Opcode::LoginCheckPassword,
            seq,
            &map(&[(
                "tokenAttrs",
                map(&[("LOGIN", map(&[("token", "t-final".into())]))]),
            )]),
        )
        .await;
    });

    let session = Session::start(client, &config(), &mut p, &env())
        .await
        .unwrap();
    let req = session
        .request_code("+70000000000", Some("ru"), false)
        .await
        .unwrap();
    assert_eq!(req.code_length, 6);
    let step = session.confirm_code(&req, "123456").await.unwrap();
    let AuthStep::PasswordRequired(challenge) = step else {
        panic!("expected a password step, got {step:?}")
    };
    let step = session.check_password(&challenge, "secret").await.unwrap();
    let AuthStep::LoggedIn { token, .. } = step else {
        panic!("expected login, got {step:?}")
    };
    assert_eq!(token.as_str(), "t-final");
    srv.await.unwrap();
}

#[tokio::test]
async fn unknown_phone_needs_registration() {
    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let mut p = profile(&root, &secrets);
    let (client, server) = tokio::io::duplex(1 << 16);
    tokio::spawn(async move {
        let mut s = Server::new(server);
        s.session_init().await;
        let (seq, _) = s.expect(Opcode::AuthRequest).await;
        s.reply(
            Opcode::AuthRequest,
            seq,
            &map(&[
                ("token", "t".into()),
                ("codeLength", 4i64.into()),
                ("altActionDuration", 30i64.into()),
            ]),
        )
        .await;
        let (seq, _) = s.expect(Opcode::Auth).await;
        s.reply(
            Opcode::Auth,
            seq,
            &map(&[(
                "tokenAttrs",
                map(&[("REGISTER", map(&[("token", "r".into())]))]),
            )]),
        )
        .await;
    });
    let session = Session::start(client, &config(), &mut p, &env())
        .await
        .unwrap();
    let req = session
        .request_code("+70000000001", None, false)
        .await
        .unwrap();
    assert!(matches!(
        session.confirm_code(&req, "0000").await.unwrap(),
        AuthStep::RegistrationRequired
    ));
    assert!(p.auth_token().unwrap().is_none());
}

/// Hands out pre-made in-memory streams, one per connection attempt.
struct QueueConnector {
    streams: Mutex<mpsc::UnboundedReceiver<DuplexStream>>,
}

impl Connector for QueueConnector {
    fn connect(&self) -> ConnectFuture<'_> {
        Box::pin(async move {
            let s = self.streams.lock().await.recv().await.expect("a stream");
            Ok(Box::new(s) as Box<dyn Stream>)
        })
    }
}

#[tokio::test]
async fn supervisor_reconnects_and_logs_in_again() {
    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let p = profile(&root, &secrets);
    p.set_auth_token("saved-token").unwrap();

    let (tx, rx) = mpsc::unbounded_channel();
    let connector = Arc::new(QueueConnector {
        streams: Mutex::new(rx),
    });
    let mut server_sides = Vec::new();
    for _ in 0..2 {
        let (client, server) = tokio::io::duplex(1 << 16);
        tx.send(client).unwrap();
        server_sides.push(server);
    }
    let second = server_sides.pop().unwrap();
    let first = server_sides.pop().unwrap();
    let srv = tokio::spawn(async move {
        let mut s = Server::new(first);
        let init1 = s.session_init().await;
        let login1 = s.login(1).await;
        drop(s); // the connection drops
        let mut s = Server::new(second);
        let init2 = s.session_init().await;
        s.login(1).await;
        (init1, login1, init2, s)
    });

    let (handle, mut events) = Supervisor::spawn(config(), connector, p, env());
    let mut seen = Vec::new();
    while seen.iter().filter(|e| **e == "logged in").count() < 2 {
        let e = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        seen.push(match e {
            CoreEvent::Connected => "connected",
            CoreEvent::LoggedIn(_) => "logged in",
            CoreEvent::Disconnected(_) => "disconnected",
            other => panic!("unexpected {other:?}"),
        });
    }
    assert_eq!(
        seen,
        [
            "connected",
            "logged in",
            "disconnected",
            "connected",
            "logged in"
        ]
    );
    let (init1, login1, init2, _keep) = srv.await.unwrap();
    assert_eq!(get(&login1, "token").as_str(), Some("saved-token"));
    assert_eq!(get(&init1, "clientSessionId").as_i64(), Some(1));
    assert_eq!(get(&init2, "clientSessionId").as_i64(), Some(2));
    handle.stop();
}

#[tokio::test]
async fn supervisor_without_token_asks_for_login() {
    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let (_tx, rx) = mpsc::unbounded_channel();
    let connector = Arc::new(QueueConnector {
        streams: Mutex::new(rx),
    });
    let (_handle, mut events) =
        Supervisor::spawn(config(), connector, profile(&root, &secrets), env());
    let e = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(e, CoreEvent::AuthRequired), "{e:?}");
}
