# non_zcp_vs_zcp_differences Analysis

2026-11-17

Function-by-function differences between the E46 M3 non-ZCP image (`projects/e46_m3_non_zcp_mk60`, "this image" or N below) and the ZCP image (`projects/817.3_e46_m3_zcp_mk60`, Z). Both projects were decompiled whole, read-only, and every function that is not instruction-identical between the two was paired with its partner and diffed after normalising addresses, register names and labels, so that only source-level changes remain. The headline: of the 58 functions in this image that are not the ZCP's, 31 are the non-M's unchanged and 27 are edited once more; apart from the M track mode button logic, the behavioural changes are few and small (a handful of thresholds and gains in one control subtree, a temperature term in the pump driver, two fault-summary bits, a calibration-driven constant), and three of the apparent "new" functions are artefacts of how the recovery split one body. The M track mode button handling (third channel, 37-row table, sync routine 0x21FE0, fault 0xC2000) is one row below and is covered in `e46_m3_non_zcp_mk60_image.md` and `zcp_m_track_mode.md`.

## Summary

| Subsystem | Functions changed (N) | Nature of the change | This image's version |
|---|---|---|---|
| M track mode button logic (mode machine, table, sync) | 9 + 2 unique (0x217C0, 0x21CC0) | two channels, 28 rows, no sync routine, no 0xC2000; plus one factory-window guard the ZCP added to the state machine | the non-M's, see the image doc |
| 0x2BC34 control subtree (RAM 0x400B00..0x400DFF) | 13 unique | the same procedures with one RAM word inserted at 0x400B32, five constants changed (offset threshold 120→150, two gains ×5→×11, one factor 75%→50%, defaults 25→110), a parameter block moved from immediates into ROM at 0x3EBC4, one hysteresis routine simplified, one init call dropped; 0x2EA98 is one body where the ZCP project shows four recovery fragments | a third edit |
| fault manager (0x328EE branch) and `fault_latched` | 6 unique | two summary bits computed differently (0x400E01 bit 7, the four wheel bytes 0x4005BA..BD bit 6); otherwise identical logic | a third edit (two bits), rest recompiled |
| power-state routine 0x18AE4 | 1 (non-M) | the ZCP's 456-instruction version additionally maintains bit 7 of the status byte (N 0x40113D, Z 0x40116D) and reloads its block base from the pool; the restart request itself is unchanged | the non-M's |
| wheel-speed side state (0x13630 branch) | 13 (non-M) + 3 small | same procedures recompiled; the only source changes are one parameter (1024→1400 passed to 0x14B7C), one constant made calibration-driven in the ZCP (800 → table 0x3DDAC) and the ZCP reading its constants through individual pool words | the non-M's |
| actuator driver 0x1F834 | 1 unique | the ZCP adds a temperature-scaled pump-pressure increment (table 0x3E8DC); this image uses fixed 200/400 per tick | nearer the non-M's (77%) |
| dispatcher 0x20ED4 | 1 unique | identical call list minus the sync routine | the non-M's minus one call |
| KWP handlers SID 0x27, 0x31, 0x18 and 0x31F48 | 3 non-M + 1 unique | no behavioural difference found for 0x27 and 0x31 (recompiled); 0x31F48 adds a second accepted state (4 or 5) to one status byte; SID 0x18 recompiled | the non-M's / third edit |
| 0x3A1BE (under 0x3929C) | 1 unique | one curve: upper value 9500→10000, slope doubled | a third edit |
| 0x21074 (dispatcher child) | 1 (non-M) | the ZCP adds a third condition (bit 0 of 0x400530) to one request | the non-M's |
| arbiter table entry 0x1E120 | 1 (non-M) | identical logic; the ZCP project split off its 8-instruction tail as 0x1E2DA | the non-M's |
| channel-1 button getter 0x1711C | 1 (non-M) | identical logic; the ZCP's 0x1724C is a shorter compile | the non-M's |
| data section | 65 tables same length different values, 20 different length | 28-byte curve records differing in one word, pointer tables, the factory address directory, the KWP RAM layout, the fault attribute table, the 0x2BC34 parameter block, the ZCP-only pump temperature table | see the data section |

What matters most: (1) the ZCP's pump driver 0x1F9C0 scales the pressure build-up per tick by temperature; this image's 0x1F834 does not. (2) The 0x2BC34 subtree runs with different constants here, and the ZCP's version of its state init sets bit 0 of 0x400DCA, which this image never sets. (3) The ZCP's power-state routine keeps a bit-7 flag of the status byte that this image's non-M routine does not have; the restart-request path is otherwise the same code.

## 1. The 58 functions that are not the ZCP's, by subsystem

Addresses are this image's; the ZCP partner is in brackets. "Recompiled" means the decompiled bodies are the same statements and the instruction difference is register allocation or scheduling.

### 1.1 M track mode button logic

Covered in `e46_m3_non_zcp_mk60_image.md` ("What the ZCP build adds") and `zcp_m_track_mode.md`. In the pair diffs, the only behavioural difference not described there: the ZCP state machine 0x21AF2 (and its second entry 0x21CF0) applies a forced mode from the stored-mode byte only when the factory window flag (bit 1 of 0x4005AE) is clear; this image's 0x218BA / 0x21AAC (the non-M's) apply it unconditionally. The validity check 0x21B8C (Z 0x21DF0) reads the published pair of channel 0 only, where the ZCP's reads per channel. The row-enable 0x21CC0 writes 28 enables (Z 37) and the table lookup 0x217C0 loops over 28 rows. The two init helpers 0x21C02 and 0x21C24 fill two channels (Z 0x21E98, 0x21EB8: three). The ZCP's 4-instruction 0x21FCC (writes one row-enable byte) has no callers in either project.

### 1.2 The 0x2BC34 control subtree (Z 0x2C15C → 0x2C1A2)

The subtree keeps its structure; every function here calls the same callees in the same order. The RAM map of the subtree differs by one inserted 16-bit word: from 0x400B32 up, this image's words sit 4 bytes higher than the ZCP's (N 0x400B36/38/3A/3C/3E are Z 0x400B32/34/36/38/3A), which is why so many functions here are not instruction-identical. The changes with an effect:

| N | Z | What differs |
|---|---|---|
| 0x2BC7A (350) | 0x2C1A2 (346) | none: the body is identical statement for statement; the final call goes to 0x2EA98 here and to the first fragment 0x2EF90 there |
| 0x2C108 (67) | 0x2C624 (65) | none (a signed compare rewritten) |
| 0x2C3D4 (563) | 0x2C8EC (558) | the lateral-offset threshold returned by 0x2FBCA is subtracted above 150 here, above 120 in the ZCP (`if (0x96 < s) s -= 0x96` against 0x78); otherwise identical apart from the RAM shift |
| 0x2FBCA (31) | 0x300BA (23) | the threshold routine itself: returns 150 + min(90, abs(0x400D30)·90/150) here, 120 + min(60, abs(0x400D30)·60/150) in the ZCP (0x96/0x5A against 0x78/0x3C), for parameter 2 only |
| 0x2C908 (237) | 0x2CE0C (226) | the same procedure (the 7% 6-gram score is register allocation); one ordering change in the update of the counter byte 0x400B12: both clamp the sum at zero and skip the update when both 0x400B0F and 0x400B10 are set, this image writes the clamped value in one place |
| 0x2CDA0 (356) | 0x2D290 (360) | the ZCP sets bit 2 of 0x400B42 also when bit 1 of the power-state byte 0x40113C is clear; this image sets it only from the three mode queries (0x215FC(1), (3), (2)) |
| 0x2D162 (181, from the application init 0x2D138) | 0x2D65E | the state init: this image zeroes 0x400B38, 0x400B3C, 0x400B3A (the shifted words), the ZCP 0x400B34, 0x400B38, 0x400B32; the ZCP ends by calling 0x30F60, which sets bit 0 of 0x400DCA; this image has no such call and nothing else sets that bit |
| 0x2E5A0 (461) | 0x2EAA0 (459) | recompiled; identical statements |
| 0x2EA98 (550) | 0x2EF90 + 0x2F03C + 0x2F1FC + 0x2F394 | one body in both images: the ZCP project's three extra "functions" have no callers, begin after branches inside 0x2EF90's body (0x2F03A `br 0x2F072`) and use registers set by it; the recovery split the ZCP body at mid-body literal pools. The sizes agree (1100 bytes against 448+320+172+158 = 1098). The body builds the subtree's diagnostic record 0x400D5A..0x400D7F from the slip estimates; the statement sets differ only in that record (this image fills 0x400D7D from 0x400B12·100/50, the ZCP fills 0x400D7A and 0x400D7F) and in the ZCP's use of three more small helpers (0x2FE38, 0x2FE90, 0x2FED0, 0x2FEF0) where this image has the arithmetic inline |
| 0x2FA5C (183) | 0x2FF54 (179) | two factors: the rate comparison uses `(a−b)·50/100` here and `(a−b)·75/100` in the ZCP (0x32 against 0x4B), both directions |
| 0x3061C (264) | 0x30AE8 (250) | the four calls to 0x30840 pass the gain `0x400D1E·11` and `(&0x400CC6)[0x400D1B]·11` here, `·5` in the ZCP; two range checks rewritten unsigned |
| 0x3092A (132) | 0x30DE0 (123) | the four default gains are 110 (0x6E) here and 25 (0x19) in the ZCP, and the two products are divided by 11 here and by 5 there; the table test over 0x3EC14 is the same |
| 0x30A4E (28) | 0x30EE8 (57) | simplified: this image buckets the value by 50 and fires 0x2D3E4 once when the bucket steps up by one below 3; the ZCP keeps a hysteresis bit in 0x400D9E with four thresholds (0x30, 0x35, 0x62, 0x66) and two firing windows |

The constants this subtree reads from ROM also moved: this image keeps a 16-word parameter block at 0x3EBC4 (50, 90, 50, 150, 70, 13000, 16500, 2, 150, 127, −127, 25, 10, 10, 10, 10 …) that its functions address word by word; the ZCP has those values as immediates and its 0x3F248 table is sixteen words of 300 where this image's 0x3EC14 is the same sixteen words of 300 (the table test in 0x3092A is unchanged).

### 1.3 The fault manager (10 ms body → 0x328EE → 0x32FC8)

| N | Z | What differs |
|---|---|---|
| 0x33140 (571) | 0x335FC | recompiled, identical statements |
| 0x33640 (573) | 0x33AF8 | recompiled, identical statements |
| 0x32D12 (190) | 0x331D2 | recompiled, identical statements |
| 0x32FD6 (181) | 0x33496 | the summary bit 7 of 0x400E01: here it mirrors bit 0 of 0x400E26 (bit 7 cleared, then set to bit 0 of 0x400E26); in the ZCP it is set when any of 0x400E26 & 0x3F7F is set and never cleared in this routine |
| 0x33CAC (275) | 0x34168 | at the end, the ZCP copies bits 4..7 of 0x400E3E into bit 6 of the four wheel status bytes 0x4005BA..0x4005BD (one fault bit per wheel); this image clears bit 6 of all four |
| `fault_latched` 0x32B48 (18) | 0x33004 (20) | the same lookup over the latch array (0x400E08 here, 0x400E38 there); the ZCP has one more instruction |

### 1.4 The power-state routine

0x18AE4 (428 instructions) is the non-M's 0x18A64 by signature; the ZCP's 0x18C38 has 456. The normalised diff shows the same procedure: the same reads of the ASIC bytes (0x400626, 0x400627, 0x40062B, 0x40062E, 0x40062F), the same fault posts 0x80002 through 0x32AD8 and `fault_latched` 0x80008/0x80010, the same counters in 0x401152..0x401156, the same window test on 0x4005AC and the same call of the session reset 0x1A0FA when the 10-count expires with bit 1 of 0x40113C set and bit 0 of 0x4006EA clear. The ZCP adds exactly one piece of state: bit 7 of the status byte (N 0x40113D, Z 0x40116D) is cleared when bit 0 of 0x401152 is set at entry and set when the ASIC byte 0x40062B has bit 6 set or 0x401155 is negative, the same condition that sets bit 0 of 0x401152. Nothing in this image's routine touches that bit. The rest of the 28 extra instructions are re-loads of the 0x40116C block base from the pool (the pool listing shows 0x40116C fifteen times against this image's single 0x40113C). The restart request (bit 0 of 0x4005AE) and its consumers are the same code in both images.

### 1.5 The wheel-speed side state (dispatcher → 0x13630)

All thirteen large functions here are the non-M's; the ZCP's are recompiles with four small source edits:

| N | Z | What differs |
|---|---|---|
| 0x13EBA (789) | 0x13F0E (814) | recompiled: the same flag assembly in 0x400534..0x400537, the same counters, the same three curve lookups (tables 0x3D8C8, 0x3D8E4, 0x3D900 here; 0x3DDD8, 0x3DDF4, 0x3DE24 there, identical contents), the same calls. The ZCP reads its constants through individual pool words (hence the many 2-byte "tables" at 0x3DCFC..0x3DE42 in the ZCP inventory) where this image uses block offsets |
| 0x14CC0 (28) | 0x14DD0 (31) | the call of 0x14B7C passes 0x578 (1400) here and 0x400 (1024) in the ZCP as its fourth argument |
| 0x148C0 (138) | 0x14990 (143) | for parameter 3 the ZCP reads the base value from the table 0x3DDAC; this image uses the constant 800; the +2000 offset for parameter 2 is the same in both (written as +3000 −1000 in the ZCP) |
| 0x14690 (42) | 0x14758 (41) | calls 0x149D4 here and 0x14AAE there: the same 22-instruction routine (`0x40059E −= (0x40056A − 5)·0x83 >> 8` when 0x40056A > 4), compiled two ways |
| 0x13724, 0x137F6, 0x13898, 0x13A40, 0x13CA0, 0x146E4, 0x14B7C, 0x14C4E | 0x13724, 0x1380C, 0x138B4, 0x13A6C, 0x13BF0, 0x147AA, 0x14C7C, 0x14D5C | recompiled; 0x14B7C's four curve tables are the same contents at 0x3D83C.. here and 0x3DD02.. there |
| 0x13ACC (9), 0x13ADE (11), 0x149D4 (22) | 0x13AFE (11), 0x13B14 (14), 0x14AAE (23) | fragments of the branch's state selection (write 0, 2 or 3 into the mode byte and call 0x13724 on change); the ZCP's 0x13AFE adds a speed test (`0x400936 < 0x708` selects 2) that this image's fragment does not have; none of the three has a caller in either project (jump-table pieces) |

### 1.6 The actuator driver

0x1F834 (207) against the ZCP's 0x1F9C0 (276), both under 0x1F44C from the dispatcher, both over the two pump circuits (`pump_circuit_active(i)` for i = 0, 1) and the pressure words 0x400824/0x40082C/0x400830/0x400838. The ZCP computes a per-tick increment from temperature: it takes 0x4008DE (or 20 when bit 0 of 0x400DFE is set), and scales 200 through the table 0x3E8DC (`0064 0064 FFEC FFFD 0023 0046 0000 61A8`: breakpoints −20, −3, 35, 70) when the temperature is below −3 or above 69; this image adds a fixed 200 per tick (400 on the first tick) with no temperature term, as the non-M does. The table 0x3E8DC has no counterpart in this image. The rest of the routine (the 0x3E196 / 0x3E1AA interpolation, the 0x4007BA scaling by 14, the split between the two pressure words) is the same.

### 1.7 KWP handlers and the dispatcher

| N | Z | What differs |
|---|---|---|
| SID 0x27 handler 0x31A82 (105, non-M) | 0x31F4A (102) | no difference in the decompiled body (seed compare through 0x30F7C / 0x31A34, level test against 0x4011F5) |
| SID 0x31 handler 0x3233C (125, non-M) | 0x327FC (125) | no difference (routine ids 0x23, 0x25, 0x26 through 0x3B158(3), 0x2D428 / 0x2D6C4) |
| SID 0x18 handler 0x31FE0 (22) | 0x324A2 (33) | no difference in the decompiled body (count from 0x30C6A, reply through 0x30B90) |
| 0x31F48 (76, non-M) | 0x32410 (73) | a status reply of 0x14 bytes at 0x4005C2: byte 0x4005CD is 1 when the state getter 0x2D6F4 returns 4 **or 5** here, 4 only in the ZCP |
| dispatcher 0x20ED4 (129) | 0x210F8 (130) | the same 40 calls in the same order, minus the call of the sync routine 0x21FE0 after the mode state machine |
| 0x21074 (115, non-M) | 0x2129C (119) | the ZCP calls 0x1ED24(0) and 0x1ED24(1) also when bit 0 of 0x400530 is set (third disjunct); this image only on the two original conditions (bit 3 of 0x400F00 clear, or bit 0 of 0x400932 set) |
| 0x3A1BE (147) | 0x3A676 (147) | one curve: above 8000 the value falls from 10000 with slope 0x132/1024 here, from 9500 (0x251C) with slope 0x99/1024 in the ZCP; the lower branch `(x−8000)/300 + 50` is the same |
| 0x1E120 (31, non-M) | 0x1E2AC (23) + 0x1E2DA (8) | identical logic; the ZCP project split the 8-instruction tail (sets bit 3 of 0x40085A, and bit 2 of a byte for parameter 3) into a caller-less fragment |
| 0x1711C (22, non-M) | 0x1724C (18) | identical logic (bit 6 of the second input byte, pressed when clear, after the validity bits) |

## 2. The ZCP functions with no identical counterpart here

142 ZCP functions have no instruction-identical function in this image. They fall into four groups:

| Group | Count | What they are |
|---|---|---|
| empty jump-table entries | 73 | zero-instruction "functions" at 0x2F524..0x2F8AC that the ZCP recovery registered for the 255-entry table 0x2F4D0; this image's table 0x2EFD8 has none registered. Not code |
| partners of the 58 above | 50 | listed in section 1 by subsystem |
| recovery fragments of bodies that are whole here | 8 | 0x2F03C, 0x2F1FC, 0x2F394 (pieces of 0x2EA98's body), 0x1E2DA (tail of 0x1E120), 0x2EF38 (9, the tail of this image's 0x2EA36 min-of-magnitudes helper), 0x13AFE and 0x13B14 (state-selection fragments, see 1.5), 0x17270 and 0x17296 (4-instruction `return x−1` stubs beside the getters, no callers) |
| genuinely ZCP-only | 11 | below |

| ZCP function | Instructions | What it is | Callers here |
|---|---|---|---|
| 0x21FE0 | 271 | the channel sync routine of the M track mode | the dispatcher exists (0x20ED4) and simply does not call it |
| 0x17278 | 15 | the third button channel getter (bit 0 of 0x40063D) | the channel reader exists (0x21B48) without the channel-2 branch |
| 0x21FCC | 4 | writes one row-enable byte of the 37-row table | no callers in the ZCP either |
| 0x30F60 | 5 | sets bit 0 of 0x400DCA | its caller is the subtree init (Z 0x2D65E); this image's 0x2D162 ends without the call, and nothing else here sets that bit |
| 0x14AAE | 23 | `0x40059E −= (0x40056A−5)·0x83>>8` | this image has the same routine as 0x149D4, called from the same place (0x14690); the ZCP compiled it differently |
| 0x21E98, 0x21EB8 | 10, 8 | mode-block init helpers over three channels | this image has the two-channel versions 0x21C02, 0x21C24 |
| 0x21D8C, 0x21DF0, 0x21EC8, 0x21AA2, 0x21F80, 0x219F0, 0x21AF2, 0x21CF0 | — | the three-channel mode machine | the two-channel versions, section 1.1 |
| 0x230, 0x2F6 | 15, 3 | vector-area stubs below `codeLO`, auto-analysis noise | same bytes exist here, not registered as functions |

So the only features absent as a whole are the M button channel and its sync; every other ZCP-only function is either a different compile of a routine this image has, or a recovery artefact.

## 3. Data and calibration

Method: every pool word of every function that lands in the data section was collected in both images (462 bases here, 494 in the ZCP), each base paired with its ZCP counterpart through the reader function's twin and the base's ordinal position in that reader, and the bytes up to the next base compared.

| Result | Count |
|---|---|
| tables paired | 440 of 462 |
| identical bytes | 355 (5,740 bytes) |
| same length, different values | 65 |
| different length | 20 |
| bases here with no ZCP partner | 22 (the nine UART records, whose readers are 4-instruction stubs the pairing cannot follow; the 0x3EBC4 parameter block; two bytes shared by the CAN packers) |
| ZCP bases with no partner here | 59 (the 2-byte constants of the 0x13630 branch, read through pool words only in the ZCP; the pump temperature table 0x3E8DC/0x3E8E0; the UART records; 0x3E9AE, six words `000C 000C 000C 0190 0190 0190` read by Z 0x206AC) |

**Same tables, different values.** Most of the 65 are pointer words (code addresses that moved: the command table 0x3DC08, the KWP table 0x3ED24, the CAN message and signal tables, the UART driver pointers) or RAM addresses that moved with the layout. The calibration values that actually differ:

| Table (N / Z) | Reader | Difference |
|---|---|---|
| 28-byte curve records 0x3D36E/0x3D82E, 0x3D38A/0x3D84A, 0x3D3A6/0x3D866, 0x3D3C2/0x3D882, 0x3D416/0x3D8D6, 0x3D486/0x3D946, 0x3D4A2/0x3D962 | 0xE844, 0x13106, 0xD69A, 0xD4CC (the observer branch) | one word at offset 0x14 (the eleventh of fourteen): 0x1F58→0x3009, 0x50E2→0x62DA, 0x2CFF→0x3706, 0x115→0x145, 0x70E→0x49A, 0x13A→0x13C, 0x141→0x142 |
| fourteen 28-byte records 0x3D54A..0x3D6EE / 0x3DA0A..0x3DBAE | 0xD98C, 0xE070 | the same word in each: 0xC620→0xC764, 0x18AC→0x13D4, 0x397F→0x3AB8, 0x227C→0x229C, 0xC06→0xB51, 0x297F→0x33A6, 0x426→0x52A, 0x27DE→0x2896, 0x39DE→0x3AE2, 0xABD→0xAB3, 0x45BA→0x45BE, 0x2DFB→0x2E26, 0x3C7→0x46C, 0xF1C7→0xF133 |
| 0x3D746 / 0x3DC06 (2 words) and the 112-byte 0x3D74A / 0x3DC0A | 0xD98C, 0xE070 | 0x1EA 0x62 → 0x211 0x6A; three words at +0x50 are 0 here and 0x18EC, 0x18EC, 0x1369 in the ZCP |
| 28-byte records 0x3E874/0x3EEC8, 0x3E890/0x3EEE4, 0x3E8AC/0x3EF00, 0x3E8C8/0x3EF1C, 0x3E8E4/0x3EF38 | 0x29606, 0x29898, 0x26258 (lateral branch) | one word: 0x26→0x25, 0x736→0x735, 0x61D→0x960, 0x736→0x735, 0x83→0xC9 |
| 0x3DA10 / 0x3DF38 (76 bytes) | 0x15BDC (init) | two words at +0x48: `0003 FCE4` here, `0004 0318` in the ZCP (a count and a signed value, −796 against +792) |
| 0x3EEA4 / 0x3F4D8 (2,012 bytes, 6-byte fault records `0000 00nn Fnnn`) | 0x30C2C | one record: code 0xCD carries 0xF0CD here and 0x5E5F in the ZCP |
| 0x3DEC9 / 0x3E3F1 (471 bytes, 8-byte CAN records) | 0x1C8EC | 117 words, all code pointers (0x1DDC4 → 0x1DF50 and so on) |
| 0x3DBBC / 0x3E0E4 (76 bytes) | `mfg_addr_directory` 0x1BE8A | the factory address directory: RAM 0x40063A, 0x4006B4, 0x4006C3 here against 0x40063C, 0x4006B6, 0x4006C5; ROM 0x3DAF0, 0x3DB08, 0x3DB26 against 0x3E018, 0x3E030, 0x3E04E (the mask table, the dataset-prefix bytes and the version word, at their new places) |
| 0x3ECB4 / 0x3F2E8 (80 bytes) and 0x3EC94..0x3ECAC | 0x31326 (KWP) | the KWP RAM block 0x4011D7.. against 0x401207.., the layout shift |
| 0x3DB08 / 0x3E030, 0x3DB18 / 0x3E040, 0x3EC36 / 0x3F26A, 0x3E0A8, 0x3EAB0, 0x3F9B8, 0x3DA80 | banner, dataset names | the dataset prefix `AUP` against `AWP`, the stamp `81 0E 06 02 0E 1E 69 03` against `80 0D 02 03 0B 14 74 03` (after the common `90 00 0E`), `AUTW24F1.DFT` against `NOPARADJ.UST` in the eighth slot, `24` against `26`/`33` in the names |
| 0x3FCE4 / 0x40318 | 0x15CAC (init) | the image check word `4527 D066` against `E5A6 292C` |
| coding descriptors 0x3FCCC/0x3FCD4/0x3FCDC | `coding_image_seal` 0x3B2EE | RAM 0x400FF2, 0x40100E, 0x401016 against 0x401022, 0x40103E, 0x401046; lengths 0x7B, 0x73, 0x77 and version bytes 1, 8, 9 identical |

**Different length.** The 20 are layout, not content: the ZCP reads constants of the 0x13630 branch and the actuator driver through separate pool words (so its inventory shows 2-byte bases where this image shows blocks of 22 to 296 bytes with identical contents, for example 0x3D8E4 here against 0x3DDF4..0x3DE42 there), the transition table is 896 against 1,184 bytes (28 against 37 rows), and the 0x2BC34 parameter block 0x3EBC4 has no ZCP equivalent.

**Present in one image only.** The pump temperature table at Z 0x3E8DC (`0064 0064 FFEC FFFD 0023 0046 0000 61A8`), read only by the ZCP's 0x1F9C0; the ZCP's 0x3DDAC base-value word for 0x14990's parameter 3; the parameter block 0x3EBC4 here. Everything else exists in both.

**CAN.** The twelve message records (0x3DCC2 / 0x3E1EA) are identical bytes: identifiers 0x153, 0x1F0, 0x1F3 transmitted; 0x316, 0x329, 0x1F5, 0x43F, 0x43D, 0x235, 0x615, 0x613, 0x610 received; the same lengths and periods. The signal table (0x3DE98 / 0x3E3C0) has 78 of its first 80 eight-byte entries identical in the first four bytes (position, width, encoding), and the getter pointers of 0x153's signals resolve to functions that are identical twins (the first two by signature; the next four point into shared code rather than at function entries in either project). The 0x153 byte 7 bit 1 getter is the identical 0x1DC54 / 0x1DDE0. The frame layouts therefore match.

**KWP service table** (0x3ED24 / 0x3F358): seventeen records, identical SID, length, mask and level bytes; fourteen handlers identical twins; three handler bodies differ (SID 0x27 at 0x31A82, SID 0x31 at 0x3233C, SID 0x18 at 0x31FE0; section 1.7, no behavioural change found).

**Factory command table** (0x3DC08 / 0x3E130): thirty slots, slots 5 and 28 empty in both; twenty-five populated slots are identical twins by signature, and the remaining three (slots 12, 13, 18: 0x15572, 0x15566, 0x1BA6A, each 4 to 6 instructions) are byte-identical bodies whose signatures are too short to be unique.

## Method and provenance

Both projects were copied (`.gpr` and `.rep`) to a scratch directory and processed with `analyzeHeadless -noanalysis -readOnly`; a one-off script decompiled every function of each program to a text file and wrote the listing with pool-word values beside it. The pairing used the exact-signature twins of `tools/ghidra/mcore_port.sh` and, for the rest, the best 6-gram containment from `tools/ghidra/mcore_fuzzy_port.sh`'s masked dumps. Before diffing, every ZCP address in the decompiled text was mapped to this image's address space through the pool constants of identical twin pairs (RAM, ROM tables, callees), decompiler temporaries and labels were renumbered by first appearance and type prefixes stripped, so that the unified diffs show source changes only; the diffs quoted above were read, not sampled. The data comparison used the pool words of every function, read from the raw files. No project was modified. Companion documents: `e46_m3_non_zcp_mk60_image.md`, `zcp_mk60_image.md`, `zcp_m_track_mode.md`, `nonm_mk60_image.md`.
