"""Find bsr callers and lrw/jsri pool references to the given addresses."""
# SPDX-License-Identifier: MIT
# Part of MK60-M-Core-Disassembly-Process. See LICENSE / CREDITS.md.
import struct, sys
d = open(sys.argv[1], 'rb').read()
lo, hi = int(sys.argv[2], 16), int(sys.argv[3], 16)
tg = [int(x, 16) for x in sys.argv[4:]]
for pc in range(lo, hi, 2):
    op = struct.unpack('>H', d[pc:pc+2])[0]
    if op >> 11 == 0x1F:                       # bsr disp11
        disp = op & 0x7FF
        if disp & 0x400: disp -= 0x800
        t = pc + 2 + (disp << 1)
        if t in tg: print('%05x bsr  -> %05x' % (pc, t))
    if op >> 12 == 7:                          # lrw / jsri / jmpi
        slot = ((pc + 2) & ~3) + ((op & 0xFF) << 2)
        if slot + 4 <= len(d):
            v = struct.unpack('>I', d[slot:slot+4])[0]
            if v in tg:
                kind = {15: 'jsri/jmpi'}.get((op >> 8) & 15, 'lrw')
                print('%05x %s -> %05x' % (pc, kind, v))
