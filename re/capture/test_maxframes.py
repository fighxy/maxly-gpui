"""Self-check for maxframes on frames with known content (no network, no Frida)."""

from __future__ import annotations

import sys
from pathlib import Path

import lz4.block
import msgpack

sys.path.insert(0, str(Path(__file__).resolve().parent))
from maxframes import HEADER, StreamDecoder, describe  # noqa: E402

# Known frame from github.com/nyakokitsu/MaxProtoExplanation: ver 10, seq 7, opcode 17 (AUTH_REQUEST).
KNOWN = bytes.fromhex(
    "0a00000700110000003083a570686f6e65ac2b3739393939393939393939a474797065aa53544152545f41555448"
    "a86c616e6775616765a27275"
)


def frame(ver: int, cmd: int, seq: int, op: int, flags: int, payload: bytes) -> bytes:
    return HEADER.pack(ver, cmd, seq, op, (flags << 24) | len(payload)) + payload


def main() -> None:
    dec = StreamDecoder()
    (f,) = list(dec.feed(KNOWN))
    d = describe(f)
    assert (d["ver"], d["cmd"], d["seq"], d["opcode"], d["flags"]) == (10, 0, 7, 17, 0), d
    assert d["payload"]["type"] == "START_AUTH" and d["payload"]["phone"].startswith("<redacted:"), d

    # Split across chunks, two frames back to back, second one lz4-compressed.
    body = msgpack.packb({"chats": [{"id": i, "title": "x" * 40} for i in range(50)]})
    comp = lz4.block.compress(body, store_size=False)
    stream = frame(11, 1, 2, 53, 0, msgpack.packb({"ok": True})) + frame(11, 1, 3, 53, 1, comp)
    dec = StreamDecoder()
    got = []
    for i in range(0, len(stream), 7):
        got.extend(dec.feed(stream[i : i + 7]))
    assert [g.seq for g in got] == [2, 3] and not dec.buf
    d2 = describe(got[1])
    assert d2["codec"] == "lz4-block" and d2["plain_len"] == len(body) and len(d2["payload"]["chats"]) == 50, d2
    print("maxframes ok")


if __name__ == "__main__":
    main()
