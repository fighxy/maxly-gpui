use crate::{IdentityError, Result};

const ALPHABET: &[u8; 36] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// A random name in the style Windows gives new computers: `DESKTOP-` and 7 characters of
/// `A-Z0-9` (ADR-008).
pub fn random_device_name() -> Result<String> {
    let mut bytes = [0u8; 7];
    getrandom::fill(&mut bytes).map_err(|e| IdentityError::Random(e.to_string()))?;
    let suffix: String = bytes
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(*b) % ALPHABET.len()]))
        .collect();
    Ok(format!("DESKTOP-{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_like_a_default_windows_name() {
        let name = random_device_name().unwrap();
        assert_eq!(name.len(), 15);
        let (prefix, suffix) = name.split_at(8);
        assert_eq!(prefix, "DESKTOP-");
        assert!(
            suffix
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        );
    }
}
