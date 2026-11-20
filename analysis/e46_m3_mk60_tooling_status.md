# E46 M3 MK60 — M·CORE tooling and 818.3 coverage

2026-11-20. A short status note: what Motorola M·CORE (MMC2xxx) tooling exists, and what a Ghidra recovery of the 818.3 image actually covers. Companion: `e46_m3_mk60_mtrack_mode4.md`, `e46_m3_mk60_818_3_function_names.md`, `e46_m3_mk60_can_map.md`, `e46_m3_mk60_traction_control.md`.

## What exists

| Tool | Where | Works on this 818.3 image? |
|---|---|---|
| Ghidra SLEIGH module | [CUB3D/ghidra-mcore](https://github.com/CUB3D/ghidra-mcore) (Apache-2.0), vendored in [heinzboehmer/MK60-M-Core-Disassembly-Process](https://github.com/heinzboehmer/MK60-M-Core-Disassembly-Process) as `tools/ghidra-mcore`, installed by `tools/install_mcore.sh` with `tools/patches/mcore.patch`. Language id `MCORE:BE:32:default`. Upstream: [NSA/ghidra#9270](https://github.com/NationalSecurityAgency/ghidra/pull/9270) (not in a released Ghidra as of this note). | **Yes.** A recovery of this image with that module and Heinz’s `tools/ghidra/mcore_import.sh`, `mcore_recover.sh` and `mcore_verify.sh` (repo commit `6ea098d`) produced **1585** functions and verified HEALTHY. |
| GNU binutils `mcore` / `mcore-elf-objdump` | `opcodes/mcore-dis.c` and `mcore-opc.h`, still in binutils 2.45, configured with `--target=mcore-elf`. | The opcode map matches the listings used here. A target-specific `mcore-elf-objdump` was not installed on the analysis host (the distro package is x86_64-only), so coverage was not measured with objdump itself. |
| IDA processor module | [MotoFanRu/M-CORE_IDA-Pro](https://github.com/MotoFanRu/M-CORE_IDA-Pro) (IDA 8.3 / 9.0) and [MotoFanRu/IDA_Decompiler_M-CORE](https://github.com/MotoFanRu/IDA_Decompiler_M-CORE). | Not installed, not tested on this image. |
| Binary Ninja | Native M·CORE; 6.0 “Krypton” fixed big-endian recognition ([release notes](https://binary.ninja/2026/09/03/binary-ninja-6.0-krypton.html)). | Not installed, not tested on this image. |

The nam3forum thread [Flashing the MK60 with ZCP software](https://nam3forum.com/forums/forum/special-interests/coding-tuning/172815-flashing-the-mk60-with-zcp-software) (pages 5–6) describes Ghidra plus the patched CUB3D module on MK60e5 and points at Heinz’s repo. No other public SLEIGH / IDA / BN module aimed at this ECU family turned up.

## Coverage on the 818.3 dump (measured)

The dump is **262 144 bytes** (`0x0`–`0x3FFFF`). The window `0x0`–`0x5FFFF` is 384 KiB; **`0x40000`–`0x5FFFF` is not in this file** (128 KiB missing). RAM on the ECU is at `0x400000`, not `0x40000`. The Ghidra recovery of this image places code at `0x200`–`0x3C910` and data at `0x3CCF6`–`0x40000`.

How measured: a 2-byte linear sweep with a 16-bit M·CORE disassembler that follows the same opcode map as GNU `opcodes/mcore-dis.c`; function entries, bounds and instruction counts from a Ghidra export of function signatures for this image (1585 functions); names ported from the ZCP image. A naive sweep treats in-line literal pools as failed instructions (they decode as `.half`, often the two halves of a `0x0040xxxx` pointer), so function-guided figures are the ones to use.

| Measurement | Result |
|---|---|
| Functions recovered (Ghidra, verify HEALTHY, Heinz pipeline at commit `6ea098d`) | **1585** |
| Exact relocation-masked signature match to the 817.3 ZCP (from the existing port; not re-counted for this note) | **1527 / 1585 (96 %)** |
| Code-window halfwords inside some function’s `entry … next-entry` span | **123 784 / 123 784 (100 %)** of `0x200`–`0x3C910` |
| Code-window halfwords inside signature bodies (`nins × 2`) | **106 164 / 123 784 (85.77 %)**. The rest is inter-function padding, literal pools, and embedded tables. |
| Inside those signature bodies, halfwords that decode as a real instruction | **104 535 / 106 745 (97.93 %)**; **2 210** `.half` (pools). **161 / 1585** functions have at least one `.half` in the `nins × 2` span. |
| Naive linear sweep of the code window (pools counted as failures) | **117 494 / 123 784 (94.92 %)** |
| Naive linear sweep of the whole 256 KiB file | **123 198 / 131 072 (93.99 %)** |
| Non-generic names in the Ghidra recovery | A first ZCP signature port left **51** user-named functions (50 ports plus `reset`), plus seven generic `thunk_FUN_*` stubs (the earlier **58 / 1585** figure counted those thunks). A later naming pass brought user-named functions to **113 / 1585 (7.1 %)** and added 61 RAM / table labels. A further pass over the CAN layer, traction control, and driver-mode inputs brought that to **219 / 1585 (13.8 %)** named functions and 109 RAM / table labels; see `e46_m3_mk60_818_3_function_names.md`. |

“Cleanly into functions” means the HEALTHY Ghidra recovery (1585 functions; 85.77 % of the code window inside signature bodies). It is not a claim that most of the image is understood.

User-named functions on this recovery are now 219 of 1585, listed with confidence tags in `e46_m3_mk60_818_3_function_names.md`. That set includes the five driver-mode predicates, `driver_mode_get` (`0x21AE8`), `yaw_os_us_bars_update` (`0xE844`), `r_ref_eval` (`0x29606`), `r_est_eval` (`0x26258`), `interp_linear_xy` (`0x181F8`), `is_mdm_flag_write` (`0x3B080`), `tcs_driveslip_curve` (`0x3518C`, a name ported from the ZCP image), the other mode-4 consumers in `e46_m3_mk60_mtrack_mode4.md`, and the CAN-layer / TCS names in `e46_m3_mk60_can_map.md` and `e46_m3_mk60_traction_control.md`. Heinz `zcp_m_track_mode.md` documents the ZCP / non-M twins of the mode-4 consumers. Most of the 1585 recovered functions are still generic `FUN_` names.
