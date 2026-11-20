//! The bridge between the UI and `max-core`: a tokio runtime runs the login flow and the
//! supervisor, and reports [`UiEvent`]s; the UI sends [`UiCommand`]s back. `--demo` runs the
//! same screens on made-up data without any network access.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use max_core::media::{self, Attachment, Http, OutgoingAttachment};
use max_core::messages;
use max_core::{
    AuthStep, CodeRequest, CoreConfig, CoreError, CoreEvent, LoginResponse, QrLogin, QrPoll,
    Session, Supervisor, TlsConnector,
};
use max_identity::Environment;
use max_proto::schema::types::{Chat, Contact};
use max_store::{OsSecrets, Profile, SecretStore, default_profiles_dir};
use max_transport::TransportError;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// What the UI shows next.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// A short status line ("connecting", "reconnecting").
    Status(String),
    /// Show this link as a QR code.
    Qr(String),
    /// Ask for a phone number; `error` explains a failed attempt.
    AskPhone { error: Option<String> },
    /// Ask for the code sent to `phone`.
    AskCode {
        phone: String,
        length: i64,
        resend_after: i64,
        error: Option<String>,
    },
    /// The account has a 2FA password; `email` is the recovery address if known.
    NeedPassword {
        email: Option<String>,
        error: Option<String>,
    },
    /// Logged in: who we are and the chat list.
    LoggedIn { me: String, chats: Vec<ChatRow> },
    /// The history of a chat, oldest first; `has_more` when older messages exist.
    History {
        chat_id: i64,
        messages: Vec<MessageRow>,
        has_more: bool,
    },
    /// An older page of a chat, oldest first.
    Older {
        chat_id: i64,
        messages: Vec<MessageRow>,
        has_more: bool,
    },
    /// Loading a chat's history failed.
    HistoryFailed { chat_id: i64, error: String },
    /// A new message in a chat: received, or ours confirmed by the server (same `cid`).
    MessageAdded { chat_id: i64, message: MessageRow },
    /// Sending the message with this `cid` failed.
    SendFailed {
        chat_id: i64,
        cid: i64,
        error: String,
    },
    /// Image bytes for a photo or sticker address.
    Image { url: String, bytes: bytes::Bytes },
    /// An image could not be loaded.
    ImageFailed { url: String },
    /// A short note for the status line ("saved to ...").
    Notice(String),
    /// Something failed; shown with a way to start over.
    Error(String),
}

/// What the UI asks the backend to do.
#[derive(Debug, Clone)]
pub enum UiCommand {
    /// Switch to phone number login.
    UsePhone,
    /// Switch back to QR login.
    UseQr,
    /// The phone number the user typed.
    Phone(String),
    /// The code the user typed.
    Code(String),
    /// Send the code again.
    Resend,
    /// The 2FA password the user typed.
    Password(String),
    /// Load the history of a chat.
    OpenChat(i64),
    /// Send text to a chat; `cid` identifies the pending copy the UI already shows.
    SendText {
        chat_id: i64,
        text: String,
        cid: i64,
    },
    /// Load an image for display.
    FetchImage(String),
    /// Save a file attachment to the Downloads folder.
    DownloadFile {
        chat_id: i64,
        message_id: i64,
        file_id: i64,
        name: String,
    },
    /// Send a file from disk: images as photos, anything else as a file.
    SendFile {
        chat_id: i64,
        path: std::path::PathBuf,
        cid: i64,
    },
    /// Load messages older than `before` (Unix ms).
    LoadOlder { chat_id: i64, before: i64 },
    /// Tell the server the chat is read up to `message_id`.
    MarkRead { chat_id: i64, message_id: i64 },
}

/// Delivery state of a message in the history view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// On the server.
    Sent,
    /// Sent by us, no answer yet.
    Pending,
    /// The server refused it or the connection failed.
    Failed,
}

/// One message as the history view shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    /// Server id (0 while pending).
    pub id: i64,
    /// Client id of our own messages.
    pub cid: Option<i64>,
    /// Written by us.
    pub out: bool,
    /// Sender's name.
    pub sender: String,
    /// Text, or a short description of the attachments.
    pub text: String,
    /// Unix ms.
    pub time: i64,
    /// Delivery state.
    pub delivery: Delivery,
    /// Photos, files and other attachments.
    pub attachments: Vec<Attachment>,
}

impl MessageRow {
    /// One line for the chat list: the text, or what is attached.
    pub fn preview(&self) -> String {
        if !self.text.is_empty() {
            return self.text.clone();
        }
        self.attachments
            .first()
            .map(|a| match a {
                Attachment::Photo { .. } => "Фото".to_string(),
                Attachment::File { name, .. } => format!("Файл: {name}"),
                Attachment::Sticker { .. } => "Стикер".to_string(),
                Attachment::Other { kind } => attachment_label(kind).to_string(),
            })
            .unwrap_or_default()
    }
}

/// One row of the chat list.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRow {
    /// Chat id.
    pub id: i64,
    /// Title, or the other person's name for a dialog.
    pub title: String,
    /// Text of the last message.
    pub preview: String,
    /// Unread messages.
    pub unread: i32,
    /// Time of the last event, Unix ms.
    pub time: i64,
}

/// Handles to a running backend.
pub struct Backend {
    /// Keeps the tokio runtime alive for as long as the app runs.
    pub(crate) _runtime: Runtime,
    /// Events for the UI.
    pub events: UnboundedReceiver<UiEvent>,
    /// Commands from the UI.
    pub commands: UnboundedSender<UiCommand>,
}

impl Backend {
    /// Start the backend; `demo` uses made-up data and no network, `walk` steps through every
    /// demo screen on a timer.
    pub fn start(demo: bool, walk: bool) -> std::io::Result<Backend> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("maxrs-core")
            .enable_all()
            .build()?;
        let (ui_tx, events) = unbounded_channel();
        let (commands, cmd_rx) = unbounded_channel();
        if demo {
            runtime.spawn(demo_flow(ui_tx, cmd_rx, walk));
        } else {
            runtime.spawn(async move {
                if let Err(e) = real_flow(ui_tx.clone(), cmd_rx).await {
                    let _ = ui_tx.send(UiEvent::Error(e));
                }
            });
        }
        Ok(Backend {
            _runtime: runtime,
            events,
            commands,
        })
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// A readable message for a failed step: the server's own text when it sent one.
fn message(e: &CoreError) -> String {
    if let CoreError::Transport(TransportError::Server { payload, .. }) = e
        && let Some(map) = payload.as_map()
    {
        for key in ["localizedMessage", "message", "error"] {
            if let Some(text) = map
                .iter()
                .find(|(k, _)| k.as_str() == Some(key))
                .and_then(|(_, v)| v.as_str())
            {
                return text.to_string();
            }
        }
    }
    e.to_string()
}

// ---- demo --------------------------------------------------------------------------------------

async fn demo_flow(
    ui: UnboundedSender<UiEvent>,
    mut commands: UnboundedReceiver<UiCommand>,
    walk: bool,
) {
    let _ = ui.send(UiEvent::Status("Подключение…".into()));
    tokio::time::sleep(Duration::from_millis(400)).await;
    let _ = ui.send(UiEvent::Qr(
        "https://max.ru/:auth/demo-only-not-a-real-login".into(),
    ));
    if walk {
        // Step through every screen on a timer, for screenshots without input.
        let steps = [
            UiEvent::AskPhone { error: None },
            UiEvent::AskCode {
                phone: "+79001234567".into(),
                length: 6,
                resend_after: 60,
                error: Some("Неверный код".into()),
            },
            UiEvent::NeedPassword {
                email: Some("i***@example.com".into()),
                error: None,
            },
        ];
        for step in steps {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let _ = ui.send(step);
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    let mut phone = String::new();
    while !walk && let Some(cmd) = commands.recv().await {
        match cmd {
            UiCommand::UsePhone => {
                let _ = ui.send(UiEvent::AskPhone { error: None });
            }
            UiCommand::UseQr => {
                let _ = ui.send(UiEvent::Qr(
                    "https://max.ru/:auth/demo-only-not-a-real-login".into(),
                ));
            }
            UiCommand::Phone(p) => {
                phone = p;
                let _ = ui.send(UiEvent::AskCode {
                    phone: phone.clone(),
                    length: 6,
                    resend_after: 60,
                    error: None,
                });
            }
            UiCommand::Resend => {
                let _ = ui.send(UiEvent::AskCode {
                    phone: phone.clone(),
                    length: 6,
                    resend_after: 60,
                    error: None,
                });
            }
            UiCommand::Code(code) if code == "000000" => {
                let _ = ui.send(UiEvent::AskCode {
                    phone: phone.clone(),
                    length: 6,
                    resend_after: 60,
                    error: Some("Неверный код (демо: подойдёт любой, кроме 000000)".into()),
                });
            }
            UiCommand::Code(_) | UiCommand::Password(_) => break,
            UiCommand::OpenChat(_) | UiCommand::SendText { .. } => {}
            UiCommand::LoadOlder { .. } | UiCommand::MarkRead { .. } => {}
            UiCommand::FetchImage(_)
            | UiCommand::DownloadFile { .. }
            | UiCommand::SendFile { .. } => {}
        }
    }
    let t = now_ms();
    let chats = [
        ("Анна Смирнова", "До завтра! 👋", 2, 5),
        ("Команда проекта", "Иван: выкладываю сборку", 0, 40),
        ("Избранное", "Список покупок", 0, 300),
        (
            "Новости района",
            "Плановое отключение воды в четверг",
            12,
            900,
        ),
        ("Пётр", "Ок", 0, 3000),
    ];
    let chats = chats
        .iter()
        .enumerate()
        .map(|(i, (title, preview, unread, minutes_ago))| ChatRow {
            id: i as i64 + 1,
            title: (*title).into(),
            preview: (*preview).into(),
            unread: *unread,
            time: t - minutes_ago * 60_000,
        })
        .collect();
    let _ = ui.send(UiEvent::LoggedIn {
        me: "Демо-режим".into(),
        chats,
    });
    if walk {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = ui.send(UiEvent::History {
            chat_id: 1,
            messages: demo_history(1, t),
            has_more: true,
        });
        tokio::time::sleep(Duration::from_millis(800)).await;
        let cid = now_ms();
        let _ = ui.send(UiEvent::MessageAdded {
            chat_id: 1,
            message: demo_row(
                cid,
                true,
                "Отлично, тогда в 10 у входа",
                now_ms(),
                Delivery::Pending,
            ),
        });
    }
    // Serve the chat screen: history, sending, images and files.
    while let Some(cmd) = commands.recv().await {
        match cmd {
            UiCommand::OpenChat(chat_id) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let _ = ui.send(UiEvent::History {
                    chat_id,
                    messages: demo_history(chat_id, t),
                    has_more: true,
                });
            }
            UiCommand::LoadOlder { chat_id, before } => {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let (messages, has_more) = demo_older(chat_id, before, t);
                let _ = ui.send(UiEvent::Older {
                    chat_id,
                    messages,
                    has_more,
                });
            }
            UiCommand::SendText { chat_id, text, cid } => {
                let ui = ui.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let mut row = demo_row(cid, true, &text, now_ms(), Delivery::Sent);
                    row.id = cid;
                    row.cid = Some(cid);
                    let _ = ui.send(UiEvent::MessageAdded {
                        chat_id,
                        message: row,
                    });
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let reply = demo_row(cid + 1, false, "👍", now_ms(), Delivery::Sent);
                    let _ = ui.send(UiEvent::MessageAdded {
                        chat_id,
                        message: reply,
                    });
                });
            }
            UiCommand::FetchImage(url) => {
                let bytes = if url == "demo://photo" {
                    Some(demo_photo_svg())
                } else {
                    // A photo the user picked in the demo: read it locally, nothing leaves.
                    tokio::fs::read(url.trim_start_matches("demo-file://"))
                        .await
                        .ok()
                        .map(bytes::Bytes::from)
                };
                let _ = ui.send(match bytes {
                    Some(bytes) => UiEvent::Image { url, bytes },
                    None => UiEvent::ImageFailed { url },
                });
            }
            UiCommand::DownloadFile { name, .. } => {
                let _ = ui.send(UiEvent::Notice(format!("Демо: {name} не скачивается")));
            }
            UiCommand::SendFile { chat_id, path, cid } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                let size = tokio::fs::metadata(&path)
                    .await
                    .map(|m| m.len())
                    .unwrap_or(0);
                let mut row = demo_row(cid, true, "", now_ms(), Delivery::Sent);
                row.attachments = vec![if media::is_photo_name(&name) {
                    Attachment::Photo {
                        photo_id: None,
                        url: format!("demo-file://{}", path.display()),
                        width: 0,
                        height: 0,
                    }
                } else {
                    Attachment::File {
                        file_id: cid,
                        name,
                        size: i64::try_from(size).unwrap_or(0),
                    }
                }];
                tokio::time::sleep(Duration::from_millis(400)).await;
                let _ = ui.send(UiEvent::MessageAdded {
                    chat_id,
                    message: row,
                });
            }
            _ => {}
        }
    }
}

fn demo_row(id: i64, out: bool, text: &str, time: i64, delivery: Delivery) -> MessageRow {
    MessageRow {
        id,
        cid: out.then_some(id),
        out,
        sender: if out {
            "Вы".into()
        } else {
            "Анна Смирнова".into()
        },
        text: text.into(),
        time,
        delivery,
        attachments: Vec::new(),
    }
}

/// A made-up photo for the demo: an SVG drawn in memory, no network.
fn demo_photo_svg() -> bytes::Bytes {
    bytes::Bytes::from_static(
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="420" viewBox="0 0 640 420">
<defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#7cb7ff"/><stop offset="1" stop-color="#d9ecff"/></linearGradient></defs>
<rect width="640" height="420" fill="url(#g)"/>
<circle cx="520" cy="90" r="46" fill="#ffd54f"/>
<path d="M0 330 L150 210 L260 300 L380 170 L640 340 L640 420 L0 420 Z" fill="#4e8a5a"/>
<path d="M0 360 L200 280 L330 350 L470 260 L640 370 L640 420 L0 420 Z" fill="#2f6b3d"/>
</svg>"##,
    )
}

/// Older made-up messages: two more pages, then the start of the chat.
fn demo_older(chat_id: i64, before: i64, now: i64) -> (Vec<MessageRow>, bool) {
    let page = ((now - before) / (24 * 3_600_000)).clamp(0, 2);
    let base = before - 24 * 3_600_000;
    let lines = [
        (false, "Как съездили?"),
        (true, "Отлично, фото пришлю позже"),
        (false, "Жду!"),
        (true, "Кстати, на выходных свободна?"),
    ];
    let rows = lines
        .iter()
        .enumerate()
        .map(|(i, (out, text))| {
            let mut row = demo_row(
                chat_id * 1000 - page * 10 - i as i64,
                *out,
                text,
                base + i as i64 * 600_000,
                Delivery::Sent,
            );
            row.cid = None;
            row
        })
        .collect();
    (rows, page < 1)
}

/// A short made-up conversation ending a few minutes before `now`.
fn demo_history(chat_id: i64, now: i64) -> Vec<MessageRow> {
    let lines: &[(bool, &str, i64)] = &[
        (false, "Привет! Завтра всё в силе?", 95),
        (true, "Привет, да. Во сколько удобно?", 90),
        (false, "Давай с утра, часов в 10", 60),
        (true, "Договорились", 58),
        (false, "Вот место, где встретимся", 20),
        (false, "До завтра! 👋", 5),
    ];
    lines
        .iter()
        .enumerate()
        .map(|(i, (out, text, minutes_ago))| {
            let mut row = demo_row(
                chat_id * 100 + i as i64,
                *out,
                text,
                now - minutes_ago * 60_000,
                Delivery::Sent,
            );
            row.cid = None;
            if i == 4 {
                row.attachments = vec![Attachment::Photo {
                    photo_id: None,
                    url: "demo://photo".into(),
                    width: 640,
                    height: 420,
                }];
            }
            if i == 3 {
                row.attachments = vec![Attachment::File {
                    file_id: 1,
                    name: "Маршрут.pdf".into(),
                    size: 245_760,
                }];
            }
            row
        })
        .collect()
}

// ---- real ----------------------------------------------------------------------------------

async fn real_flow(
    ui: UnboundedSender<UiEvent>,
    mut commands: UnboundedReceiver<UiCommand>,
) -> Result<(), String> {
    let root = default_profiles_dir().ok_or("no local data folder")?;
    let secrets: Arc<dyn SecretStore> = Arc::new(OsSecrets::new("maxrs"));
    let mut profile = Profile::open(&root, "default", secrets).map_err(|e| e.to_string())?;
    let env = Environment::detect();
    let mut config = CoreConfig::default();
    config.transport.trace = crate::trace::transport_trace();

    if profile.auth_token().map_err(|e| e.to_string())?.is_none() {
        let _ = ui.send(UiEvent::Status("Подключение…".into()));
        let session = Session::connect(&config, &mut profile, &env)
            .await
            .map_err(|e| e.to_string())?;
        let token = login(&session, &ui, &mut commands, &env.system_language).await?;
        profile.set_auth_token(&token).map_err(|e| e.to_string())?;
        session.transport().close();
    }

    let (handle, mut events) = Supervisor::spawn(
        config.clone(),
        Arc::new(TlsConnector::new(config)),
        profile,
        env,
    );
    let names = Arc::new(RwLock::new(Names::default()));
    let http =
        Http::new(&CoreConfig::default().version.http_user_agent()).map_err(|e| e.to_string())?;
    let waiters: Arc<std::sync::Mutex<FileWaiters>> = Arc::default();
    loop {
        tokio::select! {
            event = events.recv() => {
                let Some(event) = event else { break };
                let msg = match event {
                    CoreEvent::Connected => UiEvent::Status("Вход…".into()),
                    CoreEvent::LoggedIn(login) => {
                        *names.write().unwrap_or_else(PoisonError::into_inner) = Names::from_login(&login);
                        let (me, chats) = rows_from_login(&login);
                        UiEvent::LoggedIn { me, chats }
                    }
                    CoreEvent::AuthRequired => UiEvent::Error("Нужно войти заново.".into()),
                    CoreEvent::Push(push) if media::attach_ready(&push).is_some() => {
                        if let Some((file_id, error)) = media::attach_ready(&push)
                            && let Some(tx) = waiters.lock().unwrap_or_else(PoisonError::into_inner).remove(&file_id)
                        {
                            let _ = tx.send(error);
                        }
                        continue;
                    }
                    CoreEvent::Push(push) => match messages::new_message(&push) {
                        Some(new) => {
                            let names = names.read().unwrap_or_else(PoisonError::into_inner);
                            UiEvent::MessageAdded { chat_id: new.chat_id, message: names.row(&new.message) }
                        }
                        None => continue,
                    },
                    CoreEvent::Disconnected(reason) => {
                        UiEvent::Status(format!("Нет связи, переподключение… ({reason})"))
                    }
                };
                let _ = ui.send(msg);
            }
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { break };
                let (handle, ui, names) = (handle.clone(), ui.clone(), names.clone());
                let (http, waiters) = (http.clone(), waiters.clone());
                match cmd {
                    UiCommand::FetchImage(url) => {
                        tokio::spawn(async move {
                            let _ = ui.send(match http.download(&url, media::MAX_DOWNLOAD).await {
                                Ok(bytes) => {
                                    crate::trace::note(&format!("image {url} ok, {} bytes", bytes.len()));
                                    UiEvent::Image { url, bytes }
                                }
                                Err(e) => {
                                    crate::trace::note(&format!("image {url} failed: {e}"));
                                    UiEvent::ImageFailed { url }
                                }
                            });
                        });
                    }
                    UiCommand::DownloadFile { chat_id, message_id, file_id, name } => {
                        tokio::spawn(async move {
                            let result = async {
                                let t = handle.transport()?;
                                let url = media::file_download_url(&t, chat_id, message_id, file_id).await?;
                                http.download(&url, media::MAX_DOWNLOAD).await
                            }
                            .await;
                            let _ = ui.send(match result {
                                Ok(bytes) => match save_download(&name, &bytes) {
                                    Ok(path) => UiEvent::Notice(format!("Сохранено: {}", path.display())),
                                    Err(e) => UiEvent::Notice(format!("Не удалось сохранить {name}: {e}")),
                                },
                                Err(e) => {
                                    crate::trace::note(&format!("file download failed: {e}"));
                                    UiEvent::Notice(format!("Не удалось скачать {name}: {}", message(&e)))
                                }
                            });
                        });
                    }
                    UiCommand::SendFile { chat_id, path, cid } => {
                        tokio::spawn(async move {
                            let result = send_file(&handle, &http, &waiters, chat_id, &path, cid).await;
                            let _ = ui.send(match result {
                                Ok(sent) => {
                                    let names = names.read().unwrap_or_else(PoisonError::into_inner);
                                    let mut row = names.row(&sent.message);
                                    row.cid = Some(cid);
                                    UiEvent::MessageAdded { chat_id, message: row }
                                }
                                Err(e) => {
                                    crate::trace::note(&format!("send file failed: {e}"));
                                    UiEvent::SendFailed { chat_id, cid, error: e }
                                }
                            });
                        });
                    }
                    UiCommand::OpenChat(chat_id) => {
                        tokio::spawn(async move {
                            let result = match handle.transport() {
                                Ok(t) => messages::history(&t, chat_id, now_ms(), messages::HISTORY_PAGE).await,
                                Err(e) => Err(e),
                            };
                            let _ = ui.send(match result {
                                Ok(list) => {
                                    let names = names.read().unwrap_or_else(PoisonError::into_inner);
                                    let has_more = list.len() >= usize::try_from(messages::HISTORY_PAGE).unwrap_or(40);
                                    UiEvent::History { chat_id, messages: list.iter().map(|m| names.row(m)).collect(), has_more }
                                }
                                Err(e) => UiEvent::HistoryFailed { chat_id, error: message(&e) },
                            });
                        });
                    }
                    UiCommand::LoadOlder { chat_id, before } => {
                        tokio::spawn(async move {
                            let result = match handle.transport() {
                                Ok(t) => messages::history(&t, chat_id, before, messages::HISTORY_PAGE).await,
                                Err(e) => Err(e),
                            };
                            let _ = ui.send(match result {
                                Ok(list) => {
                                    let names = names.read().unwrap_or_else(PoisonError::into_inner);
                                    let has_more = list.len() >= usize::try_from(messages::HISTORY_PAGE).unwrap_or(40);
                                    UiEvent::Older { chat_id, messages: list.iter().map(|m| names.row(m)).collect(), has_more }
                                }
                                Err(e) => UiEvent::Notice(format!("Не удалось загрузить ранние сообщения: {}", message(&e))),
                            });
                        });
                    }
                    UiCommand::MarkRead { chat_id, message_id } => {
                        tokio::spawn(async move {
                            if let Ok(t) = handle.transport() {
                                // A failed read mark is retried with the next one; nothing to show.
                                if let Err(e) = messages::mark_read(&t, chat_id, message_id).await {
                                    crate::trace::note(&format!("read mark failed: {e}"));
                                }
                            }
                        });
                    }
                    UiCommand::SendText { chat_id, text, cid } => {
                        tokio::spawn(async move {
                            let result = match handle.transport() {
                                Ok(t) => messages::send_text(&t, chat_id, &text, cid).await,
                                Err(e) => Err(e),
                            };
                            let _ = ui.send(match result {
                                Ok(sent) => {
                                    let names = names.read().unwrap_or_else(PoisonError::into_inner);
                                    let mut row = names.row(&sent.message);
                                    row.cid = Some(cid);
                                    UiEvent::MessageAdded { chat_id, message: row }
                                }
                                Err(e) => UiEvent::SendFailed { chat_id, cid, error: message(&e) },
                            });
                        });
                    }
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

type FileWaiters = std::collections::HashMap<i64, tokio::sync::oneshot::Sender<Option<String>>>;

/// Largest file the client sends.
const MAX_UPLOAD: u64 = 2 * 1024 * 1024 * 1024;

/// Upload a file from disk and send it: images as photos, anything else as a file after the
/// server's NOTIF_ATTACH.
async fn send_file(
    handle: &max_core::SupervisorHandle,
    http: &Http,
    waiters: &std::sync::Mutex<FileWaiters>,
    chat_id: i64,
    path: &std::path::Path,
    cid: i64,
) -> Result<messages::SendResponse, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let size = tokio::fs::metadata(path)
        .await
        .map_err(|e| e.to_string())?
        .len();
    if size > MAX_UPLOAD {
        return Err(format!("{name}: файл больше 2 ГБ"));
    }
    let data = bytes::Bytes::from(tokio::fs::read(path).await.map_err(|e| e.to_string())?);
    let t = handle.transport().map_err(|e| message(&e))?;
    let attachment = if media::is_photo_name(&name) {
        let token = media::upload_photo(&t, http, data, &name, media::photo_mime(&name))
            .await
            .map_err(|e| message(&e))?;
        OutgoingAttachment::Photo { token }
    } else {
        // Register before the upload so a fast NOTIF_ATTACH is not missed.
        let (tx, rx) = tokio::sync::oneshot::channel();
        let file_id = media::upload_file(&t, http, data, &name)
            .await
            .map_err(|e| message(&e))?;
        waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(file_id, tx);
        match tokio::time::timeout(Duration::from_secs(120), rx).await {
            Ok(Ok(None)) => {}
            Ok(Ok(Some(error))) => return Err(format!("сервер не принял файл: {error}")),
            Ok(Err(_)) | Err(_) => {
                waiters
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&file_id);
                return Err("сервер не подтвердил файл".into());
            }
        }
        OutgoingAttachment::File { file_id }
    };
    media::send_attachment(&t, chat_id, &attachment, None, cid)
        .await
        .map_err(|e| message(&e))
}

/// Save bytes into the Downloads folder under a name that does not overwrite anything.
fn save_download(name: &str, bytes: &[u8]) -> std::io::Result<std::path::PathBuf> {
    let dir = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = if clean.trim().is_empty() {
        "file".to_string()
    } else {
        clean
    };
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (clean.clone(), String::new()),
    };
    let mut path = dir.join(&clean);
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stem} ({n}){ext}"));
        n += 1;
    }
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// Who is who, for message rows: our id and contact names from the last LOGIN.
#[derive(Debug, Default)]
pub struct Names {
    me: i64,
    contacts: BTreeMap<i64, String>,
}

impl Names {
    /// From the initial sync.
    pub fn from_login(login: &LoginResponse) -> Names {
        let mut contacts: BTreeMap<i64, String> = login
            .contacts
            .iter()
            .map(|c| (c.id, contact_name(c)))
            .collect();
        contacts.insert(
            login.profile.contact.id,
            contact_name(&login.profile.contact),
        );
        Names {
            me: login.profile.contact.id,
            contacts,
        }
    }

    /// A history row for a message.
    pub fn row(&self, m: &messages::Message) -> MessageRow {
        let sender_id = m.sender.unwrap_or(self.me);
        MessageRow {
            id: m.id,
            cid: m.cid,
            out: sender_id == self.me,
            sender: self
                .contacts
                .get(&sender_id)
                .cloned()
                .unwrap_or_else(|| format!("id {sender_id}")),
            text: bubble_text(m),
            time: m.time,
            delivery: Delivery::Sent,
            attachments: media::attachments(m),
        }
    }
}

/// What an attachment kind is called in the chat list and history.
pub fn attachment_label(kind: &str) -> &'static str {
    match kind {
        "PHOTO" => "Фото",
        "VIDEO" => "Видео",
        "FILE" => "Файл",
        "STICKER" => "Стикер",
        "AUDIO" => "Голосовое сообщение",
        "CONTACT" => "Контакт",
        "LOCATION" => "Геопозиция",
        "POLL" => "Опрос",
        "CONTROL" => "Служебное сообщение",
        _ => "Вложение",
    }
}

/// The text shown inside a bubble: photos, files and stickers are drawn, so only the other
/// attachment kinds get a label.
pub fn bubble_text(m: &messages::Message) -> String {
    let text = m.text.clone().unwrap_or_default();
    let labels: Vec<&'static str> = media::attachments(m)
        .iter()
        .filter_map(|a| match a {
            Attachment::Other { kind } => Some(attachment_label(kind)),
            _ => None,
        })
        .collect();
    match (text.is_empty(), labels.is_empty()) {
        (_, true) => text,
        (false, false) => format!("[{}] {text}", labels.join(", ")),
        (true, false) => format!("[{}]", labels.join(", ")),
    }
}

/// The text of a message, or what its attachments are.
pub fn message_text(m: &messages::Message) -> String {
    let text = m.text.clone().unwrap_or_default();
    let attaches: Vec<&'static str> = m
        .attaches
        .iter()
        .flatten()
        .map(|a| {
            let kind =
                a.0.as_map()
                    .and_then(|map| map.iter().find(|(k, _)| k.as_str() == Some("_type")))
                    .and_then(|(_, v)| v.as_str())
                    .unwrap_or("");
            attachment_label(kind)
        })
        .collect();
    match (text.is_empty(), attaches.is_empty()) {
        (false, true) => text,
        (false, false) => format!("[{}] {text}", attaches.join(", ")),
        (true, false) => format!("[{}]", attaches.join(", ")),
        (true, true) => String::new(),
    }
}

enum Mode {
    Qr,
    Phone,
    Code { phone: String, request: CodeRequest },
}

/// Run QR or phone login, as the user switches between them, until a login token comes back.
async fn login(
    session: &Session,
    ui: &UnboundedSender<UiEvent>,
    commands: &mut UnboundedReceiver<UiCommand>,
    language: &str,
) -> Result<zeroize::Zeroizing<String>, String> {
    let mut mode = Mode::Qr;
    loop {
        let step = match mode {
            Mode::Qr => match qr_round(session, ui, commands).await? {
                QrOutcome::Step(step) => step,
                QrOutcome::UsePhone => {
                    let _ = ui.send(UiEvent::AskPhone { error: None });
                    mode = Mode::Phone;
                    continue;
                }
            },
            Mode::Phone => match commands.recv().await.ok_or("the window was closed")? {
                UiCommand::UseQr => {
                    mode = Mode::Qr;
                    continue;
                }
                UiCommand::Phone(phone) => {
                    match session.request_code(&phone, Some(language), false).await {
                        Ok(request) => {
                            ask_code(ui, &phone, &request, None);
                            mode = Mode::Code { phone, request };
                            continue;
                        }
                        Err(e) => {
                            let _ = ui.send(UiEvent::AskPhone {
                                error: Some(message(&e)),
                            });
                            continue;
                        }
                    }
                }
                _ => continue,
            },
            Mode::Code { phone, request } => {
                match commands.recv().await.ok_or("the window was closed")? {
                    UiCommand::UseQr => {
                        mode = Mode::Qr;
                        continue;
                    }
                    UiCommand::UsePhone => {
                        let _ = ui.send(UiEvent::AskPhone { error: None });
                        mode = Mode::Phone;
                        continue;
                    }
                    UiCommand::Resend => {
                        let request = match session.request_code(&phone, Some(language), true).await
                        {
                            Ok(fresh) => {
                                ask_code(ui, &phone, &fresh, None);
                                fresh
                            }
                            Err(e) => {
                                ask_code(ui, &phone, &request, Some(message(&e)));
                                request
                            }
                        };
                        mode = Mode::Code { phone, request };
                        continue;
                    }
                    UiCommand::Code(code) => match session.confirm_code(&request, &code).await {
                        Ok(step) => step,
                        Err(e) => {
                            ask_code(ui, &phone, &request, Some(message(&e)));
                            mode = Mode::Code { phone, request };
                            continue;
                        }
                    },
                    _ => {
                        mode = Mode::Code { phone, request };
                        continue;
                    }
                }
            }
        };
        match finish(session, ui, commands, step).await? {
            Some(token) => return Ok(token),
            None => mode = Mode::Qr,
        }
    }
}

fn ask_code(
    ui: &UnboundedSender<UiEvent>,
    phone: &str,
    request: &CodeRequest,
    error: Option<String>,
) {
    let _ = ui.send(UiEvent::AskCode {
        phone: phone.to_string(),
        length: request.code_length,
        resend_after: request.alt_action_after,
        error,
    });
}

/// Handle the 2FA password if asked; `None` sends the user back to the start.
async fn finish(
    session: &Session,
    ui: &UnboundedSender<UiEvent>,
    commands: &mut UnboundedReceiver<UiCommand>,
    mut step: AuthStep,
) -> Result<Option<zeroize::Zeroizing<String>>, String> {
    loop {
        match step {
            AuthStep::LoggedIn { token, .. } => return Ok(Some(token)),
            AuthStep::RegistrationRequired => {
                return Err(
                    "У этого номера нет аккаунта MAX; регистрация пока не поддерживается.".into(),
                );
            }
            AuthStep::PasswordRequired(challenge) => {
                let _ = ui.send(UiEvent::NeedPassword {
                    email: challenge.email.clone(),
                    error: None,
                });
                loop {
                    match commands.recv().await.ok_or("the window was closed")? {
                        UiCommand::Password(password) => {
                            match session.check_password(&challenge, &password).await {
                                Ok(next) => {
                                    step = next;
                                    break;
                                }
                                Err(e) => {
                                    let _ = ui.send(UiEvent::NeedPassword {
                                        email: challenge.email.clone(),
                                        error: Some(message(&e)),
                                    });
                                }
                            }
                        }
                        UiCommand::UseQr => return Ok(None),
                        _ => {}
                    }
                }
            }
        }
    }
}

enum QrOutcome {
    Step(AuthStep),
    UsePhone,
}

/// Show QR codes until one is approved, refreshing expired ones; stop if the user picks the
/// phone instead.
async fn qr_round(
    session: &Session,
    ui: &UnboundedSender<UiEvent>,
    commands: &mut UnboundedReceiver<UiCommand>,
) -> Result<QrOutcome, String> {
    loop {
        let qr: QrLogin = session.start_qr().await.map_err(|e| message(&e))?;
        let _ = ui.send(UiEvent::Qr(qr.qr_link.clone()));
        let every =
            Duration::from_millis(u64::try_from(qr.polling_interval_ms.max(1000)).unwrap_or(5000));
        loop {
            tokio::select! {
                cmd = commands.recv() => match cmd {
                    Some(UiCommand::UsePhone) => return Ok(QrOutcome::UsePhone),
                    Some(_) => continue,
                    None => return Err("the window was closed".into()),
                },
                () = tokio::time::sleep(every) => {}
            }
            match session.poll_qr(&qr).await.map_err(|e| message(&e))? {
                QrPoll::Ready => {
                    return session
                        .finish_qr(&qr)
                        .await
                        .map(QrOutcome::Step)
                        .map_err(|e| message(&e));
                }
                QrPoll::Waiting {
                    expires_at: Some(t),
                } if t <= now_ms() => break,
                QrPoll::Waiting { .. } => {}
            }
        }
    }
}

// ---- chat list -------------------------------------------------------------------------------

fn contact_name(c: &Contact) -> String {
    c.names
        .as_ref()
        .and_then(|names| names.first())
        .map(|n| {
            [n.first_name.as_deref(), n.last_name.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("id {}", c.id))
}

/// Chat list rows from the initial sync, newest first.
pub fn rows_from_login(login: &LoginResponse) -> (String, Vec<ChatRow>) {
    let me_id = login.profile.contact.id;
    let contacts: BTreeMap<i64, &Contact> = login.contacts.iter().map(|c| (c.id, c)).collect();
    let mut rows: Vec<ChatRow> = login
        .chats
        .iter()
        .map(|c| row(c, me_id, &contacts))
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.time));
    (contact_name(&login.profile.contact), rows)
}

fn row(chat: &Chat, me_id: i64, contacts: &BTreeMap<i64, &Contact>) -> ChatRow {
    let title = chat
        .title
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            let other = chat
                .participants
                .keys()
                .find(|id| **id != me_id)
                .copied()
                .unwrap_or(me_id);
            if other == me_id {
                "Избранное".to_string()
            } else {
                contacts
                    .get(&other)
                    .map_or_else(|| format!("id {other}"), |c| contact_name(c))
            }
        });
    let preview = chat
        .last_message
        .as_ref()
        .map(message_text)
        .unwrap_or_default();
    ChatRow {
        id: chat.id,
        title,
        preview,
        unread: chat.new_messages.unwrap_or(0),
        time: chat.last_event_time,
    }
}

#[cfg(test)]
mod tests {
    use max_proto::schema::types::{ContactName, Message, Profile as ProfileT};

    use super::*;

    fn contact(id: i64, first: &str) -> Contact {
        Contact {
            id,
            names: Some(vec![ContactName {
                first_name: Some(first.into()),
                ..ContactName::default()
            }]),
            ..Contact::default()
        }
    }

    #[test]
    fn dialog_titles_come_from_contacts_and_rows_are_sorted() {
        let dialog = Chat {
            id: 10,
            participants: [(1, 0), (2, 0)].into_iter().collect(),
            last_event_time: 100,
            new_messages: Some(3),
            last_message: Some(Message {
                text: Some("привет".into()),
                ..Message::default()
            }),
            ..Chat::default()
        };
        let group = Chat {
            id: 11,
            title: Some("Группа".into()),
            last_event_time: 200,
            ..Chat::default()
        };
        let saved = Chat {
            id: 12,
            participants: [(1, 0)].into_iter().collect(),
            last_event_time: 50,
            ..Chat::default()
        };
        let login = LoginResponse {
            profile: ProfileT {
                contact: contact(1, "Я"),
                ..ProfileT::default()
            },
            contacts: vec![contact(2, "Анна")],
            chats: vec![dialog, group, saved],
            ..LoginResponse::default()
        };
        let (me, rows) = rows_from_login(&login);
        assert_eq!(me, "Я");
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Группа", "Анна", "Избранное"]);
        assert_eq!((rows[1].preview.as_str(), rows[1].unread), ("привет", 3));
    }

    #[test]
    fn server_error_text_is_shown() {
        let payload = rmpv::Value::Map(vec![("localizedMessage".into(), "Неверный код".into())]);
        let e = CoreError::Transport(TransportError::Server {
            opcode: 18,
            payload,
        });
        assert_eq!(message(&e), "Неверный код");
    }
}
