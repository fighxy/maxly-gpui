//! The window content: login screens and the main view, driven by backend events.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, *};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use max_core::media::Attachment;

use crate::backend::{ChatRow, Delivery, MessageRow, UiCommand, UiEvent};
use crate::qr;

enum Screen {
    Status(String),
    Qr(String),
    Phone {
        error: Option<String>,
        sent: bool,
    },
    Code {
        phone: String,
        length: i64,
        resend_after: i64,
        error: Option<String>,
        sent: bool,
    },
    Password {
        email: Option<String>,
        error: Option<String>,
        sent: bool,
    },
    Main {
        me: String,
        chats: Vec<ChatRow>,
        selected: Option<i64>,
        status: Option<String>,
    },
    Error(String),
}

/// The messages of one chat as far as they are loaded.
#[derive(Default)]
struct History {
    messages: Vec<MessageRow>,
    loading: bool,
    error: Option<String>,
    /// Older messages exist on the server.
    has_more: bool,
    /// An older page is on its way.
    loading_older: bool,
    /// The last message id we told the server we read.
    marked: i64,
}

impl History {
    /// Put an older page in front, skipping messages already shown.
    fn prepend(&mut self, older: Vec<MessageRow>) {
        let mut fresh: Vec<MessageRow> = older
            .into_iter()
            .filter(|m| m.id == 0 || !self.messages.iter().any(|k| k.id == m.id))
            .collect();
        fresh.append(&mut self.messages);
        self.messages = fresh;
    }

    /// The newest message from the server, if it is newer than the last read mark.
    fn unread_mark(&self) -> Option<i64> {
        let newest = self
            .messages
            .iter()
            .rev()
            .find(|m| m.id != 0 && m.delivery == Delivery::Sent)?
            .id;
        (newest != self.marked).then_some(newest)
    }

    /// Add a message, or replace our pending copy with the same `cid`; skip known ids.
    fn add(&mut self, message: MessageRow) {
        if let Some(cid) = message.cid
            && let Some(pending) = self
                .messages
                .iter_mut()
                .find(|m| m.out && m.cid == Some(cid))
        {
            *pending = message;
            return;
        }
        if message.id != 0 && self.messages.iter().any(|m| m.id == message.id) {
            return;
        }
        self.messages.push(message);
    }
}

/// A photo or sticker image for display.
enum ImageState {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

/// The image format of downloaded bytes, by their signature.
fn image_format(bytes: &[u8]) -> Option<ImageFormat> {
    let head = &bytes[..bytes.len().min(256)];
    let text = String::from_utf8_lossy(head);
    if bytes.starts_with(b"\x89PNG") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF8") {
        Some(ImageFormat::Gif)
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if text.trim_start().starts_with("<svg") || text.trim_start().starts_with("<?xml") {
        Some(ImageFormat::Svg)
    } else {
        None
    }
}

/// "245 КБ" style size.
fn human_size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} ГБ", b / (1024.0 * 1024.0 * 1024.0))
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.1} МБ", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.0} КБ", b / 1024.0)
    } else {
        format!("{} Б", bytes.max(0))
    }
}

/// Root view of the main window.
pub struct AppView {
    images: HashMap<String, ImageState>,
    screen: Screen,
    histories: HashMap<i64, History>,
    phone: Entity<InputState>,
    code: Entity<InputState>,
    password: Entity<InputState>,
    composer: Entity<InputState>,
    scroll: ScrollHandle,
    commands: UnboundedSender<UiCommand>,
    _subscriptions: Vec<Subscription>,
}

impl AppView {
    /// Build the view and start listening to backend events.
    pub fn new(
        mut events: UnboundedReceiver<UiEvent>,
        commands: UnboundedSender<UiCommand>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AppView {
        let phone = cx.new(|cx| InputState::new(window, cx).placeholder("+7 900 000-00-00"));
        let code = cx.new(|cx| InputState::new(window, cx).placeholder("Код"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Пароль")
        });
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder("Сообщение"));
        let subscriptions =
            vec![
                cx.subscribe_in(&composer, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.send_message(window, cx);
                    }
                }),
            ];
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this.update(cx, |view, cx| view.apply(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        AppView {
            screen: Screen::Status("Запуск…".into()),
            histories: HashMap::new(),
            phone,
            code,
            password,
            composer,
            scroll: ScrollHandle::new(),
            images: HashMap::new(),
            commands,
            _subscriptions: subscriptions,
        }
    }

    fn apply(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        match event {
            UiEvent::History {
                chat_id,
                messages,
                has_more,
            } => {
                let h = self.histories.entry(chat_id).or_default();
                let pending: Vec<MessageRow> = h
                    .messages
                    .drain(..)
                    .filter(|m| m.delivery != Delivery::Sent)
                    .collect();
                h.messages = messages;
                h.loading = false;
                h.error = None;
                h.has_more = has_more;
                for m in pending {
                    h.add(m);
                }
                let urls: Vec<String> = h.messages.iter().flat_map(image_urls).collect();
                self.request_images(urls);
                // `--demo-walk` sends a history without a click: open that chat.
                if let Screen::Main {
                    selected, chats, ..
                } = &mut self.screen
                    && selected.is_none()
                {
                    *selected = Some(chat_id);
                    if let Some(row) = chats.iter_mut().find(|c| c.id == chat_id) {
                        row.unread = 0;
                    }
                }
                self.scroll.scroll_to_bottom();
                self.mark_read(chat_id);
            }
            UiEvent::Older {
                chat_id,
                messages,
                has_more,
            } => {
                let urls: Vec<String> = messages.iter().flat_map(image_urls).collect();
                let h = self.histories.entry(chat_id).or_default();
                h.prepend(messages);
                h.has_more = has_more;
                h.loading_older = false;
                self.request_images(urls);
            }
            UiEvent::HistoryFailed { chat_id, error } => {
                let h = self.histories.entry(chat_id).or_default();
                h.loading = false;
                h.error = Some(error);
            }
            UiEvent::MessageAdded { chat_id, message } => {
                self.request_images(image_urls(&message));
                self.message_added(chat_id, message);
                self.mark_read(chat_id);
            }
            UiEvent::Image { url, bytes } => {
                let state = match image_format(&bytes) {
                    Some(format) => {
                        ImageState::Ready(Arc::new(Image::from_bytes(format, bytes.to_vec())))
                    }
                    None => ImageState::Failed,
                };
                self.images.insert(url, state);
            }
            UiEvent::ImageFailed { url } => {
                self.images.insert(url, ImageState::Failed);
            }
            UiEvent::ChatTitles(titles) => {
                if let Screen::Main { chats, .. } = &mut self.screen {
                    for (id, title) in titles {
                        if let Some(row) = chats.iter_mut().find(|c| c.id == id) {
                            row.title = title;
                        }
                    }
                }
            }
            UiEvent::Notice(text) => {
                if let Screen::Main { status, .. } = &mut self.screen {
                    *status = Some(text);
                }
                // A failed older page must not block the next try.
                for h in self.histories.values_mut() {
                    h.loading_older = false;
                }
            }
            UiEvent::SendFailed {
                chat_id,
                cid,
                error,
            } => {
                if let Some(m) = self
                    .histories
                    .get_mut(&chat_id)
                    .and_then(|h| h.messages.iter_mut().find(|m| m.cid == Some(cid)))
                {
                    m.delivery = Delivery::Failed;
                    m.sender = error;
                }
            }
            other => self.apply_screen(other),
        }
        cx.notify();
    }

    fn message_added(&mut self, chat_id: i64, message: MessageRow) {
        let Screen::Main {
            chats, selected, ..
        } = &mut self.screen
        else {
            return;
        };
        let open = *selected == Some(chat_id);
        if let Some(pos) = chats.iter().position(|c| c.id == chat_id) {
            let mut row = chats.remove(pos);
            row.preview = message.preview();
            row.time = row.time.max(message.time);
            if !open && !message.out && message.delivery == Delivery::Sent {
                row.unread += 1;
            }
            chats.insert(0, row);
        }
        if let Some(h) = self.histories.get_mut(&chat_id) {
            h.add(message);
            if open {
                self.scroll.scroll_to_bottom();
            }
        }
    }

    fn request_images(&mut self, urls: Vec<String>) {
        for url in urls {
            if !self.images.contains_key(&url) {
                self.images.insert(url.clone(), ImageState::Loading);
                let _ = self.commands.send(UiCommand::FetchImage(url));
            }
        }
    }

    fn pick_file(&mut self, cx: &mut Context<Self>) {
        let Screen::Main {
            selected: Some(chat_id),
            ..
        } = self.screen
        else {
            return;
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Отправить".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |view, cx| view.send_file(chat_id, path, cx));
            }
        })
        .detach();
    }

    fn send_file(&mut self, chat_id: i64, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let cid = jiff::Timestamp::now().as_millisecond();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let pending = MessageRow {
            id: 0,
            cid: Some(cid),
            out: true,
            sender: String::new(),
            text: format!("Отправка: {name}"),
            time: cid,
            delivery: Delivery::Pending,
            attachments: Vec::new(),
        };
        if self
            .commands
            .send(UiCommand::SendFile { chat_id, path, cid })
            .is_ok()
        {
            self.message_added(chat_id, pending);
            cx.notify();
        }
    }

    fn apply_screen(&mut self, event: UiEvent) {
        self.screen = match (
            event,
            std::mem::replace(&mut self.screen, Screen::Status(String::new())),
        ) {
            (
                UiEvent::Status(s),
                Screen::Main {
                    me,
                    chats,
                    selected,
                    ..
                },
            ) => Screen::Main {
                me,
                chats,
                selected,
                status: Some(s),
            },
            (UiEvent::Status(s), _) => Screen::Status(s),
            (UiEvent::Qr(link), _) => Screen::Qr(link),
            (UiEvent::AskPhone { error }, _) => Screen::Phone { error, sent: false },
            (
                UiEvent::AskCode {
                    phone,
                    length,
                    resend_after,
                    error,
                },
                _,
            ) => Screen::Code {
                phone,
                length,
                resend_after,
                error,
                sent: false,
            },
            (UiEvent::NeedPassword { email, error }, _) => Screen::Password {
                email,
                error,
                sent: false,
            },
            (UiEvent::LoggedIn { me, chats }, old) => {
                let selected = match old {
                    Screen::Main { selected, .. } => selected,
                    _ => None,
                };
                Screen::Main {
                    me,
                    chats,
                    selected,
                    status: None,
                }
            }
            (UiEvent::Error(e), _) => Screen::Error(e),
            (_, old) => old,
        };
    }

    fn send(&mut self, command: UiCommand, cx: &mut Context<Self>) {
        if self.commands.send(command).is_ok() {
            match &mut self.screen {
                Screen::Phone { sent, .. }
                | Screen::Code { sent, .. }
                | Screen::Password { sent, .. } => *sent = true,
                _ => {}
            }
            cx.notify();
        }
    }

    fn open_chat(&mut self, chat_id: i64, cx: &mut Context<Self>) {
        let Screen::Main {
            chats, selected, ..
        } = &mut self.screen
        else {
            return;
        };
        *selected = Some(chat_id);
        if let Some(row) = chats.iter_mut().find(|c| c.id == chat_id) {
            row.unread = 0;
        }
        let h = self.histories.entry(chat_id).or_default();
        if h.messages.is_empty() && !h.loading {
            h.loading = true;
            h.error = None;
            let _ = self.commands.send(UiCommand::OpenChat(chat_id));
        }
        self.scroll.scroll_to_bottom();
        self.mark_read(chat_id);
        cx.notify();
    }

    /// Tell the server the open chat is read up to its newest message.
    fn mark_read(&mut self, chat_id: i64) {
        let open = matches!(self.screen, Screen::Main { selected: Some(id), .. } if id == chat_id);
        if !open {
            return;
        }
        if let Some(h) = self.histories.get_mut(&chat_id)
            && let Some(message_id) = h.unread_mark()
            && self
                .commands
                .send(UiCommand::MarkRead {
                    chat_id,
                    message_id,
                })
                .is_ok()
        {
            h.marked = message_id;
        }
    }

    /// Ask for the page before the oldest message shown.
    fn load_older(&mut self, chat_id: i64, cx: &mut Context<Self>) {
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        if !h.has_more || h.loading_older || h.loading {
            return;
        }
        let Some(before) = h.messages.iter().map(|m| m.time).filter(|t| *t > 0).min() else {
            return;
        };
        if self
            .commands
            .send(UiCommand::LoadOlder { chat_id, before })
            .is_ok()
        {
            h.loading_older = true;
            cx.notify();
        }
    }

    fn send_message(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Screen::Main {
            selected: Some(chat_id),
            ..
        } = self.screen
        else {
            return;
        };
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        let cid = jiff::Timestamp::now().as_millisecond();
        let pending = MessageRow {
            id: 0,
            cid: Some(cid),
            out: true,
            sender: String::new(),
            text: text.clone(),
            time: cid,
            delivery: Delivery::Pending,
            attachments: Vec::new(),
        };
        if self
            .commands
            .send(UiCommand::SendText { chat_id, text, cid })
            .is_ok()
        {
            self.message_added(chat_id, pending);
            self.composer
                .update(cx, |input, cx| input.set_value("", window, cx));
            cx.notify();
        }
    }

    fn submit_phone(&mut self, cx: &mut Context<Self>) {
        let raw = self.phone.read(cx).value().to_string();
        if let Some(phone) = normalize_phone(&raw) {
            self.send(UiCommand::Phone(phone), cx);
        } else if let Screen::Phone { error, .. } = &mut self.screen {
            *error =
                Some("Введите номер в международном формате, например +7 900 000-00-00.".into());
            cx.notify();
        }
    }

    fn submit_code(&mut self, cx: &mut Context<Self>) {
        let code: String = self
            .code
            .read(cx)
            .value()
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        if !code.is_empty() {
            self.send(UiCommand::Code(code), cx);
        }
    }

    fn submit_password(&mut self, cx: &mut Context<Self>) {
        let text = self.password.read(cx).value().to_string();
        if !text.is_empty() {
            self.send(UiCommand::Password(text), cx);
        }
    }

    fn centered(content: impl IntoElement, cx: &App) -> Div {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(content)
    }

    fn form(title: &str, hint: String, cx: &App) -> Div {
        v_flex()
            .gap_3()
            .w(px(380.))
            .child(div().text_2xl().font_semibold().child(title.to_string()))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(hint),
            )
    }

    fn error_line(error: &Option<String>, cx: &App) -> Option<Div> {
        error.as_ref().map(|e| {
            div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(e.clone())
        })
    }

    fn render_qr(&self, link: &str, cx: &mut Context<Self>) -> Div {
        let text = v_flex()
            .gap_3()
            .max_w(px(360.))
            .child(div().text_2xl().font_semibold().child("Вход в MAX"))
            .child(Label::new("1. Откройте MAX на телефоне."))
            .child(Label::new("2. Найдите вход на компьютере по QR-коду."))
            .child(Label::new("3. Наведите камеру на этот код."))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Код обновляется сам, пока вы не войдёте."),
            )
            .child(
                Button::new("use-phone")
                    .label("Войти по номеру телефона")
                    .on_click(cx.listener(|this, _, _, cx| this.send(UiCommand::UsePhone, cx))),
            );
        Self::centered(
            h_flex().gap_12().items_center().child(text).child(
                div()
                    .rounded(px(12.))
                    .overflow_hidden()
                    .child(qr::view(link, 6.0)),
            ),
            cx,
        )
    }

    fn render_phone(&self, error: &Option<String>, sent: bool, cx: &mut Context<Self>) -> Div {
        let form = Self::form(
            "Вход по номеру",
            "На этот номер придёт код подтверждения.".into(),
            cx,
        )
        .child(Input::new(&self.phone))
        .children(Self::error_line(error, cx))
        .child(
            Button::new("phone-ok")
                .primary()
                .label(if sent {
                    "Отправляем…"
                } else {
                    "Получить код"
                })
                .disabled(sent)
                .on_click(cx.listener(|this, _, _, cx| this.submit_phone(cx))),
        )
        .child(
            Button::new("back-to-qr")
                .ghost()
                .label("Войти по QR-коду")
                .on_click(cx.listener(|this, _, _, cx| this.send(UiCommand::UseQr, cx))),
        );
        Self::centered(form, cx)
    }

    fn render_code(
        &self,
        phone: &str,
        length: i64,
        resend_after: i64,
        error: &Option<String>,
        sent: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut hint = format!(
            "Код из {length} цифр отправлен на {phone}. Он может прийти в приложение MAX на другом устройстве или по SMS."
        );
        if resend_after > 0 {
            hint.push_str(&format!(
                " Запросить код снова можно через {resend_after} с."
            ));
        }
        let form = Self::form("Код подтверждения", hint, cx)
            .child(Input::new(&self.code))
            .children(Self::error_line(error, cx))
            .child(
                Button::new("code-ok")
                    .primary()
                    .label(if sent {
                        "Проверяем…"
                    } else {
                        "Войти"
                    })
                    .disabled(sent)
                    .on_click(cx.listener(|this, _, _, cx| this.submit_code(cx))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("resend")
                            .ghost()
                            .label("Отправить ещё раз")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.send(UiCommand::Resend, cx)),
                            ),
                    )
                    .child(
                        Button::new("other-number")
                            .ghost()
                            .label("Другой номер")
                            .on_click(
                                cx.listener(|this, _, _, cx| this.send(UiCommand::UsePhone, cx)),
                            ),
                    ),
            );
        Self::centered(form, cx)
    }

    fn render_password(
        &self,
        email: &Option<String>,
        error: &Option<String>,
        sent: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let hint = email
            .as_ref()
            .map(|e| format!("Пароль от аккаунта. Почта для восстановления: {e}"))
            .unwrap_or_else(|| "У аккаунта включён облачный пароль.".into());
        let form = Self::form("Пароль", hint, cx)
            .child(Input::new(&self.password))
            .children(Self::error_line(error, cx))
            .child(
                Button::new("password-ok")
                    .primary()
                    .label(if sent {
                        "Проверяем…"
                    } else {
                        "Войти"
                    })
                    .disabled(sent)
                    .on_click(cx.listener(|this, _, _, cx| this.submit_password(cx))),
            );
        Self::centered(form, cx)
    }

    fn chat_row(row: &ChatRow, selected: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let id = row.id;
        let initials: String = row
            .title
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default();
        h_flex()
            .id(("chat", u64::from_ne_bytes(id.to_ne_bytes())))
            .gap_3()
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .when(selected, |this| this.bg(cx.theme().accent))
            .hover(|this| this.bg(cx.theme().secondary_hover))
            .on_click(cx.listener(move |this, _, _, cx| this.open_chat(id, cx)))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .size_10()
                    .rounded_full()
                    .bg(cx.theme().primary)
                    .text_color(cx.theme().primary_foreground)
                    .child(initials),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .child(div().font_medium().truncate().child(row.title.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(short_time(row.time)),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(row.preview.clone()),
                            )
                            .when(row.unread > 0, |this| {
                                this.child(
                                    div()
                                        .px_2()
                                        .rounded_full()
                                        .text_xs()
                                        .bg(cx.theme().primary)
                                        .text_color(cx.theme().primary_foreground)
                                        .child(row.unread.to_string()),
                                )
                            }),
                    ),
            )
    }

    fn attachment_view(
        &self,
        chat_id: i64,
        m: &MessageRow,
        a: &Attachment,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match a {
            Attachment::Photo { url, .. } | Attachment::Sticker { url } => {
                let sticker = matches!(a, Attachment::Sticker { .. });
                let (w, h) = match a {
                    Attachment::Photo { width, height, .. } if *width > 0 && *height > 0 => {
                        let w = (*width as f32).min(320.0);
                        (w, w * (*height as f32) / (*width as f32))
                    }
                    _ if sticker => (160.0, 160.0),
                    _ => (320.0, 210.0),
                };
                match self.images.get(url) {
                    Some(ImageState::Ready(image)) => img(image.clone())
                        .w(px(w))
                        .h(px(h))
                        .rounded(px(8.))
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                    state => div()
                        .w(px(w))
                        .h(px(h))
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(cx.theme().muted)
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(match state {
                            Some(ImageState::Failed) => "Фото не загрузилось",
                            _ => "Загрузка фото…",
                        })
                        .into_any_element(),
                }
            }
            Attachment::File {
                file_id,
                name,
                size,
            } => {
                let (message_id, file_id, name_c) = (m.id, *file_id, name.clone());
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().text_2xl().child("📄"))
                    .child(
                        v_flex()
                            .min_w_0()
                            .child(div().font_medium().truncate().child(name.clone()))
                            .child(div().text_xs().opacity(0.7).child(human_size(*size))),
                    )
                    .when(message_id != 0, |this| {
                        this.child(
                            Button::new((
                                "download",
                                u64::from_ne_bytes(m.id.to_ne_bytes()).wrapping_add(index as u64),
                            ))
                            .small()
                            .label("Скачать")
                            .on_click(cx.listener(
                                move |this, _, _, _| {
                                    let _ = this.commands.send(UiCommand::DownloadFile {
                                        chat_id,
                                        message_id,
                                        file_id,
                                        name: name_c.clone(),
                                    });
                                },
                            )),
                        )
                    })
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn bubble(
        &self,
        chat_id: i64,
        m: &MessageRow,
        chat_title: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let attachments: Vec<AnyElement> = m
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, a)| !matches!(a, Attachment::Other { .. }))
            .map(|(i, a)| self.attachment_view(chat_id, m, a, i, cx))
            .collect();
        let state = match m.delivery {
            Delivery::Sent => String::new(),
            Delivery::Pending => " · отправка…".into(),
            Delivery::Failed => " · не отправлено".into(),
        };
        let (bg, fg, muted) = if m.out {
            (
                cx.theme().primary,
                cx.theme().primary_foreground,
                cx.theme().primary_foreground.opacity(0.7),
            )
        } else {
            (
                cx.theme().secondary,
                cx.theme().secondary_foreground,
                cx.theme().muted_foreground,
            )
        };
        let show_sender = !m.out && !m.sender.is_empty() && m.sender != chat_title;
        h_flex()
            .w_full()
            .when(m.out, |this| this.justify_end())
            .child(
                v_flex()
                    .max_w(px(520.))
                    .px_3()
                    .py_2()
                    .gap_1()
                    .rounded(px(12.))
                    .bg(bg)
                    .text_color(fg)
                    .when(m.delivery == Delivery::Failed, |this| {
                        this.border_1().border_color(cx.theme().danger)
                    })
                    .when(show_sender, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().primary)
                                .child(m.sender.clone()),
                        )
                    })
                    .children(attachments)
                    .when(!m.text.is_empty(), |this| {
                        this.child(div().child(m.text.clone()))
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!("{}{state}", clock(m.time))),
                    ),
            )
    }

    fn render_chat(&self, chat: &ChatRow, cx: &mut Context<Self>) -> Div {
        let history = self.histories.get(&chat.id);
        let mut items: Vec<AnyElement> = Vec::new();
        let mut last_day = String::new();
        for m in history.map(|h| h.messages.as_slice()).unwrap_or_default() {
            let day = day_label(m.time);
            if day != last_day {
                items.push(
                    h_flex()
                        .w_full()
                        .justify_center()
                        .py_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(day.clone()),
                        )
                        .into_any_element(),
                );
                last_day = day;
            }
            items.push(self.bubble(chat.id, m, &chat.title, cx).into_any_element());
        }
        let chat_id = chat.id;
        // None: no older page; Some(loading) otherwise.
        let older_state = history
            .filter(|h| h.has_more && !h.messages.is_empty())
            .map(|h| h.loading_older);
        let notice = match history {
            Some(h) if h.loading => Some("Загрузка…".to_string()),
            Some(History { error: Some(e), .. }) => {
                Some(format!("Не удалось загрузить историю: {e}"))
            }
            Some(h) if h.messages.is_empty() => Some("Сообщений пока нет".to_string()),
            _ => None,
        };
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .font_semibold()
                    .child(chat.title.clone()),
            )
            .child(
                v_flex()
                    .id("history")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, cx| {
                        // Scrolling up at the top loads the previous page.
                        let up = ev.delta.pixel_delta(px(20.)).y > px(0.);
                        if up && this.scroll.offset().y >= px(-4.) {
                            this.load_older(chat_id, cx);
                        }
                    }))
                    .px_4()
                    .py_3()
                    .gap_2()
                    .when(older_state.is_some(), |this| {
                        let loading = older_state == Some(true);
                        this.child(
                            h_flex().w_full().justify_center().child(
                                Button::new("older")
                                    .ghost()
                                    .small()
                                    .label(if loading {
                                        "Загрузка…"
                                    } else {
                                        "Показать более ранние"
                                    })
                                    .disabled(loading)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.load_older(chat_id, cx)
                                    })),
                            ),
                        )
                    })
                    .when_some(notice, |this, n| {
                        this.child(
                            h_flex()
                                .w_full()
                                .justify_center()
                                .py_4()
                                .text_color(cx.theme().muted_foreground)
                                .child(n),
                        )
                    })
                    .children(items),
            )
            .child(
                h_flex()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("attach")
                            .ghost()
                            .label("📎")
                            .on_click(cx.listener(|this, _, _, cx| this.pick_file(cx))),
                    )
                    .child(div().flex_1().child(Input::new(&self.composer)))
                    .child(Button::new("send").primary().label("Отправить").on_click(
                        cx.listener(|this, _, window, cx| this.send_message(window, cx)),
                    )),
            )
    }

    fn render_main(
        &self,
        me: &str,
        chats: &[ChatRow],
        selected: Option<i64>,
        status: &Option<String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let list = v_flex()
            .id("chat-list")
            .flex_1()
            .overflow_y_scroll()
            .p_2()
            .gap_1()
            .children(
                chats
                    .iter()
                    .map(|row| Self::chat_row(row, selected == Some(row.id), cx)),
            );
        let sidebar = v_flex()
            .w(px(340.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                v_flex()
                    .px_4()
                    .py_3()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().font_semibold().child("Чаты"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(me.to_string()),
                    )
                    .when_some(status.clone(), |this, s| {
                        this.child(div().text_xs().text_color(cx.theme().warning).child(s))
                    }),
            )
            .child(list);
        let content = match selected.and_then(|id| chats.iter().find(|c| c.id == id)) {
            Some(chat) => self.render_chat(chat, cx),
            None => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("Выберите чат"),
        };
        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(sidebar)
            .child(content)
    }
}

/// Image addresses a message needs.
fn image_urls(m: &MessageRow) -> Vec<String> {
    m.attachments
        .iter()
        .filter_map(|a| match a {
            Attachment::Photo { url, .. } | Attachment::Sticker { url } => Some(url.clone()),
            _ => None,
        })
        .collect()
}

/// `+79000000000` from what the user typed: digits with a leading `+`; a Russian `8...` or
/// `9...` gets `+7`. `None` when it cannot be a phone number.
fn normalize_phone(raw: &str) -> Option<String> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    let digits = match (
        raw.trim_start().starts_with('+'),
        digits.len(),
        digits.chars().next(),
    ) {
        (false, 11, Some('8')) => format!("7{}", &digits[1..]),
        (false, 10, Some('9')) => format!("7{digits}"),
        _ => digits,
    };
    (10..=15)
        .contains(&digits.len())
        .then(|| format!("+{digits}"))
}

fn zoned(ms: i64, tz: &jiff::tz::TimeZone) -> Option<jiff::ZonedDateTime> {
    jiff::Timestamp::from_millisecond(ms)
        .ok()
        .map(|t| t.to_zoned_datetime(tz.clone()))
}

/// "14:05" for today, "19.11" for other days, in the system time zone.
fn short_time(ms: i64) -> String {
    short_time_in(ms, jiff::Timestamp::now(), &jiff::tz::TimeZone::system())
}

fn short_time_in(ms: i64, now: jiff::Timestamp, tz: &jiff::tz::TimeZone) -> String {
    if ms <= 0 {
        return String::new();
    }
    let Some(at) = zoned(ms, tz) else {
        return String::new();
    };
    if at.date() == now.to_zoned_datetime(tz.clone()).date() {
        at.strftime("%H:%M").to_string()
    } else {
        at.strftime("%d.%m").to_string()
    }
}

/// "14:05" in the system time zone.
fn clock(ms: i64) -> String {
    zoned(ms, &jiff::tz::TimeZone::system())
        .map(|z| z.strftime("%H:%M").to_string())
        .unwrap_or_default()
}

/// "Сегодня", "Вчера" or "19.11.2026" for the day of `ms`.
fn day_label(ms: i64) -> String {
    day_label_in(ms, jiff::Timestamp::now(), &jiff::tz::TimeZone::system())
}

fn day_label_in(ms: i64, now: jiff::Timestamp, tz: &jiff::tz::TimeZone) -> String {
    let Some(at) = zoned(ms, tz) else {
        return String::new();
    };
    let today = now.to_zoned_datetime(tz.clone()).date();
    if at.date() == today {
        "Сегодня".into()
    } else if today.yesterday().ok() == Some(at.date()) {
        "Вчера".into()
    } else {
        at.strftime("%d.%m.%Y").to_string()
    }
}

impl Render for AppView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.screen {
            Screen::Status(s) => Self::centered(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_2xl().font_semibold().child("maxrs"))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(s.clone()),
                    ),
                cx,
            ),
            Screen::Qr(link) => {
                let link = link.clone();
                self.render_qr(&link, cx)
            }
            Screen::Phone { error, sent } => {
                let (error, sent) = (error.clone(), *sent);
                self.render_phone(&error, sent, cx)
            }
            Screen::Code {
                phone,
                length,
                resend_after,
                error,
                sent,
            } => {
                let (phone, length, resend_after, error, sent) =
                    (phone.clone(), *length, *resend_after, error.clone(), *sent);
                self.render_code(&phone, length, resend_after, &error, sent, cx)
            }
            Screen::Password { email, error, sent } => {
                let (email, error, sent) = (email.clone(), error.clone(), *sent);
                self.render_password(&email, &error, sent, cx)
            }
            Screen::Main {
                me,
                chats,
                selected,
                status,
            } => {
                let (me, chats, selected, status) =
                    (me.clone(), chats.clone(), *selected, status.clone());
                self.render_main(&me, &chats, selected, &status, cx)
            }
            Screen::Error(e) => Self::centered(
                v_flex()
                    .items_center()
                    .gap_2()
                    .max_w(px(480.))
                    .child(div().text_xl().font_semibold().child("Не получилось"))
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child(e.clone()),
                    ),
                cx,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Delivery, History, MessageRow, day_label_in, normalize_phone, short_time_in};

    #[test]
    fn local_time_today_and_date_otherwise() {
        let tz = jiff::tz::TimeZone::get("Asia/Krasnoyarsk").unwrap();
        // 2026-11-19 12:00 UTC is 19:00 in Krasnoyarsk (UTC+7).
        let noon_utc = 1_795_089_600_000;
        let now = jiff::Timestamp::from_millisecond(noon_utc + 60_000).unwrap();
        assert_eq!(short_time_in(noon_utc, now, &tz), "19:00");
        // A day earlier shows the local date.
        assert_eq!(short_time_in(noon_utc - 86_400_000, now, &tz), "18.11");
        // 2026-11-19 20:00 UTC is already 2026-11-20 in Krasnoyarsk.
        assert_eq!(short_time_in(noon_utc + 8 * 3_600_000, now, &tz), "20.11");
        assert_eq!(short_time_in(0, now, &tz), "");
    }

    #[test]
    fn day_labels() {
        let tz = jiff::tz::TimeZone::get("Asia/Krasnoyarsk").unwrap();
        let noon_utc = 1_795_089_600_000;
        let now = jiff::Timestamp::from_millisecond(noon_utc).unwrap();
        assert_eq!(day_label_in(noon_utc, now, &tz), "Сегодня");
        assert_eq!(day_label_in(noon_utc - 86_400_000, now, &tz), "Вчера");
        assert_eq!(
            day_label_in(noon_utc - 3 * 86_400_000, now, &tz),
            "16.11.2026"
        );
    }

    #[test]
    fn phone_numbers() {
        assert_eq!(
            normalize_phone("+7 900 123-45-67").as_deref(),
            Some("+79001234567")
        );
        assert_eq!(
            normalize_phone("8 (900) 123-45-67").as_deref(),
            Some("+79001234567")
        );
        assert_eq!(
            normalize_phone("9001234567").as_deref(),
            Some("+79001234567")
        );
        assert_eq!(
            normalize_phone("+375 29 123 45 67").as_deref(),
            Some("+375291234567")
        );
        assert_eq!(normalize_phone("123"), None);
    }

    fn row(id: i64, cid: Option<i64>, out: bool, delivery: Delivery) -> MessageRow {
        MessageRow {
            id,
            cid,
            out,
            sender: String::new(),
            text: format!("m{id}"),
            time: id,
            delivery,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn confirmed_message_replaces_the_pending_copy_and_duplicates_are_skipped() {
        let mut h = History::default();
        h.add(row(1, None, false, Delivery::Sent));
        h.add(row(0, Some(77), true, Delivery::Pending));
        h.add(row(5, Some(77), true, Delivery::Sent));
        h.add(row(1, None, false, Delivery::Sent));
        let ids: Vec<(i64, Delivery)> = h.messages.iter().map(|m| (m.id, m.delivery)).collect();
        assert_eq!(ids, [(1, Delivery::Sent), (5, Delivery::Sent)]);
    }
}

#[cfg(test)]
mod media_tests {
    use super::{human_size, image_format};
    use gpui_kit::ImageFormat;

    #[test]
    fn image_signatures() {
        assert!(matches!(
            image_format(b"\x89PNG\r\n\x1a\n...."),
            Some(ImageFormat::Png)
        ));
        assert!(matches!(
            image_format(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(ImageFormat::Jpeg)
        ));
        assert!(matches!(
            image_format(b"RIFF\0\0\0\0WEBPVP8 "),
            Some(ImageFormat::Webp)
        ));
        assert!(matches!(
            image_format(b"<svg xmlns=..."),
            Some(ImageFormat::Svg)
        ));
        assert!(image_format(b"not an image").is_none());
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(512), "512 Б");
        assert_eq!(human_size(245_760), "240 КБ");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 МБ");
    }
}

#[cfg(test)]
mod paging_tests {
    use super::{Delivery, History, MessageRow};

    fn row(id: i64, delivery: Delivery) -> MessageRow {
        MessageRow {
            id,
            cid: None,
            out: false,
            sender: String::new(),
            text: String::new(),
            time: id * 10,
            delivery,
            attachments: Vec::new(),
        }
    }

    #[test]
    fn older_page_goes_in_front_without_duplicates() {
        let mut h = History {
            messages: vec![row(5, Delivery::Sent), row(6, Delivery::Sent)],
            ..History::default()
        };
        h.prepend(vec![
            row(3, Delivery::Sent),
            row(4, Delivery::Sent),
            row(5, Delivery::Sent),
        ]);
        let ids: Vec<i64> = h.messages.iter().map(|m| m.id).collect();
        assert_eq!(ids, [3, 4, 5, 6]);
    }

    #[test]
    fn read_mark_follows_the_newest_server_message_once() {
        let mut h = History {
            messages: vec![
                row(1, Delivery::Sent),
                row(2, Delivery::Sent),
                row(0, Delivery::Pending),
            ],
            ..History::default()
        };
        assert_eq!(h.unread_mark(), Some(2), "pending copies have no server id");
        h.marked = 2;
        assert_eq!(h.unread_mark(), None);
        h.messages.push(row(3, Delivery::Sent));
        assert_eq!(h.unread_mark(), Some(3));
    }
}
