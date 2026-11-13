// Recover functions whose only reference is a code pointer stored in an UNTYPED table in the
// data section. The CAN signal descriptor tables (0xD8xxx) hold the address of a getter per
// signal (`lrw; ld.b; lsri; andi; jmp r15`, 4–6 bytes apart), the KWP service table (0xF30A0)
// the handler per record at +4 (`kwp_3e_tester_present`): an untyped table word creates no
// Ghidra reference, so RecoverRefCode (which seeds from DATA references) never saw them, the
// prologue scan does not see a leaf, and nothing calls them directly — about 75 functions per
// image stayed raw `??`, named by the symbol port but never disassembled.
//
// Sweep: every 4-aligned word in [SWEEP_LO, DATA_HI) — the data section from the prologue pin
// up, islands included — whose value is a 2-aligned address in [CODE_LO, CODE_HI), the code
// region proper. Seed guards, the same as RecoverCallTargets': not inside a pointer-table run,
// undefined or a small scalar nothing reads/writes, and the bytes pseudo-decode as a function —
// the frame prologue, or a clean leaf that reaches `jmp r15`/`rts` within 40 instructions with
// no branch out and no 0x0000/0xFFFF padding word (a calibration value can look like a code
// address, so this is deliberately stricter than "decodes cleanly"). A target that is already an
// instruction start owned by no function is functionized as well (the handler a spurious pool
// function had swallowed, once FixLiteralPools removed that function). A DATA reference from
// the table word to the target is recorded so body tracing, FunctionizeOrphans, ClassifyData
// and DataTableInventory see the edge; the word itself is not typed (data types in the data
// section are the curator's / ApplyAnnotations' business).
//
// Args: CODE_LO CODE_HI SWEEP_LO DATA_HI (hex, no 0x). Idempotent; PtrTargets reports what is
// left (`tablePtrTargetsNotCode`, want 0).
//@category MK60
import ghidra.app.script.GhidraScript;
import ghidra.app.util.PseudoDisassembler;
import ghidra.app.util.PseudoInstruction;
import ghidra.program.model.address.*;
import ghidra.program.model.data.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.symbol.*;
import java.util.*;

public class RecoverTablePointers extends GhidraScript {
    private long LO = 0x42000L, HI = 0xD7000L, SLO = 0xD7000L, DHI = 0xF6A24L;
    private Memory mem; private Listing ls; private ReferenceManager rm; private PseudoDisassembler pd;
    private final ArrayList<long[]> tableRanges = new ArrayList<>();

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        if (a.length >= 4) { LO = Long.parseLong(a[0], 16); HI = Long.parseLong(a[1], 16); SLO = Long.parseLong(a[2], 16); DHI = Long.parseLong(a[3], 16); }
        mem = currentProgram.getMemory(); ls = currentProgram.getListing();
        rm = currentProgram.getReferenceManager(); pd = new PseudoDisassembler(currentProgram);
        findTableRanges();

        int words = 0, toCode = 0, seeds = 0, disasm = 0, fns = 0, refs = 0, orphanFn = 0,
            skippedTyped = 0, skippedTable = 0, skippedDecode = 0, midBody = 0, failed = 0;
        ArrayList<String> notes = new ArrayList<>();
        for (long p = (SLO + 3) & ~3L; p + 4 <= DHI; p += 4) {
            long v; try { v = mem.getInt(toAddr(p)) & 0xFFFFFFFFL; } catch (Exception e) { continue; }
            if (v < LO || v >= HI || (v & 1) != 0) continue;
            words++;
            Address w = toAddr(p), t = toAddr(v);
            if (getInstructionAt(t) != null) {
                toCode++;
                Function g = getFunctionAt(t);
                if (g == null) {
                    if (getFunctionContaining(t) != null) { midBody++; continue; }
                    dropGenericLabels(t);
                    try { g = createFunction(t, null); } catch (Exception e) { g = null; }
                    if (g == null) { failed++; continue; }
                    orphanFn++; fns++;
                }
                if (addRef(w, t)) refs++;
                continue;
            }
            if (getInstructionContaining(t) != null) { midBody++; continue; }
            if (inTable(v)) { skippedTable++; continue; }
            if (!seedable(t)) { skippedTyped++; continue; }
            if (!(isPrologue(t) || leafReturns(t))) { skippedDecode++; continue; }
            seeds++;
            clearScalarsAt(t);
            try { disassemble(t); } catch (Exception e) {}
            if (getInstructionAt(t) == null) { failed++; continue; }
            disasm++;
            dropGenericLabels(t);
            Function g = null;
            try { g = createFunction(t, null); } catch (Exception e) {}
            if (g == null) failed++; else fns++;
            if (addRef(w, t)) refs++;
            if (notes.size() < 12) notes.add(String.format("%06x <- word %06x%s", v, p, g != null && !g.getName().startsWith("FUN_") ? " (" + g.getName() + ")" : ""));
        }
        if (!notes.isEmpty()) println("RecoverTablePointers: seeded: " + String.join(" ", notes) + (seeds > notes.size() ? " ..." : ""));
        println("RecoverTablePointers: codePtrWords=" + words + " toInstruction=" + toCode + " seeds=" + seeds + " disassembled=" + disasm
                + " newFunctions=" + fns + " (orphanCodeFunctionized=" + orphanFn + ") refsAdded=" + refs
                + " skippedTyped=" + skippedTyped + " skippedTable=" + skippedTable + " skippedNotFunctionLike=" + skippedDecode
                + " midBody=" + midBody + " failed=" + failed);
    }

    private boolean addRef(Address w, Address t) {
        for (Reference r : rm.getReferencesFrom(w)) if (t.equals(r.getToAddress())) return false;
        try { rm.addMemoryReference(w, t, RefType.DATA, SourceType.ANALYSIS, 0); return true; } catch (Exception e) { return false; }
    }

    private void clearScalarsAt(Address a) {
        for (long q = a.getOffset(); q < a.getOffset() + 4; ) {
            Data d = ls.getDataContaining(toAddr(q));
            if (d != null && d.isDefined()) { try { clearListing(d.getAddress(), d.getMaxAddress()); } catch (Exception e) {} q = d.getMaxAddress().getOffset() + 1; }
            else q++;
        }
    }

    private void dropGenericLabels(Address a) {
        for (Symbol s : currentProgram.getSymbolTable().getSymbols(a)) {
            String n = s.getName();
            if (s.getSource() != SourceType.DEFAULT && (n.startsWith("tbl_") || n.startsWith("DAT_") || n.startsWith("LAB_") || n.startsWith("PTR_")))
                try { s.delete(); } catch (Exception e) {}
        }
    }

    private boolean seedable(Address a) {
        Data d = ls.getDataContaining(a);
        if (d == null || !d.isDefined() || d.getDataType() instanceof Undefined || d.getDataType() instanceof DefaultDataType) return true;
        if (d.getLength() > 4) return false;
        DataType dt = d.getDataType();
        if (dt instanceof Pointer || dt instanceof Array || dt instanceof Structure || dt instanceof Union || dt instanceof AbstractStringDataType) return false;
        for (Reference r : getReferencesTo(d.getAddress())) {
            RefType rt = r.getReferenceType();
            if (rt.isRead() || rt.isWrite()) return false;
        }
        return true;
    }

    private int halfword(Address a) { try { return mem.getShort(a) & 0xFFFF; } catch (Exception e) { return -1; } }

    private boolean isPrologue(Address a) {
        int h0 = halfword(a), h1 = halfword(a.add(2));
        return (h0 & 0xFE0F) == 0x2400 && (h1 & 0xFFF0) == 0x0070;
    }

    // Clean leaf: >= 2 instructions, reaches `jmp r15`/`rts` within 40, no br/jmpi/bsr/jsri
    // before it, no padding word.
    private boolean leafReturns(Address a) {
        try {
            Address p = a;
            for (int i = 0; i < 40; i++) {
                int w = halfword(p);
                if (w == 0 || w == 0xFFFF) return false;
                PseudoInstruction ins = pd.disassemble(p);
                if (ins == null) return false;
                String m = ins.getMnemonicString();
                if (m.equals("rts") || (m.equalsIgnoreCase("jmp") && ins.getDefaultOperandRepresentation(0).equals("r15"))) return i >= 1;
                if (m.equals("br") || m.equals("jmpi") || m.equals("bsr") || m.equals("jsri")) return false;
                int l = ins.getLength(); if (l <= 0) return false;
                p = p.add(l);
            }
        } catch (Exception e) {}
        return false;
    }

    private void findTableRanges() {
        long rs = -1; int rl = 0;
        for (long p = LO & ~3L; p + 4 <= DHI; p += 4) {
            long v; try { v = mem.getInt(toAddr(p)) & 0xFFFFFFFFL; } catch (Exception e) { v = 0; }
            boolean ptr = (v >= LO && v < DHI);
            if (ptr) { if (rs < 0) { rs = p; rl = 0; } rl++; }
            else { if (rs >= 0 && rl >= 4) tableRanges.add(new long[]{rs, rs + rl * 4L}); rs = -1; rl = 0; }
        }
        if (rs >= 0 && rl >= 4) tableRanges.add(new long[]{rs, rs + rl * 4L});
    }
    private boolean inTable(long o) { for (long[] t : tableRanges) if (o >= t[0] && o < t[1]) return true; return false; }
}
