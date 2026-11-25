// Read-only: functions in a range, code refs into it, and callers of given addresses.
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class GapRange extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Address lo = toAddr(Long.parseLong(a[0], 16)), hi = toAddr(Long.parseLong(a[1], 16));
        FunctionManager fm = currentProgram.getFunctionManager();
        Listing L = currentProgram.getListing();
        ReferenceManager rm = currentProgram.getReferenceManager();
        println("RANGE " + lo + ".." + hi + " in " + currentProgram.getName());
        for (Function f : fm.getFunctions(new AddressSet(lo, hi), true))
            println("  FUNC " + f.getEntryPoint() + " " + f.getName() + " size=" + f.getBody().getNumAddresses());
        // pointer-sized pool words anywhere in code that point into the range
        AddressIterator ai = rm.getReferenceDestinationIterator(new AddressSet(lo, hi), true);
        while (ai.hasNext()) {
            Address t = ai.next();
            Function tf = fm.getFunctionContaining(t);
            CodeUnit cu = L.getCodeUnitAt(t);
            StringBuilder sb = new StringBuilder();
            int n = 0;
            for (Reference r : rm.getReferencesTo(t)) {
                Function ff = fm.getFunctionContaining(r.getFromAddress());
                sb.append(" ").append(r.getFromAddress()).append("/").append(r.getReferenceType())
                  .append("@").append(ff == null ? "-" : ff.getName());
                if (++n > 4) { sb.append(" ..."); break; }
            }
            println(String.format("  REF->%s infn=%s cu=%s :%s", t, tf == null ? "-" : tf.getName(),
                cu == null ? "-" : cu.toString(), sb));
        }
        for (int i = 2; i < a.length; i++) {
            Address t = toAddr(Long.parseLong(a[i], 16));
            for (Reference r : rm.getReferencesTo(t)) {
                Function ff = fm.getFunctionContaining(r.getFromAddress());
                CodeUnit cu = L.getCodeUnitAt(r.getFromAddress());
                println("  CALLER of " + t + ": " + r.getFromAddress() + " " + r.getReferenceType() + " fn="
                    + (ff == null ? "-" : ff.getName()) + " cu=" + (cu == null ? "-" : cu.toString()));
            }
        }
    }
}
