# zcp_mk60_image Analysis

2026-11-17 · Claude Doc: https://claude.ai/artifact/DK83vCNyaBfAtxmYs6JvNF

`images/e46_m3_zcp_mk60_full_dump.bin` (the dump handed in as `projects/817.3_e46_m3_zcp_mk60_full_dump.bin`) is the E46 M3 ZCP build of the same Continental Teves MK60 firmware as the non-M E46 image in `nonm_mk60_image.md`: the same 256 KB chip, the same flat program with no loader, the same factory K-line protocol instruction for instruction, the same memory map, and 59% of its instruction mass inside functions that are identical to a non-M function. What differs is the control software: about seventy large functions under the 10 ms control dispatcher were restructured, the calibration tail carries a different dataset family (`AWT?26F1.DFT` against `APT?27F1.DFT`), and the ZCP data section runs past the end of the 256 KB dump: a first 256 KB read was truncated, and the complete 266,240-byte (0x41000) read of 2026-11-17 holds the last calibration block at 0x40000..0x4031F with the chip erased from 0x40320 (see the calibration tail). Against the E9x M3 it stands exactly where the non-M stands: library leaves, actuator drivers and the factory protocol match, the control and diagnostic stacks do not, and CCP is absent. Project: `projects/817.3_e46_m3_zcp_mk60` (project `E46_M3_ZCP_mk60`, program `e46_m3_zcp_mk60_full_dump.cpu.hex`), recovered with the pipeline at commit cf0f68b.

## What the image is

A complete 256 KB flash read mapped at address 0, holding one flat program. There is no separate loader region and no second image.

| Property | ZCP dump (this image) | Non-M dump | E9x M3 7846816A |
|---|---|---|---|
| Size | 266,240 bytes (0x41000): data to 0x4031F, erased 0xFF from 0x40320; a first 256 KB read was truncated and was replaced on 2026-11-17 | 262,144 bytes, 1,332 bytes of 0xFF at 0x3FACC | about 2 MB `.0pa`, application 748 KB at 0x48000 |
| Vector table | at 0x0; reset 0x14F90, exception stubs 0x14E70..0x14F64 (default 0x14F14 in 79 slots), ISRs to 0x17EE7 | at 0x0; reset 0x14DE0, stubs 0x14CC8..0x14DBC, ISRs to 0x17D13 | application vectors at 0x48000, loader below |
| Code / data boundary | code to 0x3CDD0, call-target islands to 0x3D1B6, data from 0x3D1B6 | code to 0x3CB60, data from 0x3CF46 | code to about 0xD7000, calibration above |
| Resident loader | none: reset clears both register files and runs straight into application init (one 449-instruction function 0x14F90), no flash-writing code, no copy to RAM | none (reset 0x14DE0 then init 0x14E70, the same code split in two) | yes, 0x0..0x47FFF |
| CPU | M·CORE, big-endian; RAM at 0x40xxxx; bit-0 tag on code pointers | same | same |
| Peripheral blocks touched | 0xEFF000, 0xFF8000, 0xFFF000, 0xEF8000, 0xE60000; the ten discovered MMIO pages are the non-M's ten | 0xEFF000, 0xFF8000, 0xFFF000, 0xEF8000, 0xE60000 | 0xD80000, 0xDB0000, 0xE10000, 0xFA0000, 0xFC0000 |
| Identification | no `AZ1...` block, no seven-digit number string | same | `AZ1RAE00008` block at 0xF68FC |
| Calibration names | `AWTA26F1.DFT` from 0x3D1DF, then `AWTY`, `AWTO`, `AWTB24F1`, `AWPY3381.ZC4`, `AWTC`, `AWTS`, `NOPARADJ.UST`, `AWTT33F1.DFT` at 0x3FFF4 | `APTA27F1.DFT` from 0x3CF6F, `APPY2782.KC8`, `APTW27F1.DFT`, `APTT27F1.DFT` at 0x3F854 | Teves dataset names in the calibration section |
| Functions after recovery | 1670 (410 prologue-seeded), verify HEALTHY in 3 rounds | 1577 (416 prologue-seeded) | 3608 |
| Instructions | 107,265 | 107,135 | about 260,000 |

The reset handler at 0x14F90 reads the PSR, clears the interrupt bit, zeroes r1 to r15, loads the stack pointer from the pool word 0x15344, re-enables the bit and clears the register file a second time, the same startup as the non-M's 0x14DE0; the recovery kept it and the application init that follows as one function here because nothing else enters the init. The verifier reports 0 on every gate (error bookmarks, mis-disassembled tables, code bytes in the data section, orphan runs, undefined prologues, call targets not code, pool slots in code) and 192 of 214,720 code bytes owned by no function.

## How it relates to the non-M image

The two are consecutive builds of one program on one chip, not two variants of a platform: 1427 of the 1670 ZCP functions (85%) have a non-M function with the identical relocation-masked instruction stream, and those functions hold 59% of all instructions. Every differing function has a positional partner of about the same size at the corresponding place in the non-M image, so the source layout did not change either; what changed is the body of the control algorithms.

| Measure | Result |
|---|---|
| ZCP functions with an identical signature in the non-M | 1427 of 1670 (85%) |
| Instruction mass inside identical functions | 63,196 of 107,265 (59%) |
| Large functions (20+ instructions) with an identical counterpart | 645 of 836 |
| Large functions with no identical counterpart | 191: 89 at 60% or better 6-gram containment in their partner (small edits), 29 at 30% to 60% (reworked), 73 under 30% (restructured, 41 of them under 10%) |
| Non-M functions with no identical ZCP counterpart | 231 of 1577 (189 large) |
| Identical by signature in the ZCP but at a different address | all of them: code shifts by 0x1B0 at the reset vector, 0x218 in the factory protocol, 0xE74 in the KWP handlers; the data section starts 0x270 later |
| 16-byte shingles of the raw file found in the other image | 65,873 of 129,157 (51%); 1,943 in the E9x M3 (1.5%) |

**Where the differences are.** Grouping the non-identical instruction mass by 8 KB of address space: the factory protocol and session layer at 0x1A000..0x1BFFF differs by 0% and 2%, the initialisation at 0x16000..0x17FFF by 2%, the arithmetic library at 0x18000 by 15%, the CAN layer at 0x34000 by 8%, the top of the code at 0x3A000..0x3CDD0 by 10% and 1%, the KWP handlers from 0x31C20 up by 0%. The rest of the differing mass sits in a handful of subtrees, all reached from the 10 ms body 0x18116 (the non-M's 0x17F42, identical) through the control dispatcher 0x210F8 (130 instructions, 99% contained in the non-M's 0x20D58) or through `wheel_post_frame` 0x23F5A. Named by the ported names nearest to them:

| Subtree (ZCP) | Entered from | Functions restructured (instructions, 6-gram containment in the partner) | RAM pages | What it is |
|---|---|---|---|---|
| 0x660..0x14000, the region below the vector handlers | 0x210F8 via 0x140C, 0x163C via `wheel_post_frame`, 0x3CA8 via 0x14C0 | 0xA8E4 (4528, 15%), 0x8278 (1475, 17%), 0x451A (353, 3%), 0x61C4 (257, 3%), 0x9488 (201, 4%), 0x1D24 (305, 26%); 0x660 (1118) and 0x6614 (597) reworked at 61% and 51% | 0x401000..0x4011FF, 0x400600, 0x400300 | the largest control functions of the image; the biggest function of either image, 0xA8E4, is the partner of the non-M's 0xA920 (4776 instructions) and shares 15% of its 6-grams |
| 0xD2A4 branch | 0x210F8 → 0xD2A4 → 0xD2E4 / 0xE834 / 0xFFFC | 0xD98C (794, 10%), 0xE070 (778, 6%), 0xE844 (1112, 14%), 0xF4B0 (847, 8%), 0x107AC (1178, 17%), 0x12438 (1027, 6%), 0x11E28 (391, 12%), 0xFD08 (293, 11%), 0xD3C8 (130, 2%) | 0x400300, 0x400400, 0x400900 | an observer and estimation block over the 0x400300..0x4004FF state; the non-M doc's weak `ayc_obs_wrapper` hit (0xD58C) lies in this branch |
| 0x13630 branch | 0x210F8 → 0x13630 → 0x13F0E | 0x13F0E (814, 11%), 0x14990 (143, 30%), 0x14C7C (112, 34%) | 0x400500, 0x400900 | wheel-speed side state (0x400500 holds the raw wheel speeds 0x4005B2..0x4005B8) |
| 0x1C16C branch | the 10 ms body | 0x1CA78 (584, 20%), 0x1C1F0 (153, 7%) | 0x400700, 0x400E00 | runs beside the fault area 0x400E24..0x400E38 |
| actuator drivers | 0x210F8 → 0x1F5D8 | 0x1F9C0 (276, 6%), 0x20998 (185, 6%) | 0x400700, 0x400800 | two functions below `valve_flow` 0x20814, `inlet_valve_level` 0x20840, `outlet_valve_level` 0x2089A, `pump_circuit_active` 0x20940 (all four identical to the non-M) |
| mode logic | 0x210F8 | 0x21FE0 (271, 1%) | 0x400800 | new in the ZCP: its positional partner in the non-M, 0x20838, is 17 instructions; sits with `driver_mode_is_on` 0x20BCC, `driver_mode_get` 0x21D2C |
| wheel control | `wheel_post_frame` → 0x23ED4 | 0x2349E (1027, 40%) | 0x400900, 0x400600, 0x401000 | beside `wheel_speed_filter` 0x244B8 and 0x246C0 (608, identical) |
| lateral estimation | `wheel_post_frame` → 0x25F70 | 0x27220 (1769, 10%) | 0x400A00 | the one restructured function of a branch whose other four (0x28274, 0x28502, 0x2874E, 0x28CF8, 300 to 640 instructions) are 91% to 95% contained; near `ax_from_front_wheels` 0x2501C, `steer_offset_fault_monitor` 0x255F4 |
| 0x2C1A2 subtree | 0x210F8 → 0x2C15C | 0x2C8EC (558, 1%), 0x2EAA0 (459, 11%), 0x3075E (393, 30%), 0x2D290 (360, 13%), 0x30AE8 (250, 7%), 0x2CE0C (226, 3%), 0x2D65E (181, 3%), 0x2FF54 (179, 0%), 0x2C6A6 (134, 21%), 0x30DE0 (123, 9%), 0x2D7C8 (119, 0%); 0x2C1A2 itself (346) at 48% | 0x400B00..0x400DFF | the largest contiguous rewrite: eleven functions over a state block the factory and KWP layers never touch |
| traction and yaw control | 0x210F8 → 0x39004 → 0x38254 / 0x37BB8, and 0x3694A from `wheel_post_frame` | 0x38C20 (430, 24%), 0x36DAC (192, 19%), 0x37D34 (154, 7%), 0x382D8 (136, 16%), 0x3A676 (147, 14%), 0x36F2C (101, 3%); 0x3639C, 0x3552C, 0x36C08, 0x37494, 0x3781C reworked at 28% to 42% | 0x400E00, 0x400F00 | around `tcs_yaw_slip_term` 0x368B4 and `tcs_driveslip_curve` 0x35644; the M3 counterparts of 0x38492 and 0x3859C are in its `tcs_*` region (0xCB1A0, 0xCB3F8) |
| power-state routine | the ASIC status reader 0x16FBC | 0x18C38 (456 against 428, 4%) | 0x401100, 0x400600 | see below |

Register allocation is not what separates these: a register-blind containment (top byte of every halfword, 8-grams) lifts only 12 of the 73 restructured functions above 50%. They are the same procedures edited at source level, recompiled, and placed in the same order.

**The power-state routine** (ZCP 0x18C38, non-M 0x18A64, the one that raises the restart request) is the same procedure: its literal pool names the same objects in the same order (the status block 0x401182 and its base 0x40116C, the ASIC bytes 0x40062C and 0x40062B, 0x400628, the control word 0x400710, the pulse words 0x4006C4, the window flags 0x4005AE, the mode byte block 0x4006EC, the window counter 0x4005AC, the session reset 0x1A286 and the two 0xEF8200/0xEF8500 ports), it opens with the same two tests of bit 6 and bit 7 of the ASIC byte, and the mnemonic sequence aligns at 81%. The 28 extra instructions are re-loads of the 0x40116C block base from the pool where the non-M keeps it in a register. The restart-request mechanism of the non-M (bit 0 of the window flags raised on the ASIC edge, the 10 ms body re-running the initialisation through 0x168A0, the idle loop `br .` at 0x168D0) is therefore present; its edge polarity is not re-derived here and rests on the non-M reading.

**The calibration tail.** Both tails hold nine dataset names in the same order and the same inline structure (the `%^%^%^#`, `%*%*%*%*$`, `#M%d#M#` and `xlT\tHXHhH` patterns recur at the same relative places), 0x270 later in the ZCP. The families differ: `AWT?26F1` against `APT?27F1`, with the B set at revision `24F1` and the T set at `33F1` in the ZCP, the control dataset `AWPY3381.ZC4` at 0x3E031 against `APPY2782.KC8` at 0x3DB47, and the eighth name `NOPARADJ.UST` at 0x3F27A where the non-M has `APTW27F1.DFT`. The twelve bytes after the control dataset name, `90 00 0E 80 0D 02 03 0B 14 74 03 00` against `90 00 0E 72 1B 09 02 0F 30 70 03 00`, are the only bytes in the protocol table block that differ; they look like a date or revision stamp and are not decoded. **The dump is truncated.** In the non-M, 632 bytes of tables follow the last dataset name (0x3F860..0x3FAC8, then 1,332 bytes of 0xFF from 0x3FACC), and 24 pool words in the traction and yaw functions (0x35164, 0x354C0, 0x355C8, 0x36084, 0x36D50, 0x380D8, 0x38320, 0x38594, 0x38948, 0x39BF0, 0x3B4A0, 0x3B6FC and their neighbours) point into that block. In the ZCP the last dataset name `AWTT33F1.DFT` ends exactly at 0x40000 and the file stops; the same functions, at their shifted addresses (0x356E4, 0x35A40, 0x35B48, 0x3626C, 0x365FC, 0x37240, 0x37F90, 0x38404, 0x38514, 0x38804, 0x38BFC, 0x39E28, 0x3B714), carry pool words pointing to 0x40000..0x402F7. The ZCP's final table block therefore lay beyond the 256 KB read. The complete read supplied it: 0x320 bytes at 0x40000..0x4031F opening with the same structure as the non-M's block (`fe 0c 01 f4 00 03 02 21`), then erased 0xFF to 0x41000, so the ZCP chip is one 4 KB sector larger than the non-M's data needs. The project was rebuilt from the complete file (converged in three rounds, HEALTHY, 1670 functions, code to 0x3CDD0, data from 0x3D1B6, unchanged) and all thirteen pool words now resolve to labelled data in that block, which is the traction and yaw calibration the restructured functions read.

**Concrete differences a reader would check for, and the answer.**

- Protocol-version bytes behind `0xA0` `3`: the same `0F 00 00 00`, at ROM 0x3E04E in the ZCP (0x3DB64 in the non-M), read through the pool word 0x1B53C of the class handler 0x1B50A. Two bytes of `FF FF` padding follow it before the UART records, the only layout change in the block.
- UART records: nine 16-byte records at 0x3E054 (0x3DB68), byte for byte the same except the driver pointers (0x1B930 and 0x1B92C against 0x1B718 and 0x1B714): the same baud ids per digit, mode byte 0x08, gaps 0x14/0x16, 0xFA, 0x08.
- Baud divisors: the same words `0x5B 0x54 0x0F 0x08 0xB6 0x2E 0x1C 0x17 0x0E` at 0x3E006 (0x3DB1C), so the same SCI clock and the same 4800-or-9600 reading.
- Command table: 30 slots at 0x3E130 (0x3DC44), 28 populated, slots 5 and 28 empty in both, every populated slot the ZCP twin of the non-M entry (slot 0 `mfg_addr_directory` 0x1C016, slot 1 the valve pulse set 0x1A3BC, slot 2 the pump level set 0x19AE4, slots 7 and 8 the request arbiter 0x1E114 and the status getter 0x1E38E, slot 9 `mfg_reply_send` 0x1BB20, slot 26 the frame check 0x1B964, slot 27 the hex parser 0x1BACC). Class 0xA6 carries `M O K L Q` beside `Z` here too, since the framed handler is identical.
- KWP: the same 17-record service table (`{sid, len, 0, 0, handler, mask, level, 0, 0}`) at 0x3F358 (0x3EBB8) with the same SIDs, lengths, masks and levels; the dispatcher 0x31C20 and `kwp_10` 0x31E18 are identical to the non-M's 0x30DAC and 0x30FA4, so `10 87` is still the only accepted session sub-function and there is no download, reset or programming hand-off. The byte pair `10 85` occurs twice in the image, at the odd offsets 0x2EBFF and 0x2EC0D inside an instruction stream, and `AA 00 8C` does not occur. The KWP RAM block moved: request SID 0x401213, sub-function 0x401214, request length 0x401217, NRC 0x401218, session byte 0x401224, security level 0x401225 (non-M 0x40124B, 0x40124C, 0x40124F, 0x401250, 0x40125C, 0x40125D).
- Identification strings: none in either; the only text is the dataset names above.
- Timer interrupt: the non-M's one 31-instruction ISR 0x17B5C is split here into the phase selector 0x17D30 (jump table 0x17D60) and the phase-0 body 0x17D80, which makes the same six calls (the pool-slot reader 0x1C0C6, 0x3CD34, the three serial helpers 0x1A314, 0x1A4B0, 0x1A426, and 0x155A6, the only caller of the 10 ms body 0x18116); the non-M's six are 0x1BEAE, 0x3CAC4, 0x1A0FC, 0x1A298, 0x1A20E and 0x153F6.

### The factory protocol on this unit, address for address

Every function of the manufacturing protocol in the non-M doc has an identical twin here, so the state table, the knock, the selection byte, the 130 ms rule (and the absence of the E5's 70 ms rule), the state 5 wait, the idle timeout of 180, the access levels, the class and sub-command set and the bench recipe in `nonm_mk60_image.md` apply unchanged; only the addresses move. The RAM variables below are the pool words of those twins.

| Element | ZCP (this image) | Non-M | E9x M3 |
|---|---|---|---|
| State machine jump table | 0x1AF68, 13 entries 0x1AF6C..0x1B818 | 0x1AD50 | 0x760D8 |
| State byte | 0x400704 | 0x400710 | 0x400A28 |
| Main cycle | 0x1AEBC | 0x1ACA4 | `mfg_test_cycle` 0x76036 |
| Session start | 0x1AE9C, called only from the application init 0x16D7E | 0x1AC84 from 0x16BCE | `mfg_session_start` 0x75FD4 |
| Session reset | 0x1A286 | 0x1A06E | `mfg_session_reset` |
| Window counter / flags | 0x4005AC / 0x4005AE (bit 0 restart request, bit 1 window flag) | 0x4005BC / 0x4005BE | — |
| Restart-request helper, RAM clear, re-init, idle loop | 0x16EB8, 0x1688A, 0x168A0, `br .` at 0x168D0 | 0x16D08, 0x166DA, 0x166F0, 0x16720 | — |
| Power-state routine, ASIC status reader | 0x18C38, 0x16FBC (status byte block 0x40062B..0x40062D) | 0x18A64, 0x16E0C (0x400639..0x40063B) | — |
| Knock state 1, selection state 2 | 0x1AFA6, 0x1AFE0 | 0x1AD8E, 0x1ADC8 | `mfg_handshake_match` |
| Framed-mode handler (state 6) | 0x1B0EE | 0x1AED6 | — |
| Frame check | 0x1B964 | 0x1B74C | `mfg_frame_check` 0x7723E |
| Class 0xA0 handler | 0x1B50A (version word 0x3E04E) | 0x1B2F2 (0x3DB64) | — |
| Class 0xA3 access-level machine | 0x1B31C | 0x1B104 | — |
| Class 0xA4/0xA5 memory handler | 0x1B622 | 0x1B40A | — |
| Command dispatch, command table | `mfg_cmd_dispatch` 0x1C028, table 0x3E130 (30 slots, 28 populated) | 0x1BE10, 0x3DC44 | 0x779D6, 0xD7750 (26 populated) |
| UART records, baud divisors, mask table | 0x3E054, 0x3E006, 0x3E018 | 0x3DB68, 0x3DB1A, 0x3DB2E | 0xD7674 |
| Reply send, hex value parser, hex pair parser | `mfg_reply_send` 0x1BB20, 0x1BACC, 0x1BB98 | 0x1B908, 0x1B8B4, 0x1B980 | `mfg_reply_send` 0x77422 |
| Banner send, hex format, hex byte, memcpy | 0x1BCF8, 0x1BA60, 0x1BA9C, 0x1BAF0 | 0x1BAE0, 0x1B848, 0x1B884, 0x1B8D8 | 0x77690, 0x772FC, 0x77338, 0x7738C |
| ASIC SPI exchange | 0x1BB5E via 0x3C830 | 0x1B946 via 0x3C5BC | `mfg_qspi_raw` |
| Valve pulse set, pulse value decode, pump level set | 0x1A3BC (pattern words 0x4006B6), 0x1B84A, 0x19AE4 (control word 0x400714) | 0x1A1A4 (0x4006C4), 0x1B632, 0x198CC (0x400720) | — |
| NVM request, synchronous read, job queue, job status, NVM wait entry | 0x16172 (block id 0x4011BC), 0x164D6, 0x1608E, 0x16314 (table 0x401190), 0x1BBF6 | 0x15FC2 (0x4011F4), 0x16326, 0x15EDE, 0x16164 (0x4011C8), 0x1B9DE | `mfg_nvm_wait_enter` |
| Control reset, checkpoint-fail flag, mode request | 0x1BD38 (state words 0x400E24..0x400E30), 0x1BCBA, 0x190C0 via 0x3C668 | 0x1BB20 (0x400DD0..0x400DDC), 0x1BAA2, 0x18EB0 via 0x3C3F4 | `mfg_control_reset`, `op_mode_request` |
| Request descriptor, receive service, long-frame install, frame copy | 0x1BDCA (`mfg_rx_descriptor`, buffer 0x401060), 0x1BFB4, 0x1BF1A, 0x1BEB8 | 0x1BBB2 (0x40101C), 0x1BD9C, 0x1BD02, 0x1BCA0 | `mfg_rx_descriptor`, `mfg_rx_service` |
| Idle timer reload / count | 0x1BCA8, 0x1BC02 | 0x1BA90, 0x1B9EA | `mfg_idle_timer_*` |
| Request arbiter, three-byte status getter (slots 7, 8) | 0x1E114, 0x1E38E (0x40085C) | 0x1DE38, 0x1E0B2 (0x400866) | empty slots |
| Reduced test-mode task | 10 ms body 0x18116 switches to 0x1819E when `ecu_mode_ge`(3) (0x1BDBE) | 0x17F42 → 0x17FCA | same |

**Variables for the bench.** State 0x400704; flags 0x4006EE (bit 0 gates the cycle, bit 4 window armed); mode byte 0x4006ED (level in the low three bits); session flags 0x4005BE (bit 0 selection refusal, bit 1 bad command, bit 5 level 2); request buffer 0x4005E9, class byte 0x4005EA, sub-command 0x4005EB, hex fields 0x4005EC and 0x4005F4; reply buffer 0x4005BF; session block 0x4006F4 (pointer 0x4006F8); window counter 0x4005AC and flags 0x4005AE; raw wheel speeds 0x4005B2, 0x4005B4, 0x4005B8, 0x4005B6; fault latch array 0x400E38. Each is the non-M variable moved by the same offset as the identical function that reads it (0x4005xx and 0x4006xx by −0x10 and −0xC, 0x400Dxx by +0x54, 0x4011xx by −0x38); the ones not read directly through a pool word (flags, mode byte, class and sub-command bytes, fault array) are derived from the block base plus the non-M offset.

## Why it is the same lineage

The subsystems that change least across a product generation are present and recognisable, and they are the same ones the non-M doc found. Matching was done on relocation-masked instruction streams: exact FNV signatures through `tools/ghidra/mcore_port.sh`, then 6-gram containment through `tools/ghidra/mcore_fuzzy_port.sh` for functions of 20 or more instructions.

| ZCP image | M3 counterpart | Match |
|---|---|---|
| 0x23F5A `wheel_post_frame` | 0x8F028 | 84% fuzzy |
| 0x1BAF0 `mfg_memcpy` | 0x7738C | 79% fuzzy |
| 0x1BCF8 `mfg_banner_send` | 0x77690 | 75% fuzzy |
| 0x1825C `curve_interp_s16` | 0x710FC | 73% fuzzy |
| 0x1BA60 `mfg_hex_format` | 0x772FC | 72% fuzzy |
| 0x2089A `outlet_valve_level` | 0x819A8 | 71% fuzzy |
| 0x14E94 `vec_misaligned_access` | 0x6CA18 | 71% fuzzy |
| 0x20840 `inlet_valve_level` | 0x818E6 | 70% fuzzy |
| 0x35644 `tcs_driveslip_curve` | 0xC795E | 70% fuzzy (the one name the fuzzy pass applied here) |
| 0x319E4 `kwp_valve_pump_test_drive` | 0xB0154 | 69% fuzzy |
| 0x1834C `interp_linear_xy` | 0x7119E | 67% fuzzy |
| 0x22600 `tyre_tol_normalise` | 0xD0480 | 65% fuzzy |
| 0x20814 `valve_flow` | 0x818B6 | 65% fuzzy |
| 0x1C028 `mfg_cmd_dispatch` | 0x779D6 | 64% fuzzy |
| 0x255F4 `steer_offset_fault_monitor` | 0xBF868 | 62% fuzzy |
| 0x368B4 `tcs_yaw_slip_term` | 0xC9450 | 60% fuzzy |
| `mfg_hex_byte`, `pump_circuit_active`, `fault_latched`, `cluster_sensor_get`, `mfg_rx_buf_end`, `mfg_addr_directory`, `ecu_mode_ge`, `uart_rx_byte_pending`, `uart_flags_get`, `cluster_fault_any`, `driver_mode_get`, `wheel_edge_count_get`, `clamp_s16`, `clamp_sym16`, `abs_sat16`, `iir_blend_q8`, `pedal_at_most`, `fcan_rx_0D1_q1`, `fcan_rx_0CD_q0` and the rest of the 24 | same names | exact signature |

The factory K-line protocol of `mfg_test_cycle.md` is present with the same structure, the hydraulics driver layer is the same design, and the arithmetic library is shared. The fuzzy scores against the M3 are the non-M's to the percentage point, as they must be: the ZCP functions in this table are identical to their non-M twins.

## How far it is from the E5

| Measure | Result (non-M in brackets) |
|---|---|
| ZCP functions with an identical signature in the M3 | 356 of 1670 (21%), nearly all short leaves [270 of 1576, 17%] |
| Identical functions of 20 or more instructions | 11 [9] |
| Instruction mass inside identical functions | 1,478 of 107,265 (1%) [1%] |
| Large functions (20+ instructions) with under 10% 6-gram overlap with any M3 function | 603 of 836 [613 of 834] |
| Large functions with 60% or better overlap | 46 [40] |

The six extra identical functions are leaves (0x28A, 0x1A73A, 0x2909E, 0x29162, 0x29A40, 0x31AE8 and the like, 20 to 53 instructions, unnamed in the M3). The control algorithms, the CAN layer and the diagnostic stack are the rewrite the non-M doc describes; nothing in the ZCP moves it closer to the E5. The CAN-identifier byte scan is as inconclusive here as there.

## CCP is not compiled in

None of the M3's CCP routines has a counterpart: the best 6-gram containment of any `ccp_*` function in this image is 10% (`ccp_cmd_START_STOP`, against the unrelated 0x2A714) and 9% (`ccp_cmd_START_STOP_ALL`), every other one including `ccp_command_dispatch`, `ccp_dto_0x798_build`, `ccp_dnload_write`, `ccp_upload_read`, `ccp_session_reset` and `ccp_cro_rx_0797_handler` is at 0% to 2%, while the manufacturing routines of the same code base match at 61% to 100% (`mfg_hex_byte`, `mfg_rx_buf_end`, `mfg_addr_directory`, `mfg_nvm_wait_enter` at 100%). The image has no 36-entry command pointer table like the M3's at 0x7A720 (its pointer tables of 20 or more entries are the 64 vectors at 0x100, 20 at 0x170E0, 24 at 0x189B4, 255 at 0x2F4D0, 20 at 0x3DC0A and the 30-slot command table at 0x3E130), no `AZ1` station identifier, and no counterpart to `cfg_flag_toggle_ccp_mode` (best 2%) or `kwp_routine_31_ccp_mode` (0%). With no resident loader either, reprogramming this unit would have to come from outside.

## Names carried over

50 names are in the project after the rebuild from the complete dump, all applied by `ApplyPortedNames.java` with a plate comment naming the method. The non-M project was ported first so that its names win, then the M3, then the fuzzy pass over both, then the three hand names; the rebuild re-ran the same sequence and the M3 call-site propagation then reached three more through `mfg_rx_service` (`mfg_rx_xform_byte` 0x1BE06, `mfg_rx_decode_frame` 0x1BE1E) and `coding_image_seal` (`coding_image_checksum` 0x3B76C). The table below is the first port's breakdown.

| Source and method | Names | Which |
|---|---|---|
| Non-M, exact signature | 36 | `vec_misaligned_access` 0x14E94, `curve_interp_s16` 0x1825C, `abs_sat16`, `clamp_s16`, `interp_linear_xy`, `iir_blend_q8`, `wheel_edge_count_get` 0x1A988, `uart_rx_byte_pending` 0x1B93C, `mfg_hex_format`, `mfg_hex_byte`, `mfg_memcpy`, `mfg_banner_send`, `mfg_rx_buf_end`, `mfg_addr_directory`, `mfg_cmd_dispatch` 0x1C028, `pedal_at_most` 0x1DF96, `valve_flow`, `inlet_valve_level`, `outlet_valve_level`, `pump_circuit_active`, `driver_mode_is_on`, `_is_off`, `_is_mdm`, `clamp_s16_0D0444`, `tyre_tol_normalise`, `wheel_post_frame` 0x23F5A, `steer_offset_fault_monitor`, `clamp_sym16`, `clamp_sym16_091A26`, `kwp_valve_pump_test_drive` 0x319E4, `fault_latched`, `cluster_fault_any`, `cluster_sensor_get`, `fcan_rx_0D1_q1`, `fcan_rx_0CD_q0`, `tcs_yaw_slip_term` 0x368B4 |
| Non-M, call-site propagation | 3 | `uart_flags_get` 0x15686, `ecu_mode_ge` 0x1BDBE, `driver_mode_get` 0x21D2C |
| E9x M3, exact signature | 0 new (21 of its matches were already placed by the non-M port; 10 source-ambiguous) | — |
| E9x M3, call-site propagation | 4 | `mfg_reply_send` 0x1BB20, `mfg_rx_descriptor` 0x1BDCA, `wheel_speed_filter` 0x244B8, `ax_from_front_wheels_0C953C` 0x2501C |
| Fuzzy, 6-gram containment at 60% or better, unique and mutual best | 1 | `tcs_driveslip_curve` 0x35644, 70% of M3 0xC795E |

The fuzzy pass adds little between close siblings because the non-M carries only 39 names and all 39 ported exactly. Against the M3 the next candidates stop just under the floor; none was applied:

| ZCP function | Instructions | Best M3 candidate | Score | What it would be |
|---|---|---|---|---|
| 0x3201C | 47 | `kwp_09_proprietary` 0xB1686 | 60% (0.5957) | the `09` service handler, slot 15 of the KWP table (level 4) |
| 0x3B7AA | 24 | `coding_image_seal` 0xCFA10 | 58% | the coding-block seal writer |
| 0x1BFB4 | 49 | `mfg_rx_service` 0x77952 | 52% | the long-frame receive service the class 0xA6 `O` command runs |
| 0x23E8C | 36 | `wheel_minmax_select` 0x8EFB0 | 52% | wheel-speed min/max selection, just below `wheel_post_frame` |
| 0x17222 | 21 | `dsc_button_input_get` 0x6FEE4 | 50% | the DSC button input reader |
| 0x38860 | 52 | `tcs_torque_intervention_flag` 0xCB708 | 49% | the traction-control torque-intervention flag |

The first three were applied by hand on 2026-11-17: `kwp_09_proprietary` 0x3201C is confirmed as the handler the service table at 0x3F358 names for SID 0x09 (record 14, level 4), `mfg_rx_service` 0x1BFB4 by its call sites (the main cycle 0x1AEBC each tick, the class handler 0x1B622, the state handler 0x1B818), and `coding_image_seal` 0x3B7AA on the user's decision, its call sites 0x3BBB0 and 0x3B5E0 not yet read. The project carried 47 names after that, and 50 after the rebuild. Porting the ZCP back to the non-M by exact signature then carried those three and the four M3 call-site names into the non-M project, which now has 46; `tcs_driveslip_curve` did not cross because its non-M twin differs.

## Open questions

- [x] The missing tail: resolved on 2026-11-17 by the complete 0x41000-byte read; the project was rebuilt from it and the block at 0x40000..0x4031F is in.
- [ ] Which bus it speaks: the CAN receive dispatch and the identifiers it filters; `fcan_rx_0D1_q1` 0x35298 and `fcan_rx_0CD_q0` 0x352FC are the entry points.
- [ ] Which diagnostic protocol the K-line carries beside the factory protocol; the KWP table at 0x3F358 and the UART driver 0x1B930 are the place to start.
- [ ] Part number and variant: no identification block; the dataset family `AWT?26F1` / `AWPY3381.ZC4` and the twelve stamp bytes at 0x3E03D are the only variant markers found.
- [ ] What the restructured control subtrees do differently (the 0x2C1A2 subtree, the 0xD2A4 observer branch, the lateral estimator 0x27220, the new 0x21FE0): the ZCP and non-M partners are placed in the same order and share RAM, so a side-by-side decompilation of each pair is the direct route.
- [x] Whether the factory protocol, its timing and its 8N1 configuration hold here: they do, instruction for instruction, at the addresses in the table above.
- [ ] The polarity of the ASIC status bit (bit 7 of 0x40062D) and the edge the restart request fires on: inherited from the non-M reading, not re-derived.
- [x] The three near-miss names (`kwp_09_proprietary`, `mfg_rx_service`, `coding_image_seal`): applied by hand on 2026-11-17, see the names section; the call sites of `coding_image_seal` remain unread.

## Method and provenance

Recovery: `tools/ghidra/mcore_import.sh` with raw `.bin` input at `BASE=0`, then `mcore_recover.sh` (three rounds, first-pass work 523, 20, 0) and `mcore_verify.sh` (HEALTHY), at commit cf0f68b, which also added the fuzzy tooling. Format checks before import: size, vector table and reset target, prologue scan (410), 16-byte shingle overlap with both references, dataset-name search, in Python on the raw file. Name porting: `mcore_port.sh` from `projects/820.3_E46_non_m_mk60` then from `projects/E9x_M3_7846816A`, then `mcore_fuzzy_port.sh` with both as sources (`DumpMasked.java`, `tools/fuzzy_port.py`, `ApplyPortedNames.java`); the work files are in `projects/817.3_e46_m3_zcp_mk60/port_work/`. The overlap and cluster figures come from the `ExportSigs` and `DumpMasked` outputs of that port (identical signatures, 6-gram and register-blind 8-gram containment, call edges to the 10 ms body); the factory-protocol addresses from the exact-signature twins of the non-M functions and the pool words their bodies load, read from the raw image; the tables from byte search in the raw image with the function pointers masked; the power-state and reset listings from `tools/ghidra/dump.sh` on snapshot copies. No project was modified for this document. Companion documents: `nonm_mk60_image.md` (the non-M E46, whose command-class table applies here unchanged), `kwp_dispatch.md`, `mfg_test_cycle.md`, `program_request_record.md` (all E9x M3).
