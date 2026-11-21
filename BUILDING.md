# Building maxly-gpui

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
  `C1083: Invalid argument` when paths get too long; `C:\dev\maxly-gpui\target` is fine.
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
cargo run -p maxly-gpui -- --demo        # made-up data, no network
cargo run -p maxly-gpui -- --demo-walk   # steps through every screen on a timer
cargo run -p maxly-gpui                  # connects to api2.oneme.ru
cargo run -p maxly-gpui -- --trace       # same, with a local protocol trace (rust/research/live-check.md)
```

`tools/window-shot.ps1 -ProcessName maxly-gpui -Out shot.png` captures only the client window;
`tools/window-click.ps1 -ProcessName maxly-gpui -X 289 -Y 25` posts a click to it without moving the cursor;
`tools/window-type.ps1 -ProcessName maxly-gpui -Text "Hi" -Enter` posts text the same way.

## Linux

The same commands. The client still tells the server it is MAX Desktop for Windows (ADR-003).
Files go to `$XDG_DATA_HOME/maxly-gpui` (usually `~/.local/share/maxly-gpui`). The database key
and the login token go to the Secret Service. A second start talks to the first through
`$XDG_RUNTIME_DIR/maxly-gpui.sock`.

OpenSSL inside SQLCipher needs a full Perl (`/usr/bin/perl` is enough; set `OPENSSL_SRC_PERL`
if `perl` is not on `PATH`). The first build of `max-store` compiles OpenSSL and takes several
minutes.

Notifications need a daemon that speaks the freedesktop notification spec (`dunst`, `mako`,
the GNOME or KDE daemon). The tray speaks StatusNotifier, not `libappindicator`: KDE, waybar
and swaybar show it; GNOME needs the AppIndicator extension.

### Arch

```sh
sudo pacman -S --needed rust perl pkgconf dbus libxkbcommon wayland vulkan-icd-loader fontconfig libxcb
# One secret store. gnome-keyring also works outside GNOME if its daemon is running.
sudo pacman -S --needed gnome-keyring
# or, on KDE: sudo pacman -S --needed kwallet
export OPENSSL_SRC_PERL=/usr/bin/perl
cargo run -p maxly-gpui -- --demo
```

A package is in `packaging/arch`. From a checkout:

```sh
cd packaging/arch
makepkg -si
```

`makepkg` fetches the public git repository, so it builds what is on the remote, not uncommitted
work in the working tree. For that, use `cargo` as above.

## Reverse engineering tools

See `re/README.md` (Ghidra, rea, Frida).
