//! Local interface settings (theme, font size), kept in `%LOCALAPPDATA%\maxrs\ui.txt` as
//! `key=value` lines. Nothing here leaves the machine or holds account data.

use std::path::PathBuf;

/// Light, dark, or following Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    /// Follow the system appearance.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl ThemeChoice {
    fn key(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }

    fn parse(s: &str) -> Option<ThemeChoice> {
        match s {
            "system" => Some(ThemeChoice::System),
            "light" => Some(ThemeChoice::Light),
            "dark" => Some(ThemeChoice::Dark),
            _ => None,
        }
    }
}

/// Font sizes the appearance page offers, in px.
pub const FONT_SIZES: [u8; 4] = [14, 15, 16, 18];

/// The interface settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiSettings {
    /// Colour scheme.
    pub theme: ThemeChoice,
    /// Base font size in px, one of [`FONT_SIZES`].
    pub font_size: u8,
}

impl Default for UiSettings {
    fn default() -> UiSettings {
        UiSettings {
            theme: ThemeChoice::System,
            font_size: 16,
        }
    }
}

impl UiSettings {
    /// Read from `text`; unknown keys and bad values keep the defaults.
    pub fn parse(text: &str) -> UiSettings {
        let mut s = UiSettings::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "theme" => {
                    if let Some(t) = ThemeChoice::parse(value.trim()) {
                        s.theme = t;
                    }
                }
                "font_size" => {
                    if let Ok(n) = value.trim().parse::<u8>()
                        && FONT_SIZES.contains(&n)
                    {
                        s.font_size = n;
                    }
                }
                _ => {}
            }
        }
        s
    }

    /// The file text.
    pub fn render(&self) -> String {
        format!("theme={}\nfont_size={}\n", self.theme.key(), self.font_size)
    }

    /// The settings file, next to the profiles folder.
    pub fn path() -> Option<PathBuf> {
        dirs::data_local_dir().map(|d| d.join("maxrs").join("ui.txt"))
    }

    /// Load from disk; a missing or unreadable file gives the defaults.
    pub fn load() -> UiSettings {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| UiSettings::parse(&t))
            .unwrap_or_default()
    }

    /// Save to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path().ok_or_else(|| std::io::Error::other("no local data folder"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.render())
    }
}

#[cfg(test)]
mod tests {
    use super::{ThemeChoice, UiSettings};

    #[test]
    fn round_trip_and_bad_values_keep_defaults() {
        let s = UiSettings {
            theme: ThemeChoice::Dark,
            font_size: 18,
        };
        assert_eq!(UiSettings::parse(&s.render()), s);
        let bad = UiSettings::parse("theme=purple\nfont_size=99\nother=1\nnoise");
        assert_eq!(bad, UiSettings::default());
        assert_eq!(UiSettings::parse("").theme, ThemeChoice::System);
    }
}
