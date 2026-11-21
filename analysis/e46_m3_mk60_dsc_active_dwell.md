# E46 M3 non-ZCP MK60: what holds the 0x153 "DSC Active" bit on

2026-11-20. This traces the `0x153` byte 0 bit 3 that Gauge.S logs as "DSC Active" (the 818.3 image) back to its writer, its thresholds and its timers, then checks it against a 100 Hz Gauge.S drive log with five brake-only DSC events. Companion documents: `e46_m3_mk60_can_map.md`, `e46_m3_mk60_gauge_s_validation.md`, `e46_m3_dataset10_vs_zcp.md`, `e46_m3_mk60_mtrack_mode4.md`, `e46_m3_mk60_818_3_function_names.md`. This is not a procedure for writing a BIN, a car, or a bus.

Confidence: **[C]** confirmed in the 818.3 listing; **[I]** inferred; **[?]** open. Times assume the 10 ms control tick used throughout this map **[I]**. The 100 Hz log fits that tick (see the validation below).

## Short answer

The 0x153 "DSC Active" bit is held on by **both** two suspected mechanisms (hysteresis and a timed hold), plus an entry debounce. There is **no fixed minimum on-time**.

1. **Hysteresis [C].** The oversteer (OS) controller switches on against the entry bar `0x40042A` (set A 4.00 °/s, tapered with speed), but it stays on against a separate **hold bar `0x40042E`** (set A 2.00 °/s with the same taper, so exactly **0.5 × the entry bar**). At 60–68 mph that is 3.26 vs 1.63 °/s.
2. **Release timer [C].** Once the error is below the hold bar, an up/down counter has to fall from 0 to below **−25** (cal `0x3D15C` = 25). That takes **26 net ticks, 0.26 s**, before the controller drops. A tick back above the hold bar adds +1, so chatter around the hold bar stretches it.
3. **Entry debounce [C].** On the way in, the same counter has to count **up** to N net ticks above the entry bar. N is **35** normally (`0x3D16A`, 0.35 s), **17** when the steer-side term `0x400938` is large in the same direction (`0x3D16C`, 0.17 s), **5** in fast-rate or re-entry cases (`0x3D160`, 0.05 s), or **114** when `0x400344 > 0`.

The ~0.95 s events are not a minimum-dwell timer. They are about 0.4 s above the entry bar, plus about 0.4 s in the hysteresis band, plus the 0.26 s release timer.

## Signal chain

| Step | Address | What it does | Conf |
|---|---|---|---|
| CAN | `can_153_get_dsc_active` `0x1DA10` | `0x153` byte 0 bit 3 = `dsc_active_153_flags` `0x400EB5` bit 2 | [C] |
| Sole writer | `dsc_active_153_flag_update` `0x383A8` (was the fuzzy `tcs_torque_intervention_flag`) | see below; the only store to `0x400EB5` in the image | [C] |
| Yaw branch | same, when `tcs_status_eb6` `0x400EB6` bit 0 = 0 | bit 2 = `(yaw_ctrl_active_flags 0x400932 & 3) != 0` | [C] |
| TCS branch | same, when `0x400EB6` bit 0 = 1 | set if (`0x400EB6` bit 1 and `tcs_slip_term_max 0x400EC2 − 0x400EC4 < 7/8·0x400EBA` and (`< 3/4·0x400EBA` or `0x400EE4 < 13`)) or `0x400EB6` bit 2. **Self-latches**: once set it stays set until `0x400EB6` bit 0 clears (site `0x383F4`) | [C] logic, [?] meaning of the TCS inputs |
| `0x400932` bit 0 | `ayc_yaw_ctrl_activation` `0xF4B0` (was `yaw_bars_secondary`) | `= (ayc_ctrl_state 0x40045D & 3) != 0 and not 0x400932 bit 7` | [C] |
| `0x400932` bit 1 | `ayc_torque_path_state` `0xFD08` | DME TPS / torque / rpm path with integrator `0x40035A` | [I] |
| `0x400932` bit 7 | `yaw_ctrl_inhibit_update` `0x11498` | inhibit = `0x400DFA` bit 5, `_is_off()`, `0x400DFA` bit 4, or `0x400E00` bit 2 | [I] |
| alt. `0x400932` bit 0 | `yaw_ctrl_alt_brake_hold` `0x12438` | sets with `0x400508` bit 0 in a 40–200 km/h window; clears only when `0x400350` and `0x400352` are both 0. Order against `0xF4B0` is unproven | [?] |

The bit therefore means **"AYC yaw controller (OS or US) active, or the torque-path flag, or (during TCS) a latched TCS-control flag."** It is a **controller state, not valve or pump activity**. It does not wait for hydraulic ramp-out, except possibly through the [?] `0x12438` path. In this log the torque request stayed idle (`0xFF`), so the yaw branch is the one that fired **[I]**.

## `ayc_yaw_ctrl_activation` (`0xF4B0`): OS controller (bit 0 of `0x40045D`)

| Item | Address / value | Conf |
|---|---|---|
| Error compared | `yaw_os_err_ctrl` `0x400936` | [I] identity, [C] use |
| Entry bar | `yaw_os_bar` `0x40042A`; set A `0x3D170` = 1396 (4.00 °/s), set B `0x3D172` = 2792 (8.00 °/s), speed-tapered | [C] |
| Hold bar (hysteresis) | `yaw_os_hold_bar` `0x40042E` (previously labelled `yaw_us_bar`); set A `0x3D174` = 698 (2.00 °/s), set B `0x3D176` = 2094 (6.00 °/s), same taper | [C] |
| On/off counter | `ayc_os_onoff_counter` `0x400458` (signed byte) | [C] |
| Entry N | 35 `0x3D16A` / 17 `0x3D16C` / 5 `0x3D160` / 114 when `0x400344 > 0`. Long N is used when `\|0x400954\| < 4571` (`0x3D178`) and not active and `0x400F60` bit 0 clear; 17 replaces 35 when `0x400938 ≥ 873` (or ≤ −873) with the same sign as `0x400934` | [C] constants, [?] identities of `0x400954` / `0x400938` |
| Count up | error > entry bar (not active), or active and error > hold bar while counter < 0 | [C] |
| Count down | error ≤ hold (active) or ≤ entry (not active) and counter > 0 or active; phase `0x40034A` bits 1–2 = 2 (ramp-out) | [C] |
| Release | counter **< −25** (`0x3D15C`), compare at `0xF79C` → clear bit 0, counter = 0, phase = 3 | [C] |
| Hard reset | `0x400480 < 1081`, `0x400931` bit 4, `0x4005AE` bit 1, `0x400DFA` bit 5 or `0x400464` bit 4 → all AYC state cleared at once | [C] logic, [I] meaning |
| Fresh-entry speed gate | `0x400480 < 1441` blocks a new entry | [C] |

**US controller (bit 1)** uses its own counter `ayc_us_onoff_counter` `0x40045A`. It has entry bar `0x40042C` (`0x3D150` = 3.00 °/s), hold bar `0x400430` (`0x3D154` = 1.50 °/s), entry N **15** (`0x3D14C`, when `0x40034D` bit 2) or **5** (`0x3D14A`), and release at counter ≤ **−1** (`0x3D148`). That gives only about 0.02 s of release delay. It also feeds DSC Active through `0x400932` bit 0 **[C]**.

## Check against the 2026-11-20 log

Method: the same offline `r_ref` (dataset 10, non-ZCP), mode-1 bar and hold = 0.5 × bar as the earlier plots, on MK60-view speed, at 100 Hz. "Last above hold" is the last sample with OS error > hold bar before DSC Active falls.

| # | Clock (PT) | DSC on | Bar cross → on (entry) | On → last above hold | of which > bar / in hold band / below hold | Last above hold → off (release) |
|---|---|---|---|---|---|---|
| 1 | 3:17:18.7 | 5.73 s | 0.12 s | 5.40 s | 0.64 / 4.40 / 0.36 s | **0.33 s** |
| 2 | 3:18:32.9 | 7.14 s | 0.42 s | 6.53 s | 1.55 / 4.53 / 0.45 s | **0.61 s** |
| 3 | 3:18:44.2 | 5.49 s | 0.50 s | 5.48 s | 3.82 / 1.49 / 0.17 s | **0.01 s** |
| 4 | 3:18:52.4 | 0.95 s | 0.22 s | 0.76 s | 0.39 / 0.36 / 0.01 s | **0.19 s** |
| 5 | 3:19:23.1 | 0.99 s | 0.12 s | 0.74 s | 0.27 / 0.43 / 0.04 s | **0.25 s** |

- **Release timer.** Events 1, 4 and 5 fall 0.19–0.33 s after the error last leaves the hold band, which matches the 0.26 s timer. Event 2's 0.61 s fits the up/down counter: error chattering around the hold bar puts ticks back. Event 3 dropped while our modelled error was still just above hold, so either our offline `r_ref` runs slightly high in that left-hander or a hard-reset input fired **[?]**.
- **Hysteresis.** In the long sweepers the hysteresis band is what stretches the bit. On events 1 and 2, 4.4–4.5 s of the window is error between 0.5 × bar and the bar, where a single-threshold model would already have released.
- **Entry.** The bit sets 0.12–0.50 s after our error first crosses the bar, which fits the 17- or 35-tick debounce plus the firmware's own filtering of the error.
- **Simulation.** Re-running the firmware state machine on the log (entry N = 17, hold = 0.5 × bar, release at < −25) reproduced 4 of 5 events, with on/off edges within about ±0.1 s on events 1, 4 and 5. It missed event 2 entirely (our modelled error spends too few net ticks over the bar before the real onset) and over-ran event 3's tail (see above). With N = 5 it catches all 5 but adds 2 short false events.
- **Short events.** There is no ~1 s minimum. A 0.95 s event is about 0.2 s debounce, plus about 0.4 s over the bar, plus about 0.4 s in the hysteresis band, plus about 0.2–0.26 s release timer.

## Earlier log checks from the same drive

- **5/5** DSC Active events fell inside a predicted OS hold (mode-1 bar, hold = 0.5 × bar). All were brake-only: `0x153` byte 3 stayed `0xFF` (no torque cut) for the whole drive.
- **Brake** (`0x153` byte 0 bit 4) behaves as the brake-pedal switch. It was on for 100 % of hard braking, off during every predicted OS hold, and off on lift-off without the pedal.
- **M-track:** the 3:16 PM 35 mph tight left (OS error 8.95 °/s) crossed the mode-1 bar but stayed well under the live mode-4 bar (19.3 °/s at 62 km/h MK60 view). It set no DSC Active, as expected with M-track on.
- **ZCP `r_ref`** (dataset with `B = 2400`, `v_ch` 119.04): events 1 and 2, the 60–68 mph right-hand sweepers, would **not** have tripped (event 2 clears by only 0.09 °/s). Events 3–5 still trip. Whole-drive first-trips go from 44 to 12, and hold time from 16.8 s to 2.4 s.

## What this means for Gauge.S

- Treat 0x153 b0.3 as **"AYC controller active"**, not hydraulic activity.
- A faithful predictor needs: entry against the bar with a 0.17–0.35 s up-count, hold against 0.5 × bar, and release 0.26 s after the error stays below 0.5 × bar. The existing "hold = bar × 0.5" latch is the hysteresis half. Adding a 26-sample release delay at 100 Hz (or 3 samples at 10 Hz) covers the timer half.

## Still open

- Exact identity of `0x400936`, `0x400954`, `0x400938`, `0x400344` and the TCS inputs `0x400EBA` / `0x400EC4` / `0x400EE4` **[?]**.
- Whether `yaw_ctrl_alt_brake_hold` (`0x12438`) can hold `0x400932` bit 0 past the AYC release (task order) **[?]**.
- Hardware tick of the yaw task: assumed 10 ms, consistent with the log **[I]**.

## Method and provenance

From the Ghidra recovery of this image (1585 functions, verify HEALTHY) with the public CUB3D/ghidra-mcore module and the scripts in Heinz’s public repo. Calibration values were read from the 818.3 flash image at the addresses given. The log check used already-decoded CAN columns only. No BIN was written, and nothing was written to a car or bus. The new names are in `e46_m3_mk60_818_3_names.csv` (source "818.3 listing (DSC Active / AYC activation)").
