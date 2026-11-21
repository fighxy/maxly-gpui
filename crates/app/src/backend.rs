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
    /// Dialog titles learned after the chat list was shown: (chat id, title).
    ChatTitles(Vec<ChatTitle>),
    /// The account's active sessions.
    Sessions(Vec<SessionRow>),
    /// The sessions could not be loaded or closed.
    SessionsFailed(String),
    /// Logged out: the token is gone and the login screens follow.
    LoggedOut,
    /// A message already shown was edited.
    MessageEdited { chat_id: i64, message: MessageRow },
    /// Messages were deleted.
    MessagesDeleted { chat_id: i64, ids: Vec<i64> },
    /// Messages between two times (Unix ms, inclusive) were deleted.
    MessagesDeletedRange { chat_id: i64, from: i64, to: i64 },
    /// Someone is typing; `who` is their name, empty when unknown.
    Typing { chat_id: i64, who: String },
    /// We read the chat on another device; `unread` is what is left.
    ReadElsewhere { chat_id: i64, unread: i32 },
    /// Another participant read the chat up to `mark` (Unix ms).
    PeerRead { chat_id: i64, mark: i64 },
    /// A chat changed or was added.
    ChatChanged(ChatRow),
    /// The people we know, for the contacts section.
    Contacts(Vec<ContactRow>),
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
    /// Send text to a chat; `cid` identifies the pending copy the UI already shows, `reply_to`
    /// the message it answers.
    SendText {
        chat_id: i64,
        text: String,
        cid: i64,
        reply_to: Option<i64>,
    },
    /// Replace the text of our message.
    EditText {
        chat_id: i64,
        message_id: i64,
        text: String,
    },
    /// Delete messages, for everyone or only for us.
    Delete {
        chat_id: i64,
        ids: Vec<i64>,
        for_me: bool,
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
    /// Load the account's active sessions.
    LoadSessions,
    /// End every session but this one.
    CloseOtherSessions,
    /// End this session and forget the token.
    Logout,
    /// We are typing in a chat.
    Typing { chat_id: i64 },
}

/// One active session of the account, for the account panel.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    /// Client name and version.
    pub client: String,
    /// Device or OS.
    pub info: String,
    /// Where the server places it.
    pub location: String,
    /// Last activity, Unix ms.
    pub time: i64,
    /// This client.
    pub current: bool,
}

impl SessionRow {
    fn from_info(s: &max_core::account::SessionInfo) -> SessionRow {
        SessionRow {
            client: s.client.clone(),
            info: s.info.clone(),
            location: s.location.clone(),
            time: s.time,
            current: s.current.unwrap_or(false),
        }
    }
}

/// Sessions with this one first, then the most recently active.
fn session_rows(list: &[max_core::account::SessionInfo]) -> Vec<SessionRow> {
    let mut rows: Vec<SessionRow> = list.iter().map(SessionRow::from_info).collect();
    rows.sort_by_key(|r| (std::cmp::Reverse(r.current), std::cmp::Reverse(r.time)));
    rows
}

/// Delivery state of a message in the history view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Delivery {
    /// On the server.
    #[default]
    Sent,
    /// Sent by us, no answer yet.
    Pending,
    /// The server refused it or the connection failed.
    Failed,
}

/// The message a reply answers, as shown above the reply's text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Quote {
    /// Server id of the quoted message.
    pub id: i64,
    /// Its sender's name (empty for channel posts).
    pub sender: String,
    /// Its text on one line, or what is attached.
    pub text: String,
}

/// One message as the history view shows it.
#[derive(Debug, Clone, PartialEq, Default)]
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
    /// Edited after sending.
    pub edited: bool,
    /// The message this one answers.
    pub reply: Option<Quote>,
    /// Forwarded: the original author or channel; text and attachments are the original's.
    pub forwarded: Option<String>,
}

impl MessageRow {
    /// One line for the chat list: the text, or what is attached.
    pub fn preview(&self) -> String {
        if !self.text.is_empty() {
            return one_line(&self.text);
        }
        let media = self
            .attachments
            .iter()
            .filter(|a| matches!(a, Attachment::Photo { .. } | Attachment::Video { .. }))
            .count();
        if media > 1 {
            return "Альбом".to_string();
        }
        self.attachments
            .first()
            .map(|a| match a {
                Attachment::Photo { .. } => "Фото".to_string(),
                Attachment::Video { .. } => "Видео".to_string(),
                Attachment::File { name, .. } => format!("Файл: {name}"),
                Attachment::Sticker { .. } => "Стикер".to_string(),
                Attachment::Other { kind } => attachment_label(kind).to_string(),
            })
            .unwrap_or_default()
    }
}

/// One person in the contacts section.
#[derive(Debug, Clone, PartialEq)]
pub struct ContactRow {
    /// User id.
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Our dialog with them, if one is in the chat list.
    pub chat_id: Option<i64>,
    /// Profile photo address.
    pub avatar: Option<String>,
}

/// A dialog's title and photo, learned after the chat list was shown.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatTitle {
    /// The dialog.
    pub chat_id: i64,
    /// The other person's name.
    pub title: String,
    /// Their profile photo address.
    pub avatar: Option<String>,
}

/// What kind of chat a row is (`Chat.type`: `DIALOG`, `CHAT`, `CHANNEL` in `core.dll`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatKind {
    /// A dialog with one person.
    #[default]
    Dialog,
    /// Saved messages: a dialog with ourselves.
    Saved,
    /// A group chat.
    Group,
    /// A channel.
    Channel,
}

/// "1 234 567": thousands split by a narrow no-break space, as Russian text writes them.
pub fn group_digits(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push('\u{202F}');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// The Russian noun form for `n`: `one` (1, 21), `few` (2–4, 22–24) or `many` (the rest,
/// 11–14).
pub fn plural<'a>(n: i64, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let n = n.unsigned_abs();
    match (n % 10, n % 100) {
        (_, 11..=14) => many,
        (1, _) => one,
        (2..=4, _) => few,
        _ => many,
    }
}

impl ChatRow {
    /// The line under the chat title: subscribers of a channel, members of a group.
    pub fn members_line(&self) -> Option<String> {
        let n = self.members?;
        Some(match self.kind {
            ChatKind::Channel => format!(
                "{} {}",
                group_digits(n),
                plural(n, "подписчик", "подписчика", "подписчиков")
            ),
            ChatKind::Group => format!(
                "{} {}",
                group_digits(n),
                plural(n, "участник", "участника", "участников")
            ),
            ChatKind::Dialog | ChatKind::Saved => return None,
        })
    }
}

/// One row of the chat list.
#[derive(Debug, Clone, PartialEq, Default)]
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
    /// The latest read mark of the other participants, Unix ms: our messages up to it are read.
    pub peer_mark: i64,
    /// Chat icon, or the other person's photo for a dialog.
    pub avatar: Option<String>,
    /// Dialog, saved messages, group or channel.
    pub kind: ChatKind,
    /// Members of a group, subscribers of a channel (`participantsCount`), when known.
    pub members: Option<i64>,
    /// The chat's description (empty if none).
    pub description: String,
    /// The chat's public link (empty if none).
    pub link: String,
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

/// Server error for a new device logging into an account with an active session and no
/// 2FA password (seen live on `AUTH` CHECK_CODE).
const NO_2FA: &str = "error.profile.active.session.no2fa";

/// A readable message for a failed step: the server's own text when it sent one.
fn message(e: &CoreError) -> String {
    if let CoreError::Transport(TransportError::Server { payload, .. }) = e
        && let Some(map) = payload.as_map()
    {
        let error = map
            .iter()
            .find(|(k, _)| k.as_str() == Some("error"))
            .and_then(|(_, v)| v.as_str());
        if error == Some(NO_2FA) {
            return "Вход по коду закрыт: у аккаунта есть активный сеанс, а облачный пароль (2FA)                     не задан. Задайте пароль в приложении MAX на телефоне или войдите по QR-коду."
                .into();
        }
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
            UiCommand::EditText { .. } | UiCommand::Delete { .. } => {}
            UiCommand::LoadOlder { .. } | UiCommand::MarkRead { .. } => {}
            UiCommand::LoadSessions | UiCommand::CloseOtherSessions | UiCommand::Logout => {}
            UiCommand::Typing { .. } => {}
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
            peer_mark: t - 3 * 60_000,
            // Drawn locally: the first two chats show a photo, the rest initials.
            avatar: (i < 2).then(|| format!("demo://avatar/{i}")),
            kind: match i {
                1 => ChatKind::Group,
                2 => ChatKind::Saved,
                3 => ChatKind::Channel,
                _ => ChatKind::Dialog,
            },
            members: match i {
                1 => Some(5),
                3 => Some(12_345),
                _ => None,
            },
            description: match i {
                1 => "Сборки, задачи и созвоны по проекту".into(),
                3 => "Новости, объявления и отключения в нашем районе".into(),
                _ => String::new(),
            },
            link: if i == 3 {
                "https://max.ru/demo_rayon".into()
            } else {
                String::new()
            },
        })
        .collect();
    let _ = ui.send(UiEvent::LoggedIn {
        me: "Демо-режим".into(),
        chats,
    });
    let contacts = [
        ("Анна Смирнова", Some(1)),
        ("Иван Петров", None),
        ("Мария Ковалёва", None),
        ("Пётр", Some(5)),
    ];
    let _ = ui.send(UiEvent::Contacts(
        contacts
            .iter()
            .enumerate()
            .map(|(i, (name, chat_id))| ContactRow {
                id: 100 + i as i64,
                name: (*name).into(),
                chat_id: *chat_id,
                avatar: (i == 0).then(|| "demo://avatar/0".to_string()),
            })
            .collect(),
    ));
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
            UiCommand::EditText {
                chat_id,
                message_id,
                text,
            } => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let mut row = demo_row(message_id, true, &text, now_ms(), Delivery::Sent);
                row.cid = None;
                row.edited = true;
                // The demo does not keep times: the UI keeps the original one.
                row.time = 0;
                let _ = ui.send(UiEvent::MessageEdited {
                    chat_id,
                    message: row,
                });
            }
            UiCommand::Delete { chat_id, ids, .. } => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let _ = ui.send(UiEvent::MessagesDeleted { chat_id, ids });
            }
            UiCommand::SendText {
                chat_id, text, cid, ..
            } => {
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
                    tokio::time::sleep(Duration::from_millis(600)).await;
                    let _ = ui.send(UiEvent::PeerRead {
                        chat_id,
                        mark: now_ms(),
                    });
                    let _ = ui.send(UiEvent::Typing {
                        chat_id,
                        who: String::new(),
                    });
                    tokio::time::sleep(Duration::from_millis(2500)).await;
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
                } else if let Some((w, h)) = url
                    .strip_prefix("demo://photo/")
                    .and_then(|s| s.split_once('x'))
                    .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
                {
                    Some(demo_photo_sized(w, h))
                } else if let Some(n) = url.strip_prefix("demo://avatar/") {
                    Some(demo_avatar_svg(n))
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
            UiCommand::LoadSessions => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let _ = ui.send(UiEvent::Sessions(demo_sessions(now_ms(), false)));
            }
            UiCommand::CloseOtherSessions => {
                tokio::time::sleep(Duration::from_millis(300)).await;
                let _ = ui.send(UiEvent::Sessions(demo_sessions(now_ms(), true)));
                let _ = ui.send(UiEvent::Notice("Другие сеансы завершены".into()));
            }
            UiCommand::Logout => {
                let _ = ui.send(UiEvent::LoggedOut);
                let _ = ui.send(UiEvent::Qr(
                    "https://max.ru/:auth/demo-only-not-a-real-login".into(),
                ));
            }
            _ => {}
        }
    }
}

/// Made-up sessions: this one, and two others unless they were closed.
fn demo_sessions(now: i64, only_current: bool) -> Vec<SessionRow> {
    let row = |client: &str, info: &str, location: &str, ago: i64, current: bool| SessionRow {
        client: client.into(),
        info: info.into(),
        location: location.into(),
        time: now - ago,
        current,
    };
    let mut rows = vec![row("MAX Desktop 26.41", "Windows", "Россия", 0, true)];
    if !only_current {
        rows.push(row(
            "MAX Android 25.20",
            "Pixel 7",
            "Россия",
            3_600_000,
            false,
        ));
        rows.push(row(
            "MAX Web",
            "Firefox",
            "Казахстан",
            5 * 86_400_000,
            false,
        ));
    }
    rows
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
        ..MessageRow::default()
    }
}

/// A made-up profile photo for the demo: a head and shoulders on a coloured square.
fn demo_avatar_svg(n: &str) -> bytes::Bytes {
    let (top, bottom) = if n == "0" {
        ("#f6a5c0", "#c2185b")
    } else {
        ("#9fd3ff", "#1565c0")
    };
    bytes::Bytes::from(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="160" height="160" viewBox="0 0 160 160">
<defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{top}"/><stop offset="1" stop-color="{bottom}"/></linearGradient></defs>
<rect width="160" height="160" fill="url(#g)"/>
<circle cx="80" cy="64" r="30" fill="#ffffff" fill-opacity="0.9"/>
<path d="M24 160 C30 112 130 112 136 160 Z" fill="#ffffff" fill-opacity="0.9"/>
</svg>"##
    ))
}

/// A made-up landscape of any size for demo albums: sky, sun and hills scaled to `w`×`h`.
fn demo_photo_sized(w: u32, h: u32) -> bytes::Bytes {
    let (wf, hf) = (f64::from(w), f64::from(h));
    let sun = (wf.min(hf) * 0.08).max(8.);
    let hill = |y: f64| hf * y;
    bytes::Bytes::from(format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#7cb7ff"/><stop offset="1" stop-color="#d9ecff"/></linearGradient></defs>
<rect width="{w}" height="{h}" fill="url(#g)"/>
<circle cx="{sx}" cy="{sy}" r="{sun}" fill="#ffd54f"/>
<path d="M0 {a} L{b} {c} L{d} {e} L{f} {g} L{w} {i} L{w} {h} L0 {h} Z" fill="#4e8a5a"/>
<path d="M0 {j} L{k} {l} L{m} {n} L{w} {o} L{w} {h} L0 {h} Z" fill="#2f6b3d"/>
</svg>"##,
        sx = wf * 0.8,
        sy = hf * 0.2,
        a = hill(0.78),
        b = wf * 0.25,
        c = hill(0.5),
        d = wf * 0.45,
        e = hill(0.7),
        f = wf * 0.62,
        g = hill(0.42),
        i = hill(0.8),
        j = hill(0.86),
        k = wf * 0.3,
        l = hill(0.67),
        m = wf * 0.7,
        n = hill(0.83),
        o = hill(0.62),
    ))
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
        (false, "Электрички в субботу: 9:12, 9:47, 10:20", 12),
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
            if i == 2 {
                row.reply = Some(Quote {
                    id: chat_id * 100 + 1,
                    sender: "Вы".into(),
                    text: "Привет, да. Во сколько удобно?".into(),
                });
            }
            if i == 5 {
                row.forwarded = Some("Пригородные поезда".into());
            }
            let photo = |w: i32, h: i32| Attachment::Photo {
                photo_id: None,
                url: format!("demo://photo/{w}x{h}"),
                width: w,
                height: h,
            };
            if i == 4 {
                // An album: wide, tall and square photos and a video.
                row.attachments = vec![
                    photo(640, 420),
                    photo(420, 640),
                    photo(600, 600),
                    Attachment::Video {
                        video_id: 1,
                        thumbnail: Some("demo://photo/1280x720".into()),
                        width: 1280,
                        height: 720,
                        duration: 42_000,
                    },
                ];
            }
            if i == 1 {
                row.attachments = vec![photo(420, 640), photo(640, 420), photo(640, 420)];
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
    let mut config = CoreConfig::default();
    config.transport.trace = crate::trace::transport_trace();
    // Each pass is one logged-in run; logout starts the next pass at the login screens.
    loop {
        let profile =
            Profile::open(&root, "default", secrets.clone()).map_err(|e| e.to_string())?;
        if !run_logged_in(profile, &config, &ui, &mut commands).await? {
            return Ok(());
        }
    }
}

/// Log in if needed, then serve the UI until it closes (`false`) or the user logs out (`true`).
async fn run_logged_in(
    mut profile: Profile,
    config: &CoreConfig,
    ui: &UnboundedSender<UiEvent>,
    commands: &mut UnboundedReceiver<UiCommand>,
) -> Result<bool, String> {
    let env = Environment::detect();
    let config = config.clone();

    if profile.auth_token().map_err(|e| e.to_string())?.is_none() {
        let _ = ui.send(UiEvent::Status("Подключение…".into()));
        let session = Session::connect(&config, &mut profile, &env)
            .await
            .map_err(|e| e.to_string())?;
        let token = login(&session, ui, commands, &env.system_language).await?;
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
                        let peers = dialog_peers(&login);
                        let (handle, names, ui) = (handle.clone(), names.clone(), ui.clone());
                        tokio::spawn(async move {
                            if let Ok(t) = handle.transport() {
                                resolve_names(&t, &names, peers.iter().map(|p| p.1)).await;
                                let (titles, contacts) = {
                                    let names = names.read().unwrap_or_else(PoisonError::into_inner);
                                    (names.titles(&peers), names.contact_rows(&peers))
                                };
                                if !titles.is_empty() {
                                    let _ = ui.send(UiEvent::ChatTitles(titles));
                                }
                                let _ = ui.send(UiEvent::Contacts(contacts));
                            }
                        });
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
                    CoreEvent::Push(push) => {
                        let Some(event) = max_core::events::decode(&push) else { continue };
                        let names = names.read().unwrap_or_else(PoisonError::into_inner);
                        match names.ui_event(event) {
                            Some(msg) => msg,
                            None => continue,
                        }
                    }
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
                            let result = async {
                                let t = handle.transport()?;
                                let list = messages::history(&t, chat_id, now_ms(), messages::HISTORY_PAGE).await?;
                                resolve_names(&t, &names, list.iter().flat_map(sender_ids)).await;
                                Ok::<_, CoreError>(list)
                            }
                            .await;
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
                            let result = async {
                                let t = handle.transport()?;
                                let list = messages::history(&t, chat_id, before, messages::HISTORY_PAGE).await?;
                                resolve_names(&t, &names, list.iter().flat_map(sender_ids)).await;
                                Ok::<_, CoreError>(list)
                            }
                            .await;
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
                    UiCommand::SendText { chat_id, text, cid, reply_to } => {
                        tokio::spawn(async move {
                            let result = match handle.transport() {
                                Ok(t) => messages::send_text(&t, chat_id, &text, cid, reply_to).await,
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
                    UiCommand::EditText { chat_id, message_id, text } => {
                        tokio::spawn(async move {
                            let result = match handle.transport() {
                                Ok(t) => messages::edit_text(&t, chat_id, message_id, &text).await,
                                Err(e) => Err(e),
                            };
                            let _ = ui.send(match result {
                                Ok(stored) => {
                                    let names = names.read().unwrap_or_else(PoisonError::into_inner);
                                    let mut row = names.row(&stored);
                                    // The answer may leave out the status of a fresh edit.
                                    row.edited = true;
                                    UiEvent::MessageEdited { chat_id, message: row }
                                }
                                Err(e) => UiEvent::Notice(format!("Не удалось изменить сообщение: {}", message(&e))),
                            });
                        });
                    }
                    UiCommand::Delete { chat_id, ids, for_me } => {
                        tokio::spawn(async move {
                            let result = match handle.transport() {
                                Ok(t) => messages::delete(&t, chat_id, &ids, for_me).await,
                                Err(e) => Err(e),
                            };
                            match result {
                                Ok(done) => {
                                    if done.len() < ids.len() {
                                        let _ = ui.send(UiEvent::Notice("Не все сообщения удалось удалить".into()));
                                    }
                                    let _ = ui.send(UiEvent::MessagesDeleted { chat_id, ids: done });
                                }
                                Err(e) => {
                                    let _ = ui.send(UiEvent::Notice(format!("Не удалось удалить: {}", message(&e))));
                                }
                            }
                        });
                    }
                    UiCommand::LoadSessions => {
                        tokio::spawn(async move {
                            let result = match handle.transport() {
                                Ok(t) => max_core::account::sessions(&t).await,
                                Err(e) => Err(e),
                            };
                            let _ = ui.send(match result {
                                Ok(list) => UiEvent::Sessions(session_rows(&list)),
                                Err(e) => UiEvent::SessionsFailed(message(&e)),
                            });
                        });
                    }
                    UiCommand::CloseOtherSessions => {
                        tokio::spawn(async move {
                            let result = async {
                                handle.close_other_sessions().await?;
                                max_core::account::sessions(&handle.transport()?).await
                            }
                            .await;
                            match result {
                                Ok(list) => {
                                    let _ = ui.send(UiEvent::Sessions(session_rows(&list)));
                                    let _ = ui.send(UiEvent::Notice("Другие сеансы завершены".into()));
                                }
                                Err(e) => {
                                    let _ = ui.send(UiEvent::SessionsFailed(message(&e)));
                                }
                            }
                        });
                    }
                    UiCommand::Typing { chat_id } => {
                        tokio::spawn(async move {
                            if let Ok(t) = handle.transport()
                                && let Err(e) = max_core::events::send_typing(&t, chat_id, max_core::events::TYPING_TEXT).await
                            {
                                crate::trace::note(&format!("typing failed: {e}"));
                            }
                        });
                    }
                    UiCommand::Logout => {
                        if let Err(e) = handle.logout().await {
                            let _ = ui.send(UiEvent::SessionsFailed(format!("Не удалось выйти: {}", message(&e))));
                            continue;
                        }
                        let _ = ui.send(UiEvent::LoggedOut);
                        return Ok(true);
                    }
                    _ => {}
                }
            }
        }
    }
    handle.stop();
    Ok(false)
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
pub fn save_download(name: &str, bytes: &[u8]) -> std::io::Result<std::path::PathBuf> {
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
    /// Profile photo addresses (`baseUrl`) of the people above that have one.
    avatars: BTreeMap<i64, String>,
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
        let avatars = login
            .contacts
            .iter()
            .chain(std::iter::once(&login.profile.contact))
            .filter_map(|c| contact_avatar(c).map(|url| (c.id, url)))
            .collect();
        Names {
            me: login.profile.contact.id,
            contacts,
            avatars,
        }
    }

    /// Ids among `ids` with no known name, each once.
    pub fn missing(&self, ids: impl IntoIterator<Item = i64>) -> Vec<i64> {
        let mut out: Vec<i64> = ids
            .into_iter()
            .filter(|id| *id != 0 && !self.contacts.contains_key(id))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Learn names from `CONTACT_INFO`.
    pub fn add(&mut self, contacts: &[Contact]) {
        for c in contacts {
            self.contacts.insert(c.id, contact_name(c));
            if let Some(url) = contact_avatar(c) {
                self.avatars.insert(c.id, url);
            }
        }
    }

    /// Everyone we know a name for except ourselves, by name; `peers` (chat id, peer id) gives
    /// the dialog to open for a contact.
    pub fn contact_rows(&self, peers: &[(i64, i64)]) -> Vec<ContactRow> {
        let mut rows: Vec<ContactRow> = self
            .contacts
            .iter()
            .filter(|(id, _)| **id != self.me)
            .map(|(id, name)| ContactRow {
                id: *id,
                name: name.clone(),
                chat_id: peers.iter().find(|(_, p)| p == id).map(|(c, _)| *c),
                avatar: self.avatars.get(id).cloned(),
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        rows
    }

    /// Known titles and photos for dialogs given as (chat id, peer id).
    pub fn titles(&self, peers: &[(i64, i64)]) -> Vec<ChatTitle> {
        peers
            .iter()
            .filter_map(|(chat, peer)| {
                self.contacts.get(peer).map(|n| ChatTitle {
                    chat_id: *chat,
                    title: n.clone(),
                    avatar: self.avatars.get(peer).cloned(),
                })
            })
            .collect()
    }

    /// What a server event means for the UI; `None` for our own typing and marks we ignore.
    pub fn ui_event(&self, event: max_core::events::ServerEvent) -> Option<UiEvent> {
        use max_core::events::ServerEvent as E;
        Some(match event {
            E::Message(new) => UiEvent::MessageAdded {
                chat_id: new.chat_id,
                message: self.row(&new.message),
            },
            E::Edited { chat_id, message } => UiEvent::MessageEdited {
                chat_id,
                message: self.row(&message),
            },
            E::Deleted {
                chat_id,
                message_ids,
            } => UiEvent::MessagesDeleted {
                chat_id,
                ids: message_ids,
            },
            E::DeletedRange { chat_id, from, to } => {
                UiEvent::MessagesDeletedRange { chat_id, from, to }
            }
            E::Typing {
                chat_id, user_id, ..
            } if user_id != self.me => UiEvent::Typing {
                chat_id,
                who: self.contacts.get(&user_id).cloned().unwrap_or_default(),
            },
            E::Typing { .. } => return None,
            E::Mark {
                chat_id,
                user_id,
                unread,
                ..
            } if user_id == self.me => UiEvent::ReadElsewhere {
                chat_id,
                unread: unread.unwrap_or(0),
            },
            E::Mark { chat_id, mark, .. } => UiEvent::PeerRead { chat_id, mark },
            E::Chat(chat) => UiEvent::ChatChanged(self.chat_row(&chat)),
        })
    }

    /// A chat list row, with names known so far.
    pub fn chat_row(&self, chat: &Chat) -> ChatRow {
        let peer = |id: i64| {
            (
                self.contacts.get(&id).cloned(),
                self.avatars.get(&id).cloned(),
            )
        };
        chat_row_with(chat, self.me, peer)
    }

    /// A history row for a message. Channel posts have no sender: not ours, no name. A
    /// forwarded message shows the original's text and attachments; a reply quotes the message
    /// it answers.
    pub fn row(&self, m: &messages::Message) -> MessageRow {
        let link = m.link.as_deref();
        let forward = link.filter(|l| l.type_ == messages::LINK_FORWARD);
        let reply = link.filter(|l| l.type_ == messages::LINK_REPLY);
        let (text, attachments) = match forward {
            Some(l) => {
                let texts = [bubble_text(m), bubble_text(&l.message)];
                let text = texts
                    .into_iter()
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let mut attachments = media::attachments(m);
                attachments.extend(media::attachments(&l.message));
                (text, attachments)
            }
            None => (bubble_text(m), media::attachments(m)),
        };
        MessageRow {
            id: m.id,
            cid: m.cid,
            out: m.sender == Some(self.me),
            sender: self.name(m.sender),
            text,
            time: m.time,
            delivery: Delivery::Sent,
            attachments,
            edited: m.status.as_deref() == Some(max_core::events::EDITED),
            reply: reply.map(|l| Quote {
                id: l.message.id,
                sender: self.name(l.message.sender),
                text: one_line(&message_text(&l.message)),
            }),
            forwarded: forward.map(|l| {
                l.chat_name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| self.name(l.message.sender))
            }),
        }
    }

    /// A sender's name: empty for none (channel posts), the id while it is unknown.
    fn name(&self, id: Option<i64>) -> String {
        id.map_or_else(String::new, |id| {
            self.contacts
                .get(&id)
                .cloned()
                .unwrap_or_else(|| format!("id {id}"))
        })
    }
}

/// Everyone a message row names: the sender and the author of a quoted or forwarded message.
pub fn sender_ids(m: &messages::Message) -> impl Iterator<Item = i64> + '_ {
    m.sender
        .into_iter()
        .chain(m.link.as_ref().and_then(|l| l.message.sender))
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

/// The text of a message, or what its attachments are; a forwarded message without its own
/// text gives the original's.
pub fn message_text(m: &messages::Message) -> String {
    let text = m.text.clone().unwrap_or_default();
    if text.is_empty()
        && m.attaches.as_ref().is_none_or(Vec::is_empty)
        && let Some(l) = m.link.as_deref()
        && l.type_ == messages::LINK_FORWARD
    {
        return message_text(&l.message);
    }
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

/// Fetch names for ids we do not know yet; a failure leaves them as ids.
async fn resolve_names(
    t: &max_transport::Transport,
    names: &RwLock<Names>,
    ids: impl IntoIterator<Item = i64>,
) {
    let missing = names
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .missing(ids);
    if missing.is_empty() {
        return;
    }
    match max_core::contacts::contact_info(t, &missing).await {
        Ok(found) => names
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .add(&found),
        Err(e) => crate::trace::note(&format!("contact info failed: {e}")),
    }
}

/// Dialogs without a title, as (chat id, the other participant).
pub fn dialog_peers(login: &LoginResponse) -> Vec<(i64, i64)> {
    let me = login.profile.contact.id;
    login
        .chats
        .iter()
        .filter(|c| c.title.as_deref().is_none_or(str::is_empty))
        .filter_map(|c| {
            c.participants
                .keys()
                .find(|id| **id != me)
                .map(|p| (c.id, *p))
        })
        .collect()
}

/// Text on one line, for chat list previews.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Chat list rows from the initial sync, newest first.
pub fn rows_from_login(login: &LoginResponse) -> (String, Vec<ChatRow>) {
    let me_id = login.profile.contact.id;
    let contacts: BTreeMap<i64, &Contact> = login.contacts.iter().map(|c| (c.id, c)).collect();
    let mut rows: Vec<ChatRow> = login
        .chats
        .iter()
        .map(|c| {
            chat_row_with(c, me_id, |id| {
                let contact = contacts.get(&id);
                (
                    contact.map(|c| contact_name(c)),
                    contact.and_then(|c| contact_avatar(c)),
                )
            })
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.time));
    (contact_name(&login.profile.contact), rows)
}

/// A chat list row; `peer` gives the name and photo of a dialog's other participant.
fn chat_row_with(
    chat: &Chat,
    me_id: i64,
    peer: impl Fn(i64) -> (Option<String>, Option<String>),
) -> ChatRow {
    let titled = chat.title.clone().filter(|t| !t.is_empty());
    let other = chat
        .participants
        .keys()
        .find(|id| **id != me_id)
        .copied()
        .unwrap_or(me_id);
    // Groups and channels have their own icon; a dialog shows the other person's photo.
    let (name, peer_avatar) = if titled.is_none() && other != me_id {
        peer(other)
    } else {
        (None, None)
    };
    let avatar = chat
        .base_icon_url
        .clone()
        .filter(|u| !u.is_empty())
        .or(peer_avatar);
    let title = titled.unwrap_or_else(|| {
        if other == me_id {
            "Избранное".to_string()
        } else {
            name.unwrap_or_else(|| format!("id {other}"))
        }
    });
    let preview = chat
        .last_message
        .as_ref()
        .map(|m| one_line(&message_text(m)))
        .unwrap_or_default();
    let kind = match chat.type_.as_str() {
        "CHANNEL" => ChatKind::Channel,
        "CHAT" => ChatKind::Group,
        _ if other == me_id => ChatKind::Saved,
        "DIALOG" => ChatKind::Dialog,
        // An unknown type: a title means a group, none a dialog.
        _ if chat.title.as_deref().is_some_and(|t| !t.is_empty()) => ChatKind::Group,
        _ => ChatKind::Dialog,
    };
    let members = match kind {
        // A channel's `participants` lists only some members: trust the count alone.
        ChatKind::Channel => chat.participants_count,
        ChatKind::Group => chat.participants_count.or_else(|| {
            let n = i64::try_from(chat.participants.len()).unwrap_or(0);
            (n > 0).then_some(n)
        }),
        ChatKind::Dialog | ChatKind::Saved => None,
    };
    ChatRow {
        id: chat.id,
        title,
        preview,
        unread: chat.new_messages.unwrap_or(0),
        time: chat.last_event_time,
        peer_mark: peer_mark(chat, me_id),
        avatar,
        kind,
        members,
        description: chat.description.clone().unwrap_or_default(),
        link: chat.link.clone().unwrap_or_default(),
    }
}

/// A contact's profile photo address, if it has one.
fn contact_avatar(c: &Contact) -> Option<String> {
    c.base_url.clone().filter(|u| !u.is_empty())
}

/// The newest read mark among the other participants (`participants` maps id to read time).
fn peer_mark(chat: &Chat, me_id: i64) -> i64 {
    chat.participants
        .iter()
        .filter(|(id, _)| **id != me_id)
        .map(|(_, mark)| *mark)
        .max()
        .unwrap_or(0)
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
    fn russian_plurals_and_digit_groups() {
        let forms = |n| plural(n, "участник", "участника", "участников");
        let got: Vec<&str> = [1, 2, 4, 5, 11, 12, 14, 21, 22, 25, 101, 111]
            .into_iter()
            .map(forms)
            .collect();
        assert_eq!(
            got,
            [
                "участник",
                "участника",
                "участника",
                "участников",
                "участников",
                "участников",
                "участников",
                "участник",
                "участника",
                "участников",
                "участник",
                "участников"
            ]
        );
        assert_eq!(group_digits(7), "7");
        assert_eq!(group_digits(12_345), "12\u{202F}345");
        assert_eq!(group_digits(1_234_567), "1\u{202F}234\u{202F}567");
    }

    #[test]
    fn chat_kinds_and_member_counts() {
        let names = Names::from_login(&login_with(vec![]));
        let row = |type_: &str, title: Option<&str>, count: Option<i64>, members: &[i64]| {
            names.chat_row(&Chat {
                id: 1,
                type_: type_.into(),
                title: title.map(Into::into),
                participants_count: count,
                participants: members.iter().map(|id| (*id, 0)).collect(),
                ..Chat::default()
            })
        };
        let channel = row("CHANNEL", Some("РБК"), Some(1_200_000), &[1]);
        assert_eq!(channel.kind, ChatKind::Channel);
        assert_eq!(
            channel.members_line().as_deref(),
            Some("1\u{202F}200\u{202F}000 подписчиков")
        );
        let no_count = row("CHANNEL", Some("Канал"), None, &[1, 2]);
        assert_eq!(
            no_count.members_line(),
            None,
            "a channel's member map is partial"
        );
        let group = row("CHAT", Some("Семья"), None, &[1, 2, 3]);
        assert_eq!(group.members_line().as_deref(), Some("3 участника"));
        let dialog = row("DIALOG", None, None, &[1, 2]);
        assert_eq!(
            (dialog.kind, dialog.members_line()),
            (ChatKind::Dialog, None)
        );
        assert_eq!(row("DIALOG", None, None, &[1]).kind, ChatKind::Saved);
    }

    #[test]
    fn avatars_are_the_chat_icon_or_the_peer_photo() {
        let with_photo = |id, first, url: &str| Contact {
            base_url: Some(url.into()),
            ..contact(id, first)
        };
        let dialog = Chat {
            id: 10,
            participants: [(1, 0), (2, 0)].into_iter().collect(),
            ..Chat::default()
        };
        let channel = Chat {
            id: 11,
            title: Some("Канал".into()),
            base_icon_url: Some("https://i.oneme.ru/c".into()),
            participants: [(1, 0), (2, 0)].into_iter().collect(),
            ..Chat::default()
        };
        let saved = Chat {
            id: 12,
            participants: [(1, 0)].into_iter().collect(),
            ..Chat::default()
        };
        let login = LoginResponse {
            profile: ProfileT {
                contact: with_photo(1, "Я", "https://i.oneme.ru/me"),
                ..ProfileT::default()
            },
            contacts: vec![with_photo(2, "Анна", "https://i.oneme.ru/a")],
            chats: vec![dialog.clone(), channel, saved],
            ..LoginResponse::default()
        };
        let (_, rows) = rows_from_login(&login);
        let avatar = |id| {
            rows.iter()
                .find(|r| r.id == id)
                .and_then(|r| r.avatar.as_deref())
        };
        assert_eq!(avatar(10), Some("https://i.oneme.ru/a"));
        assert_eq!(
            avatar(11),
            Some("https://i.oneme.ru/c"),
            "not the member's photo"
        );
        assert_eq!(avatar(12), None, "Saved messages has no photo");

        // A peer learned later through CONTACT_INFO brings the photo with the title.
        let mut names = Names::from_login(&login_with(vec![]));
        assert_eq!(names.chat_row(&dialog).avatar, None);
        names.add(&[with_photo(2, "Анна", "https://i.oneme.ru/a")]);
        assert_eq!(
            names.titles(&[(10, 2)]),
            [ChatTitle {
                chat_id: 10,
                title: "Анна".into(),
                avatar: Some("https://i.oneme.ru/a".into())
            }]
        );
        assert_eq!(
            names.contact_rows(&[(10, 2)])[0].avatar.as_deref(),
            Some("https://i.oneme.ru/a")
        );
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

    fn login_with(chats: Vec<Chat>) -> LoginResponse {
        LoginResponse {
            profile: ProfileT {
                contact: contact(1, "Я"),
                ..ProfileT::default()
            },
            chats,
            ..LoginResponse::default()
        }
    }

    #[test]
    fn replies_quote_and_forwards_show_the_original() {
        use max_proto::schema::types::MessageLink;
        let mut names = Names::from_login(&login_with(vec![]));
        names.add(&[contact(7, "Анна")]);
        let inner = |sender: Option<i64>, text: &str| Message {
            id: 5,
            sender,
            text: Some(text.into()),
            ..Message::default()
        };
        let linked = |kind: &str, link_msg: Message, chat_name: Option<&str>| Message {
            id: 6,
            sender: Some(1),
            text: Some(if kind == "REPLY" {
                "ответ".into()
            } else {
                String::new()
            }),
            link: Some(Box::new(MessageLink {
                type_: kind.into(),
                chat_name: chat_name.map(Into::into),
                message: Box::new(link_msg),
                ..MessageLink::default()
            })),
            ..Message::default()
        };
        let reply = names.row(&linked(
            "REPLY",
            inner(Some(7), "вопрос\nв две строки"),
            None,
        ));
        assert_eq!(reply.text, "ответ");
        assert_eq!(
            reply.reply,
            Some(Quote {
                id: 5,
                sender: "Анна".into(),
                text: "вопрос в две строки".into()
            })
        );
        assert!(reply.forwarded.is_none());

        let fwd = linked("FORWARD", inner(Some(7), "новость"), None);
        let row = names.row(&fwd);
        assert_eq!(
            (row.text.as_str(), row.forwarded.as_deref()),
            ("новость", Some("Анна"))
        );
        assert_eq!(message_text(&fwd), "новость", "chat list preview");
        let from_channel = names.row(&linked("FORWARD", inner(None, "пост"), Some("Канал")));
        assert_eq!(from_channel.forwarded.as_deref(), Some("Канал"));
        let ids: Vec<i64> = sender_ids(&fwd).collect();
        assert_eq!(ids, [1, 7]);
    }

    #[test]
    fn contacts_are_sorted_and_find_their_dialog() {
        let mut names = Names::from_login(&login_with(vec![]));
        names.add(&[contact(7, "борис"), contact(8, "Анна")]);
        let rows = names.contact_rows(&[(40, 7)]);
        let got: Vec<(&str, Option<i64>)> =
            rows.iter().map(|r| (r.name.as_str(), r.chat_id)).collect();
        assert_eq!(got, [("Анна", None), ("борис", Some(40))], "us left out");
    }

    #[test]
    fn channel_posts_are_not_ours_and_have_no_sender() {
        let names = Names::from_login(&login_with(vec![]));
        let post = Message {
            text: Some("новость".into()),
            ..Message::default()
        };
        let row = names.row(&post);
        assert!(!row.out);
        assert_eq!(row.sender, "");
        let mine = Message {
            sender: Some(1),
            ..Message::default()
        };
        assert!(names.row(&mine).out);
    }

    #[test]
    fn unknown_dialog_peers_get_names_later() {
        let dialog = Chat {
            id: 10,
            participants: [(1, 0), (7, 0)].into_iter().collect(),
            ..Chat::default()
        };
        let channel = Chat {
            id: -5,
            title: Some("Канал".into()),
            participants: [(1, 0)].into_iter().collect(),
            ..Chat::default()
        };
        let login = login_with(vec![dialog, channel]);
        let peers = dialog_peers(&login);
        assert_eq!(peers, [(10, 7)]);
        let mut names = Names::from_login(&login);
        assert_eq!(names.missing([7, 1, 7, 0]), [7]);
        assert!(names.titles(&peers).is_empty());
        names.add(&[contact(7, "Борис")]);
        let titles: Vec<(i64, String)> = names
            .titles(&peers)
            .into_iter()
            .map(|t| (t.chat_id, t.title))
            .collect();
        assert_eq!(titles, [(10, "Борис".to_string())]);
        assert!(names.missing([7]).is_empty());
    }

    #[test]
    fn this_session_first_then_newest() {
        use max_core::account::SessionInfo;
        let s = |client: &str, time: i64, current: bool| SessionInfo {
            client: client.into(),
            time,
            current: Some(current),
            ..SessionInfo::default()
        };
        let rows = session_rows(&[s("old", 1, false), s("me", 5, true), s("new", 9, false)]);
        let order: Vec<&str> = rows.iter().map(|r| r.client.as_str()).collect();
        assert_eq!(order, ["me", "new", "old"]);
    }

    #[test]
    fn server_events_map_to_ui_events() {
        use max_core::events::ServerEvent as E;
        let names = Names::from_login(&login_with(vec![]));
        let typing = |user_id| E::Typing {
            chat_id: 3,
            user_id,
            kind: None,
        };
        assert!(names.ui_event(typing(1)).is_none(), "our own typing");
        assert!(matches!(
            names.ui_event(typing(7)),
            Some(UiEvent::Typing { chat_id: 3, who }) if who.is_empty()
        ));
        let mark = |user_id| E::Mark {
            chat_id: 3,
            user_id,
            mark: 50,
            unread: Some(2),
        };
        assert!(matches!(
            names.ui_event(mark(1)),
            Some(UiEvent::ReadElsewhere {
                chat_id: 3,
                unread: 2
            })
        ));
        assert!(matches!(
            names.ui_event(mark(7)),
            Some(UiEvent::PeerRead {
                chat_id: 3,
                mark: 50
            })
        ));
    }

    #[test]
    fn peer_mark_is_the_newest_other_mark() {
        let chat = Chat {
            id: 3,
            participants: [(1, 900), (7, 40), (8, 60)].into_iter().collect(),
            ..Chat::default()
        };
        let row = Names::from_login(&login_with(vec![])).chat_row(&chat);
        assert_eq!(row.peer_mark, 60, "our own mark does not count");
    }

    #[test]
    fn media_previews_name_albums_and_videos() {
        let video = Attachment::Video {
            video_id: 1,
            thumbnail: None,
            width: 0,
            height: 0,
            duration: 0,
        };
        let photo = Attachment::Photo {
            photo_id: None,
            url: String::new(),
            width: 0,
            height: 0,
        };
        let row = |attachments| MessageRow {
            attachments,
            ..MessageRow::default()
        };
        assert_eq!(row(vec![video.clone()]).preview(), "Видео");
        assert_eq!(row(vec![photo, video]).preview(), "Альбом");
    }

    #[test]
    fn previews_are_one_line() {
        assert_eq!(
            one_line(
                "OZON.ru

Для  проверки	"
            ),
            "OZON.ru Для проверки"
        );
    }
}
