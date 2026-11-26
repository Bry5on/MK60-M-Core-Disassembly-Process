# Building the Windows binaries from macOS (no Windows machine)

The CLI (`e46-mk60-tool.exe`) and GUI (`e46-mk60-tool-gui.exe`) cross-compile from macOS to the Windows
MSVC targets with `cargo-xwin`, which downloads the MSVC CRT + Windows SDK (cached after the
first run). The vendored libusb (C) compiles under clang, so no system libusb or Windows
toolchain is needed. Two architectures are built:

- `x86_64-pc-windows-msvc` - 64-bit Intel/AMD (runs on ARM Windows via x64 emulation).
- `aarch64-pc-windows-msvc` - 64-bit ARM (native on Snapdragon/Surface ARM machines).

The GUI renderer is eframe's glow (OpenGL) backend everywhere EXCEPT the ARM64 Windows build, which
uses the wgpu (Direct3D 12) backend: native ARM Windows has no hardware OpenGL, so glow fails with
"egui_glow requires opengl 2.0+" and the window never opens. The wgpu feature is compiled only for
that target (gui/Cargo.toml), and main() selects it at runtime under the same cfg.

Both link a **static CRT** (`-C target-feature=+crt-static`), so each `.exe` runs standalone -
no Visual C++ redistributable needed. Minimum OS is Windows 10 (Rust's MSVC targets dropped
Windows 7/8.1). The GUI uses the Windows subsystem in release builds, so no console window
opens (debug builds keep the console for panics/logs).

## One-time setup

rustup is installed with `--no-modify-path`, so the shell default stays whatever Rust is on
PATH (e.g. Homebrew). Drive the cross toolchain by explicit `~/.cargo/bin` paths.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal --default-toolchain stable
~/.cargo/bin/rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc
~/.cargo/bin/cargo install cargo-xwin
```

## Build both apps in one shot (recommended)

`gui/make-app.sh` builds the macOS `.app` AND, when `cargo-xwin` is installed, both Windows
`.exe`s, from the same source and the same freshly-embedded firmware, so they never drift. It
drops them next to each other in `gui/`:

- `gui/E46 MK60 Tool.app` (macOS)
- `gui/E46 MK60 Tool x64.exe` (Windows, 64-bit Intel/AMD, static CRT)
- `gui/E46 MK60 Tool arm64.exe` (Windows, 64-bit ARM, static CRT)

```bash
bench/gui/make-app.sh
```

If `cargo-xwin` is missing it still builds the macOS app and prints a loud warning that the
Windows builds were skipped (so a macOS-only update can't slip out silently).

## Build manually

The GUI is kept out of the workspace (heavy egui deps, its own lock), so build it separately.
`-C target-feature=+crt-static` gives the standalone static-CRT exe, and `--remap-path-prefix=$HOME=~`
keeps the build machine's paths (incl. `~/.cargo/registry/...`, which contains the username) out of the
shipped binary - always build release with it. Swap the triple for `aarch64-pc-windows-msvc` to build ARM64.

```bash
# CLI (workspace: core + host)
cd bench && PATH="$HOME/.cargo/bin:$PATH" RUSTFLAGS="-C target-feature=+crt-static --remap-path-prefix=$HOME=~" cargo xwin build --release --target x86_64-pc-windows-msvc
```

```bash
# GUI
cd bench/gui && PATH="$HOME/.cargo/bin:$PATH" RUSTFLAGS="-C target-feature=+crt-static --remap-path-prefix=$HOME=~" cargo xwin build --release --target x86_64-pc-windows-msvc
```

Artifacts (drop `--release` for debug; swap the triple for the ARM64 path):

- `bench/target/x86_64-pc-windows-msvc/release/e46-mk60-tool.exe`
- `bench/gui/target/x86_64-pc-windows-msvc/release/e46-mk60-tool-gui.exe`

## Windows runtime notes

The core Gauge.S flow (flashing, relay, CAN, serial dumps) and the cable K-line all go over a
COM port via `serialport` and work once the port is selected. Two things need Windows-specific
attention:

- FTDI latency timer. The macOS EP0 latency trick does not apply on Windows. The cable knock
  needs the FTDI VCP latency timer at 1 ms (driver default is 16 ms, too slow for the module's
  ~130 ms window). Set it in Device Manager: Ports (COM & LPT) -> COMx -> Properties ->
  Port Settings -> Advanced -> Latency Timer -> 1, then replug. The tool reads this value from
  the registry on cable entry and warns if it is not set.

- Direct-USB paths (`ch341-dump`, `ftdi-latency-test`) use libusb, which on Windows only sees a
  device bound to WinUSB (via Zadig). The FTDI cable is normally bound to the VCP driver for its
  COM port, so those two commands need a driver swap and are not needed for the main bench flow.

The `.exe`s link a static CRT, so they run with no Visual C++ redistributable. The release GUI
uses `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`, so no console window
opens; a debug build keeps the console for panics/logs.
