// For each address: the nearest symbol at or before it (e.g. a vftable), and the functions
// that reference that symbol.
// Args: <0xaddr>...
// @category MaxRs
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.symbol.SymbolIterator;

public class SymbolAt extends GhidraScript {
    @Override
    protected void run() throws Exception {
        for (String arg : getScriptArgs()) {
            Address addr = toAddr(arg);
            SymbolIterator it = currentProgram.getSymbolTable().getSymbolIterator(addr, false);
            Symbol sym = it.hasNext() ? it.next() : null;
            if (sym == null) {
                println("MaxRs " + arg + " no symbol");
                continue;
            }
            println("MaxRs " + arg + " -> " + sym.getName(true) + " @ " + sym.getAddress()
                + " (+" + addr.subtract(sym.getAddress()) + ")");
            for (Reference ref : getReferencesTo(sym.getAddress())) {
                Function fn = getFunctionContaining(ref.getFromAddress());
                println("MaxRs   ref " + ref.getReferenceType() + " from " + ref.getFromAddress()
                    + " in " + (fn == null ? "-" : fn.getEntryPoint() + " " + fn.getName(true)));
            }
        }
    }
}
