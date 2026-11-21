# Decisions

## ADR-001: Project location and reference material
Status: accepted (2026-11-19)
Context: The official client is installed in `C:\MAX` (binaries only, 741 MB).
Decision: The project lives in `C:\dev\maxly-gpui` (git). `C:\MAX` is a read-only reverse
engineering target; nothing is written into it. Ghidra snapshots and outputs stay in `re/`.
Consequences: Every RE command reads `C:\MAX` and writes under `C:\dev\maxly-gpui\re`.

## ADR-002: Reverse engineering of the official client is the source of truth
Status: accepted (2026-11-19)
Context: The goal is feature parity with the official desktop client. PyMax (MIT) implements
the protocol but impersonates the Android client and lags behind the desktop build.
Decision: Protocol, payloads and behavior come from the official desktop client: Ghidra
decompilation of `C:\MAX` (via `rea`) and captures of its own decrypted traffic. PyMax is used
only as a hint when the binary is unclear, and every such fact is verified against the binary
or a capture before it is relied on. No PyMax code is ported.
Consequences: rust-anything cannot port a compiled C++ client (it needs source), so its role
narrows to a characterization harness where it fits; the main proof is replay of recorded
official-client traffic against the Rust client. Slower start, but the client matches what the
server expects from a desktop.

## ADR-003: Present as the official desktop client
Status: accepted (2026-11-19)
Context: The server sees the `SESSION_INIT` user agent, protocol version and behavior.
Decision: The client presents itself as MAX Desktop for Windows: `deviceType`, `appVersion`,
`buildNumber`, protocol `ver` and the other user-agent fields take the values the official
desktop build sends (currently 26.41.0.83760), read from the binary or a capture.
Consequences: The identity must track official releases; a version table is kept in one place.
The user accepts the account risk of running an unofficial client.

## ADR-004: Minimal fixed telemetry stream
Status: superseded by ADR-005 (2026-11-19)
Context: The official client sends statistics over the main socket (`LOG` opcode) through
`business/statistic/events_sender`; a client that sends none may be flagged. Real events would
leak behavior (screens, chats opened, clicks).
Decision: Send a minimal, fixed stream in the official format: only what the desktop client
must send to look normal (to be determined from RE, e.g. login/start events), with constant or
coarse values, never derived from real user actions, chats or contacts. No crash reports, log
upload, Tracer SDK, health check on third-party domains, or auto-update calls.
Consequences: Telemetry lives in one isolated module with a test that asserts its payloads
contain no user-derived data and that no other code path sends `LOG`.

## ADR-005: No telemetry by default; minimal stream only if the client stops working without it
Status: accepted (2026-11-19), supersedes ADR-004
Context: The user prefers no telemetry at all, as long as the client keeps working. No evidence
yet that the server needs `LOG` events: PyMax added fake telemetry in its v2 rewrite
(commit 811b3c9, 2026-05-23) with no stated reason, and its only ban-like report
(issue 22, `error.user.restricted.send`) has no known cause.
Decision: The client sends no telemetry: no `LOG` opcode, no crash reports, log upload, Tracer
SDK, third-party health checks or auto-update calls. The minimal fixed stream of ADR-004 is
kept as an isolated module behind a cargo feature `telemetry-minimal`, off by default, and is
enabled only if RE or testing shows that something breaks without it (server config that
requires stats, restrictions on accounts that send none).
Consequences: RE must check whether any server response or config depends on statistics.
Testing without telemetry runs on a test account, never the user's main account, and watches
for `error.user.restricted.*` errors over several days. A test asserts that no code path sends
`LOG` unless the feature is on.

## ADR-006: Release split v1 / v2 / v3
Status: accepted (2026-11-19)
Context: About 110 features of the official client, mapped in `MAP.md` with binary evidence.
Decision: Ship in three releases as proposed in `MAP.md`: v1 messaging core (connection, QR /
SMS / 2FA login, chat list, send/edit/delete/reply/forward/reactions, photos, files, video,
stickers, contacts, basic settings, notifications, tray); v2 full chat features (folders,
archive, pins, delayed messages, comments, transcription, voice and video message recording,
group and channel management, mini-apps, stories viewing, polls); v3 calls, story publishing,
livestreams, media editor, multiaccount, ESIA. Recommendations (C10, F12) are not built.
Consequences: Units are ported in waves that serve v1 first; v2 and v3 rows stay `open` until
v1 is done. Calls stay a separate track that starts only after RE of the OK calls SDK.

## ADR-007: UI on gpui with longbridge/gpui-kit
Status: accepted (2026-11-19)
Context: The user chose gpui-kit (github.com/longbridge/gpui-kit, formerly gpui-component;
version 0.10 on 2026-11-19) for the UI. It provides the parts a messenger needs: `message`,
`bubble`, `message_scroller`, `virtual_list`, `avatar`, `sidebar`, `input`, `attachment`,
`notification`, `dialog`, `menu`, `title_bar`, `tab`, `setting`, and the `gpui-webview` crate.
Decision: The `app` crate is a gpui application built with gpui-kit components; the official
QML screens (`re/out/qml_files.txt`) are the reference for layout and behavior, not for code.
Consequences: The UI depends on gpui and its platform backends (Windows first). Gaps to fill
outside gpui-kit: Lottie/thorvg stickers and animoji, video and audio playback, the QR code
view, tray icon and system notifications. Mini-apps (I3) use `gpui-webview`. The pinned
gpui-kit and gpui revisions and the minimum Rust version are set when the `app` crate starts.

## ADR-008: Device identity without hardware fingerprinting
Status: accepted (2026-11-19)
Context: The official client derives `deviceId` from a hardware fingerprint
(`desktop_utils.dll` `Utilities::getDeviceId`): `base64(SHA3-224("{}|{}|{}|{}|{}|{}"))` over the
registry `UninstallId` (or a new GUID), the volume serial folded to 16 bits, the physical disk
serial (`IOCTL_STORAGE_QUERY_PROPERTY`), all non-loopback MAC addresses, CPUID vendor, signature
and feature bits, and the computer name. The server cannot check how the hash was made, and the
official `deviceId` itself changed between the first and second launch on this machine.
`deviceName` is the real computer name, which often contains the owner's name.
Decision: `deviceId` is 28 random bytes from the OS RNG, base64 with padding (40 characters,
the same shape), generated once per profile and kept. `deviceName` defaults to a random
Windows-style name (`DESKTOP-` + 7 characters of `A-Z0-9`), kept per profile; the user may
choose the real computer name or any text. `timezone` and `locale` come from the system by
default (the server uses them for content) and can be overridden. `clientSessionId` is a
per-profile counter, +1 per launch, starting at 1 (client log: 1, 2, 3 over three launches).
Consequences: No hardware data leaves the machine. Two installs on one machine look like two
devices. `max-identity` has no code that reads disks, adapters or CPUID.

## ADR-009: Local storage encrypted with SQLCipher, keys protected by DPAPI
Status: accepted (2026-11-19)
Context: The client keeps chats, messages, the identity (ADR-008) and a login token on disk.
The official client stores its data with SQLCipher (`storage/backends/sqlcipher`).
Decision: One SQLCipher database per profile. Its 256-bit key is random, generated on first
run, and kept in the Windows user's protected store (DPAPI), never in a file next to the
database. The login token is kept in the same protected store, not in the database.
Consequences: Data is unreadable without the Windows user's logon secrets; copying the profile
folder to another account or machine does not reveal it. Losing the protected entry loses the
local cache (the account itself is unaffected: log in again). Other platforms use their own
keystore behind the same interface later.

## ADR-010: Pinned UI stack
Status: accepted (2026-11-19)
Context: ADR-007 left the gpui-kit and gpui revisions to be fixed when the `app` crate starts.
gpui-kit pins GPUI to one `gpui-pre` snapshot, and any snapshot may change GPUI's API.
Decision: `gpui-kit = "=0.10.0"` from crates.io, which brings `gpui-pre =0.3.12`. The core runs
on its own tokio runtime; events reach the UI through `tokio::sync::mpsc`, which works on
GPUI's executor. Upgrades bump gpui-kit and its GPUI snapshot together, in a commit of their
own, after `--demo-walk` screenshots look the same.
Consequences: Rust 1.97 builds it on Windows (MSVC) with no extra tools. GPUI's `test`
attribute macro shadows `#[test]` under `use gpui_kit::*`: test modules import only what they
need.

## ADR-011: Trust the Russian Trusted Root CA for MAX hosts only
Status: accepted (2026-11-19)
Context: `api2.oneme.ru` presents `*.oneme.ru` (MAX LLC) issued by "Russian Trusted Sub CA",
issued by "Russian Trusted Root CA" of the Ministry of Digital Development. That root is not in
the Mozilla store (webpki-roots) nor in this machine's Windows stores, so rustls refuses the
server (`UnknownIssuer`). The official client ships the root itself: Qt resources
`:/certs/russiantrustedca.pem` and `:/certs/russiantrustedca2024.pem` in `network.dll` (both the
same certificate, SHA-256 `D2:6D:2D:02:31:B7:C3:9F:92:CC:73:85:12:BA:54:10:35:19:E4:40:5D:68:B5:
BD:70:3E:97:88:CA:8E:CF:31`, valid 2022-03-01 to 2032-02-27), checked by
`McCerts::VerifyCertificateChain` and installed into its own `QSslConfiguration`.
Decision: For hosts `oneme.ru` and `*.oneme.ru` the client trusts exactly this root, taken
from the official client and embedded in `max-transport`, and nothing else. Other hosts keep the
Mozilla roots. The root is never added to the Windows store.
Consequences: A certificate for a MAX host from any other CA is refused, which is stricter than
the official client. The root expires in 2032; when the server moves to the 2024 root or another
one, the embedded set is updated from the then current official client.
Amendment (2026-11-20): a live run showed `i.oneme.ru` (photos) failing with `UnknownIssuer`
under the Russian root alone: media hosts use certificates from other CAs. MAX hosts now accept
a chain to the Russian root or to a Mozilla root (`webpki-roots`), which the official client
evidently allows too (it shows the same photos). Hosts outside `*.oneme.ru` still get the
Mozilla roots only, so the Russian root is never trusted for them.
