#!/usr/bin/env python3
"""Cold-open the retained storage-interruption matrix with disposable OneNote VMs."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import runpy
import subprocess
import xml.etree.ElementTree as ET

from native_runner import capture
from document_model import EXPORTER, view


def run(matrix, output, workers):
    cases = json.loads((matrix / 'native-cases.json').read_text())
    output.mkdir(parents=True, exist_ok=False)
    compare = runpy.run_path(str(Path(__file__).with_name('verify-document.py')))['compare']

    def verify(item):
        digest, name = item
        source = matrix / name / 'notebook'
        before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in source.iterdir()}
        changed, = [p for p in source.iterdir() if hashlib.md5(p.read_bytes()).hexdigest() == digest]
        destination = output / name
        capture(source, destination, pdf=True)
        compare(source, destination / 'read')
        if changed.suffix == '.onetoc2':
            document = json.loads(subprocess.check_output([EXPORTER, changed]))
            _, revision = view(document, document['root'])
            color = revision['nodes'][revision['roots']['1']]['kind']['color']
            expected = '#' + int(color).to_bytes(4, 'little')[:3].hex().upper()
            assert ET.parse(destination / 'read/hierarchy.xml').getroot().get('color') == expected, 'Native notebook color differs from the persisted TOC'
        assert before == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in source.iterdir()}
        assert json.loads((destination / 'teardown.json').read_text())['absent']
        (destination / 'verified.json').write_text(json.dumps({'source_sha256': before, 'native_comparison': True, 'vm_deleted': True}, indent=2))
        print('Verified native recovery:', name, flush=True)
        return name

    with ThreadPoolExecutor(max_workers=workers) as pool:
        verified = list(pool.map(verify, cases.items()))
    (output / 'result.json').write_text(json.dumps({'cases': sorted(verified), 'native_comparison': True}, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('matrix', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--workers', type=int, default=2)
    args = parser.parse_args()
    if not 1 <= args.workers <= 4:
        parser.error('Choose one to four simultaneous captures.')
    run(args.matrix, args.output, args.workers)
