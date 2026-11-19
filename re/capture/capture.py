"""Spawn the official MAX client under Frida and record its decoded API frames.

Usage (from C:\\dev\\maxrs):
  re\\.venv-frida\\Scripts\\python.exe -I re\\capture\\capture.py --seconds 40 --out re\\out\\cap1

Writes into --out:
  frames.jsonl  one line per api2.oneme.ru frame, identifying values redacted
  other.jsonl   other TLS hosts: host, direction, byte count, first HTTP line only
  raw_*.bin     raw plaintext per connection and direction, only with --raw
The client is killed after --seconds. Do not log in: nothing here needs an account.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path

import frida

sys.path.insert(0, str(Path(__file__).resolve().parent))
from maxframes import StreamDecoder, describe  # noqa: E402

API_HOST = "api2.oneme.ru"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=r"C:\MAX\max.exe")
    ap.add_argument("--seconds", type=float, default=40.0)
    ap.add_argument("--out", required=True)
    ap.add_argument("--raw", action="store_true", help="also keep raw plaintext (contains secrets)")
    args = ap.parse_args()

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=False)
    frames_f = (out / "frames.jsonl").open("w", encoding="utf-8")
    other_f = (out / "other.jsonl").open("w", encoding="utf-8")
    decoders: dict[tuple[str, str], StreamDecoder] = {}
    raw_files = {}
    lock = threading.Lock()
    t0 = time.monotonic()
    counts = {"frames": 0, "other": 0}

    def on_message(message: dict, data: bytes | None) -> None:
        if message.get("type") == "error":
            print("agent error:", message.get("description"), file=sys.stderr)
            return
        msg = message.get("payload", {})
        if msg.get("t") != "ssl":
            print("agent:", msg)
            return
        ts = round(time.monotonic() - t0, 3)
        key = (msg["ssl"], msg["dir"])
        with lock:
            if msg["host"] != API_HOST:
                first = data.split(b"\r\n", 1)[0][:200].decode("latin-1") if data else ""
                looks_http = first.split(" ", 1)[0] in {"GET", "POST", "PUT", "HEAD", "DELETE", "PATCH"} or first.startswith("HTTP/")
                other_f.write(json.dumps({"ts": ts, "host": msg["host"], "dir": msg["dir"], "ssl": msg["ssl"],
                                          "bytes": len(data or b""), "http": first if looks_http else None}) + "\n")
                other_f.flush()
                counts["other"] += 1
                return
            if args.raw:
                f = raw_files.get(key)
                if f is None:
                    f = raw_files[key] = (out / f"raw_{msg['ssl']}_{msg['dir']}.bin").open("wb")
                f.write(data or b"")
                f.flush()
            dec = decoders.setdefault(key, StreamDecoder())
            for frame in dec.feed(data or b""):
                rec = {"ts": ts, "dir": msg["dir"], "ssl": msg["ssl"], "fn": msg["fn"], **describe(frame)}
                frames_f.write(json.dumps(rec, ensure_ascii=False, default=str) + "\n")
                frames_f.flush()
                counts["frames"] += 1
                print(f"{ts:7.3f} {msg['dir']:>3} ver={frame.ver} cmd={frame.cmd} seq={frame.seq} "
                      f"op={frame.opcode} flags={frame.flags:#x} len={frame.payload_len}")

    agent = (Path(__file__).resolve().parent / "ssl_hook.js").read_text(encoding="utf-8")
    device = frida.get_local_device()
    pid = device.spawn([args.exe])
    session = device.attach(pid)
    script = session.create_script(agent)
    script.on("message", on_message)
    script.load()
    device.resume(pid)
    print(f"spawned {args.exe} pid={pid}; recording {args.seconds}s; do not scan the QR code")
    try:
        time.sleep(args.seconds)
    finally:
        # device.kill() has hung after recording; kill the process tree from outside Frida.
        subprocess.run(["taskkill", "/F", "/T", "/PID", str(pid)], capture_output=True, timeout=30, check=False)
        got_lock = lock.acquire(timeout=10)
        frames_f.close()
        other_f.close()
        for f in raw_files.values():
            f.close()
        leftover = {f"{k[0]}:{k[1]}": len(d.buf) for k, d in decoders.items() if d.buf}
        if got_lock:
            lock.release()
    print(f"done: {counts['frames']} api frames, {counts['other']} other chunks; incomplete tails: {leftover}", flush=True)
    # Frida's session teardown can hang after the target is killed; everything is on disk now.
    os._exit(0)


if __name__ == "__main__":
    raise SystemExit(main())
