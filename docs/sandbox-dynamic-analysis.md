# Sandbox Dynamic Analysis — Setup and Usage Guide

Arbeiterfarm's sandbox system executes malware samples inside an isolated Windows QEMU/KVM virtual machine, instruments them with [Frida](https://frida.re/), and collects detailed API call traces. The VM is snapshot-restored between runs to guarantee a clean environment every time.

## Architecture Overview

```
┌──────────────────────────────────────────────────────┐
│  Arbeiterfarm Agent Runtime                                   │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────────┐  │
│  │sandbox.trace │  │sandbox.hook │  │sandbox.      │  │
│  │  (executor)  │  │ (executor)  │  │screenshot    │  │
│  └──────┬───────┘  └──────┬──────┘  └──────┬───────┘  │
│         └─────────────────┼────────────────┘           │
│                           │ JSON-line over UDS         │
├───────────────────────────┼────────────────────────────┤
│  Sandbox Gateway (Rust)   │                             │
│  ┌────────────────────────▼──────────────────────┐     │
│  │  gateway.rs — UDS listener, VM orchestrator   │     │
│  │  ├── QMP client → savevm / loadvm / screendump│     │
│  │  ├── Agent client → TCP to guest :9111        │     │
│  │  └── Mutex-serialized: 1 trace at a time      │     │
│  └───────────────────────────────────────────────┘     │
├────────────────────────────────────────────────────────┤
│  QEMU/KVM Virtual Machine (Windows 10/11)              │
│  ┌───────────────────────────────────────────────┐     │
│  │  agent.py — Python TCP server on :9111        │     │
│  │  ├── Frida: spawn + attach + inject hooks     │     │
│  │  └── Returns JSON trace to gateway            │     │
│  └───────────────────────────────────────────────┘     │
└────────────────────────────────────────────────────────┘
```

**Data flow for `sandbox.trace`:**

1. Agent runtime resolves the artifact (PE binary) and calls the sandbox.trace executor
2. Executor base64-encodes the binary, sends `{"action":"trace",...}` to the gateway over UDS
3. Gateway acquires the VM lock, restores snapshot via QMP `loadvm`, waits 2s for the guest to boot
4. Gateway forwards the command to the Python agent inside the VM over TCP
5. Python agent writes the binary to disk, spawns it with Frida, injects hook script
6. After the timeout, agent collects the API trace via Frida RPC and returns it
7. Gateway passes the trace back to the executor
8. Executor stores the full trace as a `trace.json` artifact, returns a compact summary inline

---

## Prerequisites

### Host Machine

| Requirement | Notes |
|---|---|
| **QEMU/KVM** | `apt install qemu-kvm` (or equivalent). Hardware virtualization must be enabled in BIOS |
| **A Windows VM** | Windows 10 or 11 guest, installed in QEMU with a qcow2 disk image |
| **QMP socket** | QEMU must be started with `-qmp unix:/path/to/qmp.sock,server,nowait` |
| **Network bridge** | Guest must be reachable from the host (e.g., via `virbr0` at `192.168.122.x`) |
| **Arbeiterfarm** | Built with `cargo build --release` from the workspace root |

### Inside the Windows VM

| Requirement | Notes |
|---|---|
| **Python 3.10+** | Download from python.org, add to PATH during install |
| **Frida** | `pip install frida==16.5.2 frida-tools==13.6.0` |
| **Guest agent** | Copy `arbeiterfarm/sandbox-agent/` into the VM |
| **Firewall** | Allow inbound TCP on port 9111 (or disable Windows Firewall for the VM network) |
| **Agent autostart** | Set up agent.py to run at Windows startup (see below) |

---

## Step-by-Step Setup

### 1. Create the Windows VM

```bash
# Create a 40GB qcow2 disk
qemu-img create -f qcow2 /var/lib/af/windows.qcow2 40G

# Install Windows (attach ISO)
qemu-system-x86_64 \
  -enable-kvm \
  -m 4096 \
  -cpu host \
  -smp 2 \
  -drive file=/var/lib/af/windows.qcow2,format=qcow2 \
  -cdrom /path/to/windows10.iso \
  -boot d \
  -net nic,model=virtio \
  -net user
```

Complete the Windows installation normally.

### 2. Configure VM Networking

For the gateway to reach the guest agent, use bridged networking:

```bash
qemu-system-x86_64 \
  -enable-kvm \
  -m 4096 \
  -cpu host \
  -smp 2 \
  -drive file=/var/lib/af/windows.qcow2,format=qcow2 \
  -net nic,model=virtio \
  -net bridge,br=virbr0 \
  -qmp unix:/run/af/qmp.sock,server,nowait \
  -vnc :1
```

Inside Windows, verify the VM gets an IP on the `192.168.122.x` subnet. The default expected address is `192.168.122.10` — configure a static IP or adjust `AF_SANDBOX_AGENT`.

### 3. Install the Guest Agent

Copy the `arbeiterfarm/sandbox-agent/` directory into the VM (via shared folder, `scp`, or manual transfer). Inside the VM:

```powershell
# Install Python dependencies
cd C:\sandbox-agent
pip install -r requirements.txt

# Test the agent
python agent.py
# Should print: [sandbox-agent] starting on 0.0.0.0:9111
```

From the host, verify connectivity:

```bash
echo '{"cmd":"trace","sample_b64":""}' | nc 192.168.122.10 9111
# Should return: {"status": "error", "errors": ["missing sample_b64"]}
```

### 4. Set Up Agent Autostart

Create a Windows batch file (`C:\sandbox-agent\start_agent.bat`):

```batch
@echo off
cd C:\sandbox-agent
python agent.py
```

Place a shortcut to this batch file in:
```
C:\Users\<username>\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup
```

Alternatively, use Task Scheduler to run `python C:\sandbox-agent\agent.py` at logon.

### 5. Create the Clean Snapshot

With the agent running and Windows fully booted:

```bash
# Connect to QMP and save snapshot
socat - UNIX-CONNECT:/run/af/qmp.sock <<'EOF'
{"execute":"qmp_capabilities"}
{"execute":"human-monitor-command","arguments":{"command-line":"savevm clean"}}
EOF
```

Verify the snapshot was created:

```bash
socat - UNIX-CONNECT:/run/af/qmp.sock <<'EOF'
{"execute":"qmp_capabilities"}
{"execute":"human-monitor-command","arguments":{"command-line":"info snapshots"}}
EOF
```

The snapshot is stored inside the qcow2 image. Each `loadvm clean` restores the VM to exactly this point — agent running, Windows booted, clean filesystem.

### 6. Configure Arbeiterfarm Environment

```bash
# Required: UDS path for the sandbox gateway
export AF_SANDBOX_SOCKET=/run/af/sandbox_gateway.sock

# Required: QMP socket path
export AF_SANDBOX_QMP=/run/af/qmp.sock

# Optional: Guest agent address (default: 192.168.122.10:9111)
export AF_SANDBOX_AGENT=192.168.122.10:9111

# Optional: Snapshot name (default: clean)
export AF_SANDBOX_SNAPSHOT=clean
```

Add these to your shell profile or a `.env` file.

### 7. Start Arbeiterfarm

```bash
cargo run --release --bin af
# Startup output should include:
# [sandbox-gateway] started at /run/af/sandbox_gateway.sock
```

If `AF_SANDBOX_QMP` is not set, the gateway will print a warning and disable itself — sandbox tools will still be registered but calls will fail with "gateway not running".

---

## Tools Reference

### `sandbox.trace`

Execute a sample with comprehensive default API hooks (~60 Windows APIs across 10 categories).

**Input:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `artifact_id` | UUID | Yes | — | The PE binary to execute |
| `timeout_secs` | integer | No | 30 | How long to run (5–120 seconds) |
| `args` | string[] | No | — | Command-line arguments for the sample (max 10) |

**Output (inline summary):**

```json
{
  "total_api_calls": 1847,
  "unique_apis": 23,
  "top_apis": [
    {"api": "RegQueryValueExW", "count": 412},
    {"api": "CreateFileW", "count": 203},
    {"api": "ReadFile", "count": 189}
  ],
  "process_tree": [
    {"pid": 2840, "path": "C:\\...\\sample.exe", "args": []}
  ],
  "error_count": 0,
  "hint": "Full API trace stored as artifact. Use file.grep to search for specific APIs."
}
```

**Produced artifact:** `trace.json` — full API trace with timestamps, arguments, return values, thread IDs, and backtraces.

**Trace entry format:**

```json
{
  "ts": 1708891234567,
  "api": "CreateFileW",
  "args": {
    "path": "C:\\Windows\\System32\\config\\SAM",
    "access": -2147483648,
    "share": 1
  },
  "ret": "0x1a4",
  "tid": 2840,
  "bt": ["ntdll.dll!NtOpenFile+0x14", "kernel32.dll!CreateFileW+0x52"]
}
```

### `sandbox.hook`

Execute a sample with a custom Frida JavaScript hook script. Use this when the default hooks don't cover the APIs you need, or when you want to extract specific data (decrypted buffers, parsed configs, etc.).

**Input:**

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `artifact_id` | UUID | Yes | — | The PE binary to execute |
| `hook_script` | string | Yes | — | Frida JavaScript code (max 64KB) |
| `timeout_secs` | integer | No | 30 | How long to run (5–120 seconds) |
| `args` | string[] | No | — | Command-line arguments for the sample (max 10) |

**Output (inline summary):**

```json
{
  "trace_entries": 42,
  "error_count": 0,
  "has_custom_data": true,
  "hint": "Full hook results stored as artifact. Use file.read_range to inspect."
}
```

**Produced artifact:** `hook_results.json`

### `sandbox.screenshot`

Capture the current VM display. Uses QEMU QMP `screendump` — works even if no sample is running.

**Input:** None

**Output (inline):**

```json
{
  "format": "ppm",
  "image_b64": "UDYKMTAyNCA3NjgK...",
  "size_bytes": 2359296
}
```

No artifact is produced — the image is returned inline as base64.

---

## Hooked APIs (Default Script)

The default hook script covers **~60 Windows APIs** across 10 categories:

| Category | Count | APIs | What to look for |
|---|---|---|---|
| **File** | 11 | CreateFileW/A, ReadFile, WriteFile, DeleteFileW/A, CopyFileW/A, MoveFileW/A, FindFirstFileW | File drops, config reads, self-deletion |
| **Registry** | 10 | RegOpenKeyExW/A, RegSetValueExW/A, RegQueryValueExW/A, RegDeleteKeyW/A, RegCreateKeyExW/A | Persistence via Run keys, config storage |
| **Process** | 8 | CreateProcessW/A, OpenProcess, TerminateProcess, VirtualAllocEx, WriteProcessMemory, CreateRemoteThread, NtCreateThreadEx | Child processes, process injection, hollowing |
| **Network** | 13 | connect, send, recv, InternetOpenW/A, InternetConnectW/A, HttpOpenRequestW/A, HttpSendRequestW/A, URLDownloadToFileW/A, WSAStartup, getaddrinfo, DnsQuery_W | C2 communication, downloads, DNS queries |
| **Library** | 6 | LoadLibraryW/A, LoadLibraryExW/A, GetProcAddress, LdrLoadDll | Dynamic API resolution, DLL loading |
| **Crypto** | 5 | CryptEncrypt, CryptDecrypt, CryptHashData, BCryptEncrypt, BCryptDecrypt | Ransomware encryption, C2 traffic encryption |
| **Service** | 5 | CreateServiceW/A, StartServiceW, OpenServiceW, ChangeServiceConfigW | Service-based persistence |
| **COM** | 2 | CoCreateInstance, CoGetClassObject | COM object instantiation (WMI, Shell, etc.) |
| **Memory** | 4 | VirtualAlloc, VirtualProtect, HeapCreate, NtAllocateVirtualMemory | Shellcode allocation, RWX pages |
| **Anti-debug** | 5 | IsDebuggerPresent, CheckRemoteDebuggerPresent, NtQueryInformationProcess, GetTickCount, QueryPerformanceCounter | Evasion and sandbox detection |

Each hook captures:
- **Timestamp** (`ts`) — millisecond epoch
- **API name** (`api`)
- **Arguments** (`args`) — string arguments truncated to 256 chars, pointers as hex
- **Return value** (`ret`)
- **Thread ID** (`tid`)
- **Backtrace** (`bt`) — top 3 stack frames with symbol names and offsets

The trace is capped at **10,000 entries** to prevent memory exhaustion in the guest.

---

## The Tracer Agent

Arbeiterfarm includes a pre-configured **`tracer`** agent optimized for dynamic analysis workflows:

| Property | Value |
|---|---|
| **Tools** | `sandbox.trace`, `sandbox.hook`, `sandbox.screenshot`, `file.read_range`, `file.info`, `file.grep`, `artifact.describe`, `family.tag`, `family.list` |
| **Budget** | 15 tool calls |
| **Timeout** | 600 seconds |
| **Route** | Auto (uses the best available model) |

The tracer agent follows this workflow:
1. Identify the binary with `file.info`
2. Execute with `sandbox.trace` (default hooks)
3. Analyze the trace — search for behavioral patterns with `file.grep`
4. Optionally run `sandbox.hook` with custom Frida scripts for deeper analysis
5. Capture screenshots if GUI behavior matters
6. Summarize findings with evidence citations

### Using the tracer in a workflow

```toml
# ~/.af/workflows/dynamic-analysis.toml
[workflow]
name = "dynamic-analysis"
description = "Static triage + dynamic sandbox execution + report"

[[workflow.steps]]
agent = "surface"
group = 1
prompt = "Perform initial triage of the uploaded binary."
timeout_secs = 300

[[workflow.steps]]
agent = "tracer"
group = 2
prompt = "Execute the sample in the sandbox and analyze its runtime behavior."
timeout_secs = 600

[[workflow.steps]]
agent = "reporter"
group = 3
prompt = "Write a comprehensive report covering both static and dynamic findings."
timeout_secs = 300
```

### Using the tracer in a thinking thread

```bash
af-re think --project <project-id> \
  --goal "Execute the uploaded sample in the sandbox and identify all behavioral IOCs"
```

The supervisor will automatically invoke the tracer agent via `meta.invoke_agent`.

---

## Working with Trace Artifacts

After `sandbox.trace` runs, the full trace is stored as a project artifact (`trace.json`). Use these tools to inspect it:

```
# Search for specific API calls
file.grep artifact_id=<trace-artifact-id> pattern="CreateFileW"

# Search for network activity
file.grep artifact_id=<trace-artifact-id> pattern="InternetConnect|DnsQuery|getaddrinfo"

# Search for process injection patterns
file.grep artifact_id=<trace-artifact-id> pattern="VirtualAllocEx|WriteProcessMemory|CreateRemoteThread"

# Search for registry persistence
file.grep artifact_id=<trace-artifact-id> pattern="RegSetValueEx|RegCreateKeyEx"

# Read a specific range of the trace
file.read_range artifact_id=<trace-artifact-id> offset=0 length=100
```

### Interpreting Common Patterns

**Process injection (classic)**
```
VirtualAllocEx → WriteProcessMemory → CreateRemoteThread
```
The sample allocates memory in another process, writes code into it, then starts a thread there. This is the textbook DLL injection / shellcode injection pattern.

**Persistence via registry Run key**
```
RegOpenKeyExW  subkey="SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run"
RegSetValueExW name="MyMalware" type=1
```
The sample is registering itself to run at startup.

**C2 communication**
```
getaddrinfo    node="evil-c2-domain.com"
InternetConnectW server="evil-c2-domain.com" port=443
HttpOpenRequestW verb="POST" path="/api/beacon"
HttpSendRequestW handle=0x...
```
DNS resolution followed by an HTTP POST to an external server.

**Ransomware behavior**
```
FindFirstFileW  pattern="C:\\Users\\*"
CreateFileW     path="C:\\Users\\Documents\\report.docx" access=...
ReadFile        handle=0x... bytes=4096
BCryptEncrypt   key=0x... inputLen=4096
WriteFile       handle=0x... bytes=4096
DeleteFileW     path="C:\\Users\\Documents\\report.docx"
```
Enumerating files, reading contents, encrypting, writing back, then deleting originals.

---

## Writing Custom Hook Scripts

Custom hooks for `sandbox.hook` are standard [Frida JavaScript](https://frida.re/docs/javascript-api/). The script **must** export a `getTrace()` function via `rpc.exports`:

```javascript
'use strict';

// Your data collection array
var results = [];

// ... your Interceptor.attach() hooks ...

// Required: the gateway calls this to collect results
rpc.exports = {
    getTrace: function() { return results; }
};
```

### Example: Extract Decrypted Buffers

Hook `CryptDecrypt` and capture the plaintext output after decryption completes:

```javascript
'use strict';
var results = [];

var addr = Module.findExportByName("advapi32.dll", "CryptDecrypt");
if (addr) {
    Interceptor.attach(addr, {
        onEnter: function(args) {
            this.pbData = args[3];
            this.pdwDataLen = args[4];
        },
        onLeave: function(retval) {
            if (retval.toInt32() !== 0) {
                var len = this.pdwDataLen.readU32();
                var data = this.pbData.readByteArray(Math.min(len, 4096));
                results.push({
                    api: "CryptDecrypt",
                    plaintext_hex: Array.from(new Uint8Array(data))
                        .map(function(b) { return b.toString(16).padStart(2, '0'); })
                        .join(''),
                    length: len
                });
            }
        }
    });
}

rpc.exports = {
    getTrace: function() { return results; }
};
```

### Example: Hook a Specific DLL's Internal Functions

When static analysis (Ghidra) reveals interesting functions at known offsets:

```javascript
'use strict';
var results = [];

// Wait for the target DLL to be loaded
var mod = Process.findModuleByName("malware_config.dll");
if (mod) {
    // Hook the config parser at offset 0x1234
    Interceptor.attach(mod.base.add(0x1234), {
        onEnter: function(args) {
            this.configBuf = args[0];
            this.configLen = args[1].toInt32();
        },
        onLeave: function(retval) {
            if (retval.toInt32() === 0) {
                var data = this.configBuf.readByteArray(
                    Math.min(this.configLen, 4096)
                );
                results.push({
                    type: "parsed_config",
                    data_hex: Array.from(new Uint8Array(data))
                        .map(function(b) { return b.toString(16).padStart(2, '0'); })
                        .join('')
                });
            }
        }
    });
}

rpc.exports = { getTrace: function() { return results; } };
```

### Example: Intercept String Decryption

Common in malware that decrypts strings at runtime to evade static analysis:

```javascript
'use strict';
var decrypted = [];

// Hook a suspected decryption function found via Ghidra
// e.g., FUN_00401200 renamed to "decrypt_string" after analysis
var addr = Module.findExportByName(null, "decrypt_string");
if (!addr) {
    // If not exported, use the base address + offset
    var main = Process.enumerateModules()[0];
    addr = main.base.add(0x1200);
}

Interceptor.attach(addr, {
    onLeave: function(retval) {
        try {
            var s = retval.readUtf16String();
            if (s && s.length > 0) {
                decrypted.push({
                    string: s.substring(0, 512),
                    address: retval.toString()
                });
            }
        } catch(e) {}
    }
});

rpc.exports = { getTrace: function() { return decrypted; } };
```

### Example: Monitor Socket Data

Capture the actual bytes sent over sockets to identify C2 protocol structure:

```javascript
'use strict';
var packets = [];
var MAX_CAPTURE = 1024;

var sendAddr = Module.findExportByName("ws2_32.dll", "send");
if (sendAddr) {
    Interceptor.attach(sendAddr, {
        onEnter: function(args) {
            var socket = args[0];
            var buf = args[1];
            var len = args[2].toInt32();
            var captureLen = Math.min(len, MAX_CAPTURE);
            var data = buf.readByteArray(captureLen);
            packets.push({
                direction: "send",
                socket: socket.toString(),
                length: len,
                data_hex: Array.from(new Uint8Array(data))
                    .map(function(b) { return b.toString(16).padStart(2, '0'); })
                    .join(''),
                tid: Process.getCurrentThreadId(),
                ts: Date.now()
            });
        }
    });
}

var recvAddr = Module.findExportByName("ws2_32.dll", "recv");
if (recvAddr) {
    Interceptor.attach(recvAddr, {
        onEnter: function(args) {
            this.socket = args[0];
            this.buf = args[1];
            this.maxLen = args[2].toInt32();
        },
        onLeave: function(retval) {
            var bytesRead = retval.toInt32();
            if (bytesRead > 0) {
                var captureLen = Math.min(bytesRead, MAX_CAPTURE);
                var data = this.buf.readByteArray(captureLen);
                packets.push({
                    direction: "recv",
                    socket: this.socket.toString(),
                    length: bytesRead,
                    data_hex: Array.from(new Uint8Array(data))
                        .map(function(b) { return b.toString(16).padStart(2, '0'); })
                        .join(''),
                    tid: Process.getCurrentThreadId(),
                    ts: Date.now()
                });
            }
        }
    });
}

rpc.exports = { getTrace: function() { return packets; } };
```

### Tips for Writing Hooks

1. **Always wrap in try/catch** — Frida hooks that crash will silently fail
2. **Use `Module.findExportByName()`** — returns null if the API doesn't exist (the sample might not load that DLL)
3. **Truncate large data** — reading entire buffers can exhaust memory; cap at 4KB per capture
4. **Use `readUtf16String()` for W APIs** and `readAnsiString()` for A APIs
5. **The trace cap is 10,000 entries** — for high-volume APIs, add filtering in your hook
6. **Test hooks locally first** — use `frida -l your_hook.js -f sample.exe` on a test machine
7. **Use `Process.enumerateModules()[0]`** to get the main module when you need base addresses

---

## Environment Variables Reference

| Variable | Default | Required | Description |
|---|---|---|---|
| `AF_SANDBOX_SOCKET` | *(none)* | Yes | UDS path for the sandbox gateway. If unset, sandbox tools are not registered |
| `AF_SANDBOX_QMP` | *(none)* | Yes | QEMU QMP Unix socket path. If unset, gateway starts but prints a warning |
| `AF_SANDBOX_AGENT` | `192.168.122.10:9111` | No | TCP address of the Python guest agent inside the VM |
| `AF_SANDBOX_SNAPSHOT` | `clean` | No | Snapshot name used for `loadvm` before each execution |

---

## Troubleshooting

### "Sandbox gateway not running"

The tool executor cannot connect to the UDS socket.

**Check:**
1. Is `AF_SANDBOX_SOCKET` set?
2. Did the gateway start? Look for `[sandbox-gateway] started at ...` in stderr
3. Is `AF_SANDBOX_QMP` set? Without it, the gateway won't start
4. Check socket permissions: the socket is created with mode `0660`

### "Failed to restore VM snapshot"

QMP communication failed.

**Check:**
1. Is QEMU running with `-qmp unix:/path,server,nowait`?
2. Does the snapshot exist? Verify with `info snapshots` via QMP:
   ```bash
   socat - UNIX-CONNECT:/run/af/qmp.sock <<'EOF'
   {"execute":"qmp_capabilities"}
   {"execute":"human-monitor-command","arguments":{"command-line":"info snapshots"}}
   EOF
   ```
3. Is the QMP socket path correct in `AF_SANDBOX_QMP`?

### "Failed to communicate with guest agent"

The gateway can't reach the Python agent inside the VM.

**Check:**
1. Is the agent running inside the VM? (`python agent.py`)
2. Is the network bridge working? Can you `ping 192.168.122.10` from the host?
3. Is Windows Firewall blocking port 9111?
4. Is the 2-second post-snapshot delay enough? Some VMs take longer to resume — you may need to increase the delay in `gateway.rs` (`Duration::from_secs(2)`)

### "spawn failed" in trace errors

Frida couldn't start the sample.

**Check:**
1. Is the binary a valid Windows PE?
2. Is it 32-bit or 64-bit? The Frida agent must match the architecture
3. Does the sample require specific DLLs not present in the VM?
4. Some packed/protected binaries resist Frida spawn — try increasing the timeout
5. Check the `errors` array in the trace response for Frida-level error messages

### Trace returns 0 API calls

The sample may exit before hooks fire.

**Check:**
1. Increase `timeout_secs` — some samples have delayed execution (sleep-based evasion)
2. The sample might need command-line arguments (`args` parameter)
3. The sample might detect Frida and exit — look for `IsDebuggerPresent` or `GetTickCount` in any partial trace
4. Some samples only activate with specific triggers (network conditions, date/time, presence of certain files)

### High trace entry counts / trace truncation

Traces are capped at 10,000 entries.

**Workarounds:**
1. Use `sandbox.hook` with a filtered hook script that only hooks the APIs you care about
2. Add your own filtering logic (e.g., ignore `ReadFile` on known system handles)
3. Reduce `timeout_secs` if you only need early-stage behavior

### VM is in an inconsistent state

If the VM gets stuck (e.g., `loadvm` fails, QEMU is unresponsive):

```bash
# Force-kill QEMU and restart
pkill -f qemu-system-x86_64

# Restart QEMU with your usual flags
qemu-system-x86_64 \
  -enable-kvm -m 4096 -cpu host -smp 2 \
  -drive file=/var/lib/af/windows.qcow2,format=qcow2 \
  -net nic,model=virtio -net bridge,br=virbr0 \
  -qmp unix:/run/af/qmp.sock,server,nowait \
  -vnc :1 &

# Restore the clean snapshot
socat - UNIX-CONNECT:/run/af/qmp.sock <<'EOF'
{"execute":"qmp_capabilities"}
{"execute":"human-monitor-command","arguments":{"command-line":"loadvm clean"}}
EOF
```

---

## Security Considerations

- **VM isolation**: The Windows VM runs inside QEMU/KVM with hardware virtualization. The sample has no direct access to the host filesystem or network (beyond what the bridge provides)
- **Snapshot restore**: Every `sandbox.trace` and `sandbox.hook` call restores the VM to a clean snapshot before execution, preventing state leakage between runs
- **Mutex serialization**: Only one trace/hook runs at a time — no risk of cross-contamination between concurrent requests
- **Timeout enforcement**: The gateway enforces a 120-second maximum execution time. The Python agent also enforces its own timeout
- **Network isolation**: Consider running the VM on a network with no internet access to prevent C2 callbacks. Use `iptables` rules or a dedicated bridge with no default gateway:
  ```bash
  # Create isolated bridge (no default route)
  ip link add br-sandbox type bridge
  ip addr add 192.168.200.1/24 dev br-sandbox
  ip link set br-sandbox up
  # Do NOT add a default route for this subnet
  ```
- **Trusted executor**: The sandbox tools run as `SandboxProfile::Trusted` (in-process) because they only communicate over UDS — they don't execute arbitrary code on the host

---

## File Layout

```
arbeiterfarm/
├── crates/af-re-sandbox/               # Rust crate — gateway + executors
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                   # declare(), wire(), start_sandbox_gateway()
│       ├── specs.rs                 # ToolSpec definitions for 3 tools
│       ├── executor.rs              # ToolExecutor impls (gateway UDS calls)
│       ├── gateway.rs               # UDS daemon, VM orchestration
│       ├── qmp.rs                   # QMP client (savevm, loadvm, screendump)
│       ├── agent_client.rs          # TCP client to guest Python agent
│       └── hooks.rs                 # Default Frida hook script (~60 APIs)
└── sandbox-agent/                   # Python guest agent (runs inside VM)
    ├── agent.py                     # TCP server on :9111
    ├── hooks/
    │   └── default.js               # Standalone copy of default hooks
    └── requirements.txt             # frida==16.5.2, frida-tools==13.6.0
```

---

## Internal Protocol Reference

### Gateway UDS Protocol

Executors communicate with the gateway over a Unix domain socket using JSON lines (one JSON object per line, one request/response per connection).

**Request → Gateway:**

```json
{"action": "trace", "sample_b64": "TVqQAAM...", "timeout_secs": 30, "args": null}
```

```json
{"action": "hook", "sample_b64": "TVqQAAM...", "hook_script": "...", "timeout_secs": 30}
```

```json
{"action": "screenshot"}
```

**Response ← Gateway (success):**

```json
{
  "ok": true,
  "data": {
    "trace": [...],
    "process_tree": [...],
    "errors": []
  }
}
```

**Response ← Gateway (error):**

```json
{
  "ok": false,
  "error": "vm_error",
  "message": "failed to restore VM snapshot"
}
```

### Guest Agent TCP Protocol

The gateway communicates with the Python agent inside the VM over TCP port 9111 using JSON lines.

**Request → Agent:**

```json
{
  "cmd": "trace",
  "sample_b64": "TVqQAAMAAAA...",
  "hook_script": "...",
  "timeout": 30,
  "args": ["--config", "test.cfg"]
}
```

**Response ← Agent:**

```json
{
  "status": "ok",
  "trace": [
    {"ts": 1708891234567, "api": "CreateFileW", "args": {...}, "ret": "0x1a4", "tid": 2840, "bt": [...]}
  ],
  "process_tree": [
    {"pid": 2840, "path": "C:\\...\\sample.exe", "args": []}
  ],
  "errors": []
}
```

### QMP Protocol

The gateway communicates with QEMU via the [QMP (QEMU Machine Protocol)](https://wiki.qemu.org/Documentation/QMP) over a Unix socket.

**Handshake:**
```json
→ (read greeting)  {"QMP": {"version": {...}}}
← {"execute": "qmp_capabilities"}
→ {"return": {}}
```

**Snapshot restore:**
```json
← {"execute": "human-monitor-command", "arguments": {"command-line": "loadvm clean"}}
→ {"return": ""}
```

**Screenshot:**
```json
← {"execute": "screendump", "arguments": {"filename": "/tmp/af_sandbox_screen.ppm"}}
→ {"return": {}}
```

QMP also emits asynchronous events (e.g., `BLOCK_JOB_COMPLETED`). The QMP client skips these by checking for the `"event"` key.
