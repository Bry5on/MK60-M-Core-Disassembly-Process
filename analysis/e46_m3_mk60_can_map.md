# E46 M3 non-ZCP MK60 — CAN map on the 818.3 image

2026-11-20. Read-only analysis of the 818.3 (non-ZCP E46 M3) Continental Teves MK60 image. This is a map of both CAN controllers, every receive and transmit identifier, and the functions that unpack or pack them. It is not a procedure for writing a BIN or a car, and it is not a recipe for transmitting on the vehicle bus.

Companion documents: `zcp_can_frames.md` (ZCP twin of the same layer; several RAM addresses are shifted), `e46_m3_mk60_traction_control.md` (TCS slip chain and the `0x153` torque request), `e46_m3_mk60_mtrack_mode4.md` (what live M-track changes; the mode word is not written from CAN), `e46_m3_mk60_gauge_s_validation.md` (Gauge.S predictor and channel check against this map), `e46_m3_mk60_kline_access_and_flash.md` (workshop diagnostics stay on K-line), `e46_m3_mk60_818_3_function_names.md` (named functions and RAM, with `e46_m3_mk60_818_3_names.csv`).

Addresses below are 818.3 unless a ZCP twin is named. Confidence: **[C]** confirmed in this image’s tables or decompiled text; **[I]** inferred from a clear data-flow or a twin in the public ZCP write-up, with 818.3 RAM confirmed; **[?]** still open.

**818.3 RAM is not a 1:1 copy of the ZCP map.** Message identifiers, periods, timeouts and signal *layouts* match; packed-torque bytes, network-state, steer, engine speed and sensor-cluster words are shifted. Values below were read from this image. Do not port ZCP RAM addresses blindly.

## Two controllers, one table-driven layer

| Bus | Controller base | Role | Frames |
|---|---|---|---|
| Vehicle (powertrain / chassis) | `0xFFFC80` **[C]** | 3 TX + 9 RX | TX `0x153`, `0x1F0`, `0x1F3` every 10 ms. RX `0x316`, `0x329`, `0x1F5`, `0x43F`, `0x43D`, `0x235`, `0x615`, `0x613`, `0x610`. **No `0x545`** in this image. |
| Sensor (private) | `0x7FFC80` **[C]** | 1 TX + 3 RX | TX `0x002` (2-byte wake / sync, flags `0x88`). RX `0x130`, `0x131`, `0x132` from the yaw / ay cluster, timeout 14 ticks. |

Whether `0x7FFC80` is a second CAN module or an alias of the first is still **[?]** (the same open item as in `zcp_can_frames.md`). The firmware treats them as two instances: `can_controller_init` (`0x158B4`) takes a bus index and selects the base, with a per-bus state byte at `0x401134` / `0x401136`.

The 10 ms body runs the vehicle-bus task and a separate sensor-bus task. A tick is that 10 ms body **[C]** as the design assumption used everywhere in this map; the hardware timer was not re-traced for this note.

Message records are 12 bytes:

- bytes 0..3 identifier (big-endian)
- byte 4 DLC
- byte 5 flags (`0x08` = TX; `0x01` / `0x02` / `0x06` / `0x88` also occur and are only partly decoded **[I]**)
- byte 6 TX period in 10 ms ticks
- byte 7 RX timeout in ticks
- bytes 8..9 not decoded **[?]**
- byte 10 signal count, byte 11 first list index

A 16-byte **header** sits immediately before the records (`can_veh_cfg_header` `0x3DCB2`, `can_sens_cfg_header` `0x3F7CD`). Byte `0xE` is the TX count (3 / 1), byte `0xF` the RX count (9 / 3). Init walks `byte 0xE + byte 0xF` 12-byte records to program mailboxes.

Signal-list entries are 4 bytes (width, `(bit << 4) | byte`, unused, signal index). Signal-table entries are 8 bytes. Type in the **low 3 bits**: `0` call a getter/setter, `1` RAM word, `2` call with the frame buffer, `3` constant, `4` reload a receive timeout. TX getters often appear as type `8` (bit 3 set); some timeout slots are type `0x44` / 196. The packer masks `& 7`.

## Driver, mailboxes, filters, bit timing

The low-level driver lives at `0x3B65A`–`0x3C3F4`. Mailboxes are **16 bytes** starting at **controller + `0x80`**, 16 objects on the common variant or 32 if `can_controller_is_32slot` (`0x15412`) says so **[I]**. Layout **[C]** from the decompiled accessors:

| Offset in mailbox | Meaning |
|---|---|
| `+0x80` | control / timestamp-ish byte |
| `+0x81` | command/status nibble in the high 4 bits: `0x4` read, `0x8` program, `0xA` transmit, `0xC` abort **[I]** for the exact vendor names, **[C]** for the values used |
| `+0x82`, `+0x84` | identifier (11-bit and 29-bit packing) |
| `+0x86`–`+0x8D` | 8 data bytes |

Acceptance / enable bits are in controller words at **base+`0x0E`** and **base+`0x22`**. `can_hw_mailbox_program` (`0x3BBC4`) writes the ID from the message record and sets those mask bits. Vehicle RX records 3..11 use mailboxes 3..11 (`can_veh_mailbox_map` `0x3DE56`); the three TX frames use mailboxes 0..2 **[C]**.

**Bit timing** is programmed by `can_hw_bit_timing_set` (`0x3BD08`) into controller bytes **base+7, +8, +9**, in (field−1) form for BRP / TSEG1 / TSEG2 / SJW **[C]** as a register recipe. The 4-byte source config is passed in on the stack from init; the exact flash constants (header bytes `0x0C`..`0x0D` are a candidate: vehicle `0x94 0x41`, sensor `0x1A 0x1D`) were not pinned **[?]**. `can_hw_soft_reset` (`0x3BE78`) puts the controller in a reset/init state (control bit `0x200`, clear bit `0x80`, write `0x10` at base+7) and zeros object RAM. `can_hw_init_mode` (`0x3BB50`) sets or clears control bit `0x1000`.

`can_hw_mailbox_ram_test` (`0x3B9C0`) walks each mailbox data window with `0xAA` then `0x55`. Failure becomes fault **0xA0001** (bus 1) or **0xA0100** (bus 2) in `can_controller_init`.

**Interrupts vs poll.** The 10 ms tasks are the primary pump: they poll mailbox status through `can_mailbox_xfer` / `can_hw_status_decode` (`0x3BAE0` reads **base+`0x20`**, including a bus-off bit). Nearby routines touch other MMIO (`0xFFFA1x`, `0xFFFB20`, `0xEF8000`) that looks like a shared interrupt or timer block, not a dedicated CAN ISR. No standalone “CAN IRQ handler” was named **[I]** — treat the bus as **software-polled from the 10 ms task**, with hardware status bits at base+`0x20`.

TX path: scheduler `0x1C2E4` → packer `0x1C40A` → `0x1568C` / `can_hw_tx_write` (`0x3B878`). RX path: poll `0x1C4CC` → `can_mailbox_xfer` + `can_hw_rx_read` (`0x3B8C8`) → unpacker `0x1C594` → setter.

## Vehicle-bus layer RAM and tasks

| Element | 818.3 | Notes |
|---|---|---|
| Config header / records / lists / signals | `0x3DCB2` / `0x3DCC2` / `0x3DD52` / `0x3DE98` | 12 records, 65 list entries, 65 signal slots |
| Network state | `can_net_state` `0x40074D` | bits 0..2 state, 3 re-init, 4 bus up, 6 TX inhibit, 7 fault. **Not** the ZCP’s `0x400751` |
| Bus-off counter | `0x400775` | `can_veh_net_task` (`0x1C064`) posts **0xA0002** when `> 29` |
| Per-message status | `0x400755[12]` | bit 0 = received this tick |
| TX period counters | `0x400765[i]` | compared to record byte 6 (1 for all three TX → every tick) |
| RX timeout counters | `0x400778[slot]` | slots 0..8 of the signal table; type-4 reloads from the entry’s timeout byte |
| Net task | `can_veh_net_task` `0x1C064` | state machine + bus-off |
| Timeout / TX count | `can_veh_timeout_and_txcount` `0x1C1D8` | |
| TX scheduler | `can_veh_tx_scheduler` `0x1C2E4` | records 0..2 |
| RX poll | `can_veh_rx_poll` `0x1C4CC` | records 3..11 |

## Vehicle-bus TX

All three frames are DLC 8, period 1 tick (10 ms), flags `0x08`.

### 0x153 — DSC status / torque request (21 signals)

| Byte.bit | Width | Getter | Source / value |
|---|---|---|---|
| 0.0 | 1 | `0x1D994` | `can_153_torque_flags` `0x400788` bit 0 — positive torque request **[C]** |
| 0.1 | 1 | `0x1D9BA` | `0x400788` bit 1 — negative / drag request **[C]** |
| 0.2 | 1 | `0x1D884` | `can_mdm_status_gate` `0x400EB4` bit 6 and `tcs_status_eb6` bits 4..5 clear **[C]** |
| 0.3 | 1 | `0x1DA10` `can_153_get_dsc_active` | `0x400EB5` bit 2, whose only writer is `dsc_active_153_flag_update` (`0x383A8`). Outside TCS it is `(0x400932 & 3) != 0`, meaning an AYC yaw controller (OS or US) or the torque-path flag is active. During TCS (`0x400EB6` bit 0) it is a self-latched TCS flag. This is a controller state, not valve activity. It stays on through hysteresis (hold bar = 0.5 × entry bar) plus a 26-tick (0.26 s) release counter; see `e46_m3_mk60_dsc_active_dwell.md` **[C]** |
| 0.4 | 1 | `0x1D498` | `0x40092C` bit 0. Likely the brake-light switch **[I]** |
| 0.5 | 1 | `0x1D4DE` | lamp arbiter **[I]** (the decompiler lost the id) |
| 0.6 | 1 | `0x1D4B0` | lamp arbiter **[I]** |
| 0.7 | 1 | `0x1D4C8` | lamp arbiter **[I]** |
| 1.0 | 1 | `0x1D7FC` | DSC warning lamp / pulse counters `0x40079F` / `0x4007A0` **[I]** |
| 1.1 | 1 | `0x1D51A` | fault summary: `0x401035` bits 3..4, `0x400F00` bit 3, `tcs_status_eb6` bit 0 and bits 4..5, `0x400F60` bit 0, `0x400932` bit 0 **[C]** |
| 1.2 | 1 | `0x1D7C2` | `0x400751` bit 0, set by the speed getter when two wheels are invalid **[C]** |
| 1.3 | 13 | `0x1D730` | vehicle speed from the first two wheel blocks, `× 4/50` with both valid, `× 4/25` with one, `0x1FFF` with neither; **0.0625 km/h/count** **[C]** |
| 3 | 8 | `0x1D7CA` | `can_153_torque_b0` `0x400785` **[C]** |
| 4 | 8 | `0x1D970` | `can_153_torque_b2` `0x400787` **[C]** |
| 5 | 8 | `0x1D7EE` | `0x00` when `can_variant_byte` is `0x11`, else `0xFF` **[C]** |
| 6 | 8 | `0x1D94C` | `can_153_torque_b1` `0x400786` **[C]** |
| 7.1 | 1 | `0x1DC54` | MDM / mode-4 **status** (`can_153_mdm_status_bit`), from `_is_mdm` — not from CAN `0x615` **[C]** |
| 7.2 | 1 | `0x1D8C2` | latched 1 above 1000 in `0x401038`, or with all wheels invalid **[I]** |
| 7.4 | 2 | `0x1D4F0` | driver-mode indication **[I]** (RAM pool lost in the decomp) |
| 7.6 | 1 | `0x1DB56` | `0x400931` bit 4 **[C]** |
| 7.7 | 1 | `0x1D4A0` | port-input switch **[I]** |

Byte 7 bits 0 and 3 are unused. The *only* producer of the torque bytes and of bits 0..1 of byte 0 is `can_153_torque_request_pack` (`0x1C8EC`), described in `e46_m3_mk60_traction_control.md`. A few CAN-layer maintainers set or clear other bits of `0x400788` (the RPM setter sets bit 5; the `0x43F` setter writes bit 6).

### 0x1F0 — wheel speeds (5 signals)

Four 13-bit speeds at bytes 0, 2, 4, 6 (FL, FR, RL, RR) plus a type-2 frame-buffer getter `can_1f0_pack_wheel_status` (`0x1D66C`) that rotates a 3-bit status through the high bits of those 16-bit fields, selected by `0x4005AC` bits 0..1. Masking to 12 bits (`x & 4095`) then `× 0.0625` km/h is correct below 256 km/h (the 13th bit is only needed above that) **[C]**.

Producers: the wheel-acquisition block (speed at +0x18 of each 0x40-byte wheel object at `0x401700` / `0x401740` / `0x401780` / `0x4017C0`), not a CAN setter. Invalid: flag `0x4005BA+i` bit 7 or `0x400E00` bit 5 → `0x1FFF`. Reference speed `0x4005B0` is the floor used by the `0x1F0` and `0x153` speed getters: they transmit `max(wheel, 0x4005B0) × 4 / 25`.

### 0x1F3 — yaw / ay / ax (6 signals)

| Field | Packing | Source | LSB |
|---|---|---|---|
| Yaw | 16 bits at byte 0, sign-magnitude in bits 6..15 | `0x400978` (written by `0x2559C`) | **0.12223 deg/s** **[C]**. Not `r_ref` (`0x400972`) |
| Lateral G | 10 bits at byte 4 bit 6, offset 512 | | **0.004894 g** **[C]** |
| Longitudinal G | 10 bits at byte 3 bit 0, offset **511**, `0x3FF` invalid | | **0.003193 g** **[I]** |
| Byte 4 bit 2, byte 7 bits 0 and 3 | status flags | getters `0x1DA9E`, `0x1DAAA`, `0x1DB22` | |

Spare capacity: byte 2 and byte 6 are unused, plus scattered bits (the 818.3 list matches the public ZCP spare-capacity note). The six cluster words behind this frame come from the private sensor bus (`cluster_sensor_get` `0x34BC0` reads `0x400E9A`–`0x400EA4`).

## Vehicle-bus RX

Timeouts are in 10 ms ticks. There is **no `0x545`** record.

### 0x316 — DME, DLC 8, timeout 71 (710 ms), 8 signals, flags `0x02`

| Byte.bit | Width | Setter | 818.3 effect |
|---|---|---|---|
| 0.1 | 1 | `0x1CEB6` | bit 1 of `can_316_flags` `0x40073E`; timer `0x40078C` / reload `0x4007A6` |
| 0.2 | 1 | `0x1CF98` | timer `0x40078A` = `0xFF` when 1, else started at `0x48` |
| 0.4 | 2 | `0x1CFAE` | only if variant is `0x11`: bits in `0x40073D`, timer `0x400789` |
| 1 | 8 | `0x1D0F4` | `0x400736` = scaled value − `torque_base`. Scale: variant `0x11` → `× can_torque_scale × 32 / 51`, else `× scale × 64 / 164` |
| 2 | 16 | `0x1CF10` | `engine_rpm` `0x400730` = `value × 16 / 51`; derivative `0x40072E`; sets bit 5 of `0x400788` |
| 4 | 8 | `0x1D05A` | `0x400734` = scaled − `torque_base` (same scale as byte 1) |
| 5 | 8 | `0x1D096` | `torque_base` `0x400738` = scaled value |
| (timeout) | | `0x1DD04` | fault **0xA0008** |

`0x400730` / `0x400734` / `0x400736` / `0x400738` are the DME words TCS and the packer actually use (`e46_m3_mk60_traction_control.md`). `yaw_os_us_bars_update` also reads `0x400734` and `0x400738`. Internal RPM units are about twice a public 0.15625 rpm/bit bus scale (roughly 0.5 rpm/count) **[I]**. On the ZCP, engine speed sat at `0x400734`; do not use that address as RPM on 818.3.

### 0x329 — DME, DLC 8, timeout 71, 3 signals, flags `0x02`

| Byte.bit | Width | Setter | Effect |
|---|---|---|---|
| 0 | 8 | `0x1CE26` | bits 6..7 clear: `can_variant_byte` `0x400792` = low 6 bits, accepted `0x11` and `0x36` (`0x36` starts the `0x235` timeout); anything else posts **0xA0002**. Both bits set: `can_torque_scale` `0x400784` = low 6 bits |
| 5 | 8 | `0x1D130` | `dme_tps_byte` `0x400732` = value; `0xFF` is invalid (bits 2..3 of `0x40073E`, timer `0x40078E`) |
| (timeout) | | `0x1DCDE` | fault **0xA0008** |

The module does **not** unpack `0x329` byte 1 (coolant), byte 2 (ambient pressure) or byte 6 (a public brake bit). Those may still appear in a vehicle-bus log; they are not DSC inputs.

### 0x1F5 — steering-angle sensor, DLC 8, timeout 14 (140 ms), flags `0x00`

| Byte.bit | Width | Setter | Effect |
|---|---|---|---|
| 0 | 16 | `0x1D2C4` | `steer_angle` **`0x4007AA`**, sign-magnitude (bit 15), `0x8000` invalid (bit 4 of `0x4007B4`). **Not** the ZCP’s `0x4007AE` — on 818.3 `0x4007AE` is a `0x1F3` init marker |
| 2 | 16 | `0x1D30A` | `steer_rate` `0x4007AC`, same coding, invalid flag bit 5 of `0x4007B4` |
| 4 | 8 | `0x1DECA` | **`jmp r15` stub** — byte extracted and discarded **[C]** |
| 5.0 | 4 | `0x1D334` | mapped into bits 0..1 of `0x4007B3` |
| 5.4 | 4 | `0x1D3A0` | alive counter `0x4007B0`; an unchanged value clears the frame’s received bit |
| (timeout) | | type-4 slot | handler stub not created as its own function in this recovery |

A signed sign-magnitude decode of the form “if the raw word is above 32768, `(x − 32768) × 0.044`, else `−0.044 × x`” matches this encoding and inverts the ISO sign. 0.044 vs 0.04375 remains **[?]**. Consumer of `0x4007AA`: the vehicle-model / `r_ref` path.

### 0x43F — automatic gearbox, DLC 8, timeout 71

Gated on `trans_type` `0x40073B` ∈ {1, 5}. Gear at byte 0 bits 0..2 (`0x1D1FA`); byte 0 bit 3 (`0x1D1D8`); byte 0 bits 6..7 (`0x1D264`); byte 5 bit 2 (`0x1D16C`) writes bit 6 of `0x400788` and posts **0xC0002** if it contradicts the coded type. Timeout handler in the type-4 slot.

### 0x43D — sequential gearbox, DLC 8, timeout 71

Same gear / bit 3 when `trans_type` is 6; otherwise `0x1D212` posts **0xC0002**.

### 0x235 — DLC 4, period 2, timeout 71

Only with variant `0x36`: byte 0 bits 4..5, setter `0x1D012`, timer `0x40078F`.

### 0x615 — instrument cluster (ICL3), DLC 8, timeout 142 (1.42 s)

The receive record’s period field is 28 (280 ms). That is the table’s TX-period slot and is not necessarily the expected RX period. Public E46 CAN documentation names this frame **ICL3**, sent by the **instrument cluster** (Kombi / IKE, not IHKA) at **200 ms**. Byte 1 bit 1 is **LV_TOW** (trailer operation mode). Byte 1 bit 2 is interior night lighting.

This image unpacks **only byte 1 bit 1**. `can_rx_615_mdm_bit` (`0x1D470`) calls `coding_feature_get(3)` (`0x3B158`). Only if that returns 1 — `coding_feature3_bit` (`0x3B57C`), bit 9 of the halfword at `0x401016+0x14` (`coding_word_plus14` `0x40102A`) — does it write the extracted bit into `can_615_mdm_input` (`0x400726`) bit 0 and clear bit 7 of `can_615_timeout_flag` (`0x400729`). If feature 3 is not set, the CAN bit is ignored.

`can_timeout_615` (`0x1DDC4`) sets 0x400729 bit 7 and posts **0xB0004**. It does not clear or change 0x400726. The `0x153` packer, when feature 3 is off, forces the 0x615 timeout slot to `0xFF` and also sets 0x400729 bit 7; when feature 3 is on, it re-arms that slot.

Ambient temperature at byte 3, night lighting at byte 1 bit 2, and displayed speed at byte 6 are **not** DSC inputs. A logger on the vehicle bus may still see them.

#### 0x615 does not drive M-track **[C]**

0x400726 is a write-only location. The only 32-bit immediate of that address in the image is the setter’s literal pool. Nothing in this firmware reads the byte, so it does not gate `driver_mode_get`, the mode word `0x400868`, the cached flag `0x401015`, `_is_mdm`, or `0x153` byte 7 bit 1.

The mode word is written only by `driver_mode_init` (`0x21870`) and by the two-channel button state machine (`driver_mode_button_sm` `0x218BA` → `driver_mode_table_lookup` `0x217C0`). Channel 0, the only channel `_is_mdm` reads, is the **hardwired DSC / M button**: `dsc_button_gpio_get` (`0x170C6`) looks at GPIO shadow `0x40063A` bit 6 (pressed when set), copied from the port by `gpio_input_shadow_copy` (`0x1704A`). Channel 1 is a **different hardwired pin** (`driver_mode_ch1_gpio_get` `0x1711C`, `0x40063B` bit 6, pressed when clear), used for the second optional coded function (modes 8–11), not for M-track and not for this CAN bit. KWP status, when feature 3 is coded, also reports that GPIO getter, not 0x400726.

The physical button does not have to agree with 0x615 byte 1 bit 1; they are independent, and only the button sets the mode. A timeout or a missing 0x615 frame posts 0xB0004 and sets 0x400729 bit 7; it does not change the mode. Details of the GPIO path are in `e46_m3_mk60_mtrack_mode4.md`.

The catalog names `can_rx_615_mdm_bit` / `can_615_mdm_input` predate this trace; “mdm” in those names is misleading.

### 0x613 — instrument cluster, DLC 8, period 28, timeout 142

Bytes 0..1 → `0x400794` unless `0xFFFF` (invalid bit 7 of `0x40079D`). Timeout `0x1DD9E` posts **0xB0004**.

### 0x610 — identity / IKE, DLC 8, flags `0x01`, timeout 28 (280 ms)

Whole-frame setter `can_rx_610_ident` (`0x1D3D6`, type 2): the first frame after power-up is latched into `0x400798`… (five bytes, first compared on its high nibble only). Later frames that differ set a flag in `0x40079D`. The latch is primed at init from EEPROM block 6, so this is an identity check against a stored value **[C]**. Timeout `0x1DD64` re-arms the mailbox up to five times, then posts **0xB0004** and **0xB0002**.

## Sensor bus

Header `0x3F7CD`, records `0x3F7DD`, lists `0x3F80D`, signals `0x3F89C`. State `can_sens_net_state` `0x400E8A`, timeout slots `0x400E93`, TX counters `0x400E96`. Task pieces: `can_sens_timeout_and_txcount` (`0x346CC`), `can_sens_tx_scheduler` (`0x3481C`), `can_sens_rx_poll` (`0x349F6`, mailbox bus index 2).

| Id | DLC | Dir | Content |
|---|---|---|---|
| 0x002 | 2 | TX, flags `0x88`, period 255 | 16 zero bits, getter `0x34EA0` returns 0: wake / sync **[C]** |
| 0x130 | 8 | RX, timeout 14 | bytes 0..1, 2..3, 4..5 → `0x400E9A`, `0x400E9E`, `0x400EA2` (`cluster_sensor_get` 0, 1, 2). Byte 6 holds 2-bit status fields and an alive nibble; byte 7 is CRC (type-2 `0x34D94`). Timeout `0x34EA4` posts **0xA1000** |
| 0x131 | 8 | RX, timeout 14 | bytes 0..1, 2..3, 4..5 → `0x400E9C`, `0x400EA0`, `0x400EA4` (`cluster_sensor_get` 3, 4, 5). Same byte-6 / CRC pattern. Timeout `0x34EA8` posts **0xA1000** |
| 0x132 | 8 | RX, timeout 14 | nibbles and bytes into `0x400EA9`–`0x400EB2` and `0x400620`. Timeout pointer `0x34EAC` (not a defined function in this recovery; recorded as a label) |

Two already-named E9x ports, `fcan_rx_0D1_q1` and `fcan_rx_0CD_q0`, are **misleading on this image**: they are two-bit status setters for the cluster frames, not chassis-CAN `0x0D1` / `0x0CD`. They were left as already-named functions; read them as “2-bit cluster status.”

Bus-off on this controller posts **0xA0200** when the counter passes 14 (the constant lives at `0x346C0`, next to the sensor task) **[C]** as a fault ID, **[I]** as the exact counter RAM (not the ZCP’s `0x400EBC` — on 818.3 that address is `torque_request_word_a`).

## Bus-off, errors, and CAN-related DTCs

Raised through the common poster `0x32AD8`:

| Code | Where (818.3) | Meaning |
|---|---|---|
| 0xA0001 | `0x158B4` / pool `0x15AE8` | vehicle controller failed RAM test / init |
| 0xA0100 | `0x158B4` / pool `0x15AF0` | sensor controller failed init |
| 0xA0002 | `0x1C064` (bus-off `> 29`); also `0x1CE26` | vehicle bus-off, or unknown `0x329` variant byte |
| 0xA0008 | `0x1DD04`, `0x1DCDE` | `0x316` or `0x329` missing 71 ticks |
| 0xA0020 | pool `0x1DF08` | timer for `0x316` byte 0 bit 1 expired (`0x40078C`) **[I]** (handler not fully decompiled) |
| 0xA0200 | pool `0x346C0` | sensor-bus bus-off counter over 14 |
| 0xA1000 | `0x34EA4`, `0x34EA8` (and other pools) | `0x130` or `0x131` missing 14 ticks |
| 0xB0002, 0xB0004 | `0x1DD64`, `0x1DDC4`, `0x1DD9E` | `0x610` missing after five re-arms; `0x615` / `0x613` missing 142 ticks |
| 0xB0010 | pool `0x1DF04` | `0x316` byte 0 bit 2 timer (`0x40078A`) expired **[I]** |
| 0xB0040 | pool `0x1DF10` | variant-specific timeout **[I]** |
| 0xC0002 | `0x1D16C`, `0x1D212` (and coding-time pools) | gearbox frame contradicts `trans_type` |

`can_hw_status_decode` (`0x3BAE0`) is the register-level error decoder (warning, bus-off, overflow, and so on from base+`0x20`). That status is folded into `can_net_state` and the bus-off counter by `can_veh_net_task`.

## Diagnostic and other uses of CAN

- There are **no diagnostic request/response identifiers** (no `0x7xx`, no tester-present) in either message table. Workshop diagnostics for this module remain the **K-line / KWP** path described in `e46_m3_mk60_kline_access_and_flash.md` (`21 06` still returns the brake-pressure words `0x400984` / `0x400988`, which are *not* on CAN).
- **0x610** is an identity check against a stored EEPROM value, not a diagnostic session.
- **0x545** (oil temperature, a cruise bit in some loggers) is a DME / cluster frame this DSC **does not receive**. A logger may still see it on the vehicle bus.
- `mfg_rx_service` (`0x1BE28`) sits near the CAN layer in the function list but is the manufacturing / K-line service, not a CAN unpacker.

## 818.3 versus ZCP RAM

| Role | Typical ZCP | 818.3 |
|---|---|---|
| Network state | `0x400751` | `0x40074D` (`0x400751` is a speed-invalid / status bit) |
| Packed `0x153` torque bytes / flags | `0x400789` / `78B` / `78A` / `78C` | `0x400785` / `787` / `786` / `788` |
| Engine speed | `0x400734` | `0x400730` (`0x400734` is `0x316` byte 4 minus base) |
| Steering angle | `0x4007AE` | `0x4007AA` |
| `0x615` input byte | `0x40072A` | `0x400726` (write-only on this image; does not set the mode) |
| Cluster words | `0x400ECA`… | `0x400E9A`… (`0x400ED0` is `tcs_slip_term` here) |
| Variant byte | `0x400796` | `0x400792` |

Layouts, identifiers, periods, timeouts and the (field−1) bit-timing recipe match.

## Open questions

- Exact flash constants for CAN bit timing (BRP / TSEG / SJW in time or in kbit/s). The programming recipe is known; the source bytes are not pinned.
- Whether `0x7FFC80` is a true second controller or an alias.
- Message-record bytes 8..9, and flag bits other than `0x08`.
- A dedicated CAN interrupt handler, if any (the 10 ms poll is what actually runs the protocol).
- Sensor-bus bus-off counter RAM (fault 0xA0200 is certain; the counter address is not).
- Physical meaning of several `0x153` status bits (0.3, 0.5..0.7, 7.2, 7.4, 7.6, 7.7) and of `0x40092C` bit 0 as brake.
- 0.044 vs 0.04375 deg/count on `0x1F5`; the `0x1F3` longitudinal LSB remains **[I]**.
- Why 0x400726 is written at all if nothing reads it (leftover from a sibling image, or an unused coded path) **[I]**.
- Identity of channel 1’s hardwired pin in the car (not LV_TOW) **[?]**.

## Method and provenance

The 818.3 listings are from the raw image and a Ghidra recovery of this image (1585 functions, verify HEALTHY) with the public CUB3D/ghidra-mcore module and the scripts in Heinz’s public repo (commit `6ea098d`): `tools/install_mcore.sh` and `tools/patches/mcore.patch` provide language `MCORE:BE:32:default`. The vehicle- and sensor-bus tables, getters, setters, driver, and timeout posters were walked headless (`analyzeHeadless -noanalysis` plus the decompiler) on a working copy of that recovery. Names assigned to those functions and RAM words are in `e46_m3_mk60_818_3_function_names.md`. ZCP roles were taken from `zcp_can_frames.md` and re-checked against this listing; several RAM addresses are shifted, as tabulated above. The 0x615 / mode-input trace (write-only `0x400726`, GPIO-only mode word) is in `e46_m3_mk60_mtrack_mode4.md` and `e46_m3_mk60_traction_control.md`. No BIN was written, and nothing was written to a car.
