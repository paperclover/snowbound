#!/usr/bin/env python3
"""Generate a reproducible TOC edit history that crosses log and counter boundaries."""
import argparse
from document_model import EXPORTER, view
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    if args.destination.resolve().is_relative_to(args.source.resolve()):
        raise ValueError('Choose a destination outside the source notebook.')
    args.destination.mkdir(parents=True, exist_ok=False)
    notebook = args.destination / 'notebook'
    shutil.copytree(args.source, notebook)
    toc = notebook / 'Open Notebook.onetoc2'
    original = toc.read_bytes()
    doc = json.loads(subprocess.check_output([EXPORTER, toc]))
    _, space = view(doc, doc['root']); oid = space['roots']['1']
    color = space['nodes'][oid]['kind']['color']
    history = []
    with tempfile.TemporaryDirectory() as temporary:
        source = Path(temporary) / 'current'
        source.write_bytes(original)
        for value in range(260):
            destination = Path(temporary) / 'next'
            subprocess.run([ROOT / 'target/debug/examples/edit_property', source, destination,
                            '20001', '14001cbe', color.to_bytes(4, 'little').hex(),
                            value.to_bytes(4, 'little').hex(), doc['root'], oid], check=True)
            destination.replace(source)
            history.append({'before': color, 'after': value})
            color = value
        toc.write_bytes(source.read_bytes())
    (args.destination / 'history.json').write_text(json.dumps({
        'source_sha256': hashlib.sha256(original).hexdigest(), 'space': doc['root'], 'object': oid,
        'property': '14001cbe', 'operations': history, 'result_sha256': hashlib.sha256(toc.read_bytes()).hexdigest(),
    }, indent=2))
