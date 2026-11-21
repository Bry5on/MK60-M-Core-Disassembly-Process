# E46 M3 non-ZCP MK60 — 818.3 function and data names

2026-11-20. Read-only analysis of the 818.3 (non-ZCP E46 M3) Continental Teves MK60 image. This is a map of names assigned on a Ghidra recovery of that image, not a procedure for writing a BIN or a car.

Companion documents: `e46_m3_mk60_mtrack_mode4.md` (what live M-track mode changes), `e46_m3_mk60_can_map.md` (both CAN buses and every RX/TX), `e46_m3_mk60_traction_control.md` (TCS slip chain and the `0x153` torque request), `e46_m3_mk60_gauge_s_validation.md` (Gauge.S predictor and channel check), `e46_m3_dataset10_vs_zcp.md` (dataset-10 `r_ref` and the numeric mode-1 / mode-4 oversteer bars), `e46_m3_mk60_image_check_word.md` (the main-range check word), `e46_m3_mk60_tooling_status.md` (M·CORE tooling and coverage). The full name table is `e46_m3_mk60_818_3_names.csv`.

## Counts

A first pass of unambiguous signature and call-site ports from the already-named 817.3 ZCP recovery produced **51** user-named functions on this image (50 ports plus `reset`), out of **1585** recovered functions (verify HEALTHY). A later pass, using the same recovery and the method below, brought that to **113** named functions and added **61** user labels (RAM variables, MDM / TCS / A-B calibration tables, and the `0xF028` taper site). A further pass over the CAN layer (both buses), the traction-control path, and the driver-mode inputs brought the catalog to **219** named functions and **109** user labels (328 rows). A pass on the 0x153 "DSC Active" bit and the AYC on/off state machine (`e46_m3_mk60_dsc_active_dwell.md`) added 4 functions, renamed 3, and added 35 labels (RAM, timers, and the entry / hold / release calibration constants). The catalog is now **223** named functions and **144** user labels (367 rows). The recovery already had imported vector names and a large set of auto `tbl_*` flash-table labels; those were left in place and are not in the 144.

The 223 does not include seven generic `thunk_FUN_*` stubs. Confidence on the full catalog is **177 [C]**, **43 [I]**, and **3 [?]** for functions, and **97 [C]** / **40 [I]** / **7 [?]** for labels.

## How the names were assigned

The listing is a Ghidra 12.2 recovery of the 818.3 image with language **`MCORE:BE:32:default`**, from the public [CUB3D/ghidra-mcore](https://github.com/CUB3D/ghidra-mcore) module as installed by Heinz’s public `tools/install_mcore.sh` and `tools/patches/mcore.patch` (Heinz pipeline at commit `6ea098d`). Work was done headless (`analyzeHeadless -noanalysis`) on a working copy of that recovery so the original project was not modified.

Names were applied in this order:

1. **Exact signature port**, with Heinz’s public `tools/port_symbols.py` (relocation-invariant masked-body signatures, then call-site / table-slot propagation). Sources were the already-named 817.3 ZCP and 820.3 non-M recoveries, and a named E9x M3 image. Function names that originate on that E9x image ultimately come from [cmg5461’s documented MK60e5 analysis](https://github.com/cmg5461/mk60e5-firmware). The ZCP and non-M ports added no *new* names in this pass — the unambiguous matches were already present. The E9x image added seven short helpers only when the minimum unique-signature body was lowered to four instructions. The useful E9x overlap is real but thin; this is a different generation than the e5 image.
2. **Fuzzy n-gram containment**, with Heinz’s public `tools/fuzzy_port.py`. The published floor (6-gram, score ≥ 0.60, mutual best) produced no ports. A looser candidate list (6-gram ≥ 0.40 and 4-gram ≥ 0.50) was vetted against the 818.3 listing before any name was kept. A vehicle-specific CAN-id candidate (`can_rx_2E4_trailer`) was rejected: this module’s vehicle-bus transmit list is `0x153` / `0x1F0` / `0x1F3`.
3. **Vehicle-model and M-track identification** of still-unnamed functions, RAM, and calibration tables from the 818.3 listing itself: OS / US bars, the `0x400474` correction chain, the TCS slip term, the CAN `0x153` torque packer, coding / `k_ds`, `r_ref` / `r_est`, and the image check word. Existing user names were not overwritten, except in the DSC Active pass, where three were renamed on dataflow evidence (noted in the CSV).
4. **CAN layer, TCS, and driver-mode inputs**, still from the 818.3 listing: the vehicle- and sensor-bus tables and every getter / setter, the controller driver and timeout posters, the TCS slip chain through the `0x153` packer, and a trace of CAN `0x615` byte 1 bit 1 showing that it does not write the mode word (mode 4 is the hardwired DSC / M button). Details are in `e46_m3_mk60_can_map.md` and `e46_m3_mk60_traction_control.md`. Existing user names were not overwritten, except in the DSC Active pass, where three were renamed on dataflow evidence (noted in the CSV).

Every new name has a one-line basis in the CSV, and a plate or EOL comment with the same confidence tag in the recovery.

## Confidence

| Tag | Meaning |
|---|---|
| [C] | Confirmed from this listing (or an unambiguous signature / high-score fuzzy match that agrees with the listing). |
| [I] | Inferred: the dataflow or a moderate-score fuzzy match supports the name, but the official role is not labeled in the image. |
| [?] | Unsettled: a useful handle, not a claimed official name. |

## Names that matter for a mid-speed oversteer trip

These are the names that describe what M-track changes when the car is mid-corner, and what those changes reach. Details of the consumers are in `e46_m3_mk60_mtrack_mode4.md`; the example sweeper’s first-trip mode-4 oversteer bar (13.17 °/s at 113.8 km/h) is in `e46_m3_dataset10_vs_zcp.md`.

- `yaw_os_us_bars_update` (`0xE844`) writes `yaw_os_bar` / `yaw_os_hold_bar` (`0x40042A` / `0x40042E`; the second was labelled `yaw_us_bar` before the DSC Active pass showed it is the OS hold bar). The MDM `/128` scale uses `mdm_os_bar_v_pts` / `mdm_os_bar_scale_q7` (`0x3D1CE` / `0x3D1D6`). Site `yaw_os_bar_mode1_taper` (`0xF028`) reapplies the mode-1 speed taper (and a 3.00 °/s floor). [C]
- `yaw_os_bar_trip_filter` (`0xD98C`) compares `yaw_vs_bar` (`0x400922`) to `yaw_os_bar` and writes `yaw_os_corr_filtered` (`0x400474`). Its wrapper is `ayc_obs_wrapper` (`0xD2E4`) — also a vetted 4-gram match to the E9x name. [C]
- That correction is read by `actuator_cmd_from_yaw_corr` (`0x4E30`) and `hyd_plant_main` (`0xA8E4`): a wider bar makes the trip harder, so hydraulic commands move only indirectly. [C] for the dataflow. The name `hyd_plant_main` itself is [?] (ABS vs DSC yaw-brake vs a shared valve mux is not labeled).
- TCS: `tcs_driveslip_total` (`0x351C4`) and `tcs_mdm_slip_term` (`0x36750`) write `tcs_slip_target` (`0x400F02`) and `tcs_slip_term_max` (`0x400EC2`). `tcs_torque_request_build` (`0x38E06`) feeds `can_153_torque_request_pack` (`0x1C8EC`), which packs `can_153_torque_b0`–`b2` (`0x400785`–`0x400787`) onto CAN `0x153`. There is no `_is_mdm` test in the packer; the requests move because the TCS RAM moves. Byte 7 bit 1 of the same frame is the status announcement `can_153_mdm_status_bit` (`0x1DC54`), not torque. [C]
- `is_mdm_flag` (`0x401015`), `driver_mode` (`0x400868`), `k_ds` (`0x40100F`), `r_ref_eval` (`0x29606`), `r_est_eval` (`0x26258`), and `cal_A_columns` / `cal_B_columns` are named. A, B, T3 and T4 are **not** mode-4-selected. [C]
- `lat_est_limit` (`0x400AA5`) and the `0x10BD` / `0x85F` threshold in `mdm_threshold_10BD` (`0x28BE8`) are named; they have no further consumers outside their writers. [C]

## What is still open

- **MSR** (overrun / engine-drag): no direct `_is_mdm` or flag read, and the torque packer’s second-sign path (`0x400EB7` bit 5, inputs `0x400F60` / `0x400F8E`) has not been identified as MSR. The best candidate writer of `0x400F8E` is `msr_or_drag_term_update` (`0x3A1BE`) [I]. See `e46_m3_mk60_traction_control.md`. [?]
- **EBV, ASC brake, CBC / ADB:** EBV was not identified; a dedicated TCS wheel-brake path was not separated from the shared hydraulic plant; no CBC / ADB path was identified under those names. [?]
- **`hyd_plant_main` (`0xA8E4`):** the dataflow from `yaw_os_corr_filtered` is confirmed; the official function name is not. [?]
- Official names of `actuator_cmd_from_yaw_corr` and `yaw_corr_gate_flag`, physical units of `lat_est_limit` and the `0x10BD` / `0x85F` pair, the 818.3 lamp-dispatcher addresses, and a live confirmation of CAN `0x153` byte 7 bit 1 on a car.
- Identity of driver-mode channel 1’s hardwired pin (not CAN `0x615` / LV_TOW; that bit is write-only and does not set the mode — `e46_m3_mk60_can_map.md`). [?]

An ambiguous fuzzy tie at `0x25A48` (`steer_task` vs `control_dispatcher`) was left unnamed.

## Names behind the 0x153 "DSC Active" bit

- `can_153_get_dsc_active` (`0x1DA10`) reads `dsc_active_153_flags` (`0x400EB5`) bit 2, whose only writer is `dsc_active_153_flag_update` (`0x383A8`; previously the fuzzy `tcs_torque_intervention_flag`). [C]
- `ayc_yaw_ctrl_activation` (`0xF4B0`; previously `yaw_bars_secondary`) is the OS/US controller on/off state machine. It writes `ayc_ctrl_state` (`0x40045D`) and `yaw_ctrl_active_flags` (`0x400932`) bit 0, using counters `ayc_os_onoff_counter` / `ayc_us_onoff_counter` (`0x400458` / `0x40045A`). Its constants are `cal_ayc_os_entry_ticks_long` / `_steer` / `_short` (35 / 17 / 5 at `0x3D16A` / `0x3D16C` / `0x3D160`) and `cal_ayc_os_release_ticks` (25 at `0x3D15C`). [C]
- `ayc_torque_path_state` (`0xFD08`, 0x400932 bit 1) [I], `yaw_ctrl_inhibit_update` (`0x11498`, bit 7) [I], `yaw_ctrl_alt_brake_hold` (`0x12438`) [?], `yaw_ctrl_flag_clear_aux` (`0x1222C`) [?].

## The table

`e46_m3_mk60_818_3_names.csv` has one row per named function and per new label (367 rows: 223 functions, 144 labels), with columns `address`, `kind`, `old_name`, `new_name`, `confidence`, `source`, and `basis`. `old_name` is the generic `FUN_…` (or empty, for a new label) unless the name was already present. `source` is one of: already present (ZCP signature port); unique signature or fuzzy n-gram versus a named E9x M3 image; or the 818.3 listing (M-track / vehicle model, dataset 10, the image check word, the CAN layer, TCS, or DSC Active / AYC activation).

## Method and provenance

Function bounds are from the Ghidra recovery of this image (1585 functions, verify HEALTHY) with the public CUB3D/ghidra-mcore module and the scripts in Heinz’s public repo. E9x-origin names that were kept after a signature or vetted fuzzy match are from cmg5461’s documented analysis, carried with Heinz’s public `port_symbols.py` and `fuzzy_port.py`. M-track, dataset-10, check-word, CAN-layer, TCS, and driver-mode names were assigned from the 818.3 listing and the notes cited above. No BIN was written, and nothing was written to a car.
