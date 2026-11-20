//! A one-line, privacy-safe picture of a payload for local protocol traces.
//!
//! Map keys (the schema) and numbers are kept, so request parameters, ids, times and error
//! codes stay readable. Strings are replaced by their length, except error texts, enum-like
//! constants (`READ_MESSAGE`, `CHECK_CODE`) and the host of a URL. Anything under a key that
//! names a phone number, token or code is hidden whatever its type (maps and arrays under such
//! a key are still walked, so `tokenAttrs` shows its shape).

use std::fmt::Write;

use rmpv::Value;

/// String keys whose values are kept: server error texts and client-side enum names.
const KEEP_KEYS: &[&str] = &[
    "error",
    "localizedMessage",
    "deviceType",
    "appVersion",
    "osVersion",
    "locale",
    "deviceLocale",
    "screen",
];

/// Key fragments (lowercase) whose values are always hidden.
const SECRET_KEYS: &[&str] = &["phone", "token", "code", "password", "hash", "secret"];

/// Arrays longer than this show only their first items.
const MAX_ITEMS: usize = 5;
/// Nesting past this depth is cut.
const MAX_DEPTH: usize = 12;

/// Render `value` with personal data removed.
pub fn redacted(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value, None, 0);
    out
}

fn is_secret(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    // `errorCode` carries a server code and `authTokenType` an enum name, not secrets.
    !key.starts_with("error")
        && !key.ends_with("type")
        && SECRET_KEYS.iter().any(|s| key.contains(s))
}

fn is_constant(s: &str) -> bool {
    (2..=64).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn url_host(s: &str) -> Option<&str> {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&rest[..end])
}

fn write_value(out: &mut String, value: &Value, key: Option<&str>, depth: usize) {
    let scalar = !matches!(value, Value::Nil | Value::Map(_) | Value::Array(_));
    if scalar && key.is_some_and(is_secret) {
        out.push_str("<hidden>");
        return;
    }
    if depth > MAX_DEPTH {
        out.push('…');
        return;
    }
    match value {
        Value::Nil => out.push_str("nil"),
        Value::Boolean(b) => {
            let _ = write!(out, "{b}");
        }
        Value::Integer(i) => {
            let _ = write!(out, "{i}");
        }
        Value::F32(f) => {
            let _ = write!(out, "{f}");
        }
        Value::F64(f) => {
            let _ = write!(out, "{f}");
        }
        Value::String(s) => match s.as_str() {
            Some(s) if key.is_some_and(|k| KEEP_KEYS.contains(&k)) || is_constant(s) => {
                let _ = write!(out, "{s:?}");
            }
            Some(s) => match url_host(s) {
                Some(host) => {
                    let _ = write!(out, "<url {host} {}>", s.len());
                }
                None => {
                    let _ = write!(out, "<str {}>", s.chars().count());
                }
            },
            None => {
                let _ = write!(out, "<str {}>", s.as_bytes().len());
            }
        },
        Value::Binary(b) => {
            let _ = write!(out, "<bin {}>", b.len());
        }
        Value::Ext(t, b) => {
            let _ = write!(out, "<ext {t} {}>", b.len());
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().take(MAX_ITEMS).enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(out, item, None, depth + 1);
            }
            if items.len() > MAX_ITEMS {
                let _ = write!(out, ", …+{}", items.len() - MAX_ITEMS);
            }
            out.push(']');
        }
        Value::Map(entries) => {
            out.push('{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                match k {
                    // Keys are schema names or ids; both are safe to show.
                    Value::String(s) => out.push_str(s.as_str().unwrap_or("<key>")),
                    other => write_value(out, other, None, depth + 1),
                }
                out.push_str(": ");
                write_value(out, v, k.as_str(), depth + 1);
            }
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| (Value::from(k), v))
                .collect(),
        )
    }

    #[test]
    fn keeps_schema_numbers_and_constants() {
        let v = map(vec![
            ("chatId", Value::from(-42)),
            ("type", Value::from("READ_MESSAGE")),
            ("notify", Value::from(true)),
        ]);
        assert_eq!(
            redacted(&v),
            r#"{chatId: -42, type: "READ_MESSAGE", notify: true}"#
        );
    }

    #[test]
    fn hides_text_names_and_secrets() {
        let v = map(vec![
            ("text", Value::from("Привет, как дела?")),
            ("phone", Value::from(79_001_234_567_i64)),
            ("token", Value::from("abcdef")),
            ("verifyCode", Value::from("123456")),
            ("name", Value::from("Иван")),
            ("authTokenType", Value::from("CHECK_CODE")),
            (
                "tokenAttrs",
                map(vec![("LOGIN", map(vec![("token", Value::from("t"))]))]),
            ),
        ]);
        assert_eq!(
            redacted(&v),
            "{text: <str 17>, phone: <hidden>, token: <hidden>, verifyCode: <hidden>, name: <str 4>, \
             authTokenType: \"CHECK_CODE\", tokenAttrs: {LOGIN: {token: <hidden>}}}"
        );
    }

    #[test]
    fn keeps_error_texts_and_url_hosts() {
        let v = map(vec![
            ("error", Value::from("proto.payload")),
            ("localizedMessage", Value::from("Ошибка")),
            ("url", Value::from("https://i.oneme.ru/i?r=secret")),
        ]);
        assert_eq!(
            redacted(&v),
            r#"{error: "proto.payload", localizedMessage: "Ошибка", url: <url i.oneme.ru 29>}"#
        );
    }

    #[test]
    fn long_arrays_are_cut() {
        let v = Value::Array((0..8).map(Value::from).collect());
        assert_eq!(redacted(&v), "[0, 1, 2, 3, 4, …+3]");
    }
}
