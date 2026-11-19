//! `SESSION_INIT` (opcode 6), the first request on every connection.
//!
//! Layout: `core.dll` `FUN_1805c6d20` (payload) and `FUN_1805c7360` (user agent constructor,
//! member order); values: `config.dll` and capture `cap1`.

use serde::{Deserialize, Serialize};

/// Device and application identity. Members in the order the desktop client declares them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserAgent {
    /// `"DESKTOP"` for the Windows client.
    pub device_type: String,
    /// Marketing version, for example `"26.41.0"`.
    pub app_version: String,
    /// For example `"Windows 11 Version 25H2"`.
    pub os_version: String,
    /// UI language, two letters.
    pub locale: String,
    /// System locale, two letters.
    pub device_locale: String,
    /// Computer name.
    pub device_name: String,
    /// Screen scale class from the config, `"2.0x"`.
    pub screen: String,
    /// IANA time zone name.
    pub timezone: String,
    /// Product code of the installed MSI.
    pub msi_product_code: String,
    /// Build number, for example `83760`.
    pub build_number: i32,
}

/// `SESSION_INIT` request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInit {
    /// Device and application identity.
    pub user_agent: UserAgent,
    /// Persistent per-install device id (base64).
    pub device_id: String,
    /// Session counter; `1` on the first launch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_session_id: Option<i64>,
}

/// `SESSION_INIT` response fields seen in capture `cap1` (before login). Other fields are
/// ignored until they are confirmed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct SessionInitResponse {
    /// Opaque seed for the calls SDK.
    #[serde(rename = "callsSeed", default)]
    pub calls_seed: Option<String>,
    /// Country code the server sees, for example `"RU"`.
    #[serde(default)]
    pub location: Option<String>,
    /// Whether QR login is enabled.
    #[serde(rename = "qr-auth-enabled", default)]
    pub qr_auth_enabled: Option<bool>,
    /// Purpose unknown; `true` in the capture.
    #[serde(default)]
    pub lang: Option<bool>,
    /// Country codes for registration rules.
    #[serde(rename = "reg-country-code", default)]
    pub reg_country_code: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{from_msgpack, to_msgpack};

    /// Values with the same lengths as the capture (`deviceName` 6 chars, `deviceId` 40 chars,
    /// a 16-char time zone), so the encoded size must equal the captured 314 bytes.
    pub(crate) fn sample() -> SessionInit {
        SessionInit {
            user_agent: UserAgent {
                device_type: "DESKTOP".into(),
                app_version: "26.41.0".into(),
                os_version: "Windows 11 Version 25H2".into(),
                locale: "ru".into(),
                device_locale: "ru".into(),
                device_name: "ABCDEF".into(),
                screen: "2.0x".into(),
                timezone: "Europe/Volgograd".into(),
                msi_product_code: "{75D8F5A4-6EC6-4206-B9D7-B4AEA1D503FE}".into(),
                build_number: 83760,
            },
            device_id: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==".into(),
            client_session_id: Some(1),
        }
    }

    #[test]
    fn encoded_size_matches_the_desktop_client() {
        let bytes = to_msgpack(&sample()).unwrap();
        assert_eq!(
            bytes.len(),
            314,
            "capture cap1: SESSION_INIT payload_len 314"
        );
        // Starts with a 3-entry map and the "userAgent" key, as the official serializer.
        assert_eq!(&bytes[..11], b"\x83\xa9userAgent");
        // Capture cap2 markers: msiProductCode str8, buildNumber int32, deviceId str8,
        // clientSessionId int64.
        let after = |key: &[u8]| {
            let at = bytes.windows(key.len()).position(|w| w == key).unwrap();
            bytes[at + key.len()]
        };
        assert_eq!(after(b"\xaemsiProductCode"), 0xd9);
        assert_eq!(after(b"\xabbuildNumber"), 0xd2);
        assert_eq!(after(b"\xa8deviceId"), 0xd9);
        assert_eq!(after(b"\xafclientSessionId"), 0xd3);
        assert_eq!(after(b"\xa8timezone"), 0xb0);
    }

    #[test]
    fn roundtrip() {
        let s = sample();
        let back: SessionInit = from_msgpack(&to_msgpack(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn decodes_captured_response_shape() {
        #[derive(Serialize)]
        struct Raw<'a> {
            #[serde(rename = "callsSeed")]
            calls_seed: &'a str,
            location: &'a str,
            #[serde(rename = "qr-auth-enabled")]
            qr: bool,
            lang: bool,
            #[serde(rename = "reg-country-code")]
            reg: Vec<&'a str>,
            #[serde(rename = "future-field")]
            future: u8,
        }
        let raw = Raw {
            calls_seed: "seed",
            location: "RU",
            qr: true,
            lang: true,
            reg: vec!["AZ", "AM"],
            future: 1,
        };
        let r: SessionInitResponse = from_msgpack(&to_msgpack(&raw).unwrap()).unwrap();
        assert_eq!(r.location.as_deref(), Some("RU"));
        assert_eq!(r.qr_auth_enabled, Some(true));
        assert_eq!(r.reg_country_code.unwrap(), ["AZ", "AM"]);
    }
}
