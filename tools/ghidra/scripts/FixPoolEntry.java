// Move a function entry that was placed inside the preceding literal pool.
// args: wrongEntry realEntry [commit]
//  - removes the function at wrongEntry
//  - retypes [wrongEntry, realEntry) as 32-bit pointer pool words
//  - creates the function at realEntry, prints before/after and a decompile.
import ghidra.app.script.GhidraScript;
import ghidra.app.cmd.function.CreateFunctionCmd;
import ghidra.app.decompiler.*;
import ghidra.program.model.address.*;
import ghidra.program.model.data.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;

public class FixPoolEntry extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] a = getScriptArgs();
        Address wrong = toAddr(Long.parseLong(a[0], 16));
        Address real = toAddr(Long.parseLong(a[1], 16));
        boolean commit = a.length > 2 && a[2].equals("commit");
        FunctionManager fm = currentProgram.getFunctionManager();
        Listing L = currentProgram.getListing();

        Function f = fm.getFunctionAt(wrong);
        if (f == null) { println("ABORT: no function at " + wrong); return; }
        if (f.getSymbol().getSource() == SourceType.USER_DEFINED) { println("ABORT: user-named " + f.getName()); return; }
        if (!f.getBody().contains(real)) { println("ABORT: " + real + " not inside " + f.getName()); return; }
        if (L.getInstructionAt(real) == null || !L.getInstructionAt(real).getMnemonicString().toLowerCase().startsWith("subi")) {
            println("ABORT: no subi prologue at " + real); return;
        }
        long poolLen = real.subtract(wrong);
        if (poolLen <= 0 || poolLen % 4 != 0 || poolLen > 64) { println("ABORT: odd pool length " + poolLen); return; }
        println("BEFORE " + f.getName() + " @" + f.getEntryPoint() + " body=" + f.getBody());
        for (long o = 0; o < poolLen; o += 4) {
            Address p = wrong.add(o);
            println(String.format("  pool %s = 0x%08x refsTo=%d", p, getInt(p) & 0xffffffffL,
                currentProgram.getReferenceManager().getReferenceCountTo(p)));
        }
        if (!commit) { println("DRY RUN, nothing changed"); return; }

        fm.removeFunction(wrong);
        clearListing(wrong, real.subtract(1));
        for (long o = 0; o < poolLen; o += 4) {
            long v = getInt(wrong.add(o)) & 0xffffffffL;
            boolean ptr = currentProgram.getMemory().getBlock(toAddr(v)) != null && v >= 0x400000L;
            createData(wrong.add(o), ptr ? PointerDataType.dataType : DWordDataType.dataType);
        }
        // re-disassemble from the real entry in case clearing touched anything, then create function
        disassemble(real);
        CreateFunctionCmd cmd = new CreateFunctionCmd(real);
        if (!cmd.applyTo(currentProgram, monitor)) { println("ERROR create: " + cmd.getStatusMsg()); return; }
        Function nf = fm.getFunctionAt(real);
        nf.setComment("Entry corrected from " + wrong + " (was inside the literal pool of the preceding function). Called via jsri from kwp_09_proprietary.");
        println("AFTER " + nf.getName() + " @" + nf.getEntryPoint() + " body=" + nf.getBody());
        for (long o = 0; o < poolLen; o += 4) println("  data " + L.getCodeUnitAt(wrong.add(o)));

        DecompInterface di = new DecompInterface();
        di.openProgram(currentProgram);
        DecompileResults r = di.decompileFunction(nf, 60, monitor);
        println("DECOMPILE:\n" + (r.decompileCompleted() ? r.getDecompiledFunction().getC() : r.getErrorMessage()));
        di.dispose();
    }
}
