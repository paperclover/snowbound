import contextlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

import vm


class VmLifecycleTest(unittest.TestCase):
    def setUp(self):
        self.original = {name: getattr(vm, name) for name in (
            "VM_HOME", "IMAGES", "MEDIA", "INSTANCES", "RUN", "AGENT_ISO", "TARGETS",
        )}
        self.temporary = tempfile.TemporaryDirectory(prefix="one-vm-test-")
        home = Path(self.temporary.name)
        vm.VM_HOME = home
        vm.IMAGES = home / "images"
        vm.MEDIA = home / "media"
        vm.INSTANCES = home / "instances"
        vm.RUN = home / "run"
        vm.AGENT_ISO = vm.MEDIA / "win7-agent.iso"
        vm.TARGETS = home / "targets.json"
        vm.IMAGES.mkdir(parents=True)
        for base in ("win7", "win11"):
            subprocess.run([
                vm.qemu("qemu-img"), "create", "-q", "-f", "qcow2",
                str(vm.build_disk(base)), "64M",
            ], check=True)
            with contextlib.redirect_stdout(io.StringIO()):
                vm.seal(base)

    def tearDown(self):
        self.temporary.cleanup()
        for name, value in self.original.items():
            setattr(vm, name, value)

    def test_clone_has_unique_identity_and_removable_overlay(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            vm.create_instance("alpha")
            vm.create_instance("beta")
        config = json.loads(vm.instance_path("alpha").read_text())
        other = json.loads(vm.instance_path("beta").read_text())
        targets = json.loads(vm.TARGETS.read_text())
        self.assertEqual(config["hostname"], "ONE-ALPHA")
        self.assertEqual(targets["alpha"]["base"],
                         "http://127.0.0.1:%d" % config["port"])
        self.assertEqual(targets["alpha"]["token"], config["token"])
        self.assertNotEqual(config["hostname"], other["hostname"])
        self.assertNotEqual(config["port"], other["port"])
        self.assertNotEqual(config["token"], other["token"])
        self.assertNotIn(config["token"], output.getvalue())
        self.assertTrue((vm.IMAGES / "instances" / "alpha.qcow2").is_file())
        self.assertTrue((vm.MEDIA / "instances" / "alpha.iso").is_file())
        with contextlib.redirect_stdout(io.StringIO()):
            vm.delete_instance("alpha")
            vm.delete_instance("beta")
        self.assertFalse(vm.instance_path("alpha").exists())
        self.assertNotIn("alpha", json.loads(vm.TARGETS.read_text()))

    def test_reserved_target_name_is_rejected(self):
        with self.assertRaisesRegex(SystemExit, "reserved"):
            vm.create_instance("local")
        with self.assertRaisesRegex(SystemExit, "reserved"):
            vm.create_instance("win11-build")

    def test_clone_records_its_base(self):
        with contextlib.redirect_stdout(io.StringIO()):
            vm.create_instance("arm", base="win11")
        self.assertEqual(json.loads(vm.instance_path("arm").read_text())["base"], "win11")
        backing = json.loads(subprocess.check_output([
            vm.qemu("qemu-img"), "info", "--output=json",
            str(vm.IMAGES / "instances" / "arm.qcow2"),
        ]))["backing-filename"]
        self.assertEqual(backing, str(vm.base_disk("win11")))


if __name__ == "__main__":
    unittest.main()
