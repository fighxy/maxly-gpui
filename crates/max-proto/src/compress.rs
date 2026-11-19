use std::borrow::Cow;

use crate::{MAX_PAYLOAD_LEN, ProtoError, Result};

/// Default upper bound for a decompressed payload (16 MiB).
pub const MAX_PLAIN_LEN: usize = 16 * 1024 * 1024;

/// Decompress a payload according to the header `flags`.
///
/// * `0`: plain, returned as is.
/// * `0xFF`: zstd, which the desktop protocol never uses: [`ProtoError::ZstdUnsupported`].
/// * anything else: lz4 block without a size prefix. The server sets the flag to
///   [`compression_hint`] of the plain and compressed sizes (all 9 server frames of capture
///   `cap1`), so `flags * len` is tried as the output size first; the result never depends on
///   the hint being right, only on `max_plain`.
pub fn decompress(flags: u8, payload: &[u8], max_plain: usize) -> Result<Cow<'_, [u8]>> {
    match flags {
        0 => Ok(Cow::Borrowed(payload)),
        0xFF => Err(ProtoError::ZstdUnsupported),
        hint => {
            let guess = (usize::from(hint) * payload.len()).clamp(1, max_plain.max(1));
            match lz4_flex::block::decompress(payload, guess) {
                Ok(plain) => Ok(Cow::Owned(plain)),
                Err(lz4_flex::block::DecompressError::OutputTooSmall { .. })
                    if guess < max_plain =>
                {
                    Ok(Cow::Owned(lz4_flex::block::decompress(payload, max_plain)?))
                }
                Err(e) => Err(e.into()),
            }
        }
    }
}

/// The flag value the server writes for an lz4 payload: `ceil(plain_len / compressed_len)`,
/// at least 1 and at most 254 (`0xFF` means zstd).
pub fn compression_hint(plain_len: usize, compressed_len: usize) -> u8 {
    if compressed_len == 0 {
        return 1;
    }
    let ratio = plain_len.div_ceil(compressed_len).clamp(1, 254);
    // INFALLIBLE: clamped to 1..=254 above.
    ratio as u8
}

/// Compress a payload the way the server does (lz4 block, no size prefix) and return it with
/// its flag. The desktop client never compresses; this exists for tests and a local test server.
pub fn compress_server_payload(plain: &[u8]) -> Result<(u8, Vec<u8>)> {
    let packed = lz4_flex::block::compress(plain);
    if packed.len() > MAX_PAYLOAD_LEN {
        return Err(ProtoError::PayloadTooLarge { len: packed.len() });
    }
    Ok((compression_hint(plain.len(), packed.len()), packed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_matches_capture() {
        // (plain, compressed, flags) from the server frames of capture cap1.
        for (plain, comp, flags) in [(201, 203, 1), (160, 131, 2), (37, 39, 1)] {
            assert_eq!(compression_hint(plain, comp), flags);
        }
    }

    #[test]
    fn roundtrip_with_and_without_a_right_hint() {
        let plain: Vec<u8> = (0..5000u32).flat_map(|i| (i % 7).to_be_bytes()).collect();
        let (flags, packed) = compress_server_payload(&plain).unwrap();
        assert!(flags > 1);
        assert_eq!(
            decompress(flags, &packed, MAX_PLAIN_LEN).unwrap(),
            &plain[..]
        );
        // A hint that is too small must still decode.
        assert_eq!(decompress(1, &packed, MAX_PLAIN_LEN).unwrap(), &plain[..]);
    }

    #[test]
    fn plain_is_borrowed() {
        assert!(matches!(
            decompress(0, b"abc", 10).unwrap(),
            Cow::Borrowed(b"abc")
        ));
    }

    #[test]
    fn zstd_is_refused() {
        assert!(matches!(
            decompress(0xFF, b"x", 10),
            Err(ProtoError::ZstdUnsupported)
        ));
    }

    #[test]
    fn respects_the_size_limit() {
        let plain = vec![7u8; 10_000];
        let (flags, packed) = compress_server_payload(&plain).unwrap();
        assert!(matches!(
            decompress(flags, &packed, 1000),
            Err(ProtoError::Lz4(_))
        ));
    }

    #[test]
    fn corrupt_input_is_an_error() {
        assert!(matches!(
            decompress(1, &[0xF0, 0xFF, 0xFF], MAX_PLAIN_LEN),
            Err(ProtoError::Lz4(_))
        ));
    }
}
