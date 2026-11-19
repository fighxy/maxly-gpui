/// An official desktop build to present as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientVersion {
    /// `deviceType`.
    pub device_type: &'static str,
    /// `appVersion` (`config.dll` `FUN_18002a580`).
    pub app_version: &'static str,
    /// `buildNumber` (`config.dll` `FUN_18002a800`).
    pub build_number: i32,
    /// `msiProductCode`: the product code of this build's MSI, the same on every install.
    pub msi_product_code: &'static str,
    /// `screen` (`config.dll` `screen_type`).
    pub screen: &'static str,
    /// Frame header `ver`.
    pub protocol_version: u8,
}

/// MAX for Windows 26.41.0.83760 (`C:\MAX`, 2026-11-18), checked against captures cap1 and cap2.
pub const DESKTOP_26_41_0: ClientVersion = ClientVersion {
    device_type: "DESKTOP",
    app_version: "26.41.0",
    build_number: 83760,
    msi_product_code: "{75D8F5A4-6EC6-4206-B9D7-B4AEA1D503FE}",
    screen: "2.0x",
    protocol_version: max_proto::PROTOCOL_VERSION,
};

impl ClientVersion {
    /// The HTTP user agent of this build, `OneMe Desktop/<appVersion>.<buildNumber>`
    /// (`config.dll` `user_agent` and `FUN_18002a800`, which joins a name, `/` and the full
    /// version). Unconfirmed until an HTTP upload is captured.
    pub fn http_user_agent(&self) -> String {
        format!("OneMe Desktop/{}.{}", self.app_version, self.build_number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_user_agent() {
        assert_eq!(
            DESKTOP_26_41_0.http_user_agent(),
            "OneMe Desktop/26.41.0.83760"
        );
    }
}
