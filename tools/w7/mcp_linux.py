#!/usr/bin/env python3
"""MCP server for disposable SSH-only Linux Samba VMs."""

import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parent
VM = ROOT / "linux_vm.py"


TOOLS = [
    {
        "name": "linux_vm_fetch",
        "description": (
            "Download and SHA-512 verify the official Debian 13 arm64 cloud base. "
            "The immutable image is stored outside the repository."
        ),
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "linux_vm_up",
        "description": (
            "Create a Linux clone when absent, then boot it headlessly. Creation settings "
            "are ignored for an existing clone. Set wait to return after cloud-init and "
            "Samba validation. One VM owns the shared lab address 192.168.77.1."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {"name": {"type": "string",
                                      "description": "Unique lowercase VM name."},
                           "cpus": {"type": "integer", "default": 2,
                                    "minimum": 1, "maximum": 16},
                           "memory_mb": {"type": "integer", "default": 2048,
                                         "minimum": 512, "maximum": 65536},
                           "disk_gb": {"type": "integer", "default": 16,
                                       "minimum": 8, "maximum": 1024},
                           "ssh_port": {"type": "integer", "minimum": 1024,
                                        "maximum": 65535},
                           "samba_port": {"type": "integer", "minimum": 1024,
                                          "maximum": 65535},
                           "wait": {"type": "boolean", "default": False},
                           "timeout": {"type": "integer", "default": 600,
                                       "minimum": 1, "maximum": 900}},
            "required": ["name"],
        },
    },
    {
        "name": "linux_ssh",
        "description": (
            "Run a shell command over the clone's private SSH key. Use /srv/agent for "
            "the Samba share exported to Windows as //192.168.77.1/agent."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {"name": {"type": "string"},
                           "command": {"type": "string"},
                           "timeout": {"type": "integer", "default": 120,
                                       "minimum": 1, "maximum": 900}},
            "required": ["name", "command"],
        },
    },
    {
        "name": "linux_vm_down",
        "description": (
            "Cleanly stop one VM and delete its overlay, key, and metadata. Set "
            "preserve_machine to keep the stopped VM for reproduction or reuse."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {"name": {"type": "string"},
                           "timeout": {"type": "integer", "default": 60,
                                       "minimum": 1, "maximum": 120},
                           "preserve_machine": {"type": "boolean", "default": False}},
            "required": ["name"],
        },
    },
    {
        "name": "linux_vm_status",
        "description": "List every Linux VM, or report whether one is absent, stopped, or running.",
        "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}},
    },
]


def run(argv, timeout=120):
    process = subprocess.run([sys.executable, str(VM)] + argv, capture_output=True,
                             text=True, timeout=timeout)
    output = (process.stdout + process.stderr).strip()
    return [{"type": "text", "text": output or "ok"}], process.returncode != 0


def call_tool(name, args):
    if name == "linux_vm_fetch":
        return run(["fetch"], 900)
    if name == "linux_vm_up":
        argv = ["up", args["name"]]
        for key, option in (("cpus", "--cpus"), ("memory_mb", "--memory"),
                            ("disk_gb", "--disk"), ("ssh_port", "--ssh-port"),
                            ("samba_port", "--samba-port")):
            if args.get(key) is not None:
                argv += [option, str(args[key])]
        timeout = args.get("timeout", 600)
        if args.get("wait"):
            argv += ["--wait", "--timeout", str(timeout)]
        return run(argv, timeout + 15 if args.get("wait") else 120)
    if name == "linux_ssh":
        timeout = args.get("timeout", 120)
        return run(["ssh", args["name"], "--", args["command"]], timeout)
    if name == "linux_vm_down":
        timeout = args.get("timeout", 60)
        argv = ["down", args["name"], "--timeout", str(timeout)]
        if args.get("preserve_machine"):
            argv.append("--preserve-machine")
        return run(argv, timeout + 15)
    if name == "linux_vm_status":
        return run(["status"] + ([args["name"]] if args.get("name") else []))
    return [{"type": "text", "text": "unknown tool: %s" % name}], True


def handle(method, params):
    if method == "initialize":
        return {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                "serverInfo": {"name": "one-linux", "version": "1.0.0"}}
    if method == "ping":
        return {}
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "tools/call":
        content, is_error = call_tool(params.get("name"), params.get("arguments") or {})
        result = {"content": content}
        if is_error:
            result["isError"] = True
        return result
    return None


def serve():
    for line in sys.stdin:
        try:
            message = json.loads(line)
            if message.get("id") is None:
                continue
            result = handle(message.get("method"), message.get("params") or {})
            if result is None:
                reply = {"jsonrpc": "2.0", "id": message["id"],
                         "error": {"code": -32601, "message": "unknown method"}}
            else:
                reply = {"jsonrpc": "2.0", "id": message["id"], "result": result}
        except Exception as e:
            reply = {"jsonrpc": "2.0", "id": None,
                     "error": {"code": -32603, "message": "%s: %s" %
                               (type(e).__name__, e)}}
        print(json.dumps(reply), flush=True)


if __name__ == "__main__":
    serve()
