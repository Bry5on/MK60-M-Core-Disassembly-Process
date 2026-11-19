# MK60E5 M·CORE image recovery — the process

How to turn a fresh BMW MK60E5 ABS/DSC firmware image (`.0pa` or `.cpu.hex`) into a complete,
de-noised, labeled disassembly in Ghidra. This is the canonical description of the process; the
Claude skill at [`.claude/skills/mcore-image-recovery/SKILL.md`](.claude/skills/mcore-image-recovery/SKILL.md)
is a thin pointer to this file. Do the one-time toolchain setup in [`SETUP.md`](SETUP.md) first.

## Why a raw import isn't enough

The MK60E5 ECU runs a Motorola/Freescale **M·CORE** core (big-endian, fixed 16-bit instructions).
Stock Ghidra has no M·CORE; `SETUP.md` builds a patched one (`tools/patches/mcore.patch`, regenerated
by `make_mcore_patch.py`: opcode/bitfield fixes, and the p-code width of `ld.b`/`ld.h`/`st.b`/`st.h`,
which upstream emitted as 4-byte loads and stores). Even with it, M·CORE's SLEIGH models
every branch (`bt/bf/br/bsr`) as a *computed* jump, so Ghidra's auto-analysis:

- leaves branch-/jump-/pointer-reached code as raw `??` bytes,
- falls through tail jumps into literal pools and flags them red ("bad instruction"),
- and **mis-decodes the data/calibration section into thousands of spurious functions**
  (bytes of a lookup curve that happen to decode as valid M·CORE).

The scripts here fix all of that. The boundaries that bound the work are found **programmatically**
(no hand-entered per-image addresses) — see Step 2.

> **No firmware is included** (BMW/ATE copyright). Bring your own `.0pa` or `.cpu.hex`. `.gitignore`
> already keeps images, `.bin`s, `.cpu.hex` and Ghidra `projects/` out of version control.

## Pipeline at a glance

```
 (1) mcore_import.sh    arch-check  →  parse .0pa --reloc  →  import MCORE:BE:32  →  CreateMemBlocks
 (2) mcore_recover.sh   find_boundary.py (auto codeHI/dataHI)
                        →  FixLiteralPools
                        →  [RecoverRefCode · RecoverCodeRuns · RecoverPrologues · RecoverFlowCode ·
                            RecoverCallTargets (image-wide) · RecoverTablePointers (data-section tables) ·
                            FixJumpTables · FixLiteralPools · FixResidual · RetraceBodies ·
                            FunctionizeOrphans] ×N
                        →  DataSectionClean auto  (pins codeHI = last prologue fn end, dataLO = last
                                                   real fn end incl. call-target islands → <prog>.bounds)
                        →  FixStorage2 → ClassifyData · ApplyDataTypes → ClearAnalysisBookmarks
                        →  ScanMisdisasm · PtrTargets · CountErr
                        ] × rounds, until a round starts with nothing to do
                        →  ReadOnlyFlash → AnalyzeSignatures  (params / return / stack locals, once, SIGS=0 skips)
 (3) mcore_port.sh      (optional) carry names over from an already-named reference project
                        (by signature, then by call-site / table-slot propagation from every match)
 (3b) mcore_fuzzy_port.sh  (optional, after 3) the fuzzy layer: DumpMasked → fuzzy_port.py (6-gram
                        containment of the masked instruction stream) → ApplyPortedNames
 (V) mcore_verify.sh    verify + quantify the memory map (read-only, GUI may be open)
```

Run everything from the repo root. Each driver prints a one-line usage if under-argued. In the
examples `<proj_dir>` is any directory you pick for the Ghidra project and `<name>` its project name.

Close the Ghidra GUI for any step that writes back (import and recover) — Ghidra takes an exclusive
project lock; a step that prints `Unable to lock` means the GUI is still open.

Every driver exits non-zero when its headless session fails or its result isn't clean (see each
script's header), and keeps the full Ghidra log under `<proj_dir>/` — so they are safe to chain and
a crash can't masquerade as success.

---

## Step 1 — Parse, arch-check & import

```bash
tools/ghidra/mcore_import.sh <image.0pa | image.cpu.hex | image.bin> <proj_dir> <name>
```

- **`parse_0pa.py --reloc 0x48000:0x100000:-0x8000`** — the app is stored 0x8000 above where it
  runs (VBR=0x40000 set by crt0); this writes `<stem>.cpu.hex` at **CPU runtime addresses** next to
  the `.0pa`. *Analyzing at .0pa addresses makes every absolute call land mid-function — don't skip
  the reloc.* If you already have a `.cpu.hex`, pass it and this is skipped.
- **Raw `.bin` (chip-off / dump-tool image):** converted next to it with `tools/bin2hex.py`
  (`BASE=<hex>` places it, default `0` = a full-chip dump that starts at the reset vector, which is
  how the existing images map flash; `BIN_SKIP=<n>` drops a tool header). A dump of only the
  application sectors is placed with `BASE=40000`. Verified: the M3 app region `.bin` from
  `parse_0pa.py` converted with `--base 0x40000` matches the `.cpu.hex` byte for byte. **Check the
  size first** — an MK60E5 application is ~0.75 MB from `0x40000` (the four E9x/E82 images are
  `0x40000..0xF6A23`), so a file of a few KB is not flash at all (a 4 KB RAM/stack snapshot with
  0xA5A5 fill and `0x0040xxxx` pointers was handed in first; `find_boundary.py` gave 0 prologues).
- **Pre-E5 MK60 (E46-era, `projects/820.3_E46_non_m_mk60`, 2026-11-16):** a 256 KB full-chip dump,
  `BASE=0`. Vector table at `0x0` (reset `0x14DE0`), code from `0x32C` to the last prologue at
  `0x3CAE8`, calibration tail `0x3CF46..0x40000` (`APT?27F1.DFT` names), no `AZ1…` identification
  block or part number, no loader marker. Same RAM window `0x400000..0x4017FF`, different
  peripherals (see the discovered blocks above). Recovers with the same pipeline once the bounds
  come from the image (`flashLO=0`, `codeLO=0x200`): 416 prologues, 1577 functions, HEALTHY.
  A second one, the E46 M3 ZCP dump (`projects/817.3_e46_m3_zcp_mk60`, 2026-11-17, `AWT?26F1.DFT`
  calibration names): **the chip is 0x41000 bytes (266,240), not 256 KB** — a first read stopped
  at `0x40000` and cut off a trailing calibration block `0x40000..0x40320` (same shape as the
  non-M's block at `0x3F860`, referenced from thirteen pool words in `0x356E4..0x3B714`), with
  erased `0xFF` from `0x40320` to the end. Dump the whole chip. The pipeline needed no change for
  the odd size: `bin2hex.py` keeps the erased run, `find_boundary.py` takes `dataHI` from the end
  of the image segment (`0x41000`), and the sidecar records it. Same layout as the non-M, same ten
  discovered MMIO pages, 410 prologues, 1670 functions, code to `0x3CDD0`, data from `0x3D1B6`,
  HEALTHY in 3 rounds; about half of its 16-byte shingles occur in the non-M image and 1.5% in
  the E9x M3, so it is the non-M's close sibling. One artefact of the erased tail: `ClassifyData`
  types a couple of all-`FF` words there (`0x4032C`, `0x4036C`) as pointers, because
  `0xFFFFFFFF` falls inside the auto-discovered `MMIO_auto_FFFFF000` block. Harmless.
  A third one, the E46 M3 non-ZCP dump (`projects/e46_m3_non_zcp_mk60`, 2026-11-17, `AUT?24F1.DFT`
  names): complete at 256 KB (data to `0x3FCEC`, the `0x320`-byte block the ZCP keeps past `0x40000`
  sits inside), 409 prologues, 1585 functions, code to `0x3C910`, data from `0x3CCF6`, HEALTHY in 3
  rounds, no pipeline change; 96% of its functions are the ZCP's. **Completeness check for any new
  dump:** resolve every `lrw` pool slot in the raw file and list the values in `[image end, 0x400000)`;
  bit-mask constants (`0x60001`, `0xA0008` …) are normal, a cluster of values just past the end is a
  truncated read (that is how the ZCP's missing sector was caught).
  A fourth one, the E46 811.3 non-M (`projects/811.3_e46_non_m_mk60`, 2026-11-18): 237,568 bytes
  (`0x3A000`, the read stops after the last written sector), complete by the same check, 370 prologues,
  1164 functions, code to `0x36ED0`, data from `0x372B6`, HEALTHY in 3 rounds, no pipeline change; an
  earlier build, only 53% of its functions identical to any of the other three.
- **Arch sanity check** (`find_boundary.py`, non-gating) runs *before* the slow import and warns if
  the image doesn't look like M·CORE (see Step 2) — so you don't burn an analysis run on, say, an
  ARM or PowerPC image fed in by mistake.
- Import as `MCORE:BE:32:default` with `IntelHexLoader` + auto-analyze.
- **`CreateMemBlocks.java`** adds the RAM/MMIO blocks the flash image doesn't carry (so RAM and
  peripheral refs resolve). These ranges are the same across all MK60E5 variants (same SoC):
  RAM `0x400000–0x423FFF`; MMIO `0xD50000–0xDE0FFF`, `0xE00000–0xE10FFF`, `0xF50000–0xFF0FFF`,
  `0xFFF00000–0xFFF84FFF` (volatile).
  Those are the E5 SoC. The script then **discovers** the rest from the image: every 4 KiB page in
  the SoC's 24-bit space (or the `0xFFF00000` window) that ≥ 16 aligned pointer words name, merged
  into runs and mapped volatile as `MMIO_auto_<addr>` (values outside that space are instruction
  pairs, not pointers — an earlier, looser rule mapped 786 phantom pages). The pre-E5 MK60 dump
  (`projects/820.3_E46_non_m_mk60`) gets ten of them: `0xCF0000`, `0xCF2000`, `0xCF6000–0xCF7FFF`,
  `0xCFF000`, `0xEF8000`, `0xEFF000`, `0xFF8000`, `0xFFF000`, `0xFFFF8000`, `0xFFFFF000`; the E5
  images only re-find pages the fixed list covers.

The program's name inside the project is the `.cpu.hex` basename (e.g. `7846816A.cpu.hex`).

Auto-analysis logs a handful of `Failed to create function at … since its body contains referring
thunk` errors on every image (tail-call stubs it mistakes for thunks). The driver reports them as a
count and Step 2's `RetraceBodies` fixes them — they are not a problem with the import.

After this step the project is **red** (literal-pool errors) and the calibration section is full of
spurious functions. Expected — Step 2 clears it.

---

## Step 2 — Recover & finish (bounds found automatically)

```bash
tools/ghidra/mcore_recover.sh <proj_dir> <name> <cpu_hex_path> [passes]
```

One headless session runs the recovery loop (`passes` iterations, default 6) and then the finish
chain. The finish chain can expose seeds the loop never saw (a newly typed pointer, a cleaned table
edge), so the driver repeats that session — up to `ROUNDS` (env, default 4) — **until a round
starts with nothing to do**: its first pass creates no functions and reverts nothing (and so does
its last). A round that only *ends* clean is not enough — its own finish chain may have exposed
more, which the next round's first pass would pick up; converging on the first pass means the
project left behind is a fixpoint of loop + finish, so a re-run is a no-op (the cost is one extra
round of a few seconds). It prints `round N: firstPassWork=… lastPassWork=…` after each round and
`DONE: converged …` at the end; a `NOT converged` message means raise `ROUNDS` or `passes` (or just
re-run — it resumes).

### How the boundaries are found — no hardcoded addresses

The work is bounded by the code region `[codeLO, codeHI)` and the data section `[dataLO, dataHI)`.
`codeHI` is the **prologue pin** — the ceiling of the recovery window — and `dataLO` the start of the
data section proper; between them sit the few prologue-less **call-target islands** described
below (on the four MK60E5 images recovered so far, 9–10 small helpers in ~5 KB). The driver
determines all of them for *any* image:

- **`flashLO`** is the image base: the start of the largest initialized block (`0x40000` on the
  E5 `.0pa` images, whose app segment starts at the VBR crt0 sets; `0x0` on a full-chip dump of the
  earlier MK60, whose vector table sits at the reset address). The Java passes derive it the same
  way (`imageBase()`), so nothing hardcodes `0x40000`: the VBR vector table, the pointer scans for
  table bases, the read-only flash marking and the signature pass all start there.
- **`codeLO`** is `0x42000` when the image spans it (E5 layout: `0x40000–0x41FFF` is the VBR table
  + calibration directory + OSEK init stubs — handled separately by `OsInit.java` if needed), else
  `flashLO + 0x200`, the first byte after the 128-entry vector table (`find_boundary.py --code-lo`
  overrides). Everything from `flashLO` up to it is left to auto-analysis.
- **`find_boundary.py`** reads the `.cpu.hex` and scans for the M·CORE frame prologue —
  `subi r0,N` (`h & 0xFE0F == 0x2400`) immediately followed by `stm rX-r15,(r0)`
  (`h & 0xFFF0 == 0x0070`). This pair is dense and continuous through code and essentially never
  occurs in the calibration data above it (two constrained big-endian halfwords in a row,
  p ≈ 1e-6 per position). It reports:
  - **`dataHI`** = end of the largest contiguous image segment (the stray low-/high-
    memory config blocks are excluded), and
  - **`codeHI`** = a recovery ceiling a few KB above the highest prologue (function bodies/pools run
    above their entry). It also prints a prologue histogram and an **arch verdict** — too few
    prologues ⇒ "this isn't M·CORE".
- **`DataSectionClean auto`** then pins the boundaries Ghidra-side. The **prologue pin** `codeHI`
  is the end of the last function that *starts with the prologue*; spurious data-region functions
  never start with the prologue, so nothing above it is auto-analysis's idea of code. But it is not
  quite the end of the code: on every MK60E5 image a handful of **prologue-less leaf helpers** sit
  above the last prologue function (`cmpnei r2,1; movi r2,0x1f; lsli r2,0x13; …; jmp r15` — timer
  and wheel-speed-capture configuration helpers, 16–600 bytes each, called through `jsri` from the
  ISR time-slot code and the wheel-speed latch). The clean step keeps **prologue functions** and
  **protected functions** — functions that are call or tail-jump targets of trusted code (every
  function below the prologue pin, transitively) — deletes every other function above the pin, clears
  every other instruction there, and labels the code-referenced table bases, never one that is code.
  **`dataLO`** is the end of the last real function, protected islands included; it starts the data
  section proper, which must hold no code at all. Without the protection rule the islands were
  cleared to `??` every round and their entries labelled `tbl_…` table bases, and because the
  recovery window stopped at the pin nothing ever brought them back.

- The pins are written to **`<proj_dir>/<prog>.bounds`** (`FLASH_LO`/`CODE_LO`/`CODE_HI`/`DATA_LO`/`DATA_HI`;
  an older sidecar with only `DATA_LO` is read as the prologue pin). Every later round and re-run
  uses `CODE_HI` as the recovery ceiling, because `find_boundary`'s `codeHI` is only an estimate a
  few KB *above* the real boundary: without the sidecar each run re-disassembles that gap and
  `DataSectionClean` deletes it again (tens of functions, thousands of instructions of churn per
  run). The window stays at the *prologue* pin, not at `dataLO`: the tables between the islands must
  not be re-disassembled by the general passes every round either — the islands are reached by the
  one pass that needs no window, `RecoverCallTargets`. `mcore_verify.sh` reads the same file. Delete
  it, or set `CODE_HI`, to start from the estimate again.

Override any bound via env (hex, no `0x`): `CODE_LO`, `CODE_HI`, `DATA_HI`; set `DATA_LO=<hex>` to
pin the clean floor (the prologue pin) yourself instead of `auto`; `FORCE=1` proceeds despite a non-M·CORE verdict. You can also run
`python3 -I tools/find_boundary.py <cpu_hex>` on its own to inspect the histogram before recovering.

### The recovery loop (over `[codeLO, codeHI)`, ×`passes`)

| script | what it recovers |
|---|---|
| `RecoverRefCode` | code reached through function pointers whose refs were never materialized; also seeds the VBR exception-vector table (`0x40000`, 128 entries — bit 0 of an entry is the alternate-register-file flag, not an address bit) |
| `RecoverCodeRuns` | undefined runs that classify as CODE (clean decode to a terminator, not a table/padding) |
| `RecoverPrologues` | functions *inside* an undefined run. `RecoverCodeRuns` judges a run from its first byte, so a run that opens with the previous function's literal pool is rejected whole and every function after the pool stays `??` — on a fresh image ~13 KB of the code region, 50–70 functions, raising no error bookmark and no orphan count. This pass seeds on the frame prologue itself (`subi r0,N ; stm rX-r15,(r0)`, the pair `find_boundary.py` trusts) wherever it sits in undefined bytes — or in a small scalar nothing reads or writes, the other flavour: an address-taken callback whose entry byte `ClassifyData` typed `uint8_t` — and the code decodes cleanly from there. A body whose straight-line flow falls into such a mis-typed scalar (a constant that looks like a code address, e.g. `0x80000` as a bit mask, gets PARAM references and the word *at* that address is typed mid-function) has the item cleared and the disassembly continued; an unnamed `FUN_` reached by straight fall-through that nothing calls, jumps to or points at is a spurious continuation and is absorbed. `mcore_verify.sh` reports prologues left in non-code bytes (want 0) |
| `RecoverFlowCode` | branch-only-reached blocks: computes the target of a `bt/bf/br/bsr` with empty `getFlows()` from the encoding, and the pool-slot pointer of an indirect `jmpi/jsri` — from the instruction's reference to its slot, or, when it has none, from the encoding (`slot = ((PC+2) & ~3) + (imm8 << 2)`, recorded as a `READ` reference so `FixLiteralPools` types the slot; one `jsri` in nine in script-disassembled code carries no operand reference at all, and a callee whose only caller is such a `jsri` had no reference anywhere) — and **records it as a flow reference**. Ghidra only materializes computed-flow references during auto-analysis, so every branch in script-disassembled code had none: body tracing, `RetraceBodies` and the decompiler all stopped at the first `bt/bf` of such a function. The reference is what lets the rest of the loop (and the decompiler) follow it |
| `RecoverCallTargets` | **the one pass with no window** (`codeLO..dataHI`): a called address is code by definition, so every `bsr` (displacement from the encoding), `jsri` and tail `jmpi` (pointer in the pool slot, by reference or by encoding) inside a *trusted* function — any function below the prologue pin, and transitively every function this pass reaches — is resolved and, if still raw bytes, disassembled, traced (fall-through, `bt/bf/br` targets and `jmpi` tail targets, all recorded as flow references: above the pin no other pass will) and functionized; generic `tbl_` labels on the entry are dropped, a curated name is kept and becomes the function's name; the call edge and a missing slot `READ` reference are recorded. Guards on a seed: 2-aligned, inside the image, not inside a pointer table, undefined or a small scalar nothing reads/writes, clean pseudo-decode to a terminator within 64 instructions with no `0x0000`/`0xFFFF` padding word; a padding `bsr` (`0xFFFF` = `bsr .`) is ignored and spurious data-section functions are never trusted. It recovers the two kinds nothing else sees: the prologue-less islands above the pin, and in-window callees reached only through a `jsri` without a slot reference (`(*(code *)&UNK_000c6eb8)(1,…)` in the decompiler). Both the islands and the in-window callees are 8–10 per image. `PtrTargets` reports what is left (want 0) |
| `RecoverTablePointers` | functions whose only reference is a code pointer in an **untyped data-section table**: every 4-aligned word from the prologue pin up whose value is a 2-aligned code-region address. The CAN signal descriptor tables at `0xD8xxx` hold one getter per signal (`lrw; ld.h; jmp r15`, 4–6 bytes apart — `sig20`, `can_tx_0B6_sig19`), the KWP service table at `0xF30A0` the handler per record at +4 (`kwp_3e_tester_present`); an untyped word creates no reference, so `RecoverRefCode` never saw them and ~75 per image stayed raw. Seeded only when the target is not in a pointer-table run, is undefined or a small unreferenced scalar, and pseudo-decodes as a function — the frame prologue, or a clean leaf reaching `jmp r15`/`rts` within 40 instructions with no branch out and no padding word (a calibration value can look like a code address, so this is stricter than "decodes cleanly"); a target that is already orphan code is functionized; a DATA reference from the word is recorded; the word itself is not typed. `PtrTargets` reports what is left (want 0) and, informationally, table pointers into the middle of a body (~100 per image: alternate entries and descriptor fields that happen to look like addresses) |
| `FixJumpTables` | reverts absolute-pointer jump tables mis-disassembled as code; re-disassembles genuine jump-table targets incl. `jmp rN` switches |
| `FixLiteralPools` | defines the PC-relative literal pools (only inside `[codeLO, codeHI)` — a data-section table an instruction reads is not a pool, and typing its first word as a pointer because it looks like an address is a mistake the later typing passes rightly refuse to undo) and clears the pool-as-code red. The slot of an `lrw`/`jsri`/`jmpi` comes from the instruction's operand reference or, when it has none (one in eight in script-disassembled code), from the encoding, `((PC+2) & ~3) + (imm8 << 2)`, and the READ reference is recorded. Such a **PC-relative slot is data by construction**: it is typed whatever falls through into it, a flow reference into it is junk (0xFFFF padding decodes as `bsr .`), and a function whose entry is one is **a pool decoded as code** — auto-analysis fell into the pool after a `bkpt`/padding gap, functionized it, and sometimes swallowed the real function after it; about 95 such `FUN_`s per image protected their own pools from this pass for as long as function entries were treated as code. They are removed (a name the symbol port put on one stays as a label), the slots typed, a real body that had run through its own pool after an `rte`/`rfi` (not terminal in the SLEIGH) re-created so it stops at the data, the padding stub that is left cleared. Only a *trusted* `lrw` is believed — reachable from the entry of a plausibly real function (named, prologue-started, or referred to from outside by data or by trusted code; two rounds, so junk referring to junk drops out) without crossing `rte`/`rfi`/`bkpt`/`stop`/`wait`/`doze`/`sync` or a 0x0000/0xFFFF halfword — because a pool word that decodes as `lrw` names a fake slot up to 1 KB ahead; a slot inside trusted code is ignored as fake, and an untrusted `lrw` is believed only about bytes no function owns. Still guarded not to re-type an absolute-`ld` target reached by fall-through (that's truncated code, not a pool), and never to touch bytes inside a typed `Array`/`Structure` (an inline table a curator or the annotation replay typed — clearing "the 4 bytes at the slot" would destroy the whole item and leave fragments that `RecoverCodeRuns` turns into spurious functions; `FixResidual`, `ClassifyData` and `ApplyDataTypes` carry the same guard) |
| `FixResidual` | mops up padding (`0x0000`/`0xFFFF`) and pool-edge spurious decodes |
| `RetraceBodies` | drops spurious continuation functions and re-traces truncated function bodies — truncated by fall-through *or* by a branch: M·CORE branches are computed jumps, so a body traced before `RecoverFlowCode` materialized their targets stops at the first `bt/bf/br`; without this rule ~20% of the code region ends up as instructions no function owns |
| `FunctionizeOrphans` | gives the remaining owner-less code a function, rescanning until nothing new appears (a run of 4-byte switch-case stubs, each a jump-table target, yields one function per scan): runs referenced from data (function pointers, vectors, jump tables), runs referenced from a function that tracing could not absorb — a READ/WRITE reference does not count: an `lrw` reading it makes it a pool slot, not a function — prologue-started runs (`subi r0,N; stm`, or the light flavour `subi r0,N; st.w r15,(r0,k)` of a function that saves only the link register, which is what the real code behind a pool-decoded-as-code usually starts with), and *displaced entries* — a callback whose address is taken and passed as a parameter gets its first bytes auto-typed `uint8/16/32_t` by `ClassifyData`, so the run starts a few bytes after the real entry; the item is cleared and the function created at its address when the code decodes cleanly from there. Unreferenced speculative decodes are left alone and reported by `mcore_verify.sh` |

### Then the data section + finish (once)

| script | what it does |
|---|---|
| `DataSectionClean auto <dataHI>` | pins `codeHI` (prologue pin) and `dataLO` (end of the last real function, protected call-target islands included); from the prologue pin up deletes every function that is neither prologue-started nor protected, clears every other instruction to `undefined`, and labels every code-referenced table base that is not code (a stale `tbl_` on a now-code address is removed) |
| `FixStorage2` | assigns return storage (`r2`) to recovered functions — kills `<UNASSIGNED> RETURN` warnings |
| `ClassifyData` | sizes referenced-but-undefined data to `undefined1/2/4` from ld/st access width; drops VBR=0 default vector symbols |
| `ApplyDataTypes /dev/null` | turns those into stdint typedefs (`uint8/16/32_t`) so the listing reads like C; guarded to only re-type Undefined/own-typedefs/auto byte-word-dword |
| `ClearAnalysisBookmarks` | removes the orange breadcrumb bookmarks (**must be last** of the writers) |
| `ScanMisdisasm LO HI` | read-only: pointer tables still sitting inside instructions (**want 0**) |
| `PtrTargets codeLO dataHI codeHI` | read-only, four gates (**want 0** each — the driver and `mcore_verify.sh` gate on all): call and tail-jump targets of the image's functions that are not instruction starts; `poolSlotsInCode` — the slot of a trusted `lrw`/`jsri`/`jmpi` lying inside an instruction (a pool decoded as code); `tablePtrTargetsNotCode` — code pointers in data-section words at function-like raw bytes. Also lists *suspect* call sites whose target is typed data or decodes dirty (a pool word or padding decoded as `bsr` inside a junk function; 2–5 per image, informational) |
| `CountErr` | error-bookmark count (**want 0**) + function count |

### Then signatures (once, after convergence)

| script | what it does |
|---|---|
| `ReadOnlyFlash` | marks the flash image blocks non-writable. The Intel HEX loader creates them read-write-execute, and the decompiler never folds a load from writable memory — so every PC-relative literal-pool load decompiles as `*piRam0007ab3c` / `(*pcRam0007ab40)(1,0)` instead of the pool's value (`ccp_cro_pending`, `module_method_dispatch(1,0)`), and a table jump through the pool is never resolved into a switch. Read-only, the decompiler (with its default "respect read-only flags" option) propagates the pool constants, and the parameter-ID pass below sees resolved callees. The recovery scripts classify reference types, not block permissions, so a re-run of the recovery afterwards is still a no-op |
| `AnalyzeSignatures 40000 <dataHI>` | runs Ghidra's own stack-frame and decompiler-parameter-ID commands over the final function set, so functions get parameters, a return and stack locals (source `ANALYSIS`; user-set signatures are kept). Every recovery session runs `-noanalysis` — auto-analysis is what mis-decodes the image — so without this pass every function stays `undefined FUN_x(void)` and decompiled calls show no arguments. Void returns are committed: without that, no function can ever become `void`, and four in five end up `undefined4` returning whatever the last callee left in `r2`, which the decompiler then threads through every caller as `iVar1 = …; return iVar1;`. The slowest step (it decompiles every function); `SIGS=0` skips it, `SIG_TIMEOUT` (s, default 60) bounds one function |

What the signature pass still over-approximates is the **parameter count**: about a third of the
functions come out with all six register parameters (`r2`–`r7`), and a callee declared with six
forces every caller to show `in_r3` / `extraout_r4` noise. That is hand work on the functions you
care about, callee first: fix the leaf's prototype and its callers clean up on their own.

**After a change to the SLEIGH *semantics*** (p-code only — the same bytes still decode to the same
instructions, as with the `ld.b`/`ld.h`/`st.b`/`st.h` width fix) nothing in the recovery changes:
every recovery pass works on mnemonics, operands, references and bytes, never on p-code, and the
stored listing is bytes plus prototypes that Ghidra rebuilds from the installed `.sla` on open. Only
this step reads p-code (stack frames and decompiler data flow), so re-run `install_mcore.sh`, then just
`ReadOnlyFlash` + `AnalyzeSignatures` on each project — no re-recover, no re-import, names and comments
untouched. A change to the *decode* (a new constructor, a bitfield) is a different matter: re-recover.

**Done looks like:** the driver prints `DONE: converged, 0 error bookmarks, 0 mis-disassembled
tables, 0 call targets outside code` and exits 0 (the last item covers all four `PtrTargets` gates); function count in the low thousands (not 8000+, which means the data section
wasn't cleaned). `CountErr = 0` on its own is **not** enough — it was already 0 after a run whose
last pass was still adding functions. If the driver exits 4, re-run it (idempotent; it resumes from
the sidecar bounds) or raise `ROUNDS`/`passes`.

---

## Step 3 — Port symbols from a named reference (optional; writes back; GUI closed)

Only if you have an already-named reference project (e.g. a finished DSCM90). Carries its names over
by relocation-invariant signature:

```bash
tools/ghidra/mcore_port.sh <src_proj> <src_name> <src_program> <dst_proj> <dst_name> <dst_program> [min_instr]
```

- **`ExportSigs`** emits an FNV-1a-of-masked-halfwords signature per function (masks the lrw/jsri
  pool slot and branch displacement — the only things that move between variants), plus the body's
  outgoing **edges** in order: call targets (`bsr`, and `jsri` through its pool slot), tail jumps to
  another function's entry, and the pointer tables it loads through the pool (consecutive words that
  all point at instructions). Runs on a snapshot, so the source GUI may stay open.
- **`port_symbols.py`** ports a name by **signature** only when the source name is real, its
  signature is unique among named source functions, it occurs exactly once in the target, and both
  bodies have ≥ `min_instr` (default 8) instructions. Then it **propagates** from every matched pair
  (a signature match, or a target that already carries the source's name) to a fixpoint: a matched
  caller's call targets correspond position by position, and so do the slots of a pointer table it
  loads — the compiler emits the same code for the same source, so the k-th call of the same function
  is the same callee in both images. This is a property of compiled code, not of M·CORE or of any
  image; it is what reaches the short helpers the signature floor refuses (a 2-instruction
  `return r3` has no unique signature, but "the only callee of `ccp_cmd_SET_MTA`" does). Guards:
  same edge sequence in both bodies, same table length, only `FUN_` targets are named, a target two
  anchors name differently is a conflict and left alone, a callee pair whose signatures differ is
  accepted only when the sizes are within 25%, and a source name may land on one target only. Named
  table bases (`ccp_command_table`) are ported as labels. `ported.tsv` carries the method per row
  (`signature` / `call-site` / `table-slot` / `table-label`). The rest is the **manual tail** it
  writes to `tail.txt`:
  - `src-ambiguous`/`dst-ambiguous` — one signature, several candidates that no anchor resolved;
    disambiguate by call-site context, add the pairs to `ported.tsv`, re-run `ApplyPortedNames`.
  - `conflict` — propagation disagreed; look at both anchors.
  - unmatched targets — variant-specific or never-named-in-source.
- **`ApplyPortedNames`** applies the names (guarded to only rename `FUN_`/`LAB_`) and notes the
  method in each function's plate comment.
- **Same image?** If the reference is a project of the *same* image (the driver hashes both and
  `ApplyAnnotations` applies nothing when they differ), a second pass replays *every* annotation of
  the reference by address — `ExportAnnotations` → `ApplyAnnotations`:
  - **type definitions**: enums and structures are recreated in our data-type manager (shells
    first, then fields, so nested references resolve), and **composite data** — structs, arrays,
    enums, typed pointers — is applied wherever its whole span is retypeable (no instructions, no
    composite already there); scalar types only *outside* the code region and only where we have
    no item or one of the same length — the reference's scalar sizes are auto-derived like ours,
    and a stray or shrunk scalar in a code-region pool run leaves fragments the next
    `RecoverCodeRuns` pass turns into spurious code (re-running the recovery after a port must be
    a no-op, and this is what made it not one);
  - **memory blocks** the reference has and we don't (uninitialized placeholders such as a
    `secondary_bootloader` block) are recreated with the same range and permissions;
  - **function entries**: every reference entry gets a function here unless it lies inside one of
    our whole bodies (then the body model wins and a named one becomes a **label** there). In the
    only entries that are *already* instructions here are used — raw bytes are never disassembled
    on the reference's say-so (its unnamed entries, in the code region and below `0x42000` alike,
    are its own auto-analysis noise) — and entries in the data section are ignored. New functions
    then get signatures from a final `AnalyzeSignatures` run in the port driver;
  - **names**: all function names, including the short and split-at-label ones the signature match
    refuses; user labels (cal tables, RAM/MMIO variables, low-region items — generated `tbl_`/`DAT_`/
    `LAB_` names are skipped on export and overwritten on apply); a name that clashes with a
    different real name of ours is added as a secondary label, never overwritten;
  - EOL/plate comments and non-analysis bookmarks.
  What it does **not** carry: function *signatures* — those come from `AnalyzeSignatures` in
  Step 2, computed on our own listing, not copied.

### Step 3b — The fuzzy layer (optional; after the exact port; writes back; GUI closed)

The exact port needs the *whole* masked instruction stream to hash identically. A variant that
added a bounds check, dropped a call or re-ordered two statements breaks that for the entire
function although nine tenths of its code is still the same, and a reference from different
hardware (the E9x M3 against an E46 image) shares almost nothing exactly. The fuzzy layer names
what the exact pass left generic by *n-gram containment*:

```bash
tools/ghidra/mcore_fuzzy_port.sh <dst_proj> <dst_name> <dst_program> <label>=<src_proj>:<src_name>:<src_program> [more sources…]
```

- **`DumpMasked`** (read-only, on a snapshot) writes every function's masked halfword stream —
  the same masking as `ExportSigs` (pool-slot and branch displacements dropped), but the sequence
  itself instead of its hash: `entry<TAB>nInstr<TAB>name<TAB>hw0 hw1 …`.
- **`fuzzy_port.py`** builds an inverted index of the 6-grams of every *named* source function
  (all sources together, **closest relative first** — a tie in score goes to the earlier source)
  and, for every still-generic target function of ≥ 20 instructions, scores each candidate by
  `|T ∩ S| / |T|`, the fraction of the target's 6-grams the source explains. A name is applied
  only when the best candidate scores ≥ 0.60, the runner-up is ≥ 0.10 below (unique), the target
  is also that source function's best target (mutual best — a library leaf two targets both
  resemble names neither), the source has between 0.5× and 2× the target's n-gram count (a small
  target whose n-grams all occur inside a large source is an idiom, not a twin), and the name is
  not already present in the target (every name lands once — the exact pass's names are never
  duplicated). `--n`, `--min-instr`, `--min-score`, `--margin`, `--size-lo/--size-hi` tune it
  (`FUZZY_ARGS` in the driver). Validation: with the target's names stripped it re-finds every
  exact twin of ≥ 20 instructions at 1.00 with the same name.
- **`ApplyPortedNames`** applies them (same `FUN_`/`LAB_` guard); the plate comment carries the
  method column, `fuzzy 6-gram score=0.70 src=M3:tcs_driveslip_curve@0c795e`, so a fuzzy name is
  recognisable as one and traceable to its source function.
- The **tail** (`fuzzy_tail.txt`): `ambiguous` (runner-up too close), `not-mutual`, `name-taken`,
  `below-floor` (best candidate under 0.60, with its score — the hand-review list), `nomatch`.

What to expect: between close siblings the exact pass takes almost everything and the fuzzy layer
adds little (E46 M3 ZCP from the non-M E46: 39 exact, 0 fuzzy — the non-M carries only 39 names);
across hardware it is the only thing that reaches anything beyond library leaves (non-M E46 from
the E9x M3: 24 exact + 15 fuzzy; E46 M3 ZCP from the E9x M3: 4 exact + 1 fuzzy at 0.70, with
`kwp_09_proprietary` 0.60, `coding_image_seal` 0.58, `mfg_rx_service` 0.52 just under the floor).
Lowering the floor or `--n` trades precision for a handful more — review the tail instead.

The image is already fully recovered after Step 2 — porting is about readability.

---

## Verify & quantify the memory map

```bash
tools/ghidra/mcore_verify.sh <proj_dir> <name> <image.cpu.hex>      # FULL=1 for every table row
```

One read-only run against a snapshot of the project (the GUI may stay open). It takes the bounds
from `<prog>.bounds`, runs the scanners below, and prints a summary — error bookmarks, mis-disassembled
table runs, code bytes in the data section, orphan code runs referenced by data or by a function
(all want 0), prologues still sitting in non-code bytes (functions the recovery never
disassembled; want 0), call and tail-jump targets that are not code (functions the recovery never
disassembled *or cleared*; want 0), pool slots inside instructions (a pool decoded as code; want 0),
table pointers to undisassembled functions (want 0), code bytes owned by no function at all (unreferenced
speculative decodes; informational), suspect call sites (informational), function count, table-base
count (from the prologue pin up), the island span between the two pins, and the function count in
the region below `codeLO` — exiting 4 if anything gated isn't clean. The individual
scanners, if you want one on its own (`dump.sh` shows the snapshot idiom):

- **`CountErr.java`** — error bookmarks (want 0) + function count. The headline health check.
- **`ImageMap.java`** — every block + the code/defined-data/undefined landscape; confirms where
  defined bytes end and that the code region has little residual undefined.
- **`ScanMisdisasm.java`** — detects ≥4 consecutive code-pointer words inside instructions (a
  mis-disassembled jump table). **Must read 0.**
- **`UndefPrologues.java [LO HI]`** — frame prologues in bytes that are not instructions (undefined,
  or a small scalar nothing reads/writes): functions never disassembled, invisible to every other
  check because raw `??` raises nothing. **Must read 0**; `RecoverPrologues` is what clears it.
- **`PtrTargets.java [codeLO dataHI codeHI]`** — walks every `bsr`/`jsri`/`jmpi` inside a function
  of the image, resolves the target (encoding, or the pool slot by reference or encoding) and reports
  the ones that are not instruction starts: `callTargetsNotCode` + `tailJumpTargetsNotCode` **must
  read 0** (`RecoverCallTargets` clears them). `poolSlotsInCode` — a trusted `lrw`/`jsri`/`jmpi`
  slot inside an instruction, i.e. a pool decoded as code — **must read 0** (`FixLiteralPools`
  clears it); `tablePtrTargetsNotCode` — a 4-aligned word from the prologue pin up holding a
  code-region address at function-like raw bytes — **must read 0** (`RecoverTablePointers` clears
  it). `callTargetsSuspect` (typed data / dirty decode: junk call sites), `callTargetsNotFnEntry`
  (an alternate entry into another body), `tablePtrTargetsOrphanCode` and `poolPtrCodeLikeNotCode`
  (lrw'd pointers to function-looking raw bytes; 0 on all four images) are informational. The only
  check that sees a prologue-less function that was cleared or never disassembled.
- **`DataTableInventory.java [DATA_LO DATA_HI]`** — harvests code-referenced table bases in the data
  section: how you quantify the data map (grouped by reader subsystem).
- **`UpperScan.java [LO]`** — classify undefined/code runs from an address up (PADDING / MONOTONIC /
  PTRTABLE / CODELIKE / OTHER).
- **`RegionStats.java LO HI`** — functions / instructions / undefined bytes in a range, plus
  `orphanInstrBytes` / `orphanRuns` / `orphanRefdRuns` (instructions no function owns); the verify
  driver uses it for the code region (want orphan ≈ 0), the data section (want `instrBytes=0`) and
  the region below `codeLO`.

Record, per image: the MISR-signed spans (from the in-ECU self-test range table), `find_boundary.py`'s
bounds, the `ImageMap` landscape, the `DataTableInventory` table count, and the RAM/MMIO blocks.

---

## What to watch for

- **An absolute `ld.b`/`ld.h` of a small in-flash table is not a pool slot.** `FixLiteralPools`
  harvests absolute in-flash reads as pool words too; on the pre-E5 image a `ld.h` at `+0xE` of a
  four-entry RAM-pointer table (`0x650..0x65F`) made `0x65E` a "slot", and clearing its 4 bytes
  wiped the first instruction of the function at `0x660` every pass — `ApplyDataTypes` then typed
  the hole `uint8_t`, `RecoverPrologues` re-disassembled it, and the driver never converged
  (`firstPassWork=2` forever). Absolute read targets now have to be 4-aligned word reads, and a
  slot whose 4 bytes overlap a prologue function's entry is never cleared. `RecoverCallTargets`
  and `RecoverPrologues` also restore an entry that has lost its instruction (function exists,
  body starts at `+2`) instead of refusing it as dirty/typed.
- **A pool constant that equals a flash address vouches for nothing.** `0x338F4` on the pre-E5
  image holds `0x8F38` (a constant) and Ghidra typed it as a pointer, so the two-instruction junk
  function on the pool slot at `0x8F38` counted as "referenced from data" and stayed trusted
  (`poolSlotsInCode=1`). An entry that an `lrw`/`jsri`/`jmpi` of another function READS is a pool
  word whatever else points at it (`candidateReal` in `FixLiteralPools`).
- **A tiny "dump" is not flash.** The arch check is the gate: 0 prologues on a file of a few KB
  means a config-EEPROM (512 B, bytes inverted) or RAM snapshot was handed in, not the application.
  Nothing downstream is meaningful on it; get the flash sectors (`0x40000` up, or the whole chip).
- **"No errors" ≠ correct.** Pointer bytes decode as valid M·CORE and raise no error bookmark, so a
  mis-disassembled table is invisible to `CountErr`. Always run `ScanMisdisasm` (want 0).
- **"No errors" ≠ complete, either.** Code that is correctly disassembled but owned by no function
  (a body truncated at a branch) raises nothing and still counts as `code=` in `ImageMap`. It shows
  up as a thin symbol port (a named reference function with a smaller or missing twin) and as
  `orphanInstrBytes` in `RegionStats` — the verify summary reports it and `RetraceBodies` fixes it.
- **What is left as "code bytes owned by no function" after a clean run is small and worth a
  manual glance.** `mcore_verify.sh` prints the number; on a converged image it should be a few
  hundred bytes at most, all unreferenced. What it has turned out to be: junk decodes at the edge of
  a literal pool (a run that starts with `bkpt`/`rte`/`dect` and is a dozen bytes), a lone `br` that
  no body claims, and occasionally a genuinely unreferenced routine. To inspect, run `RegionStats`
  with a small range or `UpperScan`, and fix a real one with `ForceFn.java` (functionize) or
  `ForceData.java` (revert). Anything larger, or any run that *is* referenced, means a recovery rule
  is missing — see the three cases above (branch references, vector table, displaced entries), which
  is how this list was built.
- **"No errors" ≠ complete, part three: a prologue-less function raises nothing at all.** Leaf
  helpers have no `subi r0,N; stm` pair, so neither `find_boundary.py` nor `UndefPrologues` sees
  them, and a cleared one has no orphan run to count. Above the prologue pin that was every island
  (cleared to `??` and labelled `tbl_…` each round); in the window it was any callee whose only
  reference was a `jsri` with no slot reference. What is called is code — `PtrTargets` gates on it
  and `RecoverCallTargets` recovers it, window or not. The same holds for function pointers stored in
  *untyped data-section tables* (the CAN signal descriptor tables at `0xD8xxx` hold getters such as
  `sig20`/`can_tx_0B6_sig19` at `0x7ABxx`, 4–6 bytes apart; the KWP service table's handler field):
  about 75 code-pointer words per image pointed at raw bytes, invisible to `RecoverRefCode` because
  an untyped table word creates no reference — `RecoverTablePointers` seeds them under a stricter
  decode test, and `PtrTargets` gates on what is left.
- **A literal pool decoded as code hides in plain sight.** Auto-analysis fell into pools after a
  `bkpt`/padding gap, functionized them (`ldq r4-r7,(r0); st.w ...`, or `bsr .` for 0xFFFF padding)
  and sometimes swallowed the real function after the pool (`kwp_3e_tester_present` lived inside
  `FUN_000b1210`, kwp_dispatch's pool). Nothing red, nothing orphaned, and `FixLiteralPools` could
  not type the slots because function entries and fall-through targets counted as code — about 95
  such functions and 300–380 slots per image. The rule now is that a PC-relative slot is data by
  construction (see the `FixLiteralPools` row); the symptom to look for is a `FUN_` with no
  prologue whose body reads as `ldq`/`stop`/`bsr .` and whose entry an `lrw` elsewhere loads from.
  `mcore_verify.sh` gates on `poolSlotsInCode`.
- **"No errors" ≠ complete, part two: raw `??` in the code region can be whole functions.** A
  constant that happens to look like a code address (`0x80000` used as a bit-19 mask, say) gets
  PARAM references from the `lrw` that loads it; `ClassifyData` then types the word *at* that
  address even when it is mid-function, the body that falls into it is never traced, and a run of
  functions behind a literal pool is rejected by `RecoverCodeRuns` as a unit. Nothing red, nothing
  orphaned — the only tell is `undefBytes` in the code region and the prologues inside it.
  `RecoverPrologues` recovers them; `mcore_verify.sh` gates on `UndefPrologues` = 0.
- **Don't force-disassemble the residual `??`.** After recovery, remaining undefined in the code
  region is literal/inline pools (data), and `??` in RAM is uninitialized (correct). The data section
  is *supposed* to be undefined-then-labeled.
- **Below `codeLO` (0x40000–0x41FFF) is left as auto-analysis made it.** That region is the VBR
  table, the calibration directory and the OSEK init stubs; the pipeline never cleans it, so expect a
  few dozen tiny (2–24 byte) `FUN_` functions there and `0xFFFF` padding decoded as `bsr`. They raise
  no error bookmark and don't affect the code region. `mcore_verify.sh` reports the count so it is
  visible; `OsInit.java` disassembles the real init stubs if you need them, and `ForceData.java`
  reverts a specific spurious one.
- **Trust the auto boundaries, but glance at them.** `DataSectionClean auto` prints both pins:
  `CODE_HI` is the end of the last prologue function, `DATA_LO` the end of the last protected island
  above it (the line also says how many islands and which function is last). If a disassembly looks
  truncated at the top of the code, check those numbers and the `ImageMap` landscape; `mcore_verify.sh`
  prints the island span.
- **Shared tails stay separate functions.** When several functions jump to one block (switch-case
  stubs converging on a common epilogue), Ghidra can't put it in all their bodies, so it stays its
  own small `FUN_` and the stubs end in a jump to it. `RetraceBodies` keeps such an entry even when
  it is also a fall-through target. It is also why a few reference functions still compare as
  "smaller" in a symbol port: the port signature covers the body, not the tail.
- **2-byte-body "quirk".** A scripted headless `createFunction` used to leave 2-byte function
  bodies that remove+recreate could not grow. The cause was the missing computed-flow references
  (above): an entry whose first instruction is a `br`/`bt` with no reference has nowhere to trace to.
  Now that `RecoverFlowCode` records them, such bodies retrace like any other; a retrace that comes
  back no larger is counted as `unchanged` in the `RetraceBodies` line and does not keep the
  convergence loop alive.
- **`FixLiteralPools: … ramRefsSkipped=N failed=M`.** Absolute `ld`/`st` references into RAM look
  like pool slots but are variables in an uninitialized block, so they are skipped and counted (large
  N is normal). `failed` is the rare slot that can't be typed at all (its 4 bytes run off the end of a
  block); the line names it with the exception. Both are constant across passes and not errors.
- **`CONCAT31`/`CONCAT22` everywhere, byte variables shown as 32-bit globals, and "Globals starting
  with '_' overlap smaller symbols" warnings** were the SLEIGH, not the image: upstream `mcore.sinc`
  writes `regZ = *[ram]:1 EA;` for `ld.b` (`:2` for `ld.h`, and the mirror for `st.b`/`st.h`), and
  SLEIGH sizes the access from the 4-byte register, so every byte/halfword load and store reached the
  decompiler as a word — three byte stores `AA / x / 8C` at `0x403322..24` showed as one 32-bit value
  at `0x403323..26` plus `0x8C` at `0x403327`. `mcore.patch` now makes the width explicit
  (`zext(*[ram]:1 EA)`, `*[ram]:1 EA = regZ:1`; `ld.b`/`ld.h` zero-extend on M·CORE). Check with
  `Instruction.getPcode()`: `LOAD`/`STORE` must carry a 1- or 2-byte value operand. `ldm`/`stm`/`ldq`/
  `stq`/`lrw`/`ld.w`/`st.w` are word accesses and were right.
- **`Flow Override: CALL_RETURN (COMPUTED_CALL_TERMINATOR)`** on a `jmpi`/`br` is a benign tail call.
- **Single-site fixes:** `ForceFn.java` (clear+disassemble+functionize one address), `ForceData.java`
  (revert to data), `FixFnEntry.java` (re-root a late-seeded entry). Avoid whole-body "repair" clears —
  they break tail-call stub entries.

## Script / tool reference

### Drivers & standalone tools (`tools/`)

| path | role |
|---|---|
| `tools/install_mcore.sh` | install/rebuild the patched M·CORE SLEIGH (see SETUP.md) |
| `tools/parse_0pa.py` | .0pa → `.cpu.hex` (with `--reloc`) + region `.bin`s |
| `tools/bin2hex.py` | raw `.bin` → Intel HEX at `--base` (default 0), `--skip`/`--length` for tool headers; `mcore_import.sh` calls it for `.bin` inputs |
| `tools/find_boundary.py` | prologue-scan: arch check + programmatic flashLO/codeLO/codeHI/dataHI from a `.cpu.hex` (`--code-lo` overrides) |
| `tools/port_symbols.py` | signature 1:1 matcher + call-site/table-slot propagation; writes `ported.tsv` + `tail.txt` |
| `tools/fuzzy_port.py` | step 3b: masked-halfword 6-gram containment scorer over one or more `DumpMasked` sources; writes `fuzzy.tsv` + `fuzzy_tail.txt` |
| `tools/ghidra/mcore_import.sh` | step 1: arch-check + parse + import + `CreateMemBlocks` |
| `tools/ghidra/mcore_recover.sh` | step 2: auto-bounds + recover + finish, repeated until a round starts clean; writes `<prog>.bounds` (`FLASH_LO/CODE_LO/CODE_HI/DATA_LO/DATA_HI`) |
| `tools/ghidra/mcore_port.sh` | step 3: `ExportSigs` ×2 + match + propagate + `ApplyPortedNames` |
| `tools/ghidra/mcore_fuzzy_port.sh` | step 3b: `DumpMasked` per project + `fuzzy_port.py` + `ApplyPortedNames` |
| `tools/ghidra/mcore_verify.sh` | V: read-only health check + memory-map numbers in one run (snapshot; GUI may be open) |
| `tools/ghidra/dump.sh <proj_dir> <name> <cpu_hex> ADDR:N …` | read-only disassembly dump from a snapshot (GUI may be open) |

### Ghidra scripts (`tools/ghidra/scripts/*.java`, run via `-postScript`)

The drivers run these for you; listed so you can run one on its own. Bound-configurable scripts take
`LO HI` (hex, no `0x`) and default to `0x42000`/`0xD7000` when called with no args.

| script | step / role | args |
|---|---|---|
| `CreateMemBlocks.java` | 1 — add RAM/MMIO blocks (E5 fixed list + pages discovered from pointer density) | — |
| `FixLiteralPools.java` | 2 — define pools (slots by reference or encoding), remove pools decoded as code, clear pool-as-code red | `[LO HI]` |
| `FixResidual.java` | 2 — mop up padding / pool edges | — |
| `RecoverRefCode.java` | 2 — code reached via unmaterialized pointers | `LO HI` |
| `RecoverCodeRuns.java` | 2 — functionize CODE-classified runs | `LO HI` |
| `RecoverPrologues.java` | 2 — functionize prologue-seeded code inside undefined runs; clear mis-typed scalars a body falls into | `LO HI` |
| `RecoverFlowCode.java` | 2 — branch-only-reached blocks | `LO HI` |
| `RecoverCallTargets.java` | 2 — every `bsr`/`jsri`/`jmpi` target of trusted code, image-wide; islands above the pin | `CODE_LO DATA_HI` |
| `RecoverTablePointers.java` | 2 — functions pointed to only from untyped data-section tables | `CODE_LO CODE_HI SWEEP_LO DATA_HI` |
| `FixJumpTables.java` | 2 — revert table-as-code, redo jump-table targets | `LO HI` |
| `RetraceBodies.java` | 2 — drop spurious continuations, re-trace bodies | `LO HI` |
| `FunctionizeOrphans.java` | 2 — functionize referenced / prologue-started orphan code | `LO HI` |
| `DataSectionClean.java` | 2 — pin codeHI + dataLO, delete spurious fns (keep prologue + protected), label tables | `auto DATA_HI [CODE_LO]` \| `PIN DATA_HI` |
| `FixStorage2.java` | 2 — assign return storage | — |
| `ClassifyData.java` | 2 — size referenced undefined data | — |
| `ApplyDataTypes.java` | 2 — undefined → `uint8/16/32_t` typedefs | `<types.tsv or /dev/null>` |
| `ClearAnalysisBookmarks.java` | 2 — drop breadcrumb bookmarks (**last**) | — |
| `CountErr.java` | 2/V — error-bookmark + function counts | — |
| `ReadOnlyFlash.java` | 2 — flash blocks non-writable so the decompiler folds pool loads (before `AnalyzeSignatures`) | — |
| `ExportSigs.java` | 3 — per-function porting signatures + call/jump/table edges | `<out.sigs>` |
| `ApplyPortedNames.java` | 3 — apply `addr<TAB>name` table | `<ported.tsv>` |
| `DumpMasked.java` | 3b — per-function masked instruction halfword stream (read-only) | `<out.masked>` |
| `AnalyzeSignatures.java` | 2 — params / return / stack locals via Ghidra's stack + parameter-ID commands | `LO HI [timeout_s]` |
| `ExportAnnotations.java` | 3 — dump blocks/types/entries/names/labels/comments/bookmarks/data types + image hash (read-only) | `<out.tsv>` |
| `ApplyAnnotations.java` | 3 — replay them by address; refuses on image-hash mismatch | `<annotations.tsv>` |
| `ImageMap.java` | V — block list + code/data/undefined landscape | — |
| `ScanMisdisasm.java` | 2/V — detect table-as-code (want 0) | `[LO HI]` |
| `UndefPrologues.java` | V — prologues in non-code bytes = functions never disassembled (want 0) | `[LO HI]` |
| `PtrTargets.java` | 2/V — call/tail-jump/table-pointer targets that are not code, pool slots inside code (all want 0) + suspect call sites | `[CODE_LO DATA_HI [CODE_HI]]` |
| `DataTableInventory.java` | V — harvest code-referenced table bases | `[DATA_LO DATA_HI]` |
| `UpperScan.java` | V — classify undefined/code runs from an address up | `[LO]` |
| `RegionStats.java` | V — functions/instructions/undefined/orphan-code bytes in a range | `LO HI [listMax]` |
| `DumpAt.java` | V — dump disassembly at `ADDR:N` (via `dump.sh`) | `ADDR:N …` |
| `ForceFn.java` | fix — clear+disassemble+functionize one address | `<addrHex=name …>` |
| `ForceData.java` | fix — revert addresses to data | `<addrHex …>` |
| `FixFnEntry.java` | fix — re-root a late-seeded function entry | `<addrHex=name …>` |
| `OsInit.java` | 1/fix — disassemble OSEK init stubs below `0x42000` | `<types.tsv> <addrHex …>` |

## Acknowledgment

This process was developed against and cross-validated with cmg5461's
[`mk60e5-firmware`](https://github.com/cmg5461/mk60e5-firmware): its independent M·CORE
disassembler (`mcore_dis.py`) and reference listing, its documented symbol and memory maps
(`*_symbols.md`), and its signature/self-test analysis (`bmy_resign.py`). The code in this repo is
our own, but the understanding it encodes rests on that work — credit it alongside this if you
build on it. See [CREDITS.md](CREDITS.md).
