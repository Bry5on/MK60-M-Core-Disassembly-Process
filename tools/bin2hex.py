#!/usr/bin/env python3
"""Raw binary -> Intel HEX at a base address, for feeding a chip-off / tool flash dump
into mcore_import.sh (which only knows .0pa and Intel HEX).

Run isolated:  python3 -I tools/bin2hex.py <image.bin> <out.cpu.hex> [--base 0xADDR] [--skip 0xN] [--length 0xN]

  --base    CPU address of the first emitted byte (default 0x0: a full-chip dump of the
            MK60E5 flash starts at the reset vector, which is where the existing images
            map it). A dump of only the application sectors is placed with --base 0x40000.
  --skip    drop this many bytes from the start of the file first (dump-tool header).
  --length  emit at most this many bytes.

Runs of 0xFF (erased flash) are kept: the pipeline expects a contiguous image segment.
Before importing, run `tools/find_boundary.py <out.cpu.hex>` -- a real MK60E5 application
yields ~1200+ M-CORE prologues; a config-EEPROM or RAM snapshot yields 0. stdlib only.
"""
import sys


def main():
    args = sys.argv[1:]
    if len(args) < 2:
        print(__doc__)
        sys.exit(2)
    src, dst = args[0], args[1]
    base, skip, length = 0, 0, None
    i = 2
    while i < len(args):
        if args[i] == "--base":
            base = int(args[i + 1], 0); i += 2
        elif args[i] == "--skip":
            skip = int(args[i + 1], 0); i += 2
        elif args[i] == "--length":
            length = int(args[i + 1], 0); i += 2
        else:
            print("unknown arg", args[i]); sys.exit(2)
    with open(src, "rb") as fh:
        data = fh.read()
    data = data[skip:]
    if length is not None:
        data = data[:length]
    if not data:
        print("no data to emit"); sys.exit(1)

    def rec(t, addr16, payload):
        body = bytes([len(payload), (addr16 >> 8) & 0xFF, addr16 & 0xFF, t]) + payload
        return ":%s%02X" % (body.hex().upper(), (-sum(body)) & 0xFF)

    out = []
    upper = None
    for off in range(0, len(data), 16):
        a = base + off
        hi = a >> 16
        if hi != upper:
            out.append(rec(4, 0, bytes([(hi >> 8) & 0xFF, hi & 0xFF])))
            upper = hi
        out.append(rec(0, a & 0xFFFF, data[off:off + 16]))
    out.append(":00000001FF")
    with open(dst, "w") as fh:
        fh.write("\n".join(out) + "\n")
    print("wrote %s: %d bytes at 0x%X..0x%X" % (dst, len(data), base, base + len(data) - 1))


if __name__ == "__main__":
    main()
