use std::borrow::Cow;

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::{Cmd, Opcode, ProtoError, Result, compress};

/// Header length in bytes.
pub const HEADER_LEN: usize = 10;
/// Largest payload the 24-bit length field can describe.
pub const MAX_PAYLOAD_LEN: usize = 0x00FF_FFFF;
/// Protocol version the desktop client writes in every frame (capture `cap1`).
pub const PROTOCOL_VERSION: u8 = 11;

/// The 10-byte big-endian frame header: `ver u8, cmd u8, seq u16, opcode u16, packed u32`,
/// where the top byte of `packed` is `flags` and the low 24 bits are `payload_len`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Header {
    /// Protocol version (11 for the desktop client).
    pub ver: u8,
    /// Raw frame kind; see [`Header::cmd`].
    pub cmd: u8,
    /// Request number; a response carries the request's value.
    pub seq: u16,
    /// Raw packet type; see [`Header::opcode`].
    pub opcode: u16,
    /// Compression flags: 0 plain, `0xFF` zstd, anything else lz4 block.
    pub flags: u8,
    /// Payload length in bytes (24 bits).
    pub payload_len: u32,
}

impl Header {
    /// Parse a header from its 10 bytes.
    pub fn parse(bytes: [u8; HEADER_LEN]) -> Header {
        let mut b = &bytes[..];
        let ver = b.get_u8();
        let cmd = b.get_u8();
        let seq = b.get_u16();
        let opcode = b.get_u16();
        let packed = b.get_u32();
        Header {
            ver,
            cmd,
            seq,
            opcode,
            flags: (packed >> 24) as u8,
            payload_len: packed & 0x00FF_FFFF,
        }
    }

    /// The 10 header bytes.
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        let mut b = &mut out[..];
        b.put_u8(self.ver);
        b.put_u8(self.cmd);
        b.put_u16(self.seq);
        b.put_u16(self.opcode);
        b.put_u32((u32::from(self.flags) << 24) | (self.payload_len & 0x00FF_FFFF));
        out
    }

    /// Frame kind.
    pub fn cmd(&self) -> Cmd {
        Cmd::from(self.cmd)
    }

    /// Packet type, if the desktop client knows it.
    pub fn opcode(&self) -> Option<Opcode> {
        Opcode::from_u16(self.opcode)
    }
}

/// One frame: header and the payload as sent (possibly compressed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The header; `payload_len` always equals `payload.len()`.
    pub header: Header,
    /// Payload bytes as they travel on the wire.
    pub payload: Bytes,
}

impl Frame {
    /// A client request the way the desktop client sends it: version 11, `cmd` 0, no compression.
    pub fn request(seq: u16, opcode: u16, payload: impl Into<Bytes>) -> Result<Frame> {
        Frame::new(PROTOCOL_VERSION, Cmd::Request, seq, opcode, 0, payload)
    }

    /// Any frame. Fails if the payload does not fit the 24-bit length field.
    pub fn new(
        ver: u8,
        cmd: Cmd,
        seq: u16,
        opcode: u16,
        flags: u8,
        payload: impl Into<Bytes>,
    ) -> Result<Frame> {
        let payload = payload.into();
        if payload.len() > MAX_PAYLOAD_LEN {
            return Err(ProtoError::PayloadTooLarge { len: payload.len() });
        }
        Ok(Frame {
            header: Header {
                ver,
                cmd: cmd.as_u8(),
                seq,
                opcode,
                flags,
                // INFALLIBLE: checked against MAX_PAYLOAD_LEN (24 bits) above.
                payload_len: payload.len() as u32,
            },
            payload,
        })
    }

    /// Append the encoded frame to `out`.
    pub fn encode_into(&self, out: &mut BytesMut) {
        out.reserve(HEADER_LEN + self.payload.len());
        out.put_slice(&self.header.to_bytes());
        out.put_slice(&self.payload);
    }

    /// The encoded frame.
    pub fn encode(&self) -> Bytes {
        let mut out = BytesMut::with_capacity(HEADER_LEN + self.payload.len());
        self.encode_into(&mut out);
        out.freeze()
    }

    /// The payload after decompression, at most `max_plain` bytes.
    pub fn plain_payload(&self, max_plain: usize) -> Result<Cow<'_, [u8]>> {
        compress::decompress(self.header.flags, &self.payload, max_plain)
    }
}

/// Splits a byte stream into frames. Feed it the plaintext of one TLS connection, one
/// direction, in order.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    pending: Option<Header>,
}

impl FrameDecoder {
    /// A decoder at the start of a stream.
    pub fn new() -> FrameDecoder {
        FrameDecoder::default()
    }

    /// Take the next complete frame from the front of `buf`, if there is one. Bytes of an
    /// incomplete frame stay in `buf` for the next call.
    pub fn decode(&mut self, buf: &mut BytesMut) -> Option<Frame> {
        let header = match self.pending {
            Some(h) => h,
            None => {
                if buf.len() < HEADER_LEN {
                    return None;
                }
                let mut raw = [0u8; HEADER_LEN];
                raw.copy_from_slice(&buf[..HEADER_LEN]);
                buf.advance(HEADER_LEN);
                let h = Header::parse(raw);
                self.pending = Some(h);
                h
            }
        };
        let len = header.payload_len as usize;
        if buf.len() < len {
            buf.reserve(len - buf.len());
            return None;
        }
        self.pending = None;
        Some(Frame {
            header,
            payload: buf.split_to(len).freeze(),
        })
    }

    /// Whether a header has been read and its payload is still incomplete.
    pub fn is_mid_frame(&self) -> bool {
        self.pending.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frame from github.com/nyakokitsu/MaxProtoExplanation (ver 10, seq 7, opcode 17).
    const DOC_FRAME: &str = "0a00000700110000003083a570686f6e65ac2b3739393939393939393939a474797065aa53544152545f41555448a86c616e6775616765a27275";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn header_roundtrip() {
        let h = Header {
            ver: 11,
            cmd: 1,
            seq: 0x0102,
            opcode: 0x0120,
            flags: 2,
            payload_len: 0x83,
        };
        let bytes = h.to_bytes();
        assert_eq!(
            bytes,
            [0x0b, 0x01, 0x01, 0x02, 0x01, 0x20, 0x02, 0x00, 0x00, 0x83]
        );
        assert_eq!(Header::parse(bytes), h);
        assert_eq!(h.cmd(), Cmd::Response);
        assert_eq!(h.opcode(), Some(Opcode::CreateQr));
    }

    #[test]
    fn decodes_documented_frame() {
        let mut buf = BytesMut::from(&hex(DOC_FRAME)[..]);
        let frame = FrameDecoder::new().decode(&mut buf).unwrap();
        assert!(buf.is_empty());
        let h = frame.header;
        assert_eq!(
            (h.ver, h.cmd, h.seq, h.opcode, h.flags, h.payload_len),
            (10, 0, 7, 17, 0, 0x30)
        );
        assert_eq!(h.opcode(), Some(Opcode::AuthRequest));
        assert_eq!(frame.encode().as_ref(), &hex(DOC_FRAME)[..]);
    }

    #[test]
    fn decodes_split_and_back_to_back_frames() {
        let a = Frame::request(1, 6, vec![0x80]).unwrap().encode();
        let b = Frame::request(2, 288, Vec::new()).unwrap().encode();
        let c = Frame::request(3, 1, vec![1, 2, 3, 4, 5]).unwrap().encode();
        let stream: Vec<u8> = [a.as_ref(), b.as_ref(), c.as_ref()].concat();
        for chunk in 1..stream.len() {
            let mut dec = FrameDecoder::new();
            let mut buf = BytesMut::new();
            let mut seqs = Vec::new();
            for piece in stream.chunks(chunk) {
                buf.extend_from_slice(piece);
                while let Some(f) = dec.decode(&mut buf) {
                    seqs.push(f.header.seq);
                }
            }
            assert_eq!(seqs, [1, 2, 3], "chunk size {chunk}");
            assert!(buf.is_empty() && !dec.is_mid_frame());
        }
    }

    #[test]
    fn request_matches_desktop_defaults() {
        let f = Frame::request(9, 289, vec![0u8; 47]).unwrap();
        assert_eq!(f.encode()[..HEADER_LEN], hex("0b00000901210000002f")[..]);
    }

    #[test]
    fn rejects_oversized_payload() {
        let err = Frame::request(1, 64, vec![0u8; MAX_PAYLOAD_LEN + 1]).unwrap_err();
        assert!(matches!(err, ProtoError::PayloadTooLarge { len } if len == MAX_PAYLOAD_LEN + 1));
        assert!(Frame::request(1, 64, vec![0u8; MAX_PAYLOAD_LEN]).is_ok());
    }
}
