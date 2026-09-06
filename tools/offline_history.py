"""Verify local intent acknowledgements separately from the remote publication chain."""
import re


def tokens(text):
    assert text.startswith('Concurrent edits:'), 'Unexpected append prefix'
    tail = text[len('Concurrent edits:'):]
    found = re.findall(r' \[w[0-9]+:[0-9]+\]', tail)
    assert ''.join(found) == tail and len(set(found)) == len(found), 'Malformed or duplicated append history'
    return found


def publication_links(logs, operations, partial=False):
    local = {}
    for actor, events in logs.items():
        assert events and events[0]['event'] == 'ready', 'Missing client start'
        assert partial or events[-1]['event'] == 'done', 'Incomplete client log'
        if not actor.startswith('w'): continue
        assert events[0].get('offline') is True, 'Expected an offline writer'
        edits = [event for event in events if event['event'] == 'local_commit']
        assert [event['operation'] for event in edits] == list(range(len(edits))), 'Missing or duplicate local acknowledgement'
        assert len(edits) <= operations and (partial or len(edits) == operations), 'Local operation count differs'
        assert len({event['id'] for event in edits}) == len(edits), 'Duplicate local intent ID'
        assert [event['id'] for event in edits] == sorted(event['id'] for event in edits), 'Local IDs went backwards'
        for event in edits:
            token = f' [{actor}:{event["operation"]}]'
            assert event['token'] == token and token not in local, 'Local token differs from its operation'
            assert event['started_us'] <= event['finished_us'], 'Invalid local acknowledgement interval'
            prior = tokens(event['before'])
            own = [item for item in prior if item.startswith(f' [{actor}:')]
            assert own == [f' [{actor}:{i}]' for i in range(event['operation'])], 'Local view lost or invented its own edit'
            local[token] = event
    for event in local.values():
        for token in tokens(event['before']):
            assert token in local and local[token]['started_us'] <= event['finished_us'], 'Local view invented a future token'
    links = {}
    seen_revisions = set()
    for actor, events in logs.items():
        if not actor.startswith('w'): continue
        edits = {event['id']: event for event in events if event['event'] == 'local_commit'}
        receipts = [event for event in events if event['event'] == 'remote_receipt']
        assert len({event['id'] for event in receipts}) == len(receipts), 'Duplicate remote receipt'
        assert set(event['id'] for event in receipts) <= set(edits), 'Receipt lacks a local intent'
        assert partial or len(receipts) == len(edits), 'Local success lacks remote acknowledgement'
        reopened = [event for event in events if event['event'] == 'reopened_receipt']
        if not partial:
            assert [(event['id'], event['revision']) for event in reopened] == [(event['id'], event['revision']) for event in receipts], 'Receipt changed across reopen'
        for receipt in receipts:
            intent = edits[receipt['id']]
            attempts = [event for event in events if event['event'] == 'remote_attempt' and event['revision'] == receipt['revision']]
            assert len(attempts) == 1, 'Receipt does not identify one publication attempt'
            attempt, = attempts
            assert attempt['state'] in ('Committed', 'Unknown'), 'Receipt identifies a proven-unpublished attempt'
            if attempt['state'] == 'Unknown':
                assert all(field in attempt and field in intent for field in ('space', 'object')), 'Uncertain target identity is missing'
                assert attempt['space'] == intent['space'] and attempt['object'] == intent['object'], 'Confirmation identifies another target'
                confirmed = [event for event in events if event['event'] == 'remote_confirm'
                             and event['state'] == 'Committed'
                             and receipt['revision'] in event.get('revisions', {}).get(intent['space'], [])
                             and event.get('text', '').startswith(attempt['after'])
                             and attempt['finished_us'] <= event['started_us'] <= event['finished_us'] <= receipt['at_us']]
                assert confirmed, 'Uncertain publication lacks a successful retained-revision confirmation'
                assert not any(event['event'] == 'remote_attempt' and event['started_us'] >= attempt['finished_us']
                               and event['after'] == event['before'] + intent['token'] for event in events), 'An uncertain intent was replayed'
            assert receipt['revision'] not in seen_revisions, 'A revision was acknowledged twice'
            seen_revisions.add(receipt['revision'])
            assert intent['started_us'] <= attempt['started_us'] <= attempt['finished_us'] <= receipt['at_us'], 'Receipt precedes its publication'
            assert attempt['after'] == attempt['before'] + intent['token'], 'Remote publication differs from local intent'
            tokens(attempt['after'])
            assert attempt['before'] not in links, 'Remote publications branched from the same content'
            event = {**attempt, 'event': 'commit', 'operation': intent['operation'], 'token': intent['token'], 'finished_us': receipt['at_us']}
            links[attempt['before']] = event, attempt['after']
    return links
