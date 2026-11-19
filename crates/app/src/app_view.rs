//! The window content: login screens and the main view, driven by backend events.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::{ActiveTheme, *};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::backend::{ChatRow, UiCommand, UiEvent};
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

/// Root view of the main window.
pub struct AppView {
    screen: Screen,
    phone: Entity<InputState>,
    code: Entity<InputState>,
    password: Entity<InputState>,
    commands: UnboundedSender<UiCommand>,
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
            phone,
            code,
            password,
            commands,
        }
    }

    fn apply(&mut self, event: UiEvent, cx: &mut Context<Self>) {
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
        };
        cx.notify();
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
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Screen::Main { selected, .. } = &mut this.screen {
                    *selected = Some(id);
                    cx.notify();
                }
            }))
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

    fn render_main(
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
            Some(chat) => v_flex()
                .flex_1()
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
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_color(cx.theme().muted_foreground)
                        .child("История сообщений появится на следующем шаге."),
                ),
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

/// "14:05" for today, "19.11" for other days, in the system time zone.
fn short_time(ms: i64) -> String {
    short_time_in(ms, jiff::Timestamp::now(), &jiff::tz::TimeZone::system())
}

fn short_time_in(ms: i64, now: jiff::Timestamp, tz: &jiff::tz::TimeZone) -> String {
    if ms <= 0 {
        return String::new();
    }
    let Ok(at) = jiff::Timestamp::from_millisecond(ms) else {
        return String::new();
    };
    let at = at.to_zoned_datetime(tz.clone());
    if at.date() == now.to_zoned_datetime(tz.clone()).date() {
        at.strftime("%H:%M").to_string()
    } else {
        at.strftime("%d.%m").to_string()
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
                Self::render_main(&me, &chats, selected, &status, cx)
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
    use super::{normalize_phone, short_time_in};

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
}
