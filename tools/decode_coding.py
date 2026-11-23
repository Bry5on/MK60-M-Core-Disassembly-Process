#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
# Part of MK60-M-Core-Disassembly-Process. See LICENSE / CREDITS.md.
"""
decode_coding.py — decode the packed DSC coding block of an MK60 95040 EEPROM using a
local NCS-Dummy "ModuleFunctions" export (E46-MK60.Cxx-ModuleFunctions.txt).

Assumption (confirmed on a bench 820.3 unit): the packed coding block sits at EEPROM
--base (default 0x120), so NCS coding-offset N -> EEPROM byte base+N. The 95040 is read
bit-inverted (logical = raw XOR 0xFF); pass --raw to skip the inversion.

The NCS export is read locally; nothing from it is embedded in this tool or its output
beyond the keyword/offset/mask facts needed to decode.

Usage:
  python tools/decode_coding.py <eeprom.bin> <Cxx-ModuleFunctions.txt> [--base 0x120] [--raw]
"""
import sys, re

def parse_ncs(path):
    """Return a list of (keyword, addr, mask, {masked_value: name}) in file order."""
    txt = open(path, encoding="latin1").read().splitlines()
    rx_fn = re.compile(r'^([A-Z][A-Z0-9_]+)\s+([0-9A-F]{8})\s+\(([0-9A-F]+)\)\s+([0-9A-F]{2})\s')
    rx_val = re.compile(r'^\s+(\S+)\s+([0-9A-F]{2})\s+([0-9A-F]{2})\s+(.*)$')
    out = []
    cur = None
    for ln in txt:
        m = rx_fn.match(ln)
        if m:
            kw, addr, length, mask = m.group(1), int(m.group(2), 16), int(m.group(3), 16), int(m.group(4), 16)
            cur = {"kw": kw, "addr": addr, "mask": mask, "vals": {}}
            out.append(cur)
            continue
        if cur is not None:
            mv = rx_val.match(ln)
            if mv:
                name, data, masked = mv.group(1), int(mv.group(2), 16), int(mv.group(3), 16)
                cur["vals"].setdefault(masked, name)
    return out

def main():
    a = sys.argv
    binp, ncsp = a[1], a[2]
    base = 0x120
    raw = False
    for i, t in enumerate(a):
        if t == "--base": base = int(a[i+1], 16)
        if t == "--raw": raw = True
    d = open(binp, "rb").read()
    if not raw:
        d = bytes(b ^ 0xFF for b in d)
    fns = parse_ncs(ncsp)
    maxaddr = max(f["addr"] for f in fns)
    print("EEPROM: %s   NCS: %s   base=0x%x" % (binp.split('/')[-1], ncsp.split('/')[-1], base))
    print("coding block (logical): %s" % ' '.join('%02x' % d[base+i] for i in range(maxaddr+1)))
    # CHECKSUM (NCS offset 0) = XOR(coding bytes 0x01..maxaddr) + 1  mod 256
    x = 0
    for i in range(1, maxaddr + 1):
        x ^= d[base + i]
    calc = (x + 1) & 0xff
    stored = d[base]
    print("CHECKSUM: stored=%02x  computed(XOR(payload)+1)=%02x  -> %s"
          % (stored, calc, "OK" if calc == stored else "MISMATCH"))
    print("-" * 78)
    for f in fns:
        byte = d[base + f["addr"]]
        masked = byte & f["mask"]
        name = f["vals"].get(masked)
        # value as a right-shifted raw number (for numeric fields)
        shift = (f["mask"] & -f["mask"]).bit_length() - 1
        num = masked >> shift
        label = name if name is not None else ("= %d (0x%02x)" % (num, num))
        print("  +0x%02X %-26s mask %02X  byte=%02X masked=%02X  -> %s"
              % (f["addr"], f["kw"], f["mask"], byte, masked, label))

if __name__ == "__main__":
    main()
