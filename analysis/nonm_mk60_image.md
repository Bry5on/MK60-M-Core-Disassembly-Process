# nonm_mk60_image Analysis

2026-11-16 · Claude Doc: https://claude.ai/artifact/GDFEZgPfd85xvDaucA7vJn

`images/non_m_mk60_full_dump.bin` is a genuine Continental Teves MK60 firmware, an earlier generation of the same code base as the E9x MK60E5 images on a smaller chip, not a different family. The lineage shows in the factory K-line protocol, the valve and pump drivers and the wheel and yaw control vocabulary, all of which match the M3 image function by function; the distance shows in the size, the peripheral map, the flat single program with no loader, and the absence of CCP. Project: `projects/E46_nonM_mk60` (program `non_m_mk60_full_dump.cpu.hex`), recovered with the pipeline at commit 112614f.

## What the image is

A complete 256 KB flash read mapped at address 0, holding one flat program. There is no separate loader region and no second image.

| Property | Non-M dump | E9x M3 7846816A |
|---|---|---|
| Size | 262,144 bytes (0x40000), no blank sectors | about 2 MB `.0pa`, application 748 KB at 0x48000 |
| Vector table | at 0x0; reset 0x14DE0, exception stubs 0x14CC8..0x14DBC, ISRs to 0x17D13 | application vectors at 0x48000, loader below (not in the `.0pa`) |
| Code / data boundary | code to 0x3CB60, data from 0x3CF46 | code to about 0xD7000, calibration above |
| Resident loader | none: reset goes straight into application init, no flash-writing code, no copy to RAM | yes, 0x0..0x47FFF, reached through `10 85` and `jump_to_bootloader` |
| CPU | M·CORE, big-endian; RAM at 0x40xxxx; bit-0 tag on code pointers | same |
| Peripheral blocks touched | 0xEFF000, 0xFF8000, 0xFFF000, 0xEF8000, 0xE60000 | 0xD80000, 0xDB0000, 0xE10000, 0xFA0000, 0xFC0000 |
| Identification | no `AZ1...` block, no part number string found | `AZ1RAE00008` block at 0xF68FC |
| Calibration names | `APTA27F1.DFT`, `APTY27F1.DFT`, `APPY2782.KC8` and similar from 0x3CF6F | Teves dataset names in the calibration section |
| Functions after recovery | 1577 (416 prologue-seeded), verify HEALTHY | 3608 |
| Instructions | about 107,000 | about 260,000 |

The reset handler at 0x14DE0 reads the PSR, clears an interrupt bit, writes it back and zeroes r1 to r15, the canonical M·CORE startup. The earlier 4 KB file `non_m_mk60_dump.bin` was a RAM or stack snapshot (0xA5A5 fill, no code) and the 100 KB `non_m_mk60_partial_dump.bin` was the first part of this read; both are superseded.

## Why it is the same lineage

The subsystems that change least across a product generation are present and recognisable. Matching was done on relocation-masked instruction streams: exact FNV signatures through `tools/ghidra/mcore_port.sh`, then 6-gram containment for functions of 20 or more instructions.

| New image | M3 counterpart | Match |
|---|---|---|
| 0x238B2 `wheel_post_frame` | 0x8F028 | 84% fuzzy |
| 0x1B8D8 `mfg_memcpy` | 0x7738C | 78% fuzzy |
| 0x1BAE0 `mfg_banner_send` | 0x77690 | 75% fuzzy |
| 0x18088 `curve_interp_s16` | 0x710FC | 73% fuzzy |
| 0x1B848 `mfg_hex_format` | 0x772FC | 72% fuzzy |
| 0x2051C `outlet_valve_level` | 0x819A8 | 71% fuzzy |
| 0x14CEC `vec_misaligned_access` | 0x6CA18 | 70% fuzzy |
| 0x204C8 `inlet_valve_level` | 0x818E6 | 70% fuzzy |
| 0x30B70 `kwp_valve_pump_test_drive` | 0xB0154 | 69% fuzzy |
| 0x18178 `interp_linear_xy` | 0x7119E | 66% fuzzy |
| 0x21F5C `tyre_tol_normalise` | 0xD0480 | 65% fuzzy |
| 0x2049C `valve_flow` | 0x818B6 | 64% fuzzy |
| 0x1BE10 `mfg_cmd_dispatch` | 0x779D6 | 63% fuzzy |
| 0x24F3C `steer_offset_fault_monitor` | 0xBF868 | 61% fuzzy |
| 0x36366 `tcs_yaw_slip_term` | 0xC9450 | 60% fuzzy |
| `ecu_mode_ge`, `uart_rx_byte_pending`, `uart_flags_get`, `mfg_hex_byte`, `mfg_rx_buf_end`, `mfg_addr_directory`, `fault_latched`, `cluster_fault_any`, `cluster_sensor_get`, `pump_circuit_active`, `driver_mode_get`, `wheel_edge_count_get`, `clamp_s16`, `clamp_sym16`, `abs_sat16`, `iir_blend_q8`, `pedal_at_most`, `fcan_rx_0D1_q1`, `fcan_rx_0CD_q0` and five more | same names | exact signature (24 names) |

The factory K-line protocol documented in `mfg_test_cycle.md` is therefore present in this unit with the same structure, and the hydraulics driver layer and the ABS, traction and yaw-control building blocks are the same design. A different supplier's controller would carry none of this.

## How far it is from the E5

| Measure | Result |
|---|---|
| New functions with an identical signature in the M3 | 270 of 1576 (17%), nearly all short leaves |
| Identical functions of 20 or more instructions | 9 |
| Instruction mass inside identical functions | 1% |
| Large functions (20+ instructions) with under 10% 6-gram overlap with any M3 function | 613 of 834 |
| Large functions with 60% or better overlap | 40 |

The control algorithms, the CAN layer and the diagnostic stack were rewritten or heavily reworked between this unit and the E5; the architecture, calling conventions, factory interface and actuator drivers carried over. A byte-pattern scan for CAN identifiers (0x153, 0x1F0, 0x316, 0x329, 0x545 for an E46; 0x130, 0x0A8, 0x6F1 for an E9x; 0x797/0x798 for CCP), raw and shifted, was inconclusive in both images: these halfwords occur too often in code to be read as identifiers.

## The factory protocol on this unit

The E46 image runs the manufacturing protocol documented in `mfg_test_cycle.md` state for state. It re-arms on every ignition-on by re-booting itself, and the knock window is the same 130 ms.

| Element | E46 (this image) | E9x M3 |
|---|---|---|
| State machine | 13 states, jump table 0x1AD50, state byte 0x400710, cycle 0x1ACA4 | 13 states, table 0x760D8, state 0x400A28, `mfg_test_cycle` 0x76036 |
| Knock | `@` 0x01 0x02, same match logic, state 1 (0x1AD8E) | same (`mfg_handshake_match`) |
| Selection byte | `0 1 3 4 5 6 ; <` accepted with `j`, `2 7 8 9 :` refused with `n`, `c` means stay (0x1ADC8) | same |
| UART records | nine 16-byte records at 0x3DB68, same baud ids per digit, mode byte 0x08 (8N1), gap values 0x14/0x16, 0xFA, 0x08 | same layout at 0xD7674 |
| Baud divisors | 182, 91, 46, 28, 15, 8 ... at 0x3DB1A, each the E9x value divided by about 5.7 | 1024, 521, 260, 160, 87, 43 ... |
| Framed mode | classes 0xA0 to 0xA6, same checksum frame check (0x1B74C), reply tags `i r e f c` | same |
| Access levels | 0 to 4 in the low three bits of 0x4006F9 (`ecu_mode_ge` ported), level 1 at state 4, level 3 needs `16`, level 4 needs `33` | levels in the top three bits of 0x400A11 |
| Command table | 30 slots at 0x3DC44, 28 populated, `mfg_cmd_dispatch` (0x1BE10) | 30 slots at 0xD7750, 26 populated |
| 130 ms rule | window flag bit 1 of 0x4005BE, counter 0x4005BC > 13, state < 3 resets to state 9 | same |
| State 5 wait | counter > 40 | same |
| Reduced test-mode task | 10 ms body 0x17F42 switches to 0x17FCA when `ecu_mode_ge(3)` | same |

The divisor ratio is the one hardware tell: the baud set is the same and only the SCI clock differs, so the default is still about 4800 or about 9600 baud under the two clock readings in `mfg_test_cycle.md`.

**How the window re-arms: a software re-boot at ignition-on.** The session start (0x1AC84) is called only from the application initialisation 0x16BCE, and the window counter 0x4005BC is only ever incremented. What makes the window recur is the 10 ms body: every tick it tests a restart-request bit (bit 0 of 0x4005BE, helper 0x16D08) and, when set, zeroes RAM 0x400300..0x401800 (0x166DA) and re-runs the whole initialisation through 0x166F0, which arms the protocol, refills the stack with 0xA5A5 and parks in `br .` at 0x16720. That loop is the normal idle loop: the cold boot (reset vector 0x14DE0, then 0x14E70 and 0x15CDC) ends in an identical one, the image has no `wait`, `doze` or `stop` instruction, and all work runs from the timer interrupt 0x17B5C, whose phase 0 calls the scheduler and the 10 ms body. So after the restart the tick, the counter and the state machine are live, and the restart is a re-boot that skips the reset vector. The 4 KB `0xA5A5` file from the first dump attempt was a snapshot of that refilled stack.

The request bit is raised in the power-state routine 0x18A64 on the falling edge of bit 7 of 0x40063B, after that bit has been set for ten ticks. 0x40063B is part of a status block that 0x16E0C reads from the system ASIC over SPI, so the bit is a raw ASIC signal. Its sense is taken as "terminal 15 off": the E5 derives its hardware ignition input as the inverted ASIC status bit, the bench unit answers the knock after a KL15 rising edge, and re-arming a 130 ms window at ignition-off would serve no purpose. The restart therefore fires on the KL15 rising edge, about one tick after it, and the 130 ms budget runs from then plus the initialisation time. The ten-tick transition into the debounced state also calls the session reset (0x1A06E) when the protocol is not armed, which re-initialises the serial channel.

**Differences from the E5.**

- No 70 ms rule: the E5 resets the session when no knock byte has matched by counter 7; the E46 state-1 handler has no such check, so the only deadline is the 130 ms one.
- Level 2 is free: the E5 gates it behind `op_mode_request(8)`; the E46 sets bit 5 of the session flags 0x4005CE and moves on. Levels 3 and 4 call the mode-request helper 0x18EB0 with 3 or 0x33 before the `16` / `33` check, then mask interrupts and run the control reset 0x1BB20.
- Selection refusal reads bit 0 of 0x4005CE, which the session code derives from level < 3 and the vehicle-state bytes 0x400DD0 and 0x400DDC; the E5 reads the operating-mode manager. Same purpose, different source; the E46 semantics are not pinned.
- Idle timeout 180 instead of 125, decremented every eighth tick: about 14.4 s against 10 s (0x1BA90 reloads, 0x1B9EA counts).
- The serial pacing values are shifted left 2 instead of 4, so the gap timer runs on a different clock, consistent with the different peripheral map.
- Slots 7 and 8 of the command table, empty in the E5, hold a request-arbitration routine (0x1DE38, which the cycle also calls every tick with (0,1,0), the counterpart of the E5's `module_method_dispatch(0,2,0)`) and a three-byte status getter (0x1E0B2). Class 0xA6 carries sub-commands `M O K L Q` beside `Z`; class 0xA4 lacks the E5's `S` and `H`.

**Variables for the bench.** State 0x400710; flags 0x4006FA (bit 0 gates the cycle, bit 4 window armed, bit 1 set in state 0); mode byte 0x4006F9 (level in the low three bits); session flags 0x4005CE; request buffer 0x4005F9; reply buffer 0x4005CF; window counter 0x4005BC and flags 0x4005BE (bit 0 restart request, bit 1 window flag, bits 2 and 3 cleared at tick 2, bit 4 RAM-test wrap).

**Recipe.** Switch KL15 on and immediately stream `@`, 0x01, 0x02 and a selection digit with 15 to 20 ms between bytes, repeating every 50 ms or so for a few seconds; the one-byte-per-tick constraint of the E5 driver applies here too, since the receive path is the same design. Silence after that means the wrong baud (try 4800 and 9600) or the module did not see the KL15 edge on the ASIC-sensed pin.

## CCP is not compiled in

None of the M3's CCP routines (`ccp_command_dispatch`, `ccp_dto_0x798_build`, `ccp_dnload_write`, `ccp_upload_read`, `ccp_session_reset`, `ccp_cro_rx_0797_handler`, the command handlers) has a counterpart: the best 6-gram containment for any of them in the new image is 10%, most are at 0%, while the manufacturing protocol routines from the same code base match at 63% to 79%. The new image also has no 36-entry command pointer table like the M3's at 0x7A720, no `AZ1` station identifier for EXCHANGE_ID, and no counterpart to `cfg_flag_toggle_ccp_mode` or `kwp_routine_31_ccp_mode`. A CCP written in a different style would escape the instruction-level test, but the protocol's fixed layout and the fact that every other shared subsystem matches fuzzily make that unlikely. With no resident loader either, reprogramming this unit would have to come from outside.

## Names carried over

39 names are in the project: 24 by exact signature through `mcore_port.sh` and 15 by the fuzzy pass, each of the latter with a plate comment of the form `fuzzy 6-gram 84% of M3 wheel_post_frame @08f028 (proposed)`. The fuzzy pass is a Ghidra script that dumps the masked instruction stream of every function plus a Python scorer that emits the `addr<TAB>name<TAB>method` table `ApplyPortedNames.java` reads; it is not yet in the repository's tooling.

## Open questions

- [ ] Which bus it speaks: find the CAN receive dispatch and read the identifiers it filters; an E46 unit should show the PT-CAN set 0x153, 0x1F0, 0x316, 0x329, 0x545.
- [ ] Which diagnostic protocol the K-line carries: an E46 unit should speak DS2 with an XOR checksum rather than KWP; the UART driver and the `mfg_*` entry points are already named and are the place to start.
- [ ] Part number and variant: no identification block was found; the calibration tail at 0x3CF6F is the next place to look.
- [x] Whether the factory knock window, timing and 8N1 configuration documented for the E5 hold here: they do, with the differences listed in the factory-protocol section; the window re-arms at every KL15 rising edge through a software re-boot.
- [ ] The polarity of bit 7 of 0x40063B rests on the E5 cross-reference and the bench behaviour; a direct read of the ASIC status layout would close it.
- [ ] What the selection-byte refusal flag (bit 0 of 0x4005CE, from 0x400DD0 and 0x400DDC) represents.

## Method and provenance

Recovery: `tools/ghidra/mcore_import.sh` with raw `.bin` input at `BASE=0` (added at commit f2e3168), then `mcore_recover.sh` and `mcore_verify.sh` with the image-relative layout changes of commit 112614f (flash base and code floor derived from the image; MMIO pages discovered from pointer density; two `FixLiteralPools` fixes). Format checks before import: byte statistics per 8 KB block, prologue pair scan, pointer-target classification and string search in Python on the raw file. Matching: `ExportSigs.java` on both projects for exact signatures; a masked-halfword dump (same masking as `ExportSigs`: `lrw`/`jmpi`/`jsri` displacement and branch displacement dropped) and 6-gram set containment for the fuzzy pass, run read-only on snapshot copies. The loader finding is from the recovery session's walk of the vector table, reset path and flash-register references. Companion documents: `kwp_dispatch.md`, `mfg_test_cycle.md`, `program_request_record.md` (all E9x M3).
