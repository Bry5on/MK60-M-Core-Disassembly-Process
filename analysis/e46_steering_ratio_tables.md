# E46 MK60 steering-ratio tables (LENKWINKELKENNLINIE)

Read-only findings from the raw dumps `817.3_e46_m3_zcp_mk60_full_dump.bin` (ZCP), `818.3_e46_m3_mk60_full_dump.bin` (M3 non-ZCP) and `820.3_e46_non_m_mk60_full_dump.bin` (non-M). No code path has been traced yet; the layout is inferred from the E9x M3 description in cmg5461/mk60e5-firmware `analysis/DSC_STEERING_AND_VEHICLE_MODEL_7846816A.md` §3.1 (`steer_ratio_convert`).

## Layout

| Image | Block address | Sets present |
|---|---|---|
| M3 non-ZCP | 0x3E908 | 1–6 |
| ZCP | 0x3EF5C | 1–6 (byte-identical to the non-ZCP) |
| Non-M | (calibration tail) | 1–3 only, identical to sets 1–3 above |

Immediately before the steering-ratio block is a separate 2-point curve used by another function (x = {10000, 20000}, y = {286, 857}); those words are not a header of this block. The steering-ratio block starts at the clamp list (last x of each set: 14400, 12274, 12891, 12205, 14400, 12685), then all x sets followed by all y sets, each 4 × s16. x is steering-wheel angle in sensor counts; y is the overall (secant) ratio × 1024, linearly interpolated on |angle| and clamped to the last x. The byte before the preceding curve differs (0x83 non-ZCP, 0xC9 ZCP) and belongs to another table.

| Set | x (counts) | y (ratio × 1024) |
|---|---|---|
| 1 | 0, 2971, 5028, 14400 | 18555, 17459, 17705, 16353 |
| 2 | 0, 2857, 5211, 12274 | 16384, 15309, 15565, 14438 |
| 3 | 0, 480, 1211, 12891 | 19210, 15995, 15626, 14746 |
| 4 | 0, 3634, 4800, 12205 | 17203, 16159, 16364, 15073 |
| 5 | 0, 2443, 4571, 14400 | 17224, 16773, 17234, 15913 |
| 6 | 0, 1371, 9143, 12685 | 14746, 14746, 13978, 13978 |

## Coding mapping (LENKWINKELKENNLINIE_2)

From the E46 NCS daten (reported by the repo user, not traced in code): data value d selects set d + 1.

| Wert | Data | Set | Use |
|---|---|---|---|
| wert_10 | 00 | 1 | default (inverse of wert_02's options) |
| wert_02 | 01 | 2 | non-M (DSC3, MK60_CI_09, LENK_I_50) |
| wert_03 | 02 | 3 | not identified |
| wert_04 | 03 | 4 | CSL / ZCP |
| wert_05 | 04 | 5 | M3 standard |

Set 6 has no wert in this list.

## Derived values

Using the E9x angle scale of 0.04375°/count (not confirmed for the E46):

| Set | Lock (SW °) | Road wheel at lock (°) | Overall ratio, centre → lock |
|---|---|---|---|
| 2 non-M | 537 | 38.1 | 16.00 → 14.10 |
| 4 ZCP | 534 | 36.3 | 16.80 → 14.72 |
| 5 M3 | 630 | 40.5 | 16.82 → 15.54 |

Lock angles agree with the vehicles: ZCP and non-M about 3 turns lock to lock, M3 about 3.2. All three curves show a local slow-down between roughly 110° and 230° of wheel before quickening to lock; whether that is linkage geometry or a four-point fit artefact is not established.

![non-M vs M3 vs ZCP](e46_steering_ratio_tables.png)

## Open

- [ ] Trace the E46 equivalent of `steer_ratio_convert` and the coding field that selects the set.
- [ ] Confirm the E46 angle scale (log SZL angle against road-wheel angle on turn plates).
- [ ] Identify set 3 (wert_03) and set 6.
