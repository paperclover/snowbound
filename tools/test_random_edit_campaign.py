import copy
from pathlib import Path
import json
import subprocess
import tempfile
import unittest
from random_edit_campaign import EDITOR, ROOT, export, verify
from document_model import ordered_pages


class RandomEditOracleTests(unittest.TestCase):
    def test_a_random_replacement_cannot_be_a_no_op(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / 'source.one'
            subprocess.run([ROOT / 'target/debug/examples/create_section', source, ' café ', 'No-op regression'], check=True)
            before = export(source)
            for mode in ['create', 'in-place']:
                destination = Path(temporary) / (mode + '.one')
                if mode == 'create':
                    args = [EDITOR, source, destination, '301']
                else:
                    destination.write_bytes(source.read_bytes())
                    args = [EDITOR, destination, '--in-place', '301']
                record = json.loads(subprocess.check_output(args))
                self.assertEqual(record['range'], [0, 0])
                self.assertNotEqual(source.read_bytes(), destination.read_bytes())
                verify(before, export(destination), record)

    def test_native_fixture_splice_and_unrelated_damage(self):
        source = ROOT / 'corpus/native/20260905-05/snapshots/06-attachment/notebook/synthetic.one'
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / 'edited.one'
            record = json.loads(subprocess.check_output([EDITOR, source, destination, '42']))
            before, after = export(source), export(destination)
            verify(before, after, record)
            sid, oid = record['space'], record['object']
            rid = after[0]['spaces'][sid]['contexts']['{00000000-0000-0000-0000-000000000000},0']
            for kind in ['format', 'metadata', 'payload', 'structure', 'ancestor-time', 'unrelated-time']:
                damaged = copy.deepcopy(after)
                if kind == 'format': damaged[1][sid][rid][oid][0]['format']['bold'] = not damaged[1][sid][rid][oid][0]['format']['bold']
                if kind == 'metadata': damaged[0]['spaces'][sid]['revisions'][rid]['nodes'][oid]['created'] = 0
                if kind == 'payload': damaged[2].append('another payload')
                if kind == 'structure': damaged[0]['spaces'][sid]['revisions'][rid]['nodes'][oid]['children'].append('another object')
                if kind == 'ancestor-time': damaged[0]['spaces'][sid]['revisions'][rid]['nodes'][record['page']]['modified'] = 0
                if kind == 'unrelated-time':
                    old_rid = before[0]['spaces'][sid]['contexts']['{00000000-0000-0000-0000-000000000000},0']
                    old = before[0]['spaces'][sid]['revisions'][old_rid]['nodes']
                    nodes = damaged[0]['spaces'][sid]['revisions'][rid]['nodes']
                    key = next(key for key, node in nodes.items() if node['modified'] is not None and node['modified'] == old[key]['modified'])
                    nodes[key]['modified'] = 0
                with self.assertRaises(AssertionError, msg=kind): verify(before, damaged, record)

    def test_seeded_edits_work_on_native_legacy_text(self):
        source = ROOT / 'corpus/native/20260905-05/snapshots/02-text/notebook/synthetic.one'
        with tempfile.TemporaryDirectory() as temporary:
            for seed in range(42, 74):
                destination = Path(temporary) / f'{seed}.one'
                result = subprocess.run([EDITOR, source, destination, str(seed)], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, (seed, result.stderr))
                self.assertTrue(destination.is_file())

    def test_title_cache_changes_are_required_and_bounded(self):
        source = ROOT / 'corpus/m6/native-structure-01/notebook/synthetic.one'
        before = export(source)
        sid, _, revision, page = next(p for p in ordered_pages(before[0]) if any(
            n['kind']['type'] == 'RichText' and any(f['id'] == 0x88001cb4 for f in n['extra'][0])
            for n in p[2]['nodes'].values()))
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / 'edited.one'
            record = json.loads(subprocess.check_output([EDITOR, source, destination, '98', page]))
            self.assertTrue(any(f['id'] == 0x88001cb4 for f in revision['nodes'][record['object']]['extra'][0]))
            after = export(destination)
            verify(before, after, record)
            rid = after[0]['spaces'][sid]['contexts']['{00000000-0000-0000-0000-000000000000},0']
            damaged = copy.deepcopy(after)
            damaged[0]['spaces'][sid]['revisions'][rid]['nodes'][revision['roots']['2']]['kind']['title'] = revision['nodes'][revision['roots']['2']]['kind']['title']
            with self.assertRaises(AssertionError): verify(before, damaged, record)


if __name__ == '__main__':
    unittest.main()
