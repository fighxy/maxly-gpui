use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};

use crate::{IdentityError, Result};

/// Bytes behind a device id: the size of a SHA3-224 digest, as the official client sends.
const DEVICE_ID_BYTES: usize = 28;

/// `deviceId`: 28 bytes, base64 with padding (40 characters).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeviceId(String);

impl DeviceId {
    /// A new random device id from the OS random generator (ADR-008).
    pub fn generate() -> Result<DeviceId> {
        let mut bytes = [0u8; DEVICE_ID_BYTES];
        getrandom::fill(&mut bytes).map_err(|e| IdentityError::Random(e.to_string()))?;
        Ok(DeviceId(STANDARD.encode(bytes)))
    }

    /// Check a stored device id.
    pub fn parse(text: &str) -> Result<DeviceId> {
        match STANDARD.decode(text) {
            Ok(bytes) if bytes.len() == DEVICE_ID_BYTES => Ok(DeviceId(text.to_string())),
            Ok(bytes) => Err(IdentityError::InvalidDeviceId(format!(
                "{} bytes instead of {DEVICE_ID_BYTES}",
                bytes.len()
            ))),
            Err(e) => Err(IdentityError::InvalidDeviceId(e.to_string())),
        }
    }

    /// The wire text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DeviceId {
    type Error = IdentityError;
    fn try_from(text: String) -> Result<DeviceId> {
        DeviceId::parse(&text)
    }
}

impl From<DeviceId> for String {
    fn from(id: DeviceId) -> String {
        id.0
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_the_official_shape() {
        let id = DeviceId::generate().unwrap();
        // capture cap1 / client log: 40 characters ending in "=="
        assert_eq!(id.as_str().len(), 40);
        assert!(id.as_str().ends_with("=="));
        assert_eq!(STANDARD.decode(id.as_str()).unwrap().len(), 28);
        assert_eq!(DeviceId::parse(id.as_str()).unwrap(), id);
    }

    #[test]
    fn is_random() {
        assert_ne!(DeviceId::generate().unwrap(), DeviceId::generate().unwrap());
    }

    #[test]
    fn rejects_bad_values() {
        assert!(DeviceId::parse("not base64!").is_err());
        assert!(DeviceId::parse(&STANDARD.encode([0u8; 20])).is_err());
    }
}
