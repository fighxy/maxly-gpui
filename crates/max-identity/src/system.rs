//! Facts about this machine that the user agent carries.

/// `osVersion` the way the desktop client writes it (Qt `QSysInfo::prettyProductName`):
/// `"Windows 11 Version 25H2"`. Windows 11 is any build from 22000 on.
pub fn format_windows_version(build: u32, display_version: &str) -> String {
    let product = if build >= 22000 {
        "Windows 11"
    } else {
        "Windows 10"
    };
    if display_version.is_empty() {
        product.to_string()
    } else {
        format!("{product} Version {display_version}")
    }
}

/// This machine's `osVersion`, read from the registry; `None` off Windows or when the registry
/// has no build number.
#[cfg(windows)]
pub fn os_version() -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()?;
    let build: String = key.get_value("CurrentBuildNumber").ok()?;
    let build: u32 = build.trim().parse().ok()?;
    let display: String = key.get_value("DisplayVersion").unwrap_or_default();
    Some(format_windows_version(build, display.trim()))
}

/// This machine's `osVersion`; `None` off Windows.
#[cfg(not(windows))]
pub fn os_version() -> Option<String> {
    None
}

/// Two-letter language of a locale tag: `"ru-RU"` and `"ru_RU.UTF-8"` give `"ru"`.
pub fn language_of(tag: &str) -> Option<String> {
    let lang: String = tag.split(['-', '_', '.', '@']).next()?.to_ascii_lowercase();
    (lang.len() == 2 && lang.bytes().all(|b| b.is_ascii_lowercase())).then_some(lang)
}

/// The system language as two letters (`locale` and `deviceLocale`); `"en"` if unknown.
pub fn language() -> String {
    sys_locale::get_locale()
        .and_then(|tag| language_of(&tag))
        .unwrap_or_else(|| "en".to_string())
}

/// The system IANA time zone (`timezone`); `"UTC"` if unknown.
pub fn timezone() -> String {
    iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_version_like_qt() {
        assert_eq!(
            format_windows_version(26200, "25H2"),
            "Windows 11 Version 25H2"
        );
        assert_eq!(
            format_windows_version(19045, "22H2"),
            "Windows 10 Version 22H2"
        );
        assert_eq!(format_windows_version(22000, ""), "Windows 11");
    }

    #[test]
    fn language_tags() {
        assert_eq!(language_of("ru-RU").as_deref(), Some("ru"));
        assert_eq!(language_of("en_US.UTF-8").as_deref(), Some("en"));
        assert_eq!(language_of("C"), None);
    }

    #[test]
    fn system_values_are_present() {
        assert_eq!(language().len(), 2);
        assert!(!timezone().is_empty());
        #[cfg(windows)]
        assert!(os_version().unwrap().starts_with("Windows "));
    }
}
