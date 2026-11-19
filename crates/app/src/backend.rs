//! The bridge between the UI and `max-core`: a tokio runtime runs the login flow and the
//! supervisor, and reports [`UiEvent`]s; the UI sends [`UiCommand`]s back. `--demo` runs the
//! same screens on made-up data without any network access.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    let config = CoreConfig::default();

    if profile.auth_token().map_err(|e| e.to_string())?.is_none() {
        let _ = ui.send(UiEvent::Status("Подключение…".into()));
        let session = Session::connect(&config, &mut profile, &env)
            .await
            .map_err(|e| e.to_string())?;
        let token = login(&session, &ui, &mut commands, &env.system_language).await?;
        profile.set_auth_token(&token).map_err(|e| e.to_string())?;
        session.transport().close();
    }

    let (_handle, mut events) = Supervisor::spawn(
        config.clone(),
        Arc::new(TlsConnector::new(config)),
        profile,
        env,
    );
    while let Some(event) = events.recv().await {
        let msg = match event {
            CoreEvent::Connected => UiEvent::Status("Вход…".into()),
            CoreEvent::LoggedIn(login) => {
                let (me, chats) = rows_from_login(&login);
                UiEvent::LoggedIn { me, chats }
            }
            CoreEvent::AuthRequired => UiEvent::Error("Нужно войти заново.".into()),
            CoreEvent::Push(_) => continue,
            CoreEvent::Disconnected(reason) => {
                UiEvent::Status(format!("Нет связи, переподключение… ({reason})"))
            }
        };
        let _ = ui.send(msg);
    }
    Ok(())
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
        .and_then(|m| m.text.clone())
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
