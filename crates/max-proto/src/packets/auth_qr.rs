//! QR login: `CREATE_QR` (288) then `AUTH_CHECK` (289) every `pollingInterval` ms
//! (capture `cap1`; desktop types `Auth::OneMe::CreateQr`, `Auth::OneMe::AuthCheck`).

use serde::{Deserialize, Serialize};

/// `CREATE_QR` request: an empty map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CreateQr {}

/// `CREATE_QR` response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateQrResponse {
    /// Login flow id, sent back in [`AuthCheck`].
    pub track_id: String,
    /// Link to encode in the QR code.
    pub qr_link: String,
    /// Expiry, Unix milliseconds.
    pub expires_at: i64,
    /// How often to poll, ms (5000 in the capture).
    pub polling_interval: i64,
    /// Remaining lifetime, ms.
    pub ttl: i64,
}

/// `AUTH_CHECK` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthCheck {
    /// From [`CreateQrResponse::track_id`].
    pub track_id: String,
}

/// QR status while nobody has scanned the code.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QrStatus {
    /// Expiry, Unix milliseconds.
    pub expires_at: i64,
    /// Remaining lifetime, ms.
    pub ttl: i64,
}

/// `AUTH_CHECK` response. Only the not-yet-scanned shape is confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthCheckResponse {
    /// Current status.
    pub status: QrStatus,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::to_msgpack;

    #[test]
    fn sizes_match_capture() {
        assert_eq!(to_msgpack(&CreateQr {}).unwrap().len(), 1);
        // trackId is 36 characters in the capture (masked as ***36*** in the client log).
        let check = AuthCheck {
            track_id: "x".repeat(36),
        };
        assert_eq!(to_msgpack(&check).unwrap().len(), 47);
    }
}
