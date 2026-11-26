#!/usr/bin/env bash
# Build the GUI app from one source, so the macOS and Windows builds never drift:
#   - macOS:   a double-clickable "E46 MK60 Tool.app" bundle (always), plus "E46 MK60 Tool.zip" - a
#              transfer-safe ditto archive for sending the app to another Mac.
#   - Windows: "E46 MK60 Tool x64.exe" and "E46 MK60 Tool arm64.exe", cross-compiled via
#              cargo-xwin with a static CRT (when it is installed).
# All embed the same freshly-refreshed firmware. Re-run after code changes to refresh everything.
# See BUILD_WINDOWS.md for the one-time cargo-xwin setup.
set -euo pipefail
cd "$(dirname "$0")"

APP="E46 MK60 Tool.app"
BIN="e46-mk60-tool-gui"
# Windows cross targets -> output-name label. 64-bit Intel/AMD and 64-bit ARM.
WIN_TARGETS=("x86_64-pc-windows-msvc:x64" "aarch64-pc-windows-msvc:arm64")
# Keep the build machine out of the distributed binaries: remap absolute source paths (which include
# $HOME/.cargo/registry/... = the username) that rustc bakes into panic strings. $HOME is expanded here at
# build time so nothing machine-specific is committed. Applied to every release build below.
REMAP="--remap-path-prefix=$HOME=~"

# Refresh the embedded firmware images from the PlatformIO build (if it exists), so the
# flasher never ships stale firmware. Safe no-op when a build isn't present.
FWB="../firmware/.pio/build"
for pair in gaugesV4:v4 gaugesV5:v5; do
  src="$FWB/${pair%%:*}"; dst="firmware/${pair##*:}"
  if [ -f "$src/firmware.bin" ]; then
    mkdir -p "$dst"
    cp "$src/bootloader.bin" "$src/partitions.bin" "$src/firmware.bin" "$dst/"
  fi
done
BA="$HOME/.platformio/packages/framework-arduinoespressif32/tools/partitions/boot_app0.bin"
[ -f "$BA" ] && cp "$BA" firmware/

RUSTFLAGS="$REMAP" cargo build --release

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "target/release/$BIN" "$APP/Contents/MacOS/$BIN"
chmod +x "$APP/Contents/MacOS/$BIN"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>E46 MK60 Tool</string>
  <key>CFBundleDisplayName</key><string>E46 MK60 Tool</string>
  <key>CFBundleIdentifier</key><string>com.e46mk60.tool</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleExecutable</key><string>e46-mk60-tool-gui</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "built: $PWD/$APP"

# Transfer-safe archive of the .app. A raw .app emailed or zipped by Finder can lose the executable bit or
# break the ad-hoc signature, so the recipient Mac reports it "damaged". `ditto` writes a proper zip that
# preserves the bundle, perms, and signature. On the target Mac: unzip, then clear the download quarantine
# that Gatekeeper adds (`xattr -dr com.apple.quarantine "E46 MK60 Tool.app"`) before first launch - the app is
# unsigned (ad-hoc only), so any download is quarantined.
ZIP="${APP%.app}.zip"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"
echo "built: $PWD/$ZIP"

# --- Windows builds (x64 + ARM64), kept in lockstep with the macOS build above ---
# Same source, same firmware just refreshed into firmware/ (build.rs embeds it for every target).
# Static CRT (+crt-static) so each exe runs standalone - no VC++ redistributable needed. Gated on
# cargo-xwin so this script still works without the cross toolchain; when it is missing we SKIP the
# Windows builds loudly rather than silently shipping a macOS-only update.
if [ -x "$HOME/.cargo/bin/cargo-xwin" ]; then
  rm -f "E46 MK60 Tool.exe" # drop the old unlabeled artifact from before the x64/arm64 split
  for pair in "${WIN_TARGETS[@]}"; do
    t="${pair%%:*}"; label="${pair##*:}"; out="E46 MK60 Tool $label.exe"
    PATH="$HOME/.cargo/bin:$PATH" RUSTFLAGS="-C target-feature=+crt-static $REMAP" \
      cargo xwin build --release --target "$t"
    cp "target/$t/release/$BIN.exe" "$out"
    echo "built: $PWD/$out"
  done
else
  echo "WARNING: cargo-xwin not found at ~/.cargo/bin/cargo-xwin - SKIPPED the Windows builds."
  echo "         The macOS .app is up to date but the Windows .exe files were NOT rebuilt and may be stale."
  echo "         Install the cross toolchain (see ../BUILD_WINDOWS.md) to keep them in sync."
fi
