# Glossary

| Term (official client) | Meaning | Rust name |
|---|---|---|
| OneMe | internal name of MAX (`api2.oneme.ru`, `ONEME` profile folder) | crate prefix `max-` |
| frame | 10-byte header + payload on the TCP stream | `max_proto::Frame` |
| `ver` | protocol version byte, 11 for desktop | `Frame::ver`, `PROTOCOL_VERSION` |
| `cmd` | frame kind: 0 request, 1 response, 2 event, 3 error | `max_proto::Cmd` |
| `seq` | u16 request number, echoed by the response | `Frame::seq` |
| opcode | u16 packet type | `max_proto::Opcode` |
| flags | top byte of the length word; non-zero = lz4 from the server | `Frame::flags` |
| `CommonPacket<op, Params, Response, flag>` | request/response packet type in `core.dll` | one `Opcode` variant + request/response structs |
| `CommonEvent<op, ..., Data>` | server push event type | `max_proto::Event` variant |
| `SessionInit` | opcode 6, first packet: user agent, device id, session counter | `SessionInit` |
| `UserAgent` | device and app identity inside `SessionInit` | `max_identity::UserAgent` |
| `deviceId` | persistent per-install device identifier (base64) | `max_identity::DeviceId` |
| `clientSessionId` | session counter sent in `SessionInit` | `max_identity::ClientSessionId` |
| track (`trackId`) | id of a pending login flow (QR, SMS) | `AuthTrack` |
| SFA | second factor authentication (2FA password, hint, email) | `auth::second_factor` |
| server config | key/value flags the server sends (`research/server-config-keys.md`) | `max_core::ServerConfig` |
| delayed message | scheduled message | `DelayedMessage` |
| multipin | several pinned messages in one chat | `PinnedMessages` |
| P2P / P2G | one-to-one call / group call | `calls::P2p`, `calls::Group` |
| FSG | fullscreen gallery and stories viewer components | `ui::gallery`, `ui::stories` |
| web app / mini-app | bot web application in a WebView | `webapp` |
| TAM | legacy TamTam naming (`GetDeleteTamUserTime`) | — |
| ESIA | Gosuslugi identity verification | `esia` (v3) |
