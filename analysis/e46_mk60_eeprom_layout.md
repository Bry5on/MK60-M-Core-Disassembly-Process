# E46 MK60 — 95040 config EEPROM layout

Hand-written note (2026-11-23). Companion to `e46_m3_mk60_kline_access_and_flash.md` (NVM job layer
and the factory `0xA5 'X'` single-byte EEPROM write), `kwp_dispatch.md` / `kwp_sid22.md` (the KWP
`0x1A`/`0x3B`/`0x23`/`0x3D` paths that reach the EEPROM) and `program_request_record.md` (which notes
the E46 block directory lives in flash). This maps the **serial 95040 config EEPROM** of an E46 MK60
(VIN, coding, counters) — the separate SPI part, not the on-chip program flash.

Unlike most notes here, the offsets below were read from **four live 512-byte 95040 dumps of a bench
820.3 non-M unit** (one baseline plus three VIN-write tests) and cross-checked against the 820.3
image. It is a description of an EEPROM's contents; it is not a procedure for writing a module or a
car.

Confidence: **[C]** confirmed against the dumps and/or the 820.3 listing, **[I]** inferred, **[?]** open.

## 0. The store is bit-inverted

Every data byte in the 95040 is stored **bitwise inverted**: logical value = `raw XOR 0xFF`. A raw
read (CH341A etc.) must be XORed with `0xFF` before any field below reads correctly; an erased cell
(raw `0xFF`) is logical `0x00`. This matches the NVM driver in `e46_m3_mk60_kline_access_and_flash.md`
(reads return `~rx`), and is confirmed on hardware: the VIN at 0x150 only reads as ASCII after
inversion. **All offsets/values below are logical (post-inversion).** [C]

The chip is 512 bytes (0x000–0x1FF); on the baseline unit it is populated to ~0x165, the rest `0x00`.

## 1. Map

| Offset | Len | Field | Conf |
|---|---|---|---|
| 0x000–0x001 | 2 | header `0b ee` (unknown) | ? |
| 0x002–0x004 | 3 | `"6DC"` ASCII — supplier / HW marker | C |
| **0x005–0x007** | 3 | **manufacturing date**, binary day/month/year (`15 05 02` = 2002-05-21) | C |
| 0x008–0x00E | 7 | header remainder (unknown) | ? |
| 0x020–0x031 | ~18 | counter / flag block | I |
| **0x044–0x048** | 5 | **write-log index ring** (`01 02 03 04 00`); rotates on each VIN write | C |
| **0x049–0x04D** | 5 | **write-log status bytes** (5 slots) — *not* a VIN checksum (see §3) | C |
| 0x04E–0x057 | 10 | two parallel 5-slot columns (`28`×5, `01`×5) | I |
| 0x058–0x075 | 30 | **5 × 6-byte records** `ff ff 00 0d 00 04`; last byte per record is a flag | C/I |
| 0x0BC–0x0C0 | 5 | packed field `30 26 38 58 4a` (unknown) | ? |
| 0x0CD–0x0FB | ~47 | **unpacked coding copy** (one 16-bit-LE field per parameter) | I |
| **0x120–0x12D** | 14 | **packed DSC coding block** (see §2) | C |
| 0x131–0x134 | 4 | **part number** (BCD, e.g. `06 75 90 45`) — overlaps the ID record | C |
| 0x135 / 0x138 / 0x13A | — | coding index / variant index / diagnosis index | C |
| **0x150–0x15D** | 14 | **short VIN** (14 ASCII chars, inverted) | C |
| 0x15E–0x160 | 3 | fixed marker `"=Px"` (`3d 50 78`), identical across VINs | C |
| 0x162–0x163 | 2 | `e9 f5` — candidate coding-record checksum (not a simple sum/xor) | ? |

## 2. The packed DSC coding block at 0x120

The packed coding record is at **EEPROM 0x120** (NCS coding-offset N → EEPROM 0x120+N). Confirmed by
decoding: base 0x120 yields a self-consistent configuration; other bases yield nonsense. The field
addresses/masks/keyword names are those of the DSC coding (the same `BAUART_MK60` etc. used elsewhere
in these notes); a community NCS-Dummy export of the module's coding definition supplies them. Only
the facts needed are reproduced here — offset, mask, keyword — not the full coding tables.

| EEPROM | +off | mask | keyword |
|---|---|---|---|
| 0x120 | +00 | FF | CHECKSUM (see §3) |
| 0x121 | +01 | E0 / 1F | MOTOR_ART_DSC_2 / BAUART_MK60 |
| 0x122 | +02 | 07 / 38 / C0 | DRUCKMODELL_HINTERACHSE / _VORDERACHSE / GETRIEBE |
| 0x123 | +03 | 01…80 | LOGIK_HA_SPERRE, EBV_LAMPE, BREMSLICHTSCHALTER, MOTOR_ART_ASC, PRE_VLABS, SPERR-/TRAKTIONDIFFERENZREGELUNG, UNTERSTEUERUNGSALGORYTHM |
| 0x124–0x128 | +04..08 | FF | ÜBER-/UNTERSTEUERN_MUE_0/1, UNTERSTEUERSCHWELLE |
| 0x129 | +09 | 0F / F0 | GEARMOMENTREGLER_MAX/MIN_HA |
| 0x12A | +0A | 0F / 10 / 20 | DELTA_PSI_BETA_LIMITKORR, DROSSEL, REIFENDRUCKWARNSYSTEM |
| 0x12B | +0B | FF | VOLUMENDURCHFLUSSPARA |
| 0x12C | +0C | 01/02/0C/F0 | DYN/MAX_BREMS_UNTERSTUETZUNG, HBA_SCHWELLANHEBUNG, HBA_VARIANTEN |
| 0x12D | +0D | 07 | LENKWINKELKENNLINIE |

A second, **unpacked** copy of the same values (one 16-bit-LE field per parameter) sits at 0x0CD. [I]

## 3. Two checksums, and what is *not* one

**The write-log at 0x044–0x04D is not a VIN checksum.** Across the test dumps: two VINs differing in a
single character carry *completely* different 0x049 bytes, while writing a new VIN on the same unit
only *rotates* the 0x044–0x04D arrays (ring-buffer behaviour) and changes a flag. So it is a
write/session log, and the fixed `"=Px"` marker after the VIN is a format marker, not a checksum.
**A VIN write therefore needs no computed check value** — the 14 inverted ASCII bytes at 0x150 are
enough. The firmware also treats the VIN as plain bytes (`0x1A 0x90` read, `0x3B 0x90` write,
`0x23`/`0x3D` raw), and the boot path only does RDSR + block-protect-clear, with no EEPROM content
checksum. [C]

**The packed-coding CHECKSUM (offset +00) is:**

```
CHECKSUM = ( XOR of coding bytes +01 .. +0D ) + 1   (mod 256)
```

Verified two ways: it reproduces the stored byte on the bench unit, and the firmware routine
`coding_image_checksum` (818.3 `0x3B2B0`) XORs the coding bytes in a loop and ends with `addi r4,1`
(the `+1`). The firmware additionally keeps a larger image seal at RAM `0x400FF2` (also XOR+1 over a
wider region); that one is internal and separate from this coding byte. [C]

So a coding writer sets bytes +01..+0D, computes `XOR(them)+1`, stores it at +00, and writes the
whole block inverted into the 95040. [C]

## 4. Tools

- `tools/decode_coding.py <eeprom.bin> <Cxx-ModuleFunctions.txt>` — inverts the dump, decodes the
  packed block at 0x120 against a local NCS-Dummy coding export, and verifies the CHECKSUM. The export
  file is read locally and nothing from it is embedded.

## Method

Read from four 512-byte 95040 dumps of a bench 820.3 non-M unit (one baseline + three VIN-write
tests) and cross-checked against the 820.3 image with an independent linear M·CORE disassembler
(binutils `mcore-opc.h` opcode map) and a Ghidra recovery of the image. The
inversion, the VIN location, the write-log behaviour and the coding-CHECKSUM formula are from byte
diffs across the dumps; the CHECKSUM `+1` and the boot path are from the 820.3 / 818.3 listings
(`coding_image_checksum` 0x3B2B0, EEPROM boot-init, `eeprom_read`/`_write` 0x16326/0x162B4). No EEPROM
image is included (per the repo's no-firmware policy). The coding keyword/offset/mask facts come from a
community NCS-Dummy export; BMW SGBD coding tables are not reproduced. The broader firmware
understanding (NVM job layer, MISR self-test) builds on cmg5461's `mk60e5-firmware` — see `CREDITS.md`.
