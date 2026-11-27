# E46 MK60 818.3 (M3) — EFFFxx elevated idle probe (read-only)

Hand-written note (2026-11-27). Companion to `e46_m3_mk60_flash_programmability.md`,
`e46_m3_mk60_kline_access_and_flash.md`, and `mmio_map.md`. This records a **live,
read-only** elevated (10A1) halfword sweep of `0x00EFFF00`..`0x00EFFF5E` on the
**E46 MK60 818.3 (M3)** module. It is **not** a program/erase procedure; nothing was
written to the controller and no flash was programmed.

Confidence: **[C]** live bench read on this module, **[I]** interpretation of those
reads against the earlier static map, **[open]** not established.

## Method

- Tool: E46 MK60 Tool (`local/efffxx-probe`), elevated MMIO path.
- Session: mfg handshake → level 3, keep-session-alive; one knock per run.
- Stub: `mfcr r1,ss4` (`0x10A1`) then a read-only `ld` (halfword) per address.
- Range: `0x00EFFF00`..=`0x00EFFF5E`, step `0x2`, width halfword.
- Follow-ups: extent-map re-sweep `EFFF44`..`EFFF5E`; repeated `elevated mmio-read`
  of `EFFF40` (×4) and `EFFF42` (×3).

Every address in the range answered **MAPPED**. No FAULT, no hang. A FAULT after
elevate would mean wrong/protected map, not privilege; none occurred, so the
supervisor gate opened the block for read.

## Full idle sweep (session 1)

| Addr | Value | Notes |
|---|---|---|
| EFFF00 | `0x0618` | |
| EFFF02 | `0x1100` | |
| EFFF04 | `0x00C8` | |
| EFFF06 | `0xA8A6` | high byte `0xA8` = gating/arm, idle; low = EFFF07 |
| EFFF08 | `0x0008` | |
| EFFF0A | `0x0000` | |
| EFFF0C | `0x6D2C` | |
| EFFF0E | `0x0440` | |
| EFFF10 | `0x3F02` | BKSEL bank 0 |
| EFFF12 | `0x3F02` | BKSEL bank 1 |
| EFFF14 | `0x3F02` | BKSEL bank 2 |
| EFFF16 | `0x3F02` | BKSEL bank 3 |
| EFFF18 | `0x2041` | command register at idle |
| EFFF1A | `0xFB00` | status, idle/ready |
| EFFF1C | `0x0000` | |
| EFFF1E | `0x0008` | |
| EFFF20 | `0x0407` | |
| EFFF22 | `0x4281` | |
| EFFF24 | `0x0087` | |
| EFFF26 | `0x0C67` | |
| EFFF28 | `0xBF7F` | |
| EFFF2A | `0xF299` | |
| EFFF2C | `0xFF00` | |
| EFFF2E | `0x0000` | |
| EFFF30..36 | `0x0000` | |
| EFFF38 | `0xFF00` | |
| EFFF3A..3E | `0x0000` | |
| EFFF40 | `0x0631` | data port low |
| EFFF42 | `0x2C4D` | data port high |
| EFFF44 | `0x0000` | extent map |
| EFFF46 | `0x0005` | only non-zero extent entry this session |
| EFFF48..5E | `0x0000` | |

## Confirmed predictions **[C]**

- **Four-bank BKSEL fingerprint is live.** `EFFF10/12/14/16 = 0x3F02 ×4` —
  SGFM-class four-banked register signature, confirmed on the running module (not
  only inferred from the image).
- **Status idle.** `EFFF1A = 0xFB00`.
- **Gating byte idle.** `EFFF06` high byte `0xA8` (low byte `0xA6` is adjacent
  EFFF07). Gate sitting idle; nothing armed.
- **No privilege wall.** All 48 halfwords MAPPED after 10A1 elevate — supervisor
  gate opens this block read-only.

## Data-port / extent stability **[C]**

Repeated elevated reads in a later session (after a KL15 cycle between sweeps):

| Register | First sweep | Repeated reads (same live session) |
|---|---|---|
| EFFF40 | `0x0631` | `0x0641` ×4 (stable within the run) |
| EFFF42 | `0x2C4D` | `0x384D` ×3 (stable within the run) |
| EFFF46 | `0x0005` | `0x0006` on the extent re-sweep |

**Conclusion:** `EFFF40`/`EFFF42` and `EFFF46` are latched / per-session registers.
They refresh across an arm / KL15 cycle, then hold steady while you sit reading
them. They are **not** a free-running FIFO (no increment across back-to-back
reads in the same session).

Extent map `EFFF44`..`EFFF5E` is otherwise empty (`0x0000`).

## What this means for flash programming

**Reachable, not yet writable.** The on-chip flash/NVM program controller at
EFFFxx is live, mapped, and readable under our existing 10A1 / level-3 mfg
session. Flashing is not gated behind an unreachable privilege wall.

Register roles for a future write sequence (roles **[I]** from the idle map +
stability behaviour; **opcodes and arm order remain [open]**):

- **EFFF06** — arm / enable gate. Idle `0xA8`. A program/erase sequence would
  flip this before any write; nothing happens to flash while it reads `0xA8`.
- **EFFF18 / EFFF1A** — command in, status out. Poll `EFFF1A` for ready/busy/error.
- **EFFF40 / EFFF42** — data port (latched word staging), confirmed stable at rest.
- **EFFF10/12/14/16** — four-bank BKSEL select (`0x3F02` each at idle).

**Stop here for live work.** Turning this into a working flash write means sending
program/erase commands to EFFF06 / EFFF18. Wrong command sequence, bank select, or
data staging can brick the module — this is the live NVM controller, not RAM. We
have the register map and a read-only baseline; we do **not** yet have the exact
command opcodes or the required arm / unlock order. Those must come from the
disassembly (the routine that writes these registers) or a silicon doc — not from
guessing on the bench.

Note: earlier notes place the MMC2114-family SGFM base at `0x00D00000` from the
public MMC2114/D Rev.1 manual. This probe is of the live **EFFFxx** block on the
SC550080-class part in the 818.3 (M3) module. Relationship between `0xD00000` and
EFFFxx on this silicon is still **[open]**; do not treat them as proven aliases
from this note alone.

## Open

- [ ] Recover exact EFFF18 command values and EFFF06 arm sequence from the image
      (paper only; no live write).
- [ ] Relationship of EFFFxx to the MMC2114 SGFM base `0x00D00000` on this part.
- [ ] Whether a program must also write the ECC window at `0x00100000+`.
- [ ] Meaning of the populated config words `EFFF00/02/04/08/0C/0E` and the
      `EFFF20`..`EFFF2A` run — logged, not assigned.

## Method note

No BIN was written. No program or erase command was sent. Values above are from
live elevated halfword reads on the **E46 MK60 818.3 (M3)** bench module on
2026-11-27.
