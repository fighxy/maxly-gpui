use max_proto::packets::{SessionInit, UserAgent};
use serde::{Deserialize, Serialize};

use crate::{ClientVersion, DeviceId, Result, random_device_name, system};

/// Per-profile identity state. The caller stores it (it is not secret) and loads it on every
/// launch, so the server keeps seeing the same device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Random device id (ADR-008).
    pub device_id: DeviceId,
    /// Random Windows-style name chosen for this profile.
    pub random_device_name: String,
    /// `clientSessionId` of the last launch; 0 before the first.
    pub last_session_id: i64,
}

impl Identity {
    /// A fresh profile identity.
    pub fn new_random() -> Result<Identity> {
        Ok(Identity {
            device_id: DeviceId::generate()?,
            random_device_name: random_device_name()?,
            last_session_id: 0,
        })
    }

    /// Count a new launch and return its `clientSessionId` (1 on the first launch). Store the
    /// identity afterwards.
    pub fn begin_session(&mut self) -> i64 {
        self.last_session_id = self.last_session_id.saturating_add(1);
        self.last_session_id
    }
}

/// Which `deviceName` to send.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DeviceNameChoice {
    /// The random name of the profile (default).
    #[default]
    Random,
    /// The computer's real name, like the official client.
    Computer,
    /// Any text.
    Custom(String),
}

/// User settings that shape the user agent.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IdentitySettings {
    /// `deviceName` choice.
    pub device_name: DeviceNameChoice,
    /// `timezone` override; `None` uses the system zone.
    pub timezone: Option<String>,
    /// UI language (`locale`); `None` uses the system language.
    pub ui_language: Option<String>,
}

/// Machine facts for the user agent; [`Environment::detect`] reads them, tests build them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    /// `osVersion`.
    pub os_version: String,
    /// System language, two letters (`deviceLocale`).
    pub system_language: String,
    /// System IANA time zone.
    pub timezone: String,
    /// The computer's name, used only with [`DeviceNameChoice::Computer`].
    pub computer_name: Option<String>,
}

impl Environment {
    /// Read this machine. Off Windows `osVersion` falls back to the official client's value
    /// on its reference machine, since we present as the Windows client (ADR-003).
    pub fn detect() -> Environment {
        Environment {
            os_version: system::os_version()
                .unwrap_or_else(|| "Windows 11 Version 25H2".to_string()),
            system_language: system::language(),
            timezone: system::timezone(),
            computer_name: std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty()),
        }
    }
}

/// The `userAgent` for this build, machine and settings.
pub fn user_agent(
    version: &ClientVersion,
    env: &Environment,
    identity: &Identity,
    settings: &IdentitySettings,
) -> UserAgent {
    let device_name = match &settings.device_name {
        DeviceNameChoice::Random => identity.random_device_name.clone(),
        DeviceNameChoice::Computer => env
            .computer_name
            .clone()
            .unwrap_or_else(|| identity.random_device_name.clone()),
        DeviceNameChoice::Custom(name) => name.clone(),
    };
    UserAgent {
        device_type: version.device_type.to_string(),
        app_version: version.app_version.to_string(),
        os_version: env.os_version.clone(),
        locale: settings
            .ui_language
            .clone()
            .unwrap_or_else(|| env.system_language.clone()),
        device_locale: env.system_language.clone(),
        device_name,
        screen: version.screen.to_string(),
        timezone: settings
            .timezone
            .clone()
            .unwrap_or_else(|| env.timezone.clone()),
        msi_product_code: version.msi_product_code.to_string(),
        build_number: version.build_number,
    }
}

/// The `SESSION_INIT` payload for one launch: counts the launch in `identity`.
pub fn session_init(
    version: &ClientVersion,
    env: &Environment,
    identity: &mut Identity,
    settings: &IdentitySettings,
) -> SessionInit {
    let client_session_id = identity.begin_session();
    SessionInit {
        user_agent: user_agent(version, env, identity, settings),
        device_id: identity.device_id.as_str().to_string(),
        client_session_id: Some(client_session_id),
    }
}

#[cfg(test)]
mod tests {
    use max_proto::payload::to_msgpack;

    use super::*;
    use crate::DESKTOP_26_41_0;

    fn env() -> Environment {
        Environment {
            os_version: "Windows 11 Version 25H2".into(),
            system_language: "ru".into(),
            timezone: "Europe/Volgograd".into(),
            computer_name: Some("IVAN-PC".into()),
        }
    }

    #[test]
    fn session_counter_starts_at_one() {
        let mut id = Identity::new_random().unwrap();
        assert_eq!(id.begin_session(), 1);
        assert_eq!(id.begin_session(), 2);
    }

    #[test]
    fn default_payload_has_the_desktop_size_and_no_computer_name() {
        let mut id = Identity::new_random().unwrap();
        // Same lengths as capture cap1: a 6-character device name in the capture, 15 here.
        let init = session_init(
            &DESKTOP_26_41_0,
            &env(),
            &mut id,
            &IdentitySettings::default(),
        );
        assert_eq!(init.client_session_id, Some(1));
        assert!(init.user_agent.device_name.starts_with("DESKTOP-"));
        assert_ne!(init.user_agent.device_name, "IVAN-PC");
        assert_eq!(init.user_agent.device_type, "DESKTOP");
        assert_eq!(init.user_agent.build_number, 83760);
        let len = to_msgpack(&init).unwrap().len();
        assert_eq!(
            len,
            314 - 6 + 15,
            "capture size adjusted for the device name length"
        );
    }

    #[test]
    fn settings_override_name_zone_and_language() {
        let id = Identity::new_random().unwrap();
        let s = IdentitySettings {
            device_name: DeviceNameChoice::Computer,
            timezone: Some("UTC".into()),
            ui_language: Some("en".into()),
        };
        let ua = user_agent(&DESKTOP_26_41_0, &env(), &id, &s);
        assert_eq!(ua.device_name, "IVAN-PC");
        assert_eq!(ua.timezone, "UTC");
        assert_eq!(
            (ua.locale.as_str(), ua.device_locale.as_str()),
            ("en", "ru")
        );
        let custom = IdentitySettings {
            device_name: DeviceNameChoice::Custom("Work".into()),
            ..IdentitySettings::default()
        };
        assert_eq!(
            user_agent(&DESKTOP_26_41_0, &env(), &id, &custom).device_name,
            "Work"
        );
    }

    #[test]
    fn identity_roundtrips_through_json_like_storage() {
        let id = Identity::new_random().unwrap();
        let bytes = to_msgpack(&id).unwrap();
        let back: Identity = max_proto::payload::from_msgpack(&bytes).unwrap();
        assert_eq!(back, id);
    }
}
