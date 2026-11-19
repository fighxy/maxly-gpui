//! `SESSION_INIT` (opcode 6), the first request on every connection.
//!
//! The types are the generated layouts from `core.dll` ([`crate::schema`]); values come from
//! `config.dll` and captures `cap1`/`cap2`.

/// Device and application identity (`Api::OneMe::Packets::UserAgent`).
pub type UserAgent = crate::schema::packets::UserAgent;
/// `SESSION_INIT` request payload (`Api::OneMe::Packets::SessionInit::Payload`).
pub type SessionInit = crate::schema::packets::session_init::Payload;
/// `SESSION_INIT` response (`Api::OneMe::Packets::SessionInit::Response`). The server also
/// sends `location`, `qr-auth-enabled` and more (capture `cap1`), which the official client
/// does not read either.
pub type SessionInitResponse = crate::schema::packets::session_init::Response;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{from_msgpack, to_msgpack};

    /// Values with the same lengths as the capture (`deviceName` 6 chars, `deviceId` 40 chars,
    /// a 16-char time zone), so the encoded size must equal the captured 314 bytes.
    fn sample() -> SessionInit {
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
        assert_eq!(&bytes[..11], b"\x83\xa9userAgent");
        // Capture cap2 markers: msiProductCode str8, buildNumber int32, deviceId str8,
        // clientSessionId int64, timezone fixstr.
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
    fn response_ignores_unknown_keys_and_reads_known_ones() {
        // Shape of the captured response: extra keys beside the schema's ones.
        let mut map = std::collections::BTreeMap::new();
        map.insert("location", rmpv::Value::from("RU"));
        map.insert("qr-auth-enabled", rmpv::Value::from(true));
        map.insert("lang", rmpv::Value::from(true));
        map.insert("callsSeed", rmpv::Value::from(0xDEAD_BEEFi64));
        map.insert(
            "reg-country-code",
            rmpv::Value::Array(vec!["AZ".into(), "AM".into()]),
        );
        let r: SessionInitResponse = from_msgpack(&to_msgpack(&map).unwrap()).unwrap();
        assert_eq!(r.lang, Some(true));
        assert_eq!(r.calls_seed, Some(0xDEAD_BEEF));
        assert_eq!(r.reg_country_code.unwrap(), ["AZ", "AM"]);
        assert_eq!(r.app_update_type, None);
    }
}
