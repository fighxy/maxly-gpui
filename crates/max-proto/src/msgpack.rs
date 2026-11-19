//! A msgpack serializer that writes the way the official desktop client does.
//!
//! Capture `cap2` (raw `SESSION_INIT` bytes, 2026-11-19): strings use the shortest header
//! (`fixstr` below 32 bytes, then `str8`), but integers keep the width of their C++ type:
//! `buildNumber` (`int`) is `int32` (`0xd2`) and `clientSessionId` (`int64`) is `int64` (`0xd3`)
//! even for the value 1. So here every Rust integer type maps to its fixed-width marker:
//! `i8..i64` to `int8..int64`, `u8..u64` to `uint8..uint64` (the unsigned mapping is not yet
//! confirmed by a capture). Structs are maps with field names, in declaration order.

use serde::ser::{self, Serialize};

use crate::{ProtoError, Result};

/// Encode a value with desktop-style msgpack.
pub fn to_vec<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut enc = Encoder {
        out: Vec::with_capacity(128),
    };
    value
        .serialize(&mut enc)
        .map_err(|e| ProtoError::Encode(e.0))?;
    Ok(enc.out)
}

#[derive(Debug)]
struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl ser::Error for Error {
    fn custom<T: std::fmt::Display>(msg: T) -> Self {
        Error(msg.to_string())
    }
}

struct Encoder {
    out: Vec<u8>,
}

impl Encoder {
    fn str_header(&mut self, len: usize) -> Result<(), Error> {
        match len {
            0..=31 => self.out.push(0xa0 | len as u8),
            32..=0xff => self.out.extend_from_slice(&[0xd9, len as u8]),
            0x100..=0xffff => {
                self.out.push(0xda);
                self.out.extend_from_slice(&(len as u16).to_be_bytes());
            }
            _ => {
                let n = u32::try_from(len).map_err(|_| Error("string longer than 4 GiB".into()))?;
                self.out.push(0xdb);
                self.out.extend_from_slice(&n.to_be_bytes());
            }
        }
        Ok(())
    }

    fn container_header(&mut self, fix: u8, m16: u8, m32: u8, len: usize) -> Result<(), Error> {
        match len {
            0..=15 => self.out.push(fix | len as u8),
            16..=0xffff => {
                self.out.push(m16);
                self.out.extend_from_slice(&(len as u16).to_be_bytes());
            }
            _ => {
                let n = u32::try_from(len).map_err(|_| Error("container too large".into()))?;
                self.out.push(m32);
                self.out.extend_from_slice(&n.to_be_bytes());
            }
        }
        Ok(())
    }

    fn array_header(&mut self, len: usize) -> Result<(), Error> {
        self.container_header(0x90, 0xdc, 0xdd, len)
    }

    fn map_header(&mut self, len: usize) -> Result<(), Error> {
        self.container_header(0x80, 0xde, 0xdf, len)
    }

    fn compound(&mut self, kind: Kind, len: Option<usize>) -> Result<Compound<'_>, Error> {
        match len {
            Some(n) => {
                match kind {
                    Kind::Array => self.array_header(n)?,
                    Kind::Map => self.map_header(n)?,
                }
                Ok(Compound {
                    enc: self,
                    kind,
                    pending: None,
                })
            }
            None => {
                let start = self.out.len();
                Ok(Compound {
                    enc: self,
                    kind,
                    pending: Some((start, 0)),
                })
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Array,
    Map,
}

/// A sequence or map; `pending` holds (start offset, item count) when the length was unknown
/// up front, and the header is inserted at `end`.
struct Compound<'a> {
    enc: &'a mut Encoder,
    kind: Kind,
    pending: Option<(usize, usize)>,
}

impl Compound<'_> {
    fn count(&mut self) {
        if let Some((_, n)) = &mut self.pending {
            *n += 1;
        }
    }

    fn finish(self) -> Result<(), Error> {
        if let Some((start, n)) = self.pending {
            let body = self.enc.out.split_off(start);
            match self.kind {
                Kind::Array => self.enc.array_header(n)?,
                Kind::Map => self.enc.map_header(n)?,
            }
            self.enc.out.extend_from_slice(&body);
        }
        Ok(())
    }
}

impl<'a> ser::Serializer for &'a mut Encoder {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Compound<'a>;
    type SerializeTuple = Compound<'a>;
    type SerializeTupleStruct = Compound<'a>;
    type SerializeTupleVariant = Compound<'a>;
    type SerializeMap = Compound<'a>;
    type SerializeStruct = Compound<'a>;
    type SerializeStructVariant = Compound<'a>;

    fn serialize_bool(self, v: bool) -> Result<(), Error> {
        self.out.push(if v { 0xc3 } else { 0xc2 });
        Ok(())
    }
    fn serialize_i8(self, v: i8) -> Result<(), Error> {
        self.out.push(0xd0);
        self.out.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_i16(self, v: i16) -> Result<(), Error> {
        self.out.push(0xd1);
        self.out.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_i32(self, v: i32) -> Result<(), Error> {
        self.out.push(0xd2);
        self.out.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_i64(self, v: i64) -> Result<(), Error> {
        self.out.push(0xd3);
        self.out.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_u8(self, v: u8) -> Result<(), Error> {
        self.out.extend_from_slice(&[0xcc, v]);
        Ok(())
    }
    fn serialize_u16(self, v: u16) -> Result<(), Error> {
        self.out.push(0xcd);
        self.out.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_u32(self, v: u32) -> Result<(), Error> {
        self.out.push(0xce);
        self.out.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_u64(self, v: u64) -> Result<(), Error> {
        self.out.push(0xcf);
        self.out.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_f32(self, v: f32) -> Result<(), Error> {
        self.out.push(0xca);
        self.out.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_f64(self, v: f64) -> Result<(), Error> {
        self.out.push(0xcb);
        self.out.extend_from_slice(&v.to_be_bytes());
        Ok(())
    }
    fn serialize_char(self, v: char) -> Result<(), Error> {
        self.serialize_str(v.encode_utf8(&mut [0u8; 4]))
    }
    fn serialize_str(self, v: &str) -> Result<(), Error> {
        self.str_header(v.len())?;
        self.out.extend_from_slice(v.as_bytes());
        Ok(())
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<(), Error> {
        match v.len() {
            0..=0xff => self.out.extend_from_slice(&[0xc4, v.len() as u8]),
            0x100..=0xffff => {
                self.out.push(0xc5);
                self.out.extend_from_slice(&(v.len() as u16).to_be_bytes());
            }
            _ => {
                let n =
                    u32::try_from(v.len()).map_err(|_| Error("bytes longer than 4 GiB".into()))?;
                self.out.push(0xc6);
                self.out.extend_from_slice(&n.to_be_bytes());
            }
        }
        self.out.extend_from_slice(v);
        Ok(())
    }
    fn serialize_none(self) -> Result<(), Error> {
        self.out.push(0xc0);
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Error> {
        self.out.push(0xc0);
        Ok(())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Error> {
        self.serialize_unit()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<(), Error> {
        self.serialize_str(variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.map_header(1)?;
        self.serialize_str(variant)?;
        value.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Compound<'a>, Error> {
        self.compound(Kind::Array, len)
    }
    fn serialize_tuple(self, len: usize) -> Result<Compound<'a>, Error> {
        self.compound(Kind::Array, Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Compound<'a>, Error> {
        self.compound(Kind::Array, Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Compound<'a>, Error> {
        self.map_header(1)?;
        self.serialize_str(variant)?;
        self.compound(Kind::Array, Some(len))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Compound<'a>, Error> {
        self.compound(Kind::Map, len)
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Compound<'a>, Error> {
        self.compound(Kind::Map, Some(len))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Compound<'a>, Error> {
        self.map_header(1)?;
        self.serialize_str(variant)?;
        self.compound(Kind::Map, Some(len))
    }
}

impl ser::SerializeSeq for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.count();
        value.serialize(&mut *self.enc)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeTuple for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        value.serialize(&mut *self.enc)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeTupleStruct for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        value.serialize(&mut *self.enc)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeTupleVariant for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        value.serialize(&mut *self.enc)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeMap for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        self.count();
        key.serialize(&mut *self.enc)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        value.serialize(&mut *self.enc)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeStruct for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.enc.serialize_str_key(key)?;
        value.serialize(&mut *self.enc)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for Compound<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.enc.serialize_str_key(key)?;
        value.serialize(&mut *self.enc)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl Encoder {
    fn serialize_str_key(&mut self, key: &str) -> Result<(), Error> {
        self.str_header(key.len())?;
        self.out.extend_from_slice(key.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Serialize;

    use super::*;

    #[test]
    fn integers_keep_their_width() {
        assert_eq!(to_vec(&1i64).unwrap(), [0xd3, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(to_vec(&83760i32).unwrap(), [0xd2, 0, 1, 0x47, 0x30]);
        assert_eq!(to_vec(&-1i8).unwrap(), [0xd0, 0xff]);
        assert_eq!(to_vec(&7u64).unwrap(), [0xcf, 0, 0, 0, 0, 0, 0, 0, 7]);
    }

    #[test]
    fn strings_use_the_shortest_header() {
        assert_eq!(to_vec("ru").unwrap(), b"\xa2ru");
        let s31 = "a".repeat(31);
        assert_eq!(to_vec(&s31).unwrap()[0], 0xbf);
        let s38 = "a".repeat(38);
        assert_eq!(&to_vec(&s38).unwrap()[..2], &[0xd9, 38]);
        let s300 = "a".repeat(300);
        assert_eq!(&to_vec(&s300).unwrap()[..3], &[0xda, 0x01, 0x2c]);
    }

    #[test]
    fn struct_is_a_named_map_in_order() {
        #[derive(Serialize)]
        struct S {
            b: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            skip: Option<i64>,
            a: &'static str,
        }
        let bytes = to_vec(&S {
            b: true,
            skip: None,
            a: "x",
        })
        .unwrap();
        assert_eq!(bytes, b"\x82\xa1b\xc3\xa1a\xa1x");
    }

    #[test]
    fn unknown_length_containers_get_a_header() {
        struct Iter;
        impl Serialize for Iter {
            fn serialize<S: ser::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_seq((0..20).map(|i| i as u8))
            }
        }
        let bytes = to_vec(&Iter).unwrap();
        assert_eq!(&bytes[..3], &[0xdc, 0x00, 20]);
        assert_eq!(bytes.len(), 3 + 20 * 2);
        let map: BTreeMap<&str, bool> = [("k", false)].into_iter().collect();
        assert_eq!(to_vec(&map).unwrap(), b"\x81\xa1k\xc2");
    }

    #[test]
    fn empty_struct_is_an_empty_map() {
        #[derive(Serialize)]
        struct Empty {}
        assert_eq!(to_vec(&Empty {}).unwrap(), [0x80]);
    }
}
