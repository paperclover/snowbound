#!/usr/bin/env python3
"""Edit disposable notebook copies and check every changed document object."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile

from document_model import DEFAULT_CONTEXT, EXPORTER, ordered_pages, view, walk

ROOT = Path(__file__).resolve().parent.parent
EDITOR = ROOT / 'target/debug/examples/random_edit'


def export(path):
    with tempfile.TemporaryDirectory() as temporary:
        output = Path(temporary) / 'model'
        subprocess.run([EXPORTER, path, output], check=True)
        return (json.loads((output / 'document.json').read_text()),
                json.loads((output / 'text.json').read_text()),
                sorted(hashlib.sha256(p.read_bytes()).hexdigest() for p in (output / 'assets').iterdir()))


def verify(before, after, record):
    old, old_text, old_assets = before
    new, new_text, new_assets = after
    assert old_assets == new_assets, 'An edit changed embedded payload bytes'
    sid, oid = record['space'], record['object']
    old_rid, previous = view(old, sid)
    new_rid, current = view(new, sid)
    expected_runs = copy.deepcopy(old_text[sid][old_rid][oid])
    runs = previous['nodes'][oid]['kind']['runs']
    start, end = record['range']
    selected = max(i for i, run in enumerate(runs) if run['start'] <= start and end <= run['end'])
    offset = runs[selected]['start']
    assert record['run_start'] == offset and record['run_before'] == expected_runs[selected]['text'], 'The CLI logged another insertion run'
    encoded = expected_runs[selected]['text'].encode('utf-16-le')
    prefix, suffix = encoded[:(start - offset) * 2], encoded[(end - offset) * 2:]
    expected_runs[selected]['text'] = prefix.decode('utf-16-le') + record['replacement'] + suffix.decode('utf-16-le')
    assert new_text[sid][new_rid][oid] == expected_runs, 'Text or inherited run formatting differs from the intended splice'
    expected = copy.deepcopy(old)
    space = expected['spaces'][sid]
    space['contexts'][DEFAULT_CONTEXT] = new_rid
    space['revisions'][new_rid] = copy.deepcopy(previous)
    if old_rid not in space['contexts'].values(): del space['revisions'][old_rid]
    node = space['revisions'][new_rid]['nodes'][oid]
    encoded = node['kind']['text'].encode('utf-16-le')
    node['kind']['text'] = encoded[:start * 2].decode('utf-16-le') + record['replacement'] + encoded[end * 2:].decode('utf-16-le')
    revision = space['revisions'][new_rid]
    page = revision['nodes'][record['page']]
    titles = [(key, n['kind']['text']) for key, n in walk(revision, record['page'])
              if n['kind']['type'] == 'RichText' and not n['kind']['boilerplate']
              and any(field['id'] == 0x88001cb4 for field in n['extra'][0])]
    assert len(titles) <= 1
    title = titles[0][1].lstrip().split('\r')[0] if titles else ''
    if not title or titles[0][0] == oid:
        automatic = not title
        if automatic:
            body = copy.deepcopy(revision) if page['kind']['rtl'] else revision
            if page['kind']['rtl']:
                for item in body['nodes'].values():
                    if item['kind']['type'] == 'Row': item['children'].reverse()
            roots = sorted(page['children'], key=lambda key: (body['nodes'][key]['layout']['y'] or 0,
                -(body['nodes'][key]['layout']['x'] or 0)
                if page['kind']['rtl'] else body['nodes'][key]['layout']['x'] or 0))
            candidates = [n['kind']['text'].strip().split('\r')[0].rstrip()
                          for root in roots for _, n in walk(body, root)
                          if n['kind']['type'] == 'RichText' and not n['kind']['boilerplate']]
            title = next((text for text in candidates if text), '')
            encoded_title = title.encode('utf-16-le')
            end = 510
            if len(encoded_title) > end and 0xd800 <= int.from_bytes(encoded_title[end - 2:end], 'little') <= 0xdbff: end += 2
            title = encoded_title[:end].decode('utf-16-le').rstrip()
        revision['nodes'][revision['roots']['2']]['kind']['title'] = title
        page['kind']['alternate_title'] = title if automatic else ''
    observed = copy.deepcopy(new)
    changed = observed['spaces'][sid]['revisions'][new_rid]['nodes'][oid]
    assert record['started_ms'] // 1000 - 315532800 <= changed['modified'] <= record['finished_ms'] // 1000 - 315532800, 'Modification time falls outside the edit interval'
    pending = [(record['page'], [])]
    ancestors = set()
    while pending:
        key, path = pending.pop()
        assert key not in path, 'Page content has a cycle'
        if key == oid:
            ancestors.update(path)
        else:
            parent = previous['nodes'][key]
            pending.extend((child, path + [key]) for child in parent['children'] + parent['content'] + parent['structure'])
    for key in ancestors:
        if previous['nodes'][key]['modified'] is not None:
            revision['nodes'][key]['modified'] = changed['modified']
    del node['modified'], changed['modified']
    del node['kind']['runs'], changed['kind']['runs']
    assert expected == observed, 'An edit changed unrelated document structure, metadata or properties'


def run(source, output, seed, rounds):
    source = source.resolve(strict=True)
    output = output.resolve()
    if output.is_relative_to(source): raise ValueError('Choose an output directory outside the source notebook.')
    output.mkdir(parents=True, exist_ok=False)
    notebook = output / 'notebook'
    shutil.copytree(source, notebook)
    for file in notebook.rglob("*"):
        if file.is_file(): file.chmod(file.stat().st_mode | stat.S_IWUSR)
    hashes = {p.relative_to(source).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in source.rglob('*') if p.is_file()}
    (output / 'source.json').write_text(json.dumps(hashes, indent=2))
    (output / 'run.json').write_text(json.dumps({'seed': seed, 'rounds': rounds,
        'editor_sha256': hashlib.sha256(EDITOR.read_bytes()).hexdigest(),
        'oracle_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}, indent=2))
    count = 0
    with (output / 'operations.jsonl').open('w') as log:
        for file in sorted(notebook.rglob('*.one')):
            before = export(file)
            pages = [(sid, page) for sid, _, _, page in ordered_pages(before[0])]
            for _ in range(rounds):
                for sid, page in pages:
                    result = subprocess.run([EDITOR, file, '--in-place', str(seed + count), page], capture_output=True, text=True)
                    record = json.loads(result.stdout) if result.stdout else {}
                    record.update(file=file.relative_to(notebook).as_posix(), exit=result.returncode, stderr=result.stderr)
                    log.write(json.dumps(record, ensure_ascii=False) + '\n'); log.flush()
                    assert result.returncode == 0 and record.get('state') == 'Committed', f'Page edit failed: {record}'
                    after = export(file)
                    verify(before, after, record)
                    before = after
                    count += 1
    assert all(hashlib.sha256((source / path).read_bytes()).hexdigest() == digest for path, digest in hashes.items()), 'A source file changed during the campaign'
    (output / 'result.json').write_text(json.dumps({'verified_edits': count, 'source_files_unchanged': len(hashes)}, indent=2))
    print(f'Verified {count} edits; {len(hashes)} source files unchanged')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--seed', type=int, default=42)
    parser.add_argument('--rounds', type=int, default=1)
    args = parser.parse_args()
    if args.rounds <= 0 or args.seed < 0: parser.error('Use positive rounds and a nonnegative seed.')
    run(args.source, args.output, args.seed, args.rounds)
