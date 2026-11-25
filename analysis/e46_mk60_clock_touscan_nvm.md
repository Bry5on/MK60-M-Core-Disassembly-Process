# E46 MK60 — system clock, TouCAN controller, and the EEPROM SPI driver

Hand-written note (2026-11-23). Corrects and extends `mmio_map.md` (the CAN block and the clock),
`e46_m3_mk60_can_map.md` (bit timing, mailbox CODE values, the bus-off counter) and
`e46_m3_mk60_kline_access_and_flash.md` (the NVM job layer sits above the SPI driver documented
here). Addresses are from the 818.3 non-ZCP image unless noted; the four E46 images (811.3, 817.3,
818.3, 820.3) share the timing bytes and the driver. This describes what the images contain; it is
not a procedure for writing a module or a car.

Confidence: **[C]** read in the image/listing, **[I]** inferred.

## 1. The CAN controller is a Motorola TouCAN, not generic FlexCAN

`mmio_map.md` calls the CAN block "FlexCAN-style". The driver accesses match a **Motorola TouCAN**
register-for-register (as in the MC68376 / MPC555):

| Offset | TouCAN register | Evidence (818.3) |
|---|---|---|
| +0x00 | MCR | `can_hw_soft_reset` 0x3BE78 sets 0x0200 (SOFTRST), clears 0x0080 (SUPV); `can_hw_init_mode` 0x3BB50 sets 0x1000 (HALT) |
| +0x07 | CANCTRL1 | bit 7 SAMP, bits 2:0 PROPSEG; bits 6:3 preserved (mask 0x78); soft-reset writes 0x10 (LBUF) |
| +0x08 | PRESDIV | `cfg[0]-1` |
| +0x09 | CANCTRL2 | RJW[7:6], PSEG1[5:3], PSEG2[2:0] |
| +0x0A | TIMER | read after each mailbox access (0x3BBC0, 0x3BD00) — the TouCAN way to unlock a locked buffer |
| +0x20 | ESTAT | `can_hw_status_decode` 0x3BAE0: FCS[5:4] (01 = error-passive, 1x = bus-off), error bits 8..15, IDLE 7, TX/RX 6 |
| +0x22 | IMASK | mailbox 0..15 interrupt mask (`can_hw_mailbox_program` 0x3BBC4) |
| +0x27 | TXECTR | 0x3BB3E: bus-off and TXECTR==0 set status bit 11 |
| +0x80 | 16 × 16-byte message buffers | word 0 = timestamp \| CODE[7:4] \| DLC[3:0]; ID_HIGH/LOW +2/+4 (IDE bit 3, SRR/RTR bit 4); data +6..+0xD; +0xE..+0xF reserved |

Consequences:

- **Correction to `mmio_map.md`:** `0xFFFC80..0xFFFCA7` is **not** a "TX mailbox"; it is the TouCAN
  register block, ending at RX/TX-ECTR (+0x26/+0x27) — exactly the live-readable edge
  `[0xFFFC86, 0xFFFCA7]`. The 16 mailboxes are at `0xFFFD00..0xFFFDFF`; `+0xE..+0xF` not answering
  matches the reserved word. [C]
- **Correction to `e46_m3_mk60_can_map.md` mailbox CODE values** (marked [I] there): `0x4` = RX EMPTY
  (armed), `0x8` = TX not ready, **`0xA` = TX remote-response**, **`0xC` = TX "transmit once"**. The
  send kick at 0x3BBAC sets CODE 0xC then reads TIMER; `can_hw_tx_write` 0x3B878 drops to 0x8 during
  the data update and restores 0xA. [C]
- Mailboxes 16..31 (the 32-slot variant, `0x15412`) would place their mask bits in `+0x0E` — a
  generic-driver extension TouCAN itself lacks. [C for the code, I for the hardware]

## 2. CAN bit timing: both buses 500 kbit/s, and the clock is 28 MHz

`can_controller_init` 0x158B4 builds the 4-byte config from **header bytes 4..9** (not 0x0C..0x0D as
`e46_m3_mk60_can_map.md` guesses):

```
SAMP(3 samples) = (hdr[4]==3)   PRESDIV+1 = hdr[5]*2   PROPSEG = hdr[6]
PSEG1 = hdr[7]   PSEG2 = hdr[8]   RJW = hdr[9] & 7
```

| Bus | Header | Prescaler | Tq/bit (1+PROP+PS1+PS2) | Sample point |
|---|---|---|---|---|
| Vehicle 0xFFFC80, header 0x3DCB2 | `01 02 03 05 05 01` | 4 | 1+3+5+5 = 14 | 64.3 % |
| Sensor 0x7FFC80, header 0x3F7CD | `01 02 05 04 04 01` | 4 | 1+5+4+4 = 14 | 71.4 % |

All four E46 images carry identical timing bytes. A bit is 4 × 14 = **56 system clocks**; at 500 kbit/s
(E46 PT-CAN) that gives **f_sys = 28 MHz**. [C]

This is independently consistent with the SCI baud table, which begins one word earlier than
`mfg_test_cycle.md` records: at 0x3DADC id 0 = **4375**, then 91, 84, 15, 8, 182, 46, 28, 23, 14. With
the Motorola SCI formula `f_sys / (32 · SCBR)` at 28 MHz, id 0 = 200.0 baud exactly, id 7 = 31250
exactly, id 9 = 62500 exactly, and the knock-phase default (id 5, divisor 182) = **≈4808 baud**. So
the factory K-line default is **≈4800 Bd**, settling the "4800 or 9600" question in `mfg_test_cycle.md`
/ `nonm_mk60_image.md` for E46. The MFU**33** suffix (33 MHz max) fits 28 MHz. [C for the tables, I for the reading]

## 3. Sensor bus-off counter

Previously open in `e46_m3_mk60_can_map.md`. The counter is RAM **0x400E8C**:

- `0x34B68` increments it (saturating at 15) when `0x40113C & 5 == 0`, setting 0x400E8A bit 3.
- `0x34640`: if 0x400E8D bit 1 is set (status from `0x15594`, which reads ESTAT) and the counter ≥ 15,
  fault **0xA0200** is reported (`0x32AD8`).
- `0x34652`: decrements it while the low byte of 0x4005AC is zero (slow decay). [C]

## 4. The serial-EEPROM (95040) SPI driver

`e46_m3_mk60_kline_access_and_flash.md` documents the NVM job layer (request, queue) but not the SPI
driver under it. It is (818.3):

| Address | Function | SPI sequence |
|---|---|---|
| 0x16334 | `eeprom_write(addr9, src, len, mode)` | `trap #3`, CS setup `0x3C468(7)`, **WREN 0x06** (const at 0x3DA5C), then `[0x02 / 0x0A (A8), addr_lo, ~data…]` via `qspi_xfer(2, len+2, …)`, `0x3C468(4)`, `trap #2`. mode 3 = fill all bytes with `src[0]`, else copy |
| 0x163A6 | `eeprom_read(addr9, dst, len, mode)` | `[0x03 / 0x0B (A8), addr_lo]` + len bytes; return inverted (`dst[i] = ~rx[i]`) |
| 0x1640E | `eeprom_write_status(v)` | WREN, then **WRSR** `[0x01, v]` |
| 0x16450 | `eeprom_read_status()` | **RDSR** `[0x05, 0xFF]`, returns the status byte |
| 0x3C3F4 | `qspi_xfer(ch, len, tx, rx)` | QSPI transfer; the EEPROM is on channel **2** (the ASIC is on 5) |
| 0x161B4 | sole caller of `eeprom_write` | NVM job worker: mode 1 read, 2/3 write, **4 read-modify-write with OR** |
| ~0x16770 | boot init | `RDSR & 0x0C` (BP1/BP0); if a block-protect bit is set, `WRSR 0` — the firmware **clears the EEPROM write-protect at every start**; then paints RAM 0x400000..0x4002FF with 0xA5A5A5A5 |

**Every data byte in the 95040 is stored bit-inverted** (logical = `~raw`): a raw dump (CH341A etc.)
must be XORed with 0xFF before it matches coding/fault values; an erased EEPROM (raw 0xFF) reads as
zeros. The driver does not page-split (the 95040 has 16-byte pages — the job layer or caller handles
that) and does not poll WIP after a write. [C for the driver; I for the page/WIP notes.] See
`e46_mk60_eeprom_layout.md` for the on-chip layout this driver serves.

The same driver sits at matching offsets in the other images (opcode-select byte patterns):
817.3 write ≈0x16464 / read ≈0x164D6 (WREN const 0x3DF84); 820.3 ≈0x162B4 / ≈0x16326 (0x3DA9C); 811.3
≈0x13E34 / ≈0x13EAC (0x37FBC via `lrw`, different register allocation, same logic incl. inversion). [C]

## Method

CAN and SCI timing (§2) are verified at the data level on the raw images: the vehicle header
`01 02 03 05 05 01` at 0x3DCB2+4 and the baud table (4375, 91, 84, 15, 8, 182, 46, 28, 23, 14) at
0x3DADC read directly, and the 28 MHz figure follows from 56 system clocks per 500 kbit/s bit. The
TouCAN register mapping (§1), the mailbox CODE values, the bus-off counter (§3) and the SPI driver
(§4) were read from the 818.3 listing with an independent linear M·CORE disassembler (binutils
`mcore-opc.h` opcode map) and cross-checked against the other three E46 images; addresses marked [C]
are from those reads, [I] are inferences noted inline. No firmware image is included. The broader
firmware understanding builds on cmg5461's `mk60e5-firmware` — see `CREDITS.md`.
