#!/usr/bin/env python3
"""Run the public library regression lane and retain each command's output."""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path, help='New directory for logs and results')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(('ONESTORE_', 'SNOWBOUND_'))}
    environment['PYTHONPATH'] = str(root / 'tools')
    environment['CARGO_TARGET_DIR'] = str(root / 'target')
    commands = [
        ('python-dependencies', [sys.executable, '-c', 'import PIL, pdfplumber']),
        ('format', ['cargo', 'fmt', '--all', '--check']),
        ('rust', ['cargo', 'test', '--locked', '--workspace', '--all-features', '--all-targets']),
        ('doctests', ['cargo', 'test', '--locked', '--workspace', '--all-features', '--doc']),
        ('clippy', ['cargo', 'clippy', '--locked', '--workspace', '--all-features', '--all-targets', '--', '-D', 'warnings']),
        ('tools', ['cargo', 'build', '--locked', '--workspace', '--all-features', '--examples', '--bins']),
        ('python', [sys.executable, '-m', 'unittest', 'discover', '-s', 'tools', '-p', 'test_*.py', '-v']),
    ]
    manifest = {'lane': 'public', 'root': str(root), 'status': 'running',
                'started': datetime.now(timezone.utc).isoformat(), 'stages': []}
    result_path = output / 'results.json'
    result_path.write_text(json.dumps(manifest, indent=2) + '\n')
    for name, command in commands:
        print(name, flush=True)
        started = time.monotonic()
        log = output / (name + '.log')
        with log.open('w') as stream:
            try:
                status = subprocess.run(command, cwd=root, env=environment,
                                        stdout=stream, stderr=subprocess.STDOUT).returncode
            except OSError as error:
                stream.write(str(error) + '\n')
                status = None
        manifest['stages'].append({'name': name, 'command': command,
                                   'exit': status, 'seconds': time.monotonic() - started,
                                   'log': log.name})
        if status != 0:
            manifest['status'] = 'failed'
        result_path.write_text(json.dumps(manifest, indent=2) + '\n')
        if status != 0:
            raise SystemExit(f'{name} failed: {log}')
    manifest['status'] = 'passed'
    result_path.write_text(json.dumps(manifest, indent=2) + '\n')
    print(result_path)


if __name__ == '__main__':
    main()
