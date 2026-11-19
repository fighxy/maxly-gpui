// Find defined strings matching a regex and list the functions that reference them.
// Args: <regex> <out.tsv>
// Output TSV: string address, function entry, function name, string value.
// @category MaxRs
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Data;
import ghidra.program.model.listing.DataIterator;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.util.regex.Pattern;

public class FindStringRefs extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            printerr("usage: FindStringRefs <regex> <out.tsv>");
            return;
        }
        // "@file" reads the regex from a file, so cmd.exe never sees | ^ & < >.
        String regex = args[0].startsWith("@")
            ? java.nio.file.Files.readString(java.nio.file.Path.of(args[0].substring(1)), StandardCharsets.UTF_8).strip()
            : args[0];
        Pattern pattern = Pattern.compile(regex);
        int matches = 0;
        try (PrintWriter out = new PrintWriter(args[1], StandardCharsets.UTF_8)) {
            DataIterator it = currentProgram.getListing().getDefinedData(true);
            while (it.hasNext() && !monitor.isCancelled()) {
                Data data = it.next();
                if (!data.hasStringValue()) {
                    continue;
                }
                Object value = data.getValue();
                if (value == null) {
                    continue;
                }
                String text = value.toString();
                if (!pattern.matcher(text).find()) {
                    continue;
                }
                matches++;
                String clean = text.replace('\t', ' ').replace('\n', ' ').replace('\r', ' ');
                Reference[] refs = getReferencesTo(data.getAddress());
                if (refs.length == 0) {
                    out.printf("%s\t-\t-\t%s%n", data.getAddress(), clean);
                }
                for (Reference ref : refs) {
                    Address from = ref.getFromAddress();
                    Function fn = getFunctionContaining(from);
                    out.printf("%s\t%s\t%s\t%s%n", data.getAddress(),
                        fn == null ? from.toString() : fn.getEntryPoint().toString(),
                        fn == null ? "-" : fn.getName(true), clean);
                }
            }
        }
        println("FindStringRefs matches=" + matches);
    }
}
