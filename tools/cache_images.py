"""The cache's working image, stored as the byte ranges where it differs from the base."""
import struct


def working(connection):
    base, patch = connection.execute('SELECT base, working FROM replica WHERE id=1').fetchone()
    if not patch: return base
    length, = struct.unpack_from('<Q', patch)
    image = bytearray(base[:length].ljust(length, b'\0'))
    at = 8
    while at < len(patch):
        offset, size = struct.unpack_from('<QQ', patch, at)
        at += 16
        assert at + size <= len(patch) and offset + size <= length, 'Damaged working image'
        image[offset:offset + size] = patch[at:at + size]
        at += size
    return bytes(image)
