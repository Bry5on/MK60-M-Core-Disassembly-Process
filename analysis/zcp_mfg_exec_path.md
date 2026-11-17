# ZCP MK60 — mfg protocol write + code-execution path

Hand-written trace (2026-11-17), companion to `zcp_mk60_image.md`. Not pipeline output. Covers the
manufacturing (mfg_test_cycle) protocol's **memory-write** and **code-execution** primitives on the E46 M3
ZCP image, and what the `0xA6 'Z'` / state-7 hand-off actually does. All addresses are ZCP ROM/flash offsets
(big-endian M·CORE) unless marked RAM. Entry to the mfg session (the K-line knock → framed mode, access
level 1) is covered in `mfg_test_cycle.md` and the bench work; this doc starts from "already in framed mode".

Provenance: addresses marked **[dis]** were read directly from the ZCP Ghidra project
(`projects/817.3_e46_m3_zcp_mk60`) via `tools/ghidra/dump.sh` in this session. Addresses marked **[doc]** are
from `zcp_mk60_image.md` / `nonm_mk60_image.md` (the non-M command-class table "applies here unchanged", per
`zcp_mk60_image.md`). Nothing here is bench-confirmed on hardware yet — we have only ever *read* with `0xA4`.

---

## 1. There are arbitrary-memory WRITE commands (class 0xA5)

We dump with `0xA4` (read). Its twin **`0xA5` is write**, same memory handler **0x1B622** [doc]. From the
command-class table (non-M, applies to ZCP) [doc]:

| cmd | effect |
|---|---|
| `0xA5 'R'` | write **1, 2 or 4 bytes to any address** |
| `0xA5 'U'` | write **up to 24 bytes** (from request buffer 0x400605) **to any address** — the copy is `mfg_memcpy` |
| `0xA5 'T'` | write hex pairs from the request to a given address |
| `0xA5 'X'` | EEPROM write of one byte (NVM job 0xF0 via 0x15FC2 mode 2) |

`mfg_memcpy` = **0x1BAF0** [doc] (non-M 0x1B8D8, M3 0x7738C). Reads (`0xA4 'U'` etc.) and writes (`0xA5`) are
both available at **access level 1** — the level the knock grants. So the unit will take arbitrary-address
writes over the K-line at the level we already reach. **Untested on our unit.**

Note on flash: `0xA5` writes are plain stores. M·CORE flash needs the flash-controller program/erase
sequence, and the app image has **no flash-writing code** (`zcp_mk60_image.md`). So `0xA5` into a flash
address (0x0..0x3FFFF) will not program flash — but `0xA5` into **RAM (0x400000+) works**, which is the basis
for everything below.

---

## 2. `0xA6 'Z'` → state 7 — re-enters the command handler, NOT a loader door

State machine [dis]: 12-state jump table at **0x1AF68**, indexed by the **RAM state byte 0x400704** (main
cycle `FUN_0x1AEBC`; `lrw r1,0x1af68; ixw r1,state; ld.w r1; jmp r1` at 0x1AF26). `0xA6 'Z'` sets state 7
(non-M: "hand off to state 7 with the stored command byte; req len 2 selects 0x7B" [doc]).

State 7's handler is **0x1B0D8** (jump-table entry 7; it is a label *inside* the state-6 body `FUN_0x1B0C4`)
[dis]:

```
0001b0d8  lrw  r7,0x1b18c      ; -> RAM 0x4005BE (session flags)
0001b0da  ld.b r7,(r7,0x0)
0001b0dc  btsti r7,0x6
0001b0de  bt   0x1b0ee         ; bit6 set -> fall into the FRAMED COMMAND HANDLER 0x1B0EE
0001b0e0  movi r7,0x9 ; st.b r7,(r12,0x0)   ; else -> state 9 (idle)
0001b0ec  jmpi 0x1b19c
```

So **`Z` just re-enters the framed command dispatcher (0x1B0EE)** — the class/long-frame processor. It is
not a jump to a bootloader or any out-of-image address. As a *direct* reflash door it is a dead end; its only
use is keeping the long-frame machinery (below) running. (Framed handler 0x1B0EE dispatches classes
0xA0..0xA6 through a flash jump table at 0x1B1CC; class handlers are fixed flash: 0xA0 0x1B50A, 0xA3
access-level machine 0x1B31C, 0xA4/0xA5 memory handler 0x1B622 [doc].)

---

## 3. The code-execution primitive — `mfg_rx_service` jsr's into a RAM buffer (the prize)

The long-frame receive service **`mfg_rx_service` 0x1BFB4** [dis] (ZCP doc: "the long-frame receive service
the class `0xA6 'O'` command runs") gets a descriptor from **`mfg_rx_descriptor` 0x1BDCA** [dis], which
returns a **RAM** address — **0x401060** or **0x40116C** (pool words 0x1C084/0x1C088; selected by a flag,
then optionally +8/align/-2):

```
0001bdd4  lrw r2,0x1c084     ; *(0x1C084) = 0x00401060   (RAM)
0001bdd8  lrw r2,0x1c088     ; *(0x1C088) = 0x0040116C   (RAM)
```

Then the tail of `mfg_rx_service` computes a call target from a halfword *inside* that RAM buffer and calls it
[dis]:

```
0001c000  ld.h r6,(r14,0x0)   ; r6 = halfword at descriptor[0]
0001c002  mov  r2,r14
0001c004  addu r14,r6         ; target = descriptor + halfword
0001c006  addi r14,0x6        ;        + 6
0001c008  addi r2,0x2
0001c00a  jsr  r14            ; *** CALL descriptor + halfword + 6 ***
```

Descriptor is RAM (~0x401060) and the offset is added positive, so **the call always lands in RAM** — the
firmware executes code out of a buffer whose address we know and can fill (via `0xA5` writes, or the
long-frame `K`/`L`/`Q` commands that build that buffer). It is reached by **`0xA6 'O'`** (runs
`mfg_rx_service` with r13 = 0x80) [doc]; `mfg_rx_service` is also called from the 10 ms cycle with 0x82
(xref 0x1AF00 [dis]).

Gating before the `jsr` (not fully pinned) [dis]: a flag byte **0x4006ED** (bit 7) and a control byte at
**descriptor-3**, checked against r13 (0x80/0x81/0x82). The 0x82 path branches to 0x1BE1E (not the jsr); the
jsr is in the bit7-clear / 0x80-0x81 path. **Open:** the exact control-byte value and whether `O` reaches the
jsr at level 1.

### The 0xA6 long-frame commands (descriptor builders) [doc]
- `M` — set the active request descriptor's header byte (0x1BBB2 non-M / 0x1BDCA-family ZCP).
- `K` — install a long-frame descriptor via the frame check.
- `L` — copy the escaped frame into the **descriptor buffer** when its header is `@`.
- `Q` — load eight parameter bytes into the request area, then as `L`.
- `O` — run the active descriptor through `mfg_rx_service` with 0x80 (caps reply at 0x27).

---

## 4. Working hypothesis: this is the factory "upload a routine and run it" path

A "load a frame into a RAM buffer, then `jsr` into it" mechanism is almost certainly the factory's intended
**download-and-execute** path. That resolves the puzzle of "no resident flash-writing code in the image": the
factory does not keep flash code resident — it **uploads a transient flash loader into RAM over the K-line and
executes it via this path**, and that loader does the programming. If so, **a hidden bootloader is not
required** to reflash; we can do what the factory does. (The parent docs' "reprogramming must come from
outside" is about the app image having no resident loader — which is still true; this is the *vector* that
lets outside code in.)

### Emerging recipe (hypothesis, NOT bench-proven)
1. Enter the mfg session (K-line knock → framed mode, level 1); confirm the `0xA6` long-frame commands work
   (mode bit 7 clear).
2. Fill the descriptor buffer **0x401060** with `[halfword offset][…our M·CORE code…]` via `0xA5` writes (or
   `K`/`L`/`Q`).
3. Satisfy the gating (control byte at descriptor-3, flag 0x4006ED) so `mfg_rx_service` takes the jsr branch.
4. Send `0xA6 'O'` → `mfg_rx_service` → `jsr` into our bytes.

**Milestones (each earns confidence before the next; the first three write nothing to flash):**

1. **Prove the RAM write.** `0xA5`-write a scratch RAM word and read it back with `0xA4` — confirms arbitrary
   RAM write at level 1 on the live unit.
2. **Prove code execution.** Stage a routine that writes a marker to a scratch RAM word, trigger it
   (`0xA6 'O'`), and `0xA4`-read the word — proves arbitrary execution with nothing written to flash.
3. **Read-only CAN firmware dump (self-validating).** Stage a memory-read loop that walks flash, packs it into
   8-byte CAN payloads, and pushes them through the controller's transmit mailbox (registers `0xFFFC80`;
   model on the mailbox setup `0x157DC` and transmit `0x157B0`/`0x157BC` in `zcp_can_frames.md`); the Gauge.S
   CAN side captures and reassembles. This replaces the slow K-line `0xA4` read (~114 B/s, ~38 min for 256 KB)
   with a CAN transfer of the whole image in under a minute, and is checked **byte-for-byte against the
   existing K-line dump** — so it exercises the full write+exec chain read-only, with a perfect ground-truth
   comparison and nothing written to flash. The staged routine drives the CAN controller directly (init a
   mailbox, push, wait for complete), since the mfg session runs independently of the application's CAN task.
4. **Flash-program routine.** Only after the above: an M·CORE flash-program routine (flash-controller
   registers were located during image recovery; see `nonm_mk60_image.md` "flash-register references").

---

## 5. Open items / caveats / next steps

- **Gating:** one more Ghidra pass on the `0xA6 'O'` handler and the descriptor layout to pin the exact
  control byte / 0x4006ED value, and confirm `O` reaches the `jsr` at level 1.
- **Writes untested:** `0xA5` has never been exercised on our unit — add a *guarded* `mfg write` to the bench
  tool and prove a RAM write + `0xA4` read-back before building the exec chain on top.
- **Brick risk:** everything here writes a live module. We hold the full dump to restore, and BDM/chip-off is
  the backstop.
- **Flash routine:** needs the M·CORE flash-controller program/erase sequence + register addresses (from the
  recovery session's flash-register references).

## Address quick-reference (ZCP)

| what | addr | src |
|---|---|---|
| state jump table (12 states) | 0x1AF68 | [dis] |
| RAM state byte | RAM 0x400704 | [dis] |
| main cycle | 0x1AEBC | [dis] |
| state 7 handler (in state-6 body) | 0x1B0D8 (fn 0x1B0C4) | [dis] |
| session flags byte | RAM 0x4005BE | [dis] |
| framed command handler / class dispatch | 0x1B0EE (table 0x1B1CC) | [dis] |
| `mfg_rx_service` (long-frame receive) | 0x1BFB4 | [dis]/[doc] |
| `mfg_rx_descriptor` → RAM buffer | 0x1BDCA → RAM 0x401060 / 0x40116C | [dis] |
| rx_service `jsr` (code-exec) | 0x1C00A | [dis] |
| gating flag byte | RAM 0x4006ED | [dis] |
| `mfg_cmd_dispatch` / command table | 0x1C028 / 0x3E130 | [dis]/[doc] |
| `mfg_memcpy` | 0x1BAF0 | [doc] |
| memory handler (0xA4 read / 0xA5 write) | 0x1B622 | [doc] |

Companion: `zcp_mk60_image.md`, `nonm_mk60_image.md`, `mfg_test_cycle.md` (E9x M3), `program_request_record.md`.
