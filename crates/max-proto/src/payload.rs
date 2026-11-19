//! msgpack payloads: typed encoding with field names (maps, struct field order kept) and
//! decoding through [`rmpv::Value`], unwrapping ext type 1 (a nested msgpack value).

use rmpv::Value;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::{ProtoError, Result};

/// msgpack ext type that wraps another msgpack value (PyMax `payload.py`; not yet seen from
/// the desktop server).
pub const WRAPPED_VALUE_EXT: i8 = 1;

/// Encode a value the way the desktop client does: maps with field names in declaration order,
/// shortest string headers, fixed-width integers (see [`crate::msgpack`]).
pub fn to_msgpack<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    crate::msgpack::to_vec(value)
}

/// Decode a plain payload to a value tree. An empty payload is `Nil`.
pub fn decode_value(plain: &[u8]) -> Result<Value> {
    if plain.is_empty() {
        return Ok(Value::Nil);
    }
    let mut rd = plain;
    let value = rmpv::decode::read_value(&mut rd).map_err(|e| ProtoError::Decode(e.to_string()))?;
    unwrap_ext(value)
}

/// Decode a plain payload into a type.
pub fn from_msgpack<T: DeserializeOwned>(plain: &[u8]) -> Result<T> {
    let value = decode_value(plain)?;
    rmpv::ext::from_value(value).map_err(|e| ProtoError::Decode(e.to_string()))
}

fn unwrap_ext(value: Value) -> Result<Value> {
    Ok(match value {
        Value::Ext(WRAPPED_VALUE_EXT, data) => decode_value(&data)?,
        Value::Array(items) => {
            Value::Array(items.into_iter().map(unwrap_ext).collect::<Result<_>>()?)
        }
        Value::Map(entries) => Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| Ok((unwrap_ext(k)?, unwrap_ext(v)?)))
                .collect::<Result<_>>()?,
        ),
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Serialize)]
    struct Empty {}

    #[test]
    fn empty_struct_is_an_empty_map() {
        // CreateQr request: payload_len 1 in capture cap1.
        assert_eq!(to_msgpack(&Empty {}).unwrap(), [0x80]);
    }

    #[test]
    fn empty_payload_is_nil() {
        assert_eq!(decode_value(&[]).unwrap(), Value::Nil);
    }

    #[test]
    fn unwraps_nested_ext() {
        #[derive(Deserialize, Debug, PartialEq)]
        struct Inner {
            a: u8,
        }
        #[derive(Deserialize, Debug, PartialEq)]
        struct Outer {
            inner: Inner,
        }
        let inner = to_msgpack(&serde_json_like_map()).unwrap();
        let mut outer = vec![0x81, 0xa5];
        outer.extend_from_slice(b"inner");
        // ext8: 0xc7, len, type
        outer.extend_from_slice(&[0xc7, u8::try_from(inner.len()).unwrap(), 1]);
        outer.extend_from_slice(&inner);
        let decoded: Outer = from_msgpack(&outer).unwrap();
        assert_eq!(
            decoded,
            Outer {
                inner: Inner { a: 5 }
            }
        );
    }

    fn serde_json_like_map() -> std::collections::BTreeMap<&'static str, u8> {
        [("a", 5u8)].into_iter().collect()
    }

    #[test]
    fn truncated_msgpack_is_an_error() {
        // An array of two elements that holds only one.
        assert!(matches!(
            decode_value(&[0x92, 0x01]),
            Err(ProtoError::Decode(_))
        ));
    }
}
