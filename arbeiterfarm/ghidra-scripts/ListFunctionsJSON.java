// Ghidra script: export program metadata and function list as JSON.
// Usage: analyzeHeadless ... -postScript ListFunctionsJSON.java <output_path>
// Output: {"program_info":{...}, "functions":[{...}, ...]}
//
// NOTE: Uses manual JSON writing to avoid dependency on Gson (not always on classpath).
//@category Arbeiterfarm
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.mem.*;
import ghidra.program.model.symbol.*;

import java.io.*;

public class ListFunctionsJSON extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 1) {
            printerr("ListFunctionsJSON: missing output path argument");
            return;
        }
        String outputPath = args[0];

        try (PrintWriter pw = new PrintWriter(new FileWriter(outputPath))) {
            pw.println("{");

            // --- program_info ---
            pw.println("\"program_info\": {");

            String progName = escapeJson(currentProgram.getName());
            pw.print("  \"name\": \"" + progName + "\",");

            // Entry points
            pw.print("  \"entry_points\": [");
            AddressIterator entryIter = currentProgram.getSymbolTable().getExternalEntryPointIterator();
            boolean firstEntry = true;
            while (entryIter.hasNext()) {
                Address addr = entryIter.next();
                if (!firstEntry) pw.print(", ");
                firstEntry = false;
                pw.print("\"" + addr.toString() + "\"");
            }
            pw.println("],");

            // Image base
            pw.println("  \"image_base\": \"" + currentProgram.getImageBase().toString() + "\",");

            // Format, architecture, pointer size
            pw.println("  \"format\": \"" + escapeJson(currentProgram.getExecutableFormat()) + "\",");
            pw.println("  \"architecture\": \"" + escapeJson(currentProgram.getLanguageID().toString()) + "\",");
            pw.println("  \"pointer_size\": " + currentProgram.getDefaultPointerSize() + ",");

            // Compiler
            String compiler = currentProgram.getCompiler();
            if (compiler != null && !compiler.isEmpty()) {
                pw.println("  \"compiler\": \"" + escapeJson(compiler) + "\",");
            }

            // Memory sections
            pw.println("  \"sections\": [");
            MemoryBlock[] blocks = currentProgram.getMemory().getBlocks();
            for (int i = 0; i < blocks.length; i++) {
                MemoryBlock b = blocks[i];
                if (i > 0) pw.println(",");
                pw.print("    {\"name\": \"" + escapeJson(b.getName()) + "\", ");
                pw.print("\"start\": \"" + b.getStart().toString() + "\", ");
                pw.print("\"size\": " + b.getSize() + ", ");
                pw.print("\"permissions\": \"");
                pw.print(b.isRead() ? "r" : "-");
                pw.print(b.isWrite() ? "w" : "-");
                pw.print(b.isExecute() ? "x" : "-");
                pw.print("\"}");
            }
            pw.println();
            pw.println("  ]");

            pw.println("},");

            // --- functions ---
            pw.println("\"functions\": [");
            FunctionManager fm = currentProgram.getFunctionManager();
            boolean first = true;
            FunctionIterator iter = fm.getFunctions(true);
            while (iter.hasNext()) {
                Function f = iter.next();
                if (!first) {
                    pw.println(",");
                }
                first = false;

                String name = escapeJson(f.getName());
                String address = f.getEntryPoint().toString();
                long size = f.getBody().getNumAddresses();
                boolean isThunk = f.isThunk();
                String cc = escapeJson(
                    f.getCallingConventionName() != null ? f.getCallingConventionName() : "unknown"
                );

                pw.print("  {");
                pw.print("\"name\":\"" + name + "\",");
                pw.print("\"address\":\"" + address + "\",");
                pw.print("\"size\":" + size + ",");
                pw.print("\"is_thunk\":" + isThunk + ",");
                pw.print("\"calling_convention\":\"" + cc + "\"");
                pw.print("}");
            }
            pw.println();
            pw.println("]");

            pw.println("}");
        }

        println("ListFunctionsJSON: wrote " + outputPath);
    }

    private String escapeJson(String s) {
        if (s == null) return "";
        StringBuilder sb = new StringBuilder(s.length());
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '\\': sb.append("\\\\"); break;
                case '"':  sb.append("\\\""); break;
                case '\n': sb.append("\\n"); break;
                case '\r': sb.append("\\r"); break;
                case '\t': sb.append("\\t"); break;
                case '\b': sb.append("\\b"); break;
                case '\f': sb.append("\\f"); break;
                default:
                    if (c < 0x20) {
                        sb.append(String.format("\\u%04x", (int) c));
                    } else {
                        sb.append(c);
                    }
                    break;
            }
        }
        return sb.toString();
    }
}
