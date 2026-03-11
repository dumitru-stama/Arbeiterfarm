// Ghidra script: decompile specified functions to C pseudocode as JSON.
// Usage: analyzeHeadless ... -postScript DecompileFunctionsJSON.java <func1,func2,0x401000> <output_path>
// Output: {"functions":[{"name":"main","address":"00401000","decompiled":"int main(...) {...}"}, ...]}
//
// NOTE: Uses manual JSON writing to avoid dependency on Gson (not always on classpath).
//@category Claw
import ghidra.app.script.GhidraScript;
import ghidra.app.decompiler.*;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;

import java.io.*;

public class DecompileFunctionsJSON extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            printerr("DecompileFunctionsJSON: expected <func_names_csv> <output_path>");
            return;
        }
        String[] funcNames = args[0].split(",");
        String outputPath = args[1];

        DecompInterface decompiler = new DecompInterface();
        decompiler.openProgram(currentProgram);

        FunctionManager fm = currentProgram.getFunctionManager();

        try (PrintWriter pw = new PrintWriter(new FileWriter(outputPath))) {
            pw.println("{\"functions\":[");
            boolean first = true;

            for (String name : funcNames) {
                String trimmed = name.trim();
                if (trimmed.isEmpty()) continue;

                if (!first) {
                    pw.println(",");
                }
                first = false;

                Function func = findFunction(fm, trimmed);
                if (func == null) {
                    pw.print("  {\"name\":\"" + escapeJson(trimmed) + "\",");
                    pw.print("\"error\":\"function not found\"}");
                    continue;
                }

                DecompileResults dr = decompiler.decompileFunction(func, 60, monitor);
                pw.print("  {\"name\":\"" + escapeJson(func.getName()) + "\",");
                pw.print("\"address\":\"" + func.getEntryPoint().toString() + "\",");

                if (dr != null && dr.getDecompiledFunction() != null) {
                    String code = dr.getDecompiledFunction().getC();
                    pw.print("\"decompiled\":\"" + escapeJson(code) + "\"");
                } else {
                    String errMsg = (dr != null && dr.getErrorMessage() != null)
                        ? dr.getErrorMessage() : "decompilation failed";
                    pw.print("\"error\":\"" + escapeJson(errMsg) + "\"");
                }
                pw.print("}");
            }

            pw.println();
            pw.println("]}");
        }

        decompiler.dispose();
        println("DecompileFunctionsJSON: wrote " + outputPath);
    }

    private Function findFunction(FunctionManager fm, String nameOrAddr) {
        // Try as hex address first (0x...)
        if (nameOrAddr.startsWith("0x") || nameOrAddr.startsWith("0X")) {
            try {
                long addr = Long.parseUnsignedLong(nameOrAddr.substring(2), 16);
                AddressSpace space = currentProgram.getAddressFactory().getDefaultAddressSpace();
                Function f = fm.getFunctionAt(space.getAddress(addr));
                if (f != null) return f;

                // Address might be relative (e.g. from rizin) — try adding image base.
                // PIE ELFs have virtual addresses starting at 0x0, but Ghidra loads at
                // a non-zero image base (typically 0x100000).
                long imageBase = currentProgram.getImageBase().getOffset();
                if (imageBase != 0) {
                    f = fm.getFunctionAt(space.getAddress(imageBase + addr));
                    if (f != null) return f;
                }
            } catch (Exception e) {
                // fall through to name lookup
            }
        }

        // Try as plain hex address without 0x prefix (Ghidra style: "00401000")
        try {
            AddressSpace space = currentProgram.getAddressFactory().getDefaultAddressSpace();
            long addr = Long.parseUnsignedLong(nameOrAddr, 16);
            Function f = fm.getFunctionAt(space.getAddress(addr));
            if (f != null) return f;

            // Also try with image base for Ghidra-style addresses
            long imageBase = currentProgram.getImageBase().getOffset();
            if (imageBase != 0) {
                f = fm.getFunctionAt(space.getAddress(imageBase + addr));
                if (f != null) return f;
            }
        } catch (Exception e) {
            // fall through to name lookup
        }

        // Try as function name
        FunctionIterator iter = fm.getFunctions(true);
        while (iter.hasNext()) {
            Function f = iter.next();
            if (f.getName().equals(nameOrAddr)) return f;
        }
        return null;
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
