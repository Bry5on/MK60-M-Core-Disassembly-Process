# Bench dongle serial protocol — v0.1 (draft)

The contract between the PC app and the Gauge.S firmware. The dongle is **dumb**: it moves raw CAN
frames and raw K-line bytes. All intelligence (BMW-FAST framing, seed/key, CCP CRO/DTO + counters, the
dump loop, the probe sweep) lives on the PC. This keeps the PC side pure-serial and OS-agnostic.

Transport: USB-serial (V5 native USB-CDC, or V4 via FTDI on UART0). 8N1, 921600 baud (configurable).

## Packet framing
Both directions use the same frame:

```
 off  size  field
  0    1    SYNC   = 0x55
  1    1    CMD
  2    2    LEN    payload length, little-endian
  4   LEN   PAYLOAD
 4+L   2    CRC16  CRC-16/CCITT-FALSE over CMD..PAYLOAD (poly 0x1021, init 0xFFFF), little-endian
```

Resync rule: on CRC mismatch or desync, scan forward for the next `0x55` and re-attempt. USB-CDC is
reliable; the sync+CRC is cheap insurance against a dropped byte on the V4 UART path.

All multi-byte integers little-endian unless noted. CAN ids are 11-bit, sent as u16.

## Host → dongle commands
| CMD  | Name          | Payload                              | Notes |
|------|---------------|--------------------------------------|-------|
| 0x01 | PING          | —                                    | → INFO |
| 0x10 | CAN_OPEN      | bitrate u8 (0=500k), mode u8 (0=normal,1=listen-only) | start TWAI |
| 0x11 | CAN_CLOSE     | —                                    | |
| 0x12 | CAN_FILTER    | id u16, mask u16                     | mask 0 = accept-all (probe/sniff) |
| 0x13 | CAN_TX        | id u16, dlc u8, data[dlc]            | one raw frame, exactly as given |
| 0x20 | KLINE_OPEN    | baud u32, fmt u8 (optional)          | opens the K-line UART; fmt 0=8E1 (default, omitted byte), 1=8N1, 2=8O1 |
| 0x21 | KLINE_CLOSE   | —                                    | |
| 0x22 | KLINE_TX      | bytes[]                              | raw bytes on K-line; PC builds BMW-FAST + checksum |
| 0x23 | KLINE_CFG     | p1 u16, p4 u16, echo_suppress u8     | inter-byte timing (ms); echo handling |
| 0x30 | SPI_OPEN      | half_us u8 (optional, 0=default)     | init bit-bang SPI master (V5 pins); mode 0, MSB-first, CS idle-high. FW >= 0x000C |
| 0x31 | SPI_XFER      | bytes[]                              | one CS-framed transaction: clock bytes out on MOSI, sample MISO. → SPI_RX. FW >= 0x000C |
| 0x40 | RELAY_SET     | state u8 (1=on, 0=off)               | drive the EE (GPIO3) MOSFET gate: 1 = sink relay IN -> ignition energised. FW >= 0x0012 |
| 0x41 | ADC_READ      | —                                    | sample the A5 (GPIO1) ignition-feedback divider → ADC. FW >= 0x0012 |

## Dongle → host events
| CMD  | Name       | Payload                                   | Notes |
|------|------------|-------------------------------------------|-------|
| 0x81 | INFO       | fw_ver u16, board u8 (4/5), caps u8       | reply to PING / sent on boot |
| 0x90 | CAN_RX     | id u16, dlc u8, data[dlc], ts_us u32      | one received frame |
| 0x91 | CAN_STATE  | state u8 (0=running/recovered, 1=bus-off/recovering) | TWAI bus-off auto-recovery transition. FW >= 0x000D |
| 0xA0 | KLINE_RX   | bytes[]                                   | raw bytes seen (incl. self-echo unless suppressed) |
| 0xB0 | SPI_RX     | bytes[]                                   | MISO bytes from one SPI_XFER (same count as sent). FW >= 0x000C |
| 0xC0 | ADC        | mv u16                                    | calibrated millivolts at A5; Vin ~= mv * 5.545 via the 10k/2.2k divider. FW >= 0x0012 |
| 0xE0 | ERROR      | code u8, detail[]                         | e.g. TWAI bus-off, UART overrun, SPI-not-open (0x30) / no-SPI-on-board (0x31) / no-relay-on-board (0x40) |

## Raw SPI mode (25LC040 config EEPROM)
A third transport, added for reading/writing the MK60's Microchip **25LC040** SPI EEPROM (512 bytes, 3.3V)
on the bench. The dongle stays dumb: `SPI_OPEN` sets up a **bit-bang SPI master** (mode 0, MSB-first) on the
V5 board pins, and each `SPI_XFER` is **one full CS-framed transaction** — CS is driven LOW, every payload
byte is clocked out on MOSI while MISO is sampled per bit, then CS goes HIGH — and the dongle returns the
same number of received bytes in `SPI_RX`. The host builds every 25LC040 command (WREN/READ/WRITE/RDSR...),
handles the **A8-in-opcode** addressing (bit 3 of the opcode = address bit 8 on the plain 040; the
`25LC040A` variant uses a 2-byte address instead), the 16-byte write-page boundary, and WIP polling. See
`core/src/spi.rs`.

V5 pin map (config.h, `PCB_VER > 40`): **CS = GPIO 8**, **MISO = GPIO 11**, **MOSI = GPIO 41**,
**SCK = GPIO 42**; chip 3V3 + GND from the I2C/GPS header. V4 (`PCB_VER <= 40`) has no SPI header and answers
`SPI_OPEN`/`SPI_XFER` with `ERROR 0x31`.

## Relay / ignition-feedback rig (EE + A5)
A fourth bench accessory, for timing the mfg/diag handshake against the module's power-up. The Gauge.S drives
one **relay** that switches the MK60's KL15 (ignition) and reads one **12V feedback** line so the host knows
exactly when the module is powered.

- **RELAY_SET** toggles the **EE** pin (ANALOG2 pin4 = GPIO 3, the ENC_E N-MOSFET gate). HIGH turns the MOSFET
  on, sinking the relay board's `IN` to ground → relay closes → ignition energised. The pin is driven LOW at
  boot; because GPIO3 is an ESP32-S3 strapping pin, expect a brief power-on blip before `relay_setup()` pulls it
  down (harmless — the relay board self-clamps `IN` high through its back-to-back opto LEDs, so a floating gate
  reads as OFF).
- **ADC_READ** samples the **A5** pin (ANALOG2 pin1 = GPIO 1, ADC1_CH0) through the board's 10k/2.2k divider
  (11dB attenuation), returning calibrated millivolts in `ADC`. Scale to the real line with `Vin ~= mv * 5.545`.
  With KL15 at ~12.5V expect ~2250 mV at the pin.

V5 only (`#ifdef RELAY_EE`); V4 answers `RELAY_SET`/`ADC_READ` with `ERROR 0x40`. Host ops: `relay-on`,
`relay-off`, `adc-read`, `relay-timing` (energise, then poll A5 until it crosses a threshold and report the
delay). See `core/src/ops.rs`.

## Half-duplex note (K-line)
K-line is single-wire: the dongle receives its own transmitted bytes (echo). Default = pass the echo
through in `KLINE_RX`; the PC discards its own echoed bytes (and uses them for collision detection).
`KLINE_CFG echo_suppress=1` makes the dongle drop them instead.

## Typical PC-side sequence
1. open serial (auto-detect by VID:PID) → PING → INFO
2. `KLINE_OPEN 9600,8N1` → arm: KLINE_TX the BMW-FAST frames for `27 07` / `27 08 key` / `31 31 01`
3. `CAN_OPEN 500k,normal` + `CAN_FILTER accept-all` → probe: CAN_TX CONNECT per candidate id, watch CAN_RX
4. once CRO/DTO found → CAN_TX SET_MTA, loop CAN_TX UPLOAD / collect CAN_RX → reassemble dump

## Open points (v0.1)
- Confirm MK60 K-line byte format (8N1 vs 8E1) — `format` field covers both.
- V4 baud ceiling over FTDI (UART0) — 921600 fine; 1.5M if stable.
- Optional: a `CAN_TX_WAIT` that transmits and returns the first matching-id reply inline, to cut
  round-trip latency on the UPLOAD loop (vs. async CAN_RX). Decide after measuring.
