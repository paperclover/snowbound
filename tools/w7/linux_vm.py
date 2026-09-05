#!/usr/bin/env python3

import argparse
import base64
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request
import uuid

from lab_network import ensure_hub


VM_HOME = Path(os.environ.get(
    "ONE_VM_HOME", "/Volumes/Documents/OneNote VMs"
)).expanduser()
LINUX_HOME = VM_HOME / "linux"
IMAGES = LINUX_HOME / "images"
INSTANCES = LINUX_HOME / "instances"
RUN = LINUX_HOME / "run"
BASE = IMAGES / "debian-13-genericcloud-arm64.qcow2"
BASE_MANIFEST = IMAGES / "debian-13-genericcloud-arm64.json"
IMAGE_URL = os.environ.get(
    "LINUX_VM_IMAGE_URL",
    "https://cloud.debian.org/images/cloud/trixie/latest/"
    "debian-13-genericcloud-arm64.qcow2",
)
FIRMWARE = Path(os.environ.get(
    "LINUX_VM_FIRMWARE", "/opt/homebrew/share/qemu/edk2-aarch64-code.fd"
))
LAB_ADDRESS = "192.168.77.1"
NAME = re.compile(r"[a-z0-9](?:[a-z0-9-]{0,22}[a-z0-9])?")


def require_vm_home():
    if len(VM_HOME.parts) > 2 and VM_HOME.parts[1] == "Volumes":
        volume = Path("/Volumes") / VM_HOME.parts[2]
        if not os.path.ismount(volume):
            raise SystemExit("VM volume is not mounted: %s" % volume)


def executable(name):
    path = shutil.which(name) or "/opt/homebrew/bin/" + name
    if not Path(path).is_file():
        raise SystemExit("Missing executable: %s" % name)
    return path


def atomic_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temporary.replace(path)


def runtime(name):
    root = RUN / name
    return root, root / "qmp.sock", root / "qemu.pid", root / "qemu.log"


def running(name):
    try:
        os.kill(int(runtime(name)[2].read_text()), 0)
        return True
    except (FileNotFoundError, ProcessLookupError, ValueError):
        return False


def instance_path(name):
    return INSTANCES / (name + ".json")


def validate_name(name):
    if not NAME.fullmatch(name):
        raise SystemExit("VM name must be 1-24 lowercase letters, digits, or hyphens")


def load_instance(name):
    validate_name(name)
    try:
        return json.loads(instance_path(name).read_text())
    except FileNotFoundError:
        raise SystemExit("Linux VM does not exist: %s" % name)
    except (OSError, ValueError) as e:
        raise SystemExit("Cannot read Linux VM %s: %s" % (name, e))


def configs():
    if not INSTANCES.exists():
        return []
    return [(path.stem, json.loads(path.read_text()))
            for path in sorted(INSTANCES.glob("*.json"))]


def available_port(field, start, end, requested=None):
    used = {config[field] for _name, config in configs()}
    candidates = [requested] if requested else range(start, end + 1)
    for port in candidates:
        if port is None or port < 1024 or port > 65535 or port in used:
            continue
        with socket.socket() as probe:
            try:
                probe.bind(("127.0.0.1", port))
            except OSError:
                continue
        return port
    raise SystemExit("No unused %s is available" % field.replace("_", " "))


def mac(prefix, name):
    tail = uuid.uuid5(uuid.NAMESPACE_URL, prefix + name).bytes[-3:]
    kind = 4 if prefix == "lab:" else 3
    return "52:54:%02x:%02x:%02x:%02x" % (kind, *tail)


def key_path(name):
    return INSTANCES / (name + ".key")


def seed_path(name):
    return INSTANCES / (name + "-seed.iso")


def overlay_path(name):
    return INSTANCES / (name + ".qcow2")


def make_seed(path, hostname, public_key, wan_mac, lab_mac):
    samba = """[global]
workgroup = WORKGROUP
server role = standalone server
map to guest = Bad User
server min protocol = SMB2_02
interfaces = lo wan0 lab0
bind interfaces only = yes

[agent]
path = /srv/agent
read only = no
guest ok = yes
force user = agent
create mask = 0666
directory mask = 0777
"""
    dnsmasq = """port=0
interface=lab0
bind-interfaces
dhcp-range=192.168.77.10,192.168.77.250,255.255.255.0,12h
dhcp-option=3
dhcp-option=6
"""
    user_data = """#cloud-config
hostname: {hostname}
manage_etc_hosts: true
users:
  - name: agent
    gecos: OneNote test agent
    groups: [sudo]
    shell: /bin/bash
    sudo: ALL=(ALL) NOPASSWD:ALL
    lock_passwd: true
    ssh_authorized_keys:
      - {public_key}
ssh_pwauth: false
disable_root: true
package_update: true
packages: [samba, dnsmasq, cifs-utils, smbclient, fio]
write_files:
  - path: /etc/samba/smb.conf
    permissions: '0644'
    encoding: b64
    content: {samba}
  - path: /etc/dnsmasq.d/onenote-lab.conf
    permissions: '0644'
    encoding: b64
    content: {dnsmasq}
runcmd:
  - [mkdir, -p, /srv/agent]
  - [chown, agent:agent, /srv/agent]
  - [systemctl, enable, --now, dnsmasq]
  - [systemctl, enable, --now, smbd]
""".format(
        hostname=hostname,
        public_key=public_key,
        samba=base64.b64encode(samba.encode()).decode(),
        dnsmasq=base64.b64encode(dnsmasq.encode()).decode(),
    )
    network = """version: 2
ethernets:
  wan0:
    match:
      macaddress: "{wan_mac}"
    set-name: wan0
    dhcp4: true
  lab0:
    match:
      macaddress: "{lab_mac}"
    set-name: lab0
    addresses: [{lab_address}/24]
""".format(wan_mac=wan_mac, lab_mac=lab_mac, lab_address=LAB_ADDRESS)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="one-linux-seed-") as temporary:
        root = Path(temporary)
        (root / "meta-data").write_text(
            "instance-id: %s\nlocal-hostname: %s\n" % (hostname, hostname)
        )
        (root / "user-data").write_text(user_data)
        (root / "network-config").write_text(network)
        subprocess.run([
            "hdiutil", "makehybrid", "-quiet", "-iso", "-joliet",
            "-default-volume-name", "cidata", "-o", str(path), str(root),
        ], check=True)


def fetch_base():
    require_vm_home()
    if BASE.exists() and BASE_MANIFEST.exists():
        subprocess.run([executable("qemu-img"), "check", str(BASE)], check=True)
        print(BASE_MANIFEST)
        return
    headers = {}
    if os.environ.get("ONE_VM_AUTHORIZATION"):
        headers["Authorization"] = os.environ["ONE_VM_AUTHORIZATION"]
    expected = os.environ.get("LINUX_VM_SHA512", "").lower()
    if not expected:
        sums_url = urllib.parse.urljoin(IMAGE_URL, "SHA512SUMS")
        with urllib.request.urlopen(urllib.request.Request(sums_url, headers=headers)) as response:
            sums = response.read().decode()
        filename = urllib.parse.urlparse(IMAGE_URL).path.rsplit("/", 1)[-1]
        match = re.search(r"^([0-9a-f]{128})\s+\*?%s$" % re.escape(filename),
                          sums, re.MULTILINE)
        if not match:
            raise SystemExit("Image is absent from %s" % sums_url)
        expected = match.group(1)
    if not re.fullmatch(r"[0-9a-f]{128}", expected):
        raise SystemExit("LINUX_VM_SHA512 must contain 128 hexadecimal characters")
    IMAGES.mkdir(parents=True, exist_ok=True)
    temporary = BASE.with_suffix(".download.qcow2")
    digest = hashlib.sha512()
    try:
        with urllib.request.urlopen(urllib.request.Request(IMAGE_URL, headers=headers)) as response:
            with temporary.open("wb") as output:
                while chunk := response.read(8 * 1024 * 1024):
                    output.write(chunk)
                    digest.update(chunk)
        if digest.hexdigest() != expected:
            raise SystemExit("Downloaded Linux image failed SHA-512 verification")
        subprocess.run([executable("qemu-img"), "check", str(temporary)], check=True)
        temporary.replace(BASE)
        BASE.chmod(0o444)
        atomic_json(BASE_MANIFEST, {"file": BASE.name, "sha512": expected,
                                   "url": IMAGE_URL})
    except Exception:
        temporary.unlink(missing_ok=True)
        raise
    print(BASE_MANIFEST)


def create_instance(name, cpus=2, memory_mb=2048, disk_gb=16,
                    ssh_port=None, samba_port=None):
    require_vm_home()
    validate_name(name)
    if not BASE.is_file() or not BASE_MANIFEST.is_file():
        raise SystemExit("No Linux base image. Run: ./linux_vm.py fetch")
    if not 1 <= cpus <= 16:
        raise SystemExit("CPU count must be between 1 and 16")
    if not 512 <= memory_mb <= 65536:
        raise SystemExit("Memory must be between 512 and 65536 MiB")
    if not 8 <= disk_gb <= 1024:
        raise SystemExit("Disk size must be between 8 and 1024 GiB")
    LINUX_HOME.mkdir(parents=True, exist_ok=True)
    with (LINUX_HOME / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if instance_path(name).exists():
            raise SystemExit("Linux VM already exists: %s" % name)
        ssh_port = available_port("ssh_port", 22000, 22099, ssh_port)
        samba_port = available_port("samba_port", 14450, 14549, samba_port)
        hostname = "one-" + name
        private = key_path(name)
        public = private.with_suffix(".key.pub")
        overlay = overlay_path(name)
        seed = seed_path(name)
        INSTANCES.mkdir(parents=True, exist_ok=True)
        try:
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "",
                            "-C", hostname, "-f", str(private)], check=True)
            private.chmod(0o600)
            subprocess.run([
                executable("qemu-img"), "create", "-f", "qcow2", "-F", "qcow2",
                "-b", str(BASE), str(overlay),
            ], check=True)
            subprocess.run([executable("qemu-img"), "resize", str(overlay),
                            "%dG" % disk_gb], check=True)
            wan = mac("wan:", name)
            lab = mac("lab:", name)
            make_seed(seed, hostname, public.read_text().strip(), wan, lab)
            seed.chmod(0o600)
            config = {"cpus": cpus, "disk_gb": disk_gb, "hostname": hostname,
                      "lab_address": LAB_ADDRESS, "lab_mac": lab,
                      "memory_mb": memory_mb, "samba_port": samba_port,
                      "ssh_port": ssh_port, "wan_mac": wan}
            atomic_json(instance_path(name), config)
            instance_path(name).chmod(0o600)
        except Exception:
            for path in (private, public, overlay, seed, instance_path(name)):
                path.unlink(missing_ok=True)
            raise
    print(json.dumps(dict(config, name=name), sort_keys=True))


def launch(name):
    require_vm_home()
    config = load_instance(name)
    if running(name):
        raise SystemExit("Linux VM is already running: %s" % name)
    active = [other for other, _config in configs() if other != name and running(other)]
    if active:
        raise SystemExit("Only one Samba VM may use %s: %s" %
                         (LAB_ADDRESS, ", ".join(active)))
    if not FIRMWARE.is_file():
        raise SystemExit("AArch64 UEFI firmware not found: %s" % FIRMWARE)
    root, qmp_socket, pid, log_path = runtime(name)
    root.mkdir(parents=True, exist_ok=True)
    qmp_socket.unlink(missing_ok=True)
    lab_socket = ensure_hub(VM_HOME)
    command = [
        executable("qemu-system-aarch64"),
        "-name", "OneNote Linux Samba " + name,
        "-machine", "virt,accel=hvf", "-cpu", "host",
        "-smp", str(config["cpus"]), "-m", str(config["memory_mb"]),
        "-bios", str(FIRMWARE),
        "-drive", "if=none,file=%s,format=qcow2,id=root" % overlay_path(name),
        "-device", "virtio-blk-pci,drive=root",
        "-device", "virtio-scsi-pci,id=scsi",
        "-drive", "if=none,file=%s,format=raw,media=cdrom,readonly=on,id=seed" % seed_path(name),
        "-device", "scsi-cd,drive=seed",
        "-netdev", ("user,id=wan,hostfwd=tcp:127.0.0.1:%d-:22,"
                    "hostfwd=tcp:127.0.0.1:%d-:445" %
                    (config["ssh_port"], config["samba_port"])),
        "-device", "virtio-net-pci,netdev=wan,mac=%s" % config["wan_mac"],
        "-netdev", "vde,id=lab,sock=%s" % lab_socket,
        "-device", "virtio-net-pci,netdev=lab,mac=%s" % config["lab_mac"],
        "-uuid", str(uuid.uuid5(uuid.NAMESPACE_URL, "onenote-linux:" + name)),
        "-display", "none", "-serial", "stdio", "-monitor", "none",
        "-qmp", "unix:%s,server=on,wait=off" % qmp_socket,
        "-pidfile", str(pid),
    ]
    with log_path.open("ab") as log:
        subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                         start_new_session=True)
    for _ in range(50):
        if running(name):
            print("Linux VM opened: %s" % name)
            return
        time.sleep(0.1)
    raise SystemExit("Linux VM did not open. Check: %s" % log_path)


def ssh_argv(name, command=None):
    config = load_instance(name)
    known = INSTANCES / (name + ".known_hosts")
    argv = ["ssh", "-p", str(config["ssh_port"]), "-i", str(key_path(name)),
            "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes",
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", 'UserKnownHostsFile="%s"' % known,
            "-o", "ConnectTimeout=5", "agent@127.0.0.1"]
    if command is not None:
        argv.append(command)
    return argv


def run_ssh(name, command, timeout=60):
    return subprocess.run(ssh_argv(name, command), capture_output=True, text=True,
                          timeout=timeout)


def wait_instance(name, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if run_ssh(name, "true", timeout=8).returncode == 0:
                break
        except subprocess.TimeoutExpired:
            pass
        time.sleep(1)
    else:
        raise SystemExit("SSH did not become ready within %d seconds: %s" % (timeout, name))
    remaining = max(1, int(deadline - time.monotonic()))
    try:
        result = run_ssh(
            name,
            "sudo cloud-init status --wait && "
            "command -v smbd dnsmasq mount.cifs smbclient fio >/dev/null && "
            "systemctl is-active --quiet smbd dnsmasq",
            timeout=remaining,
        )
    except subprocess.TimeoutExpired:
        raise SystemExit("Cloud-init did not finish within %d seconds: %s" % (timeout, name))
    if result.returncode:
        raise SystemExit((result.stdout + result.stderr).strip() or
                         "Linux bootstrap validation failed: %s" % name)
    config = load_instance(name)
    print("Ready: %s ssh=127.0.0.1:%d smb=127.0.0.1:%d windows=//%s/agent" %
          (name, config["ssh_port"], config["samba_port"], LAB_ADDRESS))


def qmp(name, command):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(5)
        client.connect(str(runtime(name)[1]))
        stream = client.makefile("rwb", buffering=0)
        stream.readline()
        stream.write(b'{"execute":"qmp_capabilities"}\n')
        while "return" not in json.loads(stream.readline()):
            pass
        stream.write((json.dumps({"execute": command}) + "\n").encode())
        while True:
            response = json.loads(stream.readline())
            if "return" in response:
                return response["return"]
            if "error" in response:
                raise SystemExit(response["error"]["desc"])


def shutdown(name, timeout):
    if not running(name):
        raise SystemExit("Linux VM is not running: %s" % name)
    try:
        run_ssh(name, "sudo poweroff", timeout=10)
    except (subprocess.TimeoutExpired, OSError):
        qmp(name, "system_powerdown")
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not running(name):
            print("Linux VM stopped: %s" % name)
            return
        time.sleep(1)
    raise SystemExit("Linux VM did not stop within %d seconds: %s" % (timeout, name))


def delete_instance(name):
    require_vm_home()
    load_instance(name)
    if running(name):
        raise SystemExit("Shut down Linux before deleting: %s" % name)
    for path in (overlay_path(name), seed_path(name), key_path(name),
                 key_path(name).with_suffix(".key.pub"),
                 INSTANCES / (name + ".known_hosts"), instance_path(name)):
        path.unlink(missing_ok=True)
    shutil.rmtree(runtime(name)[0], ignore_errors=True)
    print("Deleted Linux VM: %s" % name)


def list_instances(name=None):
    if name:
        load_instance(name)
        print("running" if running(name) else "stopped")
        return
    rows = configs()
    if not rows:
        print("No Linux VMs.")
        return
    for vm_name, config in rows:
        state = "running" if running(vm_name) else "stopped"
        print("%-24s %-24s %-7s ssh:%d smb:%d" %
              (vm_name, config["hostname"], state, config["ssh_port"],
               config["samba_port"]))


def main():
    parser = argparse.ArgumentParser(description="Run SSH-only OneNote Linux Samba VMs")
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("fetch")
    status = commands.add_parser("status")
    status.add_argument("name", nargs="?")
    up = commands.add_parser("up")
    up.add_argument("name")
    up.add_argument("--cpus", type=int, default=2)
    up.add_argument("--memory", type=int, default=2048, dest="memory_mb")
    up.add_argument("--disk", type=int, default=16, dest="disk_gb")
    up.add_argument("--ssh-port", type=int)
    up.add_argument("--samba-port", type=int)
    up.add_argument("--wait", action="store_true")
    up.add_argument("--timeout", type=int, default=600)
    ssh = commands.add_parser("ssh")
    ssh.add_argument("name")
    ssh.add_argument("remote_command", nargs=argparse.REMAINDER)
    down = commands.add_parser("down")
    down.add_argument("name")
    down.add_argument("--timeout", type=int, default=60)
    down.add_argument("--preserve-machine", action="store_true")
    args = parser.parse_args()
    if args.command == "fetch":
        fetch_base()
    elif args.command == "up":
        if not instance_path(args.name).exists():
            create_instance(args.name, args.cpus, args.memory_mb, args.disk_gb,
                            args.ssh_port, args.samba_port)
        if running(args.name):
            print("Linux VM already running: %s" % args.name)
        else:
            launch(args.name)
        if args.wait:
            wait_instance(args.name, args.timeout)
    elif args.command == "ssh":
        command = args.remote_command
        if command[:1] == ["--"]:
            command = command[1:]
        remote = command[0] if len(command) == 1 else shlex.join(command)
        os.execvp("ssh", ssh_argv(args.name, remote if command else None))
    elif args.command == "down":
        if not instance_path(args.name).exists():
            print("Linux VM absent: %s" % args.name)
        else:
            load_instance(args.name)
            if running(args.name):
                shutdown(args.name, args.timeout)
            if args.preserve_machine:
                print("Preserved Linux VM: %s" % args.name)
            else:
                delete_instance(args.name)
    else:
        if args.name and not instance_path(args.name).exists():
            print("absent")
        else:
            list_instances(args.name)


if __name__ == "__main__":
    main()
