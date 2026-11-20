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
            ("altActionDuration", 60_000i64.into()),
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
    assert_eq!(req.alt_action_after, 60, "altActionDuration comes in ms");
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
                ("altActionDuration", 30_000i64.into()),
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

#[tokio::test]
async fn history_send_and_new_message_push() {
    use max_core::messages::{history, new_message, send_text};
    use max_transport::Incoming;

    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let mut p = profile(&root, &secrets);
    let (client, server) = tokio::io::duplex(1 << 16);
    let srv = tokio::spawn(async move {
        let mut s = Server::new(server);
        s.session_init().await;
        let (seq, body) = s.expect(Opcode::ChatHistory).await;
        assert_eq!(get(&body, "chatId").as_i64(), Some(42));
        assert_eq!(get(&body, "from").as_i64(), Some(1_000));
        assert_eq!(get(&body, "backward").as_i64(), Some(40));
        assert_eq!(get(&body, "forward").as_i64(), Some(0));
        let msg = |id: i64, time: i64, text: &str| {
            map(&[
                ("id", id.into()),
                ("time", time.into()),
                ("type", "USER".into()),
                ("sender", 2i64.into()),
                ("text", text.into()),
            ])
        };
        // Newest first on the wire; the client sorts.
        s.reply(
            Opcode::ChatHistory,
            seq,
            &map(&[(
                "messages",
                Value::Array(vec![msg(2, 900, "b"), msg(1, 800, "a")]),
            )]),
        )
        .await;
        let (seq, body) = s.expect(Opcode::MessagingSend).await;
        assert_eq!(get(&body, "chatId").as_i64(), Some(42));
        assert_eq!(get(&body, "notify").as_bool(), Some(true));
        let m = get(&body, "message");
        assert_eq!(get(m, "text").as_str(), Some("привет"));
        assert_eq!(get(m, "cid").as_i64(), Some(77));
        let stored = map(&[
            ("id", 3i64.into()),
            ("time", 1_100i64.into()),
            ("type", "USER".into()),
            ("cid", 77i64.into()),
            ("text", "привет".into()),
        ]);
        s.reply(
            Opcode::MessagingSend,
            seq,
            &map(&[("chatId", 42i64.into()), ("message", stored)]),
        )
        .await;
        // A push about a new incoming message (cmd 0, as PyMax reports for events).
        let push = map(&[
            ("chatId", 42i64.into()),
            ("message", msg(4, 1_200, "ответ")),
        ]);
        let (flags, packed) = compress_server_payload(&to_msgpack(&push).unwrap()).unwrap();
        let f = Frame::new(
            PROTOCOL_VERSION,
            Cmd::Request,
            0,
            Opcode::NotificationMessage.as_u16(),
            flags,
            packed,
        )
        .unwrap();
        s.io.write_all(&f.encode()).await.unwrap();
        s
    });

    let mut session = Session::start(client, &config(), &mut p, &env())
        .await
        .unwrap();
    let messages = history(session.transport(), 42, 1_000, 40).await.unwrap();
    let ids: Vec<i64> = messages.iter().map(|m| m.id).collect();
    assert_eq!(ids, [1, 2], "oldest first");
    let sent = send_text(session.transport(), 42, "привет", 77)
        .await
        .unwrap();
    assert_eq!(
        (sent.chat_id, sent.message.id, sent.message.cid),
        (42, 3, Some(77))
    );
    let push = match tokio::time::timeout(Duration::from_secs(5), session.pushes().recv())
        .await
        .unwrap()
        .unwrap()
    {
        Incoming::Push(p) => p,
        other => panic!("expected a push, got {other:?}"),
    };
    let new = new_message(&push).expect("a NOTIF_MESSAGE");
    assert_eq!(
        (new.chat_id, new.message.id, new.message.text.as_deref()),
        (42, 4, Some("ответ"))
    );
    let _keep = srv.await.unwrap();
}

/// A one-request HTTP/1.1 server on loopback: returns the raw request and answers `body`.
async fn http_once(body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/upload", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut req = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            req.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&req).to_string();
            if let Some(head_end) = text.find("\r\n\r\n") {
                let len: usize = text[..head_end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if req.len() >= head_end + 4 + len {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let reply = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        sock.write_all(reply.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&req).to_string()
    });
    (url, task)
}

#[tokio::test]
async fn photo_and_file_upload_then_send() {
    use max_core::media::{
        Http, OutgoingAttachment, attach_ready, send_attachment, upload_file, upload_photo,
    };
    use max_transport::Incoming;

    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let mut p = profile(&root, &secrets);
    let (photo_url, photo_http) = http_once(r#"{"photos":{"55":{"token":"ptok"}}}"#).await;
    let (file_url, file_http) = http_once("{}").await;
    let (client, server) = tokio::io::duplex(1 << 16);
    let srv = tokio::spawn(async move {
        let mut s = Server::new(server);
        s.session_init().await;
        let (seq, body) = s.expect(Opcode::PhotoUpload).await;
        assert_eq!(get(&body, "count").as_i64(), Some(1));
        s.reply(
            Opcode::PhotoUpload,
            seq,
            &map(&[("url", photo_url.as_str().into())]),
        )
        .await;
        let (seq, body) = s.expect(Opcode::MessagingSend).await;
        let attach = &get(get(&body, "message"), "attaches").as_array().unwrap()[0];
        assert_eq!(get(attach, "_type").as_str(), Some("PHOTO"));
        assert_eq!(get(attach, "photoToken").as_str(), Some("ptok"));
        s.reply(
            Opcode::MessagingSend,
            seq,
            &map(&[
                ("chatId", 42i64.into()),
                ("message", map(&[("id", 8i64.into())])),
            ]),
        )
        .await;
        let (seq, _) = s.expect(Opcode::FileUpload).await;
        let info = map(&[
            ("fileId", 900i64.into()),
            ("url", file_url.as_str().into()),
            ("token", "ftok".into()),
        ]);
        s.reply(
            Opcode::FileUpload,
            seq,
            &map(&[("info", Value::Array(vec![info]))]),
        )
        .await;
        // The server confirms the processed file with NOTIF_ATTACH.
        let push = map(&[("fileId", 900i64.into())]);
        let f = Frame::new(
            PROTOCOL_VERSION,
            Cmd::Request,
            0,
            Opcode::NotificationAttach.as_u16(),
            0,
            to_msgpack(&push).unwrap(),
        )
        .unwrap();
        s.io.write_all(&f.encode()).await.unwrap();
        s
    });

    let mut session = Session::start(client, &config(), &mut p, &env())
        .await
        .unwrap();
    let http = Http::new("OneMe Desktop/26.41.0.83760").unwrap();
    let token = upload_photo(
        session.transport(),
        &http,
        bytes::Bytes::from_static(b"\x89PNGfake"),
        "a.png",
        "image/png",
    )
    .await
    .unwrap();
    assert_eq!(token, "ptok");
    let photo_req = photo_http.await.unwrap();
    assert!(photo_req.starts_with("POST /upload "), "{photo_req}");
    assert!(
        photo_req.contains("name=\"file\"") && photo_req.contains("filename=\"a.png\""),
        "{photo_req}"
    );
    assert!(
        photo_req
            .to_ascii_lowercase()
            .contains("user-agent: oneme desktop/26.41.0.83760")
    );
    let sent = send_attachment(
        session.transport(),
        42,
        &OutgoingAttachment::Photo { token },
        None,
        1,
    )
    .await
    .unwrap();
    assert_eq!(sent.message.id, 8);

    let file_id = upload_file(
        session.transport(),
        &http,
        bytes::Bytes::from_static(b"hello"),
        "отчёт.txt",
    )
    .await
    .unwrap();
    assert_eq!(file_id, 900);
    let file_req = file_http.await.unwrap();
    assert!(
        file_req.contains("content-range: 0-4/5") || file_req.contains("Content-Range: 0-4/5"),
        "{file_req}"
    );
    assert!(
        file_req.contains("filename=%D0%BE%D1%82%D1%87%D1%91%D1%82.txt"),
        "{file_req}"
    );
    assert!(file_req.ends_with("hello"));
    let push = match tokio::time::timeout(Duration::from_secs(5), session.pushes().recv())
        .await
        .unwrap()
        .unwrap()
    {
        Incoming::Push(p) => p,
        other => panic!("expected a push, got {other:?}"),
    };
    assert_eq!(attach_ready(&push), Some((900, None)));
    let _keep = srv.await.unwrap();
}

#[tokio::test]
async fn older_page_and_mark_read() {
    use max_core::messages::{history, mark_read};

    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let mut p = profile(&root, &secrets);
    let (client, server) = tokio::io::duplex(1 << 16);
    let srv = tokio::spawn(async move {
        let mut s = Server::new(server);
        s.session_init().await;
        // Older page: `from` is the time of the oldest message shown.
        let (seq, body) = s.expect(Opcode::ChatHistory).await;
        assert_eq!(get(&body, "from").as_i64(), Some(800));
        let old = map(&[
            ("id", 0i64.into()),
            ("time", 700i64.into()),
            ("type", "USER".into()),
            ("text", "старое".into()),
        ]);
        s.reply(
            Opcode::ChatHistory,
            seq,
            &map(&[("messages", Value::Array(vec![old]))]),
        )
        .await;
        let (seq, body) = s.expect(Opcode::ChatMark).await;
        assert_eq!(get(&body, "chatId").as_i64(), Some(42));
        assert_eq!(get(&body, "messageId").as_i64(), Some(2));
        assert_eq!(get(&body, "type").as_str(), Some("READ_MESSAGE"));
        assert!(
            get(&body, "mark").as_i64().unwrap() > 1_700_000_000_000,
            "mark is the current time in ms"
        );
        s.reply(Opcode::ChatMark, seq, &map(&[("unread", 0i64.into())]))
            .await;
        s
    });
    let session = Session::start(client, &config(), &mut p, &env())
        .await
        .unwrap();
    let older = history(session.transport(), 42, 800, 40).await.unwrap();
    assert_eq!(older[0].text.as_deref(), Some("старое"));
    let r = mark_read(session.transport(), 42, 2).await.unwrap();
    assert_eq!(r.unread, Some(0));
    let _keep = srv.await.unwrap();
}

#[tokio::test]
async fn contact_info_sends_ids_only() {
    use max_core::contacts::contact_info;

    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let mut p = profile(&root, &secrets);
    let (client, server) = tokio::io::duplex(1 << 16);
    let srv = tokio::spawn(async move {
        let mut s = Server::new(server);
        s.session_init().await;
        let (seq, body) = s.expect(Opcode::ContactInfo).await;
        let ids: Vec<i64> = get(&body, "contactIds")
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        assert_eq!(ids, [7, 9]);
        assert_eq!(body.as_map().unwrap().len(), 1, "only contactIds");
        let anna = map(&[
            ("id", 7i64.into()),
            (
                "names",
                Value::Array(vec![map(&[("firstName", "Анна".into())])]),
            ),
        ]);
        s.reply(
            Opcode::ContactInfo,
            seq,
            &map(&[("contacts", Value::Array(vec![anna]))]),
        )
        .await;
        s
    });
    let session = Session::start(client, &config(), &mut p, &env())
        .await
        .unwrap();
    let found = contact_info(session.transport(), &[7, 9]).await.unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, 7);
    let _keep = srv.await.unwrap();
}

#[tokio::test]
async fn sessions_close_others_and_logout() {
    use max_core::account::sessions;

    let root = tempfile::tempdir().unwrap();
    let secrets: Arc<dyn SecretStore> = Arc::new(MemorySecrets::new());
    let p = profile(&root, &secrets);
    p.set_auth_token("saved-token").unwrap();
    let tokens = p.token_store();

    let (tx, rx) = mpsc::unbounded_channel();
    let connector = Arc::new(QueueConnector {
        streams: Mutex::new(rx),
    });
    let (client, server) = tokio::io::duplex(1 << 16);
    tx.send(client).unwrap();
    let srv = tokio::spawn(async move {
        let mut s = Server::new(server);
        s.session_init().await;
        s.login(1).await;
        let (seq, body) = s.expect(Opcode::SessionsInfo).await;
        assert_eq!(body.as_map().unwrap().len(), 0, "no parameters");
        let session = |client: &str, current: bool| {
            map(&[
                ("time", 1_000i64.into()),
                ("client", client.into()),
                ("info", "Windows".into()),
                ("location", "RU".into()),
                ("current", current.into()),
            ])
        };
        s.reply(
            Opcode::SessionsInfo,
            seq,
            &map(&[(
                "sessions",
                Value::Array(vec![
                    session("MAX Desktop", true),
                    session("MAX Android", false),
                ]),
            )]),
        )
        .await;
        let (seq, body) = s.expect(Opcode::SessionsClose).await;
        assert_eq!(body.as_map().unwrap().len(), 0, "no parameters");
        s.reply(
            Opcode::SessionsClose,
            seq,
            &map(&[("token", "new-token".into())]),
        )
        .await;
        let (seq, _) = s.expect(Opcode::Logout).await;
        s.reply(Opcode::Logout, seq, &map(&[])).await;
        s
    });

    let (handle, mut events) = Supervisor::spawn(config(), connector, p, env());
    loop {
        let e = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .unwrap()
            .unwrap();
        if matches!(e, CoreEvent::LoggedIn(_)) {
            break;
        }
    }
    let list = sessions(&handle.transport().unwrap()).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].current, Some(true));
    handle.close_other_sessions().await.unwrap();
    assert_eq!(tokens.get().unwrap().unwrap().as_str(), "new-token");
    handle.logout().await.unwrap();
    assert!(tokens.get().unwrap().is_none(), "token forgotten");
    assert!(handle.transport().is_err(), "stopped");
    let _keep = srv.await.unwrap();
}
