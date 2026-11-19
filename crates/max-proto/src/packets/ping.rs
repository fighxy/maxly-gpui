//! `PING` (opcode 1): the client sends `{"interactive": true}` every 30 s; the server answers
//! with an empty payload (capture `cap1`).

use serde::{Deserialize, Serialize};

/// `PING` request payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    /// `true` while the client is in the foreground (meaning of `false` unconfirmed).
    pub interactive: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::to_msgpack;

    #[test]
    fn size_matches_capture() {
        // capture cap1: PING payload_len 14
        let bytes = to_msgpack(&Ping { interactive: true }).unwrap();
        assert_eq!(bytes, b"\x81\xabinteractive\xc3");
        assert_eq!(bytes.len(), 14);
    }
}
