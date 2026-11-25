"""List every lrw literal in the MMIO band (0x700000..0xFFFFFF) with the code sites that load it."""
# SPDX-License-Identifier: MIT
# Part of MK60-M-Core-Disassembly-Process. See LICENSE / CREDITS.md.
import struct, sys, collections
d = open(sys.argv[1], 'rb').read()
lo, hi = int(sys.argv[2], 16), int(sys.argv[3], 16)   # code window
sites = collections.defaultdict(list)
for pc in range(lo, hi, 2):
    op = struct.unpack('>H', d[pc:pc+2])[0]
    if op >> 12 == 7 and ((op >> 8) & 15) != 15:
        slot = ((pc + 2) & ~3) + ((op & 0xFF) << 2)
        if slot + 4 <= len(d):
            v = struct.unpack('>I', d[slot:slot+4])[0]
            if 0x700000 <= v <= 0xFFFFFF:
                sites[v].append(pc)
for v in sorted(sites):
    s = sites[v]
    print('%06x  n=%-3d %s' % (v, len(s), ' '.join('%05x' % p for p in s[:8])))
