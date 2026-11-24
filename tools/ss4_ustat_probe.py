#!/usr/bin/env python3
"""Emit a small M·CORE RAM stub that elevates via SS4 then samples SGFMUSTAT.

Target: 820.3 / 818.3 non-M MK60 (SC550080MFU33), supervisor-gated SGFM at
0x00D00000. The first halfword is the documented privilege-trap trigger:

    mfcr r1, SS4   ; encoding 0x10A1

After the handler elevates (EPSR.S), the stub halfword-reads SGFMUSTAT
(0x00D0001C), stores it to a scratch word, writes a marker, and returns
(rts = 0x00CF). No program/erase commands are emitted.

This is a dry-run / hex emitter only. It does not open a K-line, serial
port, or talk to a module. Paste the hex into your existing A5 uploader,
run via the proven A6 K/L/O path, then A4-read scratch + marker.

Encodings used (matched against 820.3 dump bytes):
  mfcr rD, CRx     = 0x1000 | (cr << 4) | rd          # SS4 = CR10 → 0x10A1
  lrw  rD, slot     = 0x7000 | (rd << 8) | imm8
                   slot = ((pc + 2) & ~3) + (imm8 << 2)
  ld.h rD,(rX,d)   = 0xC000 | (rd << 8) | ((d & 0xF) << 4) | rx
  st.h rD,(rX,d)   = 0xD000 | (rd << 8) | ((d & 0xF) << 4) | rx
  rts              = 0x00CF

Usage:

  python3 tools/ss4_ustat_probe.py
  python3 tools/ss4_ustat_probe.py --base 0x401400 --scratch 0x401410
  python3 tools/ss4_ustat_probe.py --bin /tmp/stub.bin

stdlib only.
"""
from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

# Control-register numbers (M·CORE).
CR_SS4 = 10

# Defaults match the next-bench recipe in the private naming notes:
# SRAM-only probe, marker + USTAT readback via A4.
DEFAULT_BASE = 0x00401400
DEFAULT_SCRATCH = 0x00401410
DEFAULT_MARKER_ADDR = 0x00401414
DEFAULT_MARKER = 0x5A5A
DEFAULT_USTAT = 0x00D0001C


def u16(x: int) -> bytes:
    return struct.pack(">H", x & 0xFFFF)


def u32(x: int) -> bytes:
    return struct.pack(">I", x & 0xFFFFFFFF)


def enc_mfcr(rd: int, cr: int) -> int:
    return 0x1000 | ((cr & 0xF) << 4) | (rd & 0xF)


def enc_lrw(rd: int, imm8: int) -> int:
    if rd in (0, 15):
        raise ValueError("lrw r0/r15 are jmpi/jsri — pick r1..r14")
    return 0x7000 | ((rd & 0xF) << 8) | (imm8 & 0xFF)


def enc_ld_h(rd: int, rx: int, disp: int = 0) -> int:
    return 0xC000 | ((rd & 0xF) << 8) | ((disp & 0xF) << 4) | (rx & 0xF)


def enc_st_h(rd: int, rx: int, disp: int = 0) -> int:
    return 0xD000 | ((rd & 0xF) << 8) | ((disp & 0xF) << 4) | (rx & 0xF)


def lrw_imm_for(pc: int, pool_off: int) -> int:
    """imm8 such that lrw at pc resolves to pool_off (both relative to stub base)."""
    slot_base = (pc + 2) & ~3
    delta = pool_off - slot_base
    if delta < 0 or delta & 3:
        raise ValueError(f"pool 0x{pool_off:X} not reachable from pc 0x{pc:X}")
    imm = delta >> 2
    if imm > 0xFF:
        raise ValueError(f"pool 0x{pool_off:X} too far from pc 0x{pc:X}")
    return imm


def build_stub(
    *,
    base: int,
    ustat: int,
    scratch: int,
    marker_addr: int,
    marker: int,
) -> bytes:
    """Build the stub image; `base` is the absolute RAM load address (for docs only).

    Layout (offsets from stub start; pools 4-byte aligned at 0x14):

      00  mfcr r1, SS4
      02  lrw  r4, pool_ustat
      04  ld.h r0, (r4, 0)
      06  lrw  r5, pool_scratch
      08  st.h r0, (r5, 0)
      0A  lrw  r6, pool_marker_val
      0C  lrw  r7, pool_marker_addr
      0E  st.h r6, (r7, 0)
      10  rts
      12  pad
      14  ustat word
      18  scratch word
      1C  marker value word
      20  marker addr word
    """
    _ = base  # absolute base is for the operator; encodings are PC-relative
    pool_ustat = 0x14
    pool_scratch = 0x18
    pool_mval = 0x1C
    pool_maddr = 0x20

    code = bytearray()
    code += u16(enc_mfcr(1, CR_SS4))  # 0x10A1
    code += u16(enc_lrw(4, lrw_imm_for(0x02, pool_ustat)))
    code += u16(enc_ld_h(0, 4, 0))
    code += u16(enc_lrw(5, lrw_imm_for(0x06, pool_scratch)))
    code += u16(enc_st_h(0, 5, 0))
    code += u16(enc_lrw(6, lrw_imm_for(0x0A, pool_mval)))
    code += u16(enc_lrw(7, lrw_imm_for(0x0C, pool_maddr)))
    code += u16(enc_st_h(6, 7, 0))
    code += u16(0x00CF)  # rts
    code += u16(0x0000)  # pad to 0x14
    assert len(code) == 0x14
    code += u32(ustat)
    code += u32(scratch)
    code += u32(marker & 0xFFFF)
    code += u32(marker_addr)
    assert len(code) == 0x24
    return bytes(code)


def hex_dump(blob: bytes) -> str:
    return " ".join(f"{b:02X}" for b in blob)


def halfword_lines(blob: bytes) -> str:
    lines = []
    for i in range(0, len(blob), 2):
        hw = struct.unpack_from(">H", blob, i)[0]
        lines.append(f"  +0x{i:02X}: {hw:04X}")
    return "\n".join(lines)


def self_check() -> None:
    assert enc_mfcr(1, CR_SS4) == 0x10A1
    # ld.h r3,(r2,0) at privilege handler 0x14CF2 on 820.3
    assert enc_ld_h(3, 2, 0) == 0xC302
    # st.h r6,(r7,0) appears after ld.h sites in the same image
    assert enc_st_h(6, 7, 0) == 0xD607
    stub = build_stub(
        base=DEFAULT_BASE,
        ustat=DEFAULT_USTAT,
        scratch=DEFAULT_SCRATCH,
        marker_addr=DEFAULT_MARKER_ADDR,
        marker=DEFAULT_MARKER,
    )
    assert stub[:2] == b"\x10\xA1"
    assert stub[0x10:0x12] == b"\x00\xCF"
    assert stub[0x14:0x18] == u32(DEFAULT_USTAT)
    assert stub[0x18:0x1C] == u32(DEFAULT_SCRATCH)
    assert stub[0x1C:0x20] == u32(DEFAULT_MARKER)
    assert stub[0x20:0x24] == u32(DEFAULT_MARKER_ADDR)


def main(argv: list[str] | None = None) -> int:
    self_check()
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument(
        "--base",
        type=lambda s: int(s, 0),
        default=DEFAULT_BASE,
        help=f"RAM load address (default 0x{DEFAULT_BASE:X})",
    )
    p.add_argument(
        "--ustat",
        type=lambda s: int(s, 0),
        default=DEFAULT_USTAT,
        help=f"SGFMUSTAT address (default 0x{DEFAULT_USTAT:X})",
    )
    p.add_argument(
        "--scratch",
        type=lambda s: int(s, 0),
        default=DEFAULT_SCRATCH,
        help=f"where to store the USTAT halfword (default 0x{DEFAULT_SCRATCH:X})",
    )
    p.add_argument(
        "--marker-addr",
        type=lambda s: int(s, 0),
        default=DEFAULT_MARKER_ADDR,
        help=f"marker cell (default 0x{DEFAULT_MARKER_ADDR:X})",
    )
    p.add_argument(
        "--marker",
        type=lambda s: int(s, 0),
        default=DEFAULT_MARKER,
        help=f"marker halfword (default 0x{DEFAULT_MARKER:X})",
    )
    p.add_argument("--bin", type=Path, help="also write raw bytes to this path")
    p.add_argument("--quiet", action="store_true", help="print hex only")
    args = p.parse_args(argv)

    stub = build_stub(
        base=args.base,
        ustat=args.ustat,
        scratch=args.scratch,
        marker_addr=args.marker_addr,
        marker=args.marker,
    )

    if args.bin:
        args.bin.write_bytes(stub)

    if args.quiet:
        print(hex_dump(stub))
        return 0

    print(f"module family: 820.3 / 818.3 non-M MK60 (SC550080MFU33)")
    print(f"stub length:   {len(stub)} bytes")
    print(f"load base:     0x{args.base:08X}")
    print(f"SGFMUSTAT:     0x{args.ustat:08X}  -> scratch 0x{args.scratch:08X}")
    print(f"marker:        0x{args.marker:04X} @ 0x{args.marker_addr:08X}")
    print(f"pass check:    A4-read scratch ≈ 0x00C0 and marker == 0x{args.marker:04X}")
    print()
    print("halfwords:")
    print(halfword_lines(stub))
    print()
    print("hex:")
    print(hex_dump(stub))
    return 0


if __name__ == "__main__":
    sys.exit(main())
