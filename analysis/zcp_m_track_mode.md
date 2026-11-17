# zcp_m_track_mode Analysis

2026-11-17

The M track mode of the E46 M3 ZCP is a third position of a driver-mode state machine that both E46 images carry in full: mode 4, the value the ported name `_is_mdm` tests, exists in the non-M image as well, where a coding bit turns the single DSC button into a three-position cycle (short press to mode 4, long press to off). What the ZCP adds is a third button channel read from a port pin the non-M never reads (bit 0 of the input byte 0x40063D, routine 0x17278), a per-channel transition table grown from 22 to 37 rows so that this channel toggles mode 4 directly, and one new 271-instruction routine, 0x21FE0, that couples the new channel to the DSC-button channel so that both agree on one effective mode. Everything downstream of the mode value is shared: the control functions that read mode 4, the lamp requests, the CAN status bit (message 0x153, byte 7 bit 1) and the KWP status byte are the same code in both images, restructured but not re-purposed. Nothing about the mode is stored in EEPROM; it starts at mode 1 (DSC on) every power cycle. Projects: `projects/817.3_e46_m3_zcp_mk60` (program `e46_m3_zcp_mk60_full_dump.cpu.hex`) and `projects/820.3_E46_non_m_mk60` (program `non_m_mk60_full_dump.cpu.hex`), both read-only for this document.

## The mode state

`driver_mode_get(ch)` (ZCP 0x21D2C, non-M 0x2197C) returns a 32-bit word from an array indexed by a channel number: ZCP `0x400870[ch]`, non-M `0x400874[ch]`. The array is one row of a per-channel block that the initialisation routine (ZCP 0x21AA2, non-M 0x21704, called once from the control-block init 0x1E0C4 / 0x1DDE8) fills for three channels in the ZCP and two in the non-M. The channel count is the first visible difference between the images: the init loop runs three times against two, and every loop over the channels in the state machine stops at 2 against 1.

| Field (one entry per channel) | ZCP | Non-M | Init value | Read by |
|---|---|---|---|---|
| State of the button state machine, u32 | 0x400864 | 0x40086C | 1 | the state machine only |
| Mode, u32 (`driver_mode_get`) | 0x400870 | 0x400874 | ch0 1, ch1 9, ch2 1 | the predicates below, 0x21FE0, 0x22440 |
| Sub-value, u32 (*proposed* `driver_mode_sub_get`, ZCP 0x21D34, non-M 0x21984) | 0x40087C | 0x40087C | 0 | the lamp-request logic in the dispatcher, 0x21FE0 |
| Press counter, u16 | 0x400888 | 0x400884 | 0 | the transition table lookup |
| Saved counter, u16 | 0x40088E | 0x400888 | 0 | state 4/5/7 of the state machine |
| Channel enable, u8 (setter *proposed* `driver_mode_channel_enable`, ZCP 0x21D3C, non-M 0x2198C) | 0x400894 | 0x40088C | 1 | the state machine loop |
| Fallback state byte / stored mode byte pair, u8 | 0x400897 / 0x400898 + 2·ch | 0x40088E / 0x40088F + 2·ch | 8 / 7 | state 0 and state 6 |
| Published mode / sub pair, u8 (writer *proposed* `driver_mode_publish`, ZCP 0x21E80, non-M 0x21A7E) | 0x40089D / 0x40089E + 2·ch | 0x400892 / 0x400893 + 2·ch | 0x0F / 3 | the transition validity check only |
| Stuck-button flag, u8 | 0x4008A3 | 0x400896 | 0 | the stuck-button monitor |
| Row-enable bytes, one per table row | 0x4008A6 (37) | 0x400898 (22) | see coding below | the table lookup |
| Last-seen mode/sub of channels 2 and 0, change code | 0x4008CB..0x4008CF | none | 0 | 0x21FE0 (ZCP only) |

The mode values are tested by five small predicates, all identical by signature between the images (the names are the E9x M3 ones, ported):

| Predicate | ZCP | Non-M | True for mode | Callers (ZCP / non-M) |
|---|---|---|---|---|
| `driver_mode_is_on` | 0x20BCC | 0x20838 | 1, 2, 5 | 8 / 8 |
| `_is_off` | 0x20BEE | 0x2085A | 0, 6 | 16 / 16 |
| `_is_mdm` | 0x20C22 | 0x2088E | 4, 7 | 11 / 27 |
| unnamed, *proposed* `driver_mode_is_longpress` | 0x20C08 | 0x20874 | 2, 3 | 1 / 1 (the KWP SID 0x21 handler) |
| unnamed | 0x20C3C | 0x208A8 | 7 | none |

Modes 5, 6 and 7 appear in the predicates but in no transition table and no writer of either image; they are inherited vocabulary. Mode 2 is a transient state reached by holding a button for 1431 ticks and left again after 500 (see the tables); mode 3 is never written here either. The working set is therefore 1 (DSC on), 0 (DSC off) and 4 (the third mode: DTC on a non-M car coded for it, M track mode on the ZCP). Channel 1 uses a separate vocabulary, modes 8 to 14, and belongs to a second coded driver function (below).

**Writers.** The mode and sub words are written by exactly three routines in the ZCP: the table lookup `0x219F0` (through pointers into the array, when a table row fires), the pair setter `0x21D46` (*proposed* `driver_mode_set`, called only by the sync routine 0x21FE0 and by the channel-1 handler 0x22440), and the init. The non-M has the same three, except that its pair setter 0x21996 has no caller at all: the compiler kept a routine that the non-M build never uses, and the ZCP's new code is what calls it.

## The state machine and its tables

The per-channel state machine (ZCP 0x21AF2, non-M 0x2174E, *proposed* `driver_mode_button_sm`) is run once per tick of the control dispatcher (ZCP 0x210F8, non-M 0x20D58, called from the 10 ms body 0x18116 / 0x17F42), for every enabled channel. It first reads the channel's button through a per-channel reader (ZCP 0x21D8C, non-M 0x219DC) that returns 1 pressed, 0 released, or −1 when the input block is invalid; a −1 forces the channel's state to its stored mode byte. Then it runs the states:

| State | Meaning | Transition |
|---|---|---|
| 0 | take the fallback state byte (8 = "any state" wildcard in the table) and run the table once | leaves through the table |
| 1 | start-up | unless the factory window flag (bit 1 of 0x4005AE / 0x4005BE) is set: publish the current mode and go to 2 |
| 2 | idle, released | pressed → 3 with counter 0 |
| 3 | pressed, waiting for recognition | released → 2; held → counter++, table lookup (rows with threshold 21 fire here) |
| 4 | press recognised, still held | held → counter++, table (long-press rows); released → save counter, 5 |
| 5 | released after a press | counter++, table (release rows at 21, 25 or 50); pressed again → 4 with the saved counter |
| 6 | timed state after a long hold | stuck-button monitor, then table (rows at 499/500 ticks) |
| 7 | re-entry | pressed → 4, released → 2, both restoring the saved counter |

A table row is 32 bytes: channel, required state (8 = any), next state, required mode (15 = any), new mode (15 = keep), required sub (3 = any), new sub (3 = keep), and a 16-bit counter threshold; the row fires when the channel, state, mode, sub and counter all match and the row's enable byte is set. The lookup routine (ZCP 0x219F0 over 37 rows at 0x3E9F8, non-M 0x21654 over 22 rows at 0x3E4B4) then calls a validity check (ZCP 0x21DF0, non-M 0x21A20) whose every path returns 1, so the row always applies, and publishes the new pair. Ticks are the dispatcher period, taken as 10 ms from the existing documents; 21 ticks is then the press-recognition debounce, 358 about 3.6 s, 500 about 5 s, 1431 about 14 s.

**Non-M channel 0 (the DSC button), 14 rows.** Rows 0 to 3 are enabled only when bit 6 of the coding word 0x401018 is set; rows 4 to 9 only when it is clear (enable table built at init by 0x21B54 from the getter 0x3B782); rows 10 to 13 always.

| Rows | Enabled when | Behaviour |
|---|---|---|
| 0..3 | coding bit 6 set | a press in mode 1 marks sub 1; the release (21 ticks) goes to mode 0. A press in mode 0 marks sub 0; the release goes to mode 1. A two-position toggle: DSC on / DSC off |
| 4..9 | coding bit 6 clear | a press in mode 1 marks sub 2 and the release goes to mode 4 (sub 2). A press in mode 4 or in mode 0 marks sub 0 and the release goes to mode 1. Holding the button for 358 ticks in state 4 goes to mode 0 (sub 1). A three-position cycle on one button: on → short press → mode 4 → short press → on, and a long press from anywhere → off |
| 10..13 | always | holding 1431 ticks goes to mode 2 and state 6; after 500 more ticks state 6 returns to mode 1 and state 2; a release ends after 25 ticks; state 7 falls back to 6 |

So the non-M firmware already contains the "DSC → DTC → off" behaviour of the later E46 cars and selects it by coding: bit 6 of coding byte 3 set means the plain two-position toggle, clear means the three-position cycle with mode 4. On this ECU family mode 4 is the M3's MDM slot by signature, and on the non-M it is reached by a short press of the only button.

**ZCP, 37 rows.** Channel 0 keeps rows 0 to 3 (the on/off toggle) enabled unconditionally; the three-position rows are gone from channel 0 and replaced by rows 6 to 9, which say what the DSC button does when the mode is already 4: a press marks sub 1 and the release goes to mode 0 (DSC off), and state 6 leaves mode 4 or 1 for mode 2 after 499 ticks. Channel 2 is new, rows 26 to 36:

| Rows | Behaviour of channel 2 (the extra button) |
|---|---|
| 26, 27 | a press in mode 1 marks sub 2; the release (21 ticks) goes to mode 4, sub 2 |
| 28, 29 | a press in mode 4 marks sub 0; the release goes to mode 1, sub 0 |
| 32, 33 | a press in mode 0 marks sub 2; the release goes to mode 4, sub 2 |
| 30, 31, 34, 35 | holding 1431 ticks goes to mode 2 and state 6; state 6 goes back to mode 1 after 500 ticks, and from mode 0 or 1 to mode 2 after 499 |
| 36 | a release ends after 25 ticks |

Rows 6 to 9 and 26 to 36 are enabled only when bit 6 of the ZCP coding word 0x40105A is clear (0x21F80 at init, getter 0x3B9F6), and the same routine sets the channel-2 enable byte to the inverse of that bit, so with the bit set the ZCP degrades to the non-M's two-position DSC button and the extra button does nothing. Channel 1 has the same twelve rows in both images (modes 8 to 14, thresholds 21, 50, 571, 1431, 500) plus two more in the ZCP (rows 12 to 17 against 14 to 17), and is not part of this question.

## The buttons

Digital inputs reach the firmware as three bytes copied from a port page, not from the system ASIC: the routine 0x1717A (non-M 0x16FCA), called from the ASIC status reader 0x16FBC (non-M 0x16E0C) on every tick, copies the port registers 0xEFFF28, 0xEFFF2A and 0xEFFF2B into 0x40063C, 0x40063D and 0x40063E (non-M 0x40064A..0x40064C). Six small getters read single bits of these bytes; every one first requires bit 3 of the first byte and bit 4 of the second to be set and returns −1 otherwise, which the state machine treats as "input invalid, hold the mode".

| Getter (ZCP / non-M) | Bit | Polarity | Used as |
|---|---|---|---|
| 0x171F6 / 0x17046 | 0x40063C bit 6 | pressed when set | channel 0, the DSC button (through the thunk 0x21D80 / 0x219D0) |
| 0x1724C / 0x1709C | 0x40063D bit 6 | pressed when clear | channel 1, the button of the second coded function |
| 0x17278 / none | 0x40063D bit 0 | pressed when clear | channel 2, the M track mode button (ZCP only) |
| 0x171A4 / 0x16FF4 | 0x40063C bit 0 | active when set | a switch read by 0x2BA3A, 0x24010, 0x19318 and the SID 0x21 handler |
| 0x171CA / 0x1701A | 0x40063D bit 5 | active when set | a switch that goes out on CAN 0x153 byte 7 bit 7 and into the lamp driver 0x17F40 |
| 0x17222 / 0x17072 | 0x40063D bit 1 | active when clear | a switch read by the control function 0x107AC and both status reporters; the fuzzy pass's 50% candidate `dsc_button_input_get` is this routine, and the call sites say it is not the DSC button |

The ZCP's channel reader 0x21D8C is the non-M's 0x219DC plus one branch: channel 2 calls 0x17278. Nothing in the non-M reads bit 0 of 0x40064B; the pin is unused there, so the ZCP wiring brings a new input into play rather than re-reading an existing one. Channels 0 and 2 share one more rule in the reader: when bits 3 and 4 of the fault-area byte 0x400E2A (non-M 0x400DD6) are both set and the mode is on, the button reads as released, which blocks mode changes in that fault state. There is no input-layer debounce; the 21-tick recognition in the table is the debounce.

**Stuck button.** In state 6 the monitor 0x21EC8 (non-M 0x21AC8) posts a fault through the fault poster 0x32F94 (non-M 0x3213C) once the counter passes 0x32: fault word 0x91000 for channel 0, 0x92000 for channel 1, and 0xC2000 for channel 2, which is the one new fault code of the ZCP in this area (index 0xC, bit 13 of the latch array at 0x400E38). Whether that maps to a KWP DTC is not traced.

## The new routine 0x21FE0

Called from the dispatcher 0x210F8 immediately after the state machine, and from nowhere else; the non-M dispatcher 0x20D58 has no call at this position. The ZCP document's "positional partner 0x20838, 17 instructions" is `driver_mode_is_on` and is not a partner; the routine has no counterpart, and the fuzzy pass found no non-M function sharing a 6-gram with it. *Proposed* name `driver_mode_channel_sync`. What it does, reading the decompilation:

1. Compares the current mode and sub of channel 2 and channel 0 against the four bytes 0x4008CB..0x4008CE saved at its last run. If none changed it returns.
2. Writes a change code into 0x4008CF: 1 when a mode changed on either channel, 2 when only a sub changed, 3 when only a sub changed and the previous run also saw only a sub change.
3. On a sub-only change (code 2): if channel 2's sub is unchanged (so the DSC button is mid-press) and channel 0 is not in mode 2, it disables channel 2 for this press; if channel 2's sub changed and channel 2 is not in mode 2, it disables channel 0 and copies channel 2's mode and sub into channel 0. The two buttons cannot act on the same press.
4. On a mode change (codes 1 and 3): re-enables channel 2 according to the coding bit (bit 6 of 0x40105A) and channel 0 unconditionally. If channel 2's mode changed: a change to mode 2 (the 14 s hold) flips channel 0 between modes 0 and 1 with swapped sub; any other change copies channel 2's mode and sub into channel 0 and publishes. If channel 0's mode changed: when either channel is in mode 2 or 3, and channel 2 is at 4, channel 0 is forced to (4, 2); otherwise channel 2 is set to (0, 1) or (1, 0) after channel 0; in the normal case channel 0's new mode and sub are copied into channel 2 and published. A channel-0 state 6 with the DSC button released is reset to state 2 on the way.
5. Saves the four values for the next run.

The effect is one effective mode in channel 0, which is the channel every predicate reads: the M button moves channel 2 between 1 and 4 (and from 0 to 4), the sync copies it into channel 0; the DSC button moves channel 0 between 1 and 0 (and from 4 to 0), and the sync copies that into channel 2 so the M button's next press starts from the right place. Pressing the M button while DSC is off turns both DSC and the M mode on at once (rows 32 and 33), and pressing the DSC button in M track mode turns DSC off (rows 6 and 7). Those two edges are inferred from the tables and the sync, not observed on a car.

## What mode 4 changes

The consumers are the same procedures in both images; the ZCP merely restructured the traction-control tree so that most of its functions read one derived flag instead of calling the predicate.

| Consumer | ZCP | Non-M | What mode 4 does there |
|---|---|---|---|
| Traction and yaw entry: writes the byte 0x401045 (ZCP) = `_is_mdm()` each cycle | 0x3B53C, called from 0x39004 | not found: the non-M traction functions call `_is_mdm` directly | the flag read by `tcs_driveslip_curve` 0x35644, 0x37E68, 0x382D8, 0x38492, 0x3859C, 0x385DE, 0x3871C, 0x3876E; in the non-M the same role is taken by 17 direct callers in 0x346B0..0x3879C (0x3626E, 0x366C4, 0x36878, 0x36A08, 0x36BC8, 0x36DAE, 0x36FD4, 0x371AC, 0x37378, 0x378AC, 0x37B18, 0x37B98, 0x37E8C, 0x3814C, 0x3879C, 0x34F9C, 0x346B0) |
| Traction calibration pairs in the tail block (complete dump) | 0x3876E reads `tbl_040234 + 0x22 × 0x401045`, `tbl_040278[0x401045]` (3 or 6) and `tbl_04027C[0x401045]` (0 or −1000); paired 34-byte curve records at 0x401F0/0x40212, 0x40234/0x40256, 0x400E8/0x4010A, 0x4012C/0x4014E | 0x3814C with the single record 0x3FA7C; single records 0x3FA0C, 0x3FA2E, 0x3F948, 0x3F96A | the only place found so far where mode 4 selects a different calibration record rather than a branch: the ZCP's second record of each pair (0x2260 against 0x1A90; 0x44C against 0x1FE; 0x578 against 0x2EE) is the M track mode set; the consumers of the other three pairs were not read for the index |
| Traction slip term | 0x36C08 | 0x366C4 | widens the allowed slip when the speed word 0x400FC0 is under 2000 and adds a term scaled by the byte 0x40106D |
| Observer branch (the largest control function of the 0xD2A4 subtree) | 0xE844 (5 call sites) | 0xE3AC | scales two threshold pairs (0x40042A/0x40042E, 0x400434/0x400436) by curves `tbl_03d656`..`tbl_03d68E` interpolated on 0x400480 when in mode 4, and clamps them at 0x28E7/0x262D |
| Lateral estimator | 0x27220 (3 sites) | 0x26B68 | raises the limit byte 0x400ACD from 0x8E to 0xF9 in mode 4 above a speed threshold |
| Wheel-side state | 0x12E44 | 0x12E58 | clears a flag in 0x40034B when in mode 4 |
| 0x28274, 0x29110, 0x291B0 | — | 0x27668, 0x2850A, 0x285C4 | counters and thresholds (0x10BD against 0x85F) in the lateral branch |
| KWP SID 0x21 handler, local id 5 | 0x321BC | 0x31348 | bit 3 of the status byte 0x4005C5 is set when the mode is on **or** 4; no separate bit for mode 4 |
| Diagnostic status block 0x401207..0x40120C | 0x32C6C | its twin | bit 3 of 0x40120A likewise |
| CAN 0x153 signal getter | 0x1DDE0 | 0x1DB2E | see below |

So on the ZCP, M track mode is to the traction-control and estimator functions exactly what DTC is on a coded non-M car: the same procedures take the same branches, with the ZCP's recalibrated constants. Mode-4-specific table selection exists in two places: the curve pairs in 0xE844, and the paired calibration records in the tail block of the complete dump (0x40000..0x4031F), where 0x3876E picks the record and two constants by the mode flag byte 0x401045; the non-M has single records there. Everything else is branch-level threshold and gain changes inside the restructured functions, and this document does not re-derive those procedures. 
**Lamps.** The dispatcher posts lamp requests through the request arbiter 0x1E114 (non-M 0x1DE38) from the channel-0 sub-value: sub 2 (mode 4 selected) posts id 0xC and, unless coding bit 10 is set, ids 2 and 3; sub 1 (DSC off) posts ids 2 and 3 and, if coding bits 6 and 10 are both clear, id 4. The arbiter packs two bits per id into 0x400859..0x40085B and the dispatcher's first call 0x1E39E publishes them as 0x40085C..0x40085E. Ids 2 and 3 are the DSC warning lamp: the getter 0x1D988 turns them into bit 0 of byte 1 of CAN message 0x153, and the lamp driver 0x17F40 turns them into bits 0x20 and 0x08 of the output image 0x400613, which reaches the port pins 0xEFFF2B/0xEFFF3F through 0x18BA0. Both DSC off and M track mode therefore request the DSC lamp steadily; the readers of id 4 (0x40085A bit 0) and id 0xC (0x40085B bit 1), which would separate the two, were not found in either image.

**CAN.** The images have a table-driven CAN layer: twelve message records at 0x3E1EA (non-M 0x3DCFE), 12 bytes each with the identifier in bytes 2..3, and a signal table at 0x3E3C0 (non-M 0x3DED4) of 8-byte entries whose word at +4 is a getter; the transmit scheduler 0x1C364 walks the message records, the packer 0x1C596/0x1C810 places each signal, and 0x157DC programs the controller at 0xFFFC80. On the vehicle bus the module transmits three frames, 0x153, 0x1F0 and 0x1F3, and receives nine, 0x316, 0x329, 0x1F5, 0x43F, 0x43D, 0x235, 0x615, 0x613 and 0x610; the full frame and signal map is in `zcp_can_frames.md`. Message 0x153 carries 21 signals; signal 36 is a 1-bit field at **byte 7, bit 1**, with the getter 0x1DDE0 (non-M 0x1DB2E, the same procedure): when the frame is built, this getter computes the bit's value as 1 when coding bit 6 is clear and either the factory window flag is set or `_is_mdm()` is true with the ECU not in test mode and bit 6 of 0x400EE4 clear, and 0 otherwise. A K-line tester can override the *reported* value through the KWP input/output-control service (0x31306/0x31370, output id 0xB), which changes what the frame says, not the mode itself. The bit is a status announcement only: nothing received on CAN writes the mode, whose only writers are the button state machine and the sync routine. Both images send this bit, so the bus sees "mode 4 active" whether the car calls it DTC or M track mode. Neighbours in the same byte: bit 7 the switch 0x171CA, bit 6 bit 4 of 0x400959, bits 4..5 a two-bit state from 0x22270, bit 2 a speed-gated fault flag (0x1DA4E, override id 0xC).

**EEPROM.** No NVM job names any address of the mode block: the NVM request routine 0x16172 is called only from the factory protocol (0x1B3A2, 0x1B50A) and the KWP services 0x23 and 0x3D, the job queue 0x1608E from SID 0x2E, and no descriptor in the image points into 0x400864..0x4008CF. The mode is 1 after every power cycle. The only persistent input to the logic is the coding record: EEPROM record 0xA3, 15 bytes, written by KWP SID 0x2E local identifier 0x17 (handler 0x32776, descriptor at 0x3F47C into the buffer 0x4005F0, decode through 0x3B5F4 → 0x3BBD6 → 0x3BB16, then `coding_image_seal`); the word 0x40105A (non-M 0x401018, decoder 0x3B8A2) is coding byte 3 in its low byte plus bits 4..6 of coding byte 10 in bits 8..10. Bit 6 of coding byte 3 is the bit above; bit 10 (coding byte 10 bit 6, getter 0x3BA4E) suppresses the DSC-lamp requests for the third mode; bit 9 (getter 0x3BA38, feature 3 of the coded-feature switch 0x3B614) enables the second driver function behind channel 1. The start-up path that restores the coding image at 0x40103E..0x40105B from EEPROM was not traced.

## Persistence across boot cycles

The mode selection is **not remembered across power cycles, and there is no existing memory for it.** The mode
lives only in RAM `0x400870[ch]`, the init `0x21AA2` hardcodes ch0 = 1 (DSC on) every boot, and (per the
EEPROM paragraph above) no NVM descriptor points into the mode block `0x400864..0x4008CF`. The only persistent
input is the coding record (EEPROM `0xA3` → word `0x40105A`), which gates *whether the feature exists* (bit 6
of coding byte 3 = two-position toggle vs. three-position cycle with mode 4) — it does not store the current
selection, and no coding bit changes the hardcoded startup mode.

Note that an RAM write via the mfg protocol (the `0xA5` family) cannot be used to make it stick: the only exit
from the mfg session is an ignition cycle, which is a software reboot that re-zeros `0x400300..0x401800` and
re-runs init, so any value written to `0x400870` in mfg is gone before the application runs normally.

Making the selection persist therefore requires a **firmware modification (flash write)**, in one of two
flavours:

- **A1 — boot into M track mode by default (minimal).** Change the constant the init `0x21AA2` writes into
  `0x400870[0]` from `1` to `4`. Essentially a one-value flash edit; no EEPROM, no hooks. It does not remember
  the last choice — it just makes mode 4 the power-on default (short-press still cycles as normal). For a
  dedicated track setup this is often the desired behaviour and is near the minimum possible flash change.
- **A2 — remember the last selection (the literal goal).** (1) hook a mode writer (`driver_mode_publish`
  `0x21E80`, or the sync `0x21FE0`) to write ch0's new mode to a persistent byte; (2) patch `0x21AA2` to read
  that byte into `0x400870[0]` instead of the constant, clamped to a safe value with fallback to 1; (3) a
  persistent home for the byte — a free slot or an extension of coding record `0xA3` — written through the
  module's existing NVM write path.

Both are gated on a flash-write capability (the mfg `0xA5`-write → RAM-exec → flash-program chain documented
in `zcp_mfg_exec_path.md`, or BDM/chip-off). This makes M track persistence a good *first* real mod: tiny,
self-contained, and reversible from the stock dump.

Two caveats before patching: (1) the mode-4 edges and the `0x400870[0] = 4` mapping are inferred from the
tables and the sync routine, "not observed on a car" (see below) — confirm first by entering mfg and
`0xA4`-reading `0x00400870` with M track selected vs. not; (2) A1/A2 change the deliberate "DSC on at every
start" safety default.

## What is not pinned

- The physical pin behind 0x40063D bit 0 and whether the M button LED is driven from this ECU at all: the lamp arbiter ids 4 and 0xC have no reader found, and the output image 0x400613 was followed only to the port writer 0x18BA0.
- The meaning of mode 2 (14 s hold on either button, 5 s long) and the transient state 6: a service or calibration entry is the obvious guess, not shown.
- The identity of channel 1 (modes 8..14, coded feature 3, state machine 0x22440 / 0x21DAC over the 0x2C1A2 subtree's state at 0x400D43..0x400D45): a second optional driver function with its own button, present in both builds.
- The tick period: 10 ms is taken from the existing documents, not re-measured; the thresholds in seconds scale with it.
- Whether the fault 0xC2000 and the 0x153 bit reach the cluster or a tester as a visible code or signal.
- The exact control differences inside the restructured traction functions beyond the branches listed; the ZCP doc's subtree table is the map for that.

## Method and provenance

Both projects were copied (`.gpr` and `.rep`) into a scratch directory and processed there with `analyzeHeadless -noanalysis -readOnly`, as `tools/ghidra/dump.sh` does; two one-off scripts decompiled every function of each program to a text file and wrote the full listing with resolved references (pool words, memory reads and writes) beside it. The findings above are grep and read over those two files: callers from the decompiler's call graph, readers and writers of each RAM address from the decompiled text, pool words from the listing. The transition tables, the KWP service tables, the NVM descriptors, the CAN message and signal tables and the pointer tables were decoded from the raw `.bin` files with their record layouts taken from the decompiled routines that walk them (0x219F0 for the mode table, 0x1C364/0x1C596/0x157DC for the CAN tables). Non-M addresses were taken from the same decode of the non-M program, not derived by offset. No project was modified, no name was applied; every *proposed* name is a suggestion. Companion documents: `zcp_mk60_image.md` (the image and its subtrees), `nonm_mk60_image.md`, `mfg_test_cycle.md` and `kwp_dispatch.md` (the E9x M3's mode manager, which this generation does not have).
