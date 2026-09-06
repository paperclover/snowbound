"""Verify successful I/O inside overlapping SMB reader and writer guards."""
import json
from collections import Counter


class PendingOverlap(AssertionError):
    pass


def renames(request):
    return request['command'] == 17 and request.get('info_type') == 1 and request.get('info_class') == 10


def verify(events, phase=None):
    pending, files, peers, pairs = {}, {}, {}, {}
    progress = Counter()
    path_epoch = 0
    active_since = -1 if phase is None else None
    for index, event in enumerate(events):
        assert not event.get('trace_error'), 'Trace collection failed'
        assert not event.get('encrypted'), 'Encrypted traffic cannot establish I/O overlap'
        if phase is not None and event.get('control', {}).get('phase') == phase:
            active_since = index
        connection = event.get('connection')
        if event.get('opened'):
            peers[connection] = event['peer'][0]
        if event.get('closed'):
            files = {key: value for key, value in files.items() if key[0] != connection}
        if 'command' not in event:
            continue
        message = connection, event['message']
        if event['direction'] == 'request':
            if renames(event):
                files.clear()
                path_epoch += 1
            pending[message] = index, event, path_epoch
            if event['command'] == 6:
                files.pop((connection, event['file_id']), None)
            elif event['command'] == 10:
                file = files.get((connection, event['file_id']))
                if file:
                    for offset, length, flags in event['locks']:
                        if flags & 4:
                            file['locks'] = {at: lock for at, lock in file['locks'].items()
                                             if not offset <= at < offset + length}
            continue
        if event['status'] == '0x103':
            continue
        if message not in pending and event['command'] in (0, 18):
            continue
        request_index, request, request_epoch = pending.pop(message)
        if renames(request):
            files.clear()
            path_epoch += 1
        if event['status'] != '0x0':
            continue
        command = event['command']
        if command == 5:
            if request_epoch != path_epoch or any(renames(request) for _, request, _ in pending.values()):
                continue
            files[connection, event['file_id']] = {
                'path': request['path'].lower(), 'access': request['access'],
                'locks': {},
            }
            continue
        if command not in (8, 9, 10):
            continue
        key = connection, request['file_id']
        if key not in files:
            continue
        file = files[key]
        if command == 10:
            for offset, length, flags in request['locks']:
                if length != 1 or offset not in (0xfffffffb, 0xfffffffd):
                    continue
                if flags & 4:
                    file['locks'].pop(offset, None)
                else:
                    file['locks'][offset] = index, flags & 3
            continue
        if active_since is None or request_index <= active_since or not file['path'].endswith('synthetic.one') or request['length'] == 0:
            continue
        native = peers[connection].startswith('192.168.77.')
        progress[('native' if native else 'host', connection, 'read' if command == 8 else 'write')] += 1
        for other_key, other in files.items():
            if key[0] == other_key[0] or file['path'] != other['path']:
                continue
            reader, writer = (file, other) if command == 8 else (other, file)
            reader_key, writer_key = (key, other_key) if command == 8 else (other_key, key)
            reader_lock = reader['locks'].get(0xfffffffb)
            writer_lock = writer['locks'].get(0xfffffffd)
            # Only read-only handles distinguish a reader from a writer's own validation reads.
            if reader['access'] & 0x40000002 or not reader_lock or not writer_lock:
                continue
            if reader_lock[1] != 1 or writer_lock[1] != 2:
                continue
            if request_index <= max(reader_lock[0], writer_lock[0]):
                continue
            pair = reader_key + (reader_lock[0],) + writer_key + (writer_lock[0],)
            observed = pairs.setdefault(pair, {'read': 0, 'write': 0,
                'reader_peer': peers[reader_key[0]], 'writer_peer': peers[writer_key[0]]})
            observed['read' if command == 8 else 'write'] += 1
    assert active_since is not None, 'Requested trace phase was not observed'
    both = [pair for pair in pairs.values() if pair['read'] and pair['write']]
    native_writer_pairs = [pair for pair in both if pair['writer_peer'].startswith('192.168.77.')
                           and not pair['reader_peer'].startswith('192.168.77.')]
    rust_writer_pairs = [pair for pair in both if not pair['writer_peer'].startswith('192.168.77.')]
    if not both: raise PendingOverlap('No reader/writer guard pair performed both successful reads and writes while overlapping')
    if not native_writer_pairs: raise PendingOverlap('No active Rust reader overlapped successful native writes')
    if not rust_writer_pairs: raise PendingOverlap('No active reader overlapped successful Rust writes')
    return {'active_guard_pairs': len(both), 'active_native_writer_pairs': len(native_writer_pairs),
            'active_rust_writer_pairs': len(rust_writer_pairs),
            'overlapping_reads': sum(pair['read'] for pair in both),
            'overlapping_writes': sum(pair['write'] for pair in both),
            'connections': [{'kind': kind, 'connection': connection, 'operation': operation, 'count': count}
                            for (kind, connection, operation), count in sorted(progress.items())]}


if __name__ == '__main__':
    import sys
    with open(sys.argv[1]) as stream:
        result = verify(json.loads(line) for line in stream)
    print(json.dumps(result, indent=2))
