// Decompile functions to C files.
// Args: <out-dir> <selector>...
//   selector: 0x<address> (function containing it), or name:<regex> on the full function name.
// Each function goes to <out-dir>/<entry>_<short name>.c; an index.tsv lists what was written.
// @category MaxRs
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileOptions;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import java.io.File;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.regex.Pattern;

public class Decompile extends GhidraScript {
    private static final int MAX_FUNCTIONS = 400;
    private static final int TIMEOUT_SECONDS = 120;

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            printerr("usage: Decompile <out-dir> <0xaddr|name:regex>...");
            return;
        }
        File outDir = new File(args[0]);
        outDir.mkdirs();

        // "@file" reads one selector per line, so cmd.exe never sees | ^ & < >.
        java.util.List<String> selectors = new java.util.ArrayList<>();
        for (int i = 1; i < args.length; i++) {
            if (args[i].startsWith("@")) {
                for (String line : java.nio.file.Files.readAllLines(java.nio.file.Path.of(args[i].substring(1)), StandardCharsets.UTF_8)) {
                    if (!line.isBlank()) {
                        selectors.add(line.strip());
                    }
                }
            } else {
                selectors.add(args[i]);
            }
        }

        Map<Address, Function> selected = new LinkedHashMap<>();
        for (String sel : selectors) {
            if (sel.startsWith("callers:")) {
                // callers:0x<addr> -> every function that calls the function at addr,
                // following one level of thunks.
                Function target = getFunctionContaining(toAddr(sel.substring(8)));
                if (target == null) {
                    printerr("no function at " + sel);
                    continue;
                }
                // Every reference (call, jump, data pointer) to the target or to any thunk
                // chain that ends in it; the referencing non-thunk functions are selected.
                java.util.Set<Function> targets = new java.util.HashSet<>();
                java.util.ArrayDeque<Function> work = new java.util.ArrayDeque<>();
                work.add(target);
                while (!work.isEmpty()) {
                    Function t = work.poll();
                    if (!targets.add(t)) {
                        continue;
                    }
                    for (ghidra.program.model.symbol.Reference ref : getReferencesTo(t.getEntryPoint())) {
                        Function from = getFunctionContaining(ref.getFromAddress());
                        if (from == null) {
                            println("ref from non-function " + ref.getFromAddress() + " type " + ref.getReferenceType());
                        } else if (from.isThunk()) {
                            work.add(from);
                        } else {
                            selected.put(from.getEntryPoint(), from);
                        }
                    }
                }
            } else if (sel.startsWith("name:")) {
                Pattern p = Pattern.compile(sel.substring(5));
                FunctionIterator it = currentProgram.getFunctionManager().getFunctions(true);
                while (it.hasNext() && selected.size() < MAX_FUNCTIONS) {
                    Function fn = it.next();
                    if (p.matcher(fn.getName(true)).find()) {
                        selected.put(fn.getEntryPoint(), fn);
                    }
                }
            } else {
                Address addr = toAddr(sel);
                Function fn = getFunctionContaining(addr);
                if (fn == null) {
                    printerr("no function at " + sel);
                } else {
                    selected.put(fn.getEntryPoint(), fn);
                }
            }
        }

        DecompInterface decomp = new DecompInterface();
        decomp.setOptions(new DecompileOptions());
        decomp.openProgram(currentProgram);
        try (PrintWriter index = new PrintWriter(new File(outDir, "index.tsv"), StandardCharsets.UTF_8)) {
            for (Function fn : selected.values()) {
                if (monitor.isCancelled()) {
                    break;
                }
                DecompileResults res = decomp.decompileFunction(fn, TIMEOUT_SECONDS, monitor);
                String shortName = fn.getName().replaceAll("[^A-Za-z0-9_]", "_");
                if (shortName.length() > 60) {
                    shortName = shortName.substring(0, 60);
                }
                File file = new File(outDir, fn.getEntryPoint() + "_" + shortName + ".c");
                try (PrintWriter out = new PrintWriter(file, StandardCharsets.UTF_8)) {
                    out.println("// " + fn.getName(true));
                    out.println("// entry " + fn.getEntryPoint() + ", body " + fn.getBody().getNumAddresses() + " bytes");
                    if (res.decompileCompleted()) {
                        out.println(res.getDecompiledFunction().getC());
                    } else {
                        out.println("// decompile failed: " + res.getErrorMessage());
                    }
                }
                index.printf("%s\t%s\t%s%n", fn.getEntryPoint(), file.getName(), fn.getName(true));
            }
        } finally {
            decomp.dispose();
        }
        println("Decompile wrote " + selected.size() + " functions to " + outDir);
    }
}
