# Decisions

## ADR-001: Project location and reference material
Status: accepted (2026-11-19)
Context: The official client is installed in `C:\MAX` (binaries only, 741 MB).
Decision: The project lives in `C:\dev\maxrs` (git). `C:\MAX` is a read-only reverse
engineering target; nothing is written into it. Ghidra snapshots and outputs stay in `re/`.
Consequences: Every RE command reads `C:\MAX` and writes under `C:\dev\maxrs\re`.

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
