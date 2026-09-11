#!/usr/bin/env python3
"""MCP server (stdio, newline-delimited JSON-RPC) and CLI for driving the
Windows 7 box `wayback` through its AutoHotkey exec listener."""

import base64
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import urllib.error
import urllib.request

WAYBACK_BASE = os.environ.get("WIN7", "http://100.104.74.68:8777").rstrip("/")
WAYBACK_TOKEN = os.environ.get("WIN7_TOKEN")
DEFAULT_TARGET = os.environ.get("WIN7_TARGET", "wayback")
TARGETS_PATH = Path(os.environ.get(
    "WIN7_TARGETS_FILE", "/Volumes/Documents/OneNote VMs/targets.json"
)).expanduser()
VM = Path(__file__).with_name("vm.py")


class Win7Error(Exception):
    pass


def resolve_target(name):
    targets = {
        "wayback": {"base": WAYBACK_BASE, "token": WAYBACK_TOKEN},
        "local": {"base": "http://127.0.0.1:18777"},
    }
    if TARGETS_PATH.exists():
        try:
            configured = json.loads(TARGETS_PATH.read_text())
        except (OSError, ValueError) as e:
            raise Win7Error("Cannot read %s: %s" % (TARGETS_PATH, e))
        if not isinstance(configured, dict):
            raise Win7Error("Windows targets must be a JSON object: %s" % TARGETS_PATH)
        targets.update(configured)
    name = name or DEFAULT_TARGET
    target = targets.get(name)
    if not isinstance(target, dict) or not target.get("base"):
        raise Win7Error(
            "Unknown Windows target %r. Choose: %s"
            % (name, ", ".join(sorted(targets)))
        )
    token = target.get("token")
    if target.get("token_env"):
        token = os.environ.get(target["token_env"])
    return name, target["base"].rstrip("/"), token


def request(path, payload, timeout_ms=15000, target=None):
    """POST json to the listener (GET when payload is None). Raises Win7Error."""
    name, base, token = resolve_target(target)
    headers = {"Content-Type": "application/json"}
    if token:
        headers["X-Win7-Token"] = token
    body = None if payload is None else json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(base + path, data=body, headers=headers)
    try:
        # The box owns the deadline; give the socket slack so its own timeout
        # wins and we get a real stdout/stderr back instead of a dead socket.
        with urllib.request.urlopen(req, timeout=timeout_ms / 1000.0 + 15) as resp:
            raw = resp.read()
    except urllib.error.HTTPError as e:
        raise Win7Error("%s %s: HTTP %d %s" % (path, base, e.code, e.reason))
    except (urllib.error.URLError, socket.timeout, OSError) as e:
        reason = getattr(e, "reason", e)
        raise Win7Error(
            "Cannot reach %r at %s (%s). Start it and open the desktop agent."
            % (name, base, reason)
        )
    try:
        return json.loads(raw.decode("utf-8"))
    except ValueError:
        raise Win7Error("%s returned non-JSON: %r" % (path, raw[:200]))


def do_health(target=None):
    return request("/health", None, target=target)


def do_shot(target=None):
    return request("/shot", {}, target=target)


def do_exec(script, shot_delay_ms=500, timeout_ms=60000, target=None):
    return request(
        "/exec",
        {"script": script, "shot_delay_ms": shot_delay_ms, "timeout_ms": timeout_ms},
        timeout_ms,
        target,
    )


def do_cmd(command, timeout_ms=60000, target=None):
    return request(
        "/cmd", {"command": command, "timeout_ms": timeout_ms}, timeout_ms, target
    )


def do_spawn(command, target=None):
    return request("/spawn", {"command": command}, target=target)


def do_ui(target=None):
    return request("/ui", {}, target=target)


# The base64 stays inside this process on both transfers: a tool that took file
# bytes as an argument would spend the whole file as context tokens.
def do_put(local, remote, target=None):
    with open(local, "rb") as f:
        blob = f.read()
    resp = request("/put", {"path": remote, "b64": base64.b64encode(blob).decode("ascii")},
                   120000, target)
    resp.setdefault("bytes", len(blob))
    return resp


def do_get(remote, local, target=None):
    resp = request("/get", {"path": remote}, 120000, target)
    if resp.get("b64"):
        with open(local, "wb") as f:
            f.write(base64.b64decode(resp["b64"]))
    return {"bytes": resp.get("bytes"), "path": os.path.abspath(local),
            "error": resp.get("error")}


# Raw string: this text is mostly about backslashes, and rendering it correctly
# matters more than keeping the source lines joined.
EXEC_DESCRIPTION = r"""Run an AutoHotkey v2 script on the Windows 7 desktop.
Returns whatever the script printed, plus a screenshot taken shot_delay_ms
after the script exits.

Coordinates are screen-absolute and match the returned screenshot
pixel-for-pixel (CoordMode Screen is already set; do not change it). The only
way to send text back is FileAppend(text, "*") -- there is no implicit output.
Put a whole sequence of actions in one script; one call per click is slow and
blind.

BACKSLASHES. AutoHotkey's escape character is the backtick, NOT the backslash,
so a backslash inside an AHK string is already literal. You are emitting this
script as a JSON string, so one literal backslash is written "\\" in the JSON
and arrives in the script as "\". Never write "\\\\" -- that is what makes an
app receive A:\\cute.png instead of A:\cute.png. Escape inside AHK with the
backtick instead: `n newline, `t tab, `" quote.

LITERAL TEXT. Send() reads ^ + ! # { } as Ctrl/Shift/Alt/Win and key groups.
Use SendText() for anything literal -- paths, passwords, arbitrary content --
and keep Send() for actual key combinations.

CLEAN UP. When you finish a task, close the applications you opened (WinClose,
or the app's own quit path). Leaving windows stacked makes later screenshots
harder to read, and a forgotten modal swallows input from the next script.

Click something, let the UI settle:
  Click(512, 384)
  Sleep(300)

Type a literal path into the focused field:
  SendText("A:\cute.png")
  Send("{Enter}")

Shortcut, then read the result out of the clipboard:
  Send("^a^c")
  ClipWait(1)
  FileAppend(A_Clipboard, "*")

Launch an app, wait for its window, and close it when done:
  Run("mspaint.exe")
  WinWait("Paint", , 10)
  WinActivate()
  WinClose("Paint")

Raise timeout_ms when the script itself waits on the UI; raise shot_delay_ms
when an animation or app launch needs longer to settle before the screenshot."""

TARGET_PROPERTY = {
    "type": "string",
    "description": "Target name. Omit to use the configured default.",
}

TOOLS = [
    {
        "name": "win7_exec",
        "description": EXEC_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": TARGET_PROPERTY,
                "script": {"type": "string", "description": "AutoHotkey v2 source."},
                "shot_delay_ms": {
                    "type": "integer",
                    "default": 500,
                    "description": "Wait this long after the script ends, then screenshot.",
                },
                "timeout_ms": {
                    "type": "integer",
                    "default": 60000,
                    "description": "Kill the script after this long.",
                },
            },
            "required": ["script"],
        },
    },
    {
        "name": "win7_cmd",
        "description": (
            "Run a command through cmd.exe on the Windows 7 box and return its output. "
            "No screenshot -- use it to inspect files, launch programs and check state "
            "without spending a screenshot on it."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": TARGET_PROPERTY,
                "command": {"type": "string", "description": "Passed to cmd.exe /c."},
                "timeout_ms": {"type": "integer", "default": 60000},
            },
            "required": ["command"],
        },
    },
    {
        "name": "win7_spawn",
        "description": (
            "Start a detached Windows process and return immediately. Its standard "
            "handles are closed, so a long-lived GUI or capture process cannot wedge "
            "the control channel."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": TARGET_PROPERTY,
                "command": {"type": "string", "description": "Windows command line."},
            },
            "required": ["command"],
        },
    },
    {
        "name": "win7_put",
        "description": (
            "Copy a file from this Mac to the Windows 7 box. Give two paths; the bytes "
            "never pass through the conversation, so file size costs nothing."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": TARGET_PROPERTY,
                "local": {"type": "string", "description": "Path on the Mac."},
                "remote": {
                    "type": "string",
                    "description": "Windows path, e.g. C:\\\\work\\\\a.one.",
                },
            },
            "required": ["local", "remote"],
        },
    },
    {
        "name": "win7_get",
        "description": "Copy a file from the Windows 7 box back to this Mac.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "target": TARGET_PROPERTY,
                "remote": {"type": "string", "description": "Windows path."},
                "local": {"type": "string", "description": "Path on the Mac."},
            },
            "required": ["remote", "local"],
        },
    },
    {
        "name": "win7_shot",
        "description": "Screenshot the Windows 7 desktop without running anything.",
        "inputSchema": {"type": "object", "properties": {"target": TARGET_PROPERTY}},
    },
    {
        "name": "win7_ui",
        "description": (
            "Dump the foreground window's control tree as text -- class name, window "
            "text and client rect (l,t,w,h) for the window and each child control. "
            "It reads real Win32 controls, so it is excellent for dialogs, menus and "
            "standard controls, and near-useless for custom-drawn canvases like "
            "OneNote's page surface -- reach for a screenshot there instead."
        ),
        "inputSchema": {"type": "object", "properties": {"target": TARGET_PROPERTY}},
    },
]

TOOLS += [
    {
        "name": "win7_vm_up",
        "description": (
            "Create a named Windows 7 clone when absent, then boot it. Creation settings "
            "are ignored for an existing clone. Set wait to return only when its "
            "authenticated desktop agent reports the expected hostname."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string",
                         "description": "Unique 1-11 character lowercase VM name."},
                "hostname": {"type": "string", "description": "Optional Windows hostname."},
                "cpus": {"type": "integer", "default": 2, "minimum": 1, "maximum": 16},
                "memory_mb": {"type": "integer", "default": 4096,
                              "minimum": 1024, "maximum": 65536},
                "port": {"type": "integer", "minimum": 1024, "maximum": 65535},
                "display": {"type": "boolean", "default": False},
                "wait": {"type": "boolean", "default": False},
                "timeout": {"type": "integer", "default": 300, "minimum": 1,
                            "maximum": 900},
            },
            "required": ["name"],
        },
    },
    {
        "name": "win7_vm_down",
        "description": (
            "Cleanly stop one clone and delete its overlay and metadata. Set "
            "preserve_machine to keep the stopped clone for reproduction or reuse."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "timeout": {"type": "integer", "default": 60, "minimum": 1,
                            "maximum": 110},
                "preserve_machine": {"type": "boolean", "default": False},
            },
            "required": ["name"],
        },
    },
    {
        "name": "win7_vm_status",
        "description": "List every clone, or report whether one named clone is absent, stopped, or running.",
        "inputSchema": {
            "type": "object",
            "properties": {"name": {"type": "string"}},
        },
    },
]


def win_line(resp):
    win = resp.get("win")
    if not win or not (win.get("title") or win.get("class")):
        return ""
    tag = win.get("class") or ""
    if win.get("dialog"):
        tag = (tag + " dialog").strip()
    return 'window: "%s" (%s)' % (win.get("title", ""), tag)


def shot_blocks(resp, text_prefix=""):
    text = text_prefix + "screen: %sx%s" % (resp.get("w"), resp.get("h"))
    wl = win_line(resp)
    if wl:
        text += "\n" + wl
    blocks = [{"type": "text", "text": text}]
    png = resp.get("png_b64")
    if png:
        blocks.append({"type": "image", "data": png, "mimeType": "image/png"})
    return blocks


def text_result(resp):
    lines = ["exit=%s" % resp.get("exit")]
    for key in ("stdout", "stderr", "error"):
        val = resp.get(key)
        if val:
            lines.append("%s:\n%s" % (key, val))
    return "\n".join(lines)


def vm_tool(name, args):
    verb = name.removeprefix("win7_vm_")
    if verb == "status":
        argv = ["status"] + ([args["name"]] if args.get("name") else [])
    elif verb == "up":
        argv = ["up", args["name"]]
        for key, option in (("hostname", "--hostname"), ("cpus", "--cpus"),
                            ("memory_mb", "--memory"), ("port", "--port")):
            if args.get(key) is not None:
                argv += [option, str(args[key])]
        if args.get("display"):
            argv.append("--display")
        if args.get("wait"):
            argv += ["--wait", "--timeout", str(args.get("timeout", 300))]
    elif verb == "down":
        argv = ["down", args["name"], "--timeout", str(args.get("timeout", 60))]
        if args.get("preserve_machine"):
            argv.append("--preserve-machine")
    process = subprocess.run(
        [sys.executable, str(VM)] + argv,
        capture_output=True,
        text=True,
        timeout=args.get("timeout", 300) + 15 if verb == "up" and args.get("wait")
        else 120,
    )
    output = (process.stdout + process.stderr).strip()
    return [{"type": "text", "text": output or "ok"}], process.returncode != 0


def call_tool(name, args):
    if name.startswith("win7_vm_"):
        return vm_tool(name, args)
    target = args.get("target")
    if name == "win7_shot":
        return shot_blocks(do_shot(target)), False
    if name == "win7_ui":
        resp = do_ui(target)
        wl = win_line(resp)
        parts = [p for p in (wl, resp.get("controls"), resp.get("error") and
                             "error: %s" % resp["error"]) if p]
        return [{"type": "text", "text": "\n".join(parts)}], False
    if name == "win7_put":
        resp = do_put(args["local"], args["remote"], target)
        text = "wrote %s bytes to %s" % (resp.get("bytes"), resp.get("path"))
        return [{"type": "text", "text": text}], False
    if name == "win7_get":
        resp = do_get(args["remote"], args["local"], target)
        text = "read %s bytes to %s" % (resp.get("bytes"), resp.get("path"))
        return [{"type": "text", "text": text}], False
    if name == "win7_cmd":
        command = args.get("command")
        if not isinstance(command, str) or not command.strip():
            return [{"type": "text", "text": "command is required"}], True
        resp = do_cmd(command, args.get("timeout_ms", 60000), target)
        return [{"type": "text", "text": text_result(resp)}], False
    if name == "win7_spawn":
        command = args.get("command")
        if not isinstance(command, str) or not command.strip():
            return [{"type": "text", "text": "command is required"}], True
        resp = do_spawn(command, target)
        return [{"type": "text", "text": "pid=%s" % resp.get("pid")}], False
    if name == "win7_exec":
        script = args.get("script")
        if not isinstance(script, str) or not script.strip():
            return [{"type": "text", "text": "script is required"}], True
        resp = do_exec(
            script,
            args.get("shot_delay_ms", 500),
            args.get("timeout_ms", 60000),
            target,
        )
        return shot_blocks(resp, text_result(resp) + "\n"), False
    raise Win7Error("unknown tool %r" % (name,))


def handle(method, params):
    if method == "initialize":
        return {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "win7", "version": "1.0.0"},
        }
    if method == "ping":
        return {}
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "tools/call":
        try:
            content, is_error = call_tool(params.get("name"), params.get("arguments") or {})
        except Win7Error as e:
            content, is_error = [{"type": "text", "text": str(e)}], True
        # No structuredContent key, ever: Codex drops content[] outright when it
        # is present (openai/codex#10334), which silently discards the screenshot.
        result = {"content": content, "_meta": {"codex/imageDetail": "original"}}
        if is_error:
            result["isError"] = True
        return result
    return None


def serve():
    out = sys.stdout
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            print("win7: dropping unparseable line: %r" % line[:200], file=sys.stderr)
            continue
        mid = msg.get("id")
        if mid is None:
            continue  # notification: a reply would itself be a protocol error
        try:
            result = handle(msg.get("method"), msg.get("params") or {})
        except Exception as e:  # a crash here would wedge the client forever
            reply = {
                "jsonrpc": "2.0",
                "id": mid,
                "error": {"code": -32603, "message": "%s: %s" % (type(e).__name__, e)},
            }
        else:
            if result is None:
                reply = {
                    "jsonrpc": "2.0",
                    "id": mid,
                    "error": {"code": -32601, "message": "unknown method %r" % msg.get("method")},
                }
            else:
                reply = {"jsonrpc": "2.0", "id": mid, "result": result}
        out.write(json.dumps(reply) + "\n")
        out.flush()


SHOT_PATH = "screenshot.png"


def write_png(resp):
    with open(SHOT_PATH, "wb") as f:
        f.write(base64.b64decode(resp["png_b64"]))
    print("%sx%s -> %s" % (resp.get("w"), resp.get("h"), SHOT_PATH))


def cli(argv):
    target = None
    if argv[:1] == ["--target"]:
        if len(argv) < 3:
            raise Win7Error("usage: mcp_win7.py --target <name> <command>")
        target, argv = argv[1], argv[2:]
    verb = argv[0]
    if verb == "health":
        print(json.dumps(do_health(target), indent=2))
    elif verb == "shot":
        write_png(do_shot(target))
    elif verb == "ui":
        resp = do_ui(target)
        wl = win_line(resp)
        if wl:
            print(wl)
        print(resp.get("controls") or "")
        if resp.get("error"):
            print("error: %s" % resp["error"], file=sys.stderr)
    elif verb in ("put", "get"):
        if len(argv) < 3:
            raise Win7Error("usage: mcp_win7.py put <local> <remote> | get <remote> <local>")
        resp = (do_put(argv[1], argv[2], target) if verb == "put"
                else do_get(argv[1], argv[2], target))
        print("%s bytes -> %s" % (resp.get("bytes"), resp.get("path")))
    elif verb in ("exec", "cmd", "spawn"):
        if len(argv) < 2:
            raise Win7Error("usage: mcp_win7.py %s '<text>'" % verb)
        if verb == "exec":
            resp = do_exec(argv[1], target=target)
        elif verb == "cmd":
            resp = do_cmd(argv[1], target=target)
        else:
            resp = do_spawn(argv[1], target=target)
            print("pid=%s" % resp.get("pid"))
            return
        for stream, text in ((sys.stdout, resp.get("stdout")), (sys.stderr, resp.get("stderr"))):
            if text:
                stream.write(text if text.endswith("\n") else text + "\n")
        if resp.get("error"):
            print("error: %s" % resp["error"], file=sys.stderr)
        print("exit=%s" % resp.get("exit"), file=sys.stderr)
        if verb == "exec":
            write_png(resp)
    else:
        raise Win7Error(
            "usage: mcp_win7.py [--target <name>] "
            "health|shot|ui|exec <ahk>|cmd <command>|spawn <command>|put <local> <remote>"
            "|get <remote> <local>"
        )


if __name__ == "__main__":
    if len(sys.argv) > 1:
        try:
            cli(sys.argv[1:])
        except Win7Error as e:
            print("win7: %s" % e, file=sys.stderr)
            sys.exit(1)
    else:
        try:
            serve()
        except KeyboardInterrupt:
            pass
