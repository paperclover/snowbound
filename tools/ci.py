#!/usr/bin/env python3
"""Gates a revision (main by default) on formatting, Clippy, the tests, the Python suite and
every platform's build, checked out in the jj workspace ../snowbound-ci so that edits in
progress elsewhere never reach it. See tools/TESTING.md."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
CI = ROOT.parent / 'snowbound-ci'
TARGET = CI / 'target'
RUNS = TARGET / 'ci'
# Changes here rebuild every package.
EVERYTHING = ('Cargo.toml', 'Cargo.lock', 'vendor/', 'rust-toolchain', '.cargo/')
JSON = '--message-format=json'


def cargo(*args):
    return ['cargo', args[0], '--locked', JSON, *args[1:]]


def cross(script, arch, *args):
    return ['sh', script, arch, args[0], '--locked', JSON, *args[1:]]


def lanes():
    """Each lane: its commands, the packages whose dependencies (None for all) and the paths
    that select it under --changed, extra environment, and why it can't run here, if so."""
    clippy = ['clippy', '--workspace', '--all-targets', '--all-features', '--', '-D', 'warnings']
    # What ships: mobile is iOS's alone, and the tests and lab examples run only on macOS.
    cross_clippy = ['clippy', '--workspace', '--exclude', 'mobile', '--lib', '--bins', '--all-features',
                    '--', '-D', 'warnings']
    windows = CI / 'platform/windows/cargo.sh'
    linux = ROOT / 'platform/linux/cargo.sh'
    mingw = os.environ.get('LLVM_MINGW') or ROOT / 'target/windows/llvm-mingw'
    sdk = Path(os.environ.get('SNOW_LEOPARD_SDK') or ROOT / 'target/snow-leopard/MacOSX10.6.sdk')
    nightly = subprocess.run(['rustup', 'component', 'list', '--installed', '--toolchain', 'nightly'],
                             capture_output=True, text=True).stdout.split()
    result = [
        dict(name='test', minutes=45, packages=None, paths=()),
        dict(name='clippy', minutes=30, packages=None, paths=(), commands=[
            cargo(*clippy),
            cargo('clippy', '-p', 'snowbound', '--no-default-features', '--', '-D', 'warnings')]),
        dict(name='fmt', minutes=5, packages=None, paths=(), commands=[['cargo', 'fmt', '--all', '--check']]),
        dict(name='python', minutes=30, packages=None, paths=('tools/', 'corpus/'), commands=[
            # The suite runs this checkout's examples from target/debug.
            cargo('build', '--workspace', '--all-features', '--examples', '--bins'),
            ['uv', 'run', '--no-project', '--python', '3.12', '--with', 'pillow', '--with', 'pdfplumber',
             'python', '-m', 'unittest', 'discover', '-s', 'tools', '-p', 'test_*.py']],
             environment={'PYTHONPATH': str(CI / 'tools')},
             missing=None if shutil.which('uv') else 'needs uv'),
    ]
    for arch in ('x86_64', 'aarch64'):
        result.append(dict(
            name=f'windows-{arch}', minutes=45, packages=['snowbound'], paths=('platform/windows/',),
            # x86_64's nightly Clippy would lint std, which it builds, and nightly's own new lints.
            commands=[*([] if arch == 'x86_64' else [cross(windows, arch, *cross_clippy)]),
                      cross(windows, arch, 'build', '-p', 'snowbound')],
            environment={'LLVM_MINGW': str(mingw)},
            missing=None if arch == 'aarch64' or 'rust-src' in nightly else 'needs nightly with rust-src'))
    for arch in ('x86_64', 'aarch64'):
        result.append(dict(
            name=f'linux-{arch}', minutes=30, packages=['snowbound'], paths=('platform/linux/',),
            commands=[cross(linux, arch, *cross_clippy), cross(linux, arch, 'build', '-p', 'snowbound')],
            environment={'CARGO_TARGET_DIR': str(TARGET / 'linux')},
            missing=None if shutil.which('zig') else 'needs zig'))
    targets = subprocess.run(['rustup', 'target', 'list', '--installed'], capture_output=True, text=True).stdout
    wasm = web_environment()
    result.append(dict(
        name='web', minutes=20, packages=['snowbound'], paths=('crates/snowbound/web/', 'tools/release_web.py'),
        # Clippy builds it; the module itself is linked by release_web.py.
        commands=[cargo('clippy', '-p', 'snowbound', '--target', 'wasm32-unknown-unknown', '--no-default-features', '--features', 'wgpu,live', '--', '-D', 'warnings')],
        environment={**(wasm or {}), 'CARGO_TARGET_DIR': str(TARGET / 'wasm')},
        missing='needs the wasm32-unknown-unknown target' if 'wasm32-unknown-unknown' not in targets.split()
        else None if wasm else 'needs nix for a clang that builds for wasm32'))
    result.append(dict(
        name='ios', minutes=30, packages=['mobile'], paths=('apps/ios/',),
        # build-rust.sh builds the Rust half into target/ios.
        commands=[['xcodebuild', '-quiet', '-project', 'apps/ios/Snowbound.xcodeproj', '-scheme', 'Snowbound',
                   '-configuration', 'Debug', '-destination', 'generic/platform=iOS Simulator',
                   '-derivedDataPath', TARGET / 'ios-xcode', 'CODE_SIGNING_ALLOWED=NO', 'build']],
        missing=None if shutil.which('xcodebuild') else 'needs Xcode'))
    result.append(dict(
        name='macos-10.6', minutes=45, packages=['snowbound'], paths=('platform/snow-leopard/',),
        commands=[['sh', CI / 'platform/snow-leopard/cargo.sh', 'build', '--locked', JSON, '-p', 'snowbound',
                   '--no-default-features']],
        environment={'SNOW_LEOPARD_SDK': str(sdk)},
        missing=('needs nightly with rust-src' if 'rust-src' not in nightly else
                 None if sdk.exists() else 'needs the 10.6 SDK from platform/snow-leopard/remote.sh sdk')))
    return result


def web_environment():
    """A clang and llvm-ar that build SQLite for wasm32, as release_web.py finds them; none
    where it finds none."""
    import release_web
    try:
        environment = release_web.environment()
    except SystemExit:
        return None
    return {key: environment[key] for key in ('CC_wasm32_unknown_unknown', 'AR_wasm32_unknown_unknown')}


def jj(*args, cwd=ROOT):
    return subprocess.run(['jj', *args], cwd=cwd, check=True, capture_output=True, text=True).stdout


def checkout(rev):
    """Points the CI workspace's own commit, a child of main, at `rev`'s files."""
    commits = jj('log', '--no-graph', '-r', rev, '-T', 'commit_id ++ "\\n"').split()
    if len(commits) != 1:
        sys.exit(f'{rev} names {len(commits)} revisions, not one.')
    if not CI.exists():
        jj('workspace', 'add', '--name', 'ci', '-r', 'main', str(CI))
    jj('workspace', 'update-stale', cwd=CI)
    jj('rebase', '-r', '@', '-o', 'main', cwd=CI)
    jj('restore', '--from', commits[0], cwd=CI)
    return commits[0]


class Run:
    def __init__(self, folder, environment):
        self.folder = folder
        self.environment = environment
        self.processes = set()
        self.lock = threading.Lock()
        self.stopping = False

    def execute(self, command, log, deadline, environment=(), cwd=CI):
        """The command's exit status, or None where it outlived `deadline`."""
        with open(log, 'a') as stream:
            stream.write(f'$ {" ".join(map(str, command))}\n')
            stream.flush()
            try:
                process = subprocess.Popen(list(map(str, command)), cwd=cwd, stdout=stream,
                                           stderr=subprocess.STDOUT, start_new_session=True,
                                           env={**self.environment, **dict(environment)})
            except OSError as error:
                stream.write(f'error: {error}\n')
                return 127
            with self.lock:
                self.processes.add(process)
                if self.stopping:
                    stop(process)
            try:
                return process.wait(timeout=max(deadline - time.monotonic(), 0))
            except subprocess.TimeoutExpired:
                stop(process)
                stream.write('\n(timed out)\n')
                return None
            finally:
                with self.lock:
                    self.processes.discard(process)

    def stop_all(self):
        with self.lock:
            self.stopping = True
            for process in self.processes:
                stop(process)


def stop(process):
    for sent in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(process.pid, sent)
            process.wait(timeout=5)
            return
        except (ProcessLookupError, subprocess.TimeoutExpired):
            pass


def status(code):
    return 'passed' if code == 0 else 'timeout' if code is None else 'failed'


def run_lane(run, lane, deadline):
    log = run.folder / f'{lane["name"]}.log'
    for command in lane['commands']:
        code = run.execute(command, log, deadline, lane.get('environment', {}))
        if code != 0:
            return status(code), [log]
    return 'passed', [log]


def run_tests(run, deadline, packages, jobs):
    """Builds every test target, then runs those of `packages` (all where None) side by side,
    each from its package's folder as cargo would, beside the doctests."""
    log = run.folder / 'test.log'
    code = run.execute(cargo('test', '--workspace', '--all-features', '--no-run'), log, deadline)
    if code != 0:
        return status(code), [log], []
    binaries = []
    for line in log.read_text(errors='replace').splitlines():
        message = json.loads(line) if line.startswith('{') else {}
        if message.get('reason') == 'compiler-artifact' and message['profile']['test'] and message['executable']:
            folder = Path(message['manifest_path']).parent
            if packages is None or folder.name in packages:
                binaries.append((f'{folder.name}.{message["target"]["name"]}', [message['executable']], folder))
    binaries.append(('doctests', ['cargo', 'test', '--locked', '--workspace', '--all-features', '--doc'], CI))
    # The slowest last time start first, so that none of them starts last.
    timings = RUNS / 'test-seconds.json'
    seconds = json.loads(timings.read_text()) if timings.exists() else {}
    binaries.sort(key=lambda binary: -seconds.get(binary[0], float('inf')))
    (run.folder / 'test').mkdir()

    def one(binary):
        name, command, cwd = binary
        started = time.monotonic()
        part_log = run.folder / 'test' / f'{name}.log'
        code = run.execute(command, part_log, deadline, cwd=cwd)
        return {'name': name, 'status': status(code), 'seconds': round(time.monotonic() - started, 1),
                'log': str(part_log)}

    with ThreadPoolExecutor(jobs) as pool:
        parts = list(pool.map(one, binaries))
    timings.write_text(json.dumps({**seconds, **{part['name']: part['seconds'] for part in parts}}, indent=1))
    failed = [part for part in parts if part['status'] != 'passed']
    result = 'timeout' if any(part['status'] == 'timeout' for part in failed) else 'failed' if failed else 'passed'
    return result, [log, *(Path(part['log']) for part in failed)], sorted(parts, key=lambda part: -part['seconds'])


def relative(path):
    return path.removeprefix(f'{CI}/')


def diagnose(log):
    """The errors a log reports, each with the file and line it names where it names one."""
    errors, text = [], []
    lines = log.read_text(errors='replace').splitlines()
    test = None
    for index, line in enumerate(lines):
        following = lines[index + 1] if index + 1 < len(lines) else ''
        if line.startswith('{'):
            try:
                message = json.loads(line).get('message') or {}
            except json.JSONDecodeError:
                continue
            spans = [span for span in message.get('spans', []) if span.get('is_primary')]
            if message.get('level') == 'error' and spans:
                errors.append({'error': message['message'],
                               'file': f'{relative(spans[0]["file_name"])}:{spans[0]["line_start"]}'})
        elif match := re.match(r'---- (\S+) stdout ----', line):
            test = match[1]
        elif match := re.search(r'panicked at (\S+?):(\d+):\d+:$', line):
            errors.append({'error': f'{test or "test"} panicked: {following.strip()}',
                           'file': f'{relative(match[1])}:{match[2]}'})
        elif match := re.match(r'Diff in (\S+?):(\d+):', line):
            errors.append({'error': 'not formatted (cargo fmt)', 'file': f'{relative(match[1])}:{match[2]}'})
        elif match := re.match(r'(FAIL|ERROR): (\S+) \((\S+)\)', line):
            block = lines[index + 2:]
            end = next((i for i, rest in enumerate(block) if rest.startswith(('=' * 20, '-' * 20))), len(block))
            block = [rest for rest in block[:end] if rest.strip()]
            frames = [frame for frame in block if f'File "{CI}/' in frame]
            where = re.search(r'File "(\S+)", line (\d+)', frames[-1]) if frames else None
            errors.append({'error': f'{match[3]}: {block[-1].strip() if block else match[1]}',
                           'file': f'{relative(where[1])}:{where[2]}' if where else None})
        elif match := re.match(r'(/\S+?):(\d+):\d+: (?:fatal )?error: (.*)', line):
            errors.append({'error': match[3], 'file': f'{relative(match[1])}:{match[2]}'})
        elif (match := re.match(r'error(?:\[\w+\])?: (.*)', line)) and (
                where := re.match(r'\s*--> (\S+?):(\d+):\d+', following)):
            errors.append({'error': match[1], 'file': f'{relative(where[1])}:{where[2]}'})
        elif re.match(r'(error|test \S+ \.\.\. FAILED)', line):
            text.append({'error': line.strip(), 'file': None})
    if not errors and not text:
        text = [{'error': line, 'file': None} for line in lines if line.strip() and not line.startswith('{')][-3:]
    return list({(error['error'], error['file']): error for error in errors or text}.values())


def affected(changed, metadata):
    """The workspace packages that changed files reach, through what depends on them."""
    folders = {package['name']: Path(package['manifest_path']).parent.relative_to(CI).as_posix()
               for package in metadata['packages']}
    if any(path.startswith(EVERYTHING) for path in changed):
        return set(folders)
    reached = {name for name, folder in folders.items() if any(path.startswith(folder + '/') for path in changed)}
    users = {package['name']: {dependency['name'] for dependency in package['dependencies']
                               if dependency['name'] in folders} for package in metadata['packages']}
    while more := {name for name, uses in users.items() if uses & reached} - reached:
        reached |= more
    return reached


def dependencies(roots, metadata):
    uses = {package['name']: {dependency['name'] for dependency in package['dependencies']}
            for package in metadata['packages']}
    closure, pending = set(), list(roots)
    while pending:
        name = pending.pop()
        if name in uses and name not in closure:
            closure.add(name)
            pending.extend(uses[name])
    return closure


def size(path):
    if not path.is_dir() or path.is_symlink():
        return path.lstat().st_blocks * 512
    return sum(file.lstat().st_blocks * 512 for file in path.rglob('*'))


def prune(budget, started):
    """Deletes, least recently used first, the build units this run didn't use until the
    target folder fits `budget` bytes. Cargo reads a unit's fingerprint whenever it checks
    it, so the fingerprint's access time is when a build last used the unit."""
    total = disk_usage()
    if total <= budget:
        return 0
    units = {}
    for fingerprints in [*TARGET.glob('*/.fingerprint'), *TARGET.glob('*/*/.fingerprint'),
                         *TARGET.glob('*/*/*/.fingerprint')]:
        profile = fingerprints.parent
        for kind in ('deps', 'build', '.fingerprint', 'incremental', 'examples'):
            for entry in os.scandir(profile / kind) if (profile / kind).is_dir() else ():
                if match := re.search(r'-([0-9a-f]{16})(?:\.|$)', entry.name):
                    units.setdefault((profile, match[1]), []).append(Path(entry.path))
    used = {}
    for unit, paths in units.items():
        stamps = [file.stat().st_atime for path in paths if path.parent.name == '.fingerprint'
                  for file in path.iterdir()] or [path.stat().st_atime for path in paths]
        used[unit] = max(stamps)
    freed = 0
    for unit in sorted(units, key=used.get):
        if used[unit] >= started or total - freed <= budget * 0.8:
            break
        for path in units[unit]:
            freed += size(path)
            shutil.rmtree(path) if path.is_dir() and not path.is_symlink() else path.unlink()
    return total - disk_usage()


def disk_usage():
    return int(subprocess.check_output(['du', '-sk', TARGET]).split()[0]) * 1024


def table(results):
    lines = []
    for result in results:
        seconds = result.get('seconds')
        clock = f'{int(seconds // 60)}:{int(seconds % 60):02}' if seconds is not None else ''
        note = result.get('note', '')
        lines.append(f'{result["name"]:<16} {result["status"]:<8} {clock:>6}  {note}'.rstrip())
        for error in result.get('errors', [])[:6]:
            where = f'{error["file"]}  ' if error['file'] else ''
            lines.append(f'{"":<33}{where}{error["error"].splitlines()[0][:160]}')
        if len(result.get('errors', [])) > 6:
            lines.append(f'{"":<33}… {len(result["errors"]) - 6} more in {result["log"]}')
        elif result['status'] in ('failed', 'timeout'):
            lines.append(f'{"":<33}{result["log"]}')
    return '\n'.join(lines)


def main():
    every = lanes()
    names = [lane['name'] for lane in every]
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group()
    source.add_argument('--rev', default='main', help='The jj revision to gate; main by default')
    source.add_argument('--working-copy', action='store_true',
                        help="This checkout's working copy, as it is when the run starts")
    parser.add_argument('--lanes', nargs='+', metavar='LANE',
                        help=f'Only these lanes, or those starting LANE-: {", ".join(names)}')
    parser.add_argument('--changed', action='store_true',
                        help="Only the lanes, and tests of the packages, that the revision's changes from main reach")
    parser.add_argument('--jobs', type=int, default=4, help='Lanes at once (default 4)')
    parser.add_argument('--test-jobs', type=int, default=4, help='Test executables at once (default 4)')
    parser.add_argument('--timeout', type=float, metavar='MINUTES', help="Each lane's limit, overriding its own")
    parser.add_argument('--budget', type=float, default=40, metavar='GB',
                        help='Prune ../snowbound-ci/target to this size after the run (default 40)')
    args = parser.parse_args()
    if ROOT == CI:
        sys.exit(f'Run ci.py from another checkout; {CI} is its own.')
    def named(lane, name):
        return lane == name or lane.startswith(f'{name}-')
    unknown = [name for name in args.lanes or () if not any(named(lane, name) for lane in names)]
    if unknown:
        parser.error(f'unknown lanes {unknown}; choose from {names}')
    chosen = [lane for lane in every if not args.lanes or any(named(lane['name'], name) for name in args.lanes)]

    RUNS.mkdir(parents=True, exist_ok=True)
    lock = open(RUNS / 'lock', 'w')
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print('Waiting for another ci.py run to finish…', file=sys.stderr, flush=True)
        fcntl.flock(lock, fcntl.LOCK_EX)
    started_at, started = datetime.now().astimezone(), time.monotonic()
    wall = time.time()
    rev = '@' if args.working_copy else args.rev
    commit = checkout(rev)
    described = jj('log', '--no-graph', '-r', commit, '-T',
                   'change_id.short() ++ " " ++ commit_id.short() ++ " " ++ description.first_line()').strip()
    print(f'Gating {rev}: {described}', flush=True)

    tested = None
    changed = None
    if args.changed:
        changed = [line for line in jj('diff', '--from', 'main', '--to', commit, '--name-only').splitlines() if line]
        metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--format-version=1', '--no-deps'], cwd=CI))
        tested = affected(changed, metadata)
        chosen = [lane for lane in chosen
                  if any(path.startswith(lane['paths']) for path in changed)
                  or tested & (dependencies(lane['packages'], metadata) if lane['packages'] else tested)]
        print(f'Changed from main: {len(changed)} files, reaching {", ".join(sorted(tested)) or "no packages"}',
              flush=True)

    folder = RUNS / 'runs' / started_at.strftime('%Y%m%d-%H%M%S')
    folder.mkdir(parents=True)
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(('ONESTORE_', 'SNOWBOUND_', 'CARGO_TARGET_DIR'))}
    run = Run(folder, environment)

    def gate(lane):
        result = {'name': lane['name']}
        if lane.get('missing'):
            print(f'{lane["name"]}: skipped, {lane["missing"]}', flush=True)
            return {**result, 'status': 'skipped', 'note': lane['missing']}
        began = time.monotonic()
        deadline = began + 60 * (args.timeout or lane['minutes'])
        if lane['name'] == 'test':
            outcome, logs, parts = run_tests(run, deadline, tested, args.test_jobs)
            result['parts'] = parts
        else:
            outcome, logs = run_lane(run, lane, deadline)
        errors = [error for log in logs for error in diagnose(log)] if outcome != 'passed' else []
        if outcome == 'timeout':
            errors.insert(0, {'error': f'timed out after {args.timeout or lane["minutes"]:g} minutes', 'file': None})
        seconds = round(time.monotonic() - began, 1)
        print(f'{lane["name"]}: {outcome} in {seconds:.0f}s', flush=True)
        return {**result, 'status': outcome, 'seconds': seconds, 'log': str(logs[0]), 'errors': errors}

    pool = ThreadPoolExecutor(args.jobs)
    try:
        results = list(pool.map(gate, chosen))
    except KeyboardInterrupt:
        pool.shutdown(wait=False, cancel_futures=True)
        run.stop_all()
        raise
    passed = all(result['status'] in ('passed', 'skipped') for result in results)
    freed = prune(args.budget * 1e9, wall)
    summary = {
        'status': 'passed' if passed else 'failed', 'revision': rev, 'commit': commit, 'described': described,
        'workspace': str(CI), 'started': started_at.isoformat(timespec='seconds'),
        'seconds': round(time.monotonic() - started, 1), 'changed': changed,
        'tested_packages': sorted(tested) if tested is not None else None,
        'pruned_bytes': freed, 'lanes': results,
    }
    (folder / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    for old in sorted((RUNS / 'runs').iterdir())[:-20]:
        shutil.rmtree(old)
    print()
    print(table(results))
    print(f'\n{summary["status"]} in {summary["seconds"] / 60:.1f} min: {folder / "summary.json"}')
    sys.exit(0 if passed else 1)


if __name__ == '__main__':
    main()
