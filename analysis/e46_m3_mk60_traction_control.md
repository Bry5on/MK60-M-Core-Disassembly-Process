# E46 M3 non-ZCP MK60 — traction control on the 818.3 image

2026-11-20. Read-only analysis of the 818.3 (non-ZCP E46 M3) Continental Teves MK60 image. This is a map of the ASC / TCS slip chain, how M-track changes the slip targets, and how a torque request is packed onto CAN `0x153`. It is not a procedure for writing a BIN or a car.

Companion documents: `e46_m3_mk60_can_map.md` (both buses, every RX/TX, the `0x153` byte layout, and the finding that `0x615` never drives the mode), `e46_m3_mk60_mtrack_mode4.md` (the wider bars and the GPIO button that actually sets mode 4), `e46_m3_mk60_gauge_s_validation.md` (how those `0x153` / wheel-speed signals check against a Gauge.S predictor), `e46_m3_dataset10_vs_zcp.md` (dataset-10 `r_ref` and the numeric mode-1 / mode-4 oversteer bars), `e46_m3_mk60_818_3_function_names.md` (named functions and RAM, with `e46_m3_mk60_818_3_names.csv`), `zcp_can_frames.md` / `zcp_m_track_mode.md` (ZCP twins; several RAM addresses are shifted).

Addresses below are 818.3 unless a ZCP twin is named. Confidence: **[C]** confirmed in this image’s tables or decompiled text; **[I]** inferred from a clear data-flow or a twin in the public ZCP write-up, with 818.3 RAM confirmed; **[?]** still open.

**818.3 RAM is not a 1:1 copy of the ZCP map.** The packed `0x153` torque bytes sit at `0x400785`–`0x400787` / flags `0x400788` here; on the ZCP those roles were documented at `0x400789` / `0x40078B` / `0x40078C`, which are countdown timers on 818.3. Engine speed is `0x400730`, not `0x400734`. See the RAM table in `e46_m3_mk60_can_map.md`.

## How TCS works

The module estimates drive slip from the four wheel-speed blocks, compares it to a speed-dependent target (widened in M-track / MDM), and asks the DME to cut — or, on a second-sign path, to add or hold — torque on CAN `0x153`. Yaw / oversteer work is a separate path that can command brakes through the hydraulic plant; that path is not itself TCS, but some of its flags are OR-ed into the `0x153` fault and status bits.

### Wheel speed and reference speed

Each wheel lives in a 0x40-byte block starting at `0x401700`, `0x401740`, `0x401780`, `0x4017C0`. The speed word used for CAN and for slip is at **+0x18** (`0x401718`, `0x401758`, `0x401798`, `0x4017D8`). A per-wheel flag byte at `0x4005BA`–`0x4005BD` bit 7 (or `0x400E00` bit 5) marks the wheel invalid.

Reference speed `0x4005B0` is the floor used by the `0x1F0` and `0x153` speed getters: they transmit `max(wheel, 0x4005B0) × 4 / 25`, or `0x1FFF` when that wheel is invalid. That packing is **0.0625 km/h per count** **[C]**.

Engine speed is not from `0x1F0`. CAN `0x316` bytes 2..3 are written by `can_rx_316_rpm` (`0x1CF10`) to `engine_rpm` **`0x400730`** as `value × 16 / 51`, with a filtered derivative at `0x40072E`. TCS reads `0x400730` (pool in `tcs_torque_request_build`). Internal units are about twice a public 0.15625 rpm/bit bus scale (roughly 0.5 rpm/count) **[I]**.

### Drive-slip estimate and targets **[C]**

- `tcs_driveslip_curve` (`0x3518C`) interpolates `tcs_driveslip_curve_tbl` (`0x3FAF8`). In M-track it uses the cached `is_mdm_flag` (`0x401015`) as a table index (the second record of several traction calibration pairs).
- `tcs_driveslip_total` (`0x351C4`) combines the curve into `tcs_slip_target` (`0x400F02`).
- `tcs_target_to_channel` (`0x35074`) **[I]** distributes the target onto a channel object at `0x400644`.
- In MDM / M-track (modes 4 or 7), `tcs_mdm_slip_term` (`0x36750`) applies the `tcs_mtrack_tail` (`0x3F9CC`) term into `tcs_slip_term` (`0x400ED0`).
- `tcs_slip_aggregate` (`0x37F60`) and `tcs_slip_term_tick` (`0x37D9C`) maintain `tcs_slip_term_max` (`0x400EC2`) and the aggregate words `0x400EF2` / `0x400EF4`.
- Related already-named helpers: `tcs_entry_gate` (`0x355B4`), `tcs_yaw_slip_term` (`0x363FC`), `tcs_torque_intervention_flag` (`0x383A8`), `tcs_traction_entry` (`0x38B4C`).

The `_is_mdm` predicate (`0x209FE`) is true for driver modes 4 or 7 on **channel 0 only**. The cached flag is written each cycle by `is_mdm_flag_write` (`0x3B080`). Channel 0 is the hardwired DSC / M button, not a CAN bit — see below, and `e46_m3_mk60_mtrack_mode4.md`.

### Engine intervention: TCS → DME on `0x153` **[C]**

`dme_torque_inputs_scale` (`0x38DB0`) prepares DME torque / TPS inputs (`0x400734`, `0x400736`, `torque_base` `0x400738`).

`tcs_torque_request_build` (`0x38E06`) runs every TCS tick and does the actual request:

- If `torque_sign_flags` (`0x400EB7`) bit 4 is set, it uses the **positive / cut** path driven by `tcs_slip_term_max` (`0x400EC2`) and a related term at `0x400EB8`.
- If bit 5 is set, it uses the **second-sign** path: terms at `0x400F72` / `0x400F8C`, gated by `0x400F60` bit 0 and `0x400F8E` bit 0.
- It writes `torque_request_word_a` (`0x400EBC`) and `torque_request_word_b` (`0x400EBE`), and updates `0x400EB7`.

`can_153_torque_request_pack` (`0x1C8EC`, 584 instructions) is the CAN-side packer, not the TCS controller. It is the only writer of the packed request bytes. It reads those request words plus `torque_base` (`0x400738`) and `can_variant_byte` (`0x400792`):

- Variant `0x11`: `(request + base) × 8 / 5`, then cap.
- A positive request goes to `can_153_torque_b0` (`0x400785`) with flag bit 0 of `can_153_torque_flags` (`0x400788`).
- A negative / drag request is inverted into `0x400785` and also uses `can_153_torque_b2` (`0x400787`, byte 4 on the wire) with flag bit 1.
- `can_153_torque_b1` (`0x400786`) is the third packed byte (byte 6 on the wire).
- Idle / no request writes `0xFF / 0 / 0` into the three bytes and clears the flags **[I]** (same behaviour as the public ZCP write-up; the 818.3 packer decomp is call-heavy, but the byte destinations are direct writes).

Those four RAM locations are the *only* producers of the `0x153` torque bytes and of bits 0..1 of byte 0. The getters `can_153_get_torque_b3` / `b4` / `b6` and `can_153_get_tq_flag_pos` / `neg` just copy them onto the bus every 10 ms. The full byte/bit layout of `0x153` is in `e46_m3_mk60_can_map.md`.

There is **no `_is_mdm` test in the packer**. Engine-torque requests move because the TCS RAM that feeds `0x38E06` moves when the slip targets move. Byte 7 bit 1 of the same frame is the MDM **status** announcement (`can_153_mdm_status_bit` `0x1DC54`), not a torque byte and not an input.

### M-track and the mode input **[C]**

M-track widens the ASC / TCS slip targets (flag-indexed curve pairs plus the slip term at `0x36750`) and, separately, widens the yaw oversteer / understeer bars that feed the hydraulic plant. It does not change A, B, T3, T4, `r_ref`, or `r_est`. The bar path and the numeric mode-4 first-trip bar (13.17 °/s at 113.8 km/h in an example sweeper log) are in `e46_m3_mk60_mtrack_mode4.md` and `e46_m3_dataset10_vs_zcp.md`.

The live mode is **not** set from CAN. `0x615` byte 1 bit 1 (publicly LV_TOW on ICL3) is written into `0x400726` only when coded feature 3 is set, and nothing in this image reads that byte. `_is_mdm`, `0x401015`, and `0x153` byte 7 bit 1 all come from driver-mode channel 0, the hardwired DSC / M button (`dsc_button_gpio_get` `0x170C6`, GPIO shadow `0x40063A` bit 6). Channel 1 is a second hardwired pin, not this CAN bit. See `e46_m3_mk60_can_map.md` for the 0x615 timeout and coding gate.

### What TCS consumes from CAN, and what it transmits

**Consumed (vehicle bus):**

| Signal | Frame | 818.3 RAM | TCS / related consumer |
|---|---|---|---|
| Engine speed | `0x316` B2..3 | `0x400730` | `tcs_torque_request_build`, several TCS helpers |
| DME torque byte 1 | `0x316` B1 | `0x400736` | `dme_torque_inputs_scale`, TCS / MDM wheel-side |
| DME torque byte 4 | `0x316` B4 | `0x400734` | `dme_torque_inputs_scale`, `msr_or_drag_term_update` **[I]** |
| DME base torque | `0x316` B5 | `0x400738` | packer `0x1C8EC`, TCS, also `yaw_os_us_bars_update` |
| Pedal / TPS | `0x329` B5 | `0x400732` | TCS helpers (`0x38768` and others) |
| Variant / scale | `0x329` B0 | `0x400792`, `0x400784` | packer and all `0x316` torque setters |
| Steering angle / rate | `0x1F5` | `0x4007AA`, `0x4007AC` | vehicle model / `r_ref`; not a TCS slip input |
| Gear | `0x43F` / `0x43D` | via `trans_type` `0x40073B` | gearbox path; fault 0xC0002 on contradiction |

`0x615` is **not** a TCS or mode input on this image, even though a setter writes `0x400726`.

**Transmitted (vehicle bus), TCS-relevant:**

- `0x153` bytes 3, 4, 6 and byte 0 bits 0..1: the torque request, as above.
- `0x153` byte 7 bit 1: MDM status only **[C]**.
- `0x1F0`: four wheel speeds, the same blocks TCS uses.
- `0x1F3`: measured yaw / ay / ax, not `r_ref` (`r_ref` is `0x400972` and is not on this frame).

The sensor cluster (private bus `0x130` / `0x131` / `0x132`) is the yaw-rate and lateral-acceleration unit. `cluster_sensor_get` (`0x34BC0`) reads the six words `0x400E9A`–`0x400EA4`. Those feed the vehicle-model / AYC side more than the slip target, but they are the source behind `0x1F3`.

## Open: MSR, EBV, and ASC brake

These three were looked for and are **not closed**.

**MSR** (overrun / engine-drag) **[I], still not closed.** The second-sign path in `tcs_torque_request_build` is real and distinct from the `0x400EC2` cut path. It is **not** the M-track flag (`0x401015`).

- `0x400F8E` is written only by `msr_or_drag_term_update` (`0x3A1BE`), which also reads the DME torque words `0x400734` / `0x400738`. That is the best MSR candidate **[I]**.
- `0x400F60` is *shared*: written by `0x392B0`, `0x399BC`, `0x39CC6`, and **read** by `yaw_bars_secondary`, `mdm_wheel_side_state`, the `0x153` fault-summary getter, and TCS. It is not an MSR-only flag.
- `0x400F72` is written by those same TCS-adjacent functions.

Engine-drag / MSR *likely* feeds `0x400F8E` into the second-sign half of `0x38E06`, and that request is what sets `0x153` byte 0 bit 1. It is not proven that every bit-5 request is MSR, or that EBV / CBC is uninvolved.

**EBV** (electronic brake-force distribution) was not identified under a stable name **[?]**.

**ASC brake** (a TCS function that brakes a spinning drive wheel) was not separated from the shared hydraulic plant. Yaw oversteer still goes `yaw_os_us_bars_update` (`0xE844`) → trip filter `0xD98C` → `0x400474` → `actuator_cmd_from_yaw_corr` (`0x4E30`) / `yaw_corr_gate_flag` (`0x7290`) / `hyd_plant_main` (`0xA8E4`) **[I]** for the official names. That is the stability (AYC) brake path, not TCS. Valve / ABS-lock versus TCS wheel-brake remains **[I]**. CBC / ADB were not identified under those names **[?]**.

## Open questions

- Whether every second-sign request is MSR; writers of the shared flag `0x400F60` still need a close read (`0x392B0`, `0x399BC`, `0x39CC6`).
- ASC brake (TCS wheel brake) versus the AYC hydraulic path; EBV, CBC, and ADB still **[?]**.
- Official names of `0x4E30` / `0x7290` / `0xA8E4` (ABS vs DSC yaw-brake vs a shared valve mux).
- Physical meaning of several `0x153` status bits, collected in `e46_m3_mk60_can_map.md`.
- A live confirmation that `0x153` byte 7 bit 1 tracks the M-button / three-position DSC cycle on a car.

## Method and provenance

The 818.3 listings are from the raw image and a Ghidra recovery of this image (1585 functions, verify HEALTHY) with the public CUB3D/ghidra-mcore module and the scripts in Heinz’s public repo (commit `6ea098d`): `tools/install_mcore.sh` and `tools/patches/mcore.patch` provide language `MCORE:BE:32:default`. The TCS builders, the `0x153` packer, and the `_is_mdm` / GPIO mode path were walked headless (`analyzeHeadless -noanalysis` plus the decompiler) on a working copy of that recovery. Names assigned to those functions and RAM words are in `e46_m3_mk60_818_3_function_names.md`. ZCP roles were taken from `zcp_can_frames.md` and `zcp_m_track_mode.md` and re-checked against this listing; packed-torque and several other RAM addresses are shifted, as noted above. No BIN was written, and nothing was written to a car.
