//! Wire format of the MAX desktop protocol, as the official Windows client 26.41.0.83760 speaks it.
//!
//! A TLS stream to `api2.oneme.ru:443` carries [`Frame`]s: a 10-byte big-endian [`Header`]
//! (`ver`, `cmd`, `seq`, `opcode`, flags and a 24-bit payload length) followed by a msgpack
//! payload. The server compresses its payloads with lz4 (block format); the desktop client sends
//! them uncompressed. Sources for every rule are in `rust/research/protocol-and-telemetry.md`.

mod cmd;
mod compress;
mod error;
mod frame;
pub mod msgpack;
mod opcode;
pub mod packets;
pub mod payload;
/// Every packet and type layout of the desktop client, generated from `core.dll`
/// (`re/scripts/gen_schema_rs.mjs`; reference: `rust/research/packet-schemas.md`).
pub mod schema;

pub use cmd::Cmd;
pub use compress::{MAX_PLAIN_LEN, compress_server_payload, compression_hint, decompress};
pub use error::ProtoError;
pub use frame::{Frame, FrameDecoder, HEADER_LEN, Header, MAX_PAYLOAD_LEN, PROTOCOL_VERSION};
pub use opcode::Opcode;

/// Result type of this crate.
pub type Result<T, E = ProtoError> = std::result::Result<T, E>;
