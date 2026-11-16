// Fix M-CORE literal pools that auto-analysis mis-disassembled as code.
//
// M-CORE has no large immediates: lrw/jmpi/jsri load a 32-bit constant or
// pointer from a PC-relative literal pool, which the compiler drops right after
// each function. Ghidra's analysis falls through an unconditional tail jmpi/br
// into that pool, decodes some of it as instructions, collides with the data the
// lrw reference already created, and marks "bad instruction" -- which truncates
// the decompiled function and paints the scrollbar red.
//
// This pass harvests every in-flash *read* target of an instruction (= a pool
// slot), clears whatever code landed on it, and defines it as a 4-byte pointer
// (if the value lands in mapped memory) or a dword. Flow targets (call/jump/
// branch destinations) and function entry points are excluded, so real code is
// never touched. Then it drops stale "Bad Instruction" error bookmarks whose
// address is no longer an instruction. Idempotent; re-runnable.
//
// Three things auto-analysis left that this also undoes: (1) an `lrw`/`jsri`/`jmpi` with no
// operand reference — its slot is computed from the encoding and the READ reference recorded;
// (2) a literal pool decoded as code and functionized (a FUN_ whose entry is a PC-relative slot;
// ~95 per image, each hiding the pool of a real function and sometimes the real function after
// it) — removed, then typed; (3) a real body that ran through its own pool after an `rfi`/`rte`
// the SLEIGH does not model as terminal — the slot is typed and the body re-created. PtrTargets
// reports `poolSlotsInCode` (want 0).
//
// Bounded: only slots inside [LO, HI) — the code region — are typed. Literal pools live
// between functions; a calibration table above dataLO that an instruction reads is NOT a
// pool slot, and typing its first 4 bytes as a pointer whenever they happen to look like a
// flash address is a mistake ClassifyData/ApplyDataTypes then (rightly) refuse to undo.
// Table bases in the data section are DataSectionClean's (labels) and ClassifyData's
// (width) business. With no args the old unbounded behaviour is kept.
//
// Usage (headless, writes back -- close the GUI first for the project lock):
//   -process 7846816A.cpu.hex -noanalysis -postScript FixLiteralPools.java [LO HI]
//@category MK60
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSetView;
import ghidra.program.model.data.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.RefType;
import ghidra.program.model.symbol.SourceType;
import java.util.*;

public class FixLiteralPools extends GhidraScript {
    private long LO = Long.MIN_VALUE, HI = Long.MAX_VALUE;

    @Override
    public void run() throws Exception {
        String[] _bnd = getScriptArgs(); if (_bnd.length >= 2) { LO = Long.parseLong(_bnd[0], 16); HI = Long.parseLong(_bnd[1], 16); }
        Listing listing = currentProgram.getListing();
        Memory mem = currentProgram.getMemory();
        BookmarkManager bm = currentProgram.getBookmarkManager();

        int errBefore = countErrors(bm);

        // Pool slots come in two kinds. `pcrel` = the slot of a PC-relative pool operand
        // (`lrw`/`jsri`/`jmpi`): data by construction — the instruction reads a 32-bit constant
        // there — so it is exempt from the fall-through arbitration below, a flow reference INTO it
        // is junk (0xFFFF padding decodes as `bsr .`), and a function whose entry is one is a pool
        // decoded as code. The slot is the instruction's read/data reference when it has one, else
        // computed from the encoding, slot = ((PC + 2) & ~3) + (imm8 << 2), and the READ reference
        // recorded (script-disassembled code often carries no operand reference at all).
        // The other kind is a plain in-flash read target (absolute `ld`), kept under the old rules.
        //
        // Only TRUSTED `lrw`s are believed. A pool word that decodes as `lrw` names a fake slot up to
        // 1 KB ahead, possibly inside real code, and such words sit in two places: functions that
        // ARE a pool decoded as code (unnamed, no prologue, nothing outside them refers to them),
        // and the tail of a real body that ran through `rte`/`rfi`/`bkpt`/padding into its own pool
        // (the SLEIGH does not end flow there). So trusted = reachable from the entry of a
        // plausibly real function without crossing such a word (see reachable()), and a slot that
        // lies inside trusted code is rejected as fake.
        FunctionManager fmgr = currentProgram.getFunctionManager();
        HashSet<Long> R = trustedInstructions(fmgr);
        HashSet<Long> pcrel = new HashSet<>();
        int slotRefsAdded = 0, fakeSlots = 0;
        int untrustedSlots = 0;
        InstructionIterator hit = listing.getInstructions(true);
        while (hit.hasNext()) {
            Instruction ins = hit.next();
            Address ia = ins.getAddress();
            if (getFunctionContaining(ia) == null) continue;              // orphan junk: believe nothing
            boolean trustedLrw = R.contains(ia.getOffset());
            {
                String mn = ins.getMnemonicString();
                if (!(mn.equals("lrw") || mn.equals("jsri") || mn.equals("jmpi"))) continue;
                Address slot = null;
                for (Reference r : ins.getReferencesFrom()) {
                    RefType rt = r.getReferenceType();
                    if ((rt.isRead() || rt.isData()) && !rt.isFlow() && r.getToAddress() != null && r.getToAddress().isMemoryAddress()) { slot = r.getToAddress(); break; }
                }
                if (slot == null) {
                    try { byte[] b = ins.getBytes(); slot = toAddr(((ia.getOffset() + 2) & ~3L) + ((b[1] & 0xFF) << 2)); } catch (Exception e) { continue; }
                    if (!mem.contains(slot)) continue;
                    try { currentProgram.getReferenceManager().addMemoryReference(ia, slot, RefType.READ, SourceType.ANALYSIS, 0); slotRefsAdded++; } catch (Exception e) {}
                }
                long o = slot.getOffset();
                if (!mem.contains(slot) || o < LO || o >= HI) continue;
                MemoryBlock blk = mem.getBlock(slot);
                if (blk == null || !blk.isInitialized()) continue;
                Instruction ic = getInstructionContaining(slot);
                if (ic != null && R.contains(ic.getAddress().getOffset())) { fakeSlots++; continue; }   // inside trusted code: a junk lrw's slot
                // an lrw outside trusted code (junk referred to only by junk) is believed only about
                // bytes nobody owns: typing orphan junk or raw bytes as a dword can damage nothing
                if (!trustedLrw) { if (getFunctionContaining(slot) != null) { fakeSlots++; continue; } untrustedSlots++; }
                pcrel.add(o);
            }
        }

        // Remove the functions whose entry is a (trusted) pcrel slot: a pool decoded as code (auto-
        // analysis fell into it after a `bkpt`/padding gap and functionized it; it may even have
        // swallowed the real function after the pool, which the orphan/call-target passes then
        // re-own). A name on such an entry (the symbol port carries the reference's own junk names)
        // stays behind as a label on the data; a prologue-started function is kept and reported.
        int spuriousRemoved = 0, namedRemoved = 0, prologueAtSlot = 0;
        ArrayList<Function> del = new ArrayList<>();
        for (Function f : fmgr.getFunctions(true)) {
            long e = f.getEntryPoint().getOffset();
            if (!pcrel.contains(e)) continue;
            if (isPrologue(f.getEntryPoint())) { prologueAtSlot++; continue; }
            del.add(f);
        }
        for (Function f : del) {
            boolean named = !f.getName().startsWith("FUN_");
            try { removeFunction(f); spuriousRemoved++; if (named) namedRemoved++; } catch (Exception ex) {}
        }

        // Addresses that are real code: every flow destination of a trusted instruction + every
        // remaining function entry — minus the pcrel slots, which are data whatever points at them.
        HashSet<Long> codeTargets = new HashSet<>();
        HashSet<Long> pool = new HashSet<>(pcrel);
        int ramRefs = 0, outOfRange = 0;
        InstructionIterator iit = listing.getInstructions(true);
        while (iit.hasNext()) {
            Instruction ins = iit.next();
            Function owner = getFunctionContaining(ins.getAddress());
            if (owner == null) continue;                                   // orphan junk: believe nothing
            String mn = ins.getMnemonicString();
            boolean pc = mn.equals("lrw") || mn.equals("jsri") || mn.equals("jmpi");
            for (Reference r : ins.getReferencesFrom()) {
                RefType rt = r.getReferenceType();
                Address to = r.getToAddress();
                if (to == null || !to.isMemoryAddress()) continue;
                if (rt.isFlow()) { codeTargets.add(to.getOffset()); continue; }
                if (pc) continue;                                          // pcrel slots were harvested above
                if (rt.isWrite() || !(rt.isRead() || rt.isData())) continue;
                if (!mem.contains(to)) continue;   // unmapped constants aren't flash pools
                // An absolute read is a 32-bit pool word only when it is one: 4-aligned and not a
                // byte/halfword access. A `ld.h` of a small in-flash table (RAM-pointer table at
                // 0x650..0x65F on the pre-E5 image, read at +0xE) is table data; clearing "its 4
                // bytes" from the halfword wiped the entry of the function right after the table.
                if ((to.getOffset() & 3) != 0 || mn.equals("ld.b") || mn.equals("ld.h")) { outOfRange++; continue; }
                if (to.getOffset() < LO || to.getOffset() >= HI) { outOfRange++; continue; }   // data-section table, not a pool
                MemoryBlock blk = mem.getBlock(to);
                if (blk == null || !blk.isInitialized()) { ramRefs++; continue; }  // RAM variable, not a pool
                pool.add(to.getOffset());
            }
        }
        for (Function f : fmgr.getFunctions(true)) codeTargets.add(f.getEntryPoint().getOffset());
        for (long e : pcrel) if (fmgr.getFunctionAt(toAddr(e)) == null) codeTargets.remove(e);
        pool.removeAll(codeTargets);

        ArrayList<Long> slots = new ArrayList<>(pool);
        Collections.sort(slots);

        PointerDataType ptr = new PointerDataType(currentProgram.getDataTypeManager());
        DWordDataType dword = new DWordDataType();
        int made = 0, ptrs = 0, skipped = 0, failed = 0;
        HashMap<Address, String> holed = new HashMap<>();   // real functions whose body contained a slot
        ArrayList<String> failEx = new ArrayList<>();   // first few failures, for the log
        byte[] b = new byte[4];
        for (long off : slots) {
            Address p = toAddr(off);
            try {
                Data d = getDataAt(p);
                if (d != null && d.isDefined() && d.getLength() == 4) { skipped++; continue; }
                // Inside (or at the base of) a typed table — an Array/Structure a curator or the
                // annotation replay applied — this is table data, not a pool slot: clearing the
                // 4 bytes would destroy the whole item and leave fragments for RecoverCodeRuns.
                Data dc = listing.getDataContaining(p);
                if (dc != null && (dc.getDataType() instanceof Array || dc.getDataType() instanceof Structure)) { skipped++; continue; }
                // Arbitration: a read-target reached by fall-through from a non-terminator is
                // truncated-function CODE (see FixTruncated), not a literal pool -- leave it.
                // Real pools sit after a terminator (jmpi/br/rts) and have no fall-through in.
                // Not for a PC-relative slot: that is data whatever falls into it (a pool decoded
                // as code is exactly a chain of junk instructions falling through each other).
                boolean pcSlot = pcrel.contains(off);
                Instruction prevI = getInstructionBefore(p);
                if (!pcSlot && prevI != null && p.equals(prevI.getFallThrough())) { skipped++; continue; }
                // Never clear through a function entry: a slot whose 4 bytes overlap the entry of a
                // function that starts with the frame prologue is table data next to real code.
                boolean entryInside = false;
                for (long q = off; q < off + 4; q += 2) { Function qf = fmgr.getFunctionAt(toAddr(q)); if (qf != null && (q != off || isPrologue(toAddr(q)))) entryInside = true; }
                if (entryInside) { skipped++; continue; }
                if (pcSlot && getInstructionContaining(p) != null) {
                    Function cf = getFunctionContaining(p);
                    if (cf != null) holed.put(cf.getEntryPoint(), cf.getName());   // body ran into its pool (e.g. after `rfi`)
                }
                clearListing(p, p.add(3));
                mem.getBytes(p, b);
                long v = ((long)(b[0] & 0xFF) << 24) | ((b[1] & 0xFF) << 16)
                        | ((b[2] & 0xFF) << 8) | (b[3] & 0xFF);
                boolean isPtr = false;
                if (v != 0) {
                    try { isPtr = mem.contains(toAddr(v)); } catch (Exception ignore) {}
                }
                createData(p, isPtr ? ptr : dword);
                if (isPtr) ptrs++;
                made++;
            } catch (Exception e) {
                // A slot that can't be cleared/typed, e.g. its 4 bytes run off the end of a
                // block (0x8ed in the low config block). Stable across passes and harmless.
                failed++;
                if (failEx.size() < 3) { String m = String.valueOf(e.getMessage()); failEx.add(p + ":" + e.getClass().getSimpleName() + "[" + (m.length() > 70 ? m.substring(0, 70) : m) + "]"); }
            }
        }

        // A real function whose body had run into a pool (an `rfi`/`rte` the SLEIGH does not model
        // as terminal, then padding, then the pool): re-create it so the body stops at the data.
        int bodiesRetraced = 0, stubsRemoved = 0;
        for (Map.Entry<Address, String> e : holed.entrySet()) {
            Function f = getFunctionAt(e.getKey()); if (f == null) continue;
            String name = e.getValue();
            try {
                removeFunction(f);
                Function nf = createFunction(e.getKey(), null);
                if (nf == null) { createFunction(e.getKey(), name); continue; }
                if (!name.startsWith("FUN_") && !nf.getName().equals(name)) nf.setName(name, SourceType.USER_DEFINED);
                bodiesRetraced++;
                // what is left of a pool decoded as code whose entry was the padding halfword before
                // the pool: an unnamed, unreferenced stub of a word or two — drop it
                if (nf.getBody().getNumAddresses() <= 4 && !candidateReal(nf, null)) { removeFunction(nf); stubsRemoved++; }
            } catch (Exception ex) {}
        }

        // Orphan padding decoded as code around the pools just typed — a lone `bkpt` (0x0000) or
        // `bsr .` (0xFFFF) that no function owns and nothing but itself refers to — is cleared.
        int paddingCleared = 0;
        ArrayList<Address> pad = new ArrayList<>();
        InstructionIterator pit = listing.getInstructions(new ghidra.program.model.address.AddressSet(toAddr(LO), toAddr(Math.min(HI, mem.getMaxAddress().getOffset() + 1) - 1)), true);
        while (pit.hasNext()) {
            Instruction ins = pit.next();
            int w; try { w = mem.getShort(ins.getAddress()) & 0xFFFF; } catch (Exception e) { continue; }
            if (w != 0 && w != 0xFFFF) continue;
            if (getFunctionContaining(ins.getAddress()) != null) continue;
            boolean referred = false;
            for (Reference r : getReferencesTo(ins.getAddress())) {
                Address from = r.getFromAddress();
                if (from.equals(ins.getAddress())) continue;
                if (getInstructionAt(from) == null || getFunctionContaining(from) != null) { referred = true; break; }
            }
            if (!referred) pad.add(ins.getAddress());
        }
        for (Address a : pad) { try { clearListing(a, a.add(1)); paddingCleared++; } catch (Exception e) {} }

        // Drop stale bad-instruction bookmarks now sitting on data.
        ArrayList<Bookmark> stale = new ArrayList<>();
        Iterator<Bookmark> bit = bm.getBookmarksIterator(BookmarkType.ERROR);
        while (bit.hasNext()) {
            Bookmark bk = bit.next();
            if (getInstructionAt(bk.getAddress()) == null) stale.add(bk);
        }
        for (Bookmark bk : stale) bm.removeBookmark(bk);

        int errAfter = countErrors(bm);
        println("FixLiteralPools: poolSlots=" + slots.size() + " defined=" + made
                + " pointers=" + ptrs + " alreadyData=" + skipped + " ramRefsSkipped=" + ramRefs + " outsideCodeRegion=" + outOfRange
                + " failed=" + failed + " slotRefsAdded=" + slotRefsAdded + " fakeSlotsIgnored=" + fakeSlots + " slotsFromUntrustedLrw=" + untrustedSlots + " spuriousFnsRemoved=" + spuriousRemoved
                + " (named=" + namedRemoved + ") prologueFnsAtSlot=" + prologueAtSlot + " bodiesRetraced=" + bodiesRetraced + " stubsRemoved=" + stubsRemoved + " paddingCleared=" + paddingCleared
                + (failEx.isEmpty() ? "" : " (failed = slot can't be typed, harmless; e.g. " + String.join(" ", failEx) + ")"));
        println("FixLiteralPools: errorBookmarks before=" + errBefore
                + " removedStale=" + stale.size() + " after=" + errAfter);
    }

    private static final Set<String> STOP = new HashSet<>(Arrays.asList("rte", "rfi", "bkpt", "stop", "wait", "doze", "sync"));

    // Instructions reachable from f's entry by fall-through and in-body jumps, never crossing an
    // rte/rfi/bkpt/stop/wait/doze/sync or a 0xFFFF/0x0000 halfword (padding and pool words the
    // SLEIGH decodes without ending flow).
    private ArrayList<Address> reachable(Function f) {
        ArrayList<Address> out = new ArrayList<>();
        HashSet<Address> seen = new HashSet<>();
        ArrayDeque<Address> q = new ArrayDeque<>(); q.add(f.getEntryPoint());
        AddressSetView body = f.getBody();
        Memory mem = currentProgram.getMemory();
        while (!q.isEmpty() && out.size() < 20000) {
            Address a = q.poll();
            if (!seen.add(a)) continue;
            Instruction ins = getInstructionAt(a);
            if (ins == null || !body.contains(a)) continue;
            int w; try { w = mem.getShort(a) & 0xFFFF; } catch (Exception e) { continue; }
            if (w == 0xFFFF || w == 0) continue;
            out.add(a);
            if (STOP.contains(ins.getMnemonicString())) continue;
            Address ft = ins.getFallThrough(); if (ft != null) q.add(ft);
            if (!ins.getFlowType().isCall()) for (Address t : ins.getFlows()) if (t != null && body.contains(t)) q.add(t);
        }
        return out;
    }

    // Named, prologue-started, or referred to (call, jump, pointer — not a read) from outside its
    // own body by data or by trusted code (R null = by any instruction).
    private boolean candidateReal(Function f, HashSet<Long> R) {
        if (!f.getName().startsWith("FUN_") || isPrologue(f.getEntryPoint())) return true;
        // An entry that a PC-relative operand of another function READS is a pool word by
        // construction, whatever else points at it (a pool constant elsewhere that happens to
        // equal its address gets typed as a pointer and would otherwise vouch for it).
        for (Reference r : getReferencesTo(f.getEntryPoint())) {
            if (!r.getReferenceType().isRead() || f.getBody().contains(r.getFromAddress())) continue;
            Instruction ri = getInstructionAt(r.getFromAddress());
            if (ri == null) continue;
            String rm = ri.getMnemonicString();
            if (rm.equals("lrw") || rm.equals("jsri") || rm.equals("jmpi")) return false;
        }
        for (Reference r : getReferencesTo(f.getEntryPoint())) {
            RefType rt = r.getReferenceType();
            if (rt.isRead() || rt.isWrite()) continue;
            Address from = r.getFromAddress();
            if (f.getBody().contains(from)) continue;
            if (getInstructionAt(from) == null) return true;               // from data
            if (R == null || R.contains(from.getOffset())) return true;
        }
        return false;
    }

    // Two rounds: plausibly real functions by any outside reference, then only by references from
    // code that round found trusted (junk referring to junk drops out).
    private HashSet<Long> trustedInstructions(FunctionManager fmgr) {
        HashSet<Long> R = null;
        for (int round = 0; round < 2; round++) {
            HashSet<Long> next = new HashSet<>();
            for (Function f : fmgr.getFunctions(true)) {
                if (!candidateReal(f, R)) continue;
                for (Address a : reachable(f)) next.add(a.getOffset());
            }
            R = next;
        }
        return R;
    }

    private boolean isPrologue(Address e) {
        try {
            int h0 = currentProgram.getMemory().getShort(e) & 0xFFFF, h1 = currentProgram.getMemory().getShort(e.add(2)) & 0xFFFF;
            return (h0 & 0xFE0F) == 0x2400 && (h1 & 0xFFF0) == 0x0070;
        } catch (Exception ex) { return false; }
    }

    private int countErrors(BookmarkManager bm) {
        int n = 0;
        Iterator<Bookmark> it = bm.getBookmarksIterator(BookmarkType.ERROR);
        while (it.hasNext()) { it.next(); n++; }
        return n;
    }
}
