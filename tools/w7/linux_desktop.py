"""Guest half of the one-linux desktop tools.

mcp_linux.py pipes this source plus a `main(request)` call into `python3 -` over
SSH, so the guest needs no installed agent and always runs the current code.
"""

import base64
import json
import os
import signal
import struct
import subprocess
import tempfile
import time

UI_LINE_LIMIT = 1500


def screen():
    png = subprocess.run(["maim", "--hidecursor"], capture_output=True, check=True).stdout
    width, height = struct.unpack(">II", png[16:24])
    return {"png_b64": base64.b64encode(png).decode("ascii"), "w": width, "h": height}


def output(*argv):
    process = subprocess.run(argv, capture_output=True, text=True)
    return process.stdout.strip() if process.returncode == 0 else None


def active_window():
    window = output("xdotool", "getactivewindow")
    if not window:
        return None
    # Debian's xdotool predates getwindowclassname; WM_CLASS reads `= "inst", "Class"`.
    wm_class = output("xprop", "-notype", "-id", window, "WM_CLASS") or ""
    pid = output("xdotool", "getwindowpid", window)
    return {"title": output("xdotool", "getwindowname", window) or "",
            "class": wm_class.rsplit('"', 2)[-2] if wm_class.count('"') >= 2 else "",
            "pid": int(pid) if pid else None}


def run_script(script, timeout_ms, shot_delay_ms):
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        # Files, not pipes: an app the script starts in the background inherits
        # these descriptors and would otherwise hold the SSH channel open.
        process = subprocess.Popen(["bash", "-c", script], stdin=subprocess.DEVNULL,
                                   stdout=out, stderr=err, start_new_session=True)
        try:
            code = process.wait(timeout_ms / 1000)
            error = None
            time.sleep(shot_delay_ms / 1000)
            shot = screen()
        except subprocess.TimeoutExpired:
            shot = screen()  # before the kill, while whatever blocked is on screen
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            code = None
            error = "script timed out after %d ms, process group killed" % timeout_ms
        out.seek(0)
        err.seek(0)
        return dict(shot, exit=code, error=error, win=active_window(),
                    stdout=out.read().decode("utf-8", "replace"),
                    stderr=err.read().decode("utf-8", "replace"))


def ui_tree():
    import gi
    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi, GLib

    win = active_window()
    desktop = Atspi.get_desktop(0)
    apps = [desktop.get_child_at_index(i) for i in range(desktop.get_child_count())]
    chosen = [app for app in apps if win and app.get_process_id() == win["pid"]] or apps
    lines = []

    def walk(node, depth):
        if len(lines) >= UI_LINE_LIMIT:
            return
        states = node.get_state_set()
        if depth and not states.contains(Atspi.StateType.SHOWING):
            return
        line = "%s%s %r" % ("  " * depth, node.get_role_name(), node.get_name())
        if depth:
            try:
                rect = node.get_extents(Atspi.CoordType.SCREEN)
                line += " %d,%d,%d,%d" % (rect.x, rect.y, rect.width, rect.height)
            except GLib.Error:
                pass
        text = node.get_text_iface()
        if text:
            value = Atspi.Text.get_text(text, 0, 200)
            if value and value != node.get_name():
                line += " = %r" % value
        if states.contains(Atspi.StateType.FOCUSED):
            line += " [focused]"
        lines.append(line)
        for i in range(node.get_child_count()):
            walk(node.get_child_at_index(i), depth + 1)

    for app in chosen:
        walk(app, 0)
    if len(lines) >= UI_LINE_LIMIT:
        lines.append("... truncated at %d lines" % UI_LINE_LIMIT)
    return {"win": win, "controls": "\n".join(lines)}


def main(request):
    request = json.loads(request)
    verb = request["verb"]
    if verb == "exec":
        reply = run_script(request["script"], request["timeout_ms"],
                           request["shot_delay_ms"])
    elif verb == "shot":
        reply = dict(screen(), win=active_window())
    elif verb == "ui":
        reply = ui_tree()
    else:
        raise ValueError("unknown verb %r" % verb)
    print(json.dumps(reply))
