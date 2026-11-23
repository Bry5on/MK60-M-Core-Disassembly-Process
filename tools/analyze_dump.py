#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Part of MK60-M-Core-Disassembly-Process. See LICENSE / CREDITS.md.
"""
analyze_dump.py — quick flash-writability triage for an MK60 (E46) image.

When a new image turns up (e.g. a reportedly flashable part number), this answers the
decisive questions without Ghidra:
  1. Security field at 0x200 ("LOCK"+FF = secured, backdoor unset) -> different from 818.3?
  2. Any flash program services in the KWP stack (0x34 RequestDownload, 0x36 TransferData,
     0x37 RequestTransferExit)? -> the smoking gun.
  3. Is a flash-controller band referenced (0xD00000 / 0xF60000 / 0xD50000 / 0xC00000) that
     817.3/818.3 do not reference?
  4. Bootloader markers ("SBL!", loader vectors)?
  5. FF map: where content ends, and whether anything sits above 0x40000.

Usage:
  python tools/analyze_dump.py <image.bin> [reference.bin ...]

All values are heuristics over the raw image (BASE=0, big-endian M.CORE).
"""
import struct, sys

LOCK = b"LOCK"

def rd(p):
    with open(p, "rb") as f:
        return f.read()

def u16(d, a):
    return struct.unpack(">H", d[a:a+2])[0] if a+2 <= len(d) else None

def u32(d, a):
    return struct.unpack(">I", d[a:a+4])[0] if a+4 <= len(d) else None

def sec_field(d):
    """0x200: 4-byte security word, then 8-byte backdoor key (AN2286 layout)."""
    if len(d) < 0x210:
        return None
    sw = d[0x200:0x204]
    key = d[0x204:0x20C]
    secured = sw == LOCK                      # 818.3 family: exactly "LOCK"
    backdoor_set = any(b != 0xFF for b in key)
    return {
        "raw": sw.hex(" "),
        "ascii": "".join(chr(b) if 32 <= b < 127 else "." for b in sw),
        "backdoor_key": key.hex(" "),
        "secured_like_818": secured,
        "backdoor_set": backdoor_set,
    }

def kwp_services(d):
    """Locate the KWP service-table WINDOW (records, not a byte block) by clustering and
    report whether flash services 0x34/0x36/0x37 sit on the service grid.
    818.3 @0x3ED24, 817.3 @0x3F358 (records of SID/len/mask/level/handler)."""
    flash_sids = {0x34: "RequestDownload", 0x36: "TransferData", 0x37: "RequestTransferExit"}
    known = {0x14,0x10,0x20,0x18,0x17,0x1A,0x21,0x27,0x30,0x31,0x3B,0x22,0x2E,0x3E,0x09,0x23,0x3D}
    lo = min(0x3C000, max(0, len(d) - 0x4000))   # data region, roughly behind the code
    win = 0xC0                                    # window large enough for 17 records
    best = (-1, 0)                                # (pos, distinct_known_hits)
    for a in range(lo, len(d) - win, 2):
        hits = len(known & set(d[a:a + win]))
        if hits > best[1]:
            best = (a, hits)
    pos, hits = best
    out = {"table_window_at": "0x%x" % pos if pos >= 0 else "-",
           "known_sids_in_window": "%d/17" % hits}
    seg = d[pos:pos + win] if pos >= 0 else b""
    # derive the record grid: positions of known SIDs in the window -> modal stride
    sid_pos = [i for i, b in enumerate(seg) if b in known]
    stride, col = 0, 0
    if len(sid_pos) >= 4:
        from collections import Counter
        gaps = Counter(b - a for a, b in zip(sid_pos, sid_pos[1:]) if 4 <= b - a <= 24)
        if gaps:
            stride = gaps.most_common(1)[0][0]
            col = Counter(p % stride for p in sid_pos).most_common(1)[0][0]
    out["record_stride"] = ("%d B (SID column @ off%%%d==%d)" % (stride, stride, col)) if stride else "unclear"
    grid = {i for i in range(len(seg)) if stride and i % stride == col} if stride else set()
    for sid, name in flash_sids.items():
        on_grid = any(seg[i] == sid for i in grid)
        anywhere = sid in set(seg)
        if on_grid:
            out[name] = "!!! 0x%02x AS A SERVICE ID (on grid) -- check the download path in Ghidra" % sid
        elif anywhere:
            out[name] = "0x%02x only as a data/handler byte (no service), like 818.3" % sid
        else:
            out[name] = "0x%02x absent, like 818.3" % sid
    out["note"] = ("0x34+0x36 together on the grid = strong download path; 818.3/817.3/820.3 "
                   "have neither on grid.")
    return out

def mmio_scan(d, code_lo, code_hi):
    """All lrw literals in interesting bands + flash-controller candidates."""
    bands = {}
    flash_cands = []
    FLASH_HINT = [(0x00C00000, 0x00CFFFFF), (0x00D00000, 0x00D0FFFF),
                  (0x00D50000, 0x00DEFFFF), (0x00F50000, 0x00F6FFFF)]
    for pc in range(code_lo, min(code_hi, len(d) - 2), 2):
        op = u16(d, pc)
        if op is None:
            break
        if op >> 12 == 7 and ((op >> 8) & 15) != 15:        # lrw rN,[slot]
            slot = ((pc + 2) & ~3) + ((op & 0xFF) << 2)
            v = u32(d, slot)
            if v is None:
                continue
            if 0x00700000 <= v <= 0x00FFFFFF:
                bands.setdefault(v & 0xFFFF00, []).append(pc)
            for lo, hi in FLASH_HINT:
                if lo <= v <= hi:
                    flash_cands.append((pc, v))
    return bands, flash_cands

def loader_markers(d):
    out = {}
    for marker in (b"SBL!", b"BTLD", b"LOAD", b"AZ1"):
        p = d.find(marker)
        out[marker.decode("latin1")] = hex(p) if p >= 0 else "-"
    out["word@0x4000"] = hex(u32(d, 0x4000)) if len(d) > 0x4004 else "-"
    out["word@0x4010"] = hex(u32(d, 0x4010)) if len(d) > 0x4014 else "-"
    return out

def ff_map(d):
    """Last non-FF byte and whether content sits above 0x40000."""
    last = -1
    for i in range(len(d) - 1, -1, -1):
        if d[i] != 0xFF:
            last = i
            break
    above = any(b != 0xFF for b in d[0x40000:]) if len(d) > 0x40000 else None
    return {
        "size": "0x%x (%d B)" % (len(d), len(d)),
        "last_nonFF": "0x%x" % last if last >= 0 else "all FF",
        "content_above_0x40000": (None if above is None else bool(above)),
        "note": ("dump ends below 0x40000 -- the upper window (0x40000..0x5FFFF) was not read; "
                 "raise the reader's scan ceiling for a 384 KB part") if len(d) <= 0x40000 else "",
    }

def analyze(path):
    d = rd(path)
    print("=" * 70)
    print("file:", path)
    print("\n[FF map]")
    for k, v in ff_map(d).items():
        print("  %-22s %s" % (k, v))
    code_hi = min(len(d), 0x3D000)
    print("\n[security field 0x200]")
    sf = sec_field(d)
    if sf:
        for k, v in sf.items():
            print("  %-22s %s" % (k, v))
    print("\n[KWP flash services]")
    for k, v in kwp_services(d).items():
        print("  %-22s %s" % (k, v))
    print("\n[loader markers]")
    for k, v in loader_markers(d).items():
        print("  %-22s %s" % (k, v))
    print("\n[MMIO bands + flash-controller candidates]")
    bands, cands = mmio_scan(d, 0x200, code_hi)
    for base in sorted(bands):
        print("  band 0x%06x  n=%d" % (base, len(bands[base])))
    if cands:
        print("  >>> FLASH-CONTROLLER CANDIDATES (not present in 817/818):")
        for pc, v in cands[:20]:
            print("      pc 0x%05x -> 0x%06x" % (pc, v))
    else:
        print("  no flash-controller candidates (like 817.3/818.3)")

def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return
    for p in sys.argv[1:]:
        analyze(p)

if __name__ == "__main__":
    main()
