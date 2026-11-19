"""Print the msgpack type markers of the first frame in a raw capture file, without values.

For each map key: the key, the marker byte of its value and the marker's name. String values
show only their length, so no secret leaves the file.
Usage: python -I msgpack_markers.py <raw_*_out.bin>
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

NAMES = {0xC0: "nil", 0xC2: "false", 0xC3: "true", 0xCC: "uint8", 0xCD: "uint16", 0xCE: "uint32",
         0xCF: "uint64", 0xD0: "int8", 0xD1: "int16", 0xD2: "int32", 0xD3: "int64",
         0xD9: "str8", 0xDA: "str16", 0xDB: "str32", 0xDE: "map16", 0xDC: "array16"}


class Reader:
    def __init__(self, data: bytes) -> None:
        self.d = data
        self.i = 0

    def byte(self) -> int:
        b = self.d[self.i]
        self.i += 1
        return b

    def take(self, n: int) -> bytes:
        out = self.d[self.i : self.i + n]
        self.i += n
        return out

    def value(self, path: str, out: list[str]) -> str | None:
        """Read one value; returns the decoded string for keys, records markers in out."""
        m = self.byte()
        if m <= 0x7F:
            out.append(f"{path}: positive fixint")
            return None
        if 0x80 <= m <= 0x8F or m == 0xDE:
            n = m & 0x0F if m != 0xDE else struct.unpack(">H", self.take(2))[0]
            out.append(f"{path}: {'fixmap' if m != 0xDE else 'map16'}({n})")
            for _ in range(n):
                key = self.read_str()
                self.value(f"{path}.{key}" if path else key, out)
            return None
        if 0xA0 <= m <= 0xBF or m in (0xD9, 0xDA, 0xDB):
            if m <= 0xBF:
                n = m & 0x1F
            else:
                n = {0xD9: 1, 0xDA: 2, 0xDB: 4}[m]
                n = int.from_bytes(self.take(n), "big")
            self.take(n)
            out.append(f"{path}: {'fixstr' if m <= 0xBF else NAMES[m]} len={n}")
            return None
        if m in (0xCC, 0xCD, 0xCE, 0xCF, 0xD0, 0xD1, 0xD2, 0xD3):
            width = {0xCC: 1, 0xCD: 2, 0xCE: 4, 0xCF: 8, 0xD0: 1, 0xD1: 2, 0xD2: 4, 0xD3: 8}[m]
            self.take(width)
            out.append(f"{path}: {NAMES[m]}")
            return None
        if m in (0xC0, 0xC2, 0xC3):
            out.append(f"{path}: {NAMES[m]}")
            return None
        out.append(f"{path}: marker 0x{m:02x} (stop)")
        self.i = len(self.d)
        return None

    def read_str(self) -> str:
        m = self.byte()
        if 0xA0 <= m <= 0xBF:
            return self.take(m & 0x1F).decode()
        if m == 0xD9:
            return self.take(self.byte()).decode()
        return f"<key marker 0x{m:02x}>"


def main() -> None:
    data = Path(sys.argv[1]).read_bytes()
    length = int.from_bytes(data[6:10], "big") & 0xFFFFFF
    print(f"header {data[:10].hex()} payload_len={length}")
    out: list[str] = []
    Reader(data[10 : 10 + length]).value("", out)
    print("\n".join(out))


if __name__ == "__main__":
    main()
