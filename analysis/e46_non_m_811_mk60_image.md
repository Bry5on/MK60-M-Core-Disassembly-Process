# e46_non_m_811_mk60_image Analysis

2026-11-18

`images/811.3_e46_non_m_mk60_full_dump.bin` (the dump handed in as `projects/811.3_e46_non_m_mk60_full_dump.bin`, part-number prefix 811.3) is an earlier build of the same Continental Teves MK60 firmware family as the three E46 images already documented (the 820.3 non-M in `nonm_mk60_image.md`, the 818.3 M3 in `e46_m3_non_zcp_mk60_image.md`, the 817.3 ZCP in `zcp_mk60_image.md`): the same chip family at a smaller flash fill (232 KB of data on a 256 KB part), the same flat program with no loader, the same memory map, the same factory K-line protocol with the same command table and the same KWP service table, and the same arithmetic and driver leaves. But it is far from all three: only 53% of its functions are instruction-identical to any sibling and those hold 19% of its instruction mass, against 85% to 96% and 59% to 92% among the three later builds. The control software, the diagnostic stack, the fault memory and the driver-mode logic were rewritten between this build and the 820.3. In particular this image has no table-driven driver-mode state machine and no mode 4: the DSC button runs a hand-written timer routine (0x204A0) with an on/off flag, its fault codes are single bytes rather than the 20-bit words of the later builds, and its coding image has two records rather than three. The dump is complete: data ends at 0x399D0, the chip is erased to 0x3A000, and no pool word points past the end. Project: `projects/811.3_e46_non_m_mk60` (project `E46_nonM_811_mk60`, program `811.3_e46_non_m_mk60_full_dump.cpu.hex`), recovered with the pipeline at commit 2e24e47.

## What the image is

A complete flash read of the first 0x3A000 bytes of the chip mapped at address 0, holding one flat program; the read stops at the end of the last written 4 KB sector. There is no separate loader region and no second image.

| Property | 811.3 non-M (this image) | 820.3 non-M | 818.3 M3 | 817.3 M3 ZCP |
|---|---|---|---|---|
| Size | 237,568 bytes (0x3A000): data to 0x399CF, erased 0xFF from 0x399D0 | 262,144, 1,332 bytes of 0xFF at 0x3FACC | 262,144, 788 bytes of 0xFF at 0x3FCEC | 266,240 (0x41000), erased from 0x40320 |
| Completeness | no pool value in 0x3A000..0x3FFFF; the values above 0x40000 are the shared bit-mask constants | complete | complete | a first 256 KB read was truncated |
| Vector table | at 0x0; reset 0x128E0 (one 369-instruction reset-and-init function), stubs 0x127CC..0x1287C (default 0x12870 in 79 slots), ISRs to 0x1720B | reset 0x14DE0 then init 0x14E70, stubs 0x14CC8..0x14DBC, ISRs to 0x17D13 | reset 0x14E60, ISRs to 0x17D93 | reset 0x14F90, ISRs to 0x17EE7 |
| Code / data boundary | code to 0x36ED0, call-target islands to 0x372B6, data from 0x372B6 | code to 0x3CB60, data from 0x3CF46 | code to 0x3C910, data from 0x3CCF6 | code to 0x3CDD0, data from 0x3D1B6 |
| Resident loader | none; no flash-writing code, no copy to RAM | none | none | none |
| CPU | M·CORE, big-endian; RAM at 0x40xxxx (highest address used 0x401800); bit-0 tag on code pointers | same | same | same |
| Peripheral blocks touched | 0xEFF000, 0xFF8000, 0xFFF000, 0xEF8000, 0xE60000; the same ten discovered MMIO pages | same | same | same |
| Identification | no `AZ1...` block, no seven-digit number string | same | same | same |
| Calibration names | `ASTA11F5.DFT` from 0x372DF, then `ASTY`, `ASTO`, `ASTB`, `ASPY1681.XC3` at 0x380B1, `ASTC`, `ASTS`, `ASTW`, `ASTT11F5.DFT` at 0x398C2 | `APT?27F1.DFT`, `APPY2782.KC8` | `AUT?24F1.DFT`, `AUPY2681.ZC5` | `AWT?26F1.DFT` / `AWPY3381.ZC4` |
| Functions after recovery | 1164 (370 prologue-seeded), verify HEALTHY in 3 rounds | 1577 (416) | 1585 (409) | 1670 (410) |
| Instructions | 97,619 | 107,135 | 106,745 | 107,265 |

The recovery converged in three rounds (first-pass work 494, 15, 0). The verifier reports 0 on every gate, 0 of 195,232 code bytes owned by no function, 0 bad of 245 table pointers and 762 code-referenced table bases. The dataset-name scheme is the siblings' (`AS` family, revision `11F5`, control dataset `ASPY1681.XC3` with the twelve-byte stamp `90 00 0E 80 07 06 01 0A 09 5D 03 00`), so the earlier quick look simply missed the `11F5` suffix. Reset and application init are one function here (0x128E0, 369 instructions), as in the ZCP, where the 820.3 splits them in two.

## How it relates to the three E46 siblings

| Measure | vs 820.3 non-M | vs 818.3 M3 | vs 817.3 ZCP |
|---|---|---|---|
| Functions with an identical relocation-masked signature | 612 of 1164 (53%) | 614 (53%) | 617 (53%) |
| Instruction mass inside identical functions | 18,953 of 97,619 (19%) | 20% | 20% |
| Large functions (20+ instructions) with an identical counterpart | 284 of 735 | 283 | 284 |
| Sibling functions with no identical counterpart here | 897 of 1577 (550 large) | 900 (549) | 981 (552) |
| 16-byte shingles of the raw file found in the sibling | 20,406 of 116,598 (18%) | 18% | 18% |
| Raw bytes that differ over the 232 KB | 231,104 | 231,263 | 231,343 |

632 functions are identical to at least one sibling (596 to all three, 13 to the 820.3 only, 20 to the M3 builds only); they are the leaves: the arithmetic library, the valve and pump drivers, the serial and SPI helpers, the factory-protocol helpers, the ASIC status reader, the input copy and button getters, `wheel_post_frame`, `kwp_10`. Everything above them was rewritten. Of the 451 large functions with no identical counterpart in the 820.3, 174 share under 10% of their 6-grams with their best 820.3 partner and 93 under 30%; only 45 are above 90%. By 8 KB of address space the control region below 0x12000 differs by 92% to 100% of its mass, the protocol and init region 0x16000..0x1DFFF by 32% to 66% (the helpers are shared, the state machines are not), the control subtrees above 0x1E000 by 70% to 90%, and only the top 8 KB (the ROM tables' readers and the SPI layer) is identical. The identical twins sit at many different offsets (0x5C8C, 0xAD8, 0xF20, 0x16874 and more), so the source layout was reordered as well.

The largest functions and their nearest 820.3 partners show where the rewrite is: 0x89B8 (3877 instructions, the partner of the 820.3's 4776-instruction 0xA920, 4%), 0x6BD0 (3039, 2%), 0xE926 (1218, 13%), 0x105A8 (1159, 6%), 0x25C52 (1142, the lateral estimator's partner 0x26B68, 13%), 0xC264 (1134, the observer 0xE3AC, 12%), 0x32C (1069, 3%), 0x290CC (872, 16%), 0x11534 (814, the wheel-side state 0x13E36, 2%), 0x2F034 (812, the traction entry, 3%). The ten largest unnamed functions are these plus 0xCEE4 (852) and 0x5B8C (885).

## What changed between the 811.3 and the 820.3 non-M

Both projects were decompiled whole (read-only, on snapshot copies) and the routines below were paired through identical twins, pool constants and call position, then diffed with addresses, temporaries and labels normalised. Grouped by subsystem:

**Factory protocol: same machine, same commands, moved RAM, one version byte.** The main cycle 0x1A200 (57 instructions, the 820.3's 0x1ACA4), the framed-mode handler 0x1A432 and the class handlers 0x1A816 (0xA0), 0x1A660 (0xA3), 0x1A928 (0xA4/0xA5) decompile to the same statements as the 820.3's: the same thirteen-state jump table (0x1A2AC, entries 0x1A2B0..0x1AB28), the same knock and selection, the same class and sub-command set (`0`, `1`, `3`, `4` behind 0xA0; the level machine behind 0xA3; `Z`, `M`, `O`, `K`, `L`, `Q` behind 0xA4/0xA5; `T`, `W`, `G`, `U`, `V`, `X`, `R` and the pulse commands behind 0xA6), the same reply builder 0x1AD16 with the 0xE0 status, the same idle timeout, the same 130 ms window rule. The differences the diffs show: the request buffer is two bytes shorter, so the hex field limits are 0x12/0x24 bytes here against 0x13/0x26, and the long-frame limit 0x25 against 0x27; the 820.3 accepts three extra sub-commands `H`, `I`, `J` under class 0xA6 (a copy of the request into the reply) that this build does not have; the idle counter 0x1AF12 here has no "window-armed caps the countdown at 5" clause (820.3 bit 5 of 0x4006FA); the receive service 0x1B2A6, the frame copy 0x1B1A8 and the long-frame install 0x1B214 are the same logic written slightly differently (the frame copy here recurses once to re-arm the dispatch pointer instead of storing `mfg_cmd_dispatch` into the descriptor). The command table at 0x381B0 has the same 30 slots with slots 5 and 28 empty and every populated slot the twin of the 820.3 entry. The UART records (nine at 0x380D4), the baud divisors (0x38062) and the mask table (0x38074) are the 820.3's bytes with the driver pointers moved (0x1AC40/0x1AC3C). The protocol-version bytes behind `0xA0` `3` are **`0C 00 00 00`** at ROM 0x380CE (the 820.3 has `0F 00 00 00` at 0x3DB64), read by the same `mfg_memcpy(4, …)` in the framed handler; the two bytes before them are `02 00` here and `02 01` there. So a tester that keys on the version reply sees a different value from this build.

**Factory protocol RAM on this unit.** Window counter 0x400510 and flags 0x4004FA (bit 0 restart request, bit 1 window flag, bit 2 window armed; the restart-request helper 0x16254 is the identical twin and reads 0x400E6B bit 3), state byte 0x400660 (values 9, 0xB, 10 as in the 820.3), session block 0x400648 (mode byte 0x400649 with the level in the low three bits, flags 0x40064A, countdown 0x40064B, pointer 0x400654), request buffer 0x40053B (class 0x40053C, sub-command 0x40053D, hex fields 0x40053E and 0x400546), reply buffer 0x400513 (selection byte 0x400515), session start 0x1A1E0 (called from the application init 0x16138), session reset 0x1949A, frame check 0x1AC74, hex parser 0x1ADDC, hex pair parser 0x1AEA8, pulse set 0x195D0 (pattern words 0x400614), pump level 0x18D32 (control word 0x40066C), NVM request 0x13ADA (block id 0x400E6C), job status 0x13C84, request arbiter 0x1DD6C, status getter 0x1DFE6 (0x4007CE), reduced test mode 0x174B6. The address directory that `mfg_addr_directory` 0x1B310 exposes (ROM 0x19770) lists the moved objects: 0x38074, 0x400614, 0x400606, 0x400622, 0x4005B2, 0x40067C, 0x400602, 0x368D8, 0x400604, 0x1B0AE, 0x4004FC, 0x4005FE (820.3: 0x3DB2E, 0x4006C4, 0x4006B6, 0x4006D2, 0x400662, 0x40072C, 0x4006B2, 0x3C564, 0x4006B4, 0x1BBA6, 0x400DD4, 0x4006AE).

**The protocol ROM block is laid out differently.** In the 820.3 the block runs divisors, masks, the length-prefixed control dataset name (`0C 'APPY2782.KC8'`, which is why the three bytes after the masks read `APP` there, `AUP` in the 818.3, `AWP` in the ZCP: they are the first three letters of that name, not a separate prefix field), the stamp, the version word, the UART records. Here the block inserts, between the masks and the name, twelve bytes `04 0C 03 04 03 03 10 04 02 00 00 00` and the three coding-image descriptors (`00400 4DA 73 09 00 02`, `0040 04E4 77 14 09 02`, `0040 0E73 7B 01 1D 02`: RAM address, length, two version bytes) that the 820.3 keeps at the end of its calibration tail; the 820.3's tail-side copy of those twelve bytes is `00 05 0F 03 04 03 03 03`.

**Driver-mode logic: no state machine, no table, no mode 4.** None of the 820.3's mode routines has a counterpart here: not the per-channel state machine 0x2174E, the table lookup 0x21654 over the 22-row transition table at 0x3E4B4, the channel reader 0x219DC, the validity check, the stuck-button monitor 0x21AC8, the row-enable 0x21B54, the pair setter, the publisher, nor the predicates `driver_mode_is_on`, `_is_off`, `_is_mdm` or `driver_mode_get`; the fuzzy pass finds nothing above 12% for any of them, and the image has no 32-byte-row table of the transition-table shape anywhere. What the DSC button drives instead is 0x204A0 (214 instructions), called once per tick from the dispatcher 0x20D1C with the argument `(0x400374 & 3) != 0`: it reads the button through 0x208AC (the identical getter 0x1658E clamped to 0 or 1), keeps a press counter in 0x4007E0 (u16), a debounce countdown in 0x4007E4 (reloaded with 0x15 = 21 ticks) and a flag word in 0x4007D4 whose top byte holds the state bits; it recognises a press after 21 ticks, a long hold at 0x167 (359) ticks and at 0x597 (1431) ticks, and saturates at 30000; a fault state (0x4004FE bits 3 and 4 with bit 0 of 0x4007DC) blocks the button. Its two helpers decide the outcome: 0x2064C sets the request bits in 0x4007DC (bit 0 "DSC off" after a short press when the vehicle conditions in 0x400BB1 and 0x400374 allow, bit 1 "back on", bit 6 after the long hold, bit 4 a forced-on state that also posts the fault 0x5B through 0x14120 when the button is held more than 0x33 ticks while 0x400DC0 says the power state is 2), and 0x20738 turns them into the lamp pattern 0x4007D8 (ids 1, 2, 4 selected by the sequence byte 0x4007E2: 1, 2, 4, 8, 0x20, 0x40) that the dispatcher posts through the arbiter 0x1DD6C as ids 10 and 12. There is no third position: the KWP SID 0x21 handler 0x2E3FC sets bit 3 of its status byte 0x400519 when `(0x4007DC & 0x41) == 0` (DSC on) and clears it otherwise, where the 820.3 sets it when the mode is on or 4. Whether the off state is reached by a short press or by the 359-tick hold depends on the 0x400BB1/0x400374 gates, which this document does not decode further. The coding bit the 820.3 uses to choose the two- or three-position behaviour (bit 6 of its coding byte 3, getter 0x3B782) has as its nearest match the getter 0x2D24C (50%), which 0x204A0 and 0x2064C do consult: with it set the long-hold branch is taken differently (bit 6 against bit 1 of 0x4007DC).

**Power-state routine and restart request.** 0x17F04 (436 instructions) is the same procedure as the 820.3's 0x18A64 (the ASIC bytes 0x400594..0x40059D in place of 0x400631..0x40063F, the 0xE8 threshold on 0x40059D, the 0x24/0x25/0x26/4 states of 0x40058F, the 30-count and 7-count timers, the 0x12 reload, the 0xD3/0xE4 thresholds on 0x4005A0, the 0x75..0x8A window on 0x4005A6), but its state bytes are packed differently: it keeps its flags in 0x400E32..0x400E36 (820.3 0x4011BA..0x4011BE) and writes its status bits into 0x400E33 (bits 1..7) where the 820.3 writes 0x4011A5 (bits 0..6); the bit positions are shifted by one or two throughout, and the fault it posts is the byte code 0x3F through 0x14120 and the pair 0x86/0x87 through 0x1418E, where the 820.3 posts 0x80002 and tests `fault_latched`(0x80008), (0x80010). The restart path is the 820.3's: the 10 ms body 0x17446 calls the restart helper 0x16254 (identical: bit 0 of 0x4004FA unless bit 3 of 0x400E6B), then the re-init 0x15C7A, which runs the 551-instruction initialisation 0x15C9A, the application init 0x16138, checks the NVM status (0x13F5A, bits 2..3), clears through 0x36EDC, sets bit 5 of 0x400594 and parks in `br .` at 0x15C98. The one difference in the re-init: the 820.3's 0x166F0 first fills 0x400000..0x4002FF with the 0xA5A5A5A5 pattern; this build does not.

**Fault memory: byte codes, not 20-bit words.** The fault poster 0x14120 (the 820.3's 0x3213C by position and callers) takes a one-byte code: the latch array is 0x400DC4, indexed by `code >> 3` with bit `code & 7`, and the pending mask 0x400DF0 gets bit `code >> 3`; the 820.3's poster takes `index << 16 | mask` over the word array 0x400DE4 and the 20-bit codes 0x80002, 0x91000 and so on. Codes seen: 0x3F and 0x86/0x87 (power state), 0x5B (button held in power state 2). The consumers 0x1418E (the test the 820.3 calls `fault_latched`) and the fault manager under the 10 ms body are this build's own. This affects every fault-code table in the data section (below).

**Coding image: two records.** The seal routine 0x29D50 (the 820.3's 0x3B566 by callers and shape) checks two NVM records, 0x80 and 0x84, against the descriptors at 0x38EA4 (`0040 08BE 80 12 00 02`: RAM 0x4008BE, length 0x12) and 0x38EAC (`0040 08CC 84 04 0E 02`: RAM 0x4008CC, length 0x0E) and keeps its state in 0x4009DD; the 820.3 checks three records 0x7B, 0x73, 0x77 over 0x400FE0/0x400FFC/0x401004 and keeps 0x400FE1. The three descriptors in the protocol block (0x4004DA/0x73, 0x4004E4/0x77, 0x400E73/0x7B) are a second set under the 820.3's record ids; which of the two sets the KWP SID 0x2E writer uses here was not traced.

**Initialisation and scheduling.** The application init 0x16138 reads one ADC calibration word (0x7F8010 into 0x4005DC) where the 820.3 reads two (0x7F8010 and 0xFF8010 into 0x40068C/0x40068E with a per-variant offset table); the 10 ms body 0x17446 has the same skeleton (the restart test, the re-init, the ASIC status reader 0x1634C, the factory cycle 0x1A200 when bit 0 of 0x40064A is set, else the diagnostic scheduler 0x2DBEC, then 0x13860 and 0x13618) with the 820.3's five extra calls of 0x1BEAE/0x1BEB0/0x1BEB2/0x1BEC0 (trace hooks) absent; the timer ISR 0x17058 is the 820.3's single ISR with one more phase reload (0xFB); the dispatcher 0x20D1C calls 20 control functions where the 820.3 calls 25 and has the driver-mode block replaced by the single call of 0x204A0.

**KWP.** The service table at 0x3911C has the 820.3's seventeen records with identical SID, length, mask and level bytes; the dispatcher 0x2DE78 is 97% the 820.3's (the same seven record checks through 0x13C84, the same 0x21/0x23 timers, the KWP RAM moved: request SID 0x400EA2, sub-function 0x400EA3, length 0x400EA6, NRC 0x400EA7, session byte 0x400EBF, security level 0x400EC0; 820.3 0x40124B, 0x40124C, 0x40124F, 0x401250, 0x40125C, 0x40125D); `kwp_10` 0x2E070 is identical, so `10 87` is the only accepted session sub-function and there is no download, reset or programming hand-off. The SID 0x21 handler 0x2E3FC differs in the status bits above and reports a 0x0C-byte block for local id 4 where the 820.3 reports 0x14 bytes. `10 85` occurs twice at odd offsets inside instructions (0x9A47, 0x2C199); `AA 00 8C` and `AZ1` not at all.

**CAN.** The twelve message records at 0x3836A carry the same identifiers (0x153, 0x1F0, 0x1F3 transmitted; 0x316, 0x329, 0x1F5, 0x43F, 0x43D, 0x235, 0x615, 0x613, 0x610 received) with the same lengths and periods; six receive records differ in their last byte (one signal fewer each from record 6 on). The signal table at 0x3853C has the same 8-byte entry shape (descriptor, getter); its getters are this build's own functions (0x1D15C.., 0x1CE20..), so the signal contents were not compared.

**Calibration tail.** The data section (0x372B6..0x399D0, 10,010 bytes) has the siblings' structure: nine dataset names in the same order and inline patterns, the protocol block, the KWP table, the CAN tables, and after the last name a final block opening with the same `fe 0c 01 f4 00 03 02 21` header. That block is 258 bytes here (0x398CE..0x399CF) against 616 in the 820.3: it holds the three 22-byte speed curves (one value differs: 0x076C against 0x0578 in the second curve), two 0x3E80-header curve records (both identical here; the 820.3's second one differs), the `0096 0003 005A 010E …` record, the `05DC 0003 07D0 1388 01F8` record, the `07D0 0003 2166 2710 0096 0003 FFFF` record and the check word `C0D9 5599`. It lacks everything the 820.3 added for traction control: the five rows of eight slip thresholds, the five gain rows, the 0x7530-header curve records, the 0x1388/0x03E8 records, the ff1a byte table, the twelve-entry 0x0040 table, the coding-checksum byte table and the coding descriptors. The 0x7530 pairs the ZCP indexes by mode do not exist here in any form.

## Why it is the same lineage

| This image | M3 counterpart | Match |
|---|---|---|
| 0x22A2A `wheel_post_frame` | 0x8F028 | 83% fuzzy (ported from the 820.3 at 1.00) |
| 0x1AE00 `mfg_memcpy` | 0x7738C | 79% fuzzy |
| 0x1AFE8 `mfg_banner_send` | 0x77690 | 75% fuzzy |
| 0x17560 `curve_interp_s16` | 0x710FC | 73% fuzzy |
| 0x1AD70 `mfg_hex_format` | 0x772FC | 72% fuzzy |
| 0x200AC `outlet_valve_level` | 0x819A8 | 71% fuzzy |
| 0x127F0 `vec_misaligned_access` | 0x6CA18 | 71% fuzzy |
| 0x20058 `inlet_valve_level` | 0x818E6 | 70% fuzzy |
| 0x2DC40 `kwp_valve_pump_test_drive` | 0xB0154 | 69% fuzzy |
| 0x17650 `interp_linear_xy` | 0x7119E | 67% fuzzy |
| 0x214E4 `tyre_tol_normalise` | 0xD0480 | 65% fuzzy |
| 0x2002C `valve_flow` | 0x818B6 | 65% fuzzy |
| 0x1B322 `mfg_cmd_dispatch` | 0x779D6 | 64% fuzzy |
| 0x30C2E `tcs_yaw_slip_term` | 0xC9450 | 61% fuzzy (ported from the 820.3 at 0.77) |
| 0x32C9E `tcs_cap_eval` | 0xCB576 | 84% fuzzy (new here, from the M3) |
| `mfg_hex_byte`, `pump_circuit_active`, `cluster_sensor_get` and 124 more short leaves | same | exact signature |

The factory protocol helpers, the hydraulics driver layer and the arithmetic library are shared with the whole family; the control, estimation and diagnostic stacks are this build's own.

## How far it is from the E5

| Measure | Result (820.3 in brackets) |
|---|---|
| Functions with an identical signature in the M3 | 127 of 1164 (11%) [270 of 1576, 17%] |
| Identical functions of 20 or more instructions | 7 [9] |
| Instruction mass inside identical functions | 875 of 97,619 (1%) [1%] |
| Large functions with under 10% 6-gram overlap with any M3 function | 571 of 735 [613 of 834] |
| Large functions with 60% or better overlap | 34 [40] |

Further from the E5 than its siblings, as it is further from them; the seven identical large functions are the usual leaves (0x1ADAC `mfg_hex_byte`, 0x1D9A0 `cluster_sensor_get`, 0x20164 `pump_circuit_active`, 0x23CE4, 0x2744E, 0x27524, 0x2DD44).

## CCP is not compiled in

None of the M3's CCP routines has a counterpart: every `ccp_*` function including `ccp_command_dispatch`, `ccp_dto_0x798_build`, `ccp_dnload_write`, `ccp_upload_read`, `ccp_session_reset` and `ccp_cro_rx_0797_handler` is at 0% to 10% 6-gram containment in this image, while the manufacturing routines of the same code base match at 61% to 100% (`mfg_hex_byte`, `mfg_rx_buf_end`, `mfg_addr_directory`, `mfg_nvm_wait_enter` at 100%). The image has no 36-entry command pointer table (its pointer tables of 20 or more entries are the 64 vectors at 0x100, 24 at 0x17C88 and the 30-slot command table at 0x381B0), no `AZ1` station identifier, and no counterpart to `cfg_flag_toggle_ccp_mode` or `kwp_routine_31_ccp_mode`. With no resident loader either, reprogramming this unit would have to come from outside.

## Names carried over

39 names are in the project, all applied by `ApplyPortedNames.java` with a plate comment naming the method. With `reset` and four auto-named thunks the project has 44 non-generic functions.

| Source and method | Names | Which |
|---|---|---|
| 820.3 non-M, exact signature | 30 | `vec_misaligned_access` 0x127F0, `wheel_edge_count_get` 0x13DEC, `curve_interp_s16` 0x17560, `abs_sat16`, `clamp_s16`, `interp_linear_xy`, `iir_blend_q8`, `uart_rx_byte_pending` 0x1AC4C, `mfg_hex_format`, `mfg_hex_byte`, `mfg_reply_send` 0x1AE30, `mfg_banner_send`, `mfg_rx_descriptor` 0x1B0BA, `mfg_rx_buf_end`, `mfg_addr_directory` 0x1B310, `mfg_cmd_dispatch` 0x1B322, `pedal_at_most` 0x1D24A, `cluster_sensor_get` 0x1D9A0, `fcan_rx_0D1_q1` 0x1DBAE, `fcan_rx_0CD_q0` 0x1DC12, `valve_flow`, `inlet_valve_level`, `outlet_valve_level`, `pump_circuit_active`, `clamp_s16_0D0444`, `tyre_tol_normalise`, `clamp_sym16`, `clamp_sym16_091A26`, `kwp_valve_pump_test_drive` 0x2DC40, `kwp_09_proprietary` 0x2E266 |
| 820.3 non-M, call-site propagation | 3 | `uart_flags_get` 0x12FC6, `mfg_memcpy` 0x1AE00, `ecu_mode_ge` 0x1B0AE |
| 818.3 M3, exact signature | 2 | `mfg_rx_xform_byte` 0x1B0F6, `mfg_rx_decode_frame` 0x1B10E (the 818.3 carries them from the ZCP's call-site propagation) |
| ZCP, exact signature | 0 (32 already placed) | — |
| E9x M3, exact signature | 1 (16 already placed, 10 source-ambiguous) | `coa_init_wheel_vol_offsets` 0x1DD34 |
| Fuzzy, 6-gram containment at 60% or better, unique and mutual | 3 | `wheel_post_frame` 0x22A2A (1.00 from the 820.3), `tcs_yaw_slip_term` 0x30C2E (0.77 from the 820.3), `tcs_cap_eval` 0x32C9E (0.84 from the M3) |

`mfg_rx_service` did not cross: this build's 0x1B2A6 is the same logic written differently (27%). Near misses below the floor, for hand application:

| Function | Instructions | Best candidate | Score | What it is here |
|---|---|---|---|---|
| 0x199A4 | 31 | `coding_image_checksum` (818.3) | 54% | the coding checksum beside the seal 0x29D50's tables |
| 0x22938 | 36 | `wheel_minmax_select` (M3) | 52% | just below `wheel_post_frame` |
| 0x165BA | 21 | `dsc_button_input_get` (M3) | 50% | the byte 1 bit 1 switch getter (the DSC button is 0x1658E) |
| 0x22F7C | 236 | `wheel_speed_filter` (820.3) | 50% | the wheel-speed filter, rewritten |
| 0x32E00 | 52 | `tcs_torque_intervention_flag` (M3) | 49% | traction cluster |
| 0x174B6 | 23 | `task3_10ms_testmode_cycle` (M3) | 44% | the reduced test-mode task: the role matches |
| 0x24394 | 71 | `steer_yaw_residual_update` (M3) | 43% | lateral branch |
| 0x2FDA0 | 124 | `tcs_entry_gate` (M3) | 39% | traction entry |
| 0x1B2A6 | 53 | `mfg_rx_service` (820.3) | 27% | the receive service: the role matches (called from the main cycle and the class handlers with 0x82, 0x84, 0x80) |
| 0x1AF12 | 38 | `mfg_idle_timer_tick` (M3) | 18% | the idle count: the role matches |

## Open questions

- [x] Completeness: data to 0x399D0, erased to 0x3A000, no pool word past the end.
- [x] Whether this image has a mode 4: no; the DSC button runs the timer routine 0x204A0 with on/off request bits, and no mode variable, table or predicate exists.
- [ ] What the button's outcome gates in 0x400BB1 and 0x400374 are (vehicle speed or state conditions), and whether the 359-tick hold or the short press is the "off" path on a coded car: the two helpers 0x2064C and 0x20738 are the place to read.
- [ ] The byte fault-code map: 0x3F, 0x5B, 0x86, 0x87 are seen; the latch array 0x400DC4 holds up to 26 bytes of codes; the fault manager and the KWP SID 0x18 handler 0x2E694 would give the code-to-DTC map.
- [ ] Which coding record set is live: the protocol block's three descriptors (record ids 0x73, 0x77, 0x7B) or the seal's two (0x80, 0x84).
- [ ] The CAN signal layout of 0x153: the records match the siblings', the getters are this build's own.
- [ ] Part number and variant: no identification block; `AS` family, `11F5`, `ASPY1681.XC3`, the stamp and the version word `0C` are the only markers.
- [ ] The polarity of the ASIC status bit and the restart edge: inherited from the 820.3 reading; the helper is identical.

## Method and provenance

Completeness check before anything else, as recorded in PROCESS.md: every `lrw` in the code region resolved to its pool slot, the slot values in [0x3A000, 0x400000) listed; none fell in [0x3A000, 0x40000), the rest were the shared bit-mask constants. Recovery: `tools/ghidra/mcore_import.sh` with raw `.bin` input at `BASE=0`, then `mcore_recover.sh` (three rounds, first-pass work 494, 15, 0) and `mcore_verify.sh` (HEALTHY), at commit 2e24e47; no pipeline change was needed for the 0x3A000-byte image. Name porting: `mcore_port.sh` from `projects/820.3_E46_non_m_mk60`, then `projects/818.3_e46_m3_mk60` (project `E46_M3_nonZCP_mk60`, program `e46_m3_non_zcp_mk60_full_dump.cpu.hex`, as renamed by the user), then `projects/817.3_e46_m3_zcp_mk60`, then `projects/E9x_M3_7846816A`, then `mcore_fuzzy_port.sh` with the four as sources in that order; work files in `projects/811.3_e46_non_m_mk60/port_work/`. The identity and distance figures come from the `ExportSigs` and `DumpMasked` outputs of that port; the comparison with the 820.3 from whole-program decompilations of both projects (one-off script on snapshot copies, `analyzeHeadless -noanalysis -readOnly`), paired by identical twins and pool constants and diffed with addresses, temporaries and labels normalised; the tables from byte search and comparison of the raw files; the re-init and restart listings from the same dump. No project other than this image's own was modified; `projects/` is its own repository and was not committed to. Companion documents: `nonm_mk60_image.md` (the 820.3, whose command-class table applies here), `e46_m3_non_zcp_mk60_image.md`, `zcp_mk60_image.md`, `non_zcp_vs_zcp_differences.md`, `zcp_m_track_mode.md`.
