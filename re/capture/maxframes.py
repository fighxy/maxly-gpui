"""Decode MAX TCP frames from a plaintext byte stream.

Frame: 10-byte big-endian header ">BBHHI" (ver, cmd, seq, opcode, packed_len), where the top
byte of packed_len holds compression flags and the low 24 bits the payload length, followed by
the payload (msgpack, lz4-block compressed when flags is non-zero and not 0xFF).
"""

from __future__ import annotations

import hashlib
import struct
from dataclasses import dataclass, field
from typing import Any, Iterator

import lz4.block
import msgpack

HEADER = struct.Struct(">BBHHI")
MAX_PAYLOAD = 16 * 1024 * 1024

# Keys whose values identify the user, the device or a login secret.
SECRET_KEYS = {
    "token", "authtoken", "auth_token", "logintoken", "refreshtoken", "qrlink", "trackid",
    "deviceid", "devicename", "phone", "phonenumber", "verifycode", "code", "password",
    "hash", "sessionkey", "secret", "email", "firstname", "lastname", "name", "callsseed",
}


@dataclass
class Frame:
    ver: int
    cmd: int
    seq: int
    opcode: int
    flags: int
    payload_len: int
    header_hex: str
    payload: bytes = field(repr=False)


class StreamDecoder:
    """Accumulates one direction of one connection and yields complete frames."""

    def __init__(self) -> None:
        self.buf = bytearray()

    def feed(self, data: bytes) -> Iterator[Frame]:
        self.buf.extend(data)
        while len(self.buf) >= HEADER.size:
            ver, cmd, seq, opcode, packed = HEADER.unpack_from(self.buf, 0)
            flags = packed >> 24
            length = packed & 0x00FFFFFF
            total = HEADER.size + length
            if len(self.buf) < total:
                return
            header_hex = bytes(self.buf[: HEADER.size]).hex()
            payload = bytes(self.buf[HEADER.size : total])
            del self.buf[:total]
            yield Frame(ver, cmd, seq, opcode, flags, length, header_hex, payload)


def decompress(frame: Frame) -> tuple[bytes | None, str]:
    """Return (plain payload, how). `how` names the codec or the failure."""
    if frame.flags == 0:
        return frame.payload, "none"
    if frame.flags == 0xFF:
        return None, "zstd-not-supported"
    try:
        return lz4.block.decompress(frame.payload, uncompressed_size=MAX_PAYLOAD), "lz4-block"
    except lz4.block.LZ4BlockError as exc:
        return None, f"lz4-error: {exc}"


def _ext_hook(code: int, data: bytes) -> Any:
    if code == 1:
        return msgpack.unpackb(data, raw=False, strict_map_key=False, ext_hook=_ext_hook)
    return {"$ext": code, "len": len(data)}


def decode_payload(plain: bytes) -> tuple[Any, str]:
    if not plain:
        return None, "empty"
    try:
        return msgpack.unpackb(plain, raw=False, strict_map_key=False, ext_hook=_ext_hook), "msgpack"
    except msgpack.ExtraData as exc:
        return exc.unpacked, f"msgpack-extra:{len(exc.extra)}"
    except Exception as exc:  # noqa: BLE001 - record any decode failure as data
        return None, f"msgpack-error: {exc}"


def _digest(value: Any) -> str:
    return "<redacted:" + hashlib.sha256(repr(value).encode()).hexdigest()[:8] + ">"


def redact(obj: Any) -> Any:
    """Replace values of identifying keys with a short digest; make bytes JSON-safe."""
    if isinstance(obj, dict):
        out = {}
        for k, v in obj.items():
            key = str(k)
            if key.lower().replace("-", "").replace("_", "") in SECRET_KEYS and not isinstance(v, (dict, list)):
                out[key] = _digest(v)
            else:
                out[key] = redact(v)
        return out
    if isinstance(obj, list):
        return [redact(v) for v in obj]
    if isinstance(obj, (bytes, bytearray)):
        return {"$bin": len(obj)}
    return obj


def describe(frame: Frame) -> dict[str, Any]:
    plain, codec = decompress(frame)
    payload, fmt = decode_payload(plain) if plain is not None else (None, "skipped")
    return {
        "ver": frame.ver,
        "cmd": frame.cmd,
        "seq": frame.seq,
        "opcode": frame.opcode,
        "flags": frame.flags,
        "payload_len": frame.payload_len,
        "plain_len": None if plain is None else len(plain),
        "header_hex": frame.header_hex,
        "codec": codec,
        "format": fmt,
        "payload": redact(payload),
    }
