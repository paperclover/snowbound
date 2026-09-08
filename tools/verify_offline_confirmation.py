#!/usr/bin/env python3
"""Compare cold native reads of confirmation snapshots with the recorded editing history."""
import argparse
import hashlib
import json
from pathlib import Path
import xml.etree.ElementTree as ET

from native_format import native_characters
from native_stress import native_history
from native_xml import ns
from offline_history import publication_links


def verify(output, cold, *, partial=False):
    config = json.loads((output / 'run.json').read_text())
    actors = [*(f'w{i}' for i in range(config['rust_writers'])), *(f'r{i}' for i in range(config['rust_readers']))]
    logs = {actor: [json.loads(line) for line in (output / 'rust' / f'{actor}.jsonl').read_text().splitlines()] for actor in actors}
    publication_links(logs, config['stress_operations'], partial=partial)
    documents = {}
    if config.get('document_operations'):
        from offline_document_history import document_history
        assert not partial, 'Document confirmation audit requires a complete workload'
        documents = document_history(logs, config['stress_operations'])
    unknown = [row for rows in logs.values() for row in rows if row['event'] == 'remote_attempt' and row['state'] == 'Unknown']
    assert len(unknown) == 1
    attempt, = unknown
    confirmations = {}
    for actor, rows in logs.items():
        observed = None
        for row in rows:
            if row['event'] == 'read': observed = row
            if row['event'] != 'remote_confirm': continue
            name = row['capture']
            assert Path(name).name == name and name.startswith(str(rows[0]['pid']) + '-')
            assert name not in confirmations
            if attempt['revision'] not in row['revisions'].get(attempt['space'], []):
                assert documents and row.get('current_revisions', {}).get(attempt['space']) in row['revisions'].get(attempt['space'], []), 'Retired attempt lacks a current effect-confirmation revision'
            if documents:
                assert observed and observed['text'] == row['text'] and observed['finished_us'] <= row['started_us']
                assert isinstance(observed.get('documents'), dict)
                assert all(observed['documents'].get(target) == value for target, value in attempt['document_changes'].items()), 'Confirmation omitted the uncertain document change'
            confirmations[name] = row, observed['documents'] if documents else {}
    assert confirmations
    manifest = json.loads((cold / 'run.json').read_text())['inputs']
    assert set(manifest) == set(confirmations)
    assert set(manifest) == {path.name for path in (output / 'rust/confirmations').glob('*.one')}
    results = json.loads((cold / 'results.json').read_text(encoding='utf-8-sig'))
    if isinstance(results, dict): results = [results]
    assert len(results) == len(confirmations) and len({row['name'] for row in results}) == len(results)
    native = []
    for i in range(config['stress_clients']):
        rows = [json.loads(line) for line in (output / f'n{i}/stress-events.jsonl').read_text(encoding='utf-8-sig').splitlines()]
        assert len(rows) <= config['stress_operations']
        native_history(rows, i, len(rows) if partial else config['stress_operations'], False)
        native.append({f'Native {i}:', *(row['before'] + row['token'] for row in rows)})
    checks = format_checks = 0
    for result in results:
        name = result['name'] + '.one'
        assert result['error'] is None and result['pages'] == 1
        assert result['source_sha256'] == manifest[name] == hashlib.sha256((output / 'rust/confirmations' / name).read_bytes()).hexdigest()
        page = ET.parse(cold / 'results' / result['name'] / 'page-0.xml').getroot()
        formatted = native_characters(page, page.findall('one:Outline', ns))
        paragraphs = [''.join(c for c, _ in paragraph) for paragraph in formatted]
        confirmation, observed = confirmations[name]
        expected = confirmation['text']
        assert len(paragraphs) == len(native) + 1 + len(observed) and paragraphs.count(expected) == 1
        paragraphs.remove(expected)
        if observed:
            from offline_document_history import characters, verify_native
            format_checks += verify_native(formatted, (characters(value) for value in observed.values()))
            for document in observed.values():
                assert paragraphs.count(document['text']) == 1, 'Confirmation duplicated a document paragraph'
                paragraphs.remove(document['text'])
        for index, versions in enumerate(native):
            selected = [text for text in paragraphs if text.startswith(f'Native {index}:')]
            assert len(selected) == 1 and selected[0] in versions, 'Native confirmation image contains an unrecorded edit or loses a prefix'
        checks += len(native) + 1 + len(observed)
    assert json.loads((cold / 'teardown.json').read_text()) == {'absent': True}
    confirmed_revision = documents[next(iter(attempt['document_changes']))]['states']['format']['attempt']['receipt_revision'] if documents else attempt['revision']
    return {'complete_workload': not partial, 'attempted_revision': attempt['revision'], 'confirmed_revision': confirmed_revision, 'native_images': len(results), 'exact_rust_paragraphs': len(results),
            'validated_paragraphs': checks, 'native_intended_format_checks': format_checks,
            'maximum_native_export_seconds': max(row['seconds'] for row in results)}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', type=Path)
    parser.add_argument('cold', type=Path)
    parser.add_argument('--partial', action='store_true', help='Validate preserved confirmations from an interrupted workload')
    args = parser.parse_args()
    result = verify(args.run, args.cold, partial=args.partial)
    (args.run / 'confirmation-cold-verification.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
