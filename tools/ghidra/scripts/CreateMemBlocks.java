// Create the uninitialized RAM + MMIO memory blocks the .0pa image doesn't carry,
// so RAM/peripheral references resolve (no more "Address not in memory", and the
// Symbol Tree can navigate to RAM labels). Ranges from ScanUnmapped.java +
// ARCHITECTURE.md: RAM 0x400000, ports/serial/QSPI/CAN/GPIO 0xD5-0xDE, INTC 0xE1,
// timers/CAN2/reset 0xF5-0xFF, MPU/flash-cache 0xFFF. Idempotent (skips existing).
//@category MK60
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.mem.*;

public class CreateMemBlocks extends GhidraScript {
    @Override
    public void run() throws Exception {
        //           name            start         length      volatile(MMIO)
        make("RAM",            0x400000L,  0x24000L,  false);  // 0x400000-0x423FFF (max ref 0x421FC0)
        make("MMIO_ports_D",   0xD50000L,  0x91000L,  true);   // 0xD50000-0xDE0FFF
        make("MMIO_INTC_E",    0xE00000L,  0x11000L,  true);   // 0xE00000-0xE10FFF
        make("MMIO_timers_F",  0xF50000L,  0xA1000L,  true);   // 0xF50000-0xFF0FFF
        make("MMIO_MPU_FFF",   0xFFF00000L,0x85000L,  true);   // 0xFFF00000-0xFFF84FFF
        discovered();
    }

    // The fixed list above is the E5 SoC. The earlier MK60 (full-chip dump at 0x0) talks to
    // peripherals at other addresses (0xCF0000.., 0xEF8000, 0xEFF000, 0xFF8000, 0xFFF000,
    // 0xFFFF8000 ..), so any 4 KiB page outside flash/RAM that >= MIN_HITS aligned pointer words
    // in the image name is mapped too (merged into runs, volatile). Only the SoC's 24-bit space
    // and the 0xFFF00000 window count: values outside are instruction pairs, and with 4 KiB
    // pages and this threshold noise never clusters like real register pages do (the E5 images
    // only re-find pages the fixed list covers).
    private static final int MIN_HITS = 16;
    private void discovered() {
        Memory mem = currentProgram.getMemory();
        java.util.TreeMap<Long,Integer> hits = new java.util.TreeMap<>();
        for (MemoryBlock b : mem.getBlocks()) {
            if (!b.isInitialized()) continue;
            long s = (b.getStart().getOffset() + 3) & ~3L, e = b.getEnd().getOffset();
            for (long a = s; a + 4 <= e + 1; a += 4) {
                long v; try { v = mem.getInt(toAddr(a)) & 0xFFFFFFFFL; } catch (Exception ex) { continue; }
                // the SoC decodes 24 address bits plus the 0xFFF00000 window; anything else is an
                // instruction pair or a constant, not a pointer. Flash/RAM side and small negatives out.
                boolean lowSpace = v >= 0x800000L && v < 0x1000000L;
                boolean topWindow = v >= 0xFFF00000L && v < 0xFFFFFF00L;
                if (!lowSpace && !topWindow) continue;
                hits.merge(v & ~0xFFFL, 1, Integer::sum);
            }
        }
        long runLo = -1, runHi = -1;
        java.util.ArrayList<long[]> runs = new java.util.ArrayList<>();
        for (java.util.Map.Entry<Long,Integer> en : hits.entrySet()) {
            if (en.getValue() < MIN_HITS) continue;
            long p = en.getKey();
            if (runLo >= 0 && p == runHi + 0x1000L) { runHi = p; continue; }
            if (runLo >= 0) runs.add(new long[]{runLo, runHi});
            runLo = runHi = p;
        }
        if (runLo >= 0) runs.add(new long[]{runLo, runHi});
        for (long[] r : runs) {
            long lo = r[0], len = r[1] - r[0] + 0x1000L;
            // skip anything already covered (fixed list / previous run); trim to the uncovered part
            while (len > 0 && mem.getBlock(toAddr(lo)) != null) { lo += 0x1000L; len -= 0x1000L; }
            while (len > 0 && mem.getBlock(toAddr(lo + len - 1)) != null) len -= 0x1000L;
            if (len <= 0) continue;
            boolean clash = false;
            for (long a = lo; a < lo + len; a += 0x1000L) if (mem.getBlock(toAddr(a)) != null) clash = true;
            if (clash) { println("  skip discovered " + Long.toHexString(lo) + " (overlaps a mapped block)"); continue; }
            make(String.format("MMIO_auto_%06X", lo), lo, len, true);
        }
    }

    private void make(String name, long start, long len, boolean vol) {
        Address a = toAddr(start);
        Memory mem = currentProgram.getMemory();
        if (mem.getBlock(a) != null) { println("  skip " + name + " (already mapped at " + a + ")"); return; }
        try {
            MemoryBlock b = mem.createUninitializedBlock(name, a, len, false);
            b.setRead(true); b.setWrite(true); b.setExecute(false); b.setVolatile(vol);
            println(String.format("  created %-14s %s .. %s  (%d KiB)%s",
                    name, a, a.add(len - 1), len / 1024, vol ? " volatile" : ""));
        } catch (Exception e) {
            println("  FAILED " + name + ": " + e.getMessage());
        }
    }
}
