# MAX protocol and telemetry: what the official desktop client does

Sources: the installed client in `C:\MAX` (2026-11-18 build) analyzed with string scans and
Ghidra 12.1.4 via `rea`; PyMax 2.5.0 (`ref/pymax`, commit c4cb917, MIT). Each claim names its
source. "Unverified" means the claim still needs Ghidra decompilation or a traffic capture.

## Transport

| Fact | Value | Source |
|---|---|---|
| Host, port | `api2.oneme.ru:443`, TLS | `config.dll` default JSON (`ssl_url2`, `ssl_port`, `use_encrypted_connection`) |
| API version | `api_version_uint: 11` | `config.dll` |
| Ping | every 30 000 ms; `check_packets_interval` 30 000 | `config.dll` |
| Upload chunk | 10 485 760 bytes | `config.dll` (`chunk_size`) |
| Serialization | msgpack (`msgpack_serializer`, `msgpack_extension_serializers`) | `core.dll` strings |
| Compression | lz4 only; no zstd library strings in `core.dll` or `network.dll` | `core.dll` strings |
| Fallback | `https://r-push.org/desktop/healthcheck.json` (`api_fallback_url`), driven by `business/session/api_health_checker.cpp` | `config.dll`, source paths |
| WebSocket (web client) | `wss://api.oneme.ru/websocket`, JSON | PyMax `config.py:256` |

### Frame (TCP)

PyMax `protocol/tcp/framing.py`: header `>BBHHI`, 10 bytes, big endian.

| Field | Type | Notes |
|---|---|---|
| ver | u8 | desktop sends **11** (captured); PyMax sends 10 (Android identity) |
| cmd | u8 | 0 request, 1 response, 2 event, 3 error (`protocol/enums.py`) |
| seq | u16 | request/response correlation |
| opcode | u16 | see PyMax `protocol/enums.py` (~190 opcodes) |
| packed_len | u32 | top byte = flags, low 24 bits = payload length |

Flags: `0` plain, `0xFF` zstd, any other non-zero = lz4 block (PyMax `payload.py:122-140`).
Captured: the server sends lz4 with flags 1 or 2 to the desktop; no zstd seen and no zstd
code in the desktop binaries.
msgpack ext type 1 wraps a nested msgpack value (`payload.py:14`).

## Opcodes

Full desktop table: `opcodes-desktop.md` (157 opcodes from `CommonPacket<opcode, Request,
Response, flag>` / `CommonEvent<...>` template names in `core.dll`; 21 not in PyMax, e.g.
pinned messages 240-243, chat links 221-223, calls history 163-165).

Serialized member types (from `SerializableMember<T, ..., Struct>` names in `core.dll`):

- `SessionInit::Payload`: `UserAgent`, `string`, `optional<int64>`.
- `UserAgent`: `string` and `int` members (names listed below).
- `SessionInit::Response`: `optional<int64|bool|int|string|vector<string>>` members.
- `Log::Parameters`: `vector<LogEvent>`; `LogEvent`: `int64`, `optional<int64>`, `string`,
  `optional<Polymorphic<Log::EventParams>>`. Same shape as PyMax `{events: [{time, userId,
  type, event, params, sessionId}]}`.

## Client identity (`SESSION_INIT`, opcode 6) — decompiled

`SessionInit::Payload` (`core.dll` `FUN_1805c6d20` and the builders `FUN_1805c2c70`,
`FUN_1805c9320`, `FUN_1816de0a0`): `{ userAgent: UserAgent, deviceId: string,
clientSessionId: optional<int64> }`. Template flag 0.

`UserAgent` (constructor `core.dll` `FUN_1805c7360`, members in this order; the trailing
number is the member flag, `deviceName` is the only one with 2, meaning still unknown):

| Key | Type | Desktop value | Source |
|---|---|---|---|
| `deviceType` | string | `"DESKTOP"` (`"DESKTOP_ROOMS"` for MAX-ROOMS) | `config.dll` `FUN_180040610` / `FUN_180040670` |
| `appVersion` | string | `"26.41.0"` | `config.dll` `FUN_18002a580` |
| `osVersion` | string | runtime (Windows version) | to capture |
| `locale` | string | runtime | to capture |
| `deviceLocale` | string | runtime | to capture |
| `deviceName` | string | runtime | to capture |
| `screen` | string | likely from `screen_type` `"2.0x"` | `config.dll` key; to capture |
| `timezone` | string | runtime | to capture |
| `msiProductCode` | string | MSI product GUID of the install | to capture |
| `buildNumber` | int | `83760` | `config.dll` `FUN_18002a800` builds `".../26.41.0.83760"` |

The HTTP user agent (`config.dll` `FUN_18002a800`) is `<name>/26.41.0.83760`; `<name>` is
likely `"OneMe Desktop"` (`user_agent` in the config JSON).

PyMax instead impersonates an **Android** client (`DeviceType.ANDROID`, `protocol_version: 10`).

## Captured: first 40 s of an unauthenticated session (step 3)

Source: `re/capture/capture.py` (Frida 17 on `SSL_write`/`SSL_read` of `libssl-3-x64.dll`;
the client sets `QT_TLS_BACKEND=openssl`), 2026-11-19, no login, QR never scanned. Decoded
frames are in `re/out/cap1/frames.jsonl` (local only, identifying values redacted). The
client's own log (`%LOCALAPPDATA%\ONEME\logs`, `PacketParameters` / `PacketResponse` lines)
shows the same payloads with partial masking.

| Fact | Value |
|---|---|
| Header `ver` | **11** (`0x0b`) both directions |
| `cmd` | 0 request (client), 1 response (server) |
| `seq` | u16 from 1, +1 per request; the response echoes it |
| Client compression | none: every client frame has flags 0 (largest seen: 314 bytes) |
| Server compression | lz4 block on every non-empty response, even when larger than the input (37 → 39 bytes) |
| Server flags | `0x01` or `0x02`; matches `ceil(plain_len / payload_len)` in every frame (201/203 → 1, 160/131 → 2, 37/39 → 1). Hypothesis: a buffer-size hint. Decoder must not rely on it. |
| Empty payload | `payload_len` 0 (PING response) |
| PING | opcode 1, client sends `{"interactive": true}` every 30 s; server answers cmd 1 with no payload |
| Session start | `SESSION_INIT` (6) seq 1 → response with `callsSeed`, `location` (country), `qr-auth-enabled`, `lang`, `reg-country-code` list |
| QR login | `288` `{}` → `{trackId, qrLink, expiresAt, pollingInterval: 5000, ttl}`; then `289 {trackId}` every `pollingInterval` → `{status: {expiresAt, ttl}}` |
| Other TLS via libssl in 40 s | only `localhost` (gRPC to `max-service.exe` for calls). No Tracer or other external host through OpenSSL; Tracer may use another stack (open). |

### Known detection risk: TLS fingerprint

The desktop client speaks TLS through OpenSSL 3 (`libssl-3-x64.dll`, Qt `openssl` backend).
`max-transport` uses rustls, whose ClientHello (cipher suites, extensions, their order) differs
from OpenSSL's, so a server that fingerprints TLS (JA3/JA4) can tell the two apart even when
every frame matches. Not addressed yet; options: record the official ClientHello (it is on the
wire before encryption, so any packet capture shows it) and compare, then decide whether to
shape rustls' offer or use OpenSSL through the `openssl` crate.

### msgpack encoding of the desktop client (capture `cap2`, raw `SESSION_INIT`)

Type markers of the 314-byte payload (`re/capture/msgpack_markers.py`; raw bytes deleted after
reading): maps `fixmap`; strings the shortest header (`fixstr` up to 31 bytes, `str8` for the
38-byte `msiProductCode` and the 40-byte `deviceId`); **integers fixed by C++ type**:
`buildNumber` (`int`) is `int32` `0xd2`, `clientSessionId` (`int64`) is `int64` `0xd3` even
for small values. A minimal-width encoder (Python `msgpack`, `rmp-serde`) produces 306 bytes,
so the difference is visible on the wire. `max-proto::msgpack` writes the desktop way.

Example header bytes: `0b 00 0001 0006 0000013a` (SESSION_INIT, 314 bytes),
`0b 01 0002 0120 02000083` (QR response, flags 2, 131 bytes).

### Desktop `UserAgent` runtime values (capture and client log)

| Key | Value on this machine | Rule for our client |
|---|---|---|
| `osVersion` | `"Windows 11 Version 25H2"` | product name + display version from the OS |
| `locale`, `deviceLocale` | `"ru"` | UI language / system locale, two letters |
| `deviceName` | 6-character string (masked) | computer name, by its length |
| `screen` | `"2.0x"` | `screen_type` from config |
| `timezone` | IANA zone name | system IANA zone |
| `msiProductCode` | `"{75D8F5A4-6EC6-4206-B9D7-B4AEA1D503FE}"` | product code of the 26.41.0 MSI (same for every install of this build) |
| `buildNumber` | `83760` | from the version table |

`SessionInit` also carries `deviceId` (base64, 28 bytes decoded; persistent per install,
stored in the profile; to locate) and `clientSessionId` (`1` on first launch; a counter).

### Pre-login side effects seen in the profile (`%LOCALAPPDATA%\ONEME`)

- `.crash_dumps\...\crashfree_running_uploaded` and `calls\calls-settings\tracer\...\crashfree_running_uploaded`:
  the Tracer SDK reported a crash-free session on the very first launch, before any login.
- Log line `CallsStatisticSender sendCallsStatisticEvent: result = success` at startup.
- The client log records microphones, speakers and cameras with device IDs.
- `max-service.exe` keeps running after the client exits.

## Server config

`server-config-keys.md`: 184 keys from the server config parser (`core.dll` `FUN_1804f8650`),
for example `stories`, `multipin-*`, `comments-*`, `failover-hosts`, `user-debug-report`,
`update-auto-check-enabled`, `offline-statistic-enabled`. The parity map needs a decision per key.

`offlineStatisticEnabled` in the local settings defaults (`core.dll` `FUN_181773ab0`) appears to
default to `0` (false); unverified.

## Telemetry

### Over the main socket (opcode `LOG = 5`)

The desktop client has a full statistics pipeline (source paths in `core.dll` / `max.exe`):

1. Presenters in `client/components/statistic/src/*`: navigation (`navigation_handler`,
   `stat_navigation_context`, debouncer), clicks, context menu, stickers, stories, banners,
   chat suggest, invite events, link dialogs, message and profile clickable actions.
2. Converters in `components/core/src/api_types_converters/statistic/*` (16): nav, banner,
   clickable, context menu, informer, invite, permission, registration, stickers, unknown
   contact interaction, unsafe file and URL alerts, webapp actions and bridge, chat message link,
   contact rename banner.
3. Queue: `storage/backends/sqlcipher/src/statistic/statistic_event_storage.cpp` and
   `business/statistic/log_event_storage_converter.cpp`.
4. Send: `business/statistic/events_sender.cpp`, `statistic_controller.cpp`,
   `presence_statistic_provider.cpp`.
5. Calls: `calls_client/src/calls_statistic_sender.cpp`, `calls_statistic/.../calls_statistic_controller.cpp`.

Event names seen in `core.dll`: `COLD_START`, `WARM_START`, `CHAT_OPENED`, `STORY_OPEN`,
`STORY_NAV`, `SOUND_PLAYED`, `CONTEXT_MENU`, `DANGEROUS_URL_ACTIONS`, `WEBAPP_BRIDGE`, ...
NAV keys `screen_from`, `screen_to`, `prev_time`, `action_id` are in `core.dll` and match PyMax.

PyMax payload (`telemetry/payloads.py`): `{events: [{time, userId, type: "NAV"|"PERF", event,
params, sessionId}]}`.

**PyMax fakes this telemetry on purpose** (`telemetry/service.py`): random NAV and PERF events
15-90 s after login, then every 15-45 min. This suggests the server may flag clients that
send none. Decision needed: see "Open decisions".

### Outside the main socket

| Channel | Evidence |
|---|---|
| Tracer SDK (`sdk-api.apptracer.ru`, `sdk-api-rf.apptracer.ru`) | URLs; `tracernative.dll`, `tracer_crash_reporter.dll` |
| Crash dumps | `crashpad_handler.exe`; `send_fatal_logs_to_crash_server: true` |
| Log upload | `logs_sending_max_files_count_client: 3`, `..._calls: 2` |
| Updater | `WinSparkle.dll`, `updater_oneme.dll`, `download2.max.ru`, `updater_proxy_backend.cpp` |
| Health check | `r-push.org` (third-party domain) |
| Maps | `static-maps.yandex.ru/v1` |
| Background service | `max-service\max-service.exe`, pipe `oneme_local_pipe` |

### Server control over statistics (strings in `core.dll`, unverified)

- `offline-statistic-enabled` / `offlineStatisticEnabled`: a server config key, so the server
  switches (at least offline) statistics on and off. Suggests statistics are optional.
- `msg_stat`, `msg_stat_sender` (and opcode `MSG_GET_STAT = 74` in PyMax): likely channel
  message view counters. This is a feature (view counts for channel authors), not only
  telemetry; decide separately whether to report views.
- `ab_event`, `ab-status` / `abStatus`: A/B experiment events.
- `channel-statistics-botid`, `suggested-channels-stat-limit`: server config for channel stats.

## Decisions taken

See `rust/DECISIONS.md`: ADR-003 (present as desktop), ADR-005 (no telemetry by default;
minimal stream behind the `telemetry-minimal` feature only if needed).

## Next to verify in Ghidra

- Header write: ver value, lz4 threshold, flag value used for lz4.
- `SESSION_INIT` literal values (`deviceType`, `appVersion`, `buildNumber`).
- Opcodes for `LOG`, `SESSION_INIT`, `PING`: constants next to the `sessionInit` string.
- `events_sender`: batching, triggers, whether it can be disabled by server config.
