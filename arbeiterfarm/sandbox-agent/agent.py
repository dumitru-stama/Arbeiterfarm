#!/usr/bin/env python3
"""
Sandbox guest agent — runs inside the Windows VM.

Listens on TCP port 9111 for JSON-line commands from the sandbox gateway.
Uses Frida to spawn processes, inject hook scripts, and collect API traces.

Commands:
  - trace: Spawn sample with hook script, collect API trace
  - hook:  Spawn sample with custom hook script, collect results

Protocol: one JSON line in, one JSON line out, then close.
"""

import base64
import json
import os
import socket
import sys
import tempfile
import threading
import time
import traceback

try:
    import frida
except ImportError:
    print("ERROR: frida not installed. Run: pip install frida frida-tools", file=sys.stderr)
    sys.exit(1)

LISTEN_HOST = "0.0.0.0"
LISTEN_PORT = 9111
SAMPLE_DIR = os.path.join(tempfile.gettempdir(), "af_samples")
DEFAULT_HOOK_PATH = os.path.join(os.path.dirname(__file__), "hooks", "default.js")


def load_default_hooks():
    """Load the default hook script from disk."""
    if os.path.exists(DEFAULT_HOOK_PATH):
        with open(DEFAULT_HOOK_PATH, "r") as f:
            return f.read()
    return ""


def write_sample(sample_b64):
    """Decode and write sample to a temp file. Returns the file path."""
    os.makedirs(SAMPLE_DIR, exist_ok=True)
    sample_bytes = base64.b64decode(sample_b64)
    # Use a fixed name so we can track it
    sample_path = os.path.join(SAMPLE_DIR, "sample.exe")
    with open(sample_path, "wb") as f:
        f.write(sample_bytes)
    return sample_path


def run_with_frida(sample_path, hook_script, timeout_secs, args=None):
    """
    Spawn the sample with Frida, inject hooks, wait for timeout, collect trace.
    Returns (trace_list, process_tree, errors).
    """
    trace = []
    process_tree = []
    errors = []

    if not hook_script:
        hook_script = load_default_hooks()

    cmd_args = [sample_path]
    if args:
        cmd_args.extend(args)

    device = frida.get_local_device()

    try:
        pid = device.spawn(cmd_args)
    except Exception as e:
        errors.append(f"spawn failed: {e}")
        return trace, process_tree, errors

    process_tree.append({"pid": pid, "path": sample_path, "args": args or []})

    try:
        session = device.attach(pid)
    except Exception as e:
        errors.append(f"attach failed: {e}")
        try:
            device.kill(pid)
        except Exception:
            pass
        return trace, process_tree, errors

    script = None
    try:
        script = session.create_script(hook_script)

        def on_message(message, data):
            if message.get("type") == "error":
                errors.append(message.get("description", str(message)))

        script.on("message", on_message)
        script.load()

        # Resume the spawned process
        device.resume(pid)

        # Wait for the specified timeout
        time.sleep(timeout_secs)

        # Collect trace from the script's RPC exports
        try:
            trace = script.exports_sync.get_trace()
        except Exception as e:
            errors.append(f"getTrace failed: {e}")

    except Exception as e:
        errors.append(f"frida error: {e}")
    finally:
        # Clean up
        try:
            if script:
                script.unload()
        except Exception:
            pass
        try:
            session.detach()
        except Exception:
            pass
        try:
            device.kill(pid)
        except Exception:
            pass

    return trace, process_tree, errors


def handle_trace(req):
    """Handle a 'trace' command."""
    sample_b64 = req.get("sample_b64", "")
    if not sample_b64:
        return {"status": "error", "errors": ["missing sample_b64"]}

    hook_script = req.get("hook_script", "")
    timeout_secs = min(req.get("timeout", 30), 120)
    args = req.get("args")

    sample_path = write_sample(sample_b64)
    trace, process_tree, errors = run_with_frida(
        sample_path, hook_script, timeout_secs, args
    )

    return {
        "status": "ok" if not errors else "partial",
        "trace": trace,
        "process_tree": process_tree,
        "errors": errors,
    }


def handle_hook(req):
    """Handle a 'hook' command (same as trace but expects custom hook_script)."""
    return handle_trace(req)


def handle_client(conn, addr):
    """Handle a single client connection."""
    try:
        data = b""
        while True:
            chunk = conn.recv(4096)
            if not chunk:
                break
            data += chunk
            if b"\n" in data:
                break

        if not data:
            return

        line = data.split(b"\n")[0]
        req = json.loads(line.decode("utf-8"))
        cmd = req.get("cmd", "")

        if cmd == "trace":
            resp = handle_trace(req)
        elif cmd == "hook":
            resp = handle_hook(req)
        else:
            resp = {"status": "error", "errors": [f"unknown command: {cmd}"]}

        resp_bytes = json.dumps(resp).encode("utf-8") + b"\n"
        conn.sendall(resp_bytes)

    except Exception as e:
        try:
            err_resp = json.dumps({
                "status": "error",
                "errors": [f"agent error: {e}"],
            }).encode("utf-8") + b"\n"
            conn.sendall(err_resp)
        except Exception:
            pass
        traceback.print_exc()
    finally:
        conn.close()


def main():
    print(f"[sandbox-agent] starting on {LISTEN_HOST}:{LISTEN_PORT}")

    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind((LISTEN_HOST, LISTEN_PORT))
    srv.listen(1)  # Only one trace at a time

    print(f"[sandbox-agent] listening...")

    while True:
        conn, addr = srv.accept()
        print(f"[sandbox-agent] connection from {addr}")
        # Handle synchronously (one trace at a time)
        handle_client(conn, addr)


if __name__ == "__main__":
    main()
