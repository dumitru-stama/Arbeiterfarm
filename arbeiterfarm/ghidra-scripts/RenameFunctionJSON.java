// Ghidra script: rename functions in a Ghidra project.
// Usage: analyzeHeadless ... -postScript RenameFunctionJSON.java <old1=new1,old2=new2> <output_path>
// Output: {"renamed":[{"old":"FUN_00401000","new":"parse_header","address":"00401000"}],
//          "errors":[{"old":"missing_func","error":"function not found"}]}
//
// NOTE: Uses manual JSON writing to avoid dependency on Gson (not always on classpath).
//@category Claw
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.address.*;
import ghidra.program.model.symbol.SourceType;

import java.io.*;

public class RenameFunctionJSON extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            printerr("RenameFunctionJSON: expected <old1=new1,old2=new2,...> <output_path>");
            return;
        }
        String[] pairs = args[0].split(",");
        String outputPath = args[1];

        FunctionManager fm = currentProgram.getFunctionManager();
        int txId = currentProgram.startTransaction("claw-rename");
        boolean success = false;

        try (PrintWriter pw = new PrintWriter(new FileWriter(outputPath))) {
            pw.println("{\"renamed\":[");
            boolean firstRenamed = true;
            boolean firstError = true;
            StringBuilder errors = new StringBuilder();

            for (String pair : pairs) {
                String trimmed = pair.trim();
                if (trimmed.isEmpty()) continue;

                int eq = trimmed.indexOf('=');
                if (eq < 0) {
                    if (!firstError) errors.append(",\n");
                    firstError = false;
                    errors.append("  {\"old\":\"" + escapeJson(trimmed) + "\",");
                    errors.append("\"error\":\"invalid format, expected old=new\"}");
                    continue;
                }

                String oldName = trimmed.substring(0, eq).trim();
                String newName = trimmed.substring(eq + 1).trim();

                if (oldName.isEmpty() || newName.isEmpty()) {
                    if (!firstError) errors.append(",\n");
                    firstError = false;
                    errors.append("  {\"old\":\"" + escapeJson(oldName) + "\",");
                    errors.append("\"error\":\"empty old or new name\"}");
                    continue;
                }

                Function func = findFunction(fm, oldName);
                if (func == null) {
                    if (!firstError) errors.append(",\n");
                    firstError = false;
                    errors.append("  {\"old\":\"" + escapeJson(oldName) + "\",");
                    errors.append("\"error\":\"function not found\"}");
                    continue;
                }

                try {
                    String actualOld = func.getName();
                    String address = func.getEntryPoint().toString();
                    func.setName(newName, SourceType.USER_DEFINED);

                    if (!firstRenamed) pw.println(",");
                    firstRenamed = false;
                    pw.print("  {\"old\":\"" + escapeJson(actualOld) + "\",");
                    pw.print("\"new\":\"" + escapeJson(newName) + "\",");
                    pw.print("\"address\":\"" + escapeJson(address) + "\"}");
                } catch (Exception e) {
                    if (!firstError) errors.append(",\n");
                    firstError = false;
                    errors.append("  {\"old\":\"" + escapeJson(oldName) + "\",");
                    errors.append("\"error\":\"" + escapeJson(e.getMessage()) + "\"}");
                }
            }

            pw.println();
            pw.println("],\"errors\":[");
            pw.print(errors.toString());
            pw.println();
            pw.println("]}");

            success = true;
        }

        currentProgram.endTransaction(txId, success);

        if (success) {
            currentProgram.save("claw-rename", monitor);
            println("RenameFunctionJSON: saved project after renaming, wrote " + outputPath);
        }
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
