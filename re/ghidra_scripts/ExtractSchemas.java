// Dump the serialized layout of every struct the client (de)serializes with
// Serialization::SerializableMember<T, Serialization::Msgpack::SerializedType, string_view, Struct>.
//
// Constructors register each member as: a string literal with the key, then the member's
// SerializableMember<...>::vftable, then a member flag. This script decompiles every function
// that writes such a vftable and reads that sequence back.
//
// Args: <out.tsv>
// Output TSV: struct, index, key, type, flag, function entry.
// @category MaxRs
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileOptions;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.Symbol;
import ghidra.program.model.symbol.SymbolIterator;
import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

public class ExtractSchemas extends GhidraScript {
    private static final String MEMBER = "SerializableMember<";
    private static final Pattern KEY = Pattern.compile("=\\s*\"((?:[^\"\\\\]|\\\\.)*)\";");
    private static final Pattern FLAG = Pattern.compile("\\(undefined4 \\*\\)[^=]*=\\s*(\\d+);");
    // Short keys ("id", "text") are not auto-defined as strings; the decompiler shows &DAT_<addr>.
    private static final Pattern DAT_KEY = Pattern.compile("=\\s*&DAT_([0-9a-fA-F]+);");

    /** A short printable C string at the address, or null. */
    private String cString(String hex) {
        try {
            ghidra.program.model.address.Address a = toAddr(Long.parseUnsignedLong(hex, 16));
            StringBuilder sb = new StringBuilder();
            for (int i = 0; i < 64; i++) {
                byte b = getByte(a.add(i));
                if (b == 0) {
                    return sb.length() > 0 ? sb.toString() : null;
                }
                if (b < 0x20 || b > 0x7e) {
                    return null;
                }
                sb.append((char) b);
            }
        } catch (Exception e) {
            return null;
        }
        return null;
    }

    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 1) {
            printerr("usage: ExtractSchemas <out.tsv>");
            return;
        }
        // 1. Functions that write any SerializableMember vftable.
        Map<String, Function> ctors = new LinkedHashMap<>();
        int vtables = 0;
        SymbolIterator it = currentProgram.getSymbolTable().getAllSymbols(true);
        while (it.hasNext() && !monitor.isCancelled()) {
            Symbol s = it.next();
            if (!s.getName().equals("vftable")) {
                continue;
            }
            String ns = s.getParentNamespace().getName(true);
            if (!ns.contains("Serialization::" + MEMBER)) {
                continue;
            }
            vtables++;
            for (Reference r : getReferencesTo(s.getAddress())) {
                Function f = getFunctionContaining(r.getFromAddress());
                if (f != null) {
                    ctors.put(f.getEntryPoint().toString(), f);
                }
            }
        }
        println("ExtractSchemas vtables=" + vtables + " functions=" + ctors.size());

        DecompInterface decomp = new DecompInterface();
        decomp.setOptions(new DecompileOptions());
        decomp.openProgram(currentProgram);
        int rows = 0;
        try (PrintWriter out = new PrintWriter(args[0], StandardCharsets.UTF_8)) {
            for (Function f : ctors.values()) {
                if (monitor.isCancelled()) {
                    break;
                }
                DecompileResults res = decomp.decompileFunction(f, 60, monitor);
                if (!res.decompileCompleted()) {
                    continue;
                }
                rows += parse(res.getDecompiledFunction().getC(), f, out);
            }
        } finally {
            decomp.dispose();
        }
        println("ExtractSchemas rows=" + rows);
    }

    /** Walk the C text in order: key literal, then member vftable, then flag. */
    private int parse(String c, Function f, PrintWriter out) {
        // Join wrapped lines so each statement is on one line.
        // The decompiler breaks long names after "::"; glue those back without a space.
        String[] stmts = c.replace("\r", "")
            .replaceAll("::\\s*\n\\s*", "::")
            .replaceAll("\n\\s+", " ")
            .split(";");
        String key = null;
        List<String[]> pending = new ArrayList<>();
        Map<String, Integer> index = new LinkedHashMap<>();
        int rows = 0;
        for (String raw : stmts) {
            String st = raw.trim() + ";";
            Matcher km = KEY.matcher(st);
            if (km.find() && !st.contains("vftable")) {
                key = km.group(1);
                continue;
            }
            Matcher dm = DAT_KEY.matcher(st);
            if (dm.find() && !st.contains("vftable")) {
                String s = cString(dm.group(1));
                if (s != null) {
                    key = s;
                }
                continue;
            }
            int at = st.indexOf("Serialization::" + MEMBER);
            if (at >= 0 && st.contains("vftable") && !st.contains("ISerializableMember")) {
                List<String> targs = templateArgs(st, at + "Serialization::".length() + MEMBER.length() - 1);
                if (targs.size() >= 4 && key != null) {
                    String type = clean(targs.get(0));
                    String struct = clean(targs.get(targs.size() - 1));
                    int i = index.merge(struct, 1, Integer::sum) - 1;
                    pending.add(new String[] {struct, Integer.toString(i), key, type, "", f.getEntryPoint().toString()});
                    key = null;
                }
                continue;
            }
            Matcher fm = FLAG.matcher(st);
            if (fm.find() && !pending.isEmpty() && pending.get(pending.size() - 1)[4].isEmpty()) {
                pending.get(pending.size() - 1)[4] = fm.group(1);
            }
        }
        for (String[] row : pending) {
            out.println(String.join("\t", row));
            rows++;
        }
        return rows;
    }

    /** Split the template argument list that starts at the '<' at position `open`. */
    private static List<String> templateArgs(String s, int open) {
        List<String> args = new ArrayList<>();
        int depth = 0;
        StringBuilder cur = new StringBuilder();
        for (int i = open; i < s.length(); i++) {
            char ch = s.charAt(i);
            if (ch == '<') {
                depth++;
                if (depth == 1) {
                    continue;
                }
            } else if (ch == '>') {
                depth--;
                if (depth == 0) {
                    args.add(cur.toString());
                    return args;
                }
            } else if (ch == ',' && depth == 1) {
                args.add(cur.toString());
                cur.setLength(0);
                continue;
            }
            cur.append(ch);
        }
        return args;
    }

    private static String clean(String t) {
        return t.trim()
            .replace("std::basic_string<char,std::char_traits<char>,std::allocator<char>_>", "string")
            .replace("std::basic_string_view<char,std::char_traits<char>_>", "string_view")
            .replaceAll("_>", ">")
            .replace("Api::OneMe::Packets::", "P::")
            .replace("Api::OneMe::Types::", "T::");
    }
}
