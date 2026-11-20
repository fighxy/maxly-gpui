"""Find PEM certificates inside compressed Qt resources of a binary.

Qt rcc stores compressed files as a 4-byte big-endian size followed by a zlib stream, or as a
zstd frame. This scans for both, decompresses each candidate and keeps the ones holding
"-----BEGIN CERTIFICATE-----". Output: one .pem per hit in --out, named by file offset.
Usage: python -I find_pem.py <binary> --out <dir>
"""

from __future__ import annotations

import argparse
import zlib
from pathlib import Path

MARK = b"-----BEGIN CERTIFICATE-----"


def zlib_hits(data: bytes):
    for i in range(len(data) - 2):
        if data[i] == 0x78 and data[i + 1] in (0x01, 0x5E, 0x9C, 0xDA):
            try:
                d = zlib.decompressobj()
                out = d.decompress(data[i : i + 1 << 20], 1 << 22)
            except zlib.error:
                continue
            if MARK in out:
                yield i, "zlib", out


def zstd_hits(data: bytes):
    try:
        import zstandard
    except ImportError:
        return
    magic = b"\x28\xb5\x2f\xfd"
    start = 0
    while (i := data.find(magic, start)) >= 0:
        start = i + 1
        try:
            out = zstandard.ZstdDecompressor().decompressobj().decompress(data[i : i + (1 << 20)])
        except zstandard.ZstdError:
            continue
        if MARK in out:
            yield i, "zstd", out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("binary")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    data = Path(args.binary).read_bytes()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    seen = set()
    for off, kind, blob in [*zlib_hits(data), *zstd_hits(data)]:
        if blob in seen:
            continue
        seen.add(blob)
        path = out / f"{Path(args.binary).name}.{off:08x}.{kind}.pem"
        path.write_bytes(blob)
        print(f"{off:#010x} {kind} {len(blob)} bytes -> {path.name}")


if __name__ == "__main__":
    main()
