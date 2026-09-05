import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import linux_vm


class LinuxVmLifecycleTest(unittest.TestCase):
    def setUp(self):
        names = ("VM_HOME", "LINUX_HOME", "IMAGES", "INSTANCES", "RUN", "BASE",
                 "BASE_MANIFEST")
        self.original = {name: getattr(linux_vm, name) for name in names}
        self.temporary = tempfile.TemporaryDirectory(prefix="one-linux-test-")
        home = Path(self.temporary.name)
        linux_vm.VM_HOME = home
        linux_vm.LINUX_HOME = home / "linux"
        linux_vm.IMAGES = linux_vm.LINUX_HOME / "images"
        linux_vm.INSTANCES = linux_vm.LINUX_HOME / "instances"
        linux_vm.RUN = linux_vm.LINUX_HOME / "run"
        linux_vm.BASE = linux_vm.IMAGES / "base.qcow2"
        linux_vm.BASE_MANIFEST = linux_vm.IMAGES / "base.json"
        linux_vm.IMAGES.mkdir(parents=True)
        subprocess.run([
            linux_vm.executable("qemu-img"), "create", "-q", "-f", "qcow2",
            str(linux_vm.BASE), "64M",
        ], check=True)
        linux_vm.BASE_MANIFEST.write_text("{}\n")

    def tearDown(self):
        self.temporary.cleanup()
        for name, value in self.original.items():
            setattr(linux_vm, name, value)

    def test_clone_has_unique_ports_keys_network_and_removable_overlay(self):
        with contextlib.redirect_stdout(io.StringIO()):
            linux_vm.create_instance("samba")
            linux_vm.create_instance("other")
        config = json.loads(linux_vm.instance_path("samba").read_text())
        other = json.loads(linux_vm.instance_path("other").read_text())
        self.assertEqual(config["hostname"], "one-samba")
        self.assertEqual(config["lab_address"], "192.168.77.1")
        self.assertNotEqual(config["ssh_port"], other["ssh_port"])
        self.assertNotEqual(config["samba_port"], other["samba_port"])
        self.assertNotEqual(config["lab_mac"], other["lab_mac"])
        self.assertEqual(linux_vm.key_path("samba").stat().st_mode & 0o777, 0o600)
        known_hosts = next(
            value for value in linux_vm.ssh_argv("samba")
            if value.startswith("UserKnownHostsFile=")
        )
        self.assertEqual(known_hosts, 'UserKnownHostsFile="%s"' %
                         (linux_vm.INSTANCES / "samba.known_hosts"))
        self.assertTrue(linux_vm.overlay_path("samba").is_file())
        self.assertTrue(linux_vm.seed_path("samba").is_file())
        with contextlib.redirect_stdout(io.StringIO()):
            linux_vm.delete_instance("samba")
            linux_vm.delete_instance("other")
        self.assertFalse(linux_vm.instance_path("samba").exists())

    def test_invalid_name_is_rejected(self):
        with self.assertRaisesRegex(SystemExit, "lowercase"):
            linux_vm.create_instance("Bad Name")


if __name__ == "__main__":
    unittest.main()
