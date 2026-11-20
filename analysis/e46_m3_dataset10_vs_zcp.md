# E46 M3 dataset 10 versus the ZCP calibration

Hand-written note (2026-11-20). Companion to `e46_m3_non_zcp_mk60_image.md`, `zcp_mk60_image.md`, `e46_steering_ratio_tables.md` and `e46_m3_mk60_image_check_word.md`. Addresses are 818.3 unless a ZCP twin is named.

Dataset 10 (`k_ds` from packed coding byte 1, bits 4:0, at `0x40100F` on 818.3) is the E46 M3 slot. This note records the 818.3 versus 817.3 ZCP values in that slot, what A, B, T0, T3 and T4 actually do, and a structure-aligned inventory of the rest of the two calibration tails. It is a comparison of the two images, not a procedure for writing either one.

Confidence: **[C]** read from the image or from algebra that the listing forces, **[I]** inferred, **[open]** not established.

## Dataset 10 values

The fourteen signed 16-bit columns live in program flash, not EEPROM. Bases and the dataset-10 cells:

| Column | 818.3 base | 818.3 ds10 | 817.3 ZCP base | ZCP ds10 |
|---|---|---|---|---|
| T0 | `0x3E874` | **38** at `0x3E888` | `0x3EEC8` | **37** at `0x3EEDC` |
| A  | `0x3E890` | **1846** at `0x3E8A4` | `0x3EEE4` | **1845** at `0x3EEF8` |
| B  | `0x3E8AC` | **1565** at `0x3E8C0` | `0x3EF00` | **2400** at `0x3EF14` |
| T3 | `0x3E8C8` | **1846** at `0x3E8DC` | `0x3EF1C` | **1845** at `0x3EF30` |
| T4 | `0x3E8E4` | **131** at `0x3E8F8` | `0x3EF38` | **201** at `0x3EF4C` |

`v_ch` is not a stored word. It is formed at run time from A and B:

| Image | `v_ch = √(32·A·B) / 100` |
|---|---|
| 818.3 | 96.15 km/h |
| ZCP | 119.04 km/h |

T0–T4 in every other dataset slot are **byte-identical** between the two M3 images. The only T-column difference is dataset 10. [C]

## `r_ref`: A is L, `v_ch² = 32·A·B`

The closed form at `0x29606` (ZCP twin `0x29B2E`) is:

```
r_ref = δ · v  /  ( A · (1 + v² / v_ch²) )
      = δ · v  /  ( A + v² / (32·B) )
```

with `v_ch² = 32·A·B`. A is the wheelbase length L in this formula, not merely a factor inside the product `A·B`. [C]

A and B are read from flash on every evaluation (`ld.h`); there is no RAM copy of the tables. The only 818.3 readers of the A and B bases are this routine and its sibling at `0x29898`. [C]

The sibling builds the same denominator `F = A + v²/(32·B)` (A again used alone as L; it does not form a separate `A·B` product). Above 10 km/h it scales four signals, including the yaw residual at `0x400922`. [C]

Because A enters as L, a change to B alone moves `v_ch` and the high-speed term; a change to A also scales the whole denominator, including at low speed.

## T0, T3, T4

**T0** is a wheel-speed yaw scale in the E9x AF4 family. It is not an input to `r_ref`. Dataset 10 differs by one count (38 versus 37). [C] for the column and the one-count difference; **[I]** for the physical reading, from the E9x analogue and the table layout.

**T3** is a near-copy of A (identical in every slot of both M3 images, including dataset 10). It is the length term in the estimated yaw `r_est` below. [C]

**T4** is the understeer divisor in that same estimate. Across the 28 slots of the two M3 images it tracks

```
T4  ≈  1.8 · v_ch² / g  =  32·A·B / 706320
```

to within one count in 26 of 28 slots. Dataset 3 is the shared exception (stored 116 versus a prediction of 165) on **both** images, so it is a deliberate common cal, not a ZCP-only change. On dataset 10 the ZCP value 201 is exactly the formula for A=1845, B=2400; 818.3’s 131 is the same formula for 1846/1565 (predicted 130.7, stored 131). [C] for the fit and the two dataset-10 cells; **[I]** that the 1.8 / *g* reading is the intended physical scale.

T3 and T4 have a single reader: `0x26258` (ZCP twin `0x26780`). No other `lrw` or table-of-tables pointer to those bases was found. [C]

## `r_est` at `0x26258`

The routine forms

```
r_est  =  v · δ_filt / T3  −  v · a_y / T4
```

Inputs on 818.3:

| Symbol | Address | Scale |
|---|---|---|
| `δ_filt` | `0x400A4E` | filtered steer (same path as `r_ref`) |
| `a_y` | `0x400968` | lateral acceleration, 0.01 g per bit |
| `v` | `0x401038` | vehicle speed |

If `|r_est| − |r_ref|` exceeds **698** (2.0 °/s at the 349 counts/° yaw scale used in the mode-1 bars), a debounce sets **bit 7 of `0x400AC3`**. That is a plausibility flag on the closed-form `r_ref`, not a rewrite of `r_ref` itself. [C]

Whether that bit reaches the valve or MDM request, and what a 2 °/s disagreement does to a sweeper, is still **[open]**.

## Block-level calibration inventory

The two data tails cannot be compared by a blanket address offset. Block *lengths* differ, and two name slots are swapped: the 372-byte O-slot holds `NOPARADJ.UST` on 818.3 and `AWTO26F1` on the ZCP; the 3460-byte W-slot holds `AUTW24F1` on 818.3 and `NOPARADJ` on the ZCP. Pair by size and position, not by matching NOP to NOP. [C]

### Identical between the two M3 images

- All six steering-ratio sets (see `e46_steering_ratio_tables.md`). [C]
- Coding-dependent U0–U5 tables. [C]
- Mode-1 oversteer / understeer bars: 1396 / 698 and 2792 / 2094. [C]
- MDM / M-track scale curves, including the curve at `0x287D0`. [C]
- The μ clamp at `0x558C`. [C]
- T0–T4 in every dataset slot except dataset 10, as above. [C]
- 792 of the 800-byte traction / yaw / M-track tail that 818.3 packs at `0x3F9CC–0x3FCEB` and the ZCP places at `0x40000–0x4031F`. The eight differing bytes are three coding-RAM base pointers and the image check word. [C]
- MUE tables are an EEPROM coding choice, not a flash difference between these two images. [C]

### Real differences besides dataset 10

| Block | Size / shape | What differs | Likely domain | Confidence |
|---|---|---|---|---|
| AUTA / AWTA | 751 bytes on both | 261 of 369 halfwords differ. Adjacent curves appear reordered or length-shifted, so this is not a 1:1 row map. Readers span `0x3AC6`–`0x11DC8`. | Brake / ASC / hydraulic [I] | size and mismatch count [C]; function [I] |
| mid-AUTC | ZCP is 300 bytes longer in this mid-C span | a net insert on the ZCP, plus a handful of small integer fields in the region before T0 | not fully named [open]; the insert is not the dataset-10 T columns themselves | length and the “small fields” [C]; official Teves names [open] |
| AUTY leftovers | ZCP 104 bytes longer | leftover records after the yaw-bar neighbourhood; the bar neighbourhood itself is identical | not fully named [open] | length [C] |
| AUTS | 818.3 is 32 bytes longer | one extra record on 818.3 | not fully named [open] | length [C] |
| PY / W / O | names and some relocated pointers; the 372-byte and 3460-byte slots are swapped as noted above | mostly identifiers and addresses, not a second copy of dataset 10 | naming / linkage [I] | the swap and the pointer relocations [C] |

Official Teves names for the mid-C fields, the leftover AUTY records and the extra AUTS record are **[open]**. Same-offset word compares inside AUTA are misleading and should not be read as a curve-by-curve map.

### What matters for a sweeper, and what “behave like a ZCP” would touch

For the yaw *model* on a mid-speed sweeper, the flash differences that actually enter `r_ref` and `r_est` are **dataset-10 B** (and the one-count A / T3 change) and **dataset-10 T4**. The mode-1 bars, the steering maps, the MDM scale curves and the other T-column slots do not differ. AUTA and the mid-C / AUTY / AUTS leftovers may still change brake or ASC behaviour; they have not been shown to feed `r_ref`. [C] for the readers; **[I]** that AUTA is off that path.

A *model-consistent* ZCP-like dataset-10 column would therefore be A ≈ 1845, B = 2400 and T4 = 201, with the main image check word recomputed over the resulting main range (T4 sits in that range; see `e46_m3_mk60_image_check_word.md`). A wider “behave like a ZCP” copy would also have to consider AUTA and the leftover records. Even a complete calibration copy would leave ZCP-only *code* (a third M-track button channel, a different coding-RAM map) untouched. That is a finding about the two images, not a write list.

## Open questions

- [ ] The live `k_ds` on a given car (coding byte, not a flash constant).
- [ ] Whether bit 7 of `0x400AC3` reaches the valve or MDM path, and what a 2 °/s `r_est` disagreement does in a sweeper.
- [ ] Official Teves names and functions for AUTA, the mid-C integer fields, leftover AUTY and the extra AUTS record.
- [ ] T0 in millimetres of track, or another physical unit, confirmed against a measured geometry.

## Method

A and B uses were collected from every 818.3 `lrw` (and computed-address / table-of-tables) site onto the column bases; only `0x29606` and `0x29898` read them. T3 / T4 similarly collapse to `0x26258`. The closed forms follow from the listings at those addresses. The calibration inventory was aligned by named dataset-block size and position rather than by a constant address shift; NOP/O and AUTW/W name slots were paired by size where the names are swapped.
