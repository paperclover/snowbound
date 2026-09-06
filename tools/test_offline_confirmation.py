import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from verify_offline_confirmation import verify


class ConfirmationOracle(unittest.TestCase):
    def test_native_images_must_match_the_confirmation_and_preserved_native_prefix(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            cold = root / 'cold'
            (root / 'rust/confirmations').mkdir(parents=True)
            (root / 'n0').mkdir()
            (cold / 'results/123-456').mkdir(parents=True)
            (root / 'run.json').write_text(json.dumps(dict(rust_writers=1, rust_readers=0, stress_clients=1, stress_operations=1)))
            rows = [dict(event='ready', pid=123), dict(event='remote_attempt', state='Unknown', revision='revision', space='space'),
                    dict(event='remote_confirm', capture='123-456.one', revisions={'space': ['revision']}, text='Concurrent edits: [w0:0]')]
            (root / 'rust/w0.jsonl').write_text('\n'.join(map(json.dumps, rows)))
            (root / 'n0/stress-events.jsonl').write_text(json.dumps(dict(operation=0, token=' [n0:0]', before='Native 0:')))
            snapshot = root / 'rust/confirmations/123-456.one'
            snapshot.write_bytes(b'captured snapshot')
            sha = hashlib.sha256(snapshot.read_bytes()).hexdigest()
            (cold / 'run.json').write_text(json.dumps(dict(inputs={'123-456.one': sha})))
            result = dict(name='123-456', error=None, pages=1, source_sha256=sha, seconds=1)
            (cold / 'results.json').write_text(json.dumps(result))
            (cold / 'teardown.json').write_text(json.dumps(dict(absent=True)))
            page = cold / 'results/123-456/page-0.xml'
            def xml(text, native):
                return f'<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote"><one:Outline><one:OEChildren><one:OE><one:T>{text}</one:T></one:OE><one:OE><one:T>{native}</one:T></one:OE></one:OEChildren></one:Outline></one:Page>'
            page.write_text(xml('Concurrent edits: [w0:0]', 'Native 0:'))
            with patch('verify_offline_confirmation.publication_links') as ledger:
                self.assertEqual(verify(root, cold)['validated_paragraphs'], 2)
                self.assertTrue(ledger.called)
                config = json.loads((root / 'run.json').read_text())
                config['stress_operations'] = 2
                (root / 'run.json').write_text(json.dumps(config))
                with self.assertRaisesRegex(AssertionError, 'Missing native acknowledgements'):
                    verify(root, cold)
                self.assertFalse(verify(root, cold, partial=True)['complete_workload'])
                config['stress_operations'] = 1
                (root / 'run.json').write_text(json.dumps(config))
                for text, native in [('Concurrent edits:', 'Native 0:'), ('Concurrent edits: [w0:0]', 'Native 0: [n0:1]')]:
                    page.write_text(xml(text, native))
                    with self.assertRaises(AssertionError): verify(root, cold)
                page.write_text(xml('Concurrent edits: [w0:0]', 'Native 0: [n0:0]'))
                self.assertEqual(verify(root, cold)['native_images'], 1)
                snapshot.write_bytes(b'changed')
                with self.assertRaises(AssertionError): verify(root, cold)
                snapshot.write_bytes(b'captured snapshot')
                config['document_operations'] = True
                (root / 'run.json').write_text(json.dumps(config))
                document = dict(text='x', runs=[dict(text='x', bold=True, size=18, color=0x563412)])
                rows[1]['document_changes'] = {'target': document}
                rows[2]['started_us'] = 200
                observed = dict(event='read', finished_us=100, text=rows[2]['text'], documents={'target': document})
                rows.insert(2, observed)
                (root / 'rust/w0.jsonl').write_text('\n'.join(map(json.dumps, rows)))
                markup = '<one:OE><one:T><![CDATA[<span style="font-weight:bold;font-size:18pt;color:#123456">x</span>]]></one:T></one:OE>'
                content = xml('Concurrent edits: [w0:0]', 'Native 0:').replace('</one:OEChildren>', markup + '</one:OEChildren>')
                page.write_text(content)
                with patch('offline_document_history.document_history', return_value={'target': {'text': 'x', 'format': {'receipt_revision': 'revision'}}}) as documents:
                    result = verify(root, cold)
                    self.assertEqual(result['validated_paragraphs'], 3)
                    self.assertEqual(result['native_intended_format_checks'], 3)
                    documents.assert_called_once_with({'w0': rows}, 1)
                    rows[3]['revisions'] = {'space': ['current']}
                    rows[3]['current_revisions'] = {'space': 'current'}
                    documents.return_value['target']['format']['receipt_revision'] = 'current'
                    (root / 'rust/w0.jsonl').write_text('\n'.join(map(json.dumps, rows)))
                    self.assertEqual(verify(root, cold)['confirmed_revision'], 'current')
                    rows[3]['current_revisions'] = {'space': 'unrelated'}
                    (root / 'rust/w0.jsonl').write_text('\n'.join(map(json.dumps, rows)))
                    with self.assertRaisesRegex(AssertionError, 'current effect-confirmation'): verify(root, cold)
                    rows[3]['current_revisions'] = {'space': 'current'}
                    (root / 'rust/w0.jsonl').write_text('\n'.join(map(json.dumps, rows)))
                    page.write_text(content.replace('18pt', '19pt'))
                    with self.assertRaisesRegex(AssertionError, 'font size'): verify(root, cold)
                    page.write_text(content)
                    observed['documents'] = {}
                    (root / 'rust/w0.jsonl').write_text('\n'.join(map(json.dumps, rows)))
                    with self.assertRaisesRegex(AssertionError, 'omitted'): verify(root, cold)


if __name__ == '__main__': unittest.main()
