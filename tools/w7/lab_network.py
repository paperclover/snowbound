import fcntl
import os
from pathlib import Path
import shutil
import subprocess
import time


def ensure_hub(vm_home):
    root = vm_home / "run" / "lab"
    socket_dir = root / "switch"
    pidfile = root / "vde.pid"
    root.mkdir(parents=True, exist_ok=True)
    with (root / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            pid = int(pidfile.read_text())
            os.kill(pid, 0)
            if (socket_dir / "ctl").exists():
                return socket_dir
        except (FileNotFoundError, ProcessLookupError, ValueError):
            pass
        pidfile.unlink(missing_ok=True)
        shutil.rmtree(socket_dir, ignore_errors=True)
        executable = shutil.which("vde_switch") or "/opt/homebrew/bin/vde_switch"
        if not Path(executable).is_file():
            raise SystemExit("Install the Homebrew vde package for the VM lab network")
        subprocess.run([
            executable, "--daemon", "--nostdin", "--numports", "64",
            "--sock", str(socket_dir), "--pidfile", str(pidfile),
            "--dirmode", "0700", "--mode", "0600",
        ], check=True)
        for _ in range(50):
            if pidfile.exists() and (socket_dir / "ctl").exists():
                return socket_dir
            time.sleep(0.1)
    raise SystemExit("The VM lab network did not start")
