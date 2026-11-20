# Building maxrs

## Windows

- Rust (MSVC toolchain), with the Visual Studio Build Tools C++ workload (`cl`, `nmake`, `link`).
- A full Perl for the OpenSSL build behind SQLCipher (`max-store`). Perl from Git for Windows
  does not work (it lacks modules OpenSSL's `Configure` needs). Use portable Strawberry Perl,
  unpacked without installing, and point the build at it:

  ```powershell
  [Environment]::SetEnvironmentVariable('OPENSSL_SRC_PERL', "$env:USERPROFILE\.local\strawberry-perl\perl\bin\perl.exe", 'User')
  ```

  New shells see the variable; an already open shell needs it exported by hand.
- A short path to the target directory. OpenSSL's build tree is deep, and MSVC fails with
  `C1083: Invalid argument` when paths get too long; `C:\dev\maxrs\target` is fine.
- NASM is not needed: OpenSSL is built with `no-asm`.

The first build of `max-store` compiles OpenSSL and SQLCipher and takes about 4 minutes;
later builds reuse it.

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

`max-store` has one test that writes to the Windows Credential Manager and deletes what it
writes; it is ignored by default:

```sh
cargo test -p max-store --test os_secrets -- --ignored
```

## Running the client

```sh
cargo run -p maxrs -- --demo        # made-up data, no network
cargo run -p maxrs -- --demo-walk   # steps through every screen on a timer
cargo run -p maxrs                  # connects to api2.oneme.ru
cargo run -p maxrs -- --trace       # same, with a local protocol trace (rust/research/live-check.md)
```

`tools/window-shot.ps1 -ProcessName maxrs -Out shot.png` captures only the client window;
`tools/window-click.ps1 -ProcessName maxrs -X 289 -Y 25` posts a click to it without moving the cursor.

## Reverse engineering tools

See `re/README.md` (Ghidra, rea, Frida).
