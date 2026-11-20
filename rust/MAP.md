# maxrs: port map

**Goal.** A telemetry-free MAX desktop client in Rust with a gpui-kit UI, at feature parity
with the official Windows client 26.41.0.83760 (`C:\MAX`). The official client is the source of
truth (ADR-002); it is reverse engineered, not ported line by line.

Read first: `DECISIONS.md`, `GLOSSARY.md`, `research/protocol-and-telemetry.md`,
`research/opcodes-desktop.md`, `research/server-config-keys.md`, `../re/README.md`.

## Decisions (index)

| ADR | Decision |
|---|---|
| 001 | Project in `C:\dev\maxrs`; `C:\MAX` read-only RE target |
| 002 | RE of the official client is the source of truth; PyMax only as a hint, no PyMax code |
| 003 | Present as MAX Desktop for Windows 26.41.0.83760 |
| 004 | (superseded by 005) |
| 005 | No telemetry by default; minimal fixed stream behind `telemetry-minimal`, off |
| 006 | Release split v1 / v2 / v3 as in the feature table; no recommendations |
| 007 | UI: gpui + longbridge/gpui-kit; official QML screens as reference |
| 008 | Random deviceId and device name instead of a hardware fingerprint |
| 009 | SQLCipher database per profile; DB key and login token in the DPAPI-protected store |
| 010 | UI stack pinned: gpui-kit =0.10.0 (gpui-pre 0.3.12); core on its own tokio runtime |
| 011 | MAX hosts (*.oneme.ru) trust only the Russian Trusted Root CA bundled by the official client |

## Feature parity table

Evidence: **Q** = QML files embedded in `max.exe` (`re/out/qml_files.txt`, 797 files),
**S** = build source paths (`re/out/srcpaths_*.txt`), **O** = opcodes
(`research/opcodes-desktop.md`), **K** = server config keys (`research/server-config-keys.md`).

Release (ADR-006): **v1** messaging core, **v2** full chat features,
**v3** calls and rare features, **no** = excluded. State: `open`, `claimed by <session> <date>`,
`done`.

### A. Connection and session

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| A1 | TCP/TLS to `api2.oneme.ru:443`, frame codec (ver 11, lz4 from server) | O 6; capture | v1 | done (`max-proto`, `max-transport`); not yet tried against the real server |
| A2 | `SESSION_INIT` with desktop user agent, `deviceId`, `clientSessionId` | O 6; capture | v1 | done: packet (`max-proto`), values (`max-identity`: version table, random deviceId and name, OS version, language, time zone, launch counter); stored per profile (`max-store`) |
| A3 | PING every 30 s `{interactive}`, reconnect, request timeouts | O 1; config `ping_interval` | v1 | ping, timeouts, close detection, backoff done (`max-transport`); reconnect with SESSION_INIT and LOGIN again done (`max-core::Supervisor`) |
| A4 | Failover hosts from server config | K `failover-hosts` | v1 | open |
| A5 | Login sync (chats, contacts, config on `LOGIN`) | O 19, 22 | v1 | LOGIN with saved token and initial sync done (`max-core`); sync markers stay 0 until their update rule is decoded |
| A6 | Server config flags, local feature gates | K (184 keys); O 134 `ConfigChangeData` | v1 | open |
| A7 | Server push events dispatch | O 128-165, 216, 243, 277, 293 (CommonEvent) | v1 | pushes delivered raw (`CoreEvent::Push`); typed dispatch open |
| A8 | Network change handling | S `network_change_notifier` | v1 | open |
| A9 | Upload/download HTTP endpoints, URL refresh | O 80, 82, 87, 88, 203; K `desktop-upload-url-failover`, `photo-url-refresh` | v1 | open |

### B. Account and authorization

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| B1 | QR login (create, poll, login by track) | O 288, 289, 291; Q `QrCodePage` | v1 | done in core and UI (QR drawn from the link, refreshed on expiry); not tried on the real server |
| B2 | Phone + SMS code login | O 17, 18, 23; Q `PhoneNumberPage`, `CodeEnterPage` | v1 | done in core and UI (phone, code, resend, other number, server error text); not tried on the real server; see open question 6 |
| B3 | Two-factor password: login check, restore, hint | O 101, 104, 113, 115; Q `PasswordEnterPage`, `AuthForgotPasswordPage` | v1 | login check done (`check_password`); restore and hint open |
| B4 | Registration: name, avatar presets | O 25; Q `NameEnterPage`, `AvatarPreset*` | v1 | detected (`AuthStep::RegistrationRequired`); flow open |
| B5 | 2FA setup and management (email, hint, password) | O 109-112; Q `SFACreate*`, `SFAConfig*` | v2 | open |
| B6 | Active sessions list and close | O 96, 97; Q `DevicesSettings*` | v1 | open |
| B7 | Logout | O 20 | v1 | open |
| B8 | Profile delete and cancel | O 116, 199, 200; Q `RemoveProfileDialog`, `CancelProfileDeletePopup` | v2 | open |
| B9 | Multiaccount | K `multiaccount`; Q `AccountsStackView`, `MultiaccountSectionButton` | v3 | open |
| B10 | Callout login (call-based code) | O 294; Q `CalloutPage` | v2 | open |

### C. Chat list, folders, search

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| C1 | Chat list with previews, counters, presence | O 53, 35, 132; Q `ChatListView`, `ChatDelegate`, `PresenceMarker` | v1 | open |
| C2 | Mark read / unread | O 50, 130; K `readmark-validation-disabled` | v1 | open |
| C3 | Typing indicators (send and show) | O 65, 129; K `typing-send-enabled`; Q `TypingControl` | v1 | open |
| C4 | Search: chats, messages, public, by phone | O 60, 68, 73, 46; Q `SearchPanel`, `SearchByPhoneDialog` | v1 | open |
| C5 | Local search | K `local-search-ranker`, `recent-search-enabled` | v2 | open |
| C6 | Folders: list, edit, reorder, delete, filters | O 272-277; K `folder-*`, `choose-default-folder`; Q `SettingsFolders*` | v2 | open |
| C7 | Archive | K `folder-archive`; Q `ArchiveListView`, `LeadingArchiveBar` | v2 | open |
| C8 | Chat personal config (mute, pin chat) | O 61 | v1 | open |
| C9 | Separate chat windows | K `chat-windows`; Q `ChatWindow`, `ChatWindowsManager` | v2 | open |
| C10 | Chat suggestions | O 300; Q `ChatSuggestHeader` | no (recommendations) | open |

### D. Messages

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| D1 | History load and scroll, day dividers, new-messages marker | O 49, 71; Q `HistoryListView`, `NewMessagesPanel` | v1 | open |
| D2 | Send text, formatting, links, mentions | O 64; Q `MessageTextInput`, `AddFormatLinkDialog`, `MentionContactList` | v1 | open |
| D3 | Edit, delete (for me / for all), delete range | O 66, 67, 140, 142; K `delete-msg-fys-large-chat-disabled` | v1 | open |
| D4 | Reply, forward, quote navigation | Q `Reply*`, `MessageForwardDialog`; K `reply-navigation-highlight` | v1 | open |
| D5 | Reactions: set, cancel, list, detailed | O 178-181, 155, 156; Q `ReactionPlate`; K `reactions-rank-config` | v1 | open |
| D6 | Pinned messages and multipin | O 240-243, 86; K `multipin-*`; Q `PinBar`, `PinnedMessagesPage` | v2 | open |
| D7 | Delayed (scheduled) messages | O 154; Q `DelayedMessages*` | v2 | open |
| D8 | Silent messages, confirm before send | K `silent-messages`, `confirm-send` | v2 | open |
| D9 | Link previews | O 89 | v1 | open |
| D10 | Message info, delivery state, message stats (views) | O 74; Q `MessageInfoCapsule`; K `forwarded-post-views` | v2 | open (views reporting: decide, see ADR-005) |
| D11 | Comments under channel posts | O 91; K `comments-*`; Q `Comments*` | v2 | open |
| D12 | Audio/video message transcription | O 202, 293; K `enable-*-transcription`, `retry-transcribe-*` | v2 | open |
| D13 | Multiselect actions, copy restrictions | Q `DelayedMultiselectActionsPanel`, `MessagesActionPanel` | v1 | open |
| D14 | Message search inside a chat | Q `MessagesSearchPanel` | v1 | open |
| D15 | Text selection, clipboard (HTML) | K `message-text-selection`, `desktop-clipboard-html-enabled` | v1 | open |
| D16 | Spell check | K `spell-check-win`; S `spell_check` | v2 | open |

### E. Attachments and media

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| E1 | Photo send/view, image quality, WebP | O 80; K `image-quality`, `image-width`, `webp-config`, `desktop-webp-conversion` | v1 | open |
| E2 | Files send/download, retries, download path | O 87, 88; K `desktop-attachmnets-upload-retries`; Q `FileAttachmentItem` | v1 | open |
| E3 | Video send and playback (embedded, speed) | O 82, 83; K `desktop-embed-video`, `video-speed`, `oneme-media-player` | v1 | open |
| E4 | Voice messages play and record, waveform | O 301; K `audio-msg-send-enabled`; Q `AudioAttachmentDelegate`, `InputAudioRecorderPanel` | v1 (play), v2 (record) | open |
| E5 | Video messages (circles) play and record | Q `VideoMessage*`, `InputVideoRecorderPanel`; K `ov-media-send-enabled` | v2 | open |
| E6 | Fullscreen gallery | Q `FullscreenGalleryWindow`; S `fsg_gallery` | v1 | open |
| E7 | Media editor (photo, video) | Q `FullscreenMediaEditorWindow`, `MeMediaEditor*`; K `edit-media-*` | v3 | open |
| E8 | Stickers, sticker sets, favorites, suggest | O 26-29, 259, 261; Q `StickersView`, `EmojiStickerPicker` | v1 (view, send), v2 (manage) | open |
| E9 | Emoji, animoji (Lottie/thorvg) | Q `MeAnimojis*`, `EmojiView`; K `desktop-thorvg-enable`; S `lottie_player` | v1 (emoji), v2 (animoji) | open |
| E10 | Location bubble (static map) | Q `GeoPositionBubbleContent`; `static-maps.yandex.ru` | v2 | open (map provider: decide) |
| E11 | Contact share | Q `ContactBubbleContent`; K `contact-share-enabled` | v2 | open |
| E12 | Chat media, files, audio, links tabs | O 51; Q `ChatMediaPage`, `ChatFilesPage`, `ChatAudioMessagesPage`, `ChatSharesPage` | v1 | open |
| E13 | Send from cache, media proxy | K `media-send-from-cache`, `media-proxy-enabled` | v2 | open |

### F. Groups and channels

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| F1 | Create group/channel | Q `NewChatDialog`, `SidebarChannelCreatePrivacyPanel` | v2 | open |
| F2 | Members: list, add, remove, update | O 59, 77; Q `ChatParticipantsPage`, `AddParticipantsPage` | v2 | open |
| F3 | Admins and permissions | Q `AdminPermissionsPage`, `ChatPermissionsPage`, `ChannelPermissionsPage` | v2 | open |
| F4 | Invite links: list, create, delete; join by link | O 221-223, 56, 57; Q `SidebarInviteByLinkPage` | v2 | open |
| F5 | Join requests | Q `JoinRequestsPage`, `PinnedChannelJoinRequest` | v2 | open |
| F6 | Leave, delete, clear chat | O 52, 58 | v1 | open |
| F7 | Chat/channel info and settings sidebar | O 48, 55; Q `Sidebar*` | v1 (view), v2 (edit) | open |
| F8 | Reactions settings per chat | O 257, 258; Q `ReactionsSettings*` | v2 | open |
| F9 | Chat themes | Q `ChatThemeButton`; S `chat_theme` | v3 | open |
| F10 | Common chats | O 198; Q `CommonChatsPage` | v2 | open |
| F11 | External chat restrictions | O 204 | v2 | open |
| F12 | Channel recommendations | K `channel-recommendations*`; Q `ChannelRecommendations*` | no (recommendations) | open |
| F13 | Livestreams | O 62; K `live-streams`; Q `LiveStream` | v3 | open |
| F14 | P2G blacklist, comments blacklist | K `blacklist`; Q `P2GBlacklist*`, `CommentsBlacklist*` | v2 | open |

### G. Contacts

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| G1 | Contact list, info, presence | O 32, 35, 36; Q `ContactListView` | v1 | open |
| G2 | Add (create), update, rename | O 34, 41; Q `CreateContactDialog`; K `contact-add-bottom-sheet` | v1 | open |
| G3 | Contact photos | O 39, 43 | v2 | open |
| G4 | Block / unblock, blocked list | Q `SettingsPrivacyBlockedContactsList`, `BlockedPlaceholder` | v1 | open |
| G5 | Unknown-contact warnings | K `enable-unknown-contact-bottom-sheet`; Q `NonContactPlaceholder` | v2 | open |

### H. Profile and settings

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| H1 | Own profile edit, avatar | O 16; Q `SettingsUserProfile*`, `SettingsNewAvatar` | v1 | open |
| H2 | Avatar creator | Q `AvatarCreatorComponent_*` | v3 | open |
| H3 | Privacy settings | Q `SettingsPrivacy*`, `PrivacySettingsPage` | v1 | open |
| H4 | Notification settings | K `notification-settings`, `notification-in-call-setting` | v1 | open |
| H5 | Appearance: theme, fonts, font size | K `font-size-slider`, `glass-enabled`; Q `SettingsAppearance`, `SettingsFonts` | v1 | open |
| H6 | Data and memory (cache, download path) | Q `DataAndMemorySettings`, `SettingsDownloadPathButton` | v1 | open |
| H7 | Keyboard shortcuts | Q `SettingsShortcuts`; S `shortcuts` | v2 | open |
| H8 | Language | Q `LanguageMenuButton`; S `app_locale` | v1 | open |
| H9 | Mini-app permissions | Q `SettingsPrivacyMiniApp*` | v2 | open |
| H10 | About, FAQ, support bot | Q `SettingsAbout`, `SettingsFaq` | v1 | open |
| H11 | ESIA (Gosuslugi) verification cell | Q `EsiaVerifySettingsCell` | v3 | open (needs RE) |

### I. Bots and mini-apps

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| I1 | Bot commands, bot info | O 144, 145; Q `BotCommandList`, `BotPlate` | v1 | open |
| I2 | Inline and reply keyboards, callbacks | O 118; S `keyboards` | v1 | open |
| I3 | Mini-apps (WebView), init data, phone hash | O 160, 106; K `webapp-*`; Q `WebViewWindowModule_*` | v2 (gpui-webview) | open |

### J. Stories

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| J1 | View stories (list, by owner, mark, react) | O 208-210, 213, 214, 216, 219, 220; Q `FsgStoryModule_*`, `StoriesListView` | v2 | open |
| J2 | Publish, edit, delete, stats | O 211, 212, 217, 218; K `stories-publish` | v3 | open |

### K. Polls

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| K1 | View and vote, voters, updates | O 304-306; Q `MePollsModule_*` | v2 | open |
| K2 | Create poll | Q `PollCreatePage` | v2 | open |

### L. Calls (separate track)

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| L1 | Calls history, call links | O 163-165, 84; Q `CallsListView`, `CallLinkCell` | v3 | open |
| L2 | P2P and group calls (audio, video, screen share, hold) | O 76-79, 158, 166; K `calls-*`, `call*`, `p2p-*`, `p2g-*`, `gc*`; `max-service.exe`, `libcall-service.dll`, `calls.okcdn.ru` | v3 | open (needs RE of the OK calls SDK) |
| L3 | Incoming call card, vestibule, post-call rating | Q `IncomingCallModule_*`, `CallVestibuleModule_*`, `PostCallModule_*` | v3 | open |

### M. Desktop integration

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| M1 | System notifications, unread badge | S `notifications`; Q `NotificationsModule_*` | v1 | open |
| M2 | Tray icon | Q `TrayIcon` | v1 | open |
| M3 | Single instance, `max://` deep links, local pipe | config `single_instance_id`, `internal_url_scheme`, `local_server_pipe_name` | v1 | open |
| M4 | Drag and drop, paste files | Q `HistoryDropArea` | v1 | open |
| M5 | Window state, title bar | S `window_state` | v1 | open |

### N. Organizations and safety

| # | Feature | Evidence | Release | State |
|---|---|---|---|---|
| N1 | Organization info, placeholders | O 256, 308; K `org-info-cache-ttl`, `organization-placeholder` | v2 | open |
| N2 | Dangerous link and file warnings | K `warning-links`; S `unsafe_*` | v1 | open |
| N3 | Complaints | O 161, 162 | v2 | open |
| N4 | User restriction notices | Q `UserRestrictionNotifierBasePopup` | v1 | open |

### Excluded (ADR-005 and privacy)

| Feature | Evidence | Why |
|---|---|---|
| Statistics, NAV/PERF events (`LOG`) | O 5; S `statistic/*` | ADR-005 |
| Calls statistics | S `calls_statistic*` | ADR-005 |
| Tracer SDK, crash dumps, crash-free reports | `tracernative.dll`, `crashpad_handler.exe` | ADR-005 |
| Log upload, `user-debug-report` | config `logs_sending_*`; K `user-debug-report` | ADR-005 |
| Health check on `r-push.org` | config `api_fallback_url` | ADR-005 (third party) |
| Auto-update (WinSparkle, `download2.max.ru`) | `WinSparkle.dll`, K `update-*` | ADR-005 |
| A/B experiment events | K `ab-status`; S `ab_event` | ADR-005 |
| Dev toggles, design-system demo | K `dev-toggles-enabled`; Q `SettingsDev`, `SettingsDesignSystem` | internal |

## Units (Rust)

| Unit | Covers | Wave | State |
|---|---|---|---|
| `max-proto` | frame codec, lz4, msgpack, opcode enum, packet types | 1 | done 2026-11-19: 30 tests (unit + capture headers), clippy -D warnings, fmt, no unsafe |
| `max-transport` | TLS socket, seq/response matching, ping, reconnect, failover | 1 | done 2026-11-19: 15 tests (fake server over duplex, local TLS with rcgen), 5 stable runs, clippy -D warnings, fmt, no unsafe; reconnect loop and failover hosts left to `max-core` |
| `max-identity` | user agent, device id, version table | 1 | done 2026-11-19: 12 tests, clippy -D warnings, fmt, no unsafe, no hardware probing (ADR-008) |
| `max-store` | encrypted local DB, token in OS keystore | 2 | done 2026-11-19: SQLCipher 4.20 (vendored OpenSSL), key and token in Credential Manager (DPAPI), profile with Identity and launch counter; 9 tests + 1 OS store test (run once, cleaned up); chat and message tables come with `max-core` |
| `max-core` | session, auth flows, sync, event dispatch, server config | 2 | done 2026-11-19: Session (SESSION_INIT, QR, SMS, 2FA password, LOGIN), Supervisor (reconnect, re-login), 6 flow tests on a scripted server; typed push dispatch, sync markers, logout open |
| `max-media` | uploads/downloads, image/video/audio decode, Lottie | 3 | open |
| `app` | gpui-kit UI, desktop integration | 3 | started 2026-11-19: window, QR, phone and code, 2FA password, chat list from LOGIN (local times), `--demo` and `--demo-walk` without network; message history, sending, settings open |

## Open questions

1. ~~Release split~~ accepted: ADR-006.
2. Channel view counts (D10) and stories marks: report or not (they are visible to authors)?
3. Map provider for location bubbles (E10): Yandex static maps as the official client, or none.
4. Platforms after Windows (open); ~~storage encryption~~ accepted: ADR-009.
5. Where Tracer sends from (not through OpenSSL in the pre-login capture).
6. Phone login with the desktop identity: the official desktop client offers it (`PhoneNumberPage`, `CodeEnterPage`, `CalloutPage`), so the server should accept it, possibly sending the code to the MAX app instead of SMS. If a test account shows the server refusing it for desktops, consider presenting as the Android client for login only; that needs RE of the Android APK (user agent, protocol version 10, opcodes) and a new ADR, since it contradicts ADR-003.

## Out of scope

- Porting PyMax code (ADR-002).
- MAX-ROOMS (`DESKTOP_ROOMS`) variant.
