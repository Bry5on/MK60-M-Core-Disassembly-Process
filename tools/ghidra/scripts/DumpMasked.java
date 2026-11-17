// Read-only: dump the relocation-masked instruction halfword stream of every function, for the
// fuzzy (n-gram) symbol port in tools/fuzzy_port.py. Same masking as ExportSigs.java — the only
// things that differ between two builds of the same source are the pool-slot displacement of
// lrw/jmpi/jsri (top nibble 7) and the displacement of bt/bf/br/bsr (top nibble E/F); opcodes,
// registers, immediates and ld/st offsets are kept — so two variants of one function share most
// of their halfword n-grams even when a few instructions were added, dropped or re-ordered and
// the exact FNV signature no longer matches.
// Line: entry \t nInstr \t name \t hw0 hw1 hw2 …   (masked halfwords, 4 hex digits each, in body order)
// Args: out-file.
//@category MK60
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.Memory;
import java.io.*;

public class DumpMasked extends GhidraScript {
    @Override public void run() throws Exception {
        String out = getScriptArgs()[0];
        Memory mem = currentProgram.getMemory();
        Listing lst = currentProgram.getListing();
        int fns = 0, hws = 0;
        try (BufferedWriter w = new BufferedWriter(new FileWriter(out), 1 << 20)) {
            for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
                StringBuilder sb = new StringBuilder();
                int n = 0;
                InstructionIterator it = lst.getInstructions(f.getBody(), true);
                while (it.hasNext()) {
                    Instruction in = it.next();
                    int len = in.getLength();
                    for (int off = 0; off + 2 <= len; off += 2) {
                        int hw;
                        try { hw = mem.getShort(in.getAddress().add(off)) & 0xFFFF; } catch (Exception ex) { hw = 0; }
                        int top = hw >> 12;
                        int masked;
                        if (top == 0x7) masked = hw & 0xFF00;                  // lrw/jmpi/jsri: drop disp8 (pool slot)
                        else if (top == 0xE || top == 0xF) masked = hw & 0xF800; // bt/bf/br/bsr: drop disp
                        else masked = hw;
                        if (sb.length() > 0) sb.append(' ');
                        sb.append(String.format("%04x", masked));
                        hws++;
                    }
                    n++;
                }
                w.write(String.format("%06x\t%d\t%s\t%s%n", f.getEntryPoint().getOffset(), n, f.getName(), sb));
                fns++;
            }
        }
        println("DumpMasked: wrote " + out + " (functions=" + fns + " halfwords=" + hws + ")");
    }
}
