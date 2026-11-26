# E46 MK60 Tool

Arm the MK60's (gated) CCP slave over K-line, then probe/dump memory over CAN — driven entirely
from a laptop through a **Gauge.S** board acting as a dumb USB↔CAN/K-line bridge.

```
bench/
  protocol.md     serial bridge contract (PC <-> Gauge.S)
  firmware/       PlatformIO — the dumb bridge (Arduino/ESP32)
  core/           Rust lib (e46-mk60-tool-core) — all the logic (bridge, CCP, KWP, probe)
  host/           Rust CLI (e46-mk60-tool) — arg parsing + command orchestration, on top of core
  gui/            egui desktop UI (real: thin front end over core, worker-threaded; own lock)
                  + firmware provisioning (flash/backup/restore) via the espflash library
```

The design keeps every bus-specific driver on the ESP32, so the PC side is pure serial and
**OS-agnostic** (macOS/Windows/Linux) — the only platform surface is `bridge.rs::open`.

## Building

A fresh clone builds the **CLI and GUI with just a Rust toolchain** — no firmware toolchain
required. Firmware and the Windows cross-builds are optional extras.

- **CLI** (`core` + `host`, a Cargo workspace rooted at `bench/`):
  ```bash
  cd bench
  cargo build --release        # e46-mk60-tool-core + the e46-mk60-tool CLI
  cargo test                   # optional: seed/key, framing, parse vectors
  ```
- **GUI** (a separate crate - excluded from the workspace, with its own `Cargo.lock`):
  ```bash
  cd bench/gui
  cargo build --release        # e46-mk60-tool-gui
  ./make-app.sh                # optional: the macOS "E46 MK60 Tool.app" (+ Windows .exes if cargo-xwin is set up)
  ```
  The GUI flashes the Gauge.S board, so `build.rs` embeds the dongle firmware: it prefers a live
  PlatformIO build (`firmware/.pio/build`) and otherwise falls back to the committed snapshot in
  `gui/firmware/`, so **the GUI builds without PlatformIO installed**.
- **Firmware** (only to build or flash the dongle): needs PlatformIO - see **Firmware (Gauge.S)** below.
- **Windows binaries**: cross-compiled from macOS with `cargo-xwin`; one-time setup in
  [`BUILD_WINDOWS.md`](BUILD_WINDOWS.md). `make-app.sh` drives it and skips loudly if it is absent.

## Releases

Prebuilt apps ship via **GitHub Releases**, not committed to the repo - they are large,
platform-specific build *outputs*. (The one committed binary is the firmware `.bin` under
`gui/firmware/`, because `build.rs` needs it as a build *input*.)

`make-app.sh` produces the full set from one source, in lockstep:
- `E46 MK60 Tool x64.exe`, `E46 MK60 Tool arm64.exe` - static CRT, standalone (no VC++ redist)
- `E46 MK60 Tool.zip` - the macOS `.app`, `ditto`-archived so it survives transfer

To cut a release: `cd bench/gui && ./make-app.sh`, then attach those artifacts to a tagged
GitHub release. The macOS app is **unsigned (ad-hoc)**, so note in the release that first launch
needs the quarantine cleared once: `xattr -dr com.apple.quarantine "E46 MK60 Tool.app"`. The
Windows `.exe`s need nothing.

## Firmware (Gauge.S)
Two board families, picked at build time (no runtime detection):
- **V4** — classic ESP32. PC link = external **FTDI on UART0**; flash with the board held in
  download mode (manual BOOT/RST), per the board's own flashing doc.
- **V5 / V5.1** — ESP32-S3. PC link = **native USB-CDC**; the single **USB-C** port does both
  flashing and host comms, and esptool resets into download mode on its own (no button dance).

This bench uses a **V5.1 (S3)** — flash and talk to it over USB-C:

```bash
cd bench/firmware
pio run -e gaugesV5                                       # build (S3)
pio run -e gaugesV5 -t upload --upload-port /dev/cu.usbmodem*   # flash over USB-C
# pio run -e gaugesV4 -t upload                           # classic-ESP32 board, via FTDI
```
Pins come from `src/config.h` (V4: CAN 4/5, K-line 16/17; V5: CAN 17/18, K-line 15/16). PC link =
`Serial` @ 921600 (UART0→FTDI on V4, native USB-CDC on V5); K-line = `Serial2`; CAN = TWAI @ 500k.

## Host (Rust) CLI
Build it per **Building** above; the workspace binary is `e46-mk60-tool`
(`cargo run -p e46-mk60-tool -- ping`, or run `target/release/e46-mk60-tool` directly).

Usage (`--port` auto-detects by USB VID if omitted; `--raw` hex-dumps every frame in/out):
```bash
e46-mk60-tool ping                       # dongle alive?
e46-mk60-tool sniff --ms 3000            # list CAN ids the ECU broadcasts
e46-mk60-tool trace --ms 5000            # passively hex-dump all CAN + K-line traffic
e46-mk60-tool wake                       # send the cluster/DME keepalive so the MK60 starts broadcasting
e46-mk60-tool can-monitor --wake --ms 5000  # wake, then decode its telemetry (wheel speeds, steering, DSC/speed)
e46-mk60-tool arm                        # K-line: 27 07 -> 27 08 key -> 31 31 01
e46-mk60-tool disarm                     # unlock then 31 31 00 — leave the ECU clean
e46-mk60-tool kwp "21 06"                # send any raw KWP service; tool frames + checksums it (here: read LID 6)
e46-mk60-tool kwp "1A 94"                # e.g. the SW-number ident block; --can routes over CAN ISO-TP (E5)
e46-mk60-tool probe                      # sweep CRO ids for a CCP reply (after arm); default 0x700-0x7ff
e46-mk60-tool probe --lo 0x600 --hi 0x7ff  # widen the sweep if the CRO sits outside the usual block
e46-mk60-tool dump --cro 0x797 --dto 0x798 --addr 0x0 --len 0x8000 --out dump.bin

e46-mk60-tool --raw arm                  # same, but print the K-line bytes on the wire
```

Typical bench flow: wire Gauge.S (K-line→MK60 pin 2, CAN→11/15, common 12V/GND) → `ping` → `arm`
→ `probe` → `dump`.

## MK60 CAN handshake (keepalive)
The MK60 stays **silent on CAN until it hears the rest of the car**. At boot it emits a few remote
(RTR) frames on **0x610** (ICL1, the cluster message); if nothing answers it never starts broadcasting.
`wake` answers with a short burst of spoofed cluster/DME frames — ICL1 `0x610`, IKE `0x613`/`0x615`,
DME `0x316`/`0x329`, 5 ms apart (the empirical set from peterakimball/mk60_can_tool). ICL1 carries a VIN
in bytes 0-4, but the module does **not** validate it (both zero and a real donor VIN woke the bench
module), so we send zeros. Send the burst **once per MK60 power cycle** and the module then broadcasts
standalone: wheel speeds `0x1F0`, steering `0x1F5`, ASC1/vehicle-speed `0x153`. `can-monitor` decodes
those to confirm the module is *broadcasting* — on the bench the values are meaningless (no sensors
connected), so it validates presence, not magnitudes. This gates
CAN **broadcast** only; the K-line `arm` and gated CCP `dump` are **confirmed independent of it** (the
`31 31 01` arm, not the wake, is what opens the CCP gate — see the disassembly note in the repo root), so
you can `arm`/`probe`/`dump` without ever sending the keepalive.

## Open items (need the ECU on the bench)
- K-line byte format: **confirmed 9600 8E1** (even parity) + **XOR** checksum on the E46 MK60 bench
  (2026-11-11). Both are now fixed (8E1 in firmware, XOR in `kwp.rs`); 8N1/additive removed.
- K-line echo/timing in `arm` may need tuning against real hardware.
- CRO/DTO ids and whether UPLOAD is bounded — that's what `probe` and a first small `dump` establish.
- Keepalive: does the woken MK60 stay up indefinitely, or time out and need a slow periodic re-send?
- Telemetry decode values: only confirmable on a car — the bench MK60 has no wheel/steering/pressure
  sensors, so `can-monitor` and `kwp "21 06"` return fault/default readings (K-line read still
  validated the *protocol*; the byte-offset decode is per config, unverified against real sensors).
- CCP-activity latch (`0x401164`, M3 image) — **traced, no mid-dump risk.** It's serviced by
  `can_dme_iface_update` inside `task3_10ms_body` (every 10 ms); seeded at 200 (`FUN_0x7d8ba`), it
  decrements 1/tick = **2.0 s** to zero. But every received CRO pins it to `0xFFFF`, the one value the
  service routine *skips* — so during CCP traffic it's frozen and the `F==0` poke
  (`module_method_dispatch(1,3,0)`) is unreachable. It's effectively a ~2 s post-boot settle one-shot.
  Only exposure: a **>2 s gap with zero CRO traffic** (e.g. arm then idle before the first CRO) — trivially
  avoided. State values decoded: the gate flag is RAM byte `M=0x40954D` (1=armed); query returns 4=armed,
  5=disarmed, 3=locked; the arm (`31 31 01`) sets `M=1`, the expiry sets `M=0` (both persisted to NVM via
  `nvm_queue_job`). So an expiry genuinely **disarms** (rearm needed). `M` is **restored from NVM at boot**
  (proven: boot init `FUN_0b0038` reads NVM offset `0x33` into `0x40954D`=`M`), so the module boots with its
  last-persisted arm state and the boot-settle latch clears a stale armed state unless a tester sends a CRO
  within ~2 s — a deliberate security mechanism. Practical: our `31 31 01` arm is **persistent** (NVM-written),
  so the bench module boots armed for the first ~2 s on later power cycles, and a CRO in that window can open
  the gate with no fresh arm if `M=1` was persisted.

## Post-validation cleanup (remove once the bench answers settle)
These are temporary **toggles that exist only to discover the right value**. Once a choice is proven on
the bench, hardcode the winner and delete the switch so nobody is offered an option that doesn't work:
- ~~**K-line checksum**~~ — DONE (2026-11-11): XOR confirmed, hardcoded in `core/src/kwp.rs`, `Checksum`
  enum + CLI `--checksum` flag + GUI combo removed.
- ~~**K-line byte format**~~ — DONE: 8E1 confirmed, fixed via `KLINE_SERIAL_FORMAT` in firmware
  `config.h`; the `fmt` option and the protocol's format byte are gone.
- ~~**Keepalive VIN**~~ — DONE (2026-11-11): both VINs proven to work (VIN not validated), so zero is
  hardcoded in `core/src/mk60can.rs`; the `Vin` enum + CLI `--vin` flag + GUI "VIN" combo removed.

All discovery toggles are now resolved and removed.
