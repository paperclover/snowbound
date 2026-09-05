from pathlib import Path
import signal
import unittest
from unittest.mock import Mock, patch

import crash


class AbruptStopTest(unittest.TestCase):
    def backend(self):
        backend = Mock()
        backend.runtime.return_value = (Path('/lab/owned'), Path('/lab/owned/qmp.sock'), Mock(), None)
        backend.runtime.return_value[2].read_text.return_value = '1234'
        backend.qmp.return_value = {'name': 'OneNote Windows 7 owned'}
        backend.running.side_effect = [True, False, False]
        backend.instance_path.return_value.exists.return_value = True
        return backend

    def test_stops_identified_clone_and_preserves_machine(self):
        backend = self.backend()
        with patch.object(crash, 'vm', backend), patch.object(crash.subprocess, 'check_output', return_value='qemu-system-x86_64 -qmp unix:/lab/owned/qmp.sock'), patch.object(crash.os, 'kill') as stop:
            result = crash.stop('windows', 'owned')
        stop.assert_called_once_with(1234, signal.SIGKILL)
        backend.load_instance.assert_called_once_with('owned')
        backend.delete_instance.assert_not_called()
        backend.shutdown.assert_not_called()
        self.assertTrue(result['machine_preserved'])

    def test_refuses_mismatched_vm_identity(self):
        backend = self.backend()
        backend.qmp.return_value = {'name': 'OneNote Windows 7 another'}
        with patch.object(crash, 'vm', backend), patch.object(crash.os, 'kill') as stop:
            with self.assertRaisesRegex(ValueError, 'identity'):
                crash.stop('windows', 'owned')
        stop.assert_not_called()

    def test_refuses_pid_belonging_to_another_process(self):
        backend = self.backend()
        with patch.object(crash, 'vm', backend), patch.object(crash.subprocess, 'check_output', return_value='qemu-system-x86_64 -qmp unix:/lab/other/qmp.sock'), patch.object(crash.os, 'kill') as stop:
            with self.assertRaisesRegex(ValueError, 'registered process'):
                crash.stop('windows', 'owned')
        stop.assert_not_called()


if __name__ == '__main__':
    unittest.main()
