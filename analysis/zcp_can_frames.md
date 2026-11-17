# zcp_can_frames Analysis

2026-11-17

The E46 MK60 speaks on two CAN controllers through one table-driven layer: on the vehicle bus (controller registers at 0xFFFC80) it transmits three frames, 0x153, 0x1F0 and 0x1F3, every tick of the 10 ms task and receives nine, 0x316, 0x329, 0x1F5, 0x43F, 0x43D, 0x235, 0x615, 0x613 and 0x610; on a second controller (0x7FFC80) it runs a private sensor bus on which it sends one two-byte frame 0x002 and receives 0x130, 0x131 and 0x132 from the yaw-rate and lateral-acceleration sensor cluster. Every frame is described by a 12-byte message record, a list of 4-byte signal placements, and an 8-byte signal entry per signal that names a getter routine (transmit), a setter routine or a RAM word (receive). The tables are byte for byte the same in the ZCP and the non-M images, 0x270 apart in the data section, and the getters and setters of the status frame are the same procedures; what the frames carry therefore does not distinguish the two cars. This document lists each frame, each signal's position and the routine or variable behind it, the receive timeouts and the faults they raise. Projects: `projects/817.3_e46_m3_zcp_mk60` (ZCP, the addresses given first) and `projects/820.3_E46_non_m_mk60` (non-M, in brackets); both read-only.

## The layer

| Element | ZCP | Non-M | What it is |
|---|---|---|---|
| CAN task | 0x1C16C, called from the 10 ms body 0x18116 (and its test-mode twin 0x1819E) | 0x1BF54, from 0x17F42 | runs the receive-timeout and transmit scheduler every tick |
| Network state byte | 0x400751 (bits 0..2 state, bit 3 re-init, bit 4 bus up, bit 6 transmit inhibit, bit 7 fault) | not re-read | state 0 brings the controller up through 0x159E4, 1 is running (transmit 0x1C470, receive 0x1C658), 2 is quiet, 7 counts bus-off recoveries in 0x400779 (0x1C874) |
| Bus state poll | 0x1C1F0, through 0x156C4 reading the controller status word (0x3BF9C) | 0x1BFD8 | raises fault 0xA0002 when the bus-off counter passes 29 |
| Transmit scheduler | 0x1C470 over message records 0..2; timeout countdown 0x1C364 | 0x1C24C; 0x1C140 | sends a record when its counter 0x400769[i] reaches record byte 6 (1 for all three), i.e. every tick; the counters stop while bit 6 of 0x400751 is set |
| Frame packer | 0x1C596 walks a signal list, 0x1C810 writes a field of n bits at byte and bit offset (the decompiler shows the bit offset as 0, the listing at 0x1C5F8 shifts it out of the list entry) | — | signal entry types: 0 call a getter, 1 read a RAM word, 2 call with the frame buffer, 3 constant, 4 reload a receive timeout |
| Receive poll | 0x1C658 over records 3..11, mailbox read 0x1588C, unpacker 0x1C720/0x1C7F4 | — | a received frame sets bit 0 of its status byte 0x400759[i]; each signal's setter is called with the extracted value; the list's last entry (type 4) reloads the message's timeout 0x40077C[i] |
| Timeout countdown | 0x1C364, slots 0..8 of the signal table, countdown in 0x40077C[i] while bit 4 of 0x400751 is set | — | at zero the slot's handler runs (a fault poster) |
| Controller driver | 0x157DC (mailbox setup), 0x157B0/0x157BC (transmit), 0x1588C (receive), 0x15786 (status), 0x159E4 (init, faults 0xA0001 / 0xA0100 when the controller does not come up) | — | registers at 0xFFFC80 (bus 1) or 0x7FFC80 (bus 2), 16-byte mailboxes; messages 3..11 use mailboxes 3..11 (table 0x3E37E), the three transmit frames mailboxes 0..2 |
| Message records | 0x3E1EA, 12 bytes × 12 | 0x3DCFE | bytes 0..3 identifier, 4 DLC, 5 flags (0x08 transmit), 6 transmit period in ticks, 7 receive timeout in ticks, 10 signal count, 11 first list entry; bytes 8 and 9 are not decoded |
| Signal lists | 0x3E27A, 4 bytes per entry | 0x3DD8E | byte 0 width in bits, byte 1 bit offset (high nibble) and byte offset (low nibble), byte 3 signal index |
| Signal table | 0x3E3C0, 8 bytes × 65 | 0x3DED4 | byte 0 type, byte 1 timeout reload (slots 0..8), word at +4 getter, setter or RAM address |
| KWP input/output control | 0x31306 (is output n forced?), 0x31370 (forced value), flags 0x401230/0x401232, values 0x40122C.. | 0x30492, 0x304FC | a tester can force the reported value of output ids 0..12; the getters marked *IO n* below honour it |

Ticks are the 10 ms task period, taken from the existing documents; the three transmit frames go out every tick and the receive timeouts are 0x47 = 71 ticks (710 ms) for the engine and gearbox frames, 14 (140 ms) for the steering angle, 0x8E = 142 (1.4 s) for 0x615/0x613 and 28 (280 ms) for 0x610.

## Transmitted frames on the vehicle bus

### 0x153, 8 bytes, every tick: the DSC status frame

Record 0 at 0x3E1EA (0x3DCFE), 21 signals. The *IO n* column is the KWP output-control id the getter honours.

| Byte.bit | Width | Getter ZCP (non-M) | Value | IO |
|---|---|---|---|---|
| 0.0 | 1 | 0x1DB20 (0x1D862) | bit 0 of the torque-request flag byte 0x40078C (set when a positive torque request is active) | 0 |
| 0.1 | 1 | 0x1DB46 (0x1D896) | bit 1 of 0x40078C (set when a negative, engine-drag request is active) | 1 |
| 0.2 | 1 | 0x1DA10 (0x1D752) | 1 when bit 6 of 0x400EE4 is set and bits 4..5 of 0x400EE7 are clear; in test mode the inverse of "IO 0 or 1 forced" | — |
| 0.3 | 1 | 0x1DB9C (0x1D8BE) | bit 2 of 0x400EE5 | — |
| 0.4 | 1 | 0x1D624 (0x1D35E) | bit 0 of 0x400954 | — |
| 0.5 | 1 | 0x1D66A (0x1D3A4) | lamp arbiter id 0xB (bit 0 of 0x40085E) | — |
| 0.6 | 1 | 0x1D63C (0x1D376) | lamp arbiter id 1 (bit 2 of 0x40085C) | — |
| 0.7 | 1 | 0x1D654 (0x1D38E) | lamp arbiter id 0 (bit 0 of 0x40085C) | — |
| 1.0 | 1 | 0x1D988 (0x1D6DA) | the DSC warning lamp: 1 while arbiter ids 2 or 3 are on; pulsed through the counter 0x4007A3/0x4007A4 when id 7 requests it | — |
| 1.1 | 1 | 0x1D6A6 (0x1D3F8) | fault summary: any of 0x401065 bits 3..4, 0x400F30 bit 3, 0x400EE6 bit 0, 0x400EE7 bits 4..5, 0x400F90 bit 0, 0x40095A bit 0 | — |
| 1.2 | 1 | 0x1D94E (0x1D67A) | bit 0 of 0x400755, set by the speed getter below when two wheels are invalid | — |
| 1.3 | 13 | 0x1D8BC (0x1D5E8) | vehicle speed from the first two wheel blocks (0x401700 and 0x401740, speed at +0x18 clamped to 30000 and floored at the reference 0x4005B0): sum × 4 / 50 with both valid, × 4 / 25 with one, 0x1FFF with neither | — |
| 3 | 8 | 0x1D956 (0x1D6A8) | torque request byte 0x400789 | 2 |
| 4 | 8 | 0x1DAFC (0x1D83E) | torque request byte 0x40078B | 3 |
| 5 | 8 | 0x1D97A (0x1D6CC) | 0xFF unless the engine-controller variant byte 0x400796 is 0x11, then 0 | — |
| 6 | 8 | 0x1DAD8 (0x1D81A) | torque request byte 0x40078A | 4 |
| 7.1 | 1 | 0x1DDE0 (0x1DB2E) | driver mode 4 active (see `zcp_m_track_mode.md`) | 0xB |
| 7.2 | 1 | 0x1DA4E (0x1D79C) | above 1000 in the speed word 0x401068 or with all wheels invalid: latched 1 when 0x400E32 bits 3..5 are set and bit 4 of 0x400E30 is clear (bit 4 of 0x4007A2) | 0xC |
| 7.4 | 2 | 0x1D67C (0x1D3B6) | from the indication byte 0x4008D0 of driver-mode channel 0: 3 when its low 7 bits are 3, else its bit 7 | — |
| 7.6 | 1 | 0x1DCE2 (0x1DA08) | bit 4 of 0x400959 | — |
| 7.7 | 1 | 0x1D62C (0x1D366) | the port-input switch 0x171CA (bit 5 of 0x40063D) | — |

The three torque-request bytes and the flag byte 0x40078C are written by the CAN-side control function 0x1CA78 (584 instructions, 20% contained in the non-M's) from the control outputs 0x400EEC/0x400EEE and the engine data below; with the variant byte 0x11 it scales (request + base torque 0x40073C) × 8 / 5, a positive request goes to 0x400789 with flag bit 0, a negative one to 0x400789 inverted plus 0x40078B with flag bit 1, and an idle request writes 0xFF / 0 / 0.

### 0x1F0, 8 bytes, every tick: wheel speeds

Record 1, 5 signals. Four 13-bit fields at bytes 0, 2, 4 and 6, one per wheel block (0x401718, 0x401758, 0x401798, 0x4017D8, the speed word at +0x18 of each 0x40-byte block), getters 0x1D6F4, 0x1D728, 0x1D762, 0x1D79E (non-M 0x1D42C, 0x1D460, 0x1D4B8, 0x1D4F4): the larger of the wheel speed and the reference speed 0x4005B0, × 4 / 25, or 0x1FFF when the wheel's flag byte 0x4005BA..0x4005BD has bit 7 set or bit 5 of 0x400E30 is set. The fifth entry, 0x1D7F8 (0x1D532), is called with the frame buffer: it picks one of four wheel status bytes by the tick counter (bits 0..1 of 0x4005AC select 0x401733, 0x401770, 0x4017B0 or 0x4017F3), marks the selected wheel in the top bits of byte 1 (0x20, 0x40, 0x60, and 0x80 when both bits of 0x40090C are set), sets bit 7 of byte 7 unless the status is 0xFF, and spreads the inverted status bits over the top bits of bytes 3, 5 and 7. So the high three bits of each 16-bit wheel field are status, rotating through the wheels.

### 0x1F3, 8 bytes, every tick: sensor signals

Record 2, 6 signals.

| Byte.bit | Width | Getter ZCP (non-M) | Value |
|---|---|---|---|
| 0 | 16 | 0x1DBA6 (0x1D8C8) | from 0x4009A0: magnitude × 0x180 / 2^14 clamped to 511, in bits 6..14, sign in bit 15 (0x8000 = negative); 0x8001 once after bit 7 of 0x4005BE is raised (an initialisation marker that also loads 0x4007B2/0x4007B5), 0x8002 when 0x400E2E bit 4 is set or 0x40116C bit 1 is clear, 0x8000 in the factory window |
| 3 | 10 | 0x1DD76 (0x1DAA4) | from 0x40096A: magnitude × 0x20B / 2^14 clamped to 511, offset 0x1FF, 0x3FF when the word is 0x8000 |
| 4.2 | 1 | 0x1DC2A (0x1D96C) | bits 0..1 of 0x40095A non-zero |
| 4.6 | 10 | 0x1DCEC (0x1DA30) | from 0x40098E: magnitude × 0x345 / 2^12 clamped to 510, offset 0x200, 0x3FF on fault (0x400E2E bit 5 or 0x40116C bit 1 clear), 0x200 in the factory window |
| 7.0 | 1 | 0x1DC36 (0x1D978) | latched 1 when 0x400E32 bit 5 is set and 0x400E30 bit 4 clear, above 1000 in 0x401068 or all wheels invalid (bit 2 of 0x4007A2) |
| 7.3 | 1 | 0x1DCAE (0x1D9D4) | bit 0 of 0x400968 with all four wheel flags valid and 0x400E30 bit 5 clear |

Which physical quantity 0x4009A0, 0x40096A and 0x40098E hold (yaw rate, lateral acceleration, steering-derived values) is not pinned here; they are the control layer's sensor words.

### Spare capacity in the transmitted frames

All three transmitted frames are DLC 8, the CAN payload maximum, so none can be extended; the only room for an
extra signal is in the bytes/bits their signal lists leave unassigned. Mapping each:

- **0x1F0 (wheel speeds): full.** Four 16-bit fields (13-bit speed + 3-bit rotating status) fill all 8 bytes.
- **0x153 (DSC status): effectively full.** Only **byte 7 bit 0 and bit 3** are unassigned (2 bits).
- **0x1F3 (sensor signals): has the room.** Its six signals occupy bytes 0..1 (16), byte 3 + byte 4 bits 0..1
  (10), byte 4 bit 2 (1), byte 4 bits 6..7 + byte 5 (10), byte 7 bit 0 and byte 7 bit 3. Unassigned:
  - **byte 2 — entirely free (8 bits)**
  - **byte 6 — entirely free (8 bits)**
  - byte 4 bits 3..5 (3 bits)
  - byte 7 bits 1, 2, 4, 5, 6, 7 (6 bits)

  i.e. **two whole free bytes plus ~9 scattered bits (~25 free bits)**, all already transmitted (as zero/stale,
  since no getter writes them).

**Fitting brake pressure.** Brake pressure comes out over KWP (`21 06`) as two bytes per circuit, two circuits.
Placement options in the existing transmit set:

- **8-bit per circuit — fits with no new frame.** Put circuit 1 in **0x1F3 byte 2** and circuit 2 in
  **0x1F3 byte 6**: the two fully-free bytes, one per circuit, no bit-packing. Costs resolution (one byte per
  circuit instead of two).
- **~12-bit per circuit — also fits in 0x1F3.** ~24 bits (2 × 12) can be packed across byte 2, byte 6 and the
  scattered free bits (byte 4 bits 3..5, byte 7 bits 1,2,4,5,6).
- **Full 16-bit per circuit (as KWP reports it) — does not fit the existing frames.** Two 16-bit fields need
  32 bits; the total free space across all three transmit frames is only ~27 bits (0x1F3 ~25 + 0x153 2), and
  there are no four *contiguous* free bytes anywhere (0x1F3 offers two, non-adjacent). So both full values do
  not fit even bit-packed. A full-resolution pair would need a new transmitted message record in the
  table-driven layer (new identifier, DLC 4, transmit flag 0x08, period in record byte 6, a signal list, and
  signal-table getter entries), with the transmit scheduler 0x1C470's record range widened past 0..2.

**Brake-pressure source (traced).** A CAN getter for a new signal reads a RAM word; the brake-pressure words are
the ones the KWP `21 06` read returns. `21 06` is the SID-0x21 (readDataByLocalIdentifier) local-id-6 branch:
the SID-0x21 dispatcher `0x321BC` reads the requested local id at `*0x323FC` and jumps to `0x322CA` for id 6.
That handler returns four 16-bit RAM words — `0x400984`, `0x400988`, `0x40099E`, `0x40098C` (response bytes
0xA..0xB, 0xC..0xD, 0xE..0xF, 0x10..0x11) — plus four high-bytes from getter `0x1708E(0,1,2,0x13)` (bytes
6..9) and the byte `0x400906` (byte 0x12).

Of the four words, **`0x400984` and `0x400988` are the two brake-circuit pressures**. In the acquisition
`FUN_0x24D26` they are a matched pair: each is read, scaled in place by the same routine `0x24FB0`, and written
back (`0x24D32`/`0x24D3C` for `0x400984`, `0x24D54`/`0x24D5A` for `0x400988`), and `FUN_0x24D9C` immediately
after is a divergence/fault monitor over them — the signature of two instances of one measured quantity, one
per circuit, which is what "two bytes per circuit, four total" describes. The other response pair
(`0x40098C`/`0x40099E`) is scaled by a different routine `0x24FB8` and is a different quantity. So a `0x1F3`
getter would read **`0x400984` (front) and `0x400988` (rear), 16-bit signed each**; the 8-bit-per-circuit
option takes the high byte of each. Confirmed against a working Gauge.S `21 06` decode: front at response data
offset 6, rear at offset 8 (the handler's data payload starts 4 bytes into the buffer, so offset 6 →
buffer 0xA = `0x400984`, offset 8 → buffer 0xC = `0x400988`), value `signed(x,2) / 100` → bar, i.e. `0x24FB0`
scales to centibar (0.01 bar/count).

**Preferred full-resolution route: port the AWD variant's 0x1F8 frame.** The AWD non-M E46 MK60 reportedly
already transmits brake pressure on its own vehicle-bus frame **0x1F8** (unverified; to be confirmed by dumping
that module). `0x1F8` is not transmitted or received by the ZCP, so it is a free identifier here, and it sits in
the DSC cluster's id block (0x1F0/0x1F3/0x1F5). Because the CAN layer is table-driven and the record/list/signal
tables are byte-for-byte identical across these MK60 builds, that frame is portable: add its 12-byte message
record, its signal list, and its signal-table getter entries to the ZCP tables, point the getters at the ZCP's
brake-pressure words (`0x400984` front / `0x400988` rear), and widen the transmit scheduler `0x1C470`'s record
range past 0..2. This yields a dedicated 8-byte frame carrying both circuits at full 16-bit with the factory
getter/scaling and a standard identifier, rather than squeezing reduced-resolution fields into 0x1F3's spare
bits. It needs the AWD module dumped (to read its 0x1F8 record, list and getters) and is otherwise the same
table-addition. If factory-standard decoding is not required and only the data is wanted, the 0x1F3 spare-bit
options above still work without a second module.

## Received frames on the vehicle bus

The setters write the RAM variables the control layer reads; the timeout handler in slot n of the signal table fires when 0x40077C[n] counts down to zero.

| Id | DLC | Byte.bit → width | Setter ZCP (non-M) | Effect | Timeout (ticks) → handler |
|---|---|---|---|---|---|
| 0x316 | 8 | 0.1 → 1 | 0x1D042 (0x1CDCA) | flag bit 1 of 0x400742; timer 0x400790 started at 0x48 when the bit is set; 0x4007AA reloaded | 71 → 0x1DE90: fault 0xA0008 |
| | | 0.2 → 1 | 0x1D124 (0x1CE7E) | timer 0x40078E set to 0xFF when 1, started at 0x48 otherwise | |
| | | 0.4 → 2 | 0x1D13A (0x1CEA4) | only with variant byte 0x11: bits 1..2 of 0x400741, timer 0x40078D | |
| | | 1 → 8 | 0x1D280 (0x1CFA6) | 0x40073A = value × 0x400788 × 32 / 51 (× 64 / 164 for other variants) minus 0x40073C | |
| | | 2 → 16 | 0x1D09C (0x1CDF6) | engine speed: 0x400734 = value × 16 / 51, its filtered derivative 0x400732 using the period 0x4007A8; sets bit 5 of 0x40078C | |
| | | 4 → 8 | 0x1D1E6 (0x1CF38) | 0x400738 = scaled value minus 0x40073C | |
| | | 5 → 8 | 0x1D222 (0x1CF74) | 0x40073C = scaled value (the base torque used by the request builder) | |
| 0x329 | 8 | 0 → 8 | 0x1CFB2 (0x1CD3A) | variant byte: bits 6..7 clear → 0x400796 = low 6 bits, accepted values 0x11 and 0x36 (0x36 starts the 0x235 timeout 0x400781 at 0x47), anything else sets bit 1 of 0x400755 and posts fault 0xA0002; bits 6..7 both set → 0x400788 = low 6 bits (the torque scale) | 71 → 0x1DE6A: fault 0xA0008 |
| | | 5 → 8 | 0x1D2BC (0x1CFE2) | 0x400736 = value; 0xFF marks it invalid (bits 2..3 of 0x400742, timer 0x400792) | |
| 0x1F5 | 8 | 0 → 16 | 0x1D450 (0x1D18A) | steering angle 0x4007AE, sign-magnitude (bit 15), 0x8000 = invalid (bit 4 of 0x4007B8) | 14 → 0x1DEE0 |
| | | 2 → 16 | 0x1D496 (0x1D1E6) | steering rate 0x4007B0, same coding, invalid flag bit 5 of 0x4007B8 | |
| | | 4 → 8 | 0x1E056 (0x1DD62) | a bare `jmp r15` return stub in both images (the halfword 0x00CF after the gear setter's pool): the byte is extracted and discarded | |
| | | 5.0 → 4 | 0x1D4C0 (0x1D210) | sensor status: values 0..1 → 0, 2 → 1, 3 → 2, else 3, in bits 0..1 of 0x4007B7 | |
| | | 5.4 → 4 | 0x1D52C (0x1D24E) | alive counter 0x4007B4: an unchanged value clears the frame's received bit and sets bits 4..6 of 0x4007B8 | |
| 0x43F | 8 | 0.0 → 3 | 0x1D386 (0x1D0C6) | gear through 0x1E02A into 0x400748 or 0x400749 (7 is mapped to 8), only when the transmission type 0x40073F is 1 or 5 | 71 → 0x1DEA4 |
| | | 0.3 → 1 | 0x1D364 (0x1D0A4) | bit 0 of 0x40074B, same gate | |
| | | 0.6 → 2 | 0x1D3F0 (0x1D140) | bit 3 of 0x400741 (values 0..1 → 1, 2 → 0), bit 5 of 0x400743 when bit 6 of 0x40078C is set | |
| | | 5.2 → 1 | 0x1D2F8 (0x1D060) | bit 6 of 0x40078C; fault 0xC0002 when it contradicts the transmission type (1 expects 1, 5 expects 0) | |
| 0x43D | 8 | 0.0 → 3 | 0x1D3DC (0x1D12C) | gear through 0x1E02A, only when 0x40073F is 6 | 71 → 0x1DEC6 |
| | | 0.3 → 1 | 0x1D39E (0x1D0DE) | bit 0 of 0x40074B when the type is 6, otherwise fault 0xC0002 | |
| 0x235 | 4 | 0.4 → 2 | 0x1D19E (0x1CEF6) | only with variant byte 0x36: bit 2 of 0x400741 when the value is 3, timer 0x400793 | 71 → 0x1DEA0 |
| 0x615 | 8 | 1.1 → 1 | 0x1D5FC (0x1D336) | with coded feature 3: bit 0 of 0x40072A, the CAN input of the second driver function (channel 1 of the mode logic); clears bit 7 of 0x40072D | 142 → 0x1DF50: sets that bit, fault 0xB0004 |
| 0x613 | 8 | 0 → 16 | 0x1D5CE (0x1D314) | 0x400798 = value unless 0xFFFF (bit 7 of 0x4007A1 marks invalid) | 142 → 0x1DF2A: fault 0xB0004 |
| 0x610 | 8 | whole frame | 0x1D562 (0x1D284) | the first frame after power-up is latched into 0x40079C..0x4007A0 (five bytes, the first compared on its high nibble only); later frames that differ set bit 0 of 0x4007A2. The latch is primed at init (0x1DFB0) from EEPROM block 6 through the synchronous read 0x1633C, so this is an identity check against a stored value | 28 → 0x1DEF0: re-arms the mailbox up to five times, then faults 0xB0004 and 0xB0002 |

The transmission type 0x40073F (1 or 5 reading 0x43F, 6 reading 0x43D) is chosen in 0x1CA78 from coding; 0x39268 reads it for the control layer. The variant byte 0x400796 (0x11 or 0x36) selects the scaling of the torque bytes in both directions and which of 0x235 or the 0x316 flags is used; its timeouts raise 0xB0040 (0x1DF68/0x1DF7E). Two more timers in the CAN control function raise 0xB0010 (0x1DF5C, 0x40078E) and 0xA0020 (0x1DF60, 0x400790).

The identifiers are consistent with the publicly documented E46 powertrain-bus assignment (0x316 and 0x329 engine, 0x1F5 steering-angle sensor, 0x43F and 0x43D automatic and sequential gearbox, 0x153 and 0x1F0 from the DSC, 0x61x from the instrument cluster); that mapping is outside knowledge and not derived from the image.

## The sensor bus

The second controller instance is driven by 0x34A28 (non-M 0x33BC4, both called from the 10 ms body), with its own state byte 0x400EBA, bus-off counter 0x400EBC (fault 0xA0200 past 14), scheduler 0x34CD4 (one transmit record), receive poll 0x34EAE (records 1..3), timeout slots 0x34B84 and tables at 0x3FE11 (records), 0x3FE41 (lists) and 0x3FED0 (signals); non-M 0x3F671, 0x3F6A1, 0x3F730. The ported name `cluster_sensor_get` 0x35078 (0x34214), identical to the E9x M3's by signature, reads the six words this bus delivers, which places it: this is the DSC sensor cluster, the yaw-rate and lateral-acceleration unit, on a private bus.

| Id | DLC | Direction | Content |
|---|---|---|---|
| 0x002 | 2 | transmit, flags 0x88 | 16 zero bits (getter 0x35358 returns 0): a wake or synchronisation frame |
| 0x130 | 8 | receive, timeout 14 → 0x3535C: fault 0xA1000 | bytes 0..1, 2..3, 4..5 → words 0x400ECA, 0x400ECE, 0x400ED2 (`cluster_sensor_get` 0, 1, 2); byte 6 bits 0..1 → bits 0..1 of 0x400EB4 (setter `fcan_rx_0D1_q1` 0x35298), bits 2..3 → bits 2..3 (0x352AA), bits 4..7 → alive counter 0x400ED6 (0x3526E; a repeat drops the frame); byte 7 → CRC-8 over bytes 0..6 through the table 0x3FD01 (0x35420, checked by 0x3524C) |
| 0x131 | 8 | receive, timeout 14 → 0x35360: fault 0xA1000 | bytes 0..1, 2..3, 4..5 → 0x400ECC, 0x400ED0, 0x400ED4 (`cluster_sensor_get` 3, 4, 5); byte 6 bits 0..1 → bits 4..5 of 0x400EB4 (0x352E8), bits 2..3 → bits 6..7 (`fcan_rx_0CD_q0` 0x352FC), bits 4..7 → alive counter 0x400ED7 (0x352BE); byte 7 CRC |
| 0x132 | 8 | receive, timeout 14 → 0x35364 | byte 0 low nibble → 0x400EDB, high → 0x400EDA, byte 1 low → 0x400EDC, high → 0x400ED9, byte 2 → 0x400EDE, byte 3 → 0x400EE0, byte 4 → 0x400EE2, byte 5 → 0x400621, byte 6 bits 0..3 → bits 0 and 3 of 0x400EB6 (0x35338), bits 4..7 → alive counter 0x400ED8 (0x3530E); byte 7 CRC |

0x35174 copies the six words into the byte block 0x400615..0x400620 low byte first whenever bit 3 of 0x400EB6 is set, and 0x353C6 hands that block out after checking that the inverted sum of its twelve bytes equals 0x400621 (byte 5 of 0x132): a second checksum over the delivered sensor data. The names `fcan_rx_0D1_q1` and `fcan_rx_0CD_q0` were ported from the E9x M3 by exact signature; there they belong to chassis-CAN frames 0x0D1 and 0x0CD, here the same two-bit setters serve the sensor-cluster frames, so the names are misleading on this image and should be read as "2-bit status setter".

## Faults raised by the CAN layer

| Fault word | Raised by | Meaning |
|---|---|---|
| 0xA0001 / 0xA0100 | 0x159E4 | controller 1 / 2 failed to initialise |
| 0xA0002 | 0x1C1F0, 0x1CFB2 | bus-off counter over 29; unknown engine-controller variant byte |
| 0xA0008 | 0x1DE90, 0x1DE6A | 0x316 or 0x329 missing for 71 ticks |
| 0xA0020 | 0x1DF60 | the 0x400790 timer (0x316 byte 0 bit 1) expired |
| 0xA0200 | 0x34A88 | sensor-bus bus-off counter over 14 |
| 0xA1000 | 0x3535C, 0x35360 | 0x130 or 0x131 missing for 14 ticks |
| 0xB0002, 0xB0004 | 0x1DEF0, 0x1DF50, 0x1DF2A | 0x610 missing after five re-arms; 0x615 / 0x613 missing for 142 ticks |
| 0xB0010 | 0x1DF5C | the 0x40078E timer (0x316 byte 0 bit 2) expired |
| 0xB0040 | 0x1DF68, 0x1DF7E | variant-specific timeout |
| 0xC0002 | 0x1D2F8, 0x1D39E | gearbox frame contradicts the coded transmission type |

The fault word is (latch index << 16) | bit, posted through 0x32F94 into the latch array at 0x400E38; whether and how these map to KWP trouble codes is not traced.

## Non-M comparison

The twelve vehicle-bus message records, the sensor-bus records, lists and signal table, and the 0x153 signal list are byte for byte identical between the two images (verified by decoding both); the getters and setters are the same routines at addresses shifted by the image's layout, and the ones checked (0x1DDE0/0x1DB2E, the sensor setters) decompile the same. The 0x1F0, 0x1F3 and receive lists were decoded for the ZCP and their non-M routine addresses taken from the non-M signal table at the same indices, not re-read; the non-M task, scheduler and control-function addresses come from the non-M 10 ms body's call list and the pool words that name the non-M tables; the CAN control function 0x1CA78 (non-M 0x1C854) is one of the restructured functions of the ZCP, so the torque-request arithmetic may differ in detail between the builds.

## What is not pinned

- Message record bytes 8 and 9, and byte 5 flag bits other than 0x08.
- The physical meaning and units of the 0x1F3 words 0x4009A0, 0x40096A, 0x40098E and of the six sensor-cluster words; the speed scaling of 0x1F0 and 0x153 (× 4 / 25 of the wheel-block speed) in km/h.
- Whether 0x7FFC80 is a second CAN module or an alias of the first; the ZCP image's discovered MMIO pages do not include it.
- The meaning of 0x400EE4..0x400EE7, 0x400954, 0x400959, 0x40095A and the other control-state bits reported in 0x153 byte 0 and 7; they are reported as found.
- The tick period (10 ms assumed) and so the absolute timeouts.

## Method and provenance

Same scratch snapshots and whole-program decompile and listing dumps as `zcp_m_track_mode.md`. The record, list and signal tables were decoded from the raw `.bin` of both images with the layouts taken from the routines that walk them (0x1C470, 0x1C596, 0x1C658, 0x1C720, 0x1C364, 0x34CD4, 0x34EAE, 0x34C3C); the bit-offset argument of the packer was confirmed in the listing at 0x1C5F8 and 0x1C772 after the decompiler had folded it to zero. Every getter and setter named above was read in the decompiled text; the value column paraphrases it. Nothing in either project was modified and no name was applied; *proposed* names are suggestions only. Companion documents: `zcp_m_track_mode.md`, `zcp_mk60_image.md`, `nonm_mk60_image.md`.
