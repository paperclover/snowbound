"""Independent append-intent accounting across an abrupt storage interruption."""
from collections import Counter
import re
from document_model import ordered_pages, walk


def active_text(model):
    (_, _, revision, page), = ordered_pages(model)
    text, = [node['kind']['text'] for _, node in walk(revision, page)
             if node['kind']['type'] == 'RichText' and node['kind']['text'].startswith('Concurrent edits:')]
    return text


def verify_text(baseline, current, logs):
    events = [event for rows in logs.values() for event in rows]
    intents = {event['token'] for event in events if event['event'] == 'intent'}
    possible = set()
    versions = {baseline}
    predecessors = set()
    for rows in logs.values():
        outcomes = {event['attempt']: event for event in rows if event['event'] in ('commit', 'retry', 'commit_error')}
        for event in rows:
            if event['event'] != 'intent': continue
            outcome = outcomes.get(event['attempt'])
            if outcome is None or outcome['event'] == 'commit' or (outcome['event'] == 'commit_error' and outcome['state'] != 'NotCommitted'):
                possible.add(event['token'])
                assert event['replacement'] == event['token'] and event['range'] == [len(event['before'].encode('utf-16-le')) // 2] * 2
                predecessors.add(event['before'])
                versions.add(event['before'] + event['token'])
    assert predecessors <= versions, 'A publication does not follow recorded history'
    acknowledged = [event for event in events if event['event'] == 'commit' or
                    (event['event'] == 'commit_error' and event['state'] == 'Committed')]
    assert len({event['token'] for event in acknowledged}) == len(acknowledged), 'An edit was acknowledged twice'

    def tokens(text):
        assert text.startswith(baseline), 'Previously retained content changed'
        assert text in versions, 'Text is not a recorded publication result'
        suffix = text[len(baseline):]
        found = re.findall(r' \[w\d+:\d+\]', suffix)
        assert ''.join(found) == suffix, 'Unexpected or partially persisted text'
        assert all(count == 1 for count in Counter(found).values()), 'An edit was replayed twice'
        assert set(found) <= intents, 'Text has no matching editing intent'
        return set(found)

    persisted = tokens(current)
    assert persisted <= possible, 'A definitively uncommitted edit appeared in storage'
    assert {event['token'] for event in acknowledged} <= persisted, 'A durability-acknowledged edit was lost'
    reads = [event for event in events if event['event'] == 'read']
    for read in reads:
        observed = tokens(read['text'])
        assert {event['token'] for event in acknowledged if event['finished_us'] < read['started_us']} <= observed, 'A read missed an acknowledged edit'
        assert not {event['token'] for event in acknowledged if event['started_us'] > read['finished_us']} & observed, 'A read observed a future edit'
    return {'acknowledged': len(acknowledged), 'retained': len(persisted),
            'unacknowledged_retained': sorted(persisted - {event['token'] for event in acknowledged}),
            'reads': len(reads), 'text': current}
