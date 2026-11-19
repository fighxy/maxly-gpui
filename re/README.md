# Reverse engineering of the official MAX desktop client

Target: the installed client in `C:\MAX` (read-only reference; never write into it).
Engine: Ghidra 12.1.4 via `rea` (CLI). Snapshots and outputs stay in this folder.

## Environment

- `GHIDRA_INSTALL_DIR=%USERPROFILE%\.local\ghidra_12.1.4_PUBLIC` (user env var; JDK 21 at `~/.local/jdk-21`)
- `rea` 6.8.0 lives in the Claude Desktop MSIX private storage, so `rea setup` refuses MCP
  registration. Use the CLI from this app, or reinstall from a normal terminal:
  `npm install -g rea-agents` then `rea setup --client claude_code --yes`.
- PyMax reference venv: `ref/pymax/.venv` (Python 3.14 via uv). Put `ref/pymax/.venv/Scripts`
  first on PATH when running `rust-anything` against PyMax.

## Usage

Always pass `--provider ghidra` together with `--snapshot`, otherwise the snapshot reports
`profile_mismatch`.

`rea` re-imports the binary on every CLI call (core.dll: ~8 min per query, needs
`REA_GHIDRA_STARTUP_TIMEOUT_MS=3600000` and `GHIDRA_HEADLESS_MAXMEM=12G`). Use it for
one-off overviews only.

## Fast path: persistent Ghidra project (use this)

`re/ghidra/max.gpr` holds analyzed `core.dll` and `config.dll` (import once, ~8 min for
core.dll). Queries run in ~6 s with `re/scripts/gh.sh <program> <Script.java> [args]`.
Put regexes and selector lists in files under `re/q/` and pass `@C:\...\file`: `cmd.exe`
eats `| ^ & < >` on the command line.

| Script | Args | Output |
|---|---|---|
| `FindStringRefs.java` | `@regex-file out.tsv` | string address, referencing function, string |
| `ListFunctions.java` | `@regex-file out.tsv` | entry, size, callers, full name |
| `Decompile.java` | `out-dir @selectors` (`0xaddr`, `name:regex`, `callers:0xaddr`) | one `.c` per function + `index.tsv` |
| `SymbolAt.java` | `0xaddr...` | nearest symbol and references to it |
| `ExtractSchemas.java` | `out.tsv` | every `SerializableMember` struct: key, type, flag (core.dll: ~80 s) |
| `ListVtables.java` | `@regex-file out.tsv` | vftable counts by template name |

Packet schemas end to end:

```sh
bash re/scripts/gh.sh core.dll ExtractSchemas.java 'C:\dev\maxrs\re\out\schemas.tsv'
cp re/out/schemas.tsv rust/research/packet-schemas.tsv
node re/scripts/schemas_md.mjs rust/research/packet-schemas.tsv rust/research/packet-schemas.md
node re/scripts/gen_schema_rs.mjs rust/research/packet-schemas.tsv crates/max-proto/src/schema.rs
cargo fmt --all
```

Import another binary:

```sh
cmd //c "%GHIDRA_INSTALL_DIR%\support\analyzeHeadless.bat" 'C:\dev\maxrs\re\ghidra' max -import 'C:\MAX\<file>.dll' -max-cpu 16
```

String-only extractors (no Ghidra): `re/scripts/opcodes.sh` (opcode table from
`CommonPacket<...>` template names), `re/scripts/srcpaths.sh` (build source paths).

## Traffic capture (no login needed)

Frida 17 venv: `re/.venv-frida` (frida, msgpack, lz4). The client uses OpenSSL
(`QT_TLS_BACKEND=openssl`), so `re/capture/ssl_hook.js` hooks `SSL_read/_ex`, `SSL_write/_ex`
in `libssl-3-x64.dll`.

```sh
re/.venv-frida/Scripts/python.exe -I re/capture/test_maxframes.py
re/.venv-frida/Scripts/python.exe -I re/capture/capture.py --seconds 40 --out 're\out\cap2'
```

Spawns `C:\MAX\max.exe`, records, kills it. `frames.jsonl` redacts identifying values
(tokens, QR link, track id, device id and name, phone, names); `other.jsonl` keeps only
host, size and the first HTTP line for non-API hosts; `--raw` also keeps raw plaintext
(contains secrets). Never scan the QR code during a capture.

## Done

| Binary     | Functions | Strings | Where |
|------------|----------:|--------:|-------|
| config.dll | 2186      | 460     | project `re/ghidra`, rea snapshot |
| core.dll   | 130292    | 10716   | project `re/ghidra`, rea snapshot |

Results: `rust/research/protocol-and-telemetry.md`, `opcodes-desktop.md`,
`server-config-keys.md`.
