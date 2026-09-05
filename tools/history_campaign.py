#!/usr/bin/env python3
"""Generate and compare independently specified native edit histories."""
import argparse
import copy
import json
from pathlib import Path
import random
import shutil
import signal
import subprocess
import tempfile
import xml.etree.ElementTree as ET

from document_model import EXPORTER, ordered_pages
from native_xml import ns, Text, project_text

ROOT = Path(__file__).resolve().parent.parent


def reduce_history(history):
    state = copy.deepcopy(history['initial'])
    for op in history['operations']:
        kind = op['kind']
        if kind == 'insert': state['paragraphs'].insert(op['index'], copy.deepcopy(op['paragraph']))
        elif kind == 'delete': state['paragraphs'].pop(op['index'])
        elif kind == 'move': state['paragraphs'].insert(op['to'], state['paragraphs'].pop(op['index']))
        elif kind == 'position': state['x'], state['y'] = op['x'], op['y']
        elif kind == 'cell': state['table'][op['row']][op['column']] = op['text']
        else: state['paragraphs'][op['index']][kind] = op['value']
    return state


def generate(destination, count, start=0):
    destination.mkdir(parents=True, exist_ok=False)
    notebook = destination / 'notebook'
    shutil.copytree(ROOT / 'corpus/writer/create-notebook-01/notebook', notebook)
    histories = []
    samples = ['Repeated words', 'café 東京 مرحبا', 'A😀e\u0301Z', 'literal <b> & "quotes"', 'tabs\there', ' ]]> end', 'simple text']
    def paragraph(identifier, text):
        return {'id': identifier, 'text': text, 'bold': False, 'italic': False, 'underline': False,
                'strike': False, 'color': '#000000', 'highlight': '#ffff00', 'font_size': 11,
                'link': '', 'tag': False}
    kinds = ['text', 'bold', 'italic', 'underline', 'strike', 'color', 'highlight', 'font_size', 'link', 'tag', 'insert', 'delete', 'move', 'position', 'cell']
    for seed in range(start, start + count):
        rng = random.Random(seed)
        history = {'seed': seed, 'title': f'History {seed:06}', 'initial': {
            'paragraphs': [paragraph(f'p{i}', f'Initial {i}') for i in range(4)],
            'table': [[f'Cell {row},{column}' for column in range(2)] for row in range(2)], 'x': 72, 'y': 108}, 'operations': []}
        for step in range(20):
            state = reduce_history(history)
            kind = kinds[step % len(kinds)] if seed < 3 else rng.choice(kinds)
            index = rng.randrange(len(state['paragraphs']))
            op = {'kind': kind, 'index': index}
            if kind == 'insert': op['paragraph'] = paragraph(f'new{step}', rng.choice(samples))
            elif kind == 'delete' and len(state['paragraphs']) == 1: op = {'kind': 'text', 'index': 0, 'value': rng.choice(samples)}
            elif kind == 'move': op['to'] = rng.randrange(len(state['paragraphs']))
            elif kind == 'position': op.update(x=rng.randrange(1, 7) * 36, y=rng.randrange(2, 10) * 36)
            elif kind == 'cell': op.update(row=rng.randrange(2), column=rng.randrange(2), text=rng.choice(samples))
            elif kind == 'text': op['value'] = rng.choice(samples)
            elif kind in ('bold', 'italic', 'underline', 'strike', 'tag'): op['value'] = not state['paragraphs'][index][kind]
            elif kind in ('color', 'highlight'): op['value'] = rng.choice(['#000000', '#ff0000', '#00ffff', '#ffff00', '#00ff00'])
            elif kind == 'font_size': op['value'] = rng.choice([9, 11, 14, 18])
            elif kind == 'link': op['value'] = '' if state['paragraphs'][index]['link'] else f'https://example.invalid/{seed}/{step}?a=1&b=2'
            history['operations'].append(op)
        histories.append(history)
    (notebook / 'histories.json').write_text(json.dumps(histories, ensure_ascii=False, indent=2))


def compare(capture, seed=None):
    histories = json.loads((capture / 'notebook/histories.json').read_text())
    if seed is not None:
        histories = [h for h in histories if h['seed'] == seed]
        if len(histories) != 1:
            raise ValueError('The capture must contain exactly one history with this seed.')
    native = {ET.parse(p).getroot().get('name'): ET.parse(p).getroot() for p in (capture / 'read').glob('page-*.xml')}
    with tempfile.TemporaryDirectory() as temporary:
        export = Path(temporary) / 'model'
        subprocess.run([EXPORTER, capture / 'notebook/synthetic.one', export], check=True)
        document = json.loads((export / 'document.json').read_text())
        resolved = json.loads((export / 'text.json').read_text())
    actual = {}
    for sid, rid, space, oid in ordered_pages(document):
        title = ''.join(n['kind']['text'] for n in space['nodes'].values() if n['kind']['type'] == 'RichText' and n['kind']['text'].startswith('History '))
        if title: actual[title] = (sid, rid, space, oid)
    failures = []
    for history in histories:
        try:
            expected = reduce_history(history)
            title = history['title']; page = native[title]
            sid, rid, space, oid = actual[title]
            outlines = [space['nodes'][i] for i in space['nodes'][oid]['children'] if space['nodes'][i]['kind']['type'] == 'Outline']
            assert len(outlines) == 1, (title, 'outline count')
            native_outline = page.find('one:Outline', ns)
            native_oes = native_outline.findall('one:OEChildren/one:OE', ns)
            native_text = [n.find('one:T', ns) for n in native_oes if n.find('one:T', ns) is not None]
            paragraphs = [space['nodes'][i] for i in outlines[0]['children']]
            content = [p['content'][0] for p in paragraphs]
            text_ids = [i for i in content if space['nodes'][i]['kind']['type'] == 'RichText']
            assert len(text_ids) == len(native_text) == len(expected['paragraphs']), (title, 'paragraph count')
            for wanted, oid, native_t in zip(expected['paragraphs'], text_ids, native_text, strict=True):
                runs = [r for r in resolved[sid][rid][oid] if not r['format']['hidden']]
                assert ''.join(r['text'] for r in runs) == wanted['text'], (title, wanted['id'], 'Rust text')
                assert ''.join(Text(native_t.text or '').parts) == project_text(wanted['text']), (title, wanted['id'], 'native text')
                leading = len(wanted['text']) - len(wanted['text'].lstrip(' \t'))
                actual_links = [r['link'] or '' for r in runs for _ in r['text']]
                expected_links = ['' if index < leading else wanted['link'] for index in range(len(wanted['text']))]
                assert actual_links == expected_links, (title, wanted['id'], 'hyperlink', actual_links, expected_links)
                for r in runs:
                    if not r['text']:
                        continue
                    for flag in ('bold', 'italic', 'underline', 'strike'):
                        assert bool(r['format'][flag]) == (False if flag == 'underline' and wanted['link'] else wanted[flag]), (title, wanted['id'], flag)
                    assert r['format']['font_size'] == wanted['font_size'], (title, wanted['id'], 'font size')
                    for field in ('color', 'highlight'):
                        value = '#dbdb00' if field == 'color' and wanted[field] == '#ffff00' else wanted[field]
                        rgb = bytes.fromhex(value[1:])
                        assert r['format'][field] == (0xff000000 if field == 'color' and wanted['link'] else int.from_bytes(rgb, 'little')), (title, wanted['id'], field, r['format'][field], wanted[field])
                tags = space['nodes'][oid]['tags']
                assert bool(tags) == wanted['tag'], (title, wanted['id'], 'tag')
            table = [space['nodes'][i] for i in content if space['nodes'][i]['kind']['type'] == 'Table']
            assert len(table) == 1, (title, 'table count')
            cells = [[ ''.join(r['text'] for paragraph in space['nodes'][cell]['children']
                               for text in space['nodes'][paragraph]['content'] for r in resolved[sid][rid][text] if not r['format']['hidden'])
                       for cell in space['nodes'][row]['children']] for row in table[0]['children']]
            assert cells == expected['table'], (title, 'table contents')
            for axis in ('x', 'y'):
                assert abs(outlines[0]['layout'][axis] - expected[axis]) < .002, (title, axis)
        except AssertionError as failure:
            failures.append(failure.args[0])
    (capture / 'history-differences.json').write_text(json.dumps(failures, ensure_ascii=False, indent=2))
    if failures:
        raise AssertionError(failures[0])
    print(f'Passed {len(histories)} independently specified native histories, {sum(len(h["operations"]) for h in histories)} operations.')


def shrink(capture, destination, seed):
    from native_runner import capture as native_capture
    histories = json.loads((capture / 'notebook/histories.json').read_text())
    history, = [h for h in histories if h['seed'] == seed]
    try:
        compare(capture, seed)
    except AssertionError as failure:
        signature = failure.args[0][:3]
    else:
        raise ValueError('The selected history passes; there is no discrepancy to shrink.')
    destination.mkdir(parents=True, exist_ok=False)
    (destination / 'source.json').write_text(json.dumps({'capture': str(capture.resolve()), 'seed': seed,
                                                       'signature': signature}, indent=2))
    attempt = 0

    def reproduces(candidate):
        nonlocal attempt
        try:
            reduce_history(candidate)
        except IndexError:
            return False
        attempt += 1
        root = destination / f'attempt-{attempt:03}'
        root.mkdir()
        shutil.copytree(ROOT / 'corpus/writer/create-notebook-01/notebook', root / 'input')
        (root / 'input/histories.json').write_text(json.dumps([candidate], ensure_ascii=False, indent=2))
        native_capture(root / 'input', root / 'capture', 2, ROOT / 'tools/native/history.ps1')
        observed = None
        try:
            compare(root / 'capture', seed)
        except AssertionError as failure:
            observed = failure.args[0]
        matches = observed is not None and observed[:3] == signature
        (root / 'comparison.json').write_text(json.dumps({'discrepancy': observed, 'same_failure': matches}, indent=2))
        print(f'Shrink attempt {attempt}: {len(candidate["operations"])} operations; same discrepancy: {matches}', flush=True)
        return matches

    if not reproduces(history):
        raise ValueError('The original discrepancy did not reproduce in a fresh clone.')
    granularity = 2
    while history['operations']:
        operations = history['operations']
        width = max(1, (len(operations) + granularity - 1) // granularity)
        for start in range(0, len(operations), width):
            candidate = {**history, 'operations': operations[:start] + operations[start + width:]}
            if reproduces(candidate):
                history = candidate
                granularity = max(2, granularity - 1)
                break
        else:
            if width == 1:
                break
            granularity = min(len(operations), granularity * 2)
        (destination / 'minimal.json').write_text(json.dumps(history, ensure_ascii=False, indent=2))
    for index, operation in enumerate(history['operations']):
        choices = []
        if operation['kind'] == 'text': choices = [{'value': ''}, {'value': 'A'}]
        elif operation['kind'] == 'cell': choices = [{'text': ''}, {'text': 'A'}]
        elif operation['kind'] == 'position': choices = [{'x': 0, 'y': 0}]
        elif operation['kind'] == 'font_size': choices = [{'value': 11}]
        for replacement in choices:
            candidate = copy.deepcopy(history)
            candidate['operations'][index].update(replacement)
            if candidate != history and reproduces(candidate):
                history = candidate
                break
    (destination / 'minimal.json').write_text(json.dumps(history, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    generation = sub.add_parser('generate'); generation.add_argument('destination', type=Path)
    generation.add_argument('--count', type=int, default=100); generation.add_argument('--start', type=int, default=0)
    comparison = sub.add_parser('compare'); comparison.add_argument('capture', type=Path)
    comparison.add_argument('--seed', type=int)
    shrinking = sub.add_parser('shrink'); shrinking.add_argument('capture', type=Path)
    shrinking.add_argument('destination', type=Path); shrinking.add_argument('--seed', type=int, required=True)
    args = parser.parse_args()
    if args.command == 'generate': generate(args.destination, args.count, args.start)
    elif args.command == 'compare': compare(args.capture, args.seed)
    else:
        def interrupted(_signal, _frame):
            raise KeyboardInterrupt
        signal.signal(signal.SIGTERM, interrupted)
        shrink(args.capture, args.destination, args.seed)
