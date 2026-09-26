"""Traversal of the exported document's referenced revisions."""
import os
from pathlib import Path
import shutil
import subprocess


def _exporter():
    """The document exporter built with protected sections, copied aside because a workspace
    build without the feature overwrites target/debug/examples/document."""
    if 'ONESTORE_DOCUMENT' in os.environ:
        return Path(os.environ['ONESTORE_DOCUMENT'])
    root = Path(__file__).resolve().parent.parent
    subprocess.run(['cargo', 'build', '--quiet', '-p', 'notebook', '--features', 'protected',
                    '--example', 'document'], cwd=root, check=True)
    protected = root / 'target/debug/examples/document-protected'
    shutil.copy2(root / 'target/debug/examples/document', protected)
    return protected


EXPORTER = _exporter()
BRIDGE = Path(__file__).resolve().parent.parent / 'target/debug/onestore-diagnostic'

DEFAULT_CONTEXT = '{00000000-0000-0000-0000-000000000000},0'


def view(document, sid, context=DEFAULT_CONTEXT):
    space = document['spaces'][sid]
    rid = space['contexts'][context]
    return rid, space['revisions'][rid]


def ordered_pages(document):
    _, root = view(document, document['root'])
    section = root['nodes'][root['roots']['1']]
    if section['kind']['type'] == 'Encrypted':
        return
    assert section['kind']['type'] == 'Section'
    for series in section['children']:
        for space in root['nodes'][series]['spaces']:
            rid, page = view(document, space)
            manifest = page['nodes'][page['roots']['1']]
            assert manifest['kind']['type'] == 'Manifest'
            for oid in manifest['content']:
                assert page['nodes'][oid]['kind']['type'] == 'Page'
                yield space, rid, page, oid


def version_pages(document, sid, revision):
    manifest = revision['nodes'][revision['roots']['1']]['kind']
    context = manifest['history']
    if context is None:
        return
    _, history = view(document, sid, context)
    root = history['nodes'][history['roots']['1']]
    assert root['kind']['type'] == 'VersionHistory'
    for proxy_id in root['children']:
        proxy = history['nodes'][proxy_id]
        assert proxy['kind']['type'] == 'VersionProxy'
        context = proxy['kind']['context']
        rid, revision = view(document, sid, context)
        manifest = revision['nodes'][revision['roots']['1']]
        assert manifest['kind']['type'] == 'Manifest'
        for oid in manifest['content']:
            assert revision['nodes'][oid]['kind']['type'] == 'Page'
            yield context, rid, revision, oid, proxy


def walk(space, root):
    pending = [root]
    while pending:
        oid = pending.pop()
        node = space['nodes'][oid]
        yield oid, node
        pending.extend(reversed(node['structure'] + node['content'] + node['children']))
