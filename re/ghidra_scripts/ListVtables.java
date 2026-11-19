// Count vftable symbols by the outer template name of their namespace, filtered by a regex.
// Args: <regex on namespace> <out.tsv>
// Output TSV: count, template name, one example namespace.
// @category MaxRs
import ghidra.app.script.GhidraScript;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.symbol.SymbolIterator;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.TreeMap;
import java.util.regex.Pattern;

public class ListVtables extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        String regex = args[0].startsWith("@")
            ? java.nio.file.Files.readString(java.nio.file.Path.of(args[0].substring(1)), StandardCharsets.UTF_8).strip()
            : args[0];
        Pattern p = Pattern.compile(regex);
        Map<String, Integer> counts = new TreeMap<>();
        Map<String, String> example = new TreeMap<>();
        SymbolIterator it = currentProgram.getSymbolTable().getAllSymbols(true);
        while (it.hasNext() && !monitor.isCancelled()) {
            Symbol s = it.next();
            if (!s.getName().equals("vftable")) {
                continue;
            }
            String ns = s.getParentNamespace().getName(true);
            if (!p.matcher(ns).find()) {
                continue;
            }
            int lt = ns.indexOf('<');
            String outer = lt > 0 ? ns.substring(0, lt) : ns;
            counts.merge(outer, 1, Integer::sum);
            example.putIfAbsent(outer, ns.length() > 300 ? ns.substring(0, 300) : ns);
        }
        try (PrintWriter out = new PrintWriter(args[1], StandardCharsets.UTF_8)) {
            for (Map.Entry<String, Integer> e : counts.entrySet()) {
                out.printf("%d\t%s\t%s%n", e.getValue(), e.getKey(), example.get(e.getKey()));
            }
        }
        println("ListVtables matches=" + counts.size());
    }
}
