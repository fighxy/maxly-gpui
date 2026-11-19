//! Frame headers recorded from the official desktop client (captures cap1, cap2; no login).
//! The fixture holds headers and sizes only, no payloads.

// A malformed fixture line should fail the test loudly.
#![allow(clippy::unwrap_used)]

use max_proto::{Cmd, HEADER_LEN, Header, Opcode, PROTOCOL_VERSION, compression_hint};

struct Row {
    dir: String,
    header: [u8; HEADER_LEN],
    plain_len: usize,
    opcode: u16,
    cmd: u8,
    seq: u16,
    flags: u8,
    payload_len: u32,
}

fn rows() -> Vec<Row> {
    include_str!("fixtures/capture_headers.tsv")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            let mut header = [0u8; HEADER_LEN];
            for (i, b) in header.iter_mut().enumerate() {
                *b = u8::from_str_radix(&c[2][i * 2..i * 2 + 2], 16).unwrap();
            }
            Row {
                dir: c[1].to_string(),
                header,
                plain_len: c[3].parse().unwrap(),
                opcode: c[4].parse().unwrap(),
                cmd: c[5].parse().unwrap(),
                seq: c[6].parse().unwrap(),
                flags: c[7].parse().unwrap(),
                payload_len: c[8].parse().unwrap(),
            }
        })
        .collect()
}

#[test]
fn headers_parse_as_recorded_and_roundtrip() {
    let rows = rows();
    assert!(rows.len() >= 20);
    for r in &rows {
        let h = Header::parse(r.header);
        assert_eq!(
            (h.opcode, h.cmd, h.seq, h.flags, h.payload_len),
            (r.opcode, r.cmd, r.seq, r.flags, r.payload_len)
        );
        assert_eq!(h.to_bytes(), r.header);
        assert_eq!(h.ver, PROTOCOL_VERSION);
        assert!(h.opcode().is_some(), "unknown opcode {}", h.opcode);
    }
}

#[test]
fn client_sends_requests_uncompressed_server_answers() {
    for r in rows() {
        let h = Header::parse(r.header);
        if r.dir == "out" {
            assert_eq!(h.cmd(), Cmd::Request);
            assert_eq!(h.flags, 0);
        } else {
            assert_eq!(h.cmd(), Cmd::Response);
        }
    }
}

#[test]
fn server_flag_is_the_size_ratio() {
    let server: Vec<Row> = rows()
        .into_iter()
        .filter(|r| r.dir == "in" && r.payload_len > 0)
        .collect();
    assert!(!server.is_empty());
    for r in server {
        assert_eq!(
            compression_hint(r.plain_len, r.payload_len as usize),
            r.flags,
            "seq {}",
            r.seq
        );
    }
}

#[test]
fn session_starts_with_session_init_then_qr() {
    let out: Vec<Header> = rows()
        .iter()
        .filter(|r| r.dir == "out")
        .map(|r| Header::parse(r.header))
        .collect();
    assert_eq!(out[0].opcode(), Some(Opcode::SessionInit));
    assert_eq!(out[0].seq, 1);
    assert_eq!(out[1].opcode(), Some(Opcode::CreateQr));
}
