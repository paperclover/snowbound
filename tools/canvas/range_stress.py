#!/usr/bin/env python3
"""Check source coverage across deterministic mixed-Unicode layout cases."""
import argparse
import json
from pathlib import Path
import random
import subprocess


def cases():
    rng = random.Random(20260906)
    tokens = ['a', ' ', 'é', 'é', 'שלום', 'العربية', '👩🏽‍💻', '👩‍👩‍👧‍👦', '🇨🇦', '\u00a0', 'ffi', '\u200d']
    result = []
    for index in range(500):
        text = ''.join(rng.choice(tokens) for _ in range(rng.randrange(35)))
        result.append({'id': str(index), 'width': rng.uniform(1, 250),
                       'runs': [{'text': text, 'font': 'Arial', 'size': rng.uniform(7, 25),
                                 'bold': False, 'italic': False}]})
    return result


def check(source, output):
    if len(source) != len(output['cases']):
        raise ValueError('Probe did not return every case')
    count = 0
    for case, result in zip(source, output['cases']):
        if case['id'] != result['id']:
            raise ValueError('Probe returned different case identities')
        raw = ''.join(run['text'] for run in case['runs']).encode('utf-16-le')
        end = 0
        for line in result['lines']:
            if line['start_utf16'] != end or not end <= line['end_utf16'] <= len(raw) // 2:
                raise ValueError(f"{case['id']}: noncontiguous or out-of-bounds source ranges")
            start, end = line['start_utf16'], line['end_utf16']
            if raw[2 * start:2 * end].decode('utf-16-le') != line['text']:
                raise ValueError(f"{case['id']}: line text differs from its source range")
            count += 1
        if end * 2 != len(raw):
            raise ValueError(f"{case['id']}: layout does not cover all source text")
    return count


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('probe', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if not args.probe:
        parser.error('Provide a probe command after the output directory')
    args.output.mkdir(parents=True, exist_ok=False)
    source = cases()
    path = args.output / 'cases.json'
    path.write_text(json.dumps(source, ensure_ascii=False, indent=2) + '\n')
    command = args.probe + [str(path)]
    (args.output / 'command.json').write_text(json.dumps(command) + '\n')
    with (args.output / 'output.json').open('w') as stdout, (args.output / 'stderr.txt').open('w') as stderr:
        result = subprocess.run(command, stdout=stdout, stderr=stderr)
    summary = {'seed': 20260906, 'cases': len(source), 'exit': result.returncode}
    if result.returncode == 0:
        try:
            summary['lines_checked'] = check(source, json.loads((args.output / 'output.json').read_text()))
        except (ValueError, KeyError) as error:
            summary['error'] = str(error)
    else:
        summary['error'] = 'Probe rejected a generated case; see stderr.txt'
    (args.output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary))
    raise SystemExit(1 if 'error' in summary else 0)
