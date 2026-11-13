// Recover every CALL TARGET, wherever it is in the image. A called address is code by
// definition, so this pass does not need the [codeLO, codeHI) recovery window the other passes
// are bounded by: it resolves the target of every `bsr` (11-bit displacement in the encoding),
// `jsri` and tail `jmpi` (the pointer in the literal-pool slot the instruction reads) inside a
// TRUSTED function, and disassembles + functionizes the ones that are still raw bytes.
//
// Two kinds of function hide from everything else and are what this recovers:
//   - prologue-less leaves ABOVE the auto-pinned code/data boundary (a handful of small helpers
//     at the very top of the code, `cmpnei; movi; lsli; ...; jmp r15`). The boundary is the end
//     of the last function that starts with the frame prologue, so DataSectionClean cleared them
//     to undefined and labelled their entries `tbl_` table bases, and the recovery window never
//     reached them again;
//   - callees whose only reference is a `jsri` that carries NO reference to its pool slot (code
//     disassembled by the scripts often has no operand references at all), so RecoverFlowCode
//     could not compute the target, nothing else pointed at the bytes, and they stayed `??` with
//     no reference — the decompiler shows `(*(code *)&UNK_000c6eb8)(1,...)`.
//
// Trusted = every function whose entry is in [CODE_LO, P), P = end of the last prologue
// function (the same pin DataSectionClean uses), plus — transitively — every function this pass
// reaches from one. Spurious functions in the data section (auto-analysis noise, before the
// clean step) are never trusted, so their `bsr`-shaped bytes seed nothing. Guards on a seed:
// the target is 2-aligned, inside [CODE_LO, DATA_HI), not inside a pointer table, undefined or a
// small scalar nothing reads/writes (a `tbl_` label or a ClassifyData `uint8_t` on the entry is
// fine), and pseudo-decodes cleanly to an unconditional terminator within 64 instructions with
// no 0x0000/0xFFFF padding word; a `bsr` that is padding (0xFFFF = `bsr .`) is ignored.
//
// For each seed the body is traced here as well (fall-through, and `bt/bf/br` targets computed
// from the encoding and RECORDED as flow references — above the pin no other pass will do it),
// then the function is created, generic `tbl_` labels at the entry dropped (a curated name is
// kept and becomes the function's name). The call's flow reference (COMPUTED_CALL / the `bsr`
// flow type / COMPUTED_JUMP for `jmpi`) and the missing READ reference from a `jsri`/`jmpi` to
// its pool slot are recorded, so body tracing, FixLiteralPools, ExportSigs and the decompiler
// see the edge. DataSectionClean protects (never deletes, clears or labels) a function that is
// a call target of trusted code and extends the pinned DATA_LO past the last one.
//
// Args: CODE_LO DATA_HI (hex, no 0x) — the image span. Idempotent (PtrTargets reports what is
// left; want 0).
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

public class RecoverCallTargets extends GhidraScript {
    private long LO = 0x42000L, HI = Long.MAX_VALUE;
    private Memory mem;
    private Listing ls;
    private ReferenceManager rm;
    private PseudoDisassembler pd;
    private final ArrayList<long[]> tableRanges = new ArrayList<>();
    private int flowRefs = 0, slotRefs = 0, branchRefs = 0, disasm = 0, fns = 0, islands = 0,
                skippedTyped = 0, retraced = 0, skippedPoolSlot = 0, skippedUnclean = 0, skippedTable = 0, misaligned = 0, failed = 0, sites = 0;

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        if (a.length >= 2) { LO = Long.parseLong(a[0], 16); HI = Long.parseLong(a[1], 16); }
        mem = currentProgram.getMemory(); ls = currentProgram.getListing();
        rm = currentProgram.getReferenceManager(); pd = new PseudoDisassembler(currentProgram);
        if (HI == Long.MAX_VALUE) HI = mem.getMaxAddress().getOffset() + 1;
        findTableRanges();

        long P = highestPrologueEnd();
        ArrayDeque<Address> work = new ArrayDeque<>();
        HashSet<Address> seen = new HashSet<>();
        for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
            long e = f.getEntryPoint().getOffset();
            if (e >= LO && e < P && seen.add(f.getEntryPoint())) work.add(f.getEntryPoint());
        }
        int trusted = seen.size();

        while (!work.isEmpty()) {
            Function f = getFunctionAt(work.poll());
            if (f == null) continue;
            ArrayList<Address> retrace = new ArrayList<>();
            ArrayList<Instruction> body = new ArrayList<>();
            for (Instruction i : ls.getInstructions(f.getBody(), true)) body.add(i);
            for (Instruction ins : body) {
                String m = ins.getMnemonicString();
                boolean bsr = m.equals("bsr"), jsri = m.equals("jsri"), jmpi = m.equals("jmpi");
                if (!bsr && !jsri && !jmpi) continue;
                int w = halfword(ins.getAddress());
                if (bsr && (w == 0xFFFF || w == 0xF800)) continue;       // padding / `bsr .+2`: data
                sites++;
                Address t;
                if (bsr) t = branchTarget(ins);
                else {
                    Address slot = slotFromRefs(ins);
                    if (slot == null) {
                        slot = toAddr(((ins.getAddress().getOffset() + 2) & ~3L) + ((w & 0xFF) << 2));
                        if (!mem.contains(slot)) continue;
                        try { rm.addMemoryReference(ins.getAddress(), slot, RefType.READ, SourceType.ANALYSIS, 0); slotRefs++; } catch (Exception e) {}
                    }
                    if (!mem.contains(slot)) continue;
                    long v; try { v = mem.getInt(slot) & 0xFFFFFFFFL; } catch (Exception e) { continue; }
                    t = toAddr(v & ~1L);
                }
                if (t == null || !mem.contains(t)) continue;
                long o = t.getOffset();
                if (o < LO || o >= HI) continue;
                if ((o & 1) != 0) { misaligned++; continue; }
                if (isPoolSlot(t)) { skippedPoolSlot++; continue; }   // an lrw/jsri/jmpi reads it: data, the caller is junk
                boolean call = !jmpi;

                if (getInstructionAt(t) == null) {
                    String site = ins.getAddress() + " " + m + " in " + f.getName();
                    if (getInstructionContaining(t) != null) { misaligned++; note("mid-instruction", t, site); continue; }
                    if (inTable(o)) { skippedTable++; note("in pointer table", t, site); continue; }
                    if (!seedable(t)) { skippedTyped++; note("typed/referenced data", t, site); continue; }
                    if (!cleanToTerminator(t, 64)) { skippedUnclean++; note("dirty decode", t, site); continue; }
                    trace(t);
                    if (getInstructionAt(t) == null) { failed++; continue; }
                    disasm++;
                    if (jmpi && getFunctionContaining(t) == null) retrace.add(f.getEntryPoint());   // body must absorb it
                }
                // the call edge itself
                boolean has = false;
                for (Reference r : ins.getReferencesFrom()) if (r.getReferenceType().isFlow() && t.equals(r.getToAddress())) { has = true; break; }
                if (!has) {
                    RefType rt = jmpi ? RefType.COMPUTED_JUMP : jsri ? RefType.COMPUTED_CALL : ins.getFlowType();
                    try { rm.addMemoryReference(ins.getAddress(), t, rt, SourceType.ANALYSIS, bsr ? 0 : CodeUnit.MNEMONIC); flowRefs++; } catch (Exception e) {}
                }
                // functionize a called entry that no function owns
                Function g = getFunctionAt(t);
                if (g == null && call && getFunctionContaining(t) == null) {
                    dropGenericLabels(t);
                    try { g = createFunction(t, null); } catch (Exception e) { g = null; }
                    if (g == null) { failed++; } else { fns++; if (o >= P) islands++; }
                }
                if (g == null) g = getFunctionContaining(t);
                if (g != null && seen.add(g.getEntryPoint())) work.add(g.getEntryPoint());   // now trusted
            }
            // A tail-jump target disassembled while walking this body belongs to it (Ghidra's
            // body tracing follows jumps), but the body was computed before it was code — and
            // above the pin no window-bounded pass (RetraceBodies) will grow it. Re-create.
            if (!retrace.isEmpty()) {
                for (Address e : retrace) {
                    Function g = getFunctionAt(e); if (g == null) continue;
                    String name = g.getName(); long before = g.getBody().getNumAddresses();
                    try {
                        removeFunction(g);
                        Function n = createFunction(e, null);
                        if (n == null) { createFunction(e, name); continue; }
                        if (!name.startsWith("FUN_") && !n.getName().equals(name)) n.setName(name, SourceType.USER_DEFINED);
                        if (n.getBody().getNumAddresses() > before) retraced++;
                    } catch (Exception ex) {}
                }
                retrace.clear();
            }
        }
        for (String n : notes) println("  skipped: " + n);
        println("RecoverCallTargets: trustedFns=" + trusted + " pin=" + Long.toHexString(P) + " callSites=" + sites
                + " disassembled=" + disasm + " newFunctions=" + fns + " abovePin=" + islands + " bodiesRetraced=" + retraced
                + " flowRefsAdded=" + flowRefs + " slotRefsAdded=" + slotRefs + " branchRefsAdded=" + branchRefs
                + " skippedTyped=" + skippedTyped + " skippedUnclean=" + skippedUnclean + " skippedTable=" + skippedTable + " skippedPoolSlot=" + skippedPoolSlot
                + " misaligned=" + misaligned + " failed=" + failed);
    }

    private final ArrayList<String> notes = new ArrayList<>();
    private void note(String why, Address t, String site) { if (notes.size() < 24) notes.add(String.format("%s %s <- %s", t, why, site)); }

    // Disassemble from `entry`, following fall-through and bt/bf/br targets computed from the
    // encoding (recorded as flow references), through raw bytes only. Bounded.
    private void trace(Address entry) {
        ArrayDeque<Address> q = new ArrayDeque<>(); q.add(entry);
        HashSet<Address> visited = new HashSet<>();
        int budget = 20000;
        while (!q.isEmpty() && budget > 0) {
            Address a = q.poll();
            if (!visited.add(a)) continue;
            long o = a.getOffset();
            if (o < LO || o >= HI) continue;
            if (getInstructionAt(a) == null) {
                if (getInstructionContaining(a) != null || inTable(o) || !seedable(a)) continue;
                if (!a.equals(entry) && !cleanRun(a, 16)) continue;
                clearScalarsAt(a);
                try { disassemble(a); } catch (Exception e) {}
                if (getInstructionAt(a) == null) continue;
            }
            Instruction i = getInstructionAt(a);
            while (i != null && budget-- > 0) {
                String m = i.getMnemonicString();
                if (m.equals("bt") || m.equals("bf") || m.equals("br") || m.equals("bsr")) {
                    Address t = branchTarget(i);
                    if (t != null && mem.contains(t) && halfword(i.getAddress()) != 0xFFFF) {
                        if (i.getFlows().length == 0) {
                            try { rm.addMemoryReference(i.getAddress(), t, i.getFlowType(), SourceType.ANALYSIS, 0); branchRefs++; } catch (Exception e) {}
                        }
                        if (!m.equals("bsr") && !visited.contains(t)) q.add(t);
                    }
                } else if (m.equals("jmpi")) {
                    // A tail jump's target that is not a function belongs to THIS body (Ghidra's
                    // body tracing follows jumps), so it must be code before the function is
                    // created: above the pin no later pass (RetraceBodies is window-bounded) grows
                    // the body, and DataSectionClean clears code no protected function owns.
                    Address slot = slotFromRefs(i);
                    if (slot == null) {
                        slot = toAddr(((i.getAddress().getOffset() + 2) & ~3L) + ((halfword(i.getAddress()) & 0xFF) << 2));
                        if (mem.contains(slot)) try { rm.addMemoryReference(i.getAddress(), slot, RefType.READ, SourceType.ANALYSIS, 0); slotRefs++; } catch (Exception e) {}
                    }
                    try {
                        if (mem.contains(slot)) {
                            Address t = toAddr((mem.getInt(slot) & 0xFFFFFFFFL) & ~1L);
                            if (mem.contains(t) && (t.getOffset() & 1) == 0) {
                                if (i.getFlows().length == 0) { rm.addMemoryReference(i.getAddress(), t, RefType.COMPUTED_JUMP, SourceType.ANALYSIS, CodeUnit.MNEMONIC); flowRefs++; }
                                if (getFunctionAt(t) == null && getFunctionContaining(t) == null && !visited.contains(t)) q.add(t);
                            }
                        }
                    } catch (Exception e) {}
                }
                Address ft = i.getFallThrough();
                if (ft == null) break;
                Instruction n = getInstructionAt(ft);
                if (n == null) { q.add(ft); break; }
                if (!visited.add(ft)) break;
                i = n;
            }
        }
    }

    // A PC-relative pool slot: some lrw/jsri/jmpi reads it. Never a function entry.
    private boolean isPoolSlot(Address a) {
        for (Reference r : getReferencesTo(a)) {
            if (!r.getReferenceType().isRead()) continue;
            Instruction i = getInstructionAt(r.getFromAddress());
            if (i == null) continue;
            String m = i.getMnemonicString();
            if (m.equals("lrw") || m.equals("jsri") || m.equals("jmpi")) return true;
        }
        return false;
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

    // Undefined, or a small scalar (<= 4 bytes, not pointer/array/struct/string) that nothing
    // reads, writes or jumps to.
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

    // Clean pseudo-decode to an unconditional terminator within n instructions; no padding words.
    private boolean cleanToTerminator(Address a, int n) {
        try {
            Address p = a;
            for (int i = 0; i < n; i++) {
                int w = halfword(p);
                if (w == 0 || w == 0xFFFF) return false;
                PseudoInstruction ins = pd.disassemble(p);
                if (ins == null) return false;
                String m = ins.getMnemonicString();
                if (m.equals("br") || m.equals("jmpi") || m.equalsIgnoreCase("jmp") || m.equals("rts") || m.equals("rte") || m.equals("rfi")) return true;
                int l = ins.getLength(); if (l <= 0) return false;
                p = p.add(l);
            }
        } catch (Exception e) {}
        return false;
    }

    // Clean pseudo-decode for n instructions, or to a terminator first; no padding words.
    private boolean cleanRun(Address a, int n) {
        try {
            Address p = a;
            for (int i = 0; i < n; i++) {
                int w = halfword(p);
                if (w == 0 || w == 0xFFFF) return false;
                PseudoInstruction ins = pd.disassemble(p);
                if (ins == null) return false;
                String m = ins.getMnemonicString();
                if (m.equals("br") || m.equals("jmpi") || m.equalsIgnoreCase("jmp") || m.equals("rts") || m.equals("rte") || m.equals("rfi")) return true;
                int l = ins.getLength(); if (l <= 0) return false;
                p = p.add(l);
            }
            return true;
        } catch (Exception e) { return false; }
    }

    private int halfword(Address a) { try { return mem.getShort(a) & 0xFFFF; } catch (Exception e) { return -1; } }

    private Address branchTarget(Instruction ins) {
        for (Address t : ins.getFlows()) if (t != null) return t;
        int w = halfword(ins.getAddress());
        if (w < 0) return null;
        int disp = w & 0x7FF; if ((disp & 0x400) != 0) disp -= 0x800;
        try { return ins.getAddress().add(2 + (disp << 1)); } catch (Exception e) { return null; }
    }

    private Address slotFromRefs(Instruction ins) {
        for (Reference r : ins.getReferencesFrom()) {
            RefType rt = r.getReferenceType();
            Address s = r.getToAddress();
            if ((rt.isRead() || rt.isData()) && !rt.isFlow() && s != null && mem.contains(s)) return s;
        }
        return null;
    }

    private long highestPrologueEnd() {
        long best = LO;
        for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
            long e = f.getEntryPoint().getOffset();
            if (e < LO || e >= HI) continue;
            int h0 = halfword(f.getEntryPoint()), h1 = halfword(f.getEntryPoint().add(2));
            if ((h0 & 0xFE0F) != 0x2400 || (h1 & 0xFFF0) != 0x0070) continue;
            long end = f.getBody().getMaxAddress().getOffset() + 1;
            if (end > best) best = end;
        }
        return (best + 1) & ~1L;
    }

    // Runs of >= 4 consecutive 4-byte image pointers: jump/pointer tables, never a call target.
    private void findTableRanges() {
        long rs = -1; int rl = 0;
        for (long p = LO & ~3L; p + 4 <= HI; p += 4) {
            long v; try { v = mem.getInt(toAddr(p)) & 0xFFFFFFFFL; } catch (Exception e) { v = 0; }
            boolean ptr = (v >= LO && v < HI);
            if (ptr) { if (rs < 0) { rs = p; rl = 0; } rl++; }
            else { if (rs >= 0 && rl >= 4) tableRanges.add(new long[]{rs, rs + rl * 4L}); rs = -1; rl = 0; }
        }
        if (rs >= 0 && rl >= 4) tableRanges.add(new long[]{rs, rs + rl * 4L});
    }
    private boolean inTable(long o) { for (long[] t : tableRanges) if (o >= t[0] && o < t[1]) return true; return false; }
}
