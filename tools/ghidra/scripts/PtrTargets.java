// Read-only: are all CALL TARGETS code? Walks every `bsr` / `jsri` (and tail `jmpi`) inside a
// function of the image and resolves its target — `bsr` from the 11-bit displacement in the
// encoding, `jsri`/`jmpi` from the pointer in the literal-pool slot the instruction reads —
// independent of any recovery window, and reports the targets that are NOT instruction starts.
// A called address is code by definition, so each such target is a function the recovery
// never disassembled (or cleared): a prologue-less leaf above the auto-pinned code/data
// boundary that DataSectionClean wiped and labelled `tbl_`, or a callee with no other
// reference that no seed ever reached. Neither raises an error bookmark, an orphan run or an
// undefined prologue, so this is the only check that sees them. Gated (**want 0**, mcore_verify.sh
// exits 4 otherwise) are the targets RecoverCallTargets would seed — raw/mis-typed-scalar bytes
// that pseudo-decode cleanly to a terminator (`callTargetsNotCode`, `tailJumpTargetsNotCode`);
// a target that is typed pointer/table data or decodes dirty is a junk call site (a pool word or
// padding decoded as `bsr`/`jsri` inside a spurious function) and is listed as `callTargetsSuspect`.
//
// Also reports, informationally: call targets that are instructions but not function entries
// (an alternate entry into another body), and literal-pool pointers (lrw'd function pointers)
// whose target is undefined bytes that pseudo-decode like a function (prologue, or a clean
// leaf that reaches `jmp r15`/`rts`) — candidates, not proof, since a table can decode cleanly.
//
// Two more classes, with the prologue pin as a third argument: `poolSlotsInCode` — the pool
// slot of an `lrw`/`jsri`/`jmpi` (by reference or encoding) that lies inside an instruction, i.e. a
// literal pool decoded as code (FixLiteralPools removes the spurious function that claimed it;
// want 0) — and `tablePtrTargetsNotCode` — 4-aligned words from the pin up whose value is a
// code-region address at raw bytes that decode like a function (prologue or clean leaf): code
// pointers in untyped data-section tables, RecoverTablePointers' seeds (want 0).
//
// Args: CODE_LO DATA_HI [CODE_HI] (hex, no 0x): the image span whose functions are walked and
// whose addresses count as targets (default 42000 / end of memory), and the prologue pin for the
// table sweep. Call sites below CODE_LO are ignored (0xFFFF padding in the VBR/calibration-
// directory region decodes as `bsr`).
//@category MK60
import ghidra.app.script.GhidraScript;
import ghidra.app.util.PseudoDisassembler;
import ghidra.app.util.PseudoInstruction;
import ghidra.program.model.address.Address;
import ghidra.program.model.data.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.symbol.*;
import java.util.*;

public class PtrTargets extends GhidraScript {
    private long LO = 0x42000L, HI = Long.MAX_VALUE;
    private Memory mem;
    private PseudoDisassembler pd;

    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        if (a.length >= 2) { LO = Long.parseLong(a[0], 16); HI = Long.parseLong(a[1], 16); }
        long PIN = a.length >= 3 ? Long.parseLong(a[2], 16) : -1;
        mem = currentProgram.getMemory();
        pd = new PseudoDisassembler(currentProgram);
        if (HI == Long.MAX_VALUE) HI = mem.getMaxAddress().getOffset() + 1;
        Listing ls = currentProgram.getListing();

        int padding = 0, sites = 0, nBsr = 0, nJsri = 0, nJmpi = 0, noSlot = 0, unmapped = 0, outside = 0, misaligned = 0;
        TreeMap<Long, String> callBad = new TreeMap<>();     // target -> description
        TreeMap<Long, String> tailBad = new TreeMap<>();
        TreeMap<Long, String> suspect = new TreeMap<>();
        TreeMap<Long, String> notEntry = new TreeMap<>();
        HashSet<Long> seenPool = new HashSet<>();
        TreeMap<Long, String> poolCodeLike = new TreeMap<>();
        TreeMap<Long, String> slotInCode = new TreeMap<>();     // pool slot that lies inside an instruction

        for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
            long e = f.getEntryPoint().getOffset();
            if (e < LO || e >= HI) continue;
            for (Instruction ins : ls.getInstructions(f.getBody(), true)) {
                String m = ins.getMnemonicString();
                boolean bsr = m.equals("bsr"), jsri = m.equals("jsri"), jmpi = m.equals("jmpi");
                if ((m.equals("lrw") || jsri || jmpi) && trusted(f, ins.getAddress())) {
                    Address slot = poolSlot(ins);
                    if (slot != null && mem.contains(slot) && slot.getOffset() >= LO && slot.getOffset() < HI) {
                        Instruction ic = getInstructionContaining(slot);
                        if (ic != null) {
                            Function cf = getFunctionContaining(slot);
                            slotInCode.put(slot.getOffset(), ins.getAddress() + " " + m + " in " + f.getName() + " [instruction@" + ic.getAddress()
                                    + (cf == null ? ", orphan code" : ", in " + cf.getName()) + "]");
                        }
                    }
                }
                // literal-pool pointers (lrw'd function pointers): candidates only
                if (!bsr && !jsri && !jmpi) {
                    for (Reference r : ins.getReferencesFrom()) {
                        RefType rt = r.getReferenceType();
                        if (!(rt.isRead() || rt.isData()) || rt.isFlow()) continue;
                        Address slot = r.getToAddress();
                        if (slot == null || !mem.contains(slot) || !initialized(slot)) continue;
                        if (slot.getOffset() < LO || slot.getOffset() >= HI) continue;
                        if (!seenPool.add(slot.getOffset())) continue;
                        long v; try { v = mem.getInt(slot) & 0xFFFFFFFFL; } catch (Exception ex) { continue; }
                        if (v < LO || v >= HI || (v & 1) != 0) continue;
                        Address t = toAddr(v);
                        if (getInstructionContaining(t) != null) continue;
                        Data d = ls.getDataContaining(t);
                        if (d != null && d.isDefined() && !(d.getDataType() instanceof Undefined)) continue;
                        if (isPrologue(t) || leafReturns(t))
                            poolCodeLike.put(v, String.format("slot %06x read by %s in %s", slot.getOffset(), ins.getAddress(), f.getName()));
                    }
                    continue;
                }
                int w; try { w = mem.getShort(ins.getAddress()) & 0xFFFF; } catch (Exception ex) { continue; }
                if (bsr && (w == 0xFFFF || w == 0xF800)) { padding++; continue; }   // 0xFFFF fill = `bsr .`
                sites++;
                if (bsr) nBsr++; else if (jsri) nJsri++; else nJmpi++;
                Address t = bsr ? branchTarget(ins) : indirectTarget(ins);
                if (t == null) { noSlot++; continue; }
                long o = t.getOffset();
                if (!mem.contains(t)) { unmapped++; continue; }
                if (o < LO || o >= HI) { outside++; continue; }
                if ((o & 1) != 0) { misaligned++; continue; }
                String where = ins.getAddress() + " " + m + " in " + f.getName();
                if (getInstructionAt(t) != null) {
                    if (!jmpi && getFunctionAt(t) == null) {
                        Function c = getFunctionContaining(t);
                        notEntry.put(o, where + (c == null ? " [orphan code]" : " [inside " + c.getName() + "]"));
                    }
                    continue;
                }
                String what = describe(t);
                boolean real = seedable(t) && cleanToTerminator(t, 64);   // what RecoverCallTargets would seed
                if (!real) suspect.put(o, where + " " + what);
                else if (jmpi) tailBad.put(o, where + " " + what); else callBad.put(o, where + " " + what);
            }
        }

        // code pointers in untyped data-section tables (from the prologue pin up): a target that
        // is raw bytes decoding like a function is one RecoverTablePointers would seed (want 0)
        TreeMap<Long, String> tableBad = new TreeMap<>(), tableNotEntry = new TreeMap<>();
        int tableWords = 0;
        if (PIN > 0) {
            for (long p = (PIN + 3) & ~3L; p + 4 <= HI; p += 4) {
                long v; try { v = mem.getInt(toAddr(p)) & 0xFFFFFFFFL; } catch (Exception ex) { continue; }
                if (v < LO || v >= PIN || (v & 1) != 0) continue;
                tableWords++;
                Address t = toAddr(v);
                if (getInstructionAt(t) != null) {
                    if (getFunctionAt(t) == null && getFunctionContaining(t) == null) tableNotEntry.put(v, String.format("word %06x [orphan code]", p));
                    continue;
                }
                if (getInstructionContaining(t) != null) continue;
                if (!seedable(t)) continue;
                if (isPrologue(t) || leafReturns(t)) tableBad.put(v, String.format("word %06x %s", p, describe(t)));
            }
        }
        println("PtrTargets: callSites=" + sites + " (bsr=" + nBsr + " jsri=" + nJsri + " jmpi=" + nJmpi + ")"
                + " callTargetsNotCode=" + callBad.size() + " tailJumpTargetsNotCode=" + tailBad.size()
                + " callTargetsSuspect=" + suspect.size() + " callTargetsNotFnEntry=" + notEntry.size() + " poolPtrCodeLikeNotCode=" + poolCodeLike.size()
                + " paddingBsrIgnored=" + padding
                + " poolSlotsInCode=" + slotInCode.size()
                + " tablePtrWords=" + tableWords + " tablePtrTargetsNotCode=" + tableBad.size() + " tablePtrTargetsOrphanCode=" + tableNotEntry.size()
                + " noSlotRef=" + noSlot + " slotFromEncoding=" + slotFromEncoding + " unmapped=" + unmapped + " outsideImage=" + outside + " misaligned=" + misaligned);
        list("call target not code", callBad);
        list("tail-jump target not code", tailBad);
        list("pool pointer to code-like undefined bytes", poolCodeLike);
        list("pool slot inside an instruction (pool decoded as code)", slotInCode);
        list("table pointer to function-like raw bytes", tableBad);
        list("table pointer to orphan code", tableNotEntry);
        list("suspect: call target is typed data / dirty decode (junk call site?)", suspect);
        list("call target is an instruction but not a function entry", notEntry);
    }

    private void list(String title, TreeMap<Long, String> m) {
        if (m.isEmpty()) return;
        int n = 0;
        for (Map.Entry<Long, String> e : m.entrySet()) {
            if (n++ >= 40) { println("  ... " + (m.size() - 40) + " more"); break; }
            println(String.format("  %s %06x <- %s", title, e.getKey(), e.getValue()));
        }
    }

    // Same trust as FixLiteralPools: an lrw counts only if reachable from the entry of a plausibly
    // real function (named, prologue-started, or referred to from outside its body by data or by
    // trusted code — two rounds, so junk referring to junk drops out) without crossing an
    // rte/rfi/bkpt/stop/wait/doze/sync or a 0xFFFF/0x0000 halfword.
    private static final Set<String> STOP = new HashSet<>(Arrays.asList("rte", "rfi", "bkpt", "stop", "wait", "doze", "sync"));
    private HashSet<Long> R = null;
    private boolean trusted(Function f, Address ia) {
        if (R == null) {
            R = null;
            for (int round = 0; round < 2; round++) {
                HashSet<Long> next = new HashSet<>();
                for (Function g : currentProgram.getFunctionManager().getFunctions(true)) {
                    if (!candidateReal(g, R)) continue;
                    for (Address a : reachable(g)) next.add(a.getOffset());
                }
                R = next;
            }
        }
        return R.contains(ia.getOffset());
    }
    private boolean candidateReal(Function f, HashSet<Long> R) {
        if (!f.getName().startsWith("FUN_") || isPrologue(f.getEntryPoint())) return true;
        for (Reference r : getReferencesTo(f.getEntryPoint())) {
            RefType rt = r.getReferenceType();
            if (rt.isRead() || rt.isWrite()) continue;
            Address from = r.getFromAddress();
            if (f.getBody().contains(from)) continue;
            if (getInstructionAt(from) == null) return true;
            if (R == null || R.contains(from.getOffset())) return true;
        }
        return false;
    }
    private ArrayList<Address> reachable(Function f) {
        ArrayList<Address> out = new ArrayList<>(); HashSet<Address> seen = new HashSet<>();
        ArrayDeque<Address> q = new ArrayDeque<>(); q.add(f.getEntryPoint());
        while (!q.isEmpty() && out.size() < 20000) {
            Address a = q.poll();
            if (!seen.add(a)) continue;
            Instruction ins = getInstructionAt(a);
            if (ins == null || !f.getBody().contains(a)) continue;
            int w; try { w = mem.getShort(a) & 0xFFFF; } catch (Exception e) { continue; }
            if (w == 0xFFFF || w == 0) continue;
            out.add(a);
            if (STOP.contains(ins.getMnemonicString())) continue;
            Address ft = ins.getFallThrough(); if (ft != null) q.add(ft);
            if (!ins.getFlowType().isCall()) for (Address t : ins.getFlows()) if (t != null && f.getBody().contains(t)) q.add(t);
        }
        return out;
    }

    private String describe(Address t) {
        StringBuilder sb = new StringBuilder("[");
        Instruction ic = getInstructionContaining(t);
        if (ic != null) sb.append("mid-instruction@").append(ic.getAddress());
        else {
            Data d = currentProgram.getListing().getDataContaining(t);
            if (d == null || !d.isDefined()) sb.append("undefined");
            else sb.append("data ").append(d.getDataType().getName()).append('@').append(d.getAddress());
        }
        Function c = getFunctionContaining(t);
        if (c != null) sb.append(" in ").append(c.getName());
        Symbol s = getSymbolAt(t);
        if (s != null) sb.append(" sym=").append(s.getName());
        sb.append(cleanToTerminator(t, 64) ? " decodes-clean" : " decode-dirty");
        return sb.append(']').toString();
    }

    // Undefined, or a small scalar (<= 4 bytes, not pointer/array/struct/string) nothing reads or writes.
    private boolean seedable(Address a) {
        Data d = currentProgram.getListing().getDataContaining(a);
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

    private boolean initialized(Address a) { MemoryBlock b = mem.getBlock(a); return b != null && b.isInitialized(); }

    // PC-relative branch target from the 16-bit encoding: disp = bits[0..10] signed.
    private Address branchTarget(Instruction ins) {
        try {
            byte[] b = ins.getBytes();
            int w = ((b[0] & 0xFF) << 8) | (b[1] & 0xFF);
            int disp = w & 0x7FF; if ((disp & 0x400) != 0) disp -= 0x800;
            return ins.getAddress().add(2 + (disp << 1));
        } catch (Exception e) { return null; }
    }

    // jsri/jmpi: the pointer in the pool slot the instruction reads. The slot comes from the
    // instruction's read/data reference when it has one, else from the encoding:
    // slot = ((PC + 2) & ~3) + (imm8 << 2) — the M·CORE lrw/jsri/jmpi addressing mode. Code
    // disassembled by the recovery scripts often carries no operand reference at all.
    private Address indirectTarget(Instruction ins) {
        for (Address t : ins.getFlows()) if (t != null && mem.contains(t)) return t;   // resolved already
        Address slot = poolSlot(ins);
        if (slot == null || !mem.contains(slot)) return null;
        try { return toAddr((mem.getInt(slot) & 0xFFFFFFFFL) & ~1L); } catch (Exception e) { return null; }
    }

    private Address poolSlot(Instruction ins) {
        for (Reference r : ins.getReferencesFrom()) {
            RefType rt = r.getReferenceType();
            Address s = r.getToAddress();
            if ((rt.isRead() || rt.isData()) && !rt.isFlow() && s != null && mem.contains(s)) return s;
        }
        try {
            byte[] b = ins.getBytes();
            long pc = ins.getAddress().getOffset();
            slotFromEncoding++;
            return toAddr(((pc + 2) & ~3L) + ((b[1] & 0xFF) << 2));
        } catch (Exception e) { return null; }
    }
    private int slotFromEncoding = 0;

    // Straight-line pseudo-decode from a reaches an unconditional terminator within n
    // instructions with no invalid encoding (bt/bf fall through; br/jmpi/jmp/rts/rte/rfi end).
    private boolean cleanToTerminator(Address a, int n) {
        try {
            Address p = a;
            for (int i = 0; i < n; i++) {
                int w = mem.getShort(p) & 0xFFFF;
                if (w == 0 || w == 0xFFFF) return false;          // padding word: not code
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

    private boolean isPrologue(Address a) {
        try {
            int h0 = mem.getShort(a) & 0xFFFF, h1 = mem.getShort(a.add(2)) & 0xFFFF;
            return (h0 & 0xFE0F) == 0x2400 && (h1 & 0xFFF0) == 0x0070;
        } catch (Exception e) { return false; }
    }

    // Clean leaf: reaches `jmp r15`/`rts` within 40 instructions, no br/jmpi before it.
    private boolean leafReturns(Address a) {
        try {
            Address p = a;
            for (int i = 0; i < 40; i++) {
                int w = mem.getShort(p) & 0xFFFF;
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
}
