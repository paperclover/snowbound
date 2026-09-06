import copy
import unittest
import uuid
from unittest.mock import patch

from offline_document_history import document_history, identity, verify_model, verify_native


class DocumentHistoryTests(unittest.TestCase):
    def setUp(self):
        self.logs = {'w0': [{'event': 'ready', 'document_operations': True}]}
        events = self.logs['w0']
        observed = {}
        previous = None
        for operation in range(2):
            insertion = {'guid': list(uuid.UUID(int=operation+1).bytes_le), 'text': f'Document w0:{operation} 🦀',
                         'parent': 'page' if operation == 0 else identity(previous, 1), 'author': 'Offline document writer',
                         'placement': {'Outline': {'x': 144, 'y': 144}} if operation == 0 else {'Paragraph': {'before': None}}}
            previous = insertion
            target = identity(insertion, 2)
            for step, kind in enumerate(('insert', 'format')):
                timestamp = 10 + operation*30 + step*10
                local_id = operation*3 + step + 2
                event = {'event': 'local_document_commit', 'id': local_id, 'operation': operation, 'kind': kind,
                         'space': 'space', 'object': target, 'text': insertion['text'], 'insertion': insertion if step == 0 else None,
                         'range': [1, len(insertion['text'].encode('utf-16-le'))//2-2],
                         'attributes': [{'Bold': True}, {'FontSize': 18+operation}, {'Color': [18, 52, 86]}],
                         'started_us': timestamp-2, 'finished_us': timestamp-1}
                events.append(event)
                runs = []
                for index, char in enumerate(insertion['text']):
                    selected = kind == 'format' and 0 < index < len(insertion['text'])-1
                    runs.append({'text': char, 'bold': selected, 'size': 18+operation if selected else 11,
                                 'color': 0x563412 if selected else 0xff000000})
                observed[target] = {'text': insertion['text'], 'runs': runs}
                revision = f'revision-{local_id}'
                events.append({'event': 'remote_attempt', 'revision': revision, 'state': 'Committed',
                               'document_changes': {target: copy.deepcopy(observed[target])},
                               'documents': copy.deepcopy(observed), 'started_us': timestamp, 'finished_us': timestamp+1})
                events.append({'event': 'document_receipt', 'id': local_id, 'revision': revision, 'at_us': timestamp+2})
        events.extend({'event': 'reopened_document_receipt', 'id': row['id'], 'revision': row['revision']}
                      for row in list(events) if row['event'] == 'document_receipt')
        read = {'event': 'read', 'started_us': 100, 'finished_us': 101, 'documents': observed}
        events.extend([read, {'event': 'done'}])
        self.logs['r0'] = [{'event': 'ready'}, copy.deepcopy(read), {'event': 'done'}]

    def test_document_receipts_and_reader_states_match_the_intents(self):
        documents = document_history(self.logs, 2)
        self.assertEqual(len(documents), 2)
        paragraphs = [[(char, {'bold': bold, 'font_size': size, 'color': 'automatic' if color == 0xff000000 else '#123456'})
                       for char, bold, size, color in row['new']] for row in documents.values()]
        self.assertEqual(verify_native(paragraphs, documents), sum(len(row['new'])*3 for row in documents.values()))
        paragraphs[0][1][1]['font_size'] = 19
        with self.assertRaisesRegex(AssertionError, 'font size'): verify_native(paragraphs, documents)

    def test_missing_intents_receipts_or_reopen_records_are_rejected(self):
        for name in ('local_document_commit', 'document_receipt', 'reopened_document_receipt', 'remote_attempt'):
            logs = copy.deepcopy(self.logs)
            events = logs['w0']
            events.remove(next(row for row in events if row['event'] == name))
            with self.subTest(event=name), self.assertRaises(AssertionError): document_history(logs, 2)

    def test_retired_format_receipt_requires_current_revision_and_exact_observed_effect(self):
        events = self.logs['w0']
        attempt = next(row for row in events if row['event'] == 'remote_attempt' and row['revision'] == 'revision-3')
        attempt['state'] = 'Unknown'
        receipt = next(row for row in events if row['event'] == 'document_receipt' and row['id'] == 3)
        receipt.update(revision='current-revision', at_us=25)
        next(row for row in events if row['event'] == 'reopened_document_receipt' and row['id'] == 3)['revision'] = receipt['revision']
        read = dict(event='read', started_us=21, finished_us=22, text='Concurrent edits:', documents=copy.deepcopy(attempt['documents']))
        confirmation = dict(event='remote_confirm', started_us=23, finished_us=24, state='Committed', text=read['text'],
                            revisions={'space': ['current-revision']}, current_revisions={'space': 'current-revision'})
        index = events.index(receipt)
        events[index:index] = [read, confirmation]
        result = document_history(self.logs, 2)
        target, = attempt['document_changes']
        self.assertEqual(result[target]['format']['receipt_revision'], 'current-revision')
        for field, value in [('current_revisions', {'space': 'unrelated'}),
                             ('revisions', {'space': ['revision-3', 'current-revision']}),
                             ('state', 'NotCommitted')]:
            original = confirmation[field]
            confirmation[field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError): document_history(self.logs, 2)
            confirmation[field] = original
        read['documents'][target]['runs'][1]['size'] = 12
        with self.assertRaisesRegex(AssertionError, 'differs from the uncertain formatting intent'): document_history(self.logs, 2)

    def test_model_rejects_reordered_or_reparented_insertions(self):
        documents = document_history(self.logs, 2)
        first, second = [row['insertion'] for row in documents.values()]
        outline, paragraph, appended = identity(first, 1), identity(first, 3), identity(second, 1)
        nodes = {key: {'structure': [], 'content': [], 'children': [], 'kind': {}, 'layout': {}}
                 for key in ['page', outline, paragraph, appended, *documents]}
        nodes['page']['children'] = [outline]
        nodes[outline].update(children=[paragraph, appended], layout={'x': 144, 'y': 144})
        for parent, (target, document) in zip([paragraph, appended], documents.items(), strict=True):
            nodes[parent]['content'] = [target]
            nodes[target]['kind'] = {'text': document['text']}
        revision = {'nodes': nodes}
        with patch('offline_document_history.ordered_pages', return_value=[('space', 'revision', revision, 'page')]):
            verify_model({}, documents)
            nodes[outline]['children'].reverse()
            with self.assertRaisesRegex(AssertionError, 'order'): verify_model({}, documents)
            nodes[outline]['children'] = [paragraph]
            nodes['page']['children'].append(appended)
            with self.assertRaises(AssertionError): verify_model({}, documents)
            nodes[outline]['children'] = [paragraph, appended]
            nodes['page']['children'] = [outline]
            nodes[paragraph]['content'].append(identity(second, 2))
            with self.assertRaisesRegex(AssertionError, 'content'): verify_model({}, documents)

    def test_wrong_attributes_targets_and_unconfirmed_receipts_are_rejected(self):
        for event, field, value in [('local_document_commit', 'object', 'wrong'),
                                    ('local_document_commit', 'text', 'changed'),
                                    ('document_receipt', 'revision', 'wrong'),
                                    ('remote_attempt', 'state', 'Unknown'),
                                    ('remote_attempt', 'state', 'NotCommitted')]:
            logs = copy.deepcopy(self.logs)
            next(row for row in logs['w0'] if row['event'] == event)[field] = value
            with self.subTest(event=event, field=field), self.assertRaises(AssertionError): document_history(logs, 2)
        for field, value in [('attributes', [{'Bold': False}]), ('range', [0, 1])]:
            logs = copy.deepcopy(self.logs)
            next(row for row in logs['w0'] if row.get('kind') == 'format')[field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError): document_history(logs, 2)

    def test_readers_cannot_lose_revert_or_invent_document_content(self):
        for mutation in ('missing', 'partial', 'future', 'reverted'):
            logs = copy.deepcopy(self.logs)
            read = logs['r0'][1]
            target = next(iter(read['documents']))
            if mutation == 'missing': del read['documents'][target]
            elif mutation == 'partial': read['documents'][target]['runs'][1]['bold'] = False
            elif mutation == 'future': read.update(started_us=0, finished_us=1)
            else:
                old = copy.deepcopy(read)
                old.update(started_us=102, finished_us=103)
                for run in old['documents'][target]['runs']:
                    run.update(bold=False, size=11, color=0xff000000)
                logs['r0'].insert(2, old)
            with self.subTest(mutation=mutation), self.assertRaises(AssertionError): document_history(logs, 2)

    def test_an_uncertain_document_attempt_cannot_be_replayed_under_another_revision(self):
        logs = copy.deepcopy(self.logs)
        first = copy.deepcopy(next(row for row in logs['w0'] if row['event'] == 'remote_attempt'))
        first.update(state='Unknown', revision='earlier-uncertain', started_us=8, finished_us=9)
        logs['w0'].insert(2, first)
        with self.assertRaisesRegex(AssertionError, 'more than once'): document_history(logs, 2)


if __name__ == '__main__':
    unittest.main()
