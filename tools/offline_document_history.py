"""Account for offline document intents, receipts, reader states and native formatting."""
import uuid

from document_model import ordered_pages, walk


def identity(insertion, extension):
    return '{' + str(uuid.UUID(bytes_le=bytes(insertion['guid']))).upper() + '},' + str(extension)


def characters(observed):
    actual = [(char, run['bold'], run['size'], run['color'])
              for run in observed['runs'] for char in run['text']]
    assert ''.join(row[0] for row in actual) == observed['text'], 'Document runs omit or duplicate text'
    return actual


def operation_kinds(events):
    kinds = tuple(events[0].get('document_kinds', ('insert', 'format')))
    assert kinds in (('insert', 'format'), ('insert', 'format', 'text')), 'Unknown document workload'
    return kinds


def document_history(logs, operations):
    documents = {}
    for actor, events in logs.items():
        if not actor.startswith('w'): continue
        assert events[0].get('document_operations') is True, 'Writer omitted document operations'
        kinds = operation_kinds(events)
        edits = [row for row in events if row['event'] == 'local_document_commit']
        assert [(row['operation'], row['kind']) for row in edits] == [
            (i, kind) for i in range(operations) for kind in kinds], 'Missing or duplicate document intent'
        ids = [row['id'] for row in edits]
        assert ids == sorted(set(ids)), 'Document intent IDs are duplicated or unordered'
        assert not set(ids) & {row['id'] for row in events if row['event'] == 'local_commit'}, 'Text and document intents share an ID'
        receipts = [row for row in events if row['event'] == 'document_receipt']
        reopened = [row for row in events if row['event'] == 'reopened_document_receipt']
        assert [row['id'] for row in receipts] == ids, 'Document receipt inventory differs'
        assert [(r['id'], r['revision']) for r in reopened] == [(r['id'], r['revision']) for r in receipts], 'Document receipts changed across reopen'
        linked = {}
        for intent, receipt in zip(edits, receipts, strict=True):
            attempts = [row for row in events if row['event'] == 'remote_attempt' and row['revision'] == receipt['revision']
                        and set(row.get('document_changes') or {}) == {intent['object']}]
            if not attempts and intent['kind'] == 'format':
                attempts = [row for row in events if row['event'] == 'remote_attempt' and row['state'] == 'Unknown'
                            and set(row.get('document_changes') or {}) == {intent['object']}]
            assert len(attempts) == 1, 'Document receipt lacks one publication attempt'
            attempt, = attempts
            assert attempt['state'] in ('Committed', 'Unknown'), 'Receipt identifies an unpublished document operation'
            assert intent['object'] in attempt['documents'] and attempt['document_changes'] == {intent['object']: attempt['documents'][intent['object']]}, 'Document publication changed another target'
            successful = [row for row in events if row['event'] == 'remote_attempt'
                          and row['state'] in ('Committed', 'Unknown')
                          and row.get('document_changes') == attempt['document_changes']]
            assert successful == [attempt], 'Document intent was published or attempted uncertainly more than once'
            assert intent['started_us'] <= attempt['started_us'] <= attempt['finished_us'] <= receipt['at_us'] and intent['started_us'] <= intent['finished_us'] <= receipt['at_us'], 'Document acknowledgement order is invalid'
            if attempt['state'] == 'Unknown':
                confirmed, observed = False, None
                for row in events:
                    if row['event'] == 'read': observed = row
                    if (row['event'] != 'remote_confirm' or row['state'] != 'Committed'
                            or receipt['revision'] not in row.get('revisions', {}).get(intent['space'], [])
                            or not attempt['finished_us'] <= row['started_us'] <= row['finished_us'] <= receipt['at_us']):
                        continue
                    if receipt['revision'] != attempt['revision']:
                        assert intent['kind'] == 'format' and attempt['revision'] not in row['revisions'][intent['space']], 'Replacement receipt did not retire the original attempt'
                        assert row.get('current_revisions', {}).get(intent['space']) == receipt['revision'], 'Effect receipt does not identify the confirmed current revision'
                        assert observed and observed['finished_us'] <= row['started_us'] and observed['text'] == row['text']
                        assert observed.get('documents', {}).get(intent['object']) == attempt['document_changes'][intent['object']], 'Effect confirmation differs from the uncertain formatting intent'
                    confirmed = True
                assert confirmed, 'Uncertain document publication lacks confirmation'
            else:
                assert receipt['revision'] == attempt['revision']
            linked[intent['id']] = {**attempt, 'acknowledged_us': receipt['at_us'], 'receipt_revision': receipt['revision']}
        for at in range(0, len(edits), len(kinds)):
            inserted, formatted, *replaced = edits[at:at + len(kinds)]
            number = inserted['operation']
            text = f'Document {actor}:{number} 🦀'
            insertion = inserted['insertion']
            target = identity(insertion, 2)
            assert target == inserted['object'] == formatted['object'], 'Dependent formatting addresses another object'
            assert inserted['space'] == formatted['space'] and inserted['text'] == formatted['text'] == insertion['text'] == text
            assert insertion['author'] == 'Offline document writer'
            if number % 2 == 0:
                assert insertion['placement'] == {'Outline': {'x': 144 + int(actor[1:]) * 240, 'y': 144 + number * 72}}, 'Outline placement differs from intent'
            else:
                assert insertion['placement'] == {'Paragraph': {'before': None}}
                assert insertion['parent'] == identity(edits[(number-1)*len(kinds)]['insertion'], 1), 'Paragraph lost its outline parent'
            assert formatted['range'] == [1, len(text.encode('utf-16-le')) // 2 - 2]
            assert formatted['attributes'] == [{'Bold': True}, {'FontSize': 18 + number % 9}, {'Color': [18, 52, 86]}]
            old = [(char, False, 11, 0xff000000) for char in text]
            if insertion.get('formats'):
                span, = insertion['formats']
                assert span['range'] == {'start': 0, 'end': len(text.encode('utf-16-le')) // 2}, 'Initial formatting range differs from workload'
                assert span['attributes'] == [{'FontSize': 13.5}, {'Color': [68, 85, 102]}], 'Initial formatting attributes differ from workload'
                old = [(char, False, 13.5, 0x665544) for char in text]
            new = [(char, True, 18 + number % 9, 0x563412) if 0 < i < len(text)-1 else old[i] for i, char in enumerate(text)]
            assert target not in documents, 'Two insertion intents share an object identity'
            created, changed = linked[inserted['id']], linked[formatted['id']]
            assert created['finished_us'] <= changed['started_us'], 'Formatting preceded its insertion'
            assert characters(created['documents'][target]) == old, 'Insertion publication differs from its local intent'
            assert characters(changed['documents'][target]) == new, 'Formatting publication differs from its local intent'
            states = {'insert': {'characters': old, 'attempt': created},
                      'format': {'characters': new, 'attempt': changed}}
            if replaced:
                replacement, = replaced
                end = len(text.encode('utf-16-le')) // 2
                assert replacement['object'] == target and replacement['space'] == inserted['space'], 'Text edit addresses another object'
                assert replacement['text'] == text and replacement['range'] == [end-3, end], 'Cross-run text range differs from workload'
                assert replacement['replacement'] == ' e\u0301🐈', 'Cross-run replacement differs from workload'
                final = new[:-2] + [(char, *new[-2][1:]) for char in replacement['replacement']]
                attempt = linked[replacement['id']]
                assert changed['finished_us'] <= attempt['started_us'], 'Text replacement preceded its formatting'
                assert characters(attempt['documents'][target]) == final, 'Text publication differs from its local intent'
                states['text'] = {'characters': final, 'attempt': attempt}
            documents[target] = {'insertion': insertion, 'space': inserted['space'], 'states': states}
    assert documents, 'No document operations were recorded'
    if any(events[0].get('document_graph') for events in logs.values()):
        for events in logs.values():
            assert events[0].get('document_graph') is True, 'Client omitted structural observations'
            before = None
            for row in events:
                if row['event'] not in ('read', 'document_read', 'remote_attempt'): continue
                observed = row.get('documents')
                assert isinstance(observed, dict), 'Snapshot omitted document text'
                graph = row.get('document_graph')
                assert isinstance(graph, dict), 'Snapshot omitted document graph'
                expected = {}
                for target, document in documents.items():
                    if target not in observed: continue
                    insertion = document['insertion']
                    outline = insertion['parent']
                    paragraph = identity(insertion, 1)
                    if 'Outline' in insertion['placement']:
                        outline = paragraph
                        paragraph = identity(insertion, 3)
                        expected[outline] = {'parent': insertion['parent'], 'children': [], 'content': [],
                                             'child_level': 1, 'position': insertion['placement']['Outline']}
                    assert outline in expected, 'Snapshot omitted the inserted outline'
                    expected[outline]['children'].append(paragraph)
                    expected[paragraph] = {'parent': outline, 'children': [], 'content': [target],
                                            'child_level': 1, 'position': None}
                assert graph == expected, 'Snapshot contains partial, reordered or invented paragraph structure'
                if row['event'] == 'remote_attempt':
                    assert before is not None, 'Publication omitted its observed source'
                    for image, delta in [('documents', 'document_changes'), ('document_graph', 'document_graph_changes')]:
                        expected_delta = {key: row[image].get(key) for key in before[image].keys() | row[image].keys()
                                          if before[image].get(key) != row[image].get(key)}
                        assert row.get(delta) == expected_delta, 'Publication diff omitted or invented a changed object'
                else:
                    before = row
    for actor, events in logs.items():
        previous = {}
        reads = [row for row in events if row['event'] in ('read', 'document_read') and row.get('documents') is not None]
        assert reads, f'{actor} did not observe document snapshots'
        assert any(row['documents'] for row in reads), f'{actor} never observed a created document object'
        for read in reads:
            observed = read['documents']
            assert set(previous) <= set(observed) <= set(documents), 'Reader lost an object or observed an unrecorded insertion'
            for target, document in documents.items():
                states = list(document['states'].values())
                if read['started_us'] > states[0]['attempt']['acknowledged_us']:
                    assert target in observed, 'Reader missed an acknowledged insertion'
                if target not in observed: continue
                actual = characters(observed[target])
                matches = [i for i, state in enumerate(states) if actual == state['characters']]
                assert len(matches) == 1, 'Reader observed partial or invented document content'
                current, = matches
                assert current >= previous.get(target, 0), 'Reader reverted document content'
                assert read['finished_us'] >= states[current]['attempt']['started_us'], 'Reader observed future document content'
                for i, state in enumerate(states):
                    if read['started_us'] > state['attempt']['acknowledged_us']:
                        assert current >= i, 'Reader missed acknowledged document content'
                previous[target] = current
    return documents


def verify_model(model, documents):
    children = {}
    for document in documents.values():
        insertion = document['insertion']
        if 'Outline' in insertion['placement']:
            children[identity(insertion, 1)] = [identity(insertion, 3)]
    for document in documents.values():
        insertion = document['insertion']
        if 'Paragraph' in insertion['placement']:
            children[insertion['parent']].append(identity(insertion, 1))
    found = set()
    for sid, _, revision, page in ordered_pages(model):
        nodes = revision['nodes']
        for target, node in walk(revision, page):
            if target not in documents: continue
            assert target not in found, 'Inserted text is reachable twice'
            found.add(target)
            expected = documents[target]
            insertion = expected['insertion']
            final = list(expected['states'].values())[-1]['characters']
            assert sid == expected['space'] and node['kind']['text'] == ''.join(char for char, *_ in final)
            object_id = identity(insertion, 1)
            assert object_id in nodes[insertion['parent']]['children'], 'Insertion lost its parent'
            paragraph = identity(insertion, 3) if 'Outline' in insertion['placement'] else object_id
            assert nodes[paragraph]['content'] == [target], 'Inserted paragraph content changed'
            if 'Outline' in insertion['placement']:
                position = insertion['placement']['Outline']
                assert all(nodes[object_id]['layout'][key] == position[key] for key in ('x', 'y')), 'Outline coordinates changed'
                assert nodes[object_id]['children'] == children[object_id], 'Inserted paragraph order changed'
    assert found == set(documents), 'Final model omitted an inserted object'


def verify_native(paragraphs, expected):
    from PIL import ImageColor
    by_text = {''.join(char for char, _ in paragraph): paragraph for paragraph in paragraphs}
    checks = 0
    for final in expected:
        actual = by_text[''.join(char for char, *_ in final)]
        for (char, style), (wanted, bold, size, color) in zip(actual, final, strict=True):
            assert char == wanted and bool(style.get('bold')) == bold, 'Native text or bold differs from intent'
            assert style.get('font_size', 11) == size, 'Native font size differs from intent'
            native_color = style.get('color', 'automatic')
            if color == 0xff000000:
                assert native_color in ('automatic', None), 'Native automatic color changed'
            else:
                assert ImageColor.getrgb(native_color) == (color & 255, (color >> 8) & 255, (color >> 16) & 255), 'Native color differs from intent'
            checks += 3
    return checks
