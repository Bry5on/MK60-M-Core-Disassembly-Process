// WRITE: de-noise the data/table section. `auto DATA_HI` pins P = end of the last function that
// starts with the frame prologue (the code/data boundary for any image; explicit `P DATA_HI`
// also accepted). From P up: (1) delete every function that is neither a prologue function nor a
// PROTECTED one — a call / tail-jump target of trusted code (see protectedFunctions; the
// prologue-less leaf helpers above the last prologue function that RecoverCallTargets seeds);
// (2) clear every other instruction back to undefined; (3) label each code-referenced table base
// (same harvest as DataTableInventory: Ghidra refs from outside the section + a 4-aligned pointer
// scan of imageBase..P), never one that is code. Args: `auto DATA_HI [CODE_LO]`. Prints `DATA_LO=<end of last real fn, protected
// included> CODE_HI=<P> DATA_HI=..` for the driver: DATA_LO starts the data section proper,
// CODE_HI stays the recovery-window ceiling. Does NOT impose data types — bases stay labeled
// untyped data. Close the GUI first (project lock). Re-runnable.
//@category MK60
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressIterator;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.listing.*;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.symbol.*;
import java.util.*;

public class DataSectionClean extends GhidraScript {
    // Credit: the documented table/symbol names in KNOWN below are drawn from cmg5461's
    // analysis (7846816A_symbols.md, https://github.com/cmg5461/mk60e5-firmware).
    static final Map<Long,String> KNOWN = new HashMap<>();
    private long CODE_LO = 0x42000L, FLASH_LO = 0x40000L;

    // Flash base = start of the largest initialized block (the image itself; RAM/MMIO blocks are
    // uninitialized): 0x40000 on the E5 .0pa images (VBR set by crt0), 0x0 on a full-chip dump of
    // the earlier MK60 (vector table at the reset address).
    private long imageBase() {
        long base = 0, best = -1;
        for (ghidra.program.model.mem.MemoryBlock b : currentProgram.getMemory().getBlocks()) {
            if (!b.isInitialized()) continue;
            if (b.getSize() > best) { best = b.getSize(); base = b.getStart().getOffset(); }
        }
        return base;
    }

    static void k(long a,String n){ KNOWN.put(a,n); }
    static {
        k(0xd6da4L,"AYC_feature_enable_per_mode"); k(0xd6e3aL,"veh_override_A_front");
        k(0xd6ecaL,"veh_Cf_cornering_stiffness"); k(0xd6ee2L,"veh_Cr_cornering_stiffness");
        k(0xd6f42L,"veh_l_f"); k(0xd6f5aL,"veh_l_r"); k(0xd6f72L,"veh_mass"); k(0xd6f8aL,"veh_Jz_yaw_inertia");
        k(0xd6fa2L,"veh_track_front"); k(0xd6fbaL,"veh_track_rear"); k(0xd7032L,"veh_override_enable_flag");
        k(0xd704aL,"AYC_lagblend_y"); k(0xd70aaL,"AYC_lagblend_x");
        k(0xd71f4L,"misr_range_table"); k(0xd7424L,"eeprom_block_table");
        k(0xd75a0L,"asic_init_words"); k(0xd75d2L,"asic_init_reg"); k(0xd75d4L,"asic_init_val");
        k(0xd7634L,"valve_channel_mask_table"); k(0xd766cL,"direction_invert_mask_coding");
        k(0xd7792L,"asic_wheel_read_commands"); k(0xd79f8L,"asic_reply_index_to_wheel");
        k(0xd79fcL,"direction_invert_wheel_mask"); k(0xd7b84L,"master_app_object");
        k(0xd7f45L,"ptcan_header"); k(0xd7f65L,"ptcan_msg_table"); k(0xd839eL,"ptcan_signal_table");
        k(0xd88c0L,"ptcan_variable_table"); k(0xda158L,"inlet_dp_axis"); k(0xda2f0L,"inlet_scalar_block");
        k(0xda5f8L,"pump_rate_rom"); k(0xda604L,"pump_flow_x_axis");
        k(0xe6b50L,"cos_table_C_8000"); k(0xea9d0L,"cos_table_D_16000");
        k(0xf30a0L,"kwp_service_table"); k(0xf31b4L,"eeprom_job_descriptors");
        k(0xf367cL,"dtc_table"); k(0xf576cL,"wss_direction_fault_id"); k(0xf57c4L,"fsf_block");
        k(0xf57d6L,"fsf_speedmon_limit_y"); k(0xf5822L,"fsf_speed_class_ladder"); k(0xf587eL,"fsf_mode_clamp");
        k(0xf59c3L,"fcan_header"); k(0xf59ddL,"fcan_msg_table"); k(0xf5cbdL,"fcan_signal_table");
        k(0xf5ed0L,"fcan_variable_table"); k(0xf61e6L,"tcs_curve_f61e6"); k(0xf61f6L,"tcs_driveslip_set_A");
        k(0xf625cL,"tcs_driveslip_set_B"); k(0xf654cL,"tcs_cap_curves"); k(0xf677aL,"coding_feature_tbl0");
        k(0xf67dcL,"coding_feature_tbl1"); k(0xf683eL,"coding_feature_tbl2"); k(0xf68a0L,"coding_field_limit_table");
        k(0xf690cL,"ecu_id_record"); k(0xf6910L,"sec27_seed_xor"); k(0xf691cL,"misr_word_end");
    }

    @Override public void run() throws Exception {
        Memory mem = currentProgram.getMemory();
        ReferenceManager rm = currentProgram.getReferenceManager();
        SymbolTable st = currentProgram.getSymbolTable();
        FunctionManager fm = currentProgram.getFunctionManager();

        // P = the prologue pin: end of the last function that starts with the frame prologue.
        // Everything from P up is cleaned (spurious functions deleted, instructions cleared,
        // bases labelled) EXCEPT the protected islands: functions that are call / tail-jump
        // targets of trusted code (RecoverCallTargets seeds them; prologue-less leaves such as
        // `cmpnei; movi; lsli; ...; jmp r15` sit above the last prologue function on every
        // MK60E5 image). DATA_LO, the start of the data section proper, is the end of the last
        // real function — prologue or protected — and is what the driver pins; P is reported as
        // CODE_HI, the ceiling of the recovery window (the tables between the islands must not
        // be re-disassembled every round).
        String[] _a = getScriptArgs();
        Function last = null;
        long P, DATA_HI;
        // CODE_LO (3rd arg, default 0x42000 = E5 layout) is the floor of trusted code; the
        // pointer scan for table bases starts at the image base.
        CODE_LO = _a.length >= 3 ? Long.parseLong(_a[2],16) : 0x42000L;
        FLASH_LO = imageBase();
        if (_a.length >= 2 && _a[0].equalsIgnoreCase("auto")) {  // auto-pin (any image)
            DATA_HI = Long.parseLong(_a[1],16);
            P = highestPrologueEnd(CODE_LO, DATA_HI);
            println("DataSectionClean[auto]: prologue pin (end of last prologue fn) = " + Long.toHexString(P));
        } else if (_a.length >= 2) {    // explicit pin, e.g. for a different image
            P = Long.parseLong(_a[0],16); DATA_HI = Long.parseLong(_a[1],16);
        } else {                        // default: DSCM90, end of last real fn (0xd6704)
            last = getFunctionContaining(toAddr(0xd6704L));
            if (last == null) last = getFunctionAt(toAddr(0xd6704L));
            P = (last != null) ? last.getBody().getMaxAddress().getOffset()+1 : 0xd6714L;
            P = (P + 1) & ~1L;
            DATA_HI = 0xF6A24L;
        }

        TreeMap<Long,Function> prot = protectedFunctions(P, DATA_HI);
        long DATA_LO = P;
        for (Function f : prot.values()) {
            long end = (f.getBody().getMaxAddress().getOffset() + 2) & ~1L;
            if (end > DATA_LO) { DATA_LO = end; last = f; }
        }
        println("DATA_LO=" + Long.toHexString(DATA_LO) + " CODE_HI=" + Long.toHexString(P) + " DATA_HI=" + Long.toHexString(DATA_HI)
                + " (protected call-target fns above the pin: " + prot.size()
                + (last != null ? ", last real fn " + last.getName() + "@" + last.getEntryPoint() : "") + ")");

        // --- harvest bases (same logic as DataTableInventory), from the pin up; a reference
        //     from a protected island counts as "from outside the section" ---
        TreeMap<Long,TreeSet<Long>> bases = new TreeMap<>();
        AddressIterator srcIt = rm.getReferenceSourceIterator(mem, true);
        while (srcIt.hasNext()) {
            Address from = srcIt.next(); long f = from.getOffset();
            if (f >= P && f < DATA_HI && !inProtected(prot, from)) continue;
            for (Reference r : rm.getReferencesFrom(from)) {
                long t = r.getToAddress().getOffset();
                if (t >= P && t < DATA_HI) bases.computeIfAbsent(t,k->new TreeSet<>()).add(f);
            }
        }
        for (long a = (FLASH_LO+3)&~3L; a+4 <= P; a += 4) {
            long v; try { v = mem.getInt(toAddr(a)) & 0xFFFFFFFFL; } catch (Exception e) { continue; }
            if (v >= P && v < DATA_HI) bases.computeIfAbsent(v,k->new TreeSet<>()).add(a);
        }

        // --- (1) delete spurious functions from the pin up (never a prologue or protected fn) ---
        ArrayList<Function> del = new ArrayList<>();
        for (Function fn : fm.getFunctions(toAddr(P), true)) {
            if (fn.getEntryPoint().getOffset() >= DATA_HI) break;
            if (isPrologue(mem, fn.getEntryPoint())) continue;   // real function: never delete
            if (prot.containsKey(fn.getEntryPoint().getOffset())) continue;   // called from trusted code
            del.add(fn);
        }
        int fnDeleted = 0;
        for (Function fn : del) { try { removeFunction(fn); fnDeleted++; } catch (Exception ex) {} }

        // --- (2) clear mis-disassembled instructions from the pin up -> undefined ---
        ArrayList<Address> insAddrs = new ArrayList<>();
        InstructionIterator ii = currentProgram.getListing().getInstructions(
                new AddressSet(toAddr(P), toAddr(DATA_HI-1)), true);
        while (ii.hasNext()) insAddrs.add(ii.next().getMinAddress());
        int insCleared = 0;
        ArrayList<String> clearedRuns = new ArrayList<>(); long runLo = -1, runHi = -1;
        for (Address a : insAddrs) {
            Instruction in = getInstructionAt(a);
            if (in == null) continue;
            Function cf = getFunctionContaining(a);
            if (cf != null && (isPrologue(mem, cf.getEntryPoint()) || prot.containsKey(cf.getEntryPoint().getOffset()))) continue;  // body of a real fn
            long o = a.getOffset();
            if (runLo >= 0 && o <= runHi + 2) runHi = in.getMaxAddress().getOffset();
            else { if (runLo >= 0) clearedRuns.add(String.format("%x..%x", runLo, runHi)); runLo = o; runHi = in.getMaxAddress().getOffset(); }
            clearListing(a, in.getMaxAddress()); insCleared++;
        }
        if (runLo >= 0) clearedRuns.add(String.format("%x..%x", runLo, runHi));
        if (!clearedRuns.isEmpty()) println("DataSectionClean: cleared instruction runs (first " + Math.min(12, clearedRuns.size()) + " of " + clearedRuns.size() + "): "
                + String.join(" ", clearedRuns.subList(0, Math.min(12, clearedRuns.size()))));

        // --- (3) label bases.  Priority for the PRIMARY label: an existing real cmg5461 data name
        //     > a KNOWN documented name > a generated tbl_* name. Generic auto-labels (FUN_/LAB_/
        //     DAT_/SUB_/thunk) and any tbl_* from a previous run are dropped first. A base that is
        //     code (a protected island's entry, or inside its body) is never labelled a table; a
        //     stale tbl_* there from an earlier run is removed. ---
        int labels = 0, codeBases = 0;
        for (Map.Entry<Long,TreeSet<Long>> e : bases.entrySet()) {
            long base = e.getKey();
            Address ba = toAddr(base);
            if (getInstructionContaining(ba) != null) {
                codeBases++;
                for (Symbol s : st.getSymbols(ba)) if (s.getSource()!=SourceType.DEFAULT && s.getName().startsWith("tbl_")) { try { s.delete(); } catch (Exception ex) {} }
                continue;
            }
            String cmgName = null;      // a pre-existing real (non-generic) name = cmg5461 data label
            for (Symbol s : st.getSymbols(ba)) {
                if (s.getSource()==SourceType.DEFAULT) continue;
                String nm = s.getName();
                if (nm.startsWith("FUN_")||nm.startsWith("LAB_")||nm.startsWith("DAT_")
                    ||nm.startsWith("SUB_")||nm.startsWith("thunk")||nm.startsWith("tbl_")) { s.delete(); continue; }
                if (cmgName == null) cmgName = nm;      // keep the first real name
            }
            String name;
            if (cmgName != null) name = cmgName;         // keep cmg5461's own data name as primary
            else if (KNOWN.containsKey(base)) name = KNOWN.get(base);
            else {
                String reader = null;
                for (long s : e.getValue()) {
                    Function fn = getFunctionContaining(toAddr(s));
                    if (fn != null && !fn.getName().startsWith("FUN_")) { reader = fn.getName(); break; }
                }
                if (reader == null) for (long s : e.getValue()) {    // pool-slot: reader references the slot
                    for (Reference r2 : rm.getReferencesTo(toAddr(s))) {
                        Function fn2 = getFunctionContaining(r2.getFromAddress());
                        if (fn2 != null && !fn2.getName().startsWith("FUN_")) { reader = fn2.getName(); break; }
                    }
                    if (reader != null) break;
                }
                name = (reader != null) ? "tbl_" + reader + "_" + String.format("%06x",base)
                                        : "tbl_" + String.format("%06x",base);
            }
            name = name.replaceAll("[^A-Za-z0-9_]","_");
            try { createLabel(ba, name, true, SourceType.USER_DEFINED); labels++; } catch (Exception ex) {}
        }
        println("DataSectionClean: functionsDeleted=" + fnDeleted + " instructionsCleared=" + insCleared +
                " basesLabeled=" + labels + " totalBases=" + bases.size() + " codeBasesSkipped=" + codeBases
                + " protectedFns=" + prot.size());
    }

    // Functions at or above the pin P that trusted code calls or tail-jumps to, transitively:
    // trusted = every function whose entry is in [0x42000, P), plus the protected ones. Edges
    // are the instructions' call/jump flow references (RecoverCallTargets / RecoverFlowCode
    // record them), with the `bsr` displacement and the `jsri`/`jmpi` pool slot decoded from
    // the encoding as a fallback for an instruction that carries none.
    private TreeMap<Long,Function> protectedFunctions(long P, long HI) {
        Memory mem = currentProgram.getMemory();
        Listing ls = currentProgram.getListing();
        TreeMap<Long,Function> out = new TreeMap<>();
        ArrayDeque<Function> work = new ArrayDeque<>();
        HashSet<Long> seen = new HashSet<>();
        for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
            long e = f.getEntryPoint().getOffset();
            if (e >= CODE_LO && e < P) { work.add(f); seen.add(e); }
        }
        while (!work.isEmpty()) {
            Function f = work.poll();
            for (Instruction ins : ls.getInstructions(f.getBody(), true)) {
                ArrayList<Address> targets = new ArrayList<>();
                boolean flow = false;
                for (Reference r : ins.getReferencesFrom()) {
                    RefType rt = r.getReferenceType();
                    if (rt.isCall() || rt.isJump()) { targets.add(r.getToAddress()); flow = true; }
                }
                if (!flow) {
                    String m = ins.getMnemonicString();
                    int w; try { w = mem.getShort(ins.getAddress()) & 0xFFFF; } catch (Exception ex) { continue; }
                    try {
                        if (m.equals("bsr") && w != 0xFFFF && w != 0xF800) {
                            int disp = w & 0x7FF; if ((disp & 0x400) != 0) disp -= 0x800;
                            targets.add(ins.getAddress().add(2 + (disp << 1)));
                        } else if (m.equals("jsri") || m.equals("jmpi")) {
                            Address slot = toAddr(((ins.getAddress().getOffset() + 2) & ~3L) + ((w & 0xFF) << 2));
                            if (mem.contains(slot)) targets.add(toAddr((mem.getInt(slot) & 0xFFFFFFFFL) & ~1L));
                        }
                    } catch (Exception ex) {}
                }
                for (Address t : targets) {
                    if (t == null) continue;
                    long o = t.getOffset();
                    if (o < P || o >= HI || getInstructionAt(t) == null) continue;
                    Function g = getFunctionContaining(t);
                    if (g == null || !seen.add(g.getEntryPoint().getOffset())) continue;
                    out.put(g.getEntryPoint().getOffset(), g); work.add(g);
                }
            }
        }
        return out;
    }

    private boolean inProtected(TreeMap<Long,Function> prot, Address a) {
        Map.Entry<Long,Function> e = prot.floorEntry(a.getOffset());
        return e != null && e.getValue().getBody().contains(a);
    }

    // End (max body addr + 1, 2-byte aligned) of the highest function that opens with the
    // M·CORE frame prologue `subi r0,N; stm rX-r15,(r0)` -- i.e. the last real function.
    // Spurious data-region functions don't start with this pair, so this is the code/data
    // boundary. Used by the `auto` mode so no image-specific DATA_LO has to be supplied.
    private long highestPrologueEnd(long lo, long hi) throws Exception {
        Memory mem = currentProgram.getMemory();
        long best = lo;
        for (Function f : currentProgram.getFunctionManager().getFunctions(true)) {
            long e = f.getEntryPoint().getOffset();
            if (e < lo || e >= hi) continue;
            if (!isPrologue(mem, f.getEntryPoint())) continue;
            long end = f.getBody().getMaxAddress().getOffset() + 1;
            if (end > best) best = end;
        }
        return (best + 1) & ~1L;
    }

    // M·CORE frame prologue test: `subi r0,N` (0xFE0F==0x2400) then `stm rX-r15,(r0)` (0xFFF0==0x0070).
    private boolean isPrologue(Memory mem, Address a) {
        try {
            int h0 = mem.getShort(a) & 0xFFFF;
            int h1 = mem.getShort(a.add(2)) & 0xFFFF;
            return (h0 & 0xFE0F) == 0x2400 && (h1 & 0xFFF0) == 0x0070;
        } catch (Exception e) { return false; }
    }
}
