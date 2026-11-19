use thiserror::Error;

/// Errors while encoding or decoding frames and payloads.
#[derive(Debug, Error)]
pub enum ProtoError {
    /// A payload longer than the 24-bit length field can carry.
    #[error("payload of {len} bytes exceeds the 24-bit frame limit")]
    PayloadTooLarge {
        /// Payload length in bytes.
        len: usize,
    },
    /// Flags `0xFF` mean zstd; the desktop protocol never uses it and this crate does not decode it.
    #[error("zstd-compressed payload (flags 0xFF) is not supported")]
    ZstdUnsupported,
    /// The lz4 block did not decompress within the allowed size.
    #[error("lz4 payload: {0}")]
    Lz4(#[from] lz4_flex::block::DecompressError),
    /// The plain payload is not valid msgpack, or does not fit the requested type.
    #[error("msgpack decode: {0}")]
    Decode(String),
    /// A value could not be encoded as msgpack.
    #[error("msgpack encode: {0}")]
    Encode(String),
}
