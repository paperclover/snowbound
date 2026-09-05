import base64
import ctypes
import hashlib
import json
import os
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib
from ctypes import wintypes
from http.server import ThreadingHTTPServer, BaseHTTPRequestHandler

BASE = os.path.dirname(os.path.abspath(__file__))
with open(__file__, 'rb') as source:
    AGENT_SHA256 = hashlib.sha256(source.read()).hexdigest()
LOCK = threading.Lock()

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32

SRCCOPY = 0x00CC0020
CAPTUREBLT = 0x40000000
BI_RGB = 0
DIB_RGB_COLORS = 0

# restype must be set on x64: the default c_int return truncates a 64-bit HANDLE.
user32.GetDC.restype = wintypes.HDC
user32.GetDC.argtypes = [wintypes.HWND]
user32.ReleaseDC.restype = ctypes.c_int
user32.ReleaseDC.argtypes = [wintypes.HWND, wintypes.HDC]
user32.GetSystemMetrics.restype = ctypes.c_int
user32.GetSystemMetrics.argtypes = [ctypes.c_int]
gdi32.CreateCompatibleDC.restype = wintypes.HDC
gdi32.CreateCompatibleDC.argtypes = [wintypes.HDC]
gdi32.CreateCompatibleBitmap.restype = wintypes.HBITMAP
gdi32.CreateCompatibleBitmap.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int]
gdi32.SelectObject.restype = wintypes.HGDIOBJ
gdi32.SelectObject.argtypes = [wintypes.HDC, wintypes.HGDIOBJ]
gdi32.BitBlt.restype = wintypes.BOOL
gdi32.BitBlt.argtypes = [wintypes.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                         ctypes.c_int, wintypes.HDC, ctypes.c_int, ctypes.c_int,
                         wintypes.DWORD]
gdi32.GetDIBits.restype = ctypes.c_int
gdi32.GetDIBits.argtypes = [wintypes.HDC, wintypes.HBITMAP, wintypes.UINT,
                            wintypes.UINT, ctypes.c_void_p, ctypes.c_void_p,
                            wintypes.UINT]
gdi32.DeleteObject.restype = wintypes.BOOL
gdi32.DeleteObject.argtypes = [wintypes.HGDIOBJ]
gdi32.DeleteDC.restype = wintypes.BOOL
gdi32.DeleteDC.argtypes = [wintypes.HDC]

WNDENUMPROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
user32.GetForegroundWindow.restype = wintypes.HWND
user32.GetForegroundWindow.argtypes = []
user32.GetWindowTextW.restype = ctypes.c_int
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetClassNameW.restype = ctypes.c_int
user32.GetClassNameW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.GetClientRect.restype = wintypes.BOOL
user32.GetClientRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
user32.EnumChildWindows.restype = wintypes.BOOL
user32.EnumChildWindows.argtypes = [wintypes.HWND, WNDENUMPROC, wintypes.LPARAM]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [("biSize", wintypes.DWORD),
                ("biWidth", wintypes.LONG),
                ("biHeight", wintypes.LONG),
                ("biPlanes", wintypes.WORD),
                ("biBitCount", wintypes.WORD),
                ("biCompression", wintypes.DWORD),
                ("biSizeImage", wintypes.DWORD),
                ("biXPelsPerMeter", wintypes.LONG),
                ("biYPelsPerMeter", wintypes.LONG),
                ("biClrUsed", wintypes.DWORD),
                ("biClrImportant", wintypes.DWORD)]


PREAMBLE = (
    "#Requires AutoHotkey v2.0\n"
    "#SingleInstance Off\n"
    "#Warn All, Off\n"
    'FileEncoding "UTF-8-RAW"\n'
    'SendMode "Input"\n'
    "SetWorkingDir A_ScriptDir\n"
    'CoordMode "Mouse", "Screen"\n'
    'CoordMode "Pixel", "Screen"\n'
    'CoordMode "ToolTip", "Screen"\n'
    "SetTitleMatchMode 2\n"
    "DetectHiddenWindows true\n"
)


def ahk_path():
    u64 = os.path.join(BASE, "vendor", "ahk", "AutoHotkey64.exe")
    u32 = os.path.join(BASE, "vendor", "ahk", "AutoHotkey32.exe")
    return u64 if os.path.exists(u64) else u32


def _chunk(tag, data):
    return (struct.pack(">I", len(data)) + tag + data +
            struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff))


def _to_png(bgra, w, h):
    arr = bytearray(bgra)
    r = arr[2::4]
    arr[2::4] = arr[0::4]
    arr[0::4] = r
    del arr[3::4]
    stride = w * 3
    mv = memoryview(arr)
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        raw += mv[y * stride:(y + 1) * stride]
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + _chunk(b"IHDR", ihdr) +
            _chunk(b"IDAT", zlib.compress(bytes(raw))) + _chunk(b"IEND", b""))


def capture():
    w = user32.GetSystemMetrics(0)
    h = user32.GetSystemMetrics(1)
    hdc = user32.GetDC(None)
    mem = None
    hbm = None
    try:
        mem = gdi32.CreateCompatibleDC(hdc)
        hbm = gdi32.CreateCompatibleBitmap(hdc, w, h)
        old = gdi32.SelectObject(mem, hbm)
        gdi32.BitBlt(mem, 0, 0, w, h, hdc, 0, 0, SRCCOPY | CAPTUREBLT)
        gdi32.SelectObject(mem, old)
        bmi = BITMAPINFOHEADER()
        bmi.biSize = ctypes.sizeof(BITMAPINFOHEADER)
        bmi.biWidth = w
        bmi.biHeight = -h  # negative => top-down rows, matches screenshot orientation
        bmi.biPlanes = 1
        bmi.biBitCount = 32
        bmi.biCompression = BI_RGB
        buf = ctypes.create_string_buffer(w * h * 4)
        if gdi32.GetDIBits(mem, hbm, 0, h, buf, ctypes.byref(bmi),
                           DIB_RGB_COLORS) == 0:
            raise RuntimeError("GetDIBits failed")
        data = buf.raw
    finally:
        if hbm:
            gdi32.DeleteObject(hbm)
        if mem:
            gdi32.DeleteDC(mem)
        user32.ReleaseDC(None, hdc)
    return _to_png(data, w, h), w, h


def _win_text(fn, hwnd, n=512):
    buf = ctypes.create_unicode_buffer(n)
    fn(hwnd, buf, n)
    return buf.value


def active_window():
    hwnd = user32.GetForegroundWindow()
    if not hwnd:
        return {"title": "", "class": "", "dialog": False}
    cls = _win_text(user32.GetClassNameW, hwnd)
    return {"title": _win_text(user32.GetWindowTextW, hwnd),
            "class": cls, "dialog": cls == "#32770"}


def control_tree():
    hwnd = user32.GetForegroundWindow()
    rows = []

    def add(h):
        rect = wintypes.RECT()
        user32.GetClientRect(h, ctypes.byref(rect))
        rows.append((_win_text(user32.GetClassNameW, h),
                     _win_text(user32.GetWindowTextW, h),
                     rect.left, rect.top,
                     rect.right - rect.left, rect.bottom - rect.top))

    if hwnd:
        add(hwnd)

        def cb(child, _lparam):
            add(child)
            return True
        user32.EnumChildWindows(hwnd, WNDENUMPROC(cb), 0)
    lines = ["%-24s %-28s %s" % ("class", "text", "l,t,w,h")]
    for cls, text, l, t, w, h in rows:
        lines.append("%-24s %-28s %d,%d,%d,%d" % (cls, text, l, t, w, h))
    return "\n".join(lines)


def _run(argv, timeout_ms, encoding, on_timeout=None):
    def dec(b):
        return b.decode(encoding, "replace") if b else ""

    p = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        out, err = p.communicate(timeout=timeout_ms / 1000.0)
        return p.returncode, dec(out), dec(err), None
    except subprocess.TimeoutExpired:
        if on_timeout:
            on_timeout()
        subprocess.call(["taskkill", "/F", "/T", "/PID", str(p.pid)])
        try:
            # A process the script launched can outlive taskkill still holding the
            # inherited stdout pipe; an unbounded wait here wedges the agent's lock.
            out, err = p.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            out = err = b""
            for pipe in (p.stdout, p.stderr):
                if pipe:
                    pipe.close()
        msg = "timed out after %d ms, process killed" % timeout_ms
        return None, dec(out), dec(err), msg


def run_ahk(script, timeout_ms, on_timeout=None):
    fd, path = tempfile.mkstemp(suffix=".ahk")
    os.close(fd)
    # AutoHotkey (Unicode-only) needs a UTF-8 BOM to read the file as UTF-8.
    with open(path, "wb") as f:
        f.write(b"\xef\xbb\xbf" + (PREAMBLE + script).encode("utf-8"))
    try:
        # /ErrorStdOut sends syntax errors to stderr instead of a blocking modal.
        code, out, err, error = _run([ahk_path(), "/ErrorStdOut", path], timeout_ms,
                                     "utf-8", on_timeout)
    finally:
        os.remove(path)
    if error:
        error = "script " + error
    return code, out, err, error


def _safe_capture(error):
    try:
        png, w, h = capture()
        return base64.b64encode(png).decode("ascii"), w, h, error
    except Exception as e:
        note = "screenshot failed: " + repr(e)
        return "", 0, 0, (error + "; " + note if error else note)


LOG_PATH = os.path.join(BASE, "agent.log")
LOG_LOCK = threading.Lock()


def log(text, stamp=True):
    """Console and agent.log, so a session can be reconstructed after the fact."""
    line = time.strftime("%Y-%m-%d %H:%M:%S ") + text if stamp else text
    with LOG_LOCK:
        print(line, end="" if line.endswith("\n") else "\n", flush=True)
        try:
            with open(LOG_PATH, "a", encoding="utf-8") as f:
                f.write(line if line.endswith("\n") else line + "\n")
        except OSError:
            pass          # a log that cannot be written must not take the agent down


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, obj):
        body = json.dumps(obj).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _log(self, start, code, body=None):
        log("%s %s %dms exit=%s" % (self.command, self.path,
            int((time.time() - start) * 1000), code))
        if body:
            log("".join("    | %s\n" % l for l in body.splitlines()), stamp=False)

    def _auth_ok(self):
        token = os.environ.get("WIN7_TOKEN")
        return not token or self.headers.get("X-Win7-Token") == token

    def _body(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        return json.loads(raw.decode("utf-8")) if raw else {}

    def do_GET(self):
        start = time.time()
        if not self._auth_ok():
            self._send(401, {"error": "bad token"})
        elif self.path == "/health":
            self._send(200, {"ok": True, "w": user32.GetSystemMetrics(0),
                             "h": user32.GetSystemMetrics(1),
                             "hostname": os.environ.get("COMPUTERNAME", ""),
                             "utc_us": int(time.time() * 1000000),
                             "ahk": ahk_path(), "python": sys.executable,
                             "agent_sha256": AGENT_SHA256})
        else:
            self._send(404, {"error": "not found"})
        self._log(start, None)

    def do_POST(self):
        start = time.time()
        code = None
        if not self._auth_ok():
            self._send(401, {"error": "bad token"})
            self._log(start, None)
            return
        try:
            data = self._body()
        except Exception:
            self._send(400, {"error": "bad json"})
            self._log(start, None)
            return
        body = {"/exec": data.get("script"), "/cmd": data.get("command"),
                "/spawn": data.get("command"),
                "/put": data.get("path"), "/get": data.get("path")}.get(self.path)
        try:
            if self.path == "/exec":
                resp = self._exec(data)
                code = resp["exit"]
            elif self.path == "/cmd":
                resp = self._cmd(data)
                code = resp["exit"]
            elif self.path == "/spawn":
                resp = self._spawn(data)
                code = 0
            elif self.path == "/put":
                resp = self._put(data)
            elif self.path == "/get":
                resp = self._get(data)
            elif self.path == "/shot":
                resp = self._shot()
            elif self.path == "/ui":
                resp = self._ui()
            else:
                self._send(404, {"error": "not found"})
                self._log(start, None)
                return
            self._send(200, resp)
        except Exception as e:
            self._send(200, {"error": repr(e), "exit": None, "stdout": "",
                             "stderr": "", "png_b64": "", "w": 0, "h": 0})
        self._log(start, code, body)

    def _exec(self, data):
        pre = []

        def grab():  # capture before taskkill so the blocker is still on screen
            pre.append((_safe_capture(None), active_window()))

        with LOCK:
            code, out, err, error = run_ahk(data.get("script", ""),
                                            data.get("timeout_ms", 60000), grab)
            if pre:
                (b64, w, h, note), win = pre[0]
            else:
                time.sleep(data.get("shot_delay_ms", 500) / 1000.0)
                b64, w, h, note = _safe_capture(None)
                win = active_window()
        if note:
            error = error + "; " + note if error else note
        return {"exit": code, "stdout": out, "stderr": err, "png_b64": b64,
                "w": w, "h": h, "error": error, "win": win}

    def _put(self, data):
        path = data.get("path", "")
        blob = base64.b64decode(data.get("b64", ""))
        parent = os.path.dirname(path)
        if parent and not os.path.isdir(parent):
            os.makedirs(parent)
        with open(path, "wb") as f:
            f.write(blob)
        return {"bytes": len(blob), "path": os.path.abspath(path), "error": None}

    def _get(self, data):
        with open(data.get("path", ""), "rb") as f:
            blob = f.read()
        return {"b64": base64.b64encode(blob).decode("ascii"), "bytes": len(blob),
                "error": None}

    def _cmd(self, data):
        with LOCK:
            # A raw command-line string, not a list: on Windows list2cmdline would
            # backslash-escape the embedded quotes before cmd.exe ever sees them.
            code, out, err, error = _run("cmd.exe /c " + data.get("command", ""),
                                         data.get("timeout_ms", 60000), "oem")
        return {"exit": code, "stdout": out, "stderr": err, "error": error}

    def _spawn(self, data):
        flags = subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.DETACHED_PROCESS
        process = subprocess.Popen(data.get("command", ""), stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                   close_fds=True, creationflags=flags)
        return {"pid": process.pid, "error": None}

    def _shot(self):
        b64, w, h, error = _safe_capture(None)
        win = active_window()
        return {"png_b64": b64, "w": w, "h": h, "error": error, "win": win}

    def _ui(self):
        win = active_window()
        controls = control_tree()
        return {"win": win, "controls": controls, "error": None}


def keep_awake():
    """Hold the display and system awake for as long as this process lives.

    Screen blanking alone is harmless -- BitBlt still captures the composited
    desktop -- but sleep kills the agent outright, and a locked session moves
    the desktop to Winlogon where neither AutoHotkey nor the capture can reach.
    """
    ES_CONTINUOUS, ES_SYSTEM_REQUIRED, ES_DISPLAY_REQUIRED = 0x80000000, 0x1, 0x2
    kernel32 = ctypes.windll.kernel32
    kernel32.SetThreadExecutionState.restype = wintypes.DWORD
    kernel32.SetThreadExecutionState.argtypes = [wintypes.DWORD]
    return kernel32.SetThreadExecutionState(
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)


def main():
    user32.SetProcessDPIAware()
    awake = keep_awake()
    port = int(os.environ.get("WIN7_PORT", "8777"))
    srv = ThreadingHTTPServer(("0.0.0.0", port), Handler)
    log("win7-agent listening on 0.0.0.0:%d  ahk=%s  log=%s%s"
        % (port, ahk_path(), LOG_PATH,
           "" if awake else "  (WARNING: could not inhibit sleep)"))
    srv.serve_forever()


if __name__ == "__main__":
    main()
