#!/usr/bin/env python3

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.parse
import urllib.error
import urllib.request
import uuid

from lab_network import ensure_hub


from env import ROOT, require, setting


VM_HOME = Path(setting("ONE_VM_HOME") or ROOT / "lab-unset").expanduser()
ISO = Path(setting("ONE_WIN7_ISO") or ROOT / "lab-unset/win7.iso")
IMAGES = VM_HOME / "images"
MEDIA = VM_HOME / "media"
INSTANCES = VM_HOME / "instances"
RUN = VM_HOME / "run"
DISK = IMAGES / "win7-office-build.qcow2"
BASE_DISK = IMAGES / "win7-office-base.qcow2"
BASE_MANIFEST = IMAGES / "win7-office-base.json"
AGENT_ISO = MEDIA / "win7-agent.iso"
TARGETS = VM_HOME / "targets.json"
BUILD = "win7-build"
NAME = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,9}[a-z0-9])?")
HOSTNAME = re.compile(r"[A-Z0-9](?:[A-Z0-9-]{0,13}[A-Z0-9])?")


def qemu(name):
    path = shutil.which(name) or "/opt/homebrew/bin/" + name
    if not Path(path).is_file():
        raise SystemExit("Install QEMU with: /opt/homebrew/bin/brew install qemu")
    return path


def require_vm_home():
    if VM_HOME == ROOT / "lab-unset":
        require("ONE_VM_HOME")
    if len(VM_HOME.parts) > 2 and VM_HOME.parts[1] == "Volumes":
        volume = Path("/Volumes") / VM_HOME.parts[2]
        if not os.path.ismount(volume):
            raise SystemExit("VM volume is not mounted: %s" % volume)


def runtime(name):
    root = RUN / name
    return root, root / "qmp.sock", root / "qemu.pid", root / "qemu.log", root / "screen.png"


def running(name=BUILD):
    try:
        os.kill(int(runtime(name)[2].read_text()), 0)
        return True
    except (FileNotFoundError, ProcessLookupError, ValueError):
        return False


def qmp(name, command, arguments=None):
    qmp_socket = runtime(name)[1]
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        client.connect(str(qmp_socket))
        stream = client.makefile("rwb", buffering=0)
        stream.readline()
        stream.write(b'{"execute":"qmp_capabilities"}\n')
        while "return" not in json.loads(stream.readline()):
            pass
        request = {"execute": command}
        if arguments:
            request["arguments"] = arguments
        stream.write((json.dumps(request) + "\n").encode())
        while True:
            response = json.loads(stream.readline())
            if "return" in response:
                return response["return"]
            if "error" in response:
                raise SystemExit(response["error"]["desc"])


def launch(name, disk, port, mac, cpus, memory_mb, display, drives):
    require_vm_home()
    if running(name):
        raise SystemExit("Windows is already running: %s" % name)
    root, qmp_socket, pid, log_path, _shot = runtime(name)
    root.mkdir(parents=True, exist_ok=True)
    qmp_socket.unlink(missing_ok=True)
    lab_socket = ensure_hub(VM_HOME)
    command = [
        qemu("qemu-system-x86_64"),
        "-name", "OneNote Windows 7 " + name,
        "-machine", "pc",
        "-accel", "tcg,thread=multi",
        "-cpu", "qemu64",
        "-smp", str(cpus),
        "-m", str(memory_mb),
        "-drive", "file=%s,if=ide,format=qcow2,cache=writeback" % disk,
        "-vga", "std",
        "-display", display,
        "-usb",
        "-device", "usb-tablet",
        "-netdev", "user,id=control,hostfwd=tcp:127.0.0.1:%d-:8777" % port,
        "-device", "e1000,netdev=control,mac=%s" % mac,
        "-netdev", "vde,id=lab,sock=%s" % lab_socket,
        "-device", "e1000,netdev=lab,mac=%s" % lab_mac(name),
        "-uuid", uuid_for(name),
        "-rtc", "base=localtime,clock=host,driftfix=slew",
        "-qmp", "unix:%s,server=on,wait=off" % qmp_socket,
        "-pidfile", str(pid),
    ]
    for drive in drives:
        command += ["-drive", drive]
    with log_path.open("ab") as log:
        subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=log,
            start_new_session=True,
        )
    for _ in range(50):
        if running(name):
            print("Windows opened: %s" % name)
            return
        time.sleep(0.1)
    raise SystemExit("Windows did not open. Check: %s" % log_path)


def start_build(install):
    if install and not ISO.is_file():
        raise SystemExit("Windows ISO not found: %s" % ISO)
    if not DISK.exists():
        if not install:
            raise SystemExit("No Windows disk found. Run: ./vm.py install")
        IMAGES.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            [qemu("qemu-img"), "create", "-f", "qcow2", str(DISK), "64G"],
            check=True,
        )
    drives = []
    if install:
        drives.append("file=%s,file.locking=off,if=ide,media=cdrom,readonly=on" % ISO)
    elif AGENT_ISO.exists():
        drives.append("file=%s,file.locking=off,if=ide,media=cdrom,readonly=on" % AGENT_ISO)
    launch(BUILD, DISK, 18777, "52:54:00:10:77:01", 4, 4096, "cocoa", drives)


def instance_path(name):
    return INSTANCES / (name + ".json")


def validate_name(name):
    if not NAME.fullmatch(name):
        raise SystemExit("VM name must be 1-11 lowercase letters, digits, or hyphens")


def load_instance(name):
    validate_name(name)
    try:
        return json.loads(instance_path(name).read_text())
    except FileNotFoundError:
        raise SystemExit("VM does not exist: %s" % name)
    except (OSError, ValueError) as e:
        raise SystemExit("Cannot read VM %s: %s" % (name, e))


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def configs():
    if not INSTANCES.exists():
        return []
    rows = []
    for path in sorted(INSTANCES.glob("*.json")):
        try:
            rows.append((path.stem, json.loads(path.read_text())))
        except (OSError, ValueError) as e:
            raise SystemExit("Cannot read VM manifest %s: %s" % (path, e))
    return rows


def available_port(requested=None):
    used = {18777}
    used.update(config["port"] for _name, config in configs())
    candidates = [requested] if requested else range(18778, 18878)
    for port in candidates:
        if port is None or port < 1024 or port > 65535 or port in used:
            continue
        with socket.socket() as probe:
            try:
                probe.bind(("127.0.0.1", port))
            except OSError:
                continue
        return port
    raise SystemExit("No unused control port is available")


def mac_for(port):
    return "52:54:00:01:%02x:%02x" % (port >> 8, port & 0xff)


def uuid_for(name):
    return str(uuid.uuid5(uuid.NAMESPACE_URL, "onenote-vm:" + name))


def lab_mac(name):
    tail = uuid.UUID(uuid_for(name)).bytes[-3:]
    return "52:54:02:%02x:%02x:%02x" % tuple(tail)


def make_instance_iso(path, hostname, token):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="one-vm-") as temporary:
        root = Path(temporary)
        (root / "onenote-vm.ini").write_text(
            "HOSTNAME=%s\r\nTOKEN=%s\r\n" % (hostname, token)
        )
        subprocess.run([
            "hdiutil", "makehybrid", "-quiet", "-iso", "-joliet",
            "-default-volume-name", "ONEVM", "-o", str(path), str(root),
        ], check=True)


def build_agent_iso():
    require_vm_home()
    MEDIA.mkdir(parents=True, exist_ok=True)
    temporary = AGENT_ISO.with_suffix(".tmp.iso")
    temporary.unlink(missing_ok=True)
    try:
        subprocess.run([
            "hdiutil", "makehybrid", "-quiet", "-iso", "-joliet",
            "-default-volume-name", "WIN7_AGENT", "-o", str(temporary),
            str(Path(__file__).with_name("payload")),
        ], check=True)
        temporary.replace(AGENT_ISO)
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    print(AGENT_ISO)


def update_target(name, port=None, token=None):
    try:
        targets = json.loads(TARGETS.read_text())
    except FileNotFoundError:
        targets = {}
    except (OSError, ValueError) as e:
        raise SystemExit("Cannot read %s: %s" % (TARGETS, e))
    if port is None:
        targets.pop(name, None)
    else:
        targets[name] = {"base": "http://127.0.0.1:%d" % port, "token": token}
    atomic_json(TARGETS, targets)
    TARGETS.chmod(0o600)


def create_instance(name, hostname=None, cpus=2, memory_mb=4096, port=None):
    require_vm_home()
    validate_name(name)
    if name in ("local", BUILD):
        raise SystemExit("VM name is reserved: %s" % name)
    hostname = (hostname or ("ONE-" + name)).upper()
    if not HOSTNAME.fullmatch(hostname):
        raise SystemExit("Hostname must be 1-15 letters, digits, or hyphens")
    if not 1 <= cpus <= 16:
        raise SystemExit("CPU count must be between 1 and 16")
    if not 1024 <= memory_mb <= 65536:
        raise SystemExit("Memory must be between 1024 and 65536 MiB")
    if not BASE_DISK.is_file() or not BASE_MANIFEST.is_file():
        raise SystemExit("No sealed base image. Finish the build, then run: ./vm.py seal")
    VM_HOME.mkdir(parents=True, exist_ok=True)
    with (VM_HOME / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if instance_path(name).exists():
            raise SystemExit("VM already exists: %s" % name)
        if any(c.get("hostname", "").upper() == hostname.upper() for _n, c in configs()):
            raise SystemExit("Hostname is already in use: %s" % hostname)
        port = available_port(port)
        token = secrets.token_urlsafe(24)
        overlay = IMAGES / "instances" / (name + ".qcow2")
        instance_iso = MEDIA / "instances" / (name + ".iso")
        if overlay.exists() or instance_iso.exists():
            raise SystemExit("VM artifacts already exist without a manifest: %s" % name)
        overlay.parent.mkdir(parents=True, exist_ok=True)
        temporary = overlay.with_suffix(".tmp.qcow2")
        try:
            subprocess.run([
                qemu("qemu-img"), "create", "-f", "qcow2", "-F", "qcow2",
                "-b", str(BASE_DISK), str(temporary),
            ], check=True)
            make_instance_iso(instance_iso, hostname, token)
            instance_iso.chmod(0o600)
            temporary.replace(overlay)
            config = {"cpus": cpus, "hostname": hostname, "memory_mb": memory_mb,
                      "port": port, "token": token}
            atomic_json(instance_path(name), config)
            instance_path(name).chmod(0o600)
            update_target(name, port, token)
        except Exception:
            temporary.unlink(missing_ok=True)
            instance_iso.unlink(missing_ok=True)
            overlay.unlink(missing_ok=True)
            instance_path(name).unlink(missing_ok=True)
            raise
    print(json.dumps({"cpus": cpus, "hostname": hostname, "memory_mb": memory_mb,
                      "name": name, "port": port}, sort_keys=True))


def start_instance(name, display=False):
    config = load_instance(name)
    overlay = IMAGES / "instances" / (name + ".qcow2")
    instance_iso = MEDIA / "instances" / (name + ".iso")
    if not overlay.is_file() or not instance_iso.is_file():
        raise SystemExit("VM artifacts are incomplete: %s" % name)
    drives = []
    for drive in (AGENT_ISO, instance_iso):
        if drive.is_file():
            drives.append("file=%s,file.locking=off,if=ide,media=cdrom,readonly=on" % drive)
    launch(name, overlay, config["port"], mac_for(config["port"]), config["cpus"],
           config["memory_mb"], "cocoa" if display else "none", drives)


def wait_instance(name, timeout):
    config = load_instance(name)
    deadline = time.monotonic() + timeout
    request = urllib.request.Request(
        "http://127.0.0.1:%d/health" % config["port"],
        headers={"X-Win7-Token": config["token"]},
    )
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(request, timeout=2) as response:
                health = json.load(response)
            if health.get("hostname", "").upper() == config["hostname"]:
                print("Ready: %s (%s)" % (name, config["hostname"]))
                return
        except (urllib.error.URLError, OSError, ValueError):
            pass
        time.sleep(1)
    raise SystemExit("VM did not become ready within %d seconds: %s" % (timeout, name))


def poweroff(name=BUILD):
    if not running(name):
        raise SystemExit("Windows is not running: %s" % name)
    qmp(name, "system_powerdown")
    print("Windows is shutting down: %s" % name)


def shutdown(name, timeout):
    poweroff(name)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not running(name):
            print("Windows stopped: %s" % name)
            return
        time.sleep(1)
    raise SystemExit("Windows did not stop within %d seconds: %s" % (timeout, name))


def screenshot(name=BUILD):
    if not running(name):
        raise SystemExit("Windows is not running: %s" % name)
    shot = runtime(name)[4]
    qmp(name, "screendump", {"filename": str(shot), "format": "png"})
    print(shot)


def delete_instance(name):
    require_vm_home()
    with (VM_HOME / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        load_instance(name)
        if running(name):
            raise SystemExit("Shut down Windows before deleting: %s" % name)
        (IMAGES / "instances" / (name + ".qcow2")).unlink(missing_ok=True)
        (MEDIA / "instances" / (name + ".iso")).unlink(missing_ok=True)
        instance_path(name).unlink()
        shutil.rmtree(runtime(name)[0], ignore_errors=True)
        update_target(name)
    print("Deleted VM: %s" % name)


def seal():
    require_vm_home()
    if running(BUILD):
        raise SystemExit("Shut down Windows before sealing the base image")
    if not DISK.exists():
        raise SystemExit("No Windows build disk found")
    temporary = BASE_DISK.with_suffix(".tmp.qcow2")
    if BASE_DISK.exists() or temporary.exists():
        raise SystemExit("Move the existing base image before sealing another")
    subprocess.run([qemu("qemu-img"), "check", str(DISK)], check=True)
    subprocess.run([qemu("qemu-img"), "convert", "-p", "-O", "qcow2",
                    "-o", "lazy_refcounts=off", str(DISK), str(temporary)], check=True)
    subprocess.run([qemu("qemu-img"), "check", str(temporary)], check=True)
    temporary.replace(BASE_DISK)
    digest = hashlib.sha256()
    with BASE_DISK.open("rb") as image:
        while chunk := image.read(8 * 1024 * 1024):
            digest.update(chunk)
    info = json.loads(subprocess.check_output([
        qemu("qemu-img"), "info", "--output=json", str(BASE_DISK),
    ]))
    atomic_json(BASE_MANIFEST, {"file": BASE_DISK.name, "format": info["format"],
                               "sha256": digest.hexdigest(),
                               "virtual_size": info["virtual-size"]})
    BASE_DISK.chmod(0o444)
    print(BASE_MANIFEST)


def fetch_base(manifest_url):
    require_vm_home()
    if BASE_DISK.exists() or BASE_MANIFEST.exists():
        raise SystemExit("Move the existing base image before fetching another")
    headers = {}
    if os.environ.get("ONE_VM_AUTHORIZATION"):
        headers["Authorization"] = os.environ["ONE_VM_AUTHORIZATION"]
    with urllib.request.urlopen(urllib.request.Request(manifest_url, headers=headers)) as response:
        manifest = json.load(response)
    expected = manifest.get("sha256", "").lower()
    if not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise SystemExit("Base manifest has no valid SHA-256")
    image_url = urllib.parse.urljoin(manifest_url, manifest.get("file", ""))
    IMAGES.mkdir(parents=True, exist_ok=True)
    temporary = BASE_DISK.with_suffix(".download.qcow2")
    digest = hashlib.sha256()
    try:
        with urllib.request.urlopen(urllib.request.Request(image_url, headers=headers)) as response:
            with temporary.open("wb") as output:
                while chunk := response.read(8 * 1024 * 1024):
                    output.write(chunk)
                    digest.update(chunk)
        if digest.hexdigest() != expected:
            temporary.unlink(missing_ok=True)
            raise SystemExit("Downloaded base image failed SHA-256 verification")
        subprocess.run([qemu("qemu-img"), "check", str(temporary)], check=True)
        temporary.replace(BASE_DISK)
        BASE_DISK.chmod(0o444)
        atomic_json(BASE_MANIFEST, dict(manifest, file=BASE_DISK.name))
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    print(BASE_MANIFEST)


def list_instances():
    rows = configs()
    if not rows:
        print("No Windows VMs.")
        return
    for name, config in rows:
        state = "running" if running(name) else "stopped"
        print("%-11s %-15s %-7s http://127.0.0.1:%d" %
              (name, config["hostname"], state, config["port"]))


def main():
    parser = argparse.ArgumentParser(description="Run OneNote Windows 7 VMs")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("install")
    commands.add_parser("run")
    status = commands.add_parser("status")
    status.add_argument("name", nargs="?")
    power = commands.add_parser("poweroff")
    power.add_argument("name", nargs="?", default=BUILD)
    shot = commands.add_parser("screenshot")
    shot.add_argument("name", nargs="?", default=BUILD)
    commands.add_parser("seal")
    commands.add_parser("media")
    up = commands.add_parser("up")
    up.add_argument("name")
    up.add_argument("--hostname")
    up.add_argument("--cpus", type=int, default=2)
    up.add_argument("--memory", type=int, default=4096, dest="memory_mb")
    up.add_argument("--port", type=int)
    up.add_argument("--display", action="store_true")
    up.add_argument("--wait", action="store_true")
    up.add_argument("--timeout", type=int, default=300)
    down = commands.add_parser("down")
    down.add_argument("name")
    down.add_argument("--timeout", type=int, default=60)
    down.add_argument("--preserve-machine", action="store_true")
    fetch = commands.add_parser("fetch")
    fetch.add_argument("manifest_url")
    args = parser.parse_args()
    if args.command == "install":
        start_build(True)
    elif args.command == "run":
        start_build(False)
    elif args.command == "status":
        if args.name:
            if args.name == BUILD:
                print("running" if running(args.name) else "stopped")
            elif not instance_path(args.name).exists():
                print("absent")
            else:
                load_instance(args.name)
                print("running" if running(args.name) else "stopped")
        else:
            list_instances()
    elif args.command == "poweroff":
        poweroff(args.name)
    elif args.command == "screenshot":
        screenshot(args.name)
    elif args.command == "seal":
        seal()
    elif args.command == "media":
        build_agent_iso()
    elif args.command == "up":
        if not instance_path(args.name).exists():
            create_instance(args.name, args.hostname, args.cpus, args.memory_mb, args.port)
        if running(args.name):
            print("Windows already running: %s" % args.name)
        else:
            start_instance(args.name, args.display)
        if args.wait:
            wait_instance(args.name, args.timeout)
    elif args.command == "down":
        if not instance_path(args.name).exists():
            print("VM absent: %s" % args.name)
        else:
            load_instance(args.name)
            if running(args.name):
                shutdown(args.name, args.timeout)
            if args.preserve_machine:
                print("Preserved VM: %s" % args.name)
            else:
                delete_instance(args.name)
    else:
        fetch_base(args.manifest_url)


if __name__ == "__main__":
    main()
