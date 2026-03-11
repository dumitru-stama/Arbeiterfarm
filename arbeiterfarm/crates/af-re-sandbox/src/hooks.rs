/// Default Frida hook script for comprehensive Windows API tracing (~60 APIs).
///
/// Each hook logs: timestamp, API name, args (sanitized/truncated), return value,
/// thread ID, and call stack (top 3 frames).
pub const DEFAULT_HOOK_SCRIPT: &str = r#"
'use strict';

var trace = [];
var MAX_TRACE_ENTRIES = 10000;
var MAX_STRING_LEN = 256;

function readUtf16(ptr) {
    try {
        if (ptr.isNull()) return "<null>";
        var s = ptr.readUtf16String();
        return s ? s.substring(0, MAX_STRING_LEN) : "<empty>";
    } catch (e) { return "<unreadable>"; }
}

function readAnsi(ptr) {
    try {
        if (ptr.isNull()) return "<null>";
        var s = ptr.readAnsiString();
        return s ? s.substring(0, MAX_STRING_LEN) : "<empty>";
    } catch (e) { return "<unreadable>"; }
}

function readPtr(ptr) {
    try {
        if (ptr.isNull()) return "0x0";
        return ptr.toString();
    } catch (e) { return "<err>"; }
}

function getBacktrace() {
    try {
        var bt = Thread.backtrace(this.context, Backtracer.ACCURATE).slice(0, 3);
        return bt.map(function(addr) {
            var m = DebugSymbol.fromAddress(addr);
            return m.name ? m.name + "+" + m.offset : addr.toString();
        });
    } catch (e) { return []; }
}

function log(name, args, retval) {
    if (trace.length >= MAX_TRACE_ENTRIES) return;
    trace.push({
        ts: Date.now(),
        api: name,
        args: args,
        ret: retval !== undefined ? retval.toString() : null,
        tid: Process.getCurrentThreadId(),
        bt: getBacktrace()
    });
}

function hookW(mod, name, argParser) {
    try {
        var addr = Module.findExportByName(mod, name);
        if (!addr) return;
        Interceptor.attach(addr, {
            onEnter: function(a) { this._args = argParser(a); },
            onLeave: function(retval) { log(name, this._args, retval); }
        });
    } catch (e) {}
}

// ---- File APIs ----
hookW("kernel32.dll", "CreateFileW", function(a) {
    return { path: readUtf16(a[0]), access: a[1].toInt32(), share: a[2].toInt32() };
});
hookW("kernel32.dll", "CreateFileA", function(a) {
    return { path: readAnsi(a[0]), access: a[1].toInt32(), share: a[2].toInt32() };
});
hookW("kernel32.dll", "ReadFile", function(a) {
    return { handle: readPtr(a[0]), bytes: a[2].toInt32() };
});
hookW("kernel32.dll", "WriteFile", function(a) {
    return { handle: readPtr(a[0]), bytes: a[2].toInt32() };
});
hookW("kernel32.dll", "DeleteFileW", function(a) {
    return { path: readUtf16(a[0]) };
});
hookW("kernel32.dll", "DeleteFileA", function(a) {
    return { path: readAnsi(a[0]) };
});
hookW("kernel32.dll", "CopyFileW", function(a) {
    return { src: readUtf16(a[0]), dst: readUtf16(a[1]) };
});
hookW("kernel32.dll", "CopyFileA", function(a) {
    return { src: readAnsi(a[0]), dst: readAnsi(a[1]) };
});
hookW("kernel32.dll", "MoveFileW", function(a) {
    return { src: readUtf16(a[0]), dst: readUtf16(a[1]) };
});
hookW("kernel32.dll", "MoveFileA", function(a) {
    return { src: readAnsi(a[0]), dst: readAnsi(a[1]) };
});
hookW("kernel32.dll", "FindFirstFileW", function(a) {
    return { pattern: readUtf16(a[0]) };
});

// ---- Registry APIs ----
hookW("advapi32.dll", "RegOpenKeyExW", function(a) {
    return { key: readPtr(a[0]), subkey: readUtf16(a[1]) };
});
hookW("advapi32.dll", "RegOpenKeyExA", function(a) {
    return { key: readPtr(a[0]), subkey: readAnsi(a[1]) };
});
hookW("advapi32.dll", "RegSetValueExW", function(a) {
    return { key: readPtr(a[0]), name: readUtf16(a[1]), type: a[2].toInt32() };
});
hookW("advapi32.dll", "RegSetValueExA", function(a) {
    return { key: readPtr(a[0]), name: readAnsi(a[1]), type: a[2].toInt32() };
});
hookW("advapi32.dll", "RegQueryValueExW", function(a) {
    return { key: readPtr(a[0]), name: readUtf16(a[1]) };
});
hookW("advapi32.dll", "RegQueryValueExA", function(a) {
    return { key: readPtr(a[0]), name: readAnsi(a[1]) };
});
hookW("advapi32.dll", "RegDeleteKeyW", function(a) {
    return { key: readPtr(a[0]), subkey: readUtf16(a[1]) };
});
hookW("advapi32.dll", "RegDeleteKeyA", function(a) {
    return { key: readPtr(a[0]), subkey: readAnsi(a[1]) };
});
hookW("advapi32.dll", "RegCreateKeyExW", function(a) {
    return { key: readPtr(a[0]), subkey: readUtf16(a[1]) };
});
hookW("advapi32.dll", "RegCreateKeyExA", function(a) {
    return { key: readPtr(a[0]), subkey: readAnsi(a[1]) };
});

// ---- Process APIs ----
hookW("kernel32.dll", "CreateProcessW", function(a) {
    return { app: readUtf16(a[0]), cmdline: readUtf16(a[1]) };
});
hookW("kernel32.dll", "CreateProcessA", function(a) {
    return { app: readAnsi(a[0]), cmdline: readAnsi(a[1]) };
});
hookW("kernel32.dll", "OpenProcess", function(a) {
    return { access: a[0].toInt32(), pid: a[2].toInt32() };
});
hookW("kernel32.dll", "TerminateProcess", function(a) {
    return { handle: readPtr(a[0]), exitcode: a[1].toInt32() };
});
hookW("kernel32.dll", "VirtualAllocEx", function(a) {
    return { process: readPtr(a[0]), size: a[2].toInt32(), type: a[3].toInt32() };
});
hookW("kernel32.dll", "WriteProcessMemory", function(a) {
    return { process: readPtr(a[0]), addr: readPtr(a[1]), size: a[3].toInt32() };
});
hookW("kernel32.dll", "CreateRemoteThread", function(a) {
    return { process: readPtr(a[0]), start: readPtr(a[3]) };
});
hookW("ntdll.dll", "NtCreateThreadEx", function(a) {
    return { process: readPtr(a[3]), start: readPtr(a[4]) };
});

// ---- Network APIs ----
hookW("ws2_32.dll", "connect", function(a) {
    return { socket: readPtr(a[0]) };
});
hookW("ws2_32.dll", "send", function(a) {
    return { socket: readPtr(a[0]), len: a[2].toInt32() };
});
hookW("ws2_32.dll", "recv", function(a) {
    return { socket: readPtr(a[0]), len: a[2].toInt32() };
});
hookW("wininet.dll", "InternetOpenW", function(a) {
    return { agent: readUtf16(a[0]) };
});
hookW("wininet.dll", "InternetOpenA", function(a) {
    return { agent: readAnsi(a[0]) };
});
hookW("wininet.dll", "InternetConnectW", function(a) {
    return { server: readUtf16(a[1]), port: a[2].toInt32() };
});
hookW("wininet.dll", "InternetConnectA", function(a) {
    return { server: readAnsi(a[1]), port: a[2].toInt32() };
});
hookW("wininet.dll", "HttpOpenRequestW", function(a) {
    return { verb: readUtf16(a[1]), path: readUtf16(a[2]) };
});
hookW("wininet.dll", "HttpOpenRequestA", function(a) {
    return { verb: readAnsi(a[1]), path: readAnsi(a[2]) };
});
hookW("wininet.dll", "HttpSendRequestW", function(a) {
    return { handle: readPtr(a[0]) };
});
hookW("wininet.dll", "HttpSendRequestA", function(a) {
    return { handle: readPtr(a[0]) };
});
hookW("urlmon.dll", "URLDownloadToFileW", function(a) {
    return { url: readUtf16(a[1]), path: readUtf16(a[2]) };
});
hookW("urlmon.dll", "URLDownloadToFileA", function(a) {
    return { url: readAnsi(a[1]), path: readAnsi(a[2]) };
});
hookW("ws2_32.dll", "WSAStartup", function(a) {
    return { version: a[0].toInt32() };
});
hookW("ws2_32.dll", "getaddrinfo", function(a) {
    return { node: readAnsi(a[0]), service: readAnsi(a[1]) };
});
hookW("dnsapi.dll", "DnsQuery_W", function(a) {
    return { name: readUtf16(a[0]), type: a[1].toInt32() };
});

// ---- Library APIs ----
hookW("kernel32.dll", "LoadLibraryW", function(a) {
    return { name: readUtf16(a[0]) };
});
hookW("kernel32.dll", "LoadLibraryA", function(a) {
    return { name: readAnsi(a[0]) };
});
hookW("kernel32.dll", "LoadLibraryExW", function(a) {
    return { name: readUtf16(a[0]), flags: a[2].toInt32() };
});
hookW("kernel32.dll", "LoadLibraryExA", function(a) {
    return { name: readAnsi(a[0]), flags: a[2].toInt32() };
});
hookW("kernel32.dll", "GetProcAddress", function(a) {
    return { module: readPtr(a[0]), name: readAnsi(a[1]) };
});
hookW("ntdll.dll", "LdrLoadDll", function(a) {
    return { flags: a[1].toInt32() };
});

// ---- Crypto APIs ----
hookW("advapi32.dll", "CryptEncrypt", function(a) {
    return { key: readPtr(a[0]), len: a[4].toInt32() };
});
hookW("advapi32.dll", "CryptDecrypt", function(a) {
    return { key: readPtr(a[0]) };
});
hookW("advapi32.dll", "CryptHashData", function(a) {
    return { hash: readPtr(a[0]), len: a[2].toInt32() };
});
hookW("bcrypt.dll", "BCryptEncrypt", function(a) {
    return { key: readPtr(a[0]), inputLen: a[2].toInt32() };
});
hookW("bcrypt.dll", "BCryptDecrypt", function(a) {
    return { key: readPtr(a[0]), inputLen: a[2].toInt32() };
});

// ---- Service APIs ----
hookW("advapi32.dll", "CreateServiceW", function(a) {
    return { manager: readPtr(a[0]), name: readUtf16(a[1]), display: readUtf16(a[2]) };
});
hookW("advapi32.dll", "CreateServiceA", function(a) {
    return { manager: readPtr(a[0]), name: readAnsi(a[1]), display: readAnsi(a[2]) };
});
hookW("advapi32.dll", "StartServiceW", function(a) {
    return { service: readPtr(a[0]) };
});
hookW("advapi32.dll", "OpenServiceW", function(a) {
    return { manager: readPtr(a[0]), name: readUtf16(a[1]) };
});
hookW("advapi32.dll", "ChangeServiceConfigW", function(a) {
    return { service: readPtr(a[0]) };
});

// ---- COM APIs ----
hookW("ole32.dll", "CoCreateInstance", function(a) {
    return { clsid: readPtr(a[0]) };
});
hookW("ole32.dll", "CoGetClassObject", function(a) {
    return { clsid: readPtr(a[0]) };
});

// ---- Memory APIs ----
hookW("kernel32.dll", "VirtualAlloc", function(a) {
    return { addr: readPtr(a[0]), size: a[1].toInt32(), type: a[2].toInt32(), protect: a[3].toInt32() };
});
hookW("kernel32.dll", "VirtualProtect", function(a) {
    return { addr: readPtr(a[0]), size: a[1].toInt32(), protect: a[2].toInt32() };
});
hookW("kernel32.dll", "HeapCreate", function(a) {
    return { options: a[0].toInt32(), initial: a[1].toInt32() };
});
hookW("ntdll.dll", "NtAllocateVirtualMemory", function(a) {
    return { process: readPtr(a[0]), size: a[3].toInt32() };
});

// ---- Anti-debug APIs ----
hookW("kernel32.dll", "IsDebuggerPresent", function(a) {
    return {};
});
hookW("kernel32.dll", "CheckRemoteDebuggerPresent", function(a) {
    return { process: readPtr(a[0]) };
});
hookW("ntdll.dll", "NtQueryInformationProcess", function(a) {
    return { process: readPtr(a[0]), class: a[1].toInt32() };
});
hookW("kernel32.dll", "GetTickCount", function(a) {
    return {};
});
hookW("kernel32.dll", "QueryPerformanceCounter", function(a) {
    return {};
});

// ---- Send results when script is unloaded ----
rpc.exports = {
    getTrace: function() { return trace; }
};
"#;
