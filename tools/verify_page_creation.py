"""Compare created pages with native XML and cold-reopened object graphs."""
import argparse
from copy import deepcopy
import json
from pathlib import Path
import runpy
import shutil
import subprocess
from tempfile import TemporaryDirectory
from uuid import UUID

from document_model import EXPORTER, ordered_pages, view

compare_document = runpy.run_path(str(Path(__file__).with_name('verify-document.py')))['compare']


def compare(source, capture, *, refresh_metadata_levels=False):
    files = list(source.glob('*.one'))
    assert len(files) == 1
    with TemporaryDirectory() as temporary:
        temporary = Path(temporary)
        native = temporary / 'native/read'
        shutil.copytree(capture / 'read', native)
        notebook = temporary / 'notebook'
        notebook.mkdir()
        (notebook / files[0].name).symlink_to(files[0].resolve())
        compare_document(notebook, native)
        models = []
        for ordinal, file in enumerate((files[0], capture / 'notebook' / files[0].name)):
            output = temporary / str(ordinal)
            subprocess.run([EXPORTER, file, output], check=True)
            models.append(json.loads((output / 'document.json').read_text()))
        before, after = models
        pages = list(ordered_pages(before))
        metadata = {}
        for page_sid, _, page_view, _ in pages:
            # Native uses the space GUID when it differs from the page-node GUID.
            guid = UUID(page_sid.split('}')[0].strip('{')).bytes_le
            salt = UUID('22a8c031-3600-42ee-b714-d7acda2435e8').bytes_le
            oid = '{' + str(UUID(bytes_le=bytes(a ^ b for a, b in zip(guid, salt, strict=True)))).upper() + '},1'
            metadata[oid] = (page_sid, page_view['nodes'][page_view['roots']['2']])
        assert [(sid, page) for sid, _, _, page in pages] == [
            (sid, page) for sid, _, _, page in ordered_pages(after)]
        assert before['root'] == after['root']
        assert before['spaces'].keys() == after['spaces'].keys()
        filled = refreshed = 0
        for sid in before['spaces']:
            _, original = view(before, sid)
            _, observed = view(after, sid)
            expected = deepcopy(original['nodes'])
            if sid == before['root']:
                for series_id in original['nodes'][original['roots']['1']]['children']:
                    series = expected[series_id]
                    existing = [p for p in series['extra'][0] if p['id'] == 0x24003442]
                    added = [p for p in observed['nodes'][series_id]['extra'][0] if p['id'] == 0x24003442]
                    if existing and refresh_metadata_levels:
                        for copy in existing[0]['value']['Objects']:
                            cached = expected[copy]['kind']
                            assert cached['type'] == 'Metadata'
                            level = metadata[copy][1]['kind']['level']
                            refreshed += cached['level'] != level
                            cached['level'] = level
                    if existing or not added:
                        continue
                    copies = []
                    for oid, (page_sid, node) in metadata.items():
                        if page_sid not in series['spaces']:
                            continue
                        assert oid not in expected
                        expected[oid] = node
                        copies.append(oid)
                        filled += 1
                    assert len(added) == 1
                    assert sorted(copies) == sorted(added[0]['value']['Objects'])
                    series['extra'][0].append(added[0])
            assert original['roots'] == observed['roots']
            assert expected == observed['nodes'], sid
        print(f'Passed: {len(pages)} ordered pages and preserved active graphs; {filled} missing section metadata copies filled natively; {refreshed} section metadata levels refreshed')
        return refreshed


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('capture', type=Path)
    args = parser.parse_args()
    compare(args.source, args.capture)
