import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from offline_publication_crash import state, TOKEN, confirmation_only
from verify_offline_recovery import verify


class RecoveryOracleTests(unittest.TestCase):
    def test_confirmation_cannot_hide_revision_or_content_writes(self):
        source = bytes(1024)
        changed = bytearray(source)
        changed[212:252] = bytes(range(40))
        events = [{'event':'write', 'offset':212, 'bytes':40}]
        confirmation_only(events, source, changed)
        for offset in (96, 211, 252, 1023):
            bad = changed.copy()
            bad[offset] ^= 1
            with self.assertRaises(AssertionError): confirmation_only(events, source, bad)
        for event in ({'event':'write','offset':96,'bytes':1}, {'event':'phase','name':'publish-before'}):
            with self.assertRaises(AssertionError): confirmation_only([*events,event], source, changed)

    def test_text_acknowledgements_and_revision_identity_must_agree(self):
        original = 'Concurrent edits: 🦀'
        at = len(original.encode('utf-16-le')) // 2
        row = {'event': 'state', 'status': 'uncertain', 'revision': 'new', 'remote_revision': 'old',
               'local_text': original+TOKEN, 'remote_text': original,
               'pending': [{'id': 1, 'replacement': TOKEN, 'range': [at, at]}]}
        self.assertEqual(state([row], original), row)
        for field, value in [('revision', None), ('revision', 'old'), ('local_text', original),
                             ('remote_text', original+TOKEN+TOKEN), ('status', 'missing'), ('pending', [])]:
            changed = copy.deepcopy(row)
            changed[field] = value
            with self.subTest(field=field):
                with self.assertRaises(AssertionError): state([changed], original)
        for field, value in [('id', 2), ('replacement', 'other'), ('range', [at-1, at-1])]:
            changed = copy.deepcopy(row)
            changed['pending'][0][field] = value
            with self.assertRaises(AssertionError): state([changed], original)
        row.update(status='published', remote_revision='new', remote_text=original+TOKEN, pending=[])
        self.assertEqual(state([row], original), row)
        for field, value in [('revision', 'old'), ('remote_text', original), ('pending', [{'id': 1}])]:
            changed = copy.deepcopy(row)
            changed[field] = value
            with self.assertRaises(AssertionError): state([changed], original)

    def test_native_inventory_hashes_errors_and_content_are_independently_checked(self):
        with tempfile.TemporaryDirectory() as folder:
            run, cold, source = [Path(folder)/name for name in ('run', 'cold', 'source.xml')]
            (run/'images').mkdir(parents=True)
            cold.mkdir()
            data = b'owned fixture bytes'
            digest = hashlib.sha256(data).hexdigest()
            (run/'images'/f'{digest}.one').write_bytes(data)
            expected = ['Concurrent edits:'+TOKEN, 'Other paragraph']
            (run/'results.json').write_text(json.dumps([{'remote_image': digest, 'remote_text': expected[0]}]))
            (cold/'run.json').write_text(json.dumps({'inputs': {digest+'.one': digest}}))
            record = {'name': digest, 'source_sha256': digest, 'error': None, 'pages': 1, 'seconds': 1}
            (cold/'results.json').write_text(json.dumps(record))
            (cold/'teardown.json').write_text(json.dumps({'absent': True}))
            (cold/'results'/digest).mkdir(parents=True)
            (cold/'results'/digest/'page-0.xml').touch()
            def content(path):
                return ['Concurrent edits:', 'Other paragraph'] if path == source else expected
            with patch('verify_offline_recovery.paragraphs', side_effect=content):
                self.assertEqual(verify(run, cold, source)['exact_images'], 1)
                for key, value in [('source_sha256', 'wrong'), ('error', 'failed'), ('pages', 0), ('name', 'wrong')]:
                    (cold/'results.json').write_text(json.dumps({**record, key:value}))
                    with self.subTest(key=key):
                        with self.assertRaises(AssertionError): verify(run, cold, source)
                (cold/'results.json').write_text(json.dumps([record, record]))
                with self.assertRaises(AssertionError): verify(run, cold, source)
                (cold/'results.json').write_text(json.dumps(record))
                expected[0] = 'Concurrent edits:'
                with self.assertRaises(AssertionError): verify(run, cold, source)
                expected[0] += TOKEN
                (run/'images'/f'{digest}.one').write_bytes(b'changed')
                with self.assertRaises(AssertionError): verify(run, cold, source)
