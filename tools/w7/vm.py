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
import struct
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
AGENT_ISO = MEDIA / "win7-agent.iso"
TARGETS = VM_HOME / "targets.json"
# Image stem and build-VM control port. Windows 7 is installed by hand from licensed
# media; Windows 10 (x64) and 11 (arm64) install unattended from windows_media.py ISOs.
BASES = {
    "win7": ("win7-office", 18777),
    "win10": ("win10", 18774),
    "win11": ("win11", 18775),
}
BUILD = "win7-build"
UNATTEND = Path(__file__).with_name("unattend")
FIRMWARE = Path("/opt/homebrew/share/qemu")
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


def build_disk(base):
    return IMAGES / ("%s-build.qcow2" % BASES[base][0])


def base_disk(base):
    return IMAGES / ("%s-base.qcow2" % BASES[base][0])


def base_manifest(base):
    return IMAGES / ("%s-base.json" % BASES[base][0])


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


def uefi_vars(path, template, width=1024, height=768):
    """Write an edk2 variable store whose PlatformConfig sets the GOP resolution.

    Windows keeps the firmware's framebuffer mode, and 1024x768 is the largest mode
    edk2 offers for ramfb on arm64."""
    image = bytearray(template.read_bytes())
    offset = struct.unpack_from("<H", image, 0x30)[0] + 28
    while struct.unpack_from("<H", image, offset)[0] == 0x55AA:
        name_size, data_size = struct.unpack_from("<II", image, offset + 36)
        offset = (offset + 60 + name_size + data_size + 3) & ~3
    name = "PlatformConfig\0".encode("utf-16-le")
    data = struct.pack("<II", width, height)
    guid = uuid.UUID("7235c51c-0c80-4cab-87ac-3b084a6304b1").bytes_le
    record = struct.pack("<HBBIQ16sIII16s", 0x55AA, 0x3F, 0, 7, 0, bytes(16), 0,
                         len(name), len(data), guid) + name + data
    image[offset:offset + len(record)] = record
    path.write_bytes(image)


def machine(base, disk):
    if base == "win7":
        return [qemu("qemu-system-x86_64"), "-machine", "pc", "-accel", "tcg,thread=multi",
                "-cpu", os.environ.get("ONE_VM_CPU", "qemu64"),
                "-vga", "std", "-usb", "-device", "usb-tablet",
                "-drive", "file=%s,if=ide,format=qcow2,cache=writeback" % disk], "e1000"
    variables = disk.with_suffix(".vars.fd")
    if base == "win10":
        if not variables.exists():
            uefi_vars(variables, FIRMWARE / "edk2-i386-vars.fd")
        return [qemu("qemu-system-x86_64"), "-machine", "q35", "-accel", "tcg,thread=multi",
                "-cpu", "max",
                "-drive", "if=pflash,format=raw,readonly=on,file=%s" % (FIRMWARE / "edk2-x86_64-code.fd"),
                "-drive", "if=pflash,format=raw,file=%s" % variables,
                "-vga", "std", "-usb", "-device", "usb-tablet",
                "-drive", "file=%s,if=none,id=disk,format=qcow2,cache=writeback" % disk,
                "-device", "ide-hd,drive=disk,bus=ide.0,bootindex=0"], "e1000"
    if not variables.exists():
        uefi_vars(variables, FIRMWARE / "edk2-arm-vars.fd")
    # Windows on Arm has inbox NVMe and xHCI drivers; lab-setup.cmd adds NetKVM.
    return [qemu("qemu-system-aarch64"), "-machine", "virt", "-accel", "hvf", "-cpu", "host",
            "-drive", "if=pflash,format=raw,readonly=on,file=%s" % (FIRMWARE / "edk2-aarch64-code.fd"),
            "-drive", "if=pflash,format=raw,file=%s" % variables,
            "-device", "ramfb", "-device", "qemu-xhci",
            "-device", "usb-kbd", "-device", "usb-tablet",
            "-drive", "file=%s,if=none,id=disk,format=qcow2,cache=writeback" % disk,
            "-device", "nvme,drive=disk,serial=one,bootindex=0"], "virtio-net-pci"


def cdroms(base, images):
    """Attach read-only discs. UEFI guests try the disk first, so an installer CD
    boots only until Windows has made the disk bootable."""
    drives = []
    for index, image in enumerate(images, 1):
        drive = "file=%s,file.locking=off,media=cdrom,readonly=on" % image
        if base == "win7":
            drives += ["-drive", drive + ",if=ide"]
            continue
        drives += ["-drive", drive + ",if=none,id=cd%d" % index, "-device"]
        if base == "win10":
            drives.append("ide-cd,drive=cd%d,bus=ide.%d,bootindex=%d" % (index, index, index))
        else:
            drives.append("usb-storage,drive=cd%d,bootindex=%d" % (index, index))
    return drives


def launch(name, base, disk, port, mac, cpus, memory_mb, display, images, answers=None):
    require_vm_home()
    if running(name):
        raise SystemExit("Windows is already running: %s" % name)
    root, qmp_socket, pid, log_path, _shot = runtime(name)
    root.mkdir(parents=True, exist_ok=True)
    qmp_socket.unlink(missing_ok=True)
    lab_socket = ensure_hub(VM_HOME)
    command, nic = machine(base, disk)
    command += [
        # crash.py matches this name to confirm a process belongs to the clone.
        "-name", "OneNote Windows 7 " + name,
        "-smp", str(cpus),
        "-m", str(memory_mb),
        "-display", display,
        "-netdev", "user,id=control,hostfwd=tcp:127.0.0.1:%d-:8777" % port,
        "-device", "%s,netdev=control,mac=%s" % (nic, mac),
        "-netdev", "vde,id=lab,sock=%s" % lab_socket,
        "-device", "%s,netdev=lab,mac=%s" % (nic, lab_mac(name)),
        "-uuid", uuid_for(name),
        "-rtc", "base=localtime,clock=host,driftfix=slew",
        "-qmp", "unix:%s,server=on,wait=off" % qmp_socket,
        "-pidfile", str(pid),
    ] + cdroms(base, images)
    if answers:
        # Setup reads autounattend.xml from the root of a removable drive.
        command += ["-drive", "file=fat:%s,format=raw,if=none,id=answers,readonly=on" % answers,
                    "-device", "usb-storage,drive=answers,removable=on"]
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


def stage_answers(base):
    """Copy unattend/ into the build's runtime directory with this base filled in."""
    root = runtime(base + "-build")[0] / "answers"
    shutil.rmtree(root, ignore_errors=True)
    shutil.copytree(UNATTEND, root)
    answers = root / "autounattend.xml"
    text = answers.read_text().replace("{arch}", "arm64" if base == "win11" else "amd64")
    answers.write_text(text.replace("{hostname}", "ONE-" + base.upper()))
    if base == "win11":
        shutil.copytree(MEDIA / "netkvm-arm64", root / "netkvm")
    return root


def start_build(base, install, display):
    disk = build_disk(base)
    iso = ISO if base == "win7" else MEDIA / ("%s.iso" % base)
    if install and not iso.is_file():
        raise SystemExit("Windows ISO not found: %s (run ./windows_media.py %s)" % (iso, base))
    if not disk.exists():
        if not install:
            raise SystemExit("No Windows disk found. Run: ./vm.py install --base %s" % base)
        IMAGES.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            [qemu("qemu-img"), "create", "-f", "qcow2", str(disk), "64G"],
            check=True,
        )
    images, answers = [], None
    if install:
        images.append(iso)
        if base != "win7":
            build_agent_iso()
            images.append(AGENT_ISO)
            answers = stage_answers(base)
    elif AGENT_ISO.exists():
        images.append(AGENT_ISO)
    port = BASES[base][1]
    # The Windows 7 build is finished by hand, so it opens a window by default.
    launch(base + "-build", base, disk, port, mac_for(port), 4, 4096,
           "cocoa" if display or base == "win7" else "none", images, answers)


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
    used = {port for _stem, port in BASES.values()}
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


def create_instance(name, hostname=None, cpus=2, memory_mb=4096, port=None, base="win7"):
    require_vm_home()
    validate_name(name)
    if name == "local" or name in ("%s-build" % b for b in BASES):
        raise SystemExit("VM name is reserved: %s" % name)
    hostname = (hostname or ("ONE-" + name)).upper()
    if not HOSTNAME.fullmatch(hostname):
        raise SystemExit("Hostname must be 1-15 letters, digits, or hyphens")
    if not 1 <= cpus <= 16:
        raise SystemExit("CPU count must be between 1 and 16")
    if not 1024 <= memory_mb <= 65536:
        raise SystemExit("Memory must be between 1024 and 65536 MiB")
    if not base_disk(base).is_file() or not base_manifest(base).is_file():
        raise SystemExit("No sealed %s base image. Finish the build, then run: "
                         "./vm.py seal --base %s" % (base, base))
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
                "-b", str(base_disk(base)), str(temporary),
            ], check=True)
            make_instance_iso(instance_iso, hostname, token)
            instance_iso.chmod(0o600)
            temporary.replace(overlay)
            config = {"base": base, "cpus": cpus, "hostname": hostname,
                      "memory_mb": memory_mb, "port": port, "token": token}
            atomic_json(instance_path(name), config)
            instance_path(name).chmod(0o600)
            update_target(name, port, token)
        except Exception:
            temporary.unlink(missing_ok=True)
            instance_iso.unlink(missing_ok=True)
            overlay.unlink(missing_ok=True)
            instance_path(name).unlink(missing_ok=True)
            raise
    print(json.dumps({"base": base, "cpus": cpus, "hostname": hostname,
                      "memory_mb": memory_mb, "name": name, "port": port}, sort_keys=True))


def start_instance(name, display=False):
    config = load_instance(name)
    overlay = IMAGES / "instances" / (name + ".qcow2")
    instance_iso = MEDIA / "instances" / (name + ".iso")
    if not overlay.is_file() or not instance_iso.is_file():
        raise SystemExit("VM artifacts are incomplete: %s" % name)
    images = [image for image in (AGENT_ISO, instance_iso) if image.is_file()]
    launch(name, config.get("base", "win7"), overlay, config["port"], mac_for(config["port"]),
           config["cpus"], config["memory_mb"], "cocoa" if display else "none", images)


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
        (IMAGES / "instances" / (name + ".vars.fd")).unlink(missing_ok=True)
        (MEDIA / "instances" / (name + ".iso")).unlink(missing_ok=True)
        instance_path(name).unlink()
        shutil.rmtree(runtime(name)[0], ignore_errors=True)
        update_target(name)
    print("Deleted VM: %s" % name)


def seal(base="win7"):
    require_vm_home()
    disk, sealed, manifest = build_disk(base), base_disk(base), base_manifest(base)
    if running(base + "-build"):
        raise SystemExit("Shut down Windows before sealing the base image")
    if not disk.exists():
        raise SystemExit("No Windows build disk found")
    temporary = sealed.with_suffix(".tmp.qcow2")
    if sealed.exists() or temporary.exists():
        raise SystemExit("Move the existing base image before sealing another")
    subprocess.run([qemu("qemu-img"), "check", str(disk)], check=True)
    subprocess.run([qemu("qemu-img"), "convert", "-p", "-O", "qcow2",
                    "-o", "lazy_refcounts=off", str(disk), str(temporary)], check=True)
    subprocess.run([qemu("qemu-img"), "check", str(temporary)], check=True)
    temporary.replace(sealed)
    digest = hashlib.sha256()
    with sealed.open("rb") as image:
        while chunk := image.read(8 * 1024 * 1024):
            digest.update(chunk)
    info = json.loads(subprocess.check_output([
        qemu("qemu-img"), "info", "--output=json", str(sealed),
    ]))
    atomic_json(manifest, {"file": sealed.name, "format": info["format"],
                           "sha256": digest.hexdigest(),
                           "virtual_size": info["virtual-size"]})
    sealed.chmod(0o444)
    print(manifest)


def fetch_base(manifest_url, base="win7"):
    require_vm_home()
    sealed, manifest_path = base_disk(base), base_manifest(base)
    if sealed.exists() or manifest_path.exists():
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
    temporary = sealed.with_suffix(".download.qcow2")
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
        temporary.replace(sealed)
        sealed.chmod(0o444)
        atomic_json(manifest_path, dict(manifest, file=sealed.name))
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    print(manifest_path)


def list_instances():
    rows = configs()
    if not rows:
        print("No Windows VMs.")
        return
    for name, config in rows:
        state = "running" if running(name) else "stopped"
        print("%-11s %-5s %-15s %-7s http://127.0.0.1:%d" %
              (name, config.get("base", "win7"), config["hostname"], state, config["port"]))


def main():
    parser = argparse.ArgumentParser(description="Run OneNote Windows lab VMs")
    commands = parser.add_subparsers(dest="command", required=True)
    base_option = {"choices": sorted(BASES), "default": "win7"}
    for verb in ("install", "run"):
        build = commands.add_parser(verb)
        build.add_argument("--base", **base_option)
        build.add_argument("--display", action="store_true")
    status = commands.add_parser("status")
    status.add_argument("name", nargs="?")
    power = commands.add_parser("poweroff")
    power.add_argument("name", nargs="?", default=BUILD)
    shot = commands.add_parser("screenshot")
    shot.add_argument("name", nargs="?", default=BUILD)
    commands.add_parser("seal").add_argument("--base", **base_option)
    commands.add_parser("media")
    up = commands.add_parser("up")
    up.add_argument("name")
    up.add_argument("--base", **base_option)
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
    fetch.add_argument("--base", **base_option)
    args = parser.parse_args()
    if args.command in ("install", "run"):
        start_build(args.base, args.command == "install", args.display)
    elif args.command == "status":
        if args.name:
            if args.name in ("%s-build" % b for b in BASES):
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
        seal(args.base)
    elif args.command == "media":
        build_agent_iso()
    elif args.command == "up":
        if not instance_path(args.name).exists():
            create_instance(args.name, args.hostname, args.cpus, args.memory_mb, args.port,
                            args.base)
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
        fetch_base(args.manifest_url, args.base)


if __name__ == "__main__":
    main()
