#!/usr/bin/env python3
"""Cold-open a batch of disposable section candidates in isolated OneNote caches."""
import argparse
import hashlib
import json
from pathlib import Path
import signal
import zipfile
from native_runner import ROOT, clone, command, windows, collect_artifacts


def run(inputs, output, scripts=None):
    output.mkdir(parents=True, exist_ok=False)
    files = sorted(inputs.glob('*.one'))
    assert files, 'No section candidates'
    if scripts is None:
        (output / 'scripts').mkdir()
        scripts = [output / 'scripts' / name for name in ('cold.ps1', 'probe.ps1')]
        for path in scripts: path.write_bytes((ROOT / 'tools/native' / path.name).read_bytes())
    (output / 'run.json').write_text(json.dumps({'inputs': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in files},
        'scripts': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in scripts}}, indent=2))
    archive = output / 'inputs.zip'
    try:
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as packed:
            for path in files:
                packed.writestr(path.name, path.read_bytes())
        with clone(output) as name:
            for local, remote in [(archive, r'C:\one-tests\inputs.zip'), (scripts[0], r'C:\one-tests\cold-current.ps1'),
                                  (scripts[1], r'C:\one-tests\probe.ps1')]:
                result = windows.do_put(local, remote, name)
                if result.get('error'): raise RuntimeError(result['error'])
            command(name, r'powershell -NoProfile -Command "Expand-Archive C:\one-tests\inputs.zip C:\one-tests\runs\capture\inputs"', output)
            command(name, r'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\one-tests\probe.ps1 -Root C:\one-tests\runs\capture -CloneHost ONE-' + name.upper(), output, (len(files) * 60 + 120) * 1000)
            collect_artifacts(name, output, r'results, C:\one-tests\runs\capture\results.json, C:\one-tests\runs\capture\progress.jsonl')
    finally:
        archive.unlink(missing_ok=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('inputs', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    run(args.inputs.resolve(), args.output.resolve())
