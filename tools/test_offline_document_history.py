import copy
import json
from pathlib import Path
import subprocess
import unittest
import uuid
from unittest.mock import patch

from offline_document_history import document_history, identity, verify_model, verify_native


def history(text_edits=False, graph=False, boundaries=False):
    if boundaries: text_edits, graph = True, True
    logs = {'w0': [{'event': 'ready', 'document_operations': True, 'document_kinds': ['insert', 'format', 'text'] if text_edits else ['insert', 'format']}]}
    if boundaries: logs['w0'][0]['document_kinds'] += ['split', 'right_text', 'join']
    kinds = logs['w0'][0]['document_kinds']
    events = logs['w0']
    if graph: events[0]['document_graph'] = True
    structure = {}
    observed = {}
    previous = None
    for operation in range(2):
        insertion = {'guid': list(uuid.UUID(int=operation+1).bytes_le), 'text': f'Document w0:{operation} 🦀',
                     'parent': 'page' if operation == 0 else identity(previous, 1), 'author': 'Offline document writer',
                     'placement': {'Outline': {'x': 144, 'y': 144}} if operation == 0 else {'Paragraph': {'before': None}}}
        previous = insertion
        target = identity(insertion, 2)
        end = len(insertion['text'].encode('utf-16-le'))//2
        split = {'guid': list(uuid.UUID(int=100+operation).bytes_le), 'text': target,
                 'offset': end-2, 'author': insertion['author'], 'created': 1}
        right, right_paragraph = identity(split, 2), identity(split, 1)
        for step, kind in enumerate(kinds):
            timestamp = 10 + operation*max(3, len(kinds))*10 + step*10
            local_id = operation*max(3, len(kinds)) + step + 2
            event = {'event': 'local_document_commit', 'id': local_id, 'operation': operation, 'kind': kind,
                     'space': 'space', 'object': target, 'text': insertion['text'], 'insertion': insertion if step == 0 else None,
                     'range': [1, len(insertion['text'].encode('utf-16-le'))//2-2],
                     'attributes': [{'Bold': True}, {'FontSize': 18+operation}, {'Color': [18, 52, 86]}],
                     'started_us': timestamp-2, 'finished_us': timestamp-1}
            if kind == 'text':
                end = len(insertion['text'].encode('utf-16-le'))//2
                event.update(range=[end-3, end], replacement=' e\u0301🐈')
            if boundaries: event['document'] = target
            if kind == 'split': event.update(split=split, range=[end-2, end-2])
            if kind == 'right_text': event.update(object=right, range=[0, 2], replacement='B🦋')
            if kind == 'join': event['joined'] = [target, right]
            events.append(event)
            prior_observed = copy.deepcopy(observed)
            prior_structure = copy.deepcopy(structure)
            if graph:
                events.append({'event': 'read', 'started_us': timestamp-1, 'finished_us': timestamp,
                               'documents': copy.deepcopy(observed), 'document_graph': prior_structure})
                if step == 0:
                    parent, paragraph = insertion['parent'], identity(insertion, 1)
                    if operation == 0:
                        parent, paragraph = paragraph, identity(insertion, 3)
                        structure[parent] = {'parent': insertion['parent'], 'children': [], 'content': [],
                                             'child_level': 1, 'position': {'x': 144, 'y': 144}}
                    structure[parent]['children'].append(paragraph)
                    structure[paragraph] = {'parent': parent, 'children': [], 'content': [target],
                                             'child_level': 1, 'position': None}
            if kind == 'split':
                boundary = len(insertion['text'])-1
                old = observed[target]
                left_runs, right_runs = old['runs'][:boundary], old['runs'][boundary:]
                observed[target] = {'text': old['text'][:boundary], 'runs': left_runs}
                observed[right] = {'text': old['text'][boundary:], 'runs': right_runs}
                structure[parent]['children'].insert(structure[parent]['children'].index(paragraph)+1, right_paragraph)
                structure[right_paragraph] = {**copy.deepcopy(structure[paragraph]), 'content': [right]}
            elif kind == 'right_text':
                runs = [{**observed[right]['runs'][0], 'text': c} for c in 'B🦋'] + observed[right]['runs'][2:]
                observed[right] = {'text': 'B🦋🐈', 'runs': runs}
            elif kind == 'join':
                observed[target]['text'] += observed[right]['text']
                observed[target]['runs'] += observed[right]['runs']
                del observed[right]
                structure[parent]['children'].remove(right_paragraph)
                del structure[right_paragraph]
            else:
                value = insertion['text'].replace(' 🦀', ' e\u0301🐈') if kind == 'text' else insertion['text']
                runs = []
                for index, char in enumerate(value):
                    selected = index > 0 and (kind == 'text' or (kind == 'format' and index < len(value)-1))
                    runs.append({'text': char, 'bold': selected, 'size': 18+operation if selected else 11,
                                 'color': 0x563412 if selected else 0xff000000})
                observed[target] = {'text': value, 'runs': runs}
            revision = f'revision-{local_id}'
            events.append({'event': 'remote_attempt', 'revision': revision, 'state': 'Committed',
                           'document_changes': {key: copy.deepcopy(observed.get(key)) for key in observed.keys() | prior_observed.keys()
                                                if observed.get(key) != prior_observed.get(key)},
                           'documents': copy.deepcopy(observed), 'started_us': timestamp, 'finished_us': timestamp+1})
            if graph:
                events[-1].update(document_graph=copy.deepcopy(structure), document_graph_changes={
                    key: copy.deepcopy(structure.get(key)) for key in structure.keys() | prior_structure.keys() if prior_structure.get(key) != structure.get(key)})
            events.append({'event': 'document_receipt', 'id': local_id, 'revision': revision, 'at_us': timestamp+2})
            if text_edits:
                events.append({'event': 'read', 'started_us': timestamp+3, 'finished_us': timestamp+4, 'documents': copy.deepcopy(observed), **({'document_graph': copy.deepcopy(structure)} if graph else {})})
    events.extend({'event': 'reopened_document_receipt', 'id': row['id'], 'revision': row['revision']}
                  for row in list(events) if row['event'] == 'document_receipt')
    read = {'event': 'read', 'started_us': max(100, 20*len(kinds)+10), 'finished_us': max(101, 20*len(kinds)+11), 'documents': observed, **({'document_graph': structure} if graph else {})}
    events.extend([read, {'event': 'done'}])
    logs['r0'] = [{'event': 'ready', **({'document_graph': True} if graph else {})}, copy.deepcopy(read), {'event': 'done'}]
    return logs


class DocumentHistoryTests(unittest.TestCase):
    def setUp(self):
        self.logs = history()

    def test_document_receipts_and_reader_states_match_the_intents(self):
        documents = document_history(self.logs, 2)
        self.assertEqual(len(documents), 2)
        expected = [list(row['states'].values())[-1]['characters'] for row in documents.values()]
        paragraphs = [[(char, {'bold': bold, 'font_size': size, 'color': 'automatic' if color == 0xff000000 else '#123456'})
                       for char, bold, size, color in row] for row in expected]
        self.assertEqual(verify_native(paragraphs, expected), sum(len(row)*3 for row in expected))
        paragraphs[0][1][1]['font_size'] = 19
        with self.assertRaisesRegex(AssertionError, 'font size'): verify_native(paragraphs, expected)

    def test_structural_snapshots_and_publication_differences_are_complete(self):
        baseline = history(text_edits=True, graph=True)
        self.assertEqual(len(document_history(baseline, 2)), 2)
        for mutation in ('missing-graph', 'missing-client', 'reorder', 'duplicate', 'reparent',
                         'missing-content', 'invented', 'level', 'position', 'delta'):
            logs = copy.deepcopy(baseline)
            row = logs['r0'][1]
            outline, first, second = list(row['document_graph'])
            graph = row['document_graph']
            if mutation == 'missing-graph': del row['document_graph']
            elif mutation == 'missing-client': del logs['r0'][0]['document_graph']
            elif mutation == 'reorder': graph[outline]['children'].reverse()
            elif mutation == 'duplicate': graph[outline]['children'].append(first)
            elif mutation == 'reparent': graph[second]['parent'] = first
            elif mutation == 'missing-content': graph[first]['content'] = []
            elif mutation == 'invented': graph['unrecorded'] = copy.deepcopy(graph[first])
            elif mutation == 'level': graph[first]['child_level'] = 2
            elif mutation == 'position': graph[outline]['position']['x'] = 145
            else:
                next(row for row in logs['w0'] if row['event'] == 'remote_attempt')['document_graph_changes'] = {}
            with self.subTest(mutation=mutation), self.assertRaises(AssertionError):
                document_history(logs, 2)

    def test_production_cache_workload_matches_the_independent_history_model(self):
        result = subprocess.run(['cargo', 'test', '--locked', '-p', 'onestore-offline', '--features', 'smb',
                                 '--example', 'smb_offline_client',
                                 'tests::document_workload_retains_dependencies_and_receipts_across_reopen',
                                 '--', '--exact', '--nocapture'],
                                cwd=Path(__file__).resolve().parent.parent, capture_output=True, text=True, timeout=120)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        events = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
        documents = document_history({'w0': events}, 2)
        self.assertEqual(sum(len(document['states']) for document in documents.values()), 20)
        self.assertEqual(sum(row['event'] == 'remote_attempt' and row['state'] == 'Unknown' for row in events), 12)
        self.assertEqual(sum(row['event'] == 'remote_confirm' and row['state'] == 'Committed' for row in events), 12)
        document = next(iter(documents.values()))
        states = document['states']
        for phase, next_phase, stale in [('nest', 'unnest', 'right_text'), ('delete', None, 'tail_split')]:
            read = copy.deepcopy(states[stale]['attempt'])
            started = states[phase]['attempt']['acknowledged_us'] + 1
            if next_phase:
                self.assertLess(started + 1, states[next_phase]['attempt']['started_us'])
            read.update(event='read', started_us=started, finished_us=started + 1)
            logs = {'w0': events, 'r0': [{'event': 'ready', 'document_graph': True}, read, {'event': 'done'}]}
            with self.subTest(phase=phase), self.assertRaisesRegex(AssertionError, 'missed acknowledged'):
                document_history(logs, 2)
        for mutation in ('nest-intent', 'unnest-intent', 'delete-intent', 'tail-boundary',
                         'nest-children', 'delete-resurrection', 'graph-delta', 'extra-attempt'):
            changed = copy.deepcopy(events)
            intents = {row['kind']: row for row in changed if row['event'] == 'local_document_commit' and row['operation'] == 0}
            attempts = {kind: next(row for row in changed if row['event'] == 'remote_attempt' and row['revision'] == state['attempt']['revision'])
                        for kind, state in states.items()}
            if mutation == 'nest-intent':
                intents['nest']['tree']['placement']['Move']['parent'] = intents['unnest']['tree']['placement']['Move']['parent']
            elif mutation == 'unnest-intent':
                intents['unnest']['tree']['placement']['Move']['before'] = intents['unnest']['tree']['object']
            elif mutation == 'delete-intent':
                intents['delete']['tree']['object'] = intents['nest']['tree']['object']
            elif mutation == 'tail-boundary':
                intents['tail_split']['split']['offset'] += 1
            elif mutation == 'nest-children':
                parent = intents['nest']['tree']['placement']['Move']['parent']
                attempts['nest']['document_graph'][parent]['children'] = []
            elif mutation == 'delete-resurrection':
                tail = intents['delete']['object']
                attempts['delete']['documents'][tail] = attempts['tail_split']['documents'][tail]
            elif mutation == 'graph-delta':
                attempts['nest']['document_graph_changes'] = {}
            else:
                extra = copy.deepcopy(attempts['nest'])
                extra.update(revision='unrecorded-tree-revision', state='Committed')
                changed.append(extra)
            with self.subTest(mutation=mutation), self.assertRaises(AssertionError):
                document_history({'w0': changed}, 2)

    def test_boundary_histories_retain_every_intermediate_graph_and_retired_identity(self):
        baseline = history(boundaries=True)
        documents = document_history(baseline, 2)
        for document in documents.values():
            self.assertEqual(list(document['states']), ['insert', 'format', 'text', 'split', 'right_text', 'join'])
            self.assertEqual(''.join(c for c, *_ in document['states']['join']['characters']),
                             document['insertion']['text'].replace('🦀', 'B🦋🐈'))
        for mutation in ('missing-suffix', 'half-join', 'resurrected', 'reorder', 'missing-removal',
                         'wrong-split', 'wrong-dependent-target', 'wrong-join', 'stale', 'future', 'duplicate-attempt'):
            logs = copy.deepcopy(baseline)
            split = next(row for row in logs['w0'] if row.get('kind') == 'split')
            right, right_paragraph = identity(split['split'], 2), identity(split['split'], 1)
            split_read = next(row for row in logs['w0'] if row['event'] == 'read' and row['started_us'] == 43)
            joined_read = next(row for row in logs['w0'] if row['event'] == 'read' and row['started_us'] == 63)
            if mutation == 'missing-suffix': del split_read['documents'][right]
            elif mutation == 'half-join':
                joined_read['documents'][right] = copy.deepcopy(split_read['documents'][right])
            elif mutation == 'resurrected':
                joined_read['document_graph'][right_paragraph] = copy.deepcopy(split_read['document_graph'][right_paragraph])
            elif mutation == 'reorder':
                next(node for node in split_read['document_graph'].values() if node['position'])['children'].reverse()
            elif mutation == 'missing-removal':
                join_attempt = next(row for row in logs['w0'] if row['event'] == 'remote_attempt' and row['started_us'] == 60)
                del join_attempt['document_changes'][right]
            elif mutation == 'wrong-split': split['split']['offset'] -= 1
            elif mutation == 'wrong-dependent-target':
                next(row for row in logs['w0'] if row.get('kind') == 'right_text')['object'] = split['object']
            elif mutation == 'wrong-join':
                next(row for row in logs['w0'] if row.get('kind') == 'join')['joined'].reverse()
            elif mutation == 'stale':
                joined_read.update(documents=copy.deepcopy(split_read['documents']), document_graph=copy.deepcopy(split_read['document_graph']))
            elif mutation == 'future': split_read.update(started_us=0, finished_us=1)
            else:
                attempt = copy.deepcopy(next(row for row in logs['w0'] if row['event'] == 'remote_attempt' and row['started_us'] == 40))
                attempt.update(state='Unknown', revision='duplicate')
                logs['w0'].insert(1, attempt)
            with self.subTest(mutation=mutation), self.assertRaises(AssertionError):
                document_history(logs, 2)

    def test_no_reader_phase_can_omit_an_acknowledged_text_or_paragraph(self):
        baseline = history(boundaries=True)
        for index, row in enumerate(baseline['w0']):
            if row['event'] != 'read': continue
            for field in ('documents', 'document_graph'):
                for target in row[field]:
                    changed = copy.deepcopy(baseline)
                    del changed['w0'][index][field][target]
                    with self.subTest(read=index, field=field, target=target), self.assertRaises(AssertionError):
                        document_history(changed, 2)
        extra = copy.deepcopy(next(row for row in baseline['w0'] if row['event'] == 'remote_attempt'))
        extra.update(revision='unreceipted', started_us=1000, finished_us=1001,
                     document_changes={'unrecorded': {'text': 'extra'}})
        baseline['w0'].insert(-1, extra)
        with self.assertRaisesRegex(AssertionError, 'lacks its recorded intent and receipt'):
            document_history(baseline, 2)

    def test_unknown_boundary_receipts_require_the_original_revision(self):
        for kind in ('split', 'join'):
            logs = history(boundaries=True)
            events = logs['w0']
            intent = next(row for row in events if row.get('kind') == kind)
            receipt = next(row for row in events if row['event'] == 'document_receipt' and row['id'] == intent['id'])
            attempt = next(row for row in events if row['event'] == 'remote_attempt' and row['revision'] == receipt['revision'])
            attempt['state'] = 'Unknown'
            confirmation = {'event': 'remote_confirm', 'state': 'Committed',
                            'started_us': attempt['finished_us'], 'finished_us': receipt['at_us'],
                            'revisions': {'space': [receipt['revision']]}}
            events.insert(events.index(receipt), confirmation)
            document_history(logs, 2)
            confirmation['revisions']['space'] = ['later-revision']
            receipt['revision'] = 'later-revision'
            next(row for row in events if row['event'] == 'reopened_document_receipt' and row['id'] == intent['id'])['revision'] = 'later-revision'
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                document_history(logs, 2)

    def test_cross_run_text_requires_complete_ordered_states_and_exact_receipts(self):
        logs = history(True)
        documents = document_history(logs, 2)
        for document in documents.values():
            self.assertEqual(list(document['states']), ['insert', 'format', 'text'])
            final = document['states']['text']['characters']
            self.assertEqual(''.join(char for char, *_ in final), document['insertion']['text'].replace(' 🦀', ' e\u0301🐈'))
        for event, field, value in [('local_document_commit', 'replacement', 'wrong'),
                                    ('local_document_commit', 'range', [0, 1]),
                                    ('document_receipt', 'revision', 'wrong')]:
            changed = copy.deepcopy(logs)
            row = next(row for row in changed['w0'] if row['event'] == event and
                       (row.get('kind') == 'text' or row.get('id') == 4))
            row[field] = value
            with self.subTest(event=event, field=field), self.assertRaises(AssertionError):
                document_history(changed, 2)
        changed = copy.deepcopy(logs)
        read = changed['r0'][1]
        target = next(iter(read['documents']))
        read['documents'][target]['runs'][-1]['bold'] = False
        with self.assertRaisesRegex(AssertionError, 'partial or invented'):
            document_history(changed, 2)
        prior = next(row for row in logs['w0'] if row['event'] == 'read' and row['started_us'] == 23)
        changed = copy.deepcopy(logs)
        changed['r0'][1]['documents'][target] = copy.deepcopy(prior['documents'][target])
        with self.assertRaisesRegex(AssertionError, 'missed acknowledged'):
            document_history(changed, 2)

    def test_missing_intents_receipts_or_reopen_records_are_rejected(self):
        for name in ('local_document_commit', 'document_receipt', 'reopened_document_receipt', 'remote_attempt'):
            logs = copy.deepcopy(self.logs)
            events = logs['w0']
            events.remove(next(row for row in events if row['event'] == name))
            with self.subTest(event=name), self.assertRaises(AssertionError): document_history(logs, 2)

    def test_formatted_insertions_are_atomic_in_every_observed_snapshot(self):
        for events in self.logs.values():
            for row in events:
                insertion = row.get('insertion')
                if insertion:
                    insertion['formats'] = [{'guid': list(uuid.uuid4().bytes_le),
                        'range': {'start': 0, 'end': len(insertion['text'].encode('utf-16-le')) // 2},
                        'attributes': [{'FontSize': 13.5}, {'Color': [68, 85, 102]}]}]
                for key in ('documents', 'document_changes'):
                    for document in (row.get(key) or {}).values():
                        for run in document['runs']:
                            if run['size'] == 11:
                                run['size'] = 13.5
                                run['color'] = 0x665544
        documents = document_history(self.logs, 2)
        expected = [list(row['states'].values())[-1]['characters'] for row in documents.values()]
        paragraphs = [[(char, {'bold': bold, 'font_size': size,
                       'color': '#123456' if color == 0x563412 else '#445566'})
                       for char, bold, size, color in row] for row in expected]
        self.assertEqual(verify_native(paragraphs, expected), sum(len(row)*3 for row in expected))
        baseline = copy.deepcopy(self.logs)
        for event, key in [('remote_attempt', 'document_changes'), ('read', 'documents')]:
            for field, value in [('size', 11), ('color', 0xff000000)]:
                logs = copy.deepcopy(baseline)
                row = next(row for row in logs['w0'] if row['event'] == event)
                next(iter(row[key].values()))['runs'][0][field] = value
                with self.subTest(event=event, field=field), self.assertRaises(AssertionError):
                    document_history(logs, 2)

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
        self.assertEqual(result[target]['states']['format']['attempt']['receipt_revision'], 'current-revision')
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
            nodes[target]['kind'] = {'text': ''.join(char for char, *_ in list(document['states'].values())[-1]['characters'])}
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
