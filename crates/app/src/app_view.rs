//! The window content: login screens and the main view, driven by backend events.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, *};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use max_core::media::Attachment;

use crate::album;
use crate::backend::{
    ChatKind, ChatRow, ContactRow, Delivery, MessageRow, Quote, ReactionCount, SessionRow,
    UiCommand, UiEvent, element_marks, one_line, place_row, reaction_counts,
};
use crate::desktop::{self, DesktopEvent};
use crate::instance::Instance;
use crate::qr;
use crate::rich;
use crate::settings::{FONT_SIZES, ThemeChoice, UiSettings};
use crate::viewer::{Viewer, ViewerPhoto, clamp_pan, fit_scale, rotate_pixels};
use gpui_kit::assets::IconName as Lucide;
use max_core::messages;

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

/// Extra message rows laid out above and below the viewport, so a fast scroll
/// does not paint empty space while the next rows are measured.
/// Kept to a few rows: a 120 Hz frame has about 8 ms.
const HISTORY_OVERDRAW: f32 = 240.;

/// How far one frame of a mouse-wheel glide may move, in pixels.
/// The list applies a whole notch at once; the rest is spread over later frames.
const WHEEL_FRAME_PX: f32 = 16.;

/// How long "typing" stays shown after the last typing event.
const TYPING_SHOWN: std::time::Duration = std::time::Duration::from_secs(6);
/// The least time between two typing notices we send for one chat.
const TYPING_EVERY: std::time::Duration = std::time::Duration::from_secs(5);

/// The account panel: active sessions and logout.
#[derive(Default)]
struct AccountPanel {
    /// `None` while loading.
    sessions: Option<Vec<SessionRow>>,
    error: Option<String>,
    /// An action waiting for a second click.
    confirm: Option<AccountAction>,
    /// A request is on its way.
    busy: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AccountAction {
    CloseOthers,
    Logout,
}

/// A section of the navigation rail.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Chats,
    Calls,
    Contacts,
    Settings,
}

/// A page of the settings section.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    /// Who we are, sessions, logout.
    Account,
    /// System notifications and the tray.
    Notifications,
    /// Theme and font size.
    Appearance,
    /// Where files and the profile are kept.
    Data,
    /// Version and what the client does not send.
    About,
}

impl SettingsPage {
    const ALL: [SettingsPage; 5] = [
        SettingsPage::Account,
        SettingsPage::Notifications,
        SettingsPage::Appearance,
        SettingsPage::Data,
        SettingsPage::About,
    ];

    fn title(self) -> &'static str {
        match self {
            SettingsPage::Account => "Аккаунт и устройства",
            SettingsPage::Notifications => "Уведомления",
            SettingsPage::Appearance => "Оформление",
            SettingsPage::Data => "Данные и память",
            SettingsPage::About => "О программе",
        }
    }

    fn icon(self) -> Icon {
        match self {
            SettingsPage::Account => Icon::new(Lucide::Laptop),
            SettingsPage::Notifications => Icon::new(IconName::Bell),
            SettingsPage::Appearance => Icon::new(IconName::Palette),
            SettingsPage::Data => Icon::new(IconName::HardDrive),
            SettingsPage::About => Icon::new(IconName::Info),
        }
    }
}

/// Corner radius of photo and video thumbnails in bubbles, px.
const CELL_RADIUS: f32 = 10.;

/// Prefix of a notification's tag; the chat id follows, so a click opens that chat.
const NOTIFY_TAG: &str = "chat:";

/// Title and text of a new-message notification: the chat, then the message (with its sender
/// in groups). Without `preview` it says only that a message came.
fn notification_text(chat: &ChatRow, m: &MessageRow, preview: bool) -> (String, String) {
    let title = chat.title.clone();
    if !preview {
        return (title, "Новое сообщение".to_string());
    }
    let text = match m.preview() {
        t if t.is_empty() => "Сообщение".to_string(),
        t => t,
    };
    let body = match chat.kind {
        ChatKind::Group if !m.sender.is_empty() && m.sender != chat.title => {
            format!("{}: {text}", m.sender)
        }
        _ => text,
    };
    // Windows cuts long toasts anyway; keep them short and on whole characters.
    let body = if body.chars().count() > 200 {
        format!("{}…", body.chars().take(199).collect::<String>())
    } else {
        body
    };
    (title, body)
}

/// Apply the theme and font size to the app.
fn apply_ui(ui: &UiSettings, window: &mut Window, cx: &mut App) {
    match ui.theme {
        ThemeChoice::System => Theme::sync_system_appearance(Some(window), cx),
        ThemeChoice::Light => Theme::change(ThemeMode::Light, Some(window), cx),
        ThemeChoice::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
    }
    Theme::global_mut(cx).font_size = px(f32::from(ui.font_size));
    window.refresh();
}

/// What the composer does besides writing a new message.
#[derive(Clone)]
enum Compose {
    /// The next message answers `quote`.
    Reply { quote: Quote },
    /// The composer holds the text of our message `message_id`.
    Edit { message_id: i64, original: String },
}

/// What [`History::add`] did with a message. The open transcript uses it to
/// insert or remeasure one virtual row instead of rebuilding the list.
enum Stored {
    /// A new row at the end.
    Pushed,
    /// An existing row was replaced.
    Updated(usize),
    /// The message was already shown.
    Skipped,
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
    /// Returns how many rows were inserted.
    fn prepend(&mut self, older: Vec<MessageRow>) -> usize {
        let mut fresh: Vec<MessageRow> = older
            .into_iter()
            .filter(|m| m.id == 0 || !self.messages.iter().any(|k| k.id == m.id))
            .collect();
        let added = fresh.len();
        fresh.append(&mut self.messages);
        self.messages = fresh;
        added
    }

    /// The newest incoming message, if it is newer than the last read mark. Our own messages
    /// need no mark: the server moves it on send (`MSG_SEND` answers with `mark`).
    fn unread_mark(&self) -> Option<i64> {
        let newest = self
            .messages
            .iter()
            .rev()
            .find(|m| !m.out && m.id != 0 && m.delivery == Delivery::Sent)?
            .id;
        (newest != self.marked).then_some(newest)
    }

    /// Add a message, or replace our pending copy with the same `cid`; skip known ids. A
    /// confirmed reply keeps the pending copy's quote if the server's copy has none.
    fn add(&mut self, mut message: MessageRow) -> Stored {
        if let Some(cid) = message.cid
            && let Some(ix) = self
                .messages
                .iter()
                .position(|m| m.out && m.cid == Some(cid))
        {
            if message.reply.is_none() {
                message.reply = self.messages[ix].reply.take();
            }
            self.messages[ix] = message;
            return Stored::Updated(ix);
        }
        if message.id != 0 && self.messages.iter().any(|m| m.id == message.id) {
            return Stored::Skipped;
        }
        self.messages.push(message);
        Stored::Pushed
    }

    /// Put an edited copy in place of a known message. What the copy leaves out (time, quote,
    /// forward, attachments, cid) is kept from the shown one: our edits change text only.
    fn edit(&mut self, mut new: MessageRow) {
        let Some(old) = self.messages.iter_mut().find(|m| m.id == new.id) else {
            return;
        };
        if new.time == 0 {
            new.time = old.time;
        }
        if new.reply.is_none() {
            new.reply = old.reply.take();
        }
        if new.forwarded.is_none() {
            new.forwarded = old.forwarded.take();
        }
        if new.attachments.is_empty() {
            new.attachments = std::mem::take(&mut old.attachments);
        }
        new.cid = new.cid.or(old.cid);
        *old = new;
    }
}

/// Our message whose text can be edited: sent, not forwarded, with text and no attachment that
/// adds a label to it.
fn editable(m: &MessageRow) -> bool {
    m.out
        && m.id != 0
        && m.delivery == Delivery::Sent
        && m.forwarded.is_none()
        && !m.text.is_empty()
        && m.attachments
            .iter()
            .all(|a| matches!(a, Attachment::Photo { .. } | Attachment::File { .. }))
}

/// A quote of `m` for a reply; `chat_title` names the sender of dialogs and channel posts.
fn quote_of(m: &MessageRow, chat_title: &str) -> Quote {
    let sender = if m.out {
        "Вы".to_string()
    } else if m.sender.is_empty() {
        chat_title.to_string()
    } else {
        m.sender.clone()
    };
    Quote {
        id: m.id,
        sender,
        text: m.preview(),
    }
}

/// Put a changed chat into the list: update it in place, or add it by time.
fn upsert_chat(chats: &mut Vec<ChatRow>, new: ChatRow) {
    if let Some(row) = chats.iter_mut().find(|c| c.id == new.id) {
        let preview = if new.preview.is_empty() {
            std::mem::take(&mut row.preview)
        } else {
            new.preview.clone()
        };
        let time = row.time.max(new.time);
        let peer_mark = row.peer_mark.max(new.peer_mark);
        // A dialog's photo comes from the contact, which a NOTIF_CHAT may not carry.
        let avatar = new.avatar.clone().or_else(|| row.avatar.take());
        // A server chat update does not carry pin and mute; those stay until a settings answer.
        let pin_index = row.pin_index;
        let muted = row.muted;
        *row = ChatRow {
            preview,
            time,
            peer_mark,
            avatar,
            pin_index,
            muted,
            ..new
        };
    } else {
        place_row(chats, new);
    }
}

/// A photo or sticker image for display.
enum ImageState {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

/// A Lottie emoji while it loads and after its frames are known.
enum LottieState {
    Loading,
    Ready(LottieClip),
    Failed,
}

/// One animated emoji. Every frame is rasterized off the UI thread before it is shown.
struct LottieClip {
    frames: u32,
    index: u32,
    cache: Vec<Arc<RenderImage>>,
}

/// Raster size of an animated emoji. Shown at 28px; 48 keeps it sharp on a 2× screen.
const LOTTIE_PX: u32 = 48;

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
    /// A `max.ru` link we already asked about: nothing to show, or the card.
    link_cards: HashMap<String, Option<Attachment>>,
    /// Preview images the user asked to see.
    shown_sensitive: HashSet<String>,
    /// Lottie emoji, keyed by the file address.
    lotties: HashMap<String, LottieState>,
    /// A timer is advancing the Lottie frames.
    lottie_clock: bool,
    /// Emoji a chat offers. Missing means the usual plate.
    reaction_choices: HashMap<i64, Vec<String>>,
    /// The message whose reaction plate is open (chat, message).
    reaction_picker: Option<(i64, i64)>,
    /// Who reacted to a message, while that list is open.
    reaction_people: Option<(i64, Vec<(String, String)>)>,
    /// The emoji picker is open above the composer.
    emoji_open: bool,
    /// Animated emoji already loaded. `None` until the first request finishes.
    animoji: Option<Vec<max_core::assets::AnimojiFace>>,
    /// A catalog request is in flight.
    animoji_loading: bool,
    /// Why the animated catalog is missing, when the request failed.
    animoji_note: Option<String>,
    /// Animated emoji inserted into the composer and not yet deleted.
    animoji_picks: Vec<max_core::assets::AnimojiFace>,
    screen: Screen,
    histories: HashMap<i64, History>,
    phone: Entity<InputState>,
    code: Entity<InputState>,
    password: Entity<InputState>,
    composer: Entity<InputState>,
    /// Virtual transcript of the open chat. Only the rows on screen are built.
    history_list: ListState,
    /// Chat the transcript belongs to. `None` until a chat is open.
    history_for: Option<i64>,
    /// Pixels of a mouse-wheel notch still waiting to be applied to the transcript.
    history_glide: f32,
    /// Same, for the chat list.
    chats_glide: f32,
    /// Scroll offset, in pixels, at the start of this frame. A notch is measured from here,
    /// because the list reapplies the whole wheel from that point on every event.
    history_origin: f32,
    /// Written again after the chat list resets its rows during render.
    chats_origin: Cell<f32>,
    /// Glide still queued at the start of the frame, before this frame's notches.
    history_glide_base: f32,
    chats_glide_base: Cell<f32>,
    /// A frame callback is already scheduled to spend the glide.
    glide_on: bool,
    /// Virtual chat list. Rows differ in height (a pin line, an unread badge).
    chats_list: ListState,
    /// What the chat list was last measured for, so a new message does not
    /// rebuild every row's height.
    chats_stamp: Cell<u64>,
    /// Scroll position of the contact list.
    contacts_scroll: UniformListScrollHandle,
    commands: UnboundedSender<UiCommand>,
    /// Shown instead of a chat while open.
    account: Option<AccountPanel>,
    /// Who is typing where, and since when.
    typing: HashMap<i64, (String, std::time::Instant)>,
    /// The last typing notice we sent: chat and time.
    typing_sent: Option<(i64, std::time::Instant)>,
    /// Reply or edit in the open chat's composer.
    compose: Option<Compose>,
    /// The message row whose right-click menu is open (its element key), kept tinted.
    menu_row: Option<u64>,
    /// The full-screen photo viewer, while open.
    viewer: Option<Viewer>,
    /// The viewer's photo, decoded and turned: (address, quarter turns, pixels).
    viewer_image: Option<(String, u8, Arc<RenderImage>)>,
    /// Keyboard focus of the viewer (Escape, arrows, zoom keys).
    viewer_focus: FocusHandle,
    /// The tray icon and window hiding.
    desktop: desktop::Desktop,
    /// This window, so a tray click can raise it on Linux as well as on Windows.
    window: AnyWindowHandle,
    /// The single-instance endpoint; later starts of the client show this window.
    instance: Option<Instance>,
    /// The window title without the unread count.
    title: SharedString,
    /// The window has the focus.
    window_active: bool,
    /// The unread count the title and tray show now.
    shown_unread: Option<i32>,
    /// The section picked on the navigation rail.
    section: Section,
    /// The open settings page.
    settings_page: SettingsPage,
    /// Theme and font size.
    ui: UiSettings,
    /// People for the contacts section.
    contacts: Vec<ContactRow>,
    /// The contact whose card is shown.
    contact_selected: Option<i64>,
    /// The profile panel is shown beside the open chat.
    profile_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl AppView {
    /// Build the view and start listening to backend events.
    pub fn new(
        mut events: UnboundedReceiver<UiEvent>,
        commands: UnboundedSender<UiCommand>,
        instance: Option<Instance>,
        title: SharedString,
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
        let history_list = ListState::new(0, ListAlignment::Top, px(HISTORY_OVERDRAW));
        history_list.set_follow_mode(FollowMode::Tail);
        // Loading the previous page has to wait until this scroll borrow ends.
        let older = cx.entity().downgrade();
        history_list.set_scroll_handler(move |ev, _, cx| {
            if ev.visible_range.start != 0 {
                return;
            }
            let older = older.clone();
            cx.defer(move |cx| {
                let _ = older.update(cx, |this, cx| {
                    let Some(chat_id) = this.history_for else {
                        return;
                    };
                    this.load_older(chat_id, cx);
                });
            });
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |this, _, ev: &InputEvent, window, cx| match ev {
                    InputEvent::PressEnter { .. } => this.send_message(window, cx),
                    InputEvent::Change => this.composer_changed(cx),
                    _ => {}
                },
            ),
            // "Как в системе" follows the system switching between light and dark.
            cx.observe_window_appearance(window, |this, window, cx| {
                if this.ui.theme == ThemeChoice::System {
                    apply_ui(&this.ui, window, cx);
                }
            }),
            // Notifications are for messages the user is not looking at.
            cx.observe_window_activation(window, |this, window, _| {
                this.window_active = window.is_window_active();
                if this.window_active {
                    desktop::note_shown();
                }
            }),
        ];
        let ui = UiSettings::load();
        apply_ui(&ui, window, cx);
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this.update(cx, |view, cx| view.apply(event, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // The tray and a second start of the client report through channels; look at them a few
        // times a second.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                if this.update(cx, |view, cx| view.poll_desktop(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // Closing the window hides it to the tray while the tray is there and the user wants it.
        let view = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let to_tray = view
                .update(cx, |this, _| {
                    this.ui.close_to_tray && this.desktop.has_tray()
                })
                .unwrap_or(false);
            if to_tray {
                desktop::conceal(window);
                return false;
            }
            cx.quit();
            true
        });
        // A click on a notification shows the window and opens its chat.
        let view = cx.entity().downgrade();
        cx.on_system_notification_response(move |response, cx| {
            let chat_id = response
                .tag
                .strip_prefix(NOTIFY_TAG)
                .and_then(|id| id.parse::<i64>().ok());
            let _ = view.update(cx, |this, cx| {
                this.show_main_window(cx);
                this.section = Section::Chats;
                if let Some(chat_id) = chat_id {
                    this.open_chat(chat_id, cx);
                }
                cx.notify();
            });
        });
        AppView {
            desktop: desktop::Desktop::new(),
            window: window.window_handle(),
            instance,
            title,
            window_active: true,
            shown_unread: None,
            screen: Screen::Status("Запуск…".into()),
            histories: HashMap::new(),
            phone,
            code,
            password,
            composer,
            history_list,
            history_for: None,
            history_glide: 0.,
            chats_glide: 0.,
            history_origin: 0.,
            chats_origin: Cell::new(0.),
            history_glide_base: 0.,
            chats_glide_base: Cell::new(0.),
            glide_on: false,
            chats_list: ListState::new(0, ListAlignment::Top, px(400.)),
            chats_stamp: Cell::new(0),
            contacts_scroll: UniformListScrollHandle::new(),
            images: HashMap::new(),
            link_cards: HashMap::new(),
            shown_sensitive: HashSet::new(),
            lotties: HashMap::new(),
            lottie_clock: false,
            reaction_choices: HashMap::new(),
            reaction_picker: None,
            reaction_people: None,
            emoji_open: false,
            animoji: None,
            animoji_loading: false,
            animoji_note: None,
            animoji_picks: Vec::new(),
            commands,
            account: None,
            typing: HashMap::new(),
            typing_sent: None,
            compose: None,
            menu_row: None,
            viewer: None,
            viewer_image: None,
            viewer_focus: cx.focus_handle(),
            section: Section::Chats,
            settings_page: SettingsPage::Account,
            ui,
            contacts: Vec::new(),
            contact_selected: None,
            profile_open: false,
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
                let links: Vec<String> = h.messages.iter().flat_map(uncovered_max_links).collect();
                let lotties: Vec<String> = h.messages.iter().flat_map(lottie_urls).collect();
                self.request_images(urls);
                self.request_link_previews(links);
                self.request_lotties(lotties);
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
                self.stick_history(chat_id);
                self.mark_read(chat_id);
            }
            UiEvent::Older {
                chat_id,
                messages,
                has_more,
            } => {
                let urls: Vec<String> = messages.iter().flat_map(image_urls).collect();
                let links: Vec<String> = messages.iter().flat_map(uncovered_max_links).collect();
                let lotties: Vec<String> = messages.iter().flat_map(lottie_urls).collect();
                let h = self.histories.entry(chat_id).or_default();
                let added = h.prepend(messages);
                h.has_more = has_more;
                h.loading_older = false;
                if self.history_for == Some(chat_id) && added > 0 {
                    // Keep the row the reader was looking at; the old first row
                    // may lose its day header now that older messages sit above it.
                    self.history_list.splice(0..0, added);
                    if added < self.history_list.item_count() {
                        self.history_list.remeasure_items(added..added + 1);
                    }
                }
                self.request_images(urls);
                self.request_link_previews(links);
                self.request_lotties(lotties);
            }
            UiEvent::HistoryFailed { chat_id, error } => {
                let h = self.histories.entry(chat_id).or_default();
                h.loading = false;
                h.error = Some(error);
            }
            UiEvent::MessageAdded { chat_id, message } => {
                if !message.out {
                    self.typing.remove(&chat_id);
                }
                self.request_images(image_urls(&message));
                let links = uncovered_max_links(&message);
                let lotties = lottie_urls(&message);
                self.request_link_previews(links);
                self.request_lotties(lotties);
                self.notify_message(chat_id, &message, cx);
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
                self.images.insert(url.clone(), state);
                self.remeasure_open_image(&url);
            }
            UiEvent::ImageFailed { url } => {
                self.images.insert(url.clone(), ImageState::Failed);
                self.remeasure_open_image(&url);
            }
            UiEvent::LinkCard { url, card } => {
                if let Some(Attachment::Share {
                    image: Some(image),
                    sensitive: false,
                    ..
                }) = &card
                {
                    self.request_images(vec![image.clone()]);
                }
                self.link_cards.insert(url, card);
                self.remeasure_open_history();
            }
            UiEvent::Reactions {
                chat_id,
                message_id,
                info,
                known_yours,
            } => self.apply_reactions(chat_id, message_id, &info, known_yours),
            UiEvent::ReactionChoices { chat_id, emojis } => {
                self.reaction_choices.insert(chat_id, emojis);
            }
            UiEvent::ReactionPeople { message_id, people } => {
                self.reaction_people = Some((message_id, people));
            }
            UiEvent::Lottie { url, bytes } => {
                let json = bytes.to_vec();
                let url_done = url.clone();
                cx.spawn(async move |this, cx| {
                    let rendered = cx
                        .background_executor()
                        .spawn(async move { max_lottie::render_frames(&json, LOTTIE_PX, None) })
                        .await;
                    let _ = this.update(cx, |view, cx| {
                        view.store_lottie(url_done, rendered);
                        view.ensure_lottie_clock(cx);
                        cx.notify();
                    });
                })
                .detach();
            }
            UiEvent::LottieFailed { url } => {
                self.lotties.insert(url, LottieState::Failed);
            }
            UiEvent::AnimojiCatalog(faces) => {
                let urls = faces.iter().map(|face| face.lottie_url.clone());
                self.request_lotties(urls);
                self.animoji = Some(faces);
                self.animoji_loading = false;
                self.animoji_note = None;
            }
            UiEvent::AnimojiFailed(note) => {
                self.animoji = Some(Vec::new());
                self.animoji_loading = false;
                self.animoji_note = Some(note);
            }
            UiEvent::Sessions(rows) => {
                if let Some(panel) = &mut self.account {
                    panel.sessions = Some(rows);
                    panel.error = None;
                    panel.busy = false;
                }
            }
            UiEvent::SessionsFailed(error) => {
                if let Some(panel) = &mut self.account {
                    panel.error = Some(error);
                    panel.busy = false;
                }
            }
            UiEvent::LoggedOut => {
                self.viewer = None;
                self.account = None;
                self.section = Section::Chats;
                self.settings_page = SettingsPage::Account;
                self.contacts.clear();
                self.contact_selected = None;
                self.profile_open = false;
                self.compose = None;
                self.histories.clear();
                self.history_for = None;
                self.history_list.reset(0);
                self.images.clear();
                self.link_cards.clear();
                self.shown_sensitive.clear();
                self.lotties.clear();
                self.lottie_clock = false;
                self.reaction_choices.clear();
                self.reaction_picker = None;
                self.reaction_people = None;
                self.emoji_open = false;
                self.animoji_picks.clear();
                self.screen = Screen::Status("Вы вышли из аккаунта".into());
            }
            UiEvent::MessageEdited { chat_id, message } => {
                let links = uncovered_max_links(&message);
                let lotties = lottie_urls(&message);
                self.request_link_previews(links);
                self.request_lotties(lotties);
                let message_id = message.id;
                if let Some(h) = self.histories.get_mut(&chat_id) {
                    h.edit(message);
                }
                self.remeasure_id(chat_id, message_id);
                self.refresh_preview(chat_id);
            }
            UiEvent::MessagesDeleted { chat_id, ids } => {
                self.drop_messages(chat_id, |m| m.id == 0 || !ids.contains(&m.id));
                let gone = |id: i64| ids.contains(&id);
                if let Some(Compose::Reply { quote }) = &self.compose
                    && gone(quote.id)
                {
                    self.compose = None;
                }
                self.refresh_preview(chat_id);
            }
            UiEvent::MessagesDeletedRange { chat_id, from, to } => {
                self.drop_messages(chat_id, |m| m.id == 0 || m.time < from || m.time > to);
                self.refresh_preview(chat_id);
            }
            UiEvent::Typing { chat_id, who } => {
                self.typing
                    .insert(chat_id, (who, std::time::Instant::now()));
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(TYPING_SHOWN).await;
                    let _ = this.update(cx, |view, cx| {
                        let stale = view
                            .typing
                            .get(&chat_id)
                            .is_some_and(|(_, at)| at.elapsed() >= TYPING_SHOWN);
                        if stale {
                            view.typing.remove(&chat_id);
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            UiEvent::ReadElsewhere { chat_id, unread } => {
                if let Screen::Main { chats, .. } = &mut self.screen
                    && let Some(row) = chats.iter_mut().find(|c| c.id == chat_id)
                {
                    row.unread = unread;
                }
            }
            UiEvent::PeerRead { chat_id, mark } => {
                if let Screen::Main { chats, .. } = &mut self.screen
                    && let Some(row) = chats.iter_mut().find(|c| c.id == chat_id)
                {
                    row.peer_mark = row.peer_mark.max(mark);
                }
            }
            UiEvent::ChatChanged(new) => {
                if let Screen::Main { chats, .. } = &mut self.screen {
                    upsert_chat(chats, new);
                }
            }
            UiEvent::ChatPref {
                chat_id,
                pin_index,
                muted,
            } => {
                if let Screen::Main { chats, .. } = &mut self.screen
                    && let Some(pos) = chats.iter().position(|c| c.id == chat_id)
                {
                    let mut row = chats.remove(pos);
                    if let Some(pin_index) = pin_index {
                        row.pin_index = pin_index;
                    }
                    if let Some(muted) = muted {
                        row.muted = muted;
                    }
                    place_row(chats, row);
                }
            }
            UiEvent::ChatRemoved(chat_id) => {
                if let Screen::Main {
                    chats, selected, ..
                } = &mut self.screen
                {
                    chats.retain(|c| c.id != chat_id);
                    if *selected == Some(chat_id) {
                        *selected = None;
                        self.profile_open = false;
                    }
                }
                self.histories.remove(&chat_id);
            }
            UiEvent::MarkedUnread { chat_id, unread } => {
                if let Screen::Main { chats, .. } = &mut self.screen
                    && let Some(row) = chats.iter_mut().find(|c| c.id == chat_id)
                {
                    row.unread = unread.max(1);
                }
            }
            UiEvent::Contacts(rows) => {
                self.contacts = rows;
            }
            UiEvent::ChatTitles(titles) => {
                if let Screen::Main { chats, .. } = &mut self.screen {
                    for t in titles {
                        if let Some(row) = chats.iter_mut().find(|c| c.id == t.chat_id) {
                            row.title = t.title;
                            if t.avatar.is_some() {
                                row.avatar = t.avatar;
                            }
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
                if let Some(h) = self.histories.get_mut(&chat_id)
                    && let Some(ix) = h.messages.iter().position(|m| m.cid == Some(cid))
                {
                    h.messages[ix].delivery = Delivery::Failed;
                    h.messages[ix].sender = error;
                    if self.history_for == Some(chat_id) {
                        self.history_list.remeasure_items(ix..ix + 1);
                    }
                }
            }
            other => self.apply_screen(other),
        }
        // New chats, contacts or photo addresses: fetch what is missing (known ones are skipped).
        self.request_avatars();
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
            place_row(chats, row);
        }
        if let Some(h) = self.histories.get_mut(&chat_id) {
            let own = message.out;
            let stored = h.add(message);
            if open {
                self.note_history_change(chat_id, stored, own);
            }
        }
    }

    /// Replace the open transcript and pin it to the newest row.
    fn stick_history(&mut self, chat_id: i64) {
        let open = self.history_for == Some(chat_id)
            || matches!(
                (&self.section, &self.screen),
                (Section::Chats, Screen::Main { selected: Some(id), .. }) if *id == chat_id
            );
        if !open {
            return;
        }
        let count = self
            .histories
            .get(&chat_id)
            .map(|h| h.messages.len())
            .unwrap_or(0);
        self.history_list.reset(count);
        self.history_list.set_follow_mode(FollowMode::Tail);
        self.history_for = Some(chat_id);
    }

    /// Keep the virtual transcript in step with one added or replaced message.
    /// Our own message, and a chat we are already reading at the bottom, stay at the tail.
    fn note_history_change(&mut self, chat_id: i64, stored: Stored, own: bool) {
        if self.history_for != Some(chat_id) {
            self.stick_history(chat_id);
            return;
        }
        let now = self
            .histories
            .get(&chat_id)
            .map(|h| h.messages.len())
            .unwrap_or(0);
        let list_count = self.history_list.item_count();
        match stored {
            Stored::Pushed if list_count + 1 == now => {
                self.history_list.splice(list_count..list_count, 1);
            }
            Stored::Updated(ix) if ix < list_count => {
                self.history_list.remeasure_items(ix..ix + 1);
            }
            _ => self.history_list.reset(now),
        }
        if own || self.history_list.is_following_tail() {
            self.history_list.set_follow_mode(FollowMode::Tail);
        }
    }

    /// Drop rows `keep` rejects, and the same indexes from the open transcript.
    fn drop_messages(&mut self, chat_id: i64, keep: impl Fn(&MessageRow) -> bool) {
        let Some(h) = self.histories.get_mut(&chat_id) else {
            return;
        };
        let removed: Vec<usize> = h
            .messages
            .iter()
            .enumerate()
            .filter_map(|(ix, m)| (!keep(m)).then_some(ix))
            .collect();
        if removed.is_empty() {
            return;
        }
        h.messages.retain(|m| keep(m));
        if self.history_for == Some(chat_id) {
            for ix in removed.into_iter().rev() {
                self.history_list.splice(ix..ix + 1, 0);
            }
        }
    }

    fn remeasure_id(&self, chat_id: i64, message_id: i64) {
        if self.history_for != Some(chat_id) {
            return;
        }
        let Some(ix) = self
            .histories
            .get(&chat_id)
            .and_then(|h| h.messages.iter().position(|m| m.id == message_id))
        else {
            return;
        };
        self.history_list.remeasure_items(ix..ix + 1);
    }

    /// A loaded photo changes the height of the bubble that shows it.
    fn remeasure_open_image(&self, url: &str) {
        let Some(chat_id) = self.history_for else {
            return;
        };
        let Some(history) = self.histories.get(&chat_id) else {
            return;
        };
        for (ix, message) in history.messages.iter().enumerate() {
            let hit = image_urls(message).iter().any(|u| u == url)
                || message.attachments.iter().any(
                    |a| matches!(a, Attachment::Share { image: Some(image), .. } if image == url),
                );
            if hit {
                self.history_list.remeasure_items(ix..ix + 1);
            }
        }
    }

    fn remeasure_open_history(&self) {
        if self.history_for.is_some() {
            self.history_list.remeasure();
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

    /// Ask about `max.ru` links we have not asked about yet. A failure stays empty and is not retried.
    fn request_link_previews(&mut self, urls: impl IntoIterator<Item = String>) {
        for url in urls {
            if self.link_cards.contains_key(&url) {
                continue;
            }
            self.link_cards.insert(url.clone(), None);
            let _ = self.commands.send(UiCommand::LinkInfo(url));
        }
    }

    fn request_lotties(&mut self, urls: impl IntoIterator<Item = String>) {
        let urls: Vec<String> = urls
            .into_iter()
            .filter(|url| !self.lotties.contains_key(url))
            .collect();
        for url in urls {
            self.lotties.insert(url.clone(), LottieState::Loading);
            let _ = self.commands.send(UiCommand::FetchLottie(url));
        }
    }

    /// Keep a fully rasterized emoji. A file ThorVG cannot draw stays the plain character.
    fn store_lottie(&mut self, url: String, rendered: Option<Vec<Vec<u8>>>) {
        let Some(rendered) = rendered.filter(|frames| !frames.is_empty()) else {
            self.lotties.insert(url, LottieState::Failed);
            return;
        };
        let mut cache = Vec::with_capacity(rendered.len());
        for rgba in rendered {
            let Some(buffer) = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(
                LOTTIE_PX, LOTTIE_PX, rgba,
            ) else {
                self.lotties.insert(url, LottieState::Failed);
                return;
            };
            cache.push(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])));
        }
        let frames = u32::try_from(cache.len()).unwrap_or(u32::MAX);
        self.lotties.insert(
            url,
            LottieState::Ready(LottieClip {
                frames,
                index: 0,
                cache,
            }),
        );
    }

    fn ensure_lottie_clock(&mut self, cx: &mut Context<Self>) {
        if self.lottie_clock {
            return;
        }
        let playing = self
            .lotties
            .values()
            .any(|state| matches!(state, LottieState::Ready(clip) if clip.frames > 1));
        if !playing {
            return;
        }
        self.lottie_clock = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let keep = this.update(cx, |view, cx| {
                    let mut playing = false;
                    for state in view.lotties.values_mut() {
                        if let LottieState::Ready(clip) = state
                            && clip.frames > 1
                        {
                            clip.index = (clip.index + 1) % clip.frames;
                            playing = true;
                        }
                    }
                    // A frame that is not on screen does not need a new draw.
                    // Scrolling stays on the list's own path instead of rebuilding the window.
                    if playing && view.lottie_visible() {
                        cx.notify();
                    }
                    playing
                });
                if !keep.unwrap_or(false) {
                    break;
                }
            }
            let _ = this.update(cx, |view, _| view.lottie_clock = false);
        })
        .detach();
    }

    /// Whether a playing emoji is actually on screen. The clock still advances hidden
    /// frames, but it does not ask the window to draw them.
    fn lottie_visible(&self) -> bool {
        if !self.window_active || self.viewer.is_some() {
            return false;
        }
        if self.emoji_open
            && self.animoji.as_ref().is_some_and(|faces| {
                faces
                    .iter()
                    .any(|face| self.lottie_playing(&face.lottie_url))
            })
        {
            return true;
        }
        let Some(chat_id) = self.history_for else {
            return false;
        };
        let Some(history) = self.histories.get(&chat_id) else {
            return false;
        };
        let count = history.messages.len();
        if count == 0 {
            return false;
        }
        let top = self.history_list.logical_scroll_top().item_ix;
        let start = if top >= count {
            count.saturating_sub(16)
        } else {
            top.saturating_sub(2)
        };
        let end = (start + 24).min(count);
        history.messages[start..end]
            .iter()
            .any(|m| lottie_urls(m).iter().any(|url| self.lottie_playing(url)))
    }

    fn lottie_playing(&self, url: &str) -> bool {
        matches!(
            self.lotties.get(url),
            Some(LottieState::Ready(clip)) if clip.frames > 1
        )
    }

    /// The list already jumped by the whole notch. Keep one frame of it and
    /// play the rest on the following frames, one slice per refresh.
    fn glide_history(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.glide_wheel(ev, true, window, cx);
    }

    fn glide_chats(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.glide_wheel(ev, false, window, cx);
    }

    fn glide_wheel(
        &mut self,
        ev: &ScrollEvent,
        history: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A trackpad already sends a small delta on each frame.
        if ev.delta.precise() {
            return;
        }
        let list = if history {
            self.history_list.clone()
        } else {
            self.chats_list.clone()
        };
        if list.item_count() == 0 {
            return;
        }
        let origin = if history {
            self.history_origin
        } else {
            self.chats_origin.get()
        };
        let base = if history {
            self.history_glide_base
        } else {
            self.chats_glide_base.get()
        };
        // The list already moved. Measure that, and keep only one frame of it.
        // A second notch in the same frame is measured from the same origin.
        let moved = list_scroll_px(&list) - origin;
        let extra = wheel_glide_extra(-moved, WHEEL_FRAME_PX);
        if history {
            self.history_glide = base + extra;
        } else {
            self.chats_glide = base + extra;
        }
        if extra == 0. {
            return;
        }
        list.scroll_by(px(-extra));
        self.kick_glide(window, cx);
    }

    fn step_glide(&mut self, history: bool) -> bool {
        let remaining = if history {
            self.history_glide
        } else {
            self.chats_glide
        };
        let step = glide_step(remaining);
        if step == 0. {
            if history {
                self.history_glide = 0.;
            } else {
                self.chats_glide = 0.;
            }
            return false;
        }
        let list = if history {
            self.history_list.clone()
        } else {
            self.chats_list.clone()
        };
        list.scroll_by(px(step));
        let left = remaining - step;
        let left = if left.abs() < 0.5 { 0. } else { left };
        if history {
            self.history_glide = left;
        } else {
            self.chats_glide = left;
        }
        true
    }

    fn kick_glide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.glide_on {
            return;
        }
        self.glide_on = true;
        cx.on_next_frame(window, |this, window, cx| this.pump_glide(window, cx));
    }

    /// Spend a slice of the queued wheel distance. One call is one displayed frame.
    fn pump_glide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let moved = self.step_glide(true) || self.step_glide(false);
        if self.history_glide != 0. || self.chats_glide != 0. {
            cx.on_next_frame(window, |this, window, cx| this.pump_glide(window, cx));
        } else {
            self.glide_on = false;
        }
        if moved {
            cx.notify();
        }
    }

    fn apply_reactions(
        &mut self,
        chat_id: i64,
        message_id: i64,
        info: &messages::ReactionInfo,
        known_yours: bool,
    ) {
        let found = {
            let Some(row) = self
                .histories
                .get_mut(&chat_id)
                .and_then(|h| h.messages.iter_mut().find(|m| m.id == message_id))
            else {
                return;
            };
            let kept = row
                .reactions
                .iter()
                .find(|r| r.mine)
                .map(|r| r.emoji.clone());
            let yours = if known_yours {
                info.your_reaction.as_deref()
            } else {
                kept.as_deref()
            };
            row.reactions = reaction_counts(info, yours);
            true
        };
        if found {
            self.remeasure_id(chat_id, message_id);
        }
    }

    /// The plate we show for a chat: the server's list, or the usual emoji.
    fn reaction_palette(&self, chat_id: i64) -> Vec<String> {
        self.reaction_choices
            .get(&chat_id)
            .cloned()
            .unwrap_or_else(|| {
                ["👍", "❤️", "🔥", "😂", "😢", "😮", "🙏"]
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            })
    }

    fn toggle_reaction(&mut self, chat_id: i64, message_id: i64, emoji: &str) {
        let Some(row) = self
            .histories
            .get_mut(&chat_id)
            .and_then(|h| h.messages.iter_mut().find(|m| m.id == message_id))
        else {
            return;
        };
        let removing = row.reactions.iter().any(|r| r.mine && r.emoji == emoji);
        if let Some(prev) = row.reactions.iter_mut().find(|r| r.mine) {
            prev.mine = false;
            prev.count -= 1;
        }
        row.reactions.retain(|r| r.count > 0);
        if !removing {
            if let Some(chip) = row.reactions.iter_mut().find(|r| r.emoji == emoji) {
                chip.count += 1;
                chip.mine = true;
            } else {
                row.reactions.push(ReactionCount {
                    emoji: emoji.to_string(),
                    count: 1,
                    mine: true,
                });
            }
        }
        let command = if removing {
            UiCommand::CancelReaction {
                chat_id,
                message_id,
            }
        } else {
            UiCommand::React {
                chat_id,
                message_id,
                emoji: emoji.to_string(),
            }
        };
        let _ = self.commands.send(command);
        self.reaction_picker = None;
        self.remeasure_id(chat_id, message_id);
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
            ..MessageRow::default()
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
        if *selected != Some(chat_id) {
            self.compose = None;
            self.emoji_open = false;
        }
        *selected = Some(chat_id);
        self.section = Section::Chats;
        if let Some(row) = chats.iter_mut().find(|c| c.id == chat_id) {
            row.unread = 0;
        }
        let h = self.histories.entry(chat_id).or_default();
        if h.messages.is_empty() && !h.loading {
            h.loading = true;
            h.error = None;
            let _ = self.commands.send(UiCommand::OpenChat(chat_id));
        }
        let count = self
            .histories
            .get(&chat_id)
            .map(|h| h.messages.len())
            .unwrap_or(0);
        if self.history_for != Some(chat_id) {
            self.history_list.reset(count);
            self.history_for = Some(chat_id);
        }
        self.history_list.set_follow_mode(FollowMode::Tail);
        self.mark_read(chat_id);
        cx.notify();
    }

    /// Show the newest loaded message as the chat's preview (after an edit or a delete).
    fn refresh_preview(&mut self, chat_id: i64) {
        let Some(last) = self
            .histories
            .get(&chat_id)
            .and_then(|h| h.messages.last())
            .map(MessageRow::preview)
        else {
            return;
        };
        if let Screen::Main { chats, .. } = &mut self.screen
            && let Some(row) = chats.iter_mut().find(|c| c.id == chat_id)
        {
            row.preview = last;
        }
    }

    /// Tell the open chat we are typing, at most once per [`TYPING_EVERY`].
    fn composer_changed(&mut self, cx: &mut Context<Self>) {
        let Screen::Main {
            selected: Some(chat_id),
            ..
        } = self.screen
        else {
            return;
        };
        let text = self.composer.read(cx).value().to_string();
        self.animoji_picks = max_core::assets::claim_animoji(&text, &self.animoji_picks);
        if text.trim().is_empty() {
            return;
        }
        let now = std::time::Instant::now();
        let recent = self
            .typing_sent
            .is_some_and(|(id, at)| id == chat_id && now.duration_since(at) < TYPING_EVERY);
        if !recent && self.commands.send(UiCommand::Typing { chat_id }).is_ok() {
            self.typing_sent = Some((chat_id, now));
        }
    }

    /// "печатает…" for a chat, with the name in group chats.
    fn typing_label(&self, chat: &ChatRow) -> Option<String> {
        let (who, _) = self.typing.get(&chat.id)?;
        Some(if who.is_empty() || *who == chat.title {
            "печатает…".to_string()
        } else {
            format!("{who} печатает…")
        })
    }

    /// Open the account panel and ask for the sessions.
    fn open_account(&mut self, cx: &mut Context<Self>) {
        if self.commands.send(UiCommand::LoadSessions).is_ok() {
            self.account = Some(AccountPanel::default());
            cx.notify();
        }
    }

    /// Switch the rail section; the account page loads the sessions each time it is shown.
    fn switch_section(&mut self, section: Section, cx: &mut Context<Self>) {
        self.section = section;
        if section == Section::Settings && self.settings_page == SettingsPage::Account {
            self.open_account(cx);
        }
        cx.notify();
    }

    fn open_settings_page(&mut self, page: SettingsPage, cx: &mut Context<Self>) {
        self.settings_page = page;
        if page == SettingsPage::Account {
            self.open_account(cx);
        }
        cx.notify();
    }

    /// Change and save the interface settings.
    fn set_ui(&mut self, ui: UiSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.ui = ui;
        apply_ui(&self.ui, window, cx);
        self.history_list.remeasure();
        self.chats_list.remeasure();
        if let Err(e) = self.ui.save()
            && let Screen::Main { status, .. } = &mut self.screen
        {
            *status = Some(format!("Настройки не сохранились: {e}"));
        }
        cx.notify();
    }

    /// First click asks for confirmation, the second one acts.
    fn account_action(&mut self, action: AccountAction, cx: &mut Context<Self>) {
        let Some(panel) = &mut self.account else {
            return;
        };
        if panel.confirm != Some(action) {
            panel.confirm = Some(action);
        } else {
            let command = match action {
                AccountAction::CloseOthers => UiCommand::CloseOtherSessions,
                AccountAction::Logout => UiCommand::Logout,
            };
            if self.commands.send(command).is_ok() {
                panel.confirm = None;
                panel.busy = true;
                panel.error = None;
            }
        }
        cx.notify();
    }

    fn render_account(&self, me: &str, panel: &AccountPanel, cx: &mut Context<Self>) -> Div {
        let muted = cx.theme().muted_foreground;
        let others = panel
            .sessions
            .as_ref()
            .is_some_and(|list| list.iter().any(|s| !s.current));
        let list: Vec<AnyElement> = match &panel.sessions {
            None => vec![
                div()
                    .text_sm()
                    .text_color(muted)
                    .child("Загрузка…")
                    .into_any_element(),
            ],
            Some(list) => list
                .iter()
                .map(|s| {
                    let details = [s.info.as_str(), s.location.as_str()]
                        .into_iter()
                        .filter(|t| !t.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    v_flex()
                        .py_2()
                        .gap_0p5()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().font_semibold().child(s.client.clone()))
                                .when(s.current, |this| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().success)
                                            .child("этот клиент"),
                                    )
                                }),
                        )
                        .child(div().text_sm().text_color(muted).child(details))
                        .when(!s.current && s.time > 0, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(format!("Активность: {}", short_time(s.time))),
                            )
                        })
                        .into_any_element()
                })
                .collect(),
        };
        let confirm_text = match panel.confirm {
            Some(AccountAction::CloseOthers) => {
                Some("Завершить все другие сеансы? На тех устройствах придётся войти заново.")
            }
            Some(AccountAction::Logout) => Some(
                "Выйти из аккаунта на этом компьютере? Для входа понадобится QR-код или код из SMS.",
            ),
            None => None,
        };
        let busy = panel.busy;
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
                    .child("Аккаунт и устройства"),
            )
            .child(
                v_flex()
                    .id("account")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_6()
                    .py_4()
                    .gap_3()
                    .max_w(px(640.))
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted)
                            .child(format!("Вы вошли как {me}")),
                    )
                    .child(div().font_semibold().child("Активные сеансы"))
                    .child(v_flex().children(list))
                    .when_some(panel.error.clone(), |this, e| {
                        this.child(div().text_sm().text_color(cx.theme().danger).child(e))
                    })
                    .when_some(confirm_text, |this, text| {
                        this.child(div().text_sm().text_color(cx.theme().warning).child(text))
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .pt_2()
                            .child(
                                Button::new("close-others")
                                    .label(if panel.confirm == Some(AccountAction::CloseOthers) {
                                        "Да, завершить"
                                    } else {
                                        "Завершить другие сеансы"
                                    })
                                    .warning()
                                    .disabled(busy || !others)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.account_action(AccountAction::CloseOthers, cx)
                                    })),
                            )
                            .child(
                                Button::new("logout")
                                    .label(if panel.confirm == Some(AccountAction::Logout) {
                                        "Да, выйти"
                                    } else {
                                        "Выйти"
                                    })
                                    .danger()
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.account_action(AccountAction::Logout, cx)
                                    })),
                            )
                            .when(panel.confirm.is_some(), |this| {
                                this.child(
                                    Button::new("account-cancel")
                                        .ghost()
                                        .label("Отмена")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            if let Some(panel) = &mut this.account {
                                                panel.confirm = None;
                                            }
                                            cx.notify();
                                        })),
                                )
                            }),
                    ),
            )
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
        let elements = max_core::assets::animoji_elements(&text, &self.animoji_picks);
        if let Some(Compose::Edit {
            message_id,
            original,
        }) = &self.compose
        {
            let (message_id, unchanged) = (*message_id, *original == text);
            if unchanged
                || self
                    .commands
                    .send(UiCommand::EditText {
                        chat_id,
                        message_id,
                        text,
                    })
                    .is_ok()
            {
                self.cancel_compose(window, cx);
            }
            return;
        }
        let quote = match &self.compose {
            Some(Compose::Reply { quote }) => Some(quote.clone()),
            _ => None,
        };
        let cid = jiff::Timestamp::now().as_millisecond();
        let pending = MessageRow {
            cid: Some(cid),
            out: true,
            text: text.clone(),
            time: cid,
            delivery: Delivery::Pending,
            reply: quote.clone(),
            marks: rich::autolinks(&text, &element_marks(&text, &elements)),
            ..MessageRow::default()
        };
        if self
            .commands
            .send(UiCommand::SendText {
                chat_id,
                text,
                cid,
                reply_to: quote.map(|q| q.id),
                elements,
            })
            .is_ok()
        {
            self.compose = None;
            self.animoji_picks.clear();
            self.message_added(chat_id, pending);
            self.composer
                .update(cx, |input, cx| input.set_value("", window, cx));
            cx.notify();
        }
    }

    /// Answer `m` with the next message.
    fn start_reply(
        &mut self,
        m: &MessageRow,
        chat_title: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.compose, Some(Compose::Edit { .. })) {
            self.composer
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.menu_row = None;
        self.compose = Some(Compose::Reply {
            quote: quote_of(m, chat_title),
        });
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Put our message's text into the composer for editing.
    fn start_edit(&mut self, m: &MessageRow, window: &mut Window, cx: &mut Context<Self>) {
        let text = m.text.clone();
        self.composer
            .update(cx, |input, cx| input.set_value(text.clone(), window, cx));
        self.menu_row = None;
        self.compose = Some(Compose::Edit {
            message_id: m.id,
            original: text,
        });
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Drop the reply or edit; an edit also takes its text out of the composer.
    fn cancel_compose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.compose.take(), Some(Compose::Edit { .. })) {
            self.composer
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        cx.notify();
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.composer.read(cx).focus_handle(cx);
        handle.focus(window, cx);
    }

    /// Delete a message after the choice under it.
    fn delete_message(&mut self, chat_id: i64, id: i64, for_me: bool, cx: &mut Context<Self>) {
        if self
            .commands
            .send(UiCommand::Delete {
                chat_id,
                ids: vec![id],
                for_me,
            })
            .is_ok()
        {
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

    fn chat_row(
        row: &ChatRow,
        selected: bool,
        typing: Option<String>,
        avatar: AnyElement,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = row.id;
        h_flex()
            .id(("chat", u64::from_ne_bytes(id.to_ne_bytes())))
            .gap_3()
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .when(selected, |this| this.bg(cx.theme().accent))
            .hover(|this| this.bg(cx.theme().secondary_hover))
            .on_click(cx.listener(move |this, _, _, cx| this.open_chat(id, cx)))
            .child(avatar)
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
                                v_flex()
                                    .items_end()
                                    .flex_shrink_0()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(short_time(row.time)),
                                    )
                                    .when(row.pin_index > 0 || row.muted, |this| {
                                        let mark = match (row.pin_index > 0, row.muted) {
                                            (true, true) => "закреп · тихо",
                                            (true, false) => "закреп",
                                            _ => "тихо",
                                        };
                                        this.child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(mark),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(if typing.is_some() {
                                        cx.theme().primary
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .truncate()
                                    .child(typing.clone().unwrap_or_else(|| row.preview.clone())),
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

    /// The right-click menu of a chat: pin, mute, mark unread, leave or delete.
    fn chat_menu(
        &self,
        row: &ChatRow,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let view = cx.entity().downgrade();
        let chat_id = row.id;
        let pinned = row.pin_index > 0;
        let muted = row.muted;
        let unread = row.unread;
        let kind = row.kind;
        let message_id = self.histories.get(&chat_id).and_then(|h| {
            h.messages
                .iter()
                .rev()
                .find(|m| m.id != 0 && m.delivery == Delivery::Sent)
                .map(|m| m.id)
        });
        move |menu, _, _| {
            let view_pin = view.clone();
            let view_mute = view.clone();
            let mut menu = menu
                .item(
                    PopupMenuItem::new(if pinned {
                        "Открепить"
                    } else {
                        "Закрепить"
                    })
                    .on_click(move |_, _, cx| {
                        let _ = view_pin.update(cx, |this, cx| {
                            this.send(
                                UiCommand::SetPinned {
                                    chat_id,
                                    pinned: !pinned,
                                },
                                cx,
                            );
                        });
                    }),
                )
                .item(
                    PopupMenuItem::new(if muted {
                        "Включить звук"
                    } else {
                        "Выключить звук"
                    })
                    .on_click(move |_, _, cx| {
                        let _ = view_mute.update(cx, |this, cx| {
                            this.send(
                                UiCommand::SetMuted {
                                    chat_id,
                                    muted: !muted,
                                },
                                cx,
                            );
                        });
                    }),
                );
            if unread == 0 {
                let view_unread = view.clone();
                menu = menu.item(PopupMenuItem::new("Пометить непрочитанным").on_click(
                    move |_, _, cx| {
                        let _ = view_unread.update(cx, |this, cx| {
                            this.send(
                                UiCommand::MarkUnread {
                                    chat_id,
                                    message_id,
                                },
                                cx,
                            );
                        });
                    },
                ));
            }
            if !matches!(kind, ChatKind::Saved) {
                let view_remove = view.clone();
                let leave = matches!(kind, ChatKind::Group | ChatKind::Channel);
                menu = menu.separator().item(
                    PopupMenuItem::new(if leave {
                        "Выйти"
                    } else {
                        "Удалить чат"
                    })
                    .on_click(move |_, window, cx| {
                        let _ = view_remove.update(cx, |this, cx| {
                            this.ask_remove(chat_id, leave, window, cx);
                        });
                    }),
                );
            }
            menu
        }
    }

    /// Confirm leaving a group or channel, or deleting a dialog for ourselves.
    fn ask_remove(
        &mut self,
        chat_id: i64,
        leave: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let confirm = {
                let view = view.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    let _ = view.update(cx, |this, cx| {
                        this.send(
                            if leave {
                                UiCommand::LeaveChat(chat_id)
                            } else {
                                UiCommand::DeleteChat(chat_id)
                            },
                            cx,
                        );
                    });
                    window.close_dialog(cx);
                }
            };
            dialog
                .w(px(440.))
                .title(if leave {
                    "Выйти из чата?"
                } else {
                    "Удалить чат?"
                })
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(if leave {
                            "Чат пропадёт из списка. У остальных участников он останется."
                        } else {
                            "Чат пропадёт только у вас."
                        }),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("chat-remove-cancel")
                                .ghost()
                                .label("Отмена")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("chat-remove-confirm")
                                .danger()
                                .label(if leave {
                                    "Выйти"
                                } else {
                                    "Удалить"
                                })
                                .on_click(confirm),
                        ),
                )
        });
    }

    /// Link cards under the text: `SHARE` attachments, then a cached `LINK_INFO` card for a
    /// `max.ru` link that has none.
    fn share_cards(&self, m: &MessageRow) -> Vec<Attachment> {
        let mut cards: Vec<Attachment> = m
            .attachments
            .iter()
            .filter(|a| matches!(a, Attachment::Share { .. }))
            .cloned()
            .collect();
        for url in uncovered_max_links(m) {
            if let Some(Some(card)) = self.link_cards.get(&url) {
                cards.push(card.clone());
            }
        }
        cards
    }

    /// One link card. A left click opens the address the way a link in the text does.
    fn share_card(
        &self,
        message_key: u64,
        index: usize,
        card: &Attachment,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Attachment::Share {
            url,
            title,
            description,
            host,
            image,
            sensitive,
        } = card
        else {
            return div().into_any_element();
        };
        let shown_host = if host.is_empty() {
            rich::host_of(url)
        } else {
            host.clone()
        };
        let heading = if title.is_empty() {
            shown_host.clone()
        } else {
            title.clone()
        };
        let revealed = image
            .as_ref()
            .is_some_and(|u| self.shown_sensitive.contains(u));
        let show_image = image.as_ref().is_some_and(|_| !*sensitive || revealed);
        let link = rich::Link {
            range: 0..0,
            url: url.clone(),
            hidden: !rich::same_host(&shown_host, url),
        };
        let muted = cx.theme().foreground.opacity(0.65);
        let id = message_key.wrapping_add(index as u64);
        let open = link.clone();
        v_flex()
            .w_full()
            .max_w(px(320.))
            .gap_1()
            .rounded(px(8.))
            .overflow_hidden()
            .bg(cx.theme().foreground.opacity(0.08))
            .when(show_image, |this| {
                let url = image.clone().unwrap_or_default();
                let link = link.clone();
                this.child(
                    div()
                        .id(("share-img", id))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_link(&link, window, cx);
                        }))
                        .child(self.share_image(&url, cx)),
                )
            })
            .when(*sensitive && !revealed && image.is_some(), |this| {
                let url = image.clone().unwrap_or_default();
                this.child(
                    v_flex()
                        .px_2()
                        .pt_2()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child("Изображение скрыто"),
                        )
                        .child(
                            Button::new(("share-show", id))
                                .xsmall()
                                .ghost()
                                .label("Показать")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.shown_sensitive.insert(url.clone());
                                    this.request_images(vec![url.clone()]);
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                v_flex()
                    .id(("share", id))
                    .px_2()
                    .py_2()
                    .gap_1()
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_link(&open, window, cx);
                    }))
                    .child(div().text_sm().font_medium().truncate().child(heading))
                    .when(!description.is_empty(), |this| {
                        this.child(div().text_xs().truncate().child(description.clone()))
                    })
                    .when(!shown_host.is_empty(), |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .truncate()
                                .child(shown_host),
                        )
                    }),
            )
            .into_any_element()
    }

    /// The picture on a link card, or a placeholder while it loads.
    fn share_image(&self, url: &str, cx: &mut Context<Self>) -> AnyElement {
        match self.images.get(url) {
            Some(ImageState::Ready(image)) => img(image.clone())
                .w_full()
                .h(px(140.))
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            state => div()
                .w_full()
                .h(px(140.))
                .flex()
                .items_center()
                .justify_center()
                .bg(cx.theme().foreground.opacity(0.06))
                .text_xs()
                .text_color(cx.theme().foreground.opacity(0.65))
                .child(match state {
                    Some(ImageState::Failed) => "Изображение не загрузилось",
                    _ => "Загрузка…",
                })
                .into_any_element(),
        }
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

    /// Put the unread count into the window title and the tray when it changes.
    fn sync_unread(&mut self, window: &mut Window) {
        let total: i32 = match &self.screen {
            Screen::Main { chats, .. } => chats.iter().map(|c| c.unread.max(0)).sum(),
            _ => 0,
        };
        if self.shown_unread == Some(total) {
            return;
        }
        self.shown_unread = Some(total);
        let title = if total > 0 {
            format!("({total}) {}", self.title)
        } else {
            self.title.to_string()
        };
        window.set_window_title(&title);
        self.desktop.set_unread(total);
    }

    /// A message's text with its formatting, mentions and clickable links.
    /// Chips under a message, and the plate of emoji to add one.
    fn reaction_row(
        &self,
        chat_id: i64,
        m: &MessageRow,
        key: u64,
        who: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.reaction_picker == Some((chat_id, m.id));
        let people = self
            .reaction_people
            .as_ref()
            .filter(|(id, _)| *id == m.id)
            .map(|(_, lines)| lines.clone());
        let message_id = m.id;
        v_flex()
            .w_full()
            .gap_1()
            .when(open, |this| {
                let palette = self.reaction_palette(chat_id);
                this.child(h_flex().w_full().flex_wrap().gap_1().children(
                    palette.into_iter().enumerate().map(|(i, emoji)| {
                        let chosen = emoji.clone();
                        Button::new(("plate", key.wrapping_add(i as u64)))
                            .xsmall()
                            .ghost()
                            .label(emoji)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_reaction(chat_id, message_id, &chosen);
                                cx.notify();
                            }))
                    }),
                ))
            })
            .child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .children(m.reactions.iter().enumerate().map(|(i, chip)| {
                        let emoji = chip.emoji.clone();
                        let label = format!("{} {}", chip.emoji, chip.count);
                        let mine = chip.mine;
                        Button::new(("chip", key.wrapping_add(20 + i as u64)))
                            .xsmall()
                            .when(mine, |this| this.selected(true))
                            .ghost()
                            .label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_reaction(chat_id, message_id, &emoji);
                                cx.notify();
                            }))
                    }))
                    .when(who && !m.reactions.is_empty(), |this| {
                        this.child(
                            Button::new(("who", key))
                                .xsmall()
                                .ghost()
                                .label("кто")
                                .on_click(cx.listener(move |this, _, _, _| {
                                    let _ = this.commands.send(UiCommand::ReactionDetails {
                                        chat_id,
                                        message_id,
                                    });
                                })),
                        )
                    })
                    .child(
                        Button::new(("add-react", key))
                            .xsmall()
                            .ghost()
                            .label(if open { "×" } else { "+" })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.reaction_picker = if open {
                                    None
                                } else {
                                    Some((chat_id, message_id))
                                };
                                cx.notify();
                            })),
                    ),
            )
            .when_some(people, |this, lines| {
                this.child(v_flex().children(lines.into_iter().map(|(who, emoji)| {
                    div()
                        .text_xs()
                        .child(format!("{who} {emoji}"))
                        .into_any_element()
                })))
            })
            .into_any_element()
    }

    fn rich_text(&self, m: &MessageRow, key: u64, cx: &mut Context<Self>) -> AnyElement {
        if m.marks
            .iter()
            .any(|mark| matches!(mark.kind, rich::MarkKind::Animoji { .. }))
        {
            return self.with_animoji(m, key, cx);
        }
        self.styled_text(&m.text, &m.marks, m.out, key, cx)
    }

    /// The message text with an animated emoji drawn in place of its characters.
    fn with_animoji(&self, m: &MessageRow, key: u64, cx: &mut Context<Self>) -> AnyElement {
        let mut row = h_flex().flex_wrap().items_end().gap_1();
        for (i, part) in rich::pieces(m.text.len(), &m.marks).into_iter().enumerate() {
            match part {
                rich::Piece::Text(range) if !m.text[range.clone()].is_empty() => {
                    let text = m.text[range.clone()].to_string();
                    let marks = rich::marks_in(&m.marks, range);
                    row = row.child(self.styled_text(
                        &text,
                        &marks,
                        m.out,
                        key.wrapping_add(i as u64 + 1),
                        cx,
                    ));
                }
                rich::Piece::Animoji { range, url } => {
                    let fallback = m.text[range].to_string();
                    row = row.child(self.animoji_view(&url, &fallback));
                }
                rich::Piece::Text(_) => {}
            }
        }
        row.into_any_element()
    }

    fn animoji_view(&self, url: &str, fallback: &str) -> AnyElement {
        let frame = self.lotties.get(url).and_then(|state| match state {
            LottieState::Ready(clip) => clip.cache.get(clip.index as usize).cloned(),
            _ => None,
        });
        match frame {
            Some(image) => img(image).w(px(28.)).h(px(28.)).into_any_element(),
            None => div()
                .text_sm()
                .child(fallback.to_string())
                .into_any_element(),
        }
    }

    fn styled_text(
        &self,
        text: &str,
        marks: &[rich::Mark],
        out: bool,
        key: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if marks.is_empty() {
            return div().child(text.to_string()).into_any_element();
        }
        // On our own bubbles links keep the text colour and are underlined.
        // The neutral themes draw links in the text colour; messengers use blue.
        let (_, fg, _) = Self::bubble_colors(out, cx);
        let link_color = if out {
            fg
        } else if cx.theme().is_dark() {
            rgb(0x6c_b4_ff).into()
        } else {
            rgb(0x1a_6f_d6).into()
        };
        let mono_bg = if out {
            fg.opacity(0.15)
        } else {
            cx.theme().foreground.opacity(0.08)
        };
        let runs = rich::runs(text.len(), marks);
        let highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = runs
            .iter()
            .map(|(range, f)| {
                let underline = f.underline || f.link;
                (
                    range.clone(),
                    HighlightStyle {
                        font_weight: f.bold.then_some(FontWeight::BOLD),
                        font_style: (f.italic || f.quote).then_some(FontStyle::Italic),
                        underline: underline.then_some(UnderlineStyle {
                            thickness: px(1.),
                            ..UnderlineStyle::default()
                        }),
                        strikethrough: f.strike.then_some(StrikethroughStyle {
                            thickness: px(1.),
                            ..StrikethroughStyle::default()
                        }),
                        color: (f.link || f.mention).then_some(link_color),
                        background_color: f.mono.then_some(mono_bg),
                        fade_out: f.quote.then_some(0.2),
                    },
                )
            })
            .collect();
        let mono = cx.theme().mono_font_family.clone();
        let fonts: Vec<(std::ops::Range<usize>, SharedString)> = runs
            .iter()
            .filter(|(_, f)| f.mono)
            .map(|(range, _)| (range.clone(), mono.clone()))
            .collect();
        let styled = StyledText::new(text.to_string())
            .with_highlights(highlights)
            .with_font_family_overrides(fonts);
        let links = rich::links(marks);
        if links.is_empty() {
            return styled.into_any_element();
        }
        let ranges = links.iter().map(|l| l.range.clone()).collect();
        let view = cx.entity().downgrade();
        // InteractiveText treats any mouse button as a click. A right click is the message
        // menu, so it must not open the link. Capture runs before the text records the press.
        let right_click = Rc::new(Cell::new(false));
        let pressed_right = right_click.clone();
        let hover_links = links.clone();
        let text = InteractiveText::new(("text", key), styled)
            .on_click(ranges, move |ix, window, cx| {
                if right_click.get() {
                    return;
                }
                if let Some(link) = links.get(ix) {
                    let _ = view.update(cx, |this, cx| this.open_link(link, window, cx));
                }
            })
            .tooltip(move |ix, window, cx| {
                let link = hover_links.iter().find(|l| l.range.contains(&ix))?;
                if rich::opens_directly(&link.url, link.hidden) {
                    return None;
                }
                Some(Tooltip::new(link.url.clone()).build(window, cx))
            });
        div()
            .id(("links", key))
            .capture_any_mouse_down(move |event, _, _| {
                pressed_right.set(event.button != MouseButton::Left);
            })
            .child(text)
            .into_any_element()
    }

    /// Open a link in the browser. A link whose text hides where it leads, one with a login in
    /// front of the host, or one the client does not hand to the system, is shown in a dialog
    /// first.
    fn open_link(&mut self, link: &rich::Link, window: &mut Window, cx: &mut Context<Self>) {
        let url = link.url.trim().to_string();
        let web = rich::opens_externally(&url);
        let userinfo = rich::has_userinfo(&url);
        if rich::opens_directly(&url, link.hidden) {
            cx.open_url(&url);
            return;
        }
        let notice = if !web {
            "Клиент открывает только веб-страницы и почту. Адрес можно скопировать:"
        } else if userinfo {
            "В адресе указан логин перед сайтом. Проверьте, куда она ведёт:"
        } else {
            "Текст ссылки не совпадает с адресом. Проверьте, куда она ведёт:"
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            let (open, copy) = (url.clone(), url.clone());
            dialog
                .w(px(480.))
                .title(if web {
                    "Открыть ссылку?"
                } else {
                    "Эта ссылка не открывается"
                })
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(notice),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_sm()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(url.clone()),
                        ),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("link-cancel")
                                .ghost()
                                .label("Отмена")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("link-copy")
                                .outline()
                                .label("Копировать")
                                .on_click(move |_, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                                    window.close_dialog(cx);
                                }),
                        )
                        .when(web, |this| {
                            this.child(
                                Button::new("link-open")
                                    .primary()
                                    .label("Открыть")
                                    .on_click(move |_, window, cx| {
                                        cx.open_url(&open);
                                        window.close_dialog(cx);
                                    }),
                            )
                        }),
                )
        });
    }

    /// Act on tray clicks and on later starts of the client.
    fn poll_desktop(&mut self, cx: &mut Context<Self>) {
        let mut events = self.desktop.poll();
        if let Some(instance) = &self.instance {
            while instance.wakeups.try_recv().is_ok() {
                events.push(DesktopEvent::Show);
            }
        }
        for event in events {
            match event {
                DesktopEvent::Show => self.show_main_window(cx),
                DesktopEvent::Quit => cx.quit(),
            }
        }
    }

    /// Raise the window after a tray click, a notification click, or a second start.
    fn show_main_window(&self, cx: &mut Context<Self>) {
        let _ = self
            .window
            .update(cx, |_, window, _| desktop::reveal(window));
    }

    /// A system notification for a new incoming message, unless the user is looking at that
    /// chat or turned notifications off. One notification per chat: a newer one replaces it.
    fn notify_message(&self, chat_id: i64, message: &MessageRow, cx: &mut Context<Self>) {
        if !self.ui.notifications || message.out || message.delivery != Delivery::Sent {
            return;
        }
        let Screen::Main {
            chats, selected, ..
        } = &self.screen
        else {
            return;
        };
        let looking = self.window_active
            && self.section == Section::Chats
            && *selected == Some(chat_id)
            && desktop::window_visible();
        if looking {
            return;
        }
        let Some(chat) = chats.iter().find(|c| c.id == chat_id) else {
            return;
        };
        if chat.muted {
            return;
        }
        let (title, body) = notification_text(chat, message, self.ui.preview);
        cx.show_system_notification(SystemNotification {
            tag: format!("{NOTIFY_TAG}{chat_id}").into(),
            title: title.into(),
            body: body.into(),
            actions: Vec::new(),
        });
    }

    /// Open the full-screen viewer on the photo at `url`, with every loaded photo of the chat to
    /// page through.
    fn open_viewer(
        &mut self,
        chat_id: i64,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let photos: Vec<ViewerPhoto> = self
            .histories
            .get(&chat_id)
            .map(|h| h.messages.as_slice())
            .unwrap_or_default()
            .iter()
            .flat_map(|m| {
                m.attachments.iter().filter_map(move |a| match a {
                    Attachment::Photo { url, photo_id, .. } => Some(ViewerPhoto {
                        url: url.clone(),
                        photo_id: *photo_id,
                        time: m.time,
                        sender: if m.out {
                            "Вы".to_string()
                        } else {
                            m.sender.clone()
                        },
                    }),
                    _ => None,
                })
            })
            .collect();
        let index = photos.iter().position(|p| p.url == url).unwrap_or(0);
        self.viewer = Some(Viewer::new(photos, index));
        self.menu_row = None;
        self.viewer_focus.focus(window, cx);
        cx.notify();
    }

    fn close_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.viewer = None;
        if let Some((_, _, image)) = self.viewer_image.take() {
            // Release the decoded pixels from the window's image atlas.
            let _ = window.drop_image(image);
        }
        cx.notify();
    }

    /// The viewer's photo decoded and turned, decoded again only when the photo or the turn
    /// changes.
    fn viewer_frame(&mut self, window: &mut Window, cx: &App) -> Option<Arc<RenderImage>> {
        let v = self.viewer.as_ref()?;
        let photo = v.current()?;
        if let Some((url, turns, image)) = &self.viewer_image
            && *url == photo.url
            && *turns == v.turns
        {
            return Some(image.clone());
        }
        let Some(ImageState::Ready(source)) = self.images.get(&photo.url) else {
            return None;
        };
        let decoded = source.to_image_data(cx.svg_renderer()).ok()?;
        let size = decoded.size(0);
        let (w, h) = (
            u32::try_from(size.width.0).ok()?,
            u32::try_from(size.height.0).ok()?,
        );
        let (pixels, w, h) = rotate_pixels(decoded.as_bytes(0)?, w, h, v.turns);
        let buffer = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(w, h, pixels)?;
        let frame = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        let (url, turns) = (photo.url.clone(), v.turns);
        if let Some((_, _, old)) = self.viewer_image.replace((url, turns, frame.clone())) {
            let _ = window.drop_image(old);
        }
        Some(frame)
    }

    /// Save the shown photo, as loaded, into the Downloads folder.
    fn save_viewed_photo(&mut self, cx: &mut Context<Self>) {
        let Some(v) = &mut self.viewer else { return };
        let Some(photo) = v.photos.get(v.index) else {
            return;
        };
        let Some(ImageState::Ready(image)) = self.images.get(&photo.url) else {
            return;
        };
        let ext = match image.format() {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Webp => "webp",
            ImageFormat::Gif => "gif",
            ImageFormat::Svg => "svg",
            ImageFormat::Bmp => "bmp",
            _ => "img",
        };
        let name = match photo.photo_id {
            Some(id) => format!("photo_{id}.{ext}"),
            None => format!("photo_{}.{ext}", photo.time),
        };
        v.notice = Some(match crate::backend::save_download(&name, image.bytes()) {
            Ok(path) => format!("Сохранено: {}", path.display()),
            Err(e) => format!("Не удалось сохранить: {e}"),
        });
        cx.notify();
    }

    fn viewer_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        if key == "escape" {
            self.close_viewer(window, cx);
            return;
        }
        let Some(v) = &mut self.viewer else { return };
        match key {
            "left" => v.step(-1),
            "right" => v.step(1),
            "=" | "+" => v.zoom_by(1),
            "-" => v.zoom_by(-1),
            "r" => v.rotate(),
            "0" => {
                v.zoom = 1.;
                v.pan = (0., 0.);
            }
            "s" if ev.keystroke.modifiers.control => {
                self.save_viewed_photo(cx);
                return;
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// A round dark button of the viewer with a white icon.
    fn viewer_button(
        id: &'static str,
        icon: impl Into<Icon>,
        hint: &'static str,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .p_2()
            .rounded_full()
            .cursor_pointer()
            .hover(|this| this.bg(gpui_kit::white().opacity(0.14)))
            .tooltip(move |window, cx| Tooltip::new(hint).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
            .child(icon.into().size_5().text_color(gpui_kit::white()))
    }

    /// The full-screen viewer over the whole window: the photo, a toolbar with zoom, turning
    /// and saving, and arrows to page through the chat's photos.
    fn render_viewer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let frame = self.viewer_frame(window, cx);
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));
        const BAR: f32 = 56.;
        const SIDE: f32 = 72.;
        let area = ((vw - 2. * SIDE).max(100.), (vh - BAR - 24.).max(100.));
        let v = self.viewer.as_mut()?;
        let image_size = frame.as_ref().map_or((0., 0.), |f| {
            let s = f.size(0);
            (s.width.0 as f32, s.height.0 as f32)
        });
        let fit = fit_scale(image_size, area);
        let shown = (image_size.0 * fit * v.zoom, image_size.1 * fit * v.zoom);
        v.pan = clamp_pan(v.pan, shown, area);
        let left = SIDE + (area.0 - shown.0) / 2. + v.pan.0;
        let top = BAR + (area.1 - shown.1) / 2. + v.pan.1;
        let rect = (left, top, shown.0, shown.1);
        let draggable = shown.0 > area.0 + 0.5 || shown.1 > area.1 + 0.5;
        let percent = (fit * v.zoom * 100.).round();
        let count = v.photos.len();
        let (index, notice) = (v.index, v.notice.clone());
        let title = v.current().map_or_else(String::new, |p| {
            let when = zoned(p.time, &jiff::tz::TimeZone::system())
                .map(|z| z.strftime("%d.%m.%Y, %H:%M").to_string())
                .unwrap_or_default();
            [p.sender.as_str(), when.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        });
        let light = gpui_kit::white();
        let dim = light.opacity(0.65);
        let inside = move |x: f32, y: f32| {
            x >= rect.0 && x <= rect.0 + rect.2 && y >= rect.1 && y <= rect.1 + rect.3
        };
        let photo = match frame {
            Some(frame) => img(frame)
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(shown.0))
                .h(px(shown.1))
                .into_any_element(),
            None => div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(dim)
                .child("Фото не загрузилось")
                .into_any_element(),
        };
        let arrow = |id: &'static str, icon: Lucide, by: isize, cx: &mut Context<Self>| {
            div()
                .id(id)
                .absolute()
                .top(px(BAR))
                .bottom_0()
                .w(px(SIDE))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|this| this.bg(light.opacity(0.06)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(v) = &mut this.viewer {
                        v.step(by);
                    }
                    cx.notify();
                }))
                .child(Icon::new(icon).size_8().text_color(light))
        };
        let toolbar = h_flex()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(BAR))
            .px_4()
            .gap_4()
            .items_center()
            .justify_between()
            .bg(gpui_kit::black().opacity(0.35))
            .child(
                v_flex()
                    .min_w_0()
                    .child(div().text_sm().text_color(light).truncate().child(title))
                    .child(div().text_xs().text_color(dim).truncate().child(
                        notice.unwrap_or_else(|| {
                            if count > 1 {
                                format!("{} из {count}", index + 1)
                            } else {
                                String::new()
                            }
                        }),
                    )),
            )
            .child(
                h_flex()
                    .gap_1()
                    .flex_shrink_0()
                    .items_center()
                    .child(Self::viewer_button(
                        "viewer-zoom-out",
                        Lucide::ZoomOut,
                        "Уменьшить (−)",
                        |this, _, _| {
                            if let Some(v) = &mut this.viewer {
                                v.zoom_by(-1);
                            }
                        },
                        cx,
                    ))
                    .child(
                        div()
                            .id("viewer-percent")
                            .w(px(56.))
                            .text_center()
                            .text_sm()
                            .text_color(light)
                            .cursor_pointer()
                            .tooltip(|window, cx| Tooltip::new("Вписать (0)").build(window, cx))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(v) = &mut this.viewer {
                                    v.zoom = 1.;
                                    v.pan = (0., 0.);
                                }
                                cx.notify();
                            }))
                            .child(format!("{percent}%")),
                    )
                    .child(Self::viewer_button(
                        "viewer-zoom-in",
                        Lucide::ZoomIn,
                        "Увеличить (+)",
                        |this, _, _| {
                            if let Some(v) = &mut this.viewer {
                                v.zoom_by(1);
                            }
                        },
                        cx,
                    ))
                    .child(Self::viewer_button(
                        "viewer-rotate",
                        Lucide::RotateCw,
                        "Повернуть (R)",
                        |this, _, _| {
                            if let Some(v) = &mut this.viewer {
                                v.rotate();
                            }
                        },
                        cx,
                    ))
                    .child(Self::viewer_button(
                        "viewer-save",
                        Lucide::Download,
                        "Скачать (Ctrl+S)",
                        |this, _, cx| this.save_viewed_photo(cx),
                        cx,
                    ))
                    .child(Self::viewer_button(
                        "viewer-close",
                        Lucide::X,
                        "Закрыть (Esc)",
                        |this, window, cx| this.close_viewer(window, cx),
                        cx,
                    )),
            );
        Some(
            div()
                .id("viewer")
                .absolute()
                .inset_0()
                .bg(gpui_kit::black().opacity(0.96))
                .track_focus(&self.viewer_focus)
                .on_key_down(cx.listener(Self::viewer_key))
                .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
                    let dy = f32::from(ev.delta.pixel_delta(px(20.)).y);
                    if let Some(v) = &mut this.viewer
                        && dy.abs() > 0.5
                    {
                        v.zoom_by(if dy > 0. { 1 } else { -1 });
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                        let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
                        let in_area = y > BAR && x > SIDE && x < vw - SIDE;
                        if inside(x, y) {
                            if draggable && let Some(v) = &mut this.viewer {
                                v.drag = Some((x, y));
                            }
                        } else if in_area {
                            // A click on the dark backdrop closes the viewer.
                            this.close_viewer(window, cx);
                        }
                    }),
                )
                .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                    let Some(v) = &mut this.viewer else { return };
                    let Some((lx, ly)) = v.drag else { return };
                    if ev.pressed_button != Some(MouseButton::Left) {
                        v.drag = None;
                        return;
                    }
                    let (x, y) = (f32::from(ev.position.x), f32::from(ev.position.y));
                    v.pan = (v.pan.0 + x - lx, v.pan.1 + y - ly);
                    v.drag = Some((x, y));
                    cx.notify();
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, _| {
                        if let Some(v) = &mut this.viewer {
                            v.drag = None;
                        }
                    }),
                )
                .child(photo)
                .when(index > 0, |this| {
                    this.child(arrow("viewer-prev", Lucide::ChevronLeft, -1, cx).left_0())
                })
                .when(index + 1 < count, |this| {
                    this.child(arrow("viewer-next", Lucide::ChevronRight, 1, cx).right_0())
                })
                .child(toolbar)
                .when(draggable, |this| this.cursor_grab()),
        )
    }

    /// Photos and videos of one message laid out as a grid ([`album::layout`]); returns its
    /// width with the element.
    fn album_view(
        &self,
        chat_id: i64,
        message_key: u64,
        items: &[&Attachment],
        cx: &mut Context<Self>,
    ) -> (f32, AnyElement) {
        let sizes: Vec<(f32, f32)> = items
            .iter()
            .map(|a| match a {
                Attachment::Photo { width, height, .. }
                | Attachment::Video { width, height, .. } => (*width as f32, *height as f32),
                _ => (0., 0.),
            })
            .collect();
        let layout = album::layout(&sizes, &album::LIMITS);
        let muted_bg = cx.theme().muted;
        let muted_fg = cx.theme().muted_foreground;
        let cells: Vec<AnyElement> = items
            .iter()
            .zip(&layout.cells)
            .enumerate()
            .map(|(i, (a, c))| {
                // A loaded photo opens the full-screen viewer.
                let photo_url = match a {
                    Attachment::Photo { url, .. }
                        if matches!(self.images.get(url), Some(ImageState::Ready(_))) =>
                    {
                        Some(url.clone())
                    }
                    _ => None,
                };
                let (url, duration) = match a {
                    Attachment::Photo { url, .. } => (Some(url), None),
                    Attachment::Video {
                        thumbnail,
                        duration,
                        ..
                    } => (thumbnail.as_ref(), Some(*duration)),
                    _ => (None, None),
                };
                let content = match url.and_then(|u| self.images.get(u)) {
                    // GPUI crops a cover-fitted image to the element and rounds what is visible.
                    Some(ImageState::Ready(image)) => img(image.clone())
                        .size_full()
                        .rounded(px(CELL_RADIUS))
                        .object_fit(ObjectFit::Cover)
                        .into_any_element(),
                    state => div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(match (state, duration) {
                            (Some(ImageState::Failed), _) => "Не загрузилось",
                            (_, Some(_)) if url.is_none() => "Видео",
                            _ => "",
                        })
                        .into_any_element(),
                };
                div()
                    .id(("cell", message_key.wrapping_mul(31).wrapping_add(i as u64)))
                    .absolute()
                    .left(px(c.x))
                    .top(px(c.y))
                    .w(px(c.w))
                    .h(px(c.h))
                    .rounded(px(CELL_RADIUS))
                    .overflow_hidden()
                    .bg(muted_bg)
                    .when_some(photo_url, |this, url| {
                        this.cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_viewer(chat_id, &url, window, cx)
                            }))
                    })
                    .child(content)
                    .when_some(duration, |this, ms| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    div()
                                        .size(px(44.))
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(gpui_kit::black().opacity(0.5))
                                        .text_color(gpui_kit::white())
                                        .child("▶"),
                                ),
                        )
                        .when(ms > 0, |this| {
                            this.child(
                                div()
                                    .absolute()
                                    .top(px(6.))
                                    .left(px(6.))
                                    .px(px(6.))
                                    .rounded(px(6.))
                                    .text_xs()
                                    .bg(gpui_kit::black().opacity(0.55))
                                    .text_color(gpui_kit::white())
                                    .child(media_duration(ms)),
                            )
                        })
                    })
                    .into_any_element()
            })
            .collect();
        (
            layout.width,
            div()
                .relative()
                .w(px(layout.width))
                .h(px(layout.height))
                .children(cells)
                .into_any_element(),
        )
    }

    /// Fill, text and the dimmer time colour of a message bubble.
    ///
    /// Own messages are blue in both themes. The theme primary is a button:
    /// near-black on a light window and white on a dark one.
    fn bubble_colors(out: bool, cx: &App) -> (Hsla, Hsla, Hsla) {
        if out {
            let fg: Hsla = rgb(0xffffff).into();
            (rgb(0x2563eb).into(), fg, fg.opacity(0.72))
        } else {
            (
                cx.theme().secondary,
                cx.theme().secondary_foreground,
                cx.theme().muted_foreground,
            )
        }
    }

    fn bubble(
        &self,
        chat_id: i64,
        m: &MessageRow,
        chat_title: &str,
        peer_mark: i64,
        who: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Photos and videos go into one grid; files and stickers follow it one by one.
        let media: Vec<&Attachment> = m
            .attachments
            .iter()
            .filter(|a| matches!(a, Attachment::Photo { .. } | Attachment::Video { .. }))
            .collect();
        let message_key =
            u64::from_ne_bytes(if m.id != 0 { m.id } else { m.cid.unwrap_or(0) }.to_ne_bytes());
        let album = (!media.is_empty()).then(|| self.album_view(chat_id, message_key, &media, cx));
        let album_width = album.as_ref().map(|(w, _)| *w);
        let mut attachments: Vec<AnyElement> = album.map(|(_, e)| e).into_iter().collect();
        attachments.extend(
            m.attachments
                .iter()
                .enumerate()
                .filter(|(_, a)| {
                    !matches!(
                        a,
                        Attachment::Other { .. }
                            | Attachment::Photo { .. }
                            | Attachment::Video { .. }
                            | Attachment::Share { .. }
                    )
                })
                .map(|(i, a)| self.attachment_view(chat_id, m, a, i, cx)),
        );
        let state = match m.delivery {
            Delivery::Sent if m.out && m.time <= peer_mark => " · ✓✓".into(),
            Delivery::Sent if m.out => " · ✓".into(),
            Delivery::Sent => String::new(),
            Delivery::Pending => " · отправка…".into(),
            Delivery::Failed => " · не отправлено".into(),
        };
        let (bg, fg, muted) = Self::bubble_colors(m.out, cx);
        let show_sender = !m.out && !m.sender.is_empty() && m.sender != chat_title;
        let accent = if m.out { fg } else { cx.theme().primary };
        // With an album the caption wraps at the album's width, as in other messengers.
        let max_w = album_width.map_or(520., |w| w + 24.);
        let bubble = v_flex()
            .max_w(px(max_w))
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
            .when_some(m.forwarded.clone(), |this, from| {
                this.child(
                    div()
                        .text_xs()
                        .italic()
                        .text_color(muted)
                        .child(format!("Переслано от {from}")),
                )
            })
            .when_some(m.reply.clone(), |this, q| {
                this.child(
                    v_flex()
                        .pl_2()
                        .border_l_2()
                        .border_color(accent)
                        .max_w(px(496.))
                        .child(div().text_xs().font_semibold().text_color(accent).child(
                            if q.sender.is_empty() {
                                "Сообщение".to_string()
                            } else {
                                q.sender
                            },
                        ))
                        .child(div().text_xs().text_color(muted).truncate().child(q.text)),
                )
            })
            .children(attachments)
            .when(!m.text.is_empty(), |this| {
                this.child(self.rich_text(m, message_key, cx))
            })
            .children(
                self.share_cards(m)
                    .into_iter()
                    .enumerate()
                    .map(|(i, card)| self.share_card(message_key, i, &card, cx)),
            )
            .child(div().text_xs().text_color(muted).child(format!(
                "{}{}{state}",
                if m.edited { "изменено · " } else { "" },
                clock(m.time)
            )))
            .when(m.id != 0 && m.delivery == Delivery::Sent, |this| {
                this.child(self.reaction_row(chat_id, m, message_key, who, cx))
            });
        let key =
            u64::from_ne_bytes(if m.id != 0 { m.id } else { m.cid.unwrap_or(0) }.to_ne_bytes());
        // The row under the mouse, and the row whose menu is open, are tinted across the width.
        let highlight = cx.theme().foreground.opacity(0.06);
        let menu_open = self.menu_row == Some(key);
        h_flex()
            .id(("msg", key))
            .w_full()
            .px_4()
            .py_1()
            .when(m.out, |this| this.justify_end())
            .when(menu_open, |this| this.bg(highlight))
            .hover(move |this| this.bg(highlight))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, _, cx| {
                    this.menu_row = Some(key);
                    cx.notify();
                }),
            )
            .child(bubble)
            .context_menu(self.message_menu(chat_id, m, chat_title, cx))
            .into_any_element()
    }

    /// The right-click menu of a message: reply, copy text, edit our own, delete. A message
    /// still on its way or refused offers only copying.
    fn message_menu(
        &self,
        chat_id: i64,
        m: &MessageRow,
        chat_title: &str,
        cx: &mut Context<Self>,
    ) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
        let view = cx.entity().downgrade();
        let row = m.clone();
        let title = chat_title.to_string();
        let on_server = m.id != 0 && m.delivery == Delivery::Sent;
        let can_edit = editable(m);
        move |menu, _, _| {
            let mut menu = menu;
            if on_server {
                let (view, row, title) = (view.clone(), row.clone(), title.clone());
                menu = menu.item(
                    PopupMenuItem::new("Ответить")
                        .icon(Icon::new(Lucide::Reply))
                        .on_click(move |_, window, cx| {
                            let _ = view
                                .update(cx, |this, cx| this.start_reply(&row, &title, window, cx));
                        }),
                );
            }
            if !row.text.is_empty() {
                let (view, text) = (view.clone(), row.text.clone());
                menu = menu.item(
                    PopupMenuItem::new("Копировать текст")
                        .icon(Icon::new(IconName::Copy))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                            let _ = view.update(cx, |this, cx| {
                                this.menu_row = None;
                                if let Screen::Main { status, .. } = &mut this.screen {
                                    *status = Some("Текст скопирован".into());
                                }
                                cx.notify();
                            });
                        }),
                );
            }
            if can_edit {
                let (view, row) = (view.clone(), row.clone());
                menu = menu.item(
                    PopupMenuItem::new("Изменить")
                        .icon(Icon::new(Lucide::Pencil))
                        .on_click(move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| this.start_edit(&row, window, cx));
                        }),
                );
            }
            if on_server {
                let (view, id, out) = (view.clone(), row.id, row.out);
                menu = menu.separator().item(
                    PopupMenuItem::new("Удалить")
                        .icon(Icon::new(Lucide::Trash))
                        .on_click(move |_, window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.ask_delete(chat_id, id, out, window, cx)
                            });
                        }),
                );
            }
            menu
        }
    }

    /// Ask how to delete a message: for everyone or only for us (ours), or only for us
    /// (someone else's).
    fn ask_delete(
        &mut self,
        chat_id: i64,
        id: i64,
        out: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_row = None;
        let view = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let delete = |for_me: bool| {
                let view = view.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                    let _ =
                        view.update(cx, |this, cx| this.delete_message(chat_id, id, for_me, cx));
                    window.close_dialog(cx);
                }
            };
            dialog
                .w(px(440.))
                .title("Удалить сообщение?")
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(if out {
                            "Удалить его у всех участников чата или только у вас?"
                        } else {
                            "Сообщение пропадёт только у вас; у остальных оно останется."
                        }),
                )
                .footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("delete-cancel")
                                .ghost()
                                .label("Отмена")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(if out {
                            Button::new("delete-for-me")
                                .outline()
                                .label("У меня")
                                .on_click(delete(true))
                        } else {
                            Button::new("delete-for-me")
                                .danger()
                                .label("Удалить")
                                .on_click(delete(true))
                        })
                        .when(out, |this| {
                            this.child(
                                Button::new("delete-for-all")
                                    .danger()
                                    .label("У всех")
                                    .on_click(delete(false)),
                            )
                        }),
                )
        });
    }

    /// One transcript row. A day header rides on the first message of that day,
    /// so the virtual list stays one row per message.
    fn history_item(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(chat_id) = self.history_for else {
            return div().into_any_element();
        };
        let Screen::Main { chats, .. } = &self.screen else {
            return div().into_any_element();
        };
        let Some(chat) = chats.iter().find(|c| c.id == chat_id) else {
            return div().into_any_element();
        };
        let Some(history) = self.histories.get(&chat_id) else {
            return div().into_any_element();
        };
        let Some(message) = history.messages.get(ix) else {
            return div().into_any_element();
        };
        let day = day_label(message.time);
        let new_day = ix == 0
            || history
                .messages
                .get(ix - 1)
                .is_none_or(|prev| day_label(prev.time) != day);
        let bubble = self.bubble(
            chat.id,
            message,
            &chat.title,
            chat.peer_mark,
            chat.kind != ChatKind::Channel,
            cx,
        );
        if !new_day {
            return bubble;
        }
        v_flex()
            .w_full()
            .child(
                h_flex().w_full().justify_center().py_2().child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(day),
                ),
            )
            .child(bubble)
            .into_any_element()
    }

    fn render_chat(&self, chat: &ChatRow, cx: &mut Context<Self>) -> Div {
        let history = self.histories.get(&chat.id);
        let count = history.map(|h| h.messages.len()).unwrap_or(0);
        let chat_id = chat.id;
        // None: no older page; Some(loading) otherwise.
        let older_state = history
            .filter(|h| h.has_more && !h.messages.is_empty())
            .map(|h| h.loading_older);
        // The button sits above the transcript only while the first row is in view.
        let at_top = self.history_list.logical_scroll_top().item_ix == 0;
        let notice = match history {
            Some(h) if h.loading => Some("Загрузка…".to_string()),
            Some(History { error: Some(e), .. }) => {
                Some(format!("Не удалось загрузить историю: {e}"))
            }
            Some(h) if h.messages.is_empty() => Some("Сообщений пока нет".to_string()),
            _ => None,
        };
        // The list builds only the rows in view (plus a margin), so a long chat
        // does not lay out every bubble on each scroll step.
        let transcript = list(
            self.history_list.clone(),
            cx.processor(|this, ix: usize, _, cx| this.history_item(ix, cx)),
        )
        .size_full();
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.chat_header(chat, cx))
            .when(older_state.is_some() && at_top, |this| {
                let loading = older_state == Some(true);
                this.child(
                    h_flex().w_full().justify_center().py_1().child(
                        Button::new("older")
                            .ghost()
                            .small()
                            .label(if loading {
                                "Загрузка…"
                            } else {
                                "Показать более ранние"
                            })
                            .disabled(loading)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.load_older(chat_id, cx)),
                            ),
                    ),
                )
            })
            .child(
                div()
                    .id("history")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    // A left click anywhere closes a right-click menu; drop its row's highlight.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.menu_row.take().is_some() {
                                cx.notify();
                            }
                        }),
                    )
                    // Runs after the list has already jumped by the whole notch.
                    .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| {
                        this.glide_history(ev, window, cx);
                    }))
                    .when(count == 0, |this| {
                        this.child(
                            h_flex()
                                .size_full()
                                .justify_center()
                                .items_center()
                                .text_color(cx.theme().muted_foreground)
                                .child(notice.clone().unwrap_or_default()),
                        )
                    })
                    .when(count > 0, |this| {
                        this.child(transcript)
                            .vertical_scrollbar(&self.history_list)
                    }),
            )
            .child(
                v_flex()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .children(self.compose_bar(cx))
                    .when(self.emoji_open && chat.kind != ChatKind::Channel, |this| {
                        this.child(self.emoji_panel(cx))
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .px_4()
                            .py_3()
                            .child(
                                Button::new("attach")
                                    .ghost()
                                    .label("📎")
                                    .disabled(matches!(self.compose, Some(Compose::Edit { .. })))
                                    .on_click(cx.listener(|this, _, _, cx| this.pick_file(cx))),
                            )
                            .when(chat.kind != ChatKind::Channel, |this| {
                                let open = self.emoji_open;
                                this.child(
                                    Button::new("emoji")
                                        .ghost()
                                        .selected(open)
                                        .label("😊")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.emoji_open = !this.emoji_open;
                                            if this.emoji_open
                                                && this.animoji.is_none()
                                                && !this.animoji_loading
                                                && this
                                                    .commands
                                                    .send(UiCommand::LoadAnimoji)
                                                    .is_ok()
                                            {
                                                this.animoji_loading = true;
                                            }
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(div().flex_1().child(Input::new(&self.composer)))
                            .child(
                                Button::new("send")
                                    .primary()
                                    .label(if matches!(self.compose, Some(Compose::Edit { .. })) {
                                        "Сохранить"
                                    } else {
                                        "Отправить"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.send_message(window, cx)
                                    })),
                            ),
                    ),
            )
    }

    /// The bar above the history: photo, title, and typing, subscribers or members under it.
    /// A click opens or closes the profile panel.
    fn chat_header(&self, chat: &ChatRow, cx: &mut Context<Self>) -> Stateful<Div> {
        let (subtitle, highlight) = match self.typing_label(chat) {
            Some(typing) => (Some(typing), true),
            None => (chat.members_line(), false),
        };
        h_flex()
            .id("chat-header")
            .gap_3()
            .px_4()
            .py_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .cursor_pointer()
            .hover(|this| this.bg(cx.theme().secondary_hover))
            .on_click(cx.listener(|this, _, _, cx| {
                this.profile_open = !this.profile_open;
                cx.notify();
            }))
            .child(self.avatar(&chat.title, chat.avatar.as_ref(), 40., cx))
            .child(
                v_flex()
                    .min_w_0()
                    .child(div().font_semibold().truncate().child(chat.title.clone()))
                    .when_some(subtitle, |this, line| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(if highlight {
                                    cx.theme().primary
                                } else {
                                    cx.theme().muted_foreground
                                })
                                .child(line),
                        )
                    }),
            )
    }

    /// The chat's profile beside the history: photo, title, members, description, link.
    fn render_profile(&self, chat: &ChatRow, cx: &mut Context<Self>) -> Div {
        let muted = cx.theme().muted_foreground;
        let kind = match chat.kind {
            ChatKind::Dialog => "Личный чат",
            ChatKind::Saved => "Избранное: заметки для себя",
            ChatKind::Group => "Группа",
            ChatKind::Channel => "Канал",
        };
        let subtitle = chat.members_line().unwrap_or_else(|| kind.to_string());
        let link = chat.link.clone();
        v_flex()
            .w(px(320.))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .px_4()
                    .py_3()
                    .justify_between()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().font_semibold().child("Информация"))
                    .child(
                        Button::new("profile-close")
                            .ghost()
                            .small()
                            .label("✕")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.profile_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("profile")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .gap_4()
                    .child(
                        v_flex()
                            .items_center()
                            .gap_2()
                            .child(self.avatar(&chat.title, chat.avatar.as_ref(), 96., cx))
                            .child(
                                div()
                                    .text_lg()
                                    .font_semibold()
                                    .text_center()
                                    .child(chat.title.clone()),
                            )
                            .child(div().text_sm().text_color(muted).child(subtitle)),
                    )
                    .when(!chat.description.is_empty(), |this| {
                        this.child(Self::info_row("Описание", chat.description.clone(), cx))
                    })
                    .when(!link.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_1()
                                .child(Self::info_row("Ссылка", link.clone(), cx))
                                .child(
                                    h_flex().child(
                                        Button::new("profile-copy-link")
                                            .outline()
                                            .small()
                                            .label("Копировать ссылку")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    link.clone(),
                                                ));
                                                if let Screen::Main { status, .. } =
                                                    &mut this.screen
                                                {
                                                    *status = Some("Ссылка скопирована".into());
                                                }
                                                cx.notify();
                                            })),
                                    ),
                                ),
                        )
                    })
                    .when(chat.members.is_some(), |this| {
                        this.child(Self::info_row("Тип", kind.to_string(), cx))
                    }),
            )
    }

    /// Plain emoji the picker offers. Animated ones come from the server.
    const EMOJI: &'static [&'static str] = &[
        "😀", "😁", "😂", "🤣", "😊", "😍", "😘", "😎", "🤔", "😴", "😢", "😭", "😡", "🤯", "👍",
        "👎", "👏", "🙏", "🔥", "❤️", "🎉", "✅", "❌", "⭐", "💯", "🤝", "👀", "💪", "🎂", "🚀",
    ];

    /// The plate above the composer: plain emoji, then the animated ones that play.
    fn emoji_panel(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let faces = self.animoji.clone().unwrap_or_default();
        v_flex()
            .id("emoji-panel")
            .w_full()
            .max_h(px(240.))
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Смайлики"),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(Self::EMOJI.iter().enumerate().map(|(i, emoji)| {
                        let chosen = (*emoji).to_string();
                        Button::new(("moji", i as u64))
                            .xsmall()
                            .ghost()
                            .label(*emoji)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.insert_emoji(&chosen, None, window, cx);
                            }))
                    })),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Анимированные"),
            )
            .child(if self.animoji_loading {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Загрузка…")
                    .into_any_element()
            } else if faces.is_empty() {
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        self.animoji_note
                            .clone()
                            .unwrap_or_else(|| "Анимированных смайликов нет".to_string()),
                    )
                    .into_any_element()
            } else {
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(faces.into_iter().enumerate().map(|(i, face)| {
                        let chosen = face.clone();
                        div()
                            .id(("face", i as u64))
                            .w(px(36.))
                            .h(px(36.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .rounded(px(6.))
                            .hover(|this| this.bg(cx.theme().secondary_hover))
                            .child(self.animoji_view(&face.lottie_url, &face.emoji))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.insert_emoji(&chosen.emoji, Some(chosen.clone()), window, cx);
                            }))
                            .into_any_element()
                    }))
                    .into_any_element()
            })
    }

    /// Put `emoji` at the cursor. An animated one is remembered until the text loses it.
    fn insert_emoji(
        &mut self,
        emoji: &str,
        face: Option<max_core::assets::AnimojiFace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editing = matches!(self.compose, Some(Compose::Edit { .. }));
        if let Some(face) = face.filter(|_| !editing) {
            self.animoji_picks.push(face);
        }
        let emoji = emoji.to_string();
        self.composer
            .update(cx, |input, cx| input.insert(emoji, window, cx));
        self.focus_composer(window, cx);
    }

    /// The line above the composer while replying or editing, with a button to drop it.
    fn compose_bar(&self, cx: &mut Context<Self>) -> Option<Div> {
        let (title, body) = match self.compose.as_ref()? {
            Compose::Reply { quote } => (
                if quote.sender.is_empty() {
                    "Ответ".to_string()
                } else {
                    format!("Ответ: {}", quote.sender)
                },
                quote.text.clone(),
            ),
            Compose::Edit { original, .. } => {
                ("Изменение сообщения".to_string(), one_line(original))
            }
        };
        Some(
            h_flex()
                .gap_3()
                .px_4()
                .pt_2()
                .items_center()
                .child(
                    div()
                        .w(px(3.))
                        .h(px(32.))
                        .rounded(px(2.))
                        .bg(cx.theme().primary),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(body),
                        ),
                )
                .child(
                    Button::new("compose-cancel")
                        .ghost()
                        .small()
                        .label("✕")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_compose(window, cx)),
                        ),
                ),
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
        let unread: i32 = chats.iter().map(|c| c.unread.max(0)).sum();
        let (title, list): (&str, AnyElement) = match self.section {
            Section::Chats => ("Чаты", self.chat_list(cx)),
            Section::Contacts => ("Контакты", self.contact_list(cx).into_any_element()),
            Section::Calls => (
                "Звонки",
                div()
                    .p_4()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Здесь будет история звонков.")
                    .into_any_element(),
            ),
            Section::Settings => ("Настройки", self.settings_list(cx).into_any_element()),
        };
        let sidebar = v_flex()
            .w(px(320.))
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
                    .child(div().text_lg().font_semibold().child(title))
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
            .child(div().flex_1().min_h_0().overflow_hidden().child(list));
        let content = match self.section {
            Section::Chats => match selected.and_then(|id| chats.iter().find(|c| c.id == id)) {
                Some(chat) if self.profile_open => h_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.render_chat(chat, cx))
                    .child(self.render_profile(chat, cx)),
                Some(chat) => self.render_chat(chat, cx),
                None => Self::placeholder("Выберите чат", None, cx),
            },
            Section::Contacts => self.contact_card(cx),
            Section::Calls => Self::placeholder(
                "Звонки пока не поддерживаются",
                Some(
                    "Аудио- и видеозвонки, их история и ссылки на звонки появятся в следующих версиях.",
                ),
                cx,
            ),
            Section::Settings => match self.settings_page {
                SettingsPage::Account => match &self.account {
                    Some(panel) => self.render_account(me, panel, cx),
                    None => Self::placeholder("Загрузка…", None, cx),
                },
                SettingsPage::Notifications => self.render_notifications(cx),
                SettingsPage::Appearance => self.render_appearance(cx),
                SettingsPage::Data => self.render_data(cx),
                SettingsPage::About => self.render_about(cx),
            },
        };
        h_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.rail(unread, cx))
            .child(sidebar)
            .child(content)
    }

    /// The navigation rail: chats, calls, contacts, and settings at the bottom.
    fn rail(&self, unread: i32, cx: &mut Context<Self>) -> Div {
        v_flex()
            .w(px(76.))
            .h_full()
            .flex_shrink_0()
            .py_3()
            .gap_1()
            .items_center()
            .border_r_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().sidebar)
            .child(self.rail_button(
                Section::Chats,
                Icon::new(Lucide::MessageCircle),
                "Чаты",
                unread,
                cx,
            ))
            .child(self.rail_button(Section::Calls, Icon::new(Lucide::Phone), "Звонки", 0, cx))
            .child(self.rail_button(
                Section::Contacts,
                Icon::new(Lucide::Contact),
                "Контакты",
                0,
                cx,
            ))
            .child(div().flex_1())
            .child(self.rail_button(
                Section::Settings,
                Icon::new(IconName::Settings),
                "Настройки",
                0,
                cx,
            ))
    }

    fn rail_button(
        &self,
        section: Section,
        icon: Icon,
        label: &'static str,
        badge: i32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = self.section == section;
        let color = if active {
            cx.theme().primary
        } else {
            cx.theme().muted_foreground
        };
        v_flex()
            .id(label)
            .w(px(68.))
            .py_2()
            .gap_1()
            .items_center()
            .rounded(cx.theme().radius)
            .cursor_pointer()
            .when(active, |this| this.bg(cx.theme().accent))
            .hover(|this| this.bg(cx.theme().secondary_hover))
            .text_color(color)
            .on_click(cx.listener(move |this, _, _, cx| this.switch_section(section, cx)))
            .child(
                div()
                    .relative()
                    .child(icon.text_color(color).size_5())
                    .when(badge > 0, |this| {
                        this.child(
                            div()
                                .absolute()
                                .top(px(-9.))
                                .left(px(10.))
                                .h(px(20.))
                                .min_w(px(20.))
                                .px(px(6.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .whitespace_nowrap()
                                .rounded_full()
                                .text_size(px(11.))
                                .line_height(px(14.))
                                .font_semibold()
                                .bg(cx.theme().danger)
                                // The theme's danger foreground is red text for light
                                // backgrounds; on the red pill it vanishes.
                                .text_color(gpui_kit::white())
                                .child(if badge > 99 {
                                    "99+".to_string()
                                } else {
                                    badge.to_string()
                                }),
                        )
                    }),
            )
            .child(div().text_xs().child(label))
    }

    /// A centered message filling the content area.
    fn placeholder(title: &str, hint: Option<&str>, cx: &App) -> Div {
        v_flex()
            .flex_1()
            .h_full()
            .gap_2()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_color(cx.theme().muted_foreground)
                    .child(title.to_string()),
            )
            .when_some(hint, |this, h| {
                this.child(
                    div()
                        .max_w(px(420.))
                        .text_sm()
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(h.to_string()),
                )
            })
    }

    /// A content page with a title bar and a scrolling body.
    fn page(id: &'static str, title: &str, body: Vec<AnyElement>, cx: &App) -> Div {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                div()
                    .px_6()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .font_semibold()
                    .child(title.to_string()),
            )
            .child(
                v_flex()
                    .id(id)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_6()
                    .py_4()
                    .gap_4()
                    .max_w(px(680.))
                    .children(body),
            )
    }

    /// A titled group of settings.
    fn group(title: &str, rows: Vec<AnyElement>, cx: &App) -> AnyElement {
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(title.to_string()),
            )
            .child(
                v_flex()
                    .p_4()
                    .gap_3()
                    .rounded(cx.theme().radius_lg)
                    .border_1()
                    .border_color(cx.theme().border)
                    .children(rows),
            )
            .into_any_element()
    }

    /// One labelled line with a muted value under the label.
    fn info_row(label: &str, value: String, cx: &App) -> AnyElement {
        v_flex()
            .gap_0p5()
            .child(div().child(label.to_string()))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(value),
            )
            .into_any_element()
    }

    fn settings_list(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        v_flex()
            .id("settings-list")
            .flex_1()
            .overflow_y_scroll()
            .p_2()
            .gap_1()
            .children(SettingsPage::ALL.into_iter().map(|page| {
                let active = self.settings_page == page;
                h_flex()
                    .id(page.title())
                    .gap_3()
                    .px_3()
                    .py_2p5()
                    .items_center()
                    .rounded(cx.theme().radius)
                    .cursor_pointer()
                    .when(active, |this| this.bg(cx.theme().accent))
                    .hover(|this| this.bg(cx.theme().secondary_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_settings_page(page, cx)))
                    .child(page.icon().size_4().text_color(cx.theme().muted_foreground))
                    .child(div().child(page.title()))
            }))
    }

    /// Chat rows built only for what is on screen. A pin line and an unread
    /// badge change a row's height, so each row is measured on its own.
    fn chat_list(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Screen::Main { chats, .. } = &self.screen {
            let stamp = chat_rows_stamp(chats);
            if self.chats_list.item_count() != chats.len() {
                self.chats_list.reset(chats.len());
            } else if self.chats_stamp.get() != stamp {
                self.chats_list.remeasure();
            }
            self.chats_stamp.set(stamp);
        }
        // Reset and remeasure happen above, and they change the pixel offset.
        // The wheel measures its jump from whatever the list will paint.
        self.chats_origin.set(list_scroll_px(&self.chats_list));
        self.chats_glide_base.set(self.chats_glide);
        div()
            .id("chat-scroll")
            .size_full()
            .relative()
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, window, cx| {
                this.glide_chats(ev, window, cx);
            }))
            .child(
                list(
                    self.chats_list.clone(),
                    cx.processor(|this, ix: usize, _, cx| this.chat_list_row(ix, cx)),
                )
                .size_full(),
            )
            .vertical_scrollbar(&self.chats_list)
            .into_any_element()
    }

    fn chat_list_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Screen::Main {
            chats, selected, ..
        } = &self.screen
        else {
            return div().into_any_element();
        };
        let Some(row) = chats.get(ix) else {
            return div().into_any_element();
        };
        let typing = self.typing_label(row);
        let avatar = self.avatar(&row.title, row.avatar.as_ref(), 40., cx);
        let menu = self.chat_menu(row, cx);
        Self::chat_row(row, *selected == Some(row.id), typing, avatar, cx)
            .mx_2()
            .context_menu(menu)
            .into_any_element()
    }

    fn contact_list(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.contacts.is_empty() {
            return div()
                .p_4()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Контактов пока нет")
                .into_any_element();
        }
        let count = self.contacts.len();
        uniform_list(
            "contact-list",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| this.contact_rows(range, cx)),
        )
        .track_scroll(&self.contacts_scroll)
        .size_full()
        .px_2()
        .py_1()
        .into_any_element()
    }

    fn contact_rows(
        &self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        range
            .filter_map(|ix| {
                let c = self.contacts.get(ix)?;
                let id = c.id;
                let selected = self.contact_selected == Some(id);
                Some(
                    h_flex()
                        .id(("contact", u64::from_ne_bytes(id.to_ne_bytes())))
                        .gap_3()
                        .px_3()
                        .py_2()
                        .items_center()
                        .rounded(cx.theme().radius)
                        .cursor_pointer()
                        .when(selected, |this| this.bg(cx.theme().accent))
                        .hover(|this| this.bg(cx.theme().secondary_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.contact_selected = Some(id);
                            cx.notify();
                        }))
                        .child(self.avatar(&c.name, c.avatar.as_ref(), 36., cx))
                        .child(div().truncate().child(c.name.clone()))
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// A round avatar: the photo at `url` once it is loaded, the first letter of `name` until
    /// then and when there is none.
    fn avatar(&self, name: &str, url: Option<&String>, size: f32, cx: &App) -> AnyElement {
        if let Some(ImageState::Ready(image)) = url.and_then(|u| self.images.get(u)) {
            return img(image.clone())
                .flex_shrink_0()
                .size(px(size))
                .rounded_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element();
        }
        let initial: String = name
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(size))
            .rounded_full()
            .bg(cx.theme().primary)
            .text_color(cx.theme().primary_foreground)
            .when(size >= 64., |this| this.text_3xl())
            .child(initial)
            .into_any_element()
    }

    /// Ask for the photos of every chat and contact not asked for yet.
    fn request_avatars(&mut self) {
        let mut urls: Vec<String> = self
            .contacts
            .iter()
            .filter_map(|c| c.avatar.clone())
            .collect();
        if let Screen::Main { chats, .. } = &self.screen {
            urls.extend(chats.iter().filter_map(|c| c.avatar.clone()));
        }
        self.request_images(urls);
    }

    fn contact_card(&self, cx: &mut Context<Self>) -> Div {
        let Some(c) = self
            .contact_selected
            .and_then(|id| self.contacts.iter().find(|c| c.id == id))
        else {
            return Self::placeholder("Выберите контакт", None, cx);
        };
        let chat_id = c.chat_id;
        v_flex()
            .flex_1()
            .h_full()
            .gap_3()
            .items_center()
            .justify_center()
            .child(self.avatar(&c.name, c.avatar.as_ref(), 96., cx))
            .child(div().text_xl().font_semibold().child(c.name.clone()))
            .child(match chat_id {
                Some(chat_id) => Button::new("contact-write")
                    .primary()
                    .label("Написать")
                    .on_click(cx.listener(move |this, _, _, cx| this.open_chat(chat_id, cx)))
                    .into_any_element(),
                None => div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Переписки с этим контактом в списке чатов нет")
                    .into_any_element(),
            })
    }

    fn render_notifications(&self, cx: &mut Context<Self>) -> Div {
        let ui = self.ui;
        let muted = cx.theme().muted_foreground;
        let toggle = |id: &'static str,
                      label: &'static str,
                      hint: &'static str,
                      on: bool,
                      enabled: bool,
                      set: fn(&mut UiSettings, bool),
                      cx: &mut Context<Self>| {
            h_flex()
                .gap_4()
                .justify_between()
                .items_center()
                .child(
                    v_flex()
                        .min_w_0()
                        .child(div().child(label))
                        .child(div().text_sm().text_color(muted).child(hint)),
                )
                .child(
                    Switch::new(id)
                        .checked(on)
                        .disabled(!enabled)
                        .on_click(cx.listener(move |this, checked: &bool, window, cx| {
                            let mut ui = this.ui;
                            set(&mut ui, *checked);
                            this.set_ui(ui, window, cx);
                        })),
                )
                .into_any_element()
        };
        let tray = self.desktop.has_tray();
        Self::page(
            "notifications",
            SettingsPage::Notifications.title(),
            vec![
                Self::group(
                    "Новые сообщения",
                    vec![
                        toggle(
                            "notify-on",
                            "Уведомления системы",
                            "О сообщениях в чатах, которые сейчас не открыты перед вами",
                            ui.notifications,
                            true,
                            |ui, on| ui.notifications = on,
                            cx,
                        ),
                        toggle(
                            "notify-preview",
                            "Показывать текст",
                            "Выключите, чтобы в уведомлении было только название чата",
                            ui.preview,
                            ui.notifications,
                            |ui, on| ui.preview = on,
                            cx,
                        ),
                    ],
                    cx,
                ),
                Self::group(
                    "Окно",
                    vec![toggle(
                        "close-to-tray",
                        "Закрывать в трей",
                        if tray {
                            if cfg!(target_os = "linux") {
                                "Крестик сворачивает окно и оставляет значок в трее; клиент продолжает получать сообщения"
                            } else {
                                "Крестик прячет окно в трей, клиент продолжает получать сообщения"
                            }
                        } else {
                            "Значок в трее не создался, поэтому крестик закрывает клиент"
                        },
                        ui.close_to_tray,
                        tray,
                        |ui, on| ui.close_to_tray = on,
                        cx,
                    )],
                    cx,
                ),
            ],
            cx,
        )
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> Div {
        let ui = self.ui;
        let themes = [
            (ThemeChoice::System, "Как в системе"),
            (ThemeChoice::Light, "Светлая"),
            (ThemeChoice::Dark, "Тёмная"),
        ];
        let theme_buttons = h_flex()
            .gap_2()
            .children(themes.into_iter().map(|(t, label)| {
                let b = Button::new(label).label(label).on_click(cx.listener(
                    move |this, _, window, cx| {
                        let ui = UiSettings {
                            theme: t,
                            ..this.ui
                        };
                        this.set_ui(ui, window, cx)
                    },
                ));
                if ui.theme == t {
                    b.primary()
                } else {
                    b.outline()
                }
            }));
        let size_buttons = h_flex().gap_2().children(FONT_SIZES.into_iter().map(|n| {
            let b = Button::new(("font", u64::from(n)))
                .label(format!("{n}"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    let ui = UiSettings {
                        font_size: n,
                        ..this.ui
                    };
                    this.set_ui(ui, window, cx)
                }));
            if ui.font_size == n {
                b.primary()
            } else {
                b.outline()
            }
        }));
        let sample = h_flex().child(
            v_flex()
                .px_3()
                .py_2()
                .gap_1()
                .rounded(px(12.))
                .bg(cx.theme().secondary)
                .text_color(cx.theme().secondary_foreground)
                .child(div().child("Так выглядит текст сообщений"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("12:00"),
                ),
        );
        Self::page(
            "appearance",
            SettingsPage::Appearance.title(),
            vec![
                Self::group("Тема", vec![theme_buttons.into_any_element()], cx),
                Self::group(
                    "Размер шрифта",
                    vec![size_buttons.into_any_element(), sample.into_any_element()],
                    cx,
                ),
            ],
            cx,
        )
    }

    fn render_data(&self, cx: &mut Context<Self>) -> Div {
        let downloads = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
        let profiles = max_store::default_profiles_dir();
        let open = downloads.clone();
        let files = vec![
            Self::info_row("Папка загрузок", downloads.display().to_string(), cx),
            h_flex()
                .child(
                    Button::new("open-downloads")
                        .outline()
                        .small()
                        .label("Открыть папку")
                        .on_click(move |_, _, cx| cx.open_with_system(&open)),
                )
                .into_any_element(),
        ];
        let mut local = vec![Self::info_row(
            "Профиль",
            profiles
                .map(|p| p.join("default").display().to_string())
                .unwrap_or_else(|| "не найден".into()),
            cx,
        )];
        local.push(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(local_store_note())
                .into_any_element(),
        );
        if let Some(p) = UiSettings::path() {
            local.push(Self::info_row(
                "Настройки интерфейса",
                p.display().to_string(),
                cx,
            ));
        }
        Self::page(
            "data",
            SettingsPage::Data.title(),
            vec![
                Self::group("Файлы", files, cx),
                Self::group("Данные на этом компьютере", local, cx),
            ],
            cx,
        )
    }

    fn render_about(&self, cx: &mut Context<Self>) -> Div {
        let muted = cx.theme().muted_foreground;
        let line = |text: &str| {
            div()
                .text_sm()
                .text_color(muted)
                .child(text.to_string())
                .into_any_element()
        };
        Self::page(
            "about",
            SettingsPage::About.title(),
            vec![
                Self::group(
                    "maxly-gpui",
                    vec![
                        Self::info_row("Версия", env!("CARGO_PKG_VERSION").to_string(), cx),
                        line("Клиент мессенджера MAX для Windows и Linux без телеметрии."),
                    ],
                    cx,
                ),
                Self::group(
                    "Что не отправляется",
                    vec![
                        line("Статистика и события использования"),
                        line("Отчёты о сбоях и трассировка"),
                        line("Журналы и отладочные отчёты"),
                        line("Проверка обновлений и A/B-эксперименты"),
                        line("Данные об оборудовании: идентификатор устройства случайный"),
                    ],
                    cx,
                ),
            ],
            cx,
        )
    }
}

/// Image addresses a message needs.
fn image_urls(m: &MessageRow) -> Vec<String> {
    m.attachments
        .iter()
        .filter_map(|a| match a {
            Attachment::Photo { url, .. } | Attachment::Sticker { url } => Some(url.clone()),
            Attachment::Video { thumbnail, .. } => thumbnail.clone(),
            // A sensitive preview stays unloaded until the user asks for it.
            Attachment::Share {
                image, sensitive, ..
            } if !sensitive => image.clone(),
            _ => None,
        })
        .collect()
}

/// `max.ru` links in the message that have no `SHARE` card of their own yet.
fn lottie_urls(m: &MessageRow) -> Vec<String> {
    m.marks
        .iter()
        .filter_map(|mark| match &mark.kind {
            rich::MarkKind::Animoji { url } => Some(url.clone()),
            _ => None,
        })
        .collect()
}

fn uncovered_max_links(m: &MessageRow) -> Vec<String> {
    rich::links(&m.marks)
        .into_iter()
        .filter(|link| rich::is_max_link(&link.url))
        .filter(|link| {
            !m.attachments.iter().any(
                |a| matches!(a, Attachment::Share { url, .. } if !rich::is_hidden(url, &link.url)),
            )
        })
        .map(|link| link.url)
        .collect()
}

/// "0:42", "12:05" or "1:02:03" for a length in ms.
fn media_duration(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
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

/// The part of a wheel notch the list applied beyond one frame.
///
/// The list moves by `-delta_y` immediately. The returned distance is what we
/// scroll back, then play out over the following frames. A small notch is `0`.
/// Where the database key and the login token are kept, in the language of the settings page.
fn local_store_note() -> &'static str {
    if cfg!(windows) {
        "Переписка хранится в базе SQLCipher; её ключ и токен входа лежат в Диспетчере учётных данных Windows и без вашей учётной записи не читаются."
    } else if cfg!(target_os = "linux") {
        "Переписка хранится в базе SQLCipher; её ключ и токен входа лежат в связке ключей системы (Secret Service) и без вашей сессии не читаются."
    } else {
        "Переписка хранится в базе SQLCipher; её ключ и токен входа лежат в защищённом хранилище системы."
    }
}

fn wheel_glide_extra(delta_y: f32, frame_px: f32) -> f32 {
    let applied = -delta_y;
    let keep = applied.signum() * applied.abs().min(frame_px);
    let extra = applied - keep;
    if extra.abs() < 0.5 { 0. } else { extra }
}

/// How far to move on this frame. A fraction of what is left, never less than
/// [`WHEEL_FRAME_PX`] until the tail, so 120 Hz does not turn into a crawl.
fn glide_step(remaining: f32) -> f32 {
    let mag = remaining.abs();
    if mag < 0.5 {
        return 0.;
    }
    let step = (mag * 0.35).max(WHEEL_FRAME_PX).min(mag);
    remaining.signum() * step
}

/// Positive pixels down the list. The scrollbar helper reports the same distance negated.
fn list_scroll_px(list: &ListState) -> f32 {
    -f32::from(list.scroll_px_offset_for_scrollbar().y)
}

/// Order, pin, mute and whether a badge is shown. Preview text is left out:
/// it does not change the row's height.
fn chat_rows_stamp(chats: &[ChatRow]) -> u64 {
    let mut stamp = chats.len() as u64;
    for chat in chats {
        stamp = stamp.wrapping_mul(67).wrapping_add(chat.id as u64);
        stamp = stamp
            .wrapping_mul(67)
            .wrapping_add(u64::from(chat.pin_index > 0));
        stamp = stamp.wrapping_mul(67).wrapping_add(u64::from(chat.muted));
        stamp = stamp
            .wrapping_mul(67)
            .wrapping_add(u64::from(chat.unread > 0));
    }
    stamp
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.history_origin = list_scroll_px(&self.history_list);
        self.chats_origin.set(list_scroll_px(&self.chats_list));
        self.history_glide_base = self.history_glide;
        self.chats_glide_base.set(self.chats_glide);
        self.sync_unread(window);
        match &self.screen {
            Screen::Status(s) => Self::centered(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_2xl().font_semibold().child("maxly-gpui"))
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
                let viewer = self.render_viewer(window, cx);
                div()
                    .size_full()
                    .relative()
                    .child(self.render_main(&me, &chats, selected, &status, cx))
                    .children(viewer)
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
    use super::{
        Delivery, History, MessageRow, WHEEL_FRAME_PX, day_label_in, glide_step, normalize_phone,
        short_time_in, wheel_glide_extra,
    };

    #[test]
    fn a_wheel_notch_keeps_one_frame_and_glides_the_rest() {
        // Three lines of 20px, toward older messages. The list jumps 60px;
        // one frame stays, 44px is given back to the glide.
        assert_eq!(wheel_glide_extra(60., WHEEL_FRAME_PX), -44.);
        assert_eq!(wheel_glide_extra(-60., WHEEL_FRAME_PX), 44.);
        assert_eq!(wheel_glide_extra(10., WHEEL_FRAME_PX), 0.);
        assert_eq!(wheel_glide_extra(0., WHEEL_FRAME_PX), 0.);
        // A short tail is applied whole. clamp would panic here: the floor is above the tail.
        assert_eq!(glide_step(-44.), -16.);
        assert_eq!(glide_step(-12.), -12.);
        assert_eq!(glide_step(0.2), 0.);
    }

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
            ..MessageRow::default()
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
mod chat_list_tests {
    use super::{ChatRow, upsert_chat};

    fn chat(id: i64, time: i64, preview: &str) -> ChatRow {
        ChatRow {
            id,
            title: format!("чат {id}"),
            preview: preview.into(),
            unread: 0,
            time,
            peer_mark: 0,
            ..ChatRow::default()
        }
    }

    #[test]
    fn changed_chat_keeps_its_preview_and_new_chat_goes_by_time() {
        let mut first = chat(1, 300, "привет");
        first.avatar = Some("https://i.oneme.ru/a".into());
        let mut chats = vec![first, chat(2, 100, "пока")];
        let mut renamed = chat(1, 200, "");
        renamed.title = "Новое имя".into();
        upsert_chat(&mut chats, renamed);
        assert_eq!(chats[0].title, "Новое имя");
        assert_eq!((chats[0].preview.as_str(), chats[0].time), ("привет", 300));
        assert_eq!(
            chats[0].avatar.as_deref(),
            Some("https://i.oneme.ru/a"),
            "a change without a photo keeps the old one"
        );
        upsert_chat(&mut chats, chat(3, 150, ""));
        let ids: Vec<i64> = chats.iter().map(|c| c.id).collect();
        assert_eq!(ids, [1, 3, 2]);
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
            ..MessageRow::default()
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
        h.marked = 3;
        let mut mine = row(4, Delivery::Sent);
        mine.out = true;
        h.messages.push(mine);
        assert_eq!(h.unread_mark(), None, "our own message needs no read mark");
    }
}

#[cfg(test)]
mod compose_tests {
    use max_core::media::Attachment;

    use super::{Delivery, History, MessageRow, Quote, editable, quote_of};

    fn ours(id: i64, text: &str) -> MessageRow {
        MessageRow {
            id,
            out: true,
            text: text.into(),
            time: 1_000,
            ..MessageRow::default()
        }
    }

    #[test]
    fn confirmed_reply_keeps_the_pending_quote() {
        let quote = Quote {
            id: 5,
            sender: "Анна".into(),
            text: "вопрос".into(),
        };
        let mut h = History::default();
        h.add(MessageRow {
            cid: Some(9),
            delivery: Delivery::Pending,
            reply: Some(quote.clone()),
            ..ours(0, "ответ")
        });
        h.add(MessageRow {
            cid: Some(9),
            ..ours(6, "ответ")
        });
        assert_eq!(h.messages.len(), 1);
        assert_eq!(
            (h.messages[0].id, h.messages[0].reply.clone()),
            (6, Some(quote))
        );
    }

    #[test]
    fn an_edit_changes_text_and_keeps_what_it_leaves_out() {
        let mut h = History::default();
        h.add(MessageRow {
            attachments: vec![Attachment::File {
                file_id: 1,
                name: "a.pdf".into(),
                size: 1,
            }],
            reply: Some(Quote::default()),
            ..ours(6, "старый")
        });
        h.edit(MessageRow {
            edited: true,
            time: 0,
            ..ours(6, "новый")
        });
        let m = &h.messages[0];
        assert_eq!((m.text.as_str(), m.time, m.edited), ("новый", 1_000, true));
        assert_eq!(m.attachments.len(), 1);
        assert!(m.reply.is_some());
        h.edit(ours(99, "чужой id"));
        assert_eq!(h.messages[0].text, "новый", "unknown ids change nothing");
    }

    #[test]
    fn only_our_sent_plain_messages_are_editable() {
        assert!(editable(&ours(1, "текст")));
        assert!(!editable(&ours(0, "отправка")), "pending");
        assert!(!editable(&MessageRow {
            out: false,
            ..ours(1, "текст")
        }));
        assert!(!editable(&MessageRow {
            forwarded: Some("Канал".into()),
            ..ours(1, "текст")
        }));
        assert!(!editable(&MessageRow {
            attachments: vec![Attachment::Other {
                kind: "VIDEO".into()
            }],
            ..ours(1, "[Видео] текст")
        }));
    }

    #[test]
    fn quotes_name_the_sender() {
        assert_eq!(quote_of(&ours(1, "да"), "Анна").sender, "Вы");
        let post = MessageRow {
            out: false,
            ..ours(2, "новость")
        };
        assert_eq!(
            quote_of(&post, "Канал").sender,
            "Канал",
            "channel posts have no sender"
        );
    }
}

#[cfg(test)]
mod duration_tests {
    use super::media_duration;

    #[test]
    fn durations() {
        assert_eq!(media_duration(0), "0:00");
        assert_eq!(media_duration(42_000), "0:42");
        assert_eq!(media_duration(725_400), "12:05");
        assert_eq!(media_duration(3_723_000), "1:02:03");
    }
}

#[cfg(test)]
mod notification_tests {
    use super::{ChatKind, ChatRow, MessageRow, notification_text};

    fn chat(kind: ChatKind) -> ChatRow {
        ChatRow {
            id: 1,
            title: "Семья".into(),
            kind,
            ..ChatRow::default()
        }
    }

    fn msg(sender: &str, text: &str) -> MessageRow {
        MessageRow {
            id: 5,
            sender: sender.into(),
            text: text.into(),
            ..MessageRow::default()
        }
    }

    #[test]
    fn titles_senders_and_hidden_text() {
        let group = chat(ChatKind::Group);
        assert_eq!(
            notification_text(&group, &msg("Мама", "Купи\nхлеба"), true),
            ("Семья".to_string(), "Мама: Купи хлеба".to_string())
        );
        let dialog = chat(ChatKind::Dialog);
        assert_eq!(
            notification_text(&dialog, &msg("Семья", "Привет"), true).1,
            "Привет"
        );
        assert_eq!(
            notification_text(&group, &msg("Мама", "секрет"), false).1,
            "Новое сообщение"
        );
        let long = notification_text(&dialog, &msg("", &"я".repeat(500)), true).1;
        assert_eq!(long.chars().count(), 200);
        assert!(long.ends_with('…'));
    }
}
