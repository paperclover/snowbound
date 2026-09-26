"""Reading a notebook cache (schema 15) from outside: its section images, stored as 16 KiB
chunk rows, and the state of its write-ahead log."""
import struct


def image(connection, table='base'):
    """The `base` image the queued edits apply to, or the observed `remote` image (None
    unless a conflict or uncertain attempt holds one)."""
    chunks = connection.execute(f'SELECT chunk, bytes FROM {table} ORDER BY chunk').fetchall()
    assert [chunk for chunk, _ in chunks] == list(range(len(chunks))), 'Damaged cached image'
    return b''.join(bytes(data) for _, data in chunks) or None


def wal_commits(path):
    """Commits in `<cache>-wal` since its last reset: frames whose header records the
    database size after them, among the frames carrying the log's current salts. Frames
    after the last commit belong to a transaction that has not committed."""
    try:
        data = path.with_name(path.name + '-wal').read_bytes()
    except FileNotFoundError:
        return 0, 0
    if len(data) < 32:
        return 0, 0
    magic, _, page_size, _, salt1, salt2 = struct.unpack_from('>IIIIII', data)
    assert magic in (0x377f0682, 0x377f0683), 'Not a SQLite write-ahead log'
    commits = pending = 0
    at = 32
    while at + 24 + page_size <= len(data):
        _, committed, frame1, frame2 = struct.unpack_from('>IIII', data, at)
        if (frame1, frame2) != (salt1, salt2):
            break
        if committed:
            commits, pending = commits + 1, 0
        else:
            pending += 1
        at += 24 + page_size
    return commits, pending
