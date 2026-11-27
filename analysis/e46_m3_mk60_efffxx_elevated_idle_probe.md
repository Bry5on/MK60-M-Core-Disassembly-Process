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

**Live base is EFFFxx — settled [C].** This probe proves the on-chip flash/NVM
controller the 818.3 (M3) actually exposes under supervisor is at EFFFxx (BKSEL
`0x3F02`×4, status `0xFB00`, gate `0xA8`, data port EFFF40/42). For programming,
drive EFFFxx.

Earlier notes place the MMC2114-family SGFM base at `0x00D00000` from the public
MMC2114/D Rev.1 manual. Whether this SC550080-class part *also* decodes
`0xD00000` as an alias/mirror of EFFFxx is a soft leftover and does not change
the live programming target.

## Open

- [ ] Recover exact EFFF18 command values and EFFF06 arm sequence from the image
      (paper only; no live write).
- [x] Live flash/NVM controller base on this module — **EFFFxx** (settled by this
      probe). Soft leftover only: whether `0x00D00000` is also decoded as an
      alias/mirror (does not change the programming target).
- [ ] Whether a program must also write the ECC window at `0x00100000+`.
- [ ] Meaning of the populated config words `EFFF00/02/04/08/0C/0E` and the
      `EFFF20`..`EFFF2A` run — logged, not assigned.

## Method note

No BIN was written. No program or erase command was sent. Values above are from
live elevated halfword reads on the **E46 MK60 818.3 (M3)** bench module on
2026-11-27.

---

# Write-path findings — static analysis (2026-11-27)

**Paper only.** Recovered by static disassembly of the **E46 MK60 818.3 (M3)**
flash image (`818.3.bin`, 256 KiB, md5 `7febd4b26eab79154411efc96c035b11`,
MCORE:BE:32, base `0x0`). Nothing was sent to the module; no program/erase was
performed. Addresses below are image offsets.

Confidence: **[C]** directly evidenced in the disassembly/decompile; **[I]**
interpretation; **[open]** not established.

## The NVM program/erase driver — `nvm_driver_14EC0` @ `0x00014EC0` **[C]**

This is the routine that drives EFFFxx. It reaches the controller indirectly
through two pointer tables held in flash, which is why a naive decompile shows
RAM-style symbols (`puRam000150xx`) instead of EFFFxx — the pointers resolve to
the live registers:

Register pointer table @ `0x00015000`:

| Pool word | Value | Register |
|---|---|---|
| `0x15000` | `0x00EFFF06` | gate / arm (read) |
| `0x15004` | `0x00EFFF18` | command (RMW target) |
| `0x15008` | `0x00EFFF1A` | status (polled) |
| `0x1500C` | `0x00EFFF40` | data port (word staging) |

Second pointer/const table @ `0x00014EE0` and config block @ `0x00015010`:

| Pool word | Value | Role |
|---|---|---|
| `0x14EE0` | `0x00EFFF00` | config reg (ptr present) |
| `0x14EE4` | `0x0000BFFF` | mask applied to caller word |
| `0x14EE8` | `0x00EFFF02` | config reg; driver writes byte `0x10` |
| `0x14EEC` | `0x00EFFB00` | aux reg; driver writes `0x02` then `0x10`/`0x00` |
| `0x14F10` | `0x00EFFA00` | aux reg; conditional clear of bit7 |
| `0x14F14` | `0x00EFFB00` | aux reg; conditional clear of bit7 |
| `0x15010` | `0x000000A4` | **armed/ready gate value** compared vs `*EFFF06` |
| `0x15014` | `0x00007000` | address/region field mask (bits 12–14) into EFFF18 |
| `0x15018` | `0x00000007` | low command-bit mask (bits 0–2) into EFFF18 |
| `0x1501C` | `0x000088FF` | preserve mask on EFFF18 RMW |
| `0x15020` | `0x00000100` | bit8 strobe constant |
| `0x15024` | `0x00000100` | branch-select mask on a config word |
| `0x15454` | `0x00EFFF06` | gate re-read at routine exit |
| `0x15458` | `0x00015D5C` | sub called at exit (`COMPUTED_CALL` from `0x15410`) |

## EFFF06 (gate / arm) — evidenced values **[C]**

- The driver **reads** `*EFFF06` and compares it to **`0xA4`** (`0x15010`) as the
  "armed/ready" condition (`bVar16 = (0xA4 == *EFFF06)`), and gates its success
  return on `*EFFF06 != 0xA4` (encoded as the signed byte `-0x5c`).
- EFFF06 is **not written** inside this routine — it is read/tested only. Live
  idle reads `0xA8`; the code's meaningful armed value is **`0xA4`** (matches the
  earlier bench prediction, now with static evidence).
- The actual arming write target is **[open]**: candidates are the aux registers
  this routine pokes at entry — `EFFB00` (`0x02` then `0x10`/`0x00`), `EFFF02`
  (`0x10`), `EFFA00` — not EFFF06 directly.

## EFFF18 (command) — there is no single opcode; it is a bitfield RMW **[C]**

No literal "command word" is written to EFFF18. The command is **assembled by
read-modify-write with bit strobes**, polling EFFF1A between steps. The evidenced
sequence (in order):

1. `v = (*EFFF18 & 0x88FF) | (0x07 & cfgA)` → write EFFF18  (seed: preserve
   `0x88FF`, set low command bits 0–2 from a config word `cfgA`).
2. `v &= 0xFFEC` → write EFFF18  (clear bits 0,1,4).
3. `v |= 0x40` → write EFFF18  (set **bit6** = enable/initiate).
4. `v |= (0x7000 & cfgA)` → write EFFF18  (set **bits12–14** address/region field).
5. **poll** `while ((*EFFF1A >> 3 & 1) != 1)`  (wait status **bit3** = ready).
6. `v -= 0x100` → write EFFF18  (clear **bit8** strobe).
7. Stage data: `*EFFF40 = tableWord[(cfgB & 3)*8 - 8 + (cfgA >> 12)]`  (data port
   load, indexed by bank `cfgB&3` and region `cfgA>>12`).
8. Per-word program loop: `*EFFF18 = v & ~3` → poll bit3 → `v = (v & ~3) ^ 0x10 |
   (cfgB & 3)` (toggle **bit4** program strobe + bank bits) → poll **bit1** → `v
   ^= 0x10` (drop bit4) → poll bit3 → **done when status bits 0 AND 1 are both
   set**; else repeat.
9. Exit: `*EFFF18 = v - 0x100`, write `0x07` to `*0x15400` (RAM flag), call
   `0x15D5C`, then re-read `*EFFF06`.

So the controlling EFFF18 bits are, evidenced: **bit6 (0x40)** initiate,
**bit8 (0x100)** HV/sequence strobe (added/subtracted), **bit4 (0x10)** per-word
program-pulse strobe, **bits0–1 (0x03)** bank, **bits0–2 (0x07)** low command
field, **bits12–14 (0x7000)** address/region; preserve mask **0x88FF**.
The exact ERASE-vs-PROGRAM meaning of each bit is **[open]** without a silicon
doc, but the bit positions and strobe order above are directly from the image.

## EFFF1A (status) polling **[C]**

Polled bits: **bit3** = ready/handshake (gates each step), **bit1** = pulse-done,
**bit0 & bit1 together** = word complete/success. (Live idle `0xFB00`.)

## EFFF40/42 (data port) **[C]**

`EFFF40` is loaded via `*0x1500C` with a staged word from an in-flash table,
indexed by bank and region — matches the bench finding that EFFF40/42 is a
latched data-staging register, not a FIFO.

## Bank select EFFF10/12/14/16 **[open]/[I]**

The program driver does **not** write EFFF10–16 directly; bank is carried inside
EFFF18 (`0x03` / `0x7000` fields) and the EFFF40 staging index. Whether
EFFF10–16 (`0x3F02`×4 at idle) are programmed by a separate setup routine is
**[open]**.

## Callers / invocation **[open]**

There is **no static caller, and no pointer word `0x00014EC0` anywhere in the
image**. The routine is almost certainly **relocated/copied to RAM and executed
there** — standard for a flash programmer on this MMC2114-class part (you cannot
execute from flash while erasing/programming it). The on-chip trigger path is
therefore not statically resolvable from the flash image alone. The only resolved
edge is the internal `COMPUTED_CALL` at `0x15410 → 0x15D5C`.

## Separate EFFFxx accessor cluster (not the program path) **[I]**

Functions `FUN_0003c118 / c1ac / c21c / c278 / c31c / c374 / c3f4 / c674 / c730`
near `0x0003C1xx–0x0003C7xx` read/write **EFFF00, EFFF21, EFFF2B, EFFF3D, EFFF3F**
(byte RMW) and read **EFFF06/07**. These look like a distinct peripheral/config
accessor group, **separate** from the `0x14EC0` program/erase driver; their
register roles are **[open]**.

## Updated open items

- [ ] Identify the register that actually **writes** the arm value so `*EFFF06`
      becomes `0xA4` (candidates EFFB00 / EFFF02 / EFFA00). **[open]**
- [ ] Map EFFF18 bits (0x40/0x100/0x10/0x07/0x7000) to ERASE vs PROGRAM vs
      verify — needs silicon doc; positions are evidenced, semantics **[open]**.
- [ ] Confirm the RAM-relocation/invocation path for `nvm_driver_14EC0`. **[open]**
- [ ] Whether EFFF10–16 BKSEL is programmed by a separate setup routine. **[open]**
- [ ] Roles of the `0x3C1xx–0x3C7xx` EFFFxx accessor cluster. **[open]**

### Reproduction (static, read-only)

Import `818.3.bin` (md5 `7febd4b26eab79154411efc96c035b11`) as MCORE:BE:32 base
`0x0`; disassemble `0x00014EC0`; the pointer tables at `0x00015000` and
`0x00014EE0` resolve the EFFFxx targets. No live module access required.
