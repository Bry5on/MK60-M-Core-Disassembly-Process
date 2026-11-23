// Read-only health check: call targets without functions, and probe addresses.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
import java.util.*;

public class GapCheck extends GhidraScript {
    @Override
    public void run() throws Exception {
        Listing L = currentProgram.getListing();
        FunctionManager fm = currentProgram.getFunctionManager();
        ReferenceManager rm = currentProgram.getReferenceManager();
        println("PROGRAM " + currentProgram.getName() + " lang=" + currentProgram.getLanguageID()
            + " functions=" + fm.getFunctionCount());
        // 1) call / jump-indirect targets (from instructions) that are not function entries
        TreeMap<Address, Integer> missing = new TreeMap<>();
        TreeMap<Address, Integer> notCode = new TreeMap<>();
        InstructionIterator it = L.getInstructions(true);
        int calls = 0;
        while (it.hasNext()) {
            Instruction ins = it.next();
            String m = ins.getMnemonicString().toLowerCase();
            boolean isCall = m.startsWith("jsri") || m.startsWith("bsr") || m.startsWith("jsr");
            for (Reference r : ins.getReferencesFrom()) {
                if (!(r.getReferenceType().isCall() || (isCall && r.getReferenceType().isData()))) continue;
                Address t = r.getToAddress();
                if (!t.isMemoryAddress()) continue;
                // jsri: data ref points to pool slot; resolve pointer
                if (r.getReferenceType().isData()) {
                    try { long v = getInt(t) & 0xffffffffL; t = toAddr(v); } catch (Exception e) { continue; }
                    if (currentProgram.getMemory().getBlock(t) == null) continue;
                }
                calls++;
                if (fm.getFunctionAt(t) == null) {
                    missing.merge(t, 1, Integer::sum);
                    if (L.getInstructionAt(t) == null) notCode.merge(t, 1, Integer::sum);
                }
            }
        }
        println("call refs=" + calls + " targets_without_function=" + missing.size()
            + " of_which_not_disassembled=" + notCode.size());
        int n = 0;
        for (Map.Entry<Address, Integer> e : missing.entrySet()) {
            Address a = e.getKey();
            Symbol s = currentProgram.getSymbolTable().getPrimarySymbol(a);
            CodeUnit cu = L.getCodeUnitContaining(a);
            println(String.format("  MISSING %s x%d sym=%s cu=%s", a, e.getValue(),
                s == null ? "-" : s.getName(), cu == null ? "-" : cu.toString()));
            if (++n >= 40) { println("  ..."); break; }
        }
        // 2) probe addresses passed as script args
        for (String arg : getScriptArgs()) {
            Address a = toAddr(Long.parseLong(arg, 16));
            Function f = fm.getFunctionContaining(a);
            CodeUnit cu = L.getCodeUnitAt(a);
            Symbol s = currentProgram.getSymbolTable().getPrimarySymbol(a);
            int refs = rm.getReferenceCountTo(a);
            println(String.format("  PROBE %s fn=%s cu=%s sym=%s refsTo=%d", a,
                f == null ? "-" : f.getName() + "@" + f.getEntryPoint(),
                cu == null ? "-" : cu.toString(), s == null ? "-" : s.getName(), refs));
        }
    }
}
