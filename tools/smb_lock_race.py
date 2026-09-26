#!/usr/bin/env python3
"""Compare SMB lock progress and exclusion across file access modes."""
import argparse
from collections import Counter
import fcntl
import json
import hashlib
import platform
import os
from pathlib import Path
import subprocess
import sys
import time


def worker(file, marker, mode, operations, atomic, shared):
    counts = Counter()
    for i in range(operations):
        access = ('r+b' if i % 2 else 'rb') if mode == 'mixed' else mode
        # With shared reads, readers open O_SHLOCK and may overlap each other, never a writer.
        reading = shared and access == 'rb'
        held = marker.with_name(f'{marker.name}-reader-{os.getpid()}') if reading else marker
        stage = 'open'
        try:
            lock = os.O_SHLOCK if reading else os.O_EXLOCK
            with (os.fdopen(os.open(file, (os.O_RDWR if access == 'r+b' else os.O_RDONLY) | lock | os.O_NONBLOCK), access) if atomic else file.open(access)) as stream:
                stage = 'lock'
                if not atomic:
                    fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                counts['acquired'] += 1
                owned = False
                try:
                    try:
                        held.mkdir()
                        owned = True
                        others = [p for p in marker.parent.glob(marker.name + '*') if p != held]
                        if others and not (reading and all('-reader-' in p.name for p in others)):
                            counts['overlap'] += 1
                        elif others:
                            counts['shared'] += 1
                    except FileExistsError:
                        counts['overlap'] += 1
                    time.sleep(.002)
                    try:
                        stream.read(1)
                    except OSError as error:
                        counts[f'read:{error.errno}'] += 1
                finally:
                    if owned:
                        held.rmdir()
                    try:
                        fcntl.flock(stream, fcntl.LOCK_UN)
                    except OSError as error:
                        counts[f'unlock:{error.errno}'] += 1
                stage = 'close'
        except OSError as error:
            counts[f'{stage}:{error.errno}'] += 1
        time.sleep(.002)
    print(json.dumps(counts), flush=True)


def run(output, directory, operations, atomic, clients, shared):
    output.mkdir(parents=True, exist_ok=False)
    directory.mkdir(exist_ok=False)
    (output / 'run.json').write_text(json.dumps({'operations': operations, 'clients': clients, 'atomic': atomic, 'shared_reads': shared,
        'platform': platform.platform(), 'script_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}, indent=2))
    results = {}
    for mode in ('rb', 'r+b', 'mixed'):
        file = directory / (mode + '.bin')
        file.write_bytes(b'lock probe')
        marker = output / 'held'
        processes = []
        try:
            for i in range(clients):
                processes.append(subprocess.Popen([sys.executable, __file__, '--worker', str(file), str(marker), mode, str(operations), str(int(atomic)), str(int(shared))],
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True))
            results[mode] = []
            for process in processes:
                stdout, stderr = process.communicate(timeout=60)
                assert process.returncode == 0, stderr
                results[mode].append(json.loads(stdout))
        finally:
            for process in processes:
                if process.poll() is None:
                    process.terminate()
            for process in processes:
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        (output / 'result.json').write_text(json.dumps(results, indent=2))
        print(mode, results[mode], flush=True)
    assert all(c.get('acquired', 0) and not c.get('overlap', 0) and
               all(k in ('acquired', 'shared', 'open:35', 'lock:35') for k in c)
               for counters in results.values() for c in counters), 'Lock exclusion or progress failed'


if __name__ == '__main__':
    if len(sys.argv) == 8 and sys.argv[1] == '--worker':
        worker(Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4], int(sys.argv[5]), bool(int(sys.argv[6])), bool(int(sys.argv[7])))
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('output', type=Path)
        parser.add_argument('directory', type=Path)
        parser.add_argument('--operations', type=int, default=500)
        parser.add_argument('--atomic', action='store_true')
        parser.add_argument('--clients', type=int, default=4)
        parser.add_argument('--shared-reads', action='store_true', help='With --atomic, readers open O_SHLOCK.')
        args = parser.parse_args()
        run(args.output.resolve(), args.directory.resolve(), args.operations, args.atomic, args.clients, args.shared_reads)
