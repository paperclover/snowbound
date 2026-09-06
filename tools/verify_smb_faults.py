#!/usr/bin/env python3
"""Cold-open every SMB fault artifact and compare its complete native text."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import signal
import xml.etree.ElementTree as ET

from native_format import native_characters
from native_xml import ns
from native_probe import ROOT, run as probe


def verify(root, captures):
    cases = root / 'cases'
    records = json.loads((cases / 'results.json').read_text())
    assert records and len({record['case'] for record in records}) == len(records), 'Missing or duplicate fault cases'
    packets = {f'{role}-{path.stem}': path for role in ('source', 'interrupted', 'recovered')
               for path in (cases / role).glob('*.one')}
    expected_names = {f'{role}-{record["case"]}' for record in records for role in ('interrupted', 'recovered')}
    fixtures = {record['case'].rsplit('-', 1)[0] for record in records}
    expected_names.update(f'source-{fixture}' for fixture in fixtures)
    assert set(packets) == expected_names, 'Artifact inventory differs from completed cases'
    observed = {}
    for worker in captures.glob('worker-*'):
        result = json.loads((worker / 'results.json').read_text(encoding='utf-8-sig'))
        for record in result if isinstance(result, list) else [result]:
            name = record['name']
            assert name in packets and name not in observed, 'Unexpected or duplicate native result'
            assert record['error'] is None and record['pages'] == 1, f'{name}: native open failed'
            assert record['source_sha256'] == hashlib.sha256(packets[name].read_bytes()).hexdigest(), f'{name}: native input differs'
            page, = (worker / 'results' / name).glob('page-*.xml')
            page = ET.parse(page).getroot()
            containers = page.findall('one:Title', ns) + page.findall('one:Outline', ns)
            observed[name] = [''.join(char for char, _ in paragraph) for paragraph in native_characters(page, containers)]
    assert set(observed) == expected_names, 'Missing native captures'
    for record in records:
        fixture = record['case'].rsplit('-', 1)[0]
        intent = json.loads((cases / f'{fixture}-intent.json').read_text())
        baseline = observed[f'source-{fixture}']
        assert baseline.count(intent['before']) == 1, 'The native source lacks a unique editing target'
        for role in ('interrupted', 'recovered'):
            replacement = intent[record['visible']] if role == 'interrupted' else intent['after'] + intent['suffix']
            expected = [replacement if text == intent['before'] else text for text in baseline]
            assert observed[f'{role}-{record["case"]}'] == expected, f'{role}-{record["case"]}: native content differs from the recorded outcome'
    return {'cases': len(records), 'cold_native_opens': len(observed), 'exact_native_text': True}


def run(root, output):
    assert json.loads((root / 'verification.json').read_text())['server_hashes_match']
    output.mkdir(parents=True, exist_ok=False)
    (output / 'scripts').mkdir()
    scripts = [output / 'scripts' / name for name in ('cold.ps1', 'probe.ps1')]
    for path in scripts: path.write_bytes((ROOT / 'tools/native' / path.name).read_bytes())
    packets = [(role, path) for role in ('source', 'interrupted', 'recovered')
               for path in sorted((root / 'cases' / role).glob('*.one'))]
    workers = min(8, len(packets))
    inputs = [output / f'input-{i}' for i in range(workers)]
    for path in inputs: path.mkdir()
    for i, (role, path) in enumerate(packets):
        (inputs[i % workers] / f'{role}-{path.name}').symlink_to(path.resolve())
    def capture(i): probe(inputs[i], output / f'worker-{i}', scripts)
    with ThreadPoolExecutor(max_workers=workers) as pool:
        list(pool.map(capture, range(workers)))
    result = verify(root, output)
    (output / 'verification.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run', type=Path)
    parser.add_argument('captures', type=Path)
    parser.add_argument('--verify-only', action='store_true')
    args = parser.parse_args()
    def interrupted(_signal, _frame): raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    if args.verify_only: print(json.dumps(verify(args.run.resolve(), args.captures.resolve()), indent=2))
    else: run(args.run.resolve(), args.captures.resolve())
