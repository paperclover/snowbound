import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import native_runner as runner


class NativeRunnerTest(unittest.TestCase):
    def test_shutdown_timeout_still_removes_the_owned_clone(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            manifest = output / 'instance.json'
            with patch.object(runner, 'vm') as vm, patch.object(runner, 'install_agent', return_value={}):
                vm.instance_path.return_value = manifest
                vm.create_instance.side_effect = lambda _: manifest.touch()
                vm.delete_instance.side_effect = lambda _: manifest.unlink()
                vm.running.side_effect = [True, False]
                vm.shutdown.side_effect = SystemExit('Windows did not stop')
                with runner.clone(output) as name:
                    vm.create_instance.assert_called_once_with(name)
                vm.qmp.assert_called_once_with(name, 'quit')
                vm.delete_instance.assert_called_once_with(name)
            self.assertEqual(json.loads((output / 'teardown.json').read_text()), {'absent': True})

    def test_command_timeout_is_a_failure_even_with_stdout(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            with patch.object(runner.windows, 'do_cmd', return_value={
                'exit': None, 'stdout': 'Read 1 pages.', 'error': 'timed out',
            }):
                with self.assertRaisesRegex(RuntimeError, 'timed out'):
                    runner.command('fixture', 'read', output)
            result = json.loads((output / 'commands.jsonl').read_text())
            self.assertIsNone(result['exit'])

    def test_failure_still_tears_down_the_owned_clone(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            manifest = output / 'instance.json'
            with patch.object(runner.vm, 'instance_path', return_value=manifest), \
                 patch.object(runner.vm, 'create_instance', side_effect=lambda _: manifest.touch()), \
                 patch.object(runner.vm, 'start_instance'), \
                 patch.object(runner.vm, 'wait_instance'), \
                 patch.object(runner.vm, 'running', return_value=True), \
                 patch.object(runner.vm, 'shutdown') as shutdown, \
                 patch.object(runner.vm, 'delete_instance', side_effect=lambda _: manifest.unlink()), \
                 patch.object(runner, 'install_agent', return_value={}), \
                 patch.object(runner.windows, 'do_health', return_value={}), \
                 patch.object(runner.windows, 'do_shot', side_effect=RuntimeError('unreachable')):
                with self.assertRaisesRegex(RuntimeError, 'capture failed'):
                    with runner.clone(output):
                        raise RuntimeError('capture failed')
                shutdown.assert_called_once()
            self.assertEqual(json.loads((output / 'teardown.json').read_text()), {'absent': True})

    def test_output_inside_notebook_does_not_modify_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            notebook = Path(temporary)
            output = notebook / 'new-export'
            with self.assertRaisesRegex(ValueError, 'outside'):
                runner.capture(notebook, output)
            self.assertFalse(output.exists())


if __name__ == '__main__':
    unittest.main()
