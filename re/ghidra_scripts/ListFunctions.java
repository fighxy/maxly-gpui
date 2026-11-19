// List functions whose full name matches a regex, with size and caller count.
// Args: <regex | @file> <out.tsv>
// Output TSV: entry, body size, callers, full name.
// @category MaxRs
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionIterator;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.util.regex.Pattern;

public class ListFunctions extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            printerr("usage: ListFunctions <regex|@file> <out.tsv>");
            return;
        }
        String regex = args[0].startsWith("@")
            ? java.nio.file.Files.readString(java.nio.file.Path.of(args[0].substring(1)), StandardCharsets.UTF_8).strip()
            : args[0];
        Pattern pattern = Pattern.compile(regex);
        int matches = 0;
        try (PrintWriter out = new PrintWriter(args[1], StandardCharsets.UTF_8)) {
            FunctionIterator it = currentProgram.getFunctionManager().getFunctions(true);
            while (it.hasNext() && !monitor.isCancelled()) {
                Function fn = it.next();
                String name = fn.getName(true);
                if (!pattern.matcher(name).find()) {
                    continue;
                }
                matches++;
                out.printf("%s\t%d\t%d\t%s%n", fn.getEntryPoint(), fn.getBody().getNumAddresses(),
                    fn.getCallingFunctions(monitor).size(), name);
            }
        }
        println("ListFunctions matches=" + matches);
    }
}
