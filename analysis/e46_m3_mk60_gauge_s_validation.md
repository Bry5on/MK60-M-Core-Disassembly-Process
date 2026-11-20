# E46 M3 non-ZCP MK60 — Gauge.S predictor and channel check

2026-11-20. A check of a Gauge.S yaw-bar predictor, and of the underlying MK60-view CAN channels it uses, against the 818.3 listing. Companion documents: `e46_m3_mk60_can_map.md` (the byte/bit layouts and scales), `e46_m3_mk60_traction_control.md` (the `0x153` torque-request bytes), `e46_m3_dataset10_vs_zcp.md` (dataset-10 `r_ref` and the numeric mode-1 / mode-4 oversteer bars), `e46_m3_mk60_mtrack_mode4.md` (what live M-track changes). This is not a procedure for writing a BIN, a car, or a bus.

Confidence: **[C]** confirmed against this image; **[I]** inferred; **[?]** still open.

Two predictor configs were compared. The **primary** config — the one to use — has mode-1 set-A oversteer *and* understeer bars, with hold = **bar × 0.5**. An **older copy** has oversteer only and hold = **bar − 2**. The older hold is wrong; it was not used as the reference for the verdicts below.

The predictor objects consume already-scaled yaw, steer and wheel-speed channels. Those underlying encodings were checked against the same listing (and against an MK60-view channel set that unpacks `0x1F0` / `0x1F3` / `0x1F5` / `0x153`). An example sweeper log already carries those underlying CAN columns, not the predictor outputs — the predictor objects exist only once they are pasted into a live Gauge.S config.

This predictor is **mode-1 only**. It will not reproduce a mode-4 first-trip bar (that path is MDM `/128` then the site at `0xF028`; 13.17 °/s at 113.8 km/h in an example sweeper log — see `e46_m3_dataset10_vs_zcp.md`). `k_ds` is not a live CAN field.

## Predictor math

| Channel | Verdict |
|---|---|
| Vehicle speed = `0.5 × (WS RL + WS RR)` | **Correct** **[C]**. Must stay on MK60-view km/h (`0x1F0` scale, no tyre factor, no Speedo). |
| Steering-wheel ratio, set-4 LUT (`y/1024` already as a ratio) | **Correct** as previously established for this rack. |
| `r_ref` with `L = 1846/675`, `v_ch = 96.15` (dataset 10) | **Correct** **[C]** for this non-ZCP image with live `k_ds = 10`. A ZCP `v_ch` of 119.04 should stay unused on 818.3. |
| Oversteer error = `\|Yaw\| − r_ref`, understeer error = `r_ref − \|Yaw\|` | **Correct** **[C]** (magnitudes; signs in the logs are opposite). |
| OS bar 4.00 → 3.00 deg/s (4.00 at 43.2–72 km/h, 3.00 at ≥ 126) | **Correct** **[C]** for mode-1 set A. The LUT uses 3.999 / 2.999 as in the firmware taper. |
| US bar 5.00 → 3.00 (flat 3.00 at ≥ 72) | **Correct** **[C]**; present only in the primary config. |
| OS / US hold = **bar × 0.5** | **Correct** **[C]** (firmware hold is 2.00 tapered like a 4.00 bar, 1.50 at a 3.00 US bar). The older **bar − 2** is **wrong** (too tight at high speed, where the bar is already 3.00). |
| OS / US trip / near / margin | **Correct** as first-trip compares. They do not model the item-1 IIR (usually commented on purpose: Gauge.S is typically slower than 10 ms). |
| Optional tyre-corrected speed display | Display only; must not feed `r_ref` or the bars. Fine to keep. |

A bicycle-model overlay of firmware yaw versus dataset-10 `r_ref` on the example sweeper log agreed to about 1 %.

## Underlying CAN channels

| Channel | Encoding checked | Verdict |
|---|---|---|
| WS FL / FR / RL / RR | `0x1F0`, `(x & 4095) × 0.0625` km/h | **Correct** **[C]**. The 13th bit is only needed above 256 km/h. |
| Yaw | `0x1F3`, piecewise × **0.12223** | **Correct** **[C]** (0.131 would be 7.2 % high). This is measured yaw at `0x400978`, not `r_ref`. |
| Lateral G | `0x1F3`, `((x >> 6) − 512) × 0.004894` | **Correct** **[C]** |
| Longitudinal G | `0x1F3`, `((x & 1023) − 511) × 0.003193` | Packing and offset **[C]**, LSB **[I]** |
| Steering angle | `0x1F5`, 0.044 sign-magnitude | Packing **[C]**; 0.044 vs 0.04375 **[?]** |
| Speedo | `0x153` starting at byte 1, `(x >> 4 > 4) × (x >> 3) × 0.03665` mph | Bit extract **[C]** (drops the three flag bits at 1.0..1.2, then the 13-bit speed). The `0.03665` factor is 0.0625 km/h × a 0.9437 tyre factor × 0.621371 — a **display** choice, not the firmware view. Do not feed Speedo into `r_ref`. |
| Brake | `0x153` byte 0 bit 4 | Matches getter `0x1D498` / `0x40092C` bit 0. Physical meaning **[I]** (likely the brake-light switch). A public DME bit at `0x329` byte 6 is **not** unpacked by this module. |
| “DSC Active” | `0x153` byte 0 bit 3 | **Unverifiable / likely mislabelled.** This is `0x400EB5` bit 2, not a general DSC-active flag. Prefer byte 0 bits 0..1 (torque-request active) or byte 7 bit 1 (MDM status). |
| Torque-request byte | `0x153` byte 3, `x × 0.392` | Byte 3 **is** `can_153_torque_b0` (`0x400785`) **[C]**. `× 0.392` ≈ 100/255 is a reasonable 0..100 % display if `0xFF` means “no cut” **[I]**. Idle is `0xFF`; a live intervention is packed with variant-`0x11` `(request + base) × 8 / 5`. Treat the percentage as indicative, not a calibrated newton-metre. |
| RPM | `0x316` × 0.15625 | **Correct as a public bus scale** **[C]**. Internal `0x400730` uses `× 16/51`. |
| Torque (`0x316` byte 4) | `x × 0.390625` | **Correct as a 0..100 % bus display** **[I]** versus the firmware’s variant-dependent scale into `0x400734`. |
| TPS | `0x329` byte 5 × 0.390625 | **Correct** as 0..100 % of the raw byte the module stores at `0x400732`. |
| Coolant, ambient pressure (`0x329` B1 / B2), oil temp / cruise (`0x545`), ambient temp (`0x615` B3) | public encodings | **Not consumed by this DSC.** Fine on a logger that sees the whole vehicle bus. `0x545` in particular is absent from the MK60 receive table. |
| Gear derived from Speedo / RPM / clutch | derived | Not a DSC CAN signal. |

## Useful channels this listing unlocks

Add these if a logger should watch TCS (see `e46_m3_mk60_traction_control.md`):

- `0x153` byte 0 bit 0 — positive torque request (ASC cut active)
- `0x153` byte 0 bit 1 — negative / drag request (MSR-candidate; that path is not closed)
- `0x153` bytes 3, 4, 6 — packed request bytes (`0x400785` / `0x400787` / `0x400786`)
- `0x153` byte 7 bit 1 — MDM status (output only; the mode is set by the hardwired DSC / M button, not by CAN `0x615`)
- Optional: `0x153` byte 1 bit 1 fault summary; `0x1F3` status bits

`0x615` byte 1 bit 1 is publicly LV_TOW and is **not** an M-track input on this image (`e46_m3_mk60_can_map.md`). A transmitter on that identifier would not set the mode, and would collide with the instrument cluster.

## Method and provenance

The encodings were taken from the 818.3 getters and setters documented in `e46_m3_mk60_can_map.md`, themselves from a Ghidra recovery of this image (1585 functions, verify HEALTHY) with the public CUB3D/ghidra-mcore module and the scripts in Heinz’s public repo (commit `6ea098d`). The mode-1 bar taper and dataset-10 `r_ref` constants are those in `e46_m3_dataset10_vs_zcp.md`. The example sweeper log was used only as a source of already-decoded CAN columns; no BIN was written, and nothing was written to a car.
