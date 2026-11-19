//! QR login: `CREATE_QR` (288), then `AUTH_CHECK` (289) every `pollingInterval` ms until
//! `loginAvailable`, then `AUTH_BY_TRACK` (291). Layouts from `core.dll` ([`crate::schema`]);
//! the polling seen in capture `cap1`.

use serde::{Deserialize, Serialize};

use crate::schema::packets::auth::one_me;

/// `CREATE_QR` request: an empty map (payload length 1 in capture `cap1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CreateQr {}

/// `CREATE_QR` response: `trackId`, `pollingInterval`, `qrLink`. The server also sends
/// `expiresAt` and `ttl` (capture `cap1`), which the official client does not read.
pub type CreateQrResponse = one_me::create_qr::Response;
/// `AUTH_CHECK` request: `trackId`.
pub type AuthCheck = one_me::auth_check::Parameters;
/// `AUTH_CHECK` response: `status` with `loginAvailable` and `expiresAt`.
pub type AuthCheckResponse = one_me::auth_check::Response;
/// The status inside [`AuthCheckResponse`].
pub type QrStatus = one_me::auth_check::response::Status;
/// `AUTH_BY_TRACK` request: `trackId`, once `loginAvailable` is true.
pub type AuthByTrack = one_me::auth_by_track::Parameters;
/// `AUTH_BY_TRACK` response: token attributes and profile.
pub type AuthByTrackResponse = one_me::auth_by_track::Response;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{from_msgpack, to_msgpack};

    #[test]
    fn sizes_match_capture() {
        assert_eq!(to_msgpack(&CreateQr {}).unwrap().len(), 1);
        // trackId is 36 characters in the capture (masked as ***36*** in the client log).
        let check = AuthCheck {
            track_id: "x".repeat(36),
        };
        assert_eq!(to_msgpack(&check).unwrap().len(), 47);
    }

    #[test]
    fn status_reads_the_captured_shape() {
        // capture cap1: {"status": {"expiresAt": ..., "ttl": ...}} before anyone scans
        let mut status = std::collections::BTreeMap::new();
        status.insert("expiresAt", rmpv::Value::from(1_795_093_129_042i64));
        status.insert("ttl", rmpv::Value::from(114_896i64));
        let mut map = std::collections::BTreeMap::new();
        map.insert("status", status);
        let r: AuthCheckResponse = from_msgpack(&to_msgpack(&map).unwrap()).unwrap();
        let s = r.status.unwrap();
        assert_eq!(s.expires_at, Some(1_795_093_129_042));
        assert_eq!(s.login_available, None);
    }
}
