#!/usr/bin/env python3
"""Replay shared notebook edits with disposable OneNote clients and a Linux server."""
import argparse
import errno
from contextlib import ExitStack, contextmanager
from concurrent.futures import ThreadPoolExecutor
from threading import Lock
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tarfile
import time
import xml.etree.ElementTree as ET

from native_runner import ROOT, clone, command, windows, vm, collect_artifacts
from native_xml import texts
from document_model import ordered_pages, view, walk
import linux_vm


def reachable_page_text(model):
    (sid, _, _, _), = ordered_pages(model)
    pending, retained = [sid], {}
    while pending:
        sid = pending.pop()
        if sid in retained: continue
        _, revision = view(model, sid)
        manifest = revision['nodes'][revision['roots']['1']]
        assert manifest['kind']['type'] == 'Manifest'
        page, = manifest['content']
        assert revision['nodes'][page]['kind']['type'] == 'Page'
        retained[sid] = sorted(node['kind']['text'] for _, node in walk(revision, page) if node['kind']['type'] == 'RichText')
        pending.extend(manifest['spaces'])
    return retained


def verify_final_state(model):
    (sid, main), (conflict_sid, competing) = reachable_page_text(model).items()
    main, competing = set(main), set(competing)
    assert {'Native lock recovery.', 'Client B online cell.'} <= main
    edits = {'Client A concurrent edit.', 'Client B concurrent edit.'}
    assert len(main & edits) == len(competing & edits) == 1 and edits <= main | competing
    return {'main_space': sid, 'conflict_space': conflict_sid, 'competing_edits': sorted(edits)}


def replay(output, server, stress_clients=0, stress_operations=30, sync_every=1, rust_writers=4, rust_readers=3, edit=False, seed=710, conflict_clients=0, abrupt=False, embedded_smb=False, maintenance=False, fixture=None, disconnect=False, client_timeout=600, offline=False, offline_outage=False, offline_lost_reply=False, client_profile="debug", document_operations=False, record_writes=False, offline_client_reply=False):
    if fixture is not None and not stress_clients:
        raise ValueError('Use a fixture with stress mode.')
    if linux_vm.instance_path(server).exists():
        raise ValueError('Choose a new Linux VM name; existing machines are not owned by this run.')
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    mount = output / 'mount'
    mount.mkdir()
    scripts = output / 'scripts'
    scripts.mkdir()
    for name in ('cold.ps1', 'collaborate.ps1', 'network.ps1', 'stress.ps1', 'text.ps1'):
        shutil.copyfile(ROOT / 'tools/native' / name, scripts / name)
    harness = ('native_collaboration.py', 'native_maintenance.py', 'native_disconnect.py', 'native_stress.py', 'offline_history.py', 'offline_document_history.py', 'offline_outage.py', 'verify_offline.py', 'concurrent_rust.py', 'native_runner.py',
               'crash_recovery.py', 'smb-proxy.py', 'verify_smb_overlap.py', 'w7/crash.py', 'w7/vm.py', 'w7/linux_vm.py')
    for name in harness:
        saved = output / 'harness' / name
        saved.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / 'tools' / name, saved)
    (output / 'run.json').write_text(json.dumps({'server': server, 'stress_clients': stress_clients, 'conflict_clients': conflict_clients, 'stress_operations': stress_operations, 'sync_every': sync_every, 'rust_writers': rust_writers, 'rust_readers': rust_readers, 'edit': edit, 'seed': seed, 'abrupt': abrupt, 'embedded_smb': embedded_smb, 'maintenance': maintenance, 'fixture': str(fixture) if fixture is not None else None,
        'disconnect': disconnect, 'client_timeout': client_timeout, 'client_profile': client_profile, 'offline': offline, 'document_operations': document_operations, 'record_writes': record_writes, 'offline_outage': offline_outage, 'offline_lost_reply': offline_lost_reply, 'offline_client_reply': offline_client_reply, 'harness_sha256': {name: hashlib.sha256((output / 'harness' / name).read_bytes()).hexdigest() for name in harness}, 'scripts': {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in scripts.iterdir()}}, indent=2))

    def ssh(text):
        result = linux_vm.run_ssh(server, text, timeout=90)
        with (output / 'server.jsonl').open('a') as log:
            log.write(json.dumps({'command': text, 'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}) + '\n')
        result.check_returncode()
        return result.stdout

    try:
        if not linux_vm.instance_path(server).exists():
            linux_vm.create_instance(server)
            linux_vm.launch(server)
        linux_vm.wait_instance(server, 600)
        config = linux_vm.load_instance(server)
        (output / 'linux.json').write_text(json.dumps(config, indent=2))
        if embedded_smb:
            subprocess.run(linux_vm.ssh_argv(server, 'cat > /tmp/smb-proxy.py'),
                           input=(output / 'harness/smb-proxy.py').read_bytes(), check=True)
            ssh("sudo sed -i '/^\\[global\\]/a smb ports = 1445' /etc/samba/smb.conf && sudo systemctl restart smbd")
            subprocess.run(linux_vm.ssh_argv(server, 'cat > /tmp/smb-control.json'),
                           input=json.dumps({'record_writes': record_writes}).encode(), check=True)
            ssh("sudo sh -c 'nohup python3 /tmp/smb-proxy.py /tmp/smb-control.json --port 445 --bind 0.0.0.0 --server 127.0.0.1 --server-port 1445 > /tmp/smb-trace.jsonl 2>&1 < /dev/null &'")
            ssh("sleep 1; sudo ss -ltn | grep ':445 '")
            os.environ['ONESTORE_SMB_LAB'] = f'127.0.0.1:{config["samba_port"]}'
            os.environ['ONESTORE_SMB_SHARE'] = 'agent'
        def unmount(force=False):
            for options in ([['-f']] if force else [[], ['-f']]):
                if not os.path.ismount(mount): return
                result = subprocess.run(['/sbin/umount', *options, str(mount)], capture_output=True, text=True, timeout=60)
                with (output / 'unmounts.jsonl').open('a') as log:
                    log.write(json.dumps({'force': bool(options), 'exit': result.returncode, 'stderr': result.stderr}) + '\n')
            if os.path.ismount(mount):
                raise RuntimeError('The owned SMB mount remains attached; preserve its server until it is unmounted.')
        def reconnect_mount():
            unmount()
            subprocess.run(['/sbin/mount_smbfs', '-N', f'//guest@127.0.0.1:{config["samba_port"]}/agent', mount], check=True, stdin=subprocess.DEVNULL)
        ssh('mkdir /srv/agent/m6-collaboration')
        source = ROOT / 'corpus/native-ink/cold-ui-ink/notebook'
        if stress_clients or conflict_clients or abrupt:
            source = output / 'input'
            if fixture is not None:
                source.mkdir()
                for name in ('synthetic.one', 'Open Notebook.onetoc2'):
                    shutil.copyfile(Path(fixture) / name, source / name)
            elif maintenance:
                subprocess.run([ROOT / 'target/debug/examples/maintenance_fixture', source, 'Concurrent edits:'], check=True)
            else:
                subprocess.run([ROOT / 'target/debug/examples/create_notebook', source, 'Concurrent edits:', 'Concurrency test'], check=True)
        with tarfile.open(output / 'input.tar', 'w', dereference=True) as archive:
            for name in ('synthetic.one', 'Open Notebook.onetoc2'):
                archive.add(source / name, arcname=name)
        with (output / 'input.tar').open('rb') as stream:
            subprocess.run(linux_vm.ssh_argv(server, 'tar xf - -C /srv/agent/m6-collaboration'), stdin=stream, check=True)
        ssh('chmod u+w /srv/agent/m6-collaboration/*')
        reconnect_mount()
        shared = mount / 'm6-collaboration'
        assert (shared / 'synthetic.one').read_bytes() == (source / 'synthetic.one').read_bytes()
        for probe in ('sudo smbd --version', 'uname -a', 'testparm -s 2>&1'):
            ssh(probe)
        with ExitStack() as stack:
            def start_controller(client, resume=False):
                name, folder = client['name'], client['folder']
                command(name, 'if exist C:\\one-tests\\runs\\capture\\ready del C:\\one-tests\\runs\\capture\\ready', folder)
                options = f' -ResumeCache -StartSequence {client["sequence"] + 1}' if resume else ''
                result = windows.do_spawn('cmd /c powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\\one-tests\\collaborate.ps1 -Root C:\\one-tests\\runs\\capture -SharedPath \\\\192.168.77.1\\agent\\m6-collaboration -CloneHost ONE-' + name.upper() + options + f' > C:\\one-tests\\runs\\capture\\controller-{client["sequence"]}.log 2>&1', name)
                (folder / f'spawn-{client["sequence"]}.json').write_text(json.dumps(result, indent=2))
                if result.get('error'): raise RuntimeError(result['error'])
                deadline = time.monotonic() + 120
                while time.monotonic() < deadline:
                    result = windows.do_cmd('if exist C:\\one-tests\\runs\\capture\\ready (echo ready)', target=name)
                    if result.get('exit') == 0 and 'ready' in result.get('stdout', ''): return
                    time.sleep(1)
                raise RuntimeError('The native collaboration client did not open the shared notebook')

            labels = [f'n{i}' for i in range(stress_clients or conflict_clients)] if stress_clients or conflict_clients else ['a', 'b']
            for label in labels:
                (output / label).mkdir()
            ownership = Lock()
            def start_clone(label):
                manager = clone(output / label)
                name = manager.__enter__()
                with ownership:
                    stack.push(manager)
                return name
            # Joining workers before stack exit retains ownership even if another boot fails.
            with ThreadPoolExecutor(max_workers=len(labels)) as pool:
                names = dict(zip(labels, pool.map(start_clone, labels)))
            def prepare_client(label):
                folder = output / label
                name = names[label]
                for local in scripts.iterdir():
                    remote = 'cold-current.ps1' if local.name == 'cold.ps1' else local.name
                    result = windows.do_put(local, 'C:\\one-tests\\' + remote, name)
                    if result.get('error'): raise RuntimeError(result['error'])
                command(name, 'mkdir C:\\one-tests\\runs\\capture', folder)
                command(name, 'powershell -NoProfile -ExecutionPolicy Bypass -File C:\\one-tests\\network.ps1 -LabMac ' + vm.lab_mac(name), folder)
                command(name, 'ipconfig', folder)
                command(name, 'dir \\\\192.168.77.1\\agent\\m6-collaboration', folder)
                client = {'name': name, 'folder': folder, 'sequence': 0}
                start_controller(client)
                command(name, 'ipconfig', folder)
                print('Collaboration ready:', label, name, flush=True)
                return client
            with ThreadPoolExecutor(max_workers=len(labels)) as pool:
                clients = list(pool.map(prepare_client, labels))

            def action(client, action, wait=True, **parameters):
                client['sequence'] += 1
                sequence = client['sequence']
                local = client['folder'] / f'command-{sequence:04}.json'
                local.write_text(json.dumps({'action': action, **parameters}, ensure_ascii=False))
                inbox = f'C:\\one-tests\\runs\\capture\\inbox\\{sequence:04}'
                result = windows.do_put(local, inbox + '.tmp', client['name'])
                if result.get('error'): raise RuntimeError(result['error'])
                command(client['name'], f'move {inbox}.tmp {inbox}.json', client['folder'])
                if not wait: return sequence
                return wait_action(client, sequence, action)

            def wait_action(client, sequence, action):
                deadline = time.monotonic() + 600
                remote = f'C:\\one-tests\\runs\\capture\\outbox\\{sequence}'
                while time.monotonic() < deadline:
                    result = windows.do_cmd(f'if exist {remote}\\error (type {remote}\\error & exit /b 1) else (if exist {remote}\\done (echo complete))', target=client['name'])
                    if result.get('exit') != 0: raise RuntimeError(str(result))
                    if 'complete' in result.get('stdout', ''): break
                    time.sleep(.5)
                else: raise TimeoutError(f'Native command {sequence} did not complete')
                if action == 'snapshot':
                    hierarchy = client['folder'] / f'hierarchy-{sequence:04}.xml'
                    result = windows.do_get(remote + '\\hierarchy.xml', hierarchy, client['name'])
                    if result.get('error'): raise RuntimeError(result['error'])
                    xml = hierarchy.read_text(encoding='utf-8-sig').strip()
                    if xml == '<?xml version="1.0"?>': return []
                    pages = [node for node in ET.fromstring(xml).iter() if node.tag.endswith('}Page')]
                    observed = []
                    for i in range(len(pages)):
                        destination = client['folder'] / f'snapshot-{sequence:04}-{i}.xml'
                        result = windows.do_get(remote + f'\\page-{i}.xml', destination, client['name'])
                        if result.get('error'): raise RuntimeError(result['error'])
                        observed.extend(texts(ET.parse(destination).getroot()))
                    return observed

            def wait_text(client, expected):
                deadline = time.monotonic() + 120
                while time.monotonic() < deadline:
                    action(client, 'sync')
                    observed = action(client, 'snapshot')
                    if all(text in observed for text in expected): return observed
                    time.sleep(1)
                raise AssertionError(('Native synchronization did not converge', expected, observed))

            @contextmanager
            def locked_file(name):
                deadline = time.monotonic() + 120
                while True:
                    try:
                        descriptor = os.open(shared / name, os.O_RDONLY | os.O_EXLOCK | os.O_NONBLOCK)
                        break
                    except OSError as error:
                        with (output / 'read-retries.jsonl').open('a') as log:
                            log.write(json.dumps({'file': name, 'errno': error.errno, 'error': str(error)}) + '\n')
                        if error.errno not in (errno.ENOENT, errno.EACCES, errno.EAGAIN): raise
                        if time.monotonic() >= deadline: raise
                        time.sleep(.1)
                with os.fdopen(descriptor, 'rb') as stream:
                    yield stream

            def snapshot_file(name):
                with locked_file(name) as stream:
                    return stream.read()

            def checkpoint(label):
                if embedded_smb: reconnect_mount()
                deadline, previous, incomplete = time.monotonic() + 120, None, 0
                while time.monotonic() < deadline:
                    snapshot = {name: snapshot_file(name) for name in ('synthetic.one', 'Open Notebook.onetoc2')}
                    if snapshot == previous:
                        time.sleep(.1)
                        continue
                    previous = snapshot
                    target = output / label / 'notebook'
                    target.mkdir(parents=True)
                    for name, data in snapshot.items(): (target / name).write_bytes(data)
                    result = subprocess.run([ROOT / 'target/debug/examples/document', target / 'synthetic.one', target.parent / 'model'], capture_output=True, text=True)
                    if result.returncode == 0:
                        (target.parent / 'locks.txt').write_text(ssh('sudo smbstatus --locks'))
                        print('Captured collaboration state:', label, flush=True)
                        return
                    (target.parent / 'parse-error.txt').write_text(result.stderr)
                    if result.stderr.strip() != 'Error: Error { offset: 0, message: "Document context has no revision" }':
                        result.check_returncode()
                    incomplete += 1
                    saved = output / f'{label}-incomplete-{incomplete}'
                    target.parent.rename(saved)
                    print('Preserved incomplete native document:', saved.name, flush=True)
                    time.sleep(.1)
                raise TimeoutError(f'Native document references did not settle: {label}')

            if abrupt and not conflict_clients:
                from concurrent_rust import running_clients
                from crash_recovery import active_text, verify_text
                import crash

                action(clients[0], 'prepare-stress', clients=len(clients))
                native_text = [f'Native {i}:' for i in range(len(clients))]
                for client in clients: wait_text(client, ['Concurrent edits:', *native_text])
                checkpoint('crash-initial')
                folder = output / 'rust-interrupted'
                folder.mkdir()
                try:
                    with running_clients(folder, shared / 'synthetic.one', rust_writers, rust_readers, 500, seed) as processes:
                        (folder / 'start').touch()
                        for i, client in enumerate(clients):
                            changed = native_text[i] + ' Before server stop.'
                            action(client, 'edit', expected=native_text[i], text=changed)
                            native_text[i] = changed
                        for client in clients: action(client, 'sync')
                        for client in clients: wait_text(client, native_text)
                        deadline = time.monotonic() + 60
                        while True:
                            commits = sum(line.count('"event":"commit"') for path in folder.glob('w*.jsonl') for line in path.read_text().splitlines())
                            if commits >= 10: break
                            if any(p.poll() is not None for p in processes.values()) or time.monotonic() >= deadline:
                                raise AssertionError('Writers did not overlap the server interruption')
                            time.sleep(.02)
                        assert all(p.poll() is None for p in processes.values()), 'A client stopped before the planned interruption'
                        (output / 'server-crash.json').write_text(json.dumps(crash.stop('linux', server), indent=2))
                        for client in clients: vm.qmp(client['name'], 'set_link', {'name': 'lab', 'up': False})
                        for process in processes.values(): process.terminate()
                        unmount(force=True)
                        raise InterruptedError('Recorded server interruption')
                except InterruptedError:
                    pass
                logs = {actor: [json.loads(line) for line in (folder / f'{actor}.jsonl').read_text().splitlines()] for actor in processes}
                assert not list(folder.glob('invalid-*.one')), 'A client captured malformed storage'
                linux_vm.launch(server)
                linux_vm.wait_instance(server, 600)
                recovered = output / 'server-before-native-reconnect'
                recovered.mkdir()
                with (recovered / 'synthetic.one').open('wb') as stream:
                    subprocess.run(linux_vm.ssh_argv(server, 'cat /srv/agent/m6-collaboration/synthetic.one'), stdout=stream, check=True, timeout=60)
                subprocess.run([ROOT / 'target/debug/examples/document', recovered / 'synthetic.one', recovered / 'model'], check=True)
                model = json.loads((recovered / 'model/document.json').read_text())
                text = active_text(model)
                retained = verify_text('Concurrent edits:', text, logs)
                (output / 'server-retention.json').write_text(json.dumps(retained, indent=2))
                reconnect_mount()
                for client in clients: vm.qmp(client['name'], 'set_link', {'name': 'lab', 'up': True})
                for client in clients: action(client, 'sync')
                for client in clients: wait_text(client, [text, *native_text])
                checkpoint('server-recovered')

                client = clients[0]
                pending_text = native_text[0] + ' Offline cache edit.'
                with locked_file('synthetic.one'):
                    vm.qmp(client['name'], 'set_link', {'name': 'lab', 'up': False})
                action(client, 'edit', expected=native_text[0], text=pending_text)
                assert pending_text in action(client, 'snapshot')
                (output / 'client-crash.json').write_text(json.dumps(crash.stop('windows', client['name']), indent=2))
                checkpoint('client-stopped')
                stopped = json.loads((output / 'client-stopped/model/document.json').read_text())
                assert pending_text not in [value for values in reachable_page_text(stopped).values() for value in values], 'The pending cache edit already reached the server'
                vm.start_instance(client['name'])
                vm.wait_instance(client['name'], 300)
                command(client['name'], 'powershell -NoProfile -ExecutionPolicy Bypass -File C:\\one-tests\\network.ps1 -LabMac ' + vm.lab_mac(client['name']), client['folder'])
                start_controller(client, resume=True)
                deadline = time.monotonic() + 120
                while True:
                    action(client, 'sync')
                    observed = action(client, 'snapshot')
                    if pending_text in observed or native_text[0] in observed: break
                    if time.monotonic() >= deadline:
                        raise AssertionError('Recovered cache has neither acknowledged version')
                    time.sleep(1)
                survived = pending_text in observed
                native_text[0] = pending_text if survived else native_text[0]
                (output / 'native-cache-result.json').write_text(json.dumps({'cache_acknowledged': pending_text, 'retained_after_abrupt_stop': survived, 'server_durability_acknowledged': False}, indent=2))
                for client in clients: wait_text(client, [text, *native_text])
                checkpoint('client-recovered')
                recovered_model = json.loads((output / 'client-recovered/model/document.json').read_text())
                recovered_pages = reachable_page_text(recovered_model)
                main_space, *conflict_spaces = recovered_pages
                known = {'Concurrent edits:', *[f'Native {i}:' for i in range(len(clients))], *native_text, pending_text}
                conflicts = {sid: recovered_pages[sid] for sid in conflict_spaces}
                assert all(value in known or (value.endswith(']') and text.startswith(value))
                           for values in conflicts.values() for value in values), 'A recovered conflict contains unrecorded content'
                (output / 'cache-conflicts.json').write_text(json.dumps(conflicts, indent=2))

                continued = output / 'rust-continued'
                continued.mkdir()
                with running_clients(continued, shared / 'synthetic.one', rust_writers, rust_readers, stress_operations, seed + 1) as processes:
                    (continued / 'start').touch()
                continuation = {actor: [json.loads(line) for line in (continued / f'{actor}.jsonl').read_text().splitlines()] for actor in processes}
                checkpoint('continued')
                model = json.loads((output / 'continued/model/document.json').read_text())
                final_text = active_text(model)
                result = verify_text(text, final_text, continuation)
                for client in clients: wait_text(client, [final_text, *native_text])
                (output / 'result.json').write_text(json.dumps({'server': retained, 'continuation': result, 'native_cache_retained': survived, 'expected_text': sorted([final_text, *native_text])}, indent=2))
            elif conflict_clients:
                from concurrent_rust import running_clients
                from native_stress import edit_history

                original = 'Concurrent edits:'
                native = [f'Native conflict {i}: café 🦀' for i in range(conflict_clients)]
                for client in clients: wait_text(client, [original])
                checkpoint('initial')
                try:
                    with locked_file('synthetic.one'):
                        for client in clients: vm.qmp(client['name'], 'set_link', {'name': 'lab', 'up': False})
                    for client, text in zip(clients, native):
                        action(client, 'edit', expected=original, text=text)
                        assert text in action(client, 'snapshot'), 'Native cache did not retain its intended edit'
                    folder = output / 'rust'
                    folder.mkdir()
                    with running_clients(folder, shared / 'synthetic.one', rust_writers, rust_readers, stress_operations, seed, edit=edit) as processes:
                        (folder / 'start').touch()
                    logs = {actor: [json.loads(line) for line in (folder / f'{actor}.jsonl').read_text().splitlines()] for actor in processes}
                    commits, rust_text = edit_history(logs, stress_operations, edit)
                    checkpoint('rust-before-reconnect')
                finally:
                    for client in clients: vm.qmp(client['name'], 'set_link', {'name': 'lab', 'up': True})
                expected = {rust_text, *native}
                deadline = time.monotonic() + 120
                while True:
                    for client in clients: action(client, 'sync')
                    checkpoint_label = f'convergence-{clients[0]["sequence"]}'
                    checkpoint(checkpoint_label)
                    model = json.loads((output / checkpoint_label / 'model/document.json').read_text())
                    retained = reachable_page_text(model)
                    if sorted(expected) == sorted(text for texts in retained.values() for text in texts): break
                    if time.monotonic() >= deadline:
                        (output / 'lost-edit.json').write_text(json.dumps({'expected': sorted(expected),
                            'reachable_page_text': retained, 'checkpoint': checkpoint_label}, indent=2))
                        raise AssertionError(('Competing Rust/native edits differ from recorded results', sorted(expected), retained))
                    time.sleep(1)
                checkpoint('conflict-final')
                (output / 'live-retention.json').write_text(json.dumps(retained, indent=2))
                if abrupt:
                    import crash
                    with locked_file('synthetic.one') as stream:
                        os.fsync(stream.fileno())
                    (output / 'conflict-server-crash.json').write_text(json.dumps(crash.stop('linux', server), indent=2))
                    for client in clients: vm.qmp(client['name'], 'set_link', {'name': 'lab', 'up': False})
                    linux_vm.launch(server)
                    linux_vm.wait_instance(server, 600)
                    reconnect_mount()
                    checkpoint('conflict-server-recovered')
                    model = json.loads((output / 'conflict-server-recovered/model/document.json').read_text())
                    observed = [text for texts in reachable_page_text(model).values() for text in texts]
                    assert sorted(observed) == sorted(expected), 'Server interruption lost a flushed competing result'
                    for client in clients: vm.qmp(client['name'], 'set_link', {'name': 'lab', 'up': True})
                    for client in clients: action(client, 'sync')
            elif stress_clients:
                from native_stress import exercise
                exercise(output, shared, clients, action, wait_action, wait_text, checkpoint, stress_operations, sync_every, rust_writers, rust_readers, edit, seed, embedded_smb)
            else:
                a, b = clients
                original = 'Fictitious: café, 東京, مرحبا'
                rust_text = 'Rust disjoint edit.'.ljust(len(original.encode('utf-16-le')) // 2, '.')
                other = 'Fictitious positioned outline.'
                for client in clients: wait_text(client, [original, other])
                checkpoint('initial')
                subprocess.run([ROOT / 'target/debug/examples/edit_property', shared / 'synthetic.one', '--in-place', '6000e', '1c001c22', (original + '\0').encode('utf-16-le').hex(), (rust_text + '\0').encode('utf-16-le').hex()], check=True)
                action(a, 'edit', expected=other, text='Native disjoint edit.')
                for client in clients: wait_text(client, [rust_text, 'Native disjoint edit.'])
                checkpoint('disjoint')
                ssh('sudo systemctl stop smbd && ! systemctl is-active --quiet smbd')
                try:
                    action(a, 'edit', expected=rust_text, text='Client A concurrent edit.')
                    action(b, 'edit', expected=rust_text, text='Client B concurrent edit.')
                finally:
                    ssh('sudo systemctl start smbd && systemctl is-active smbd')
                reconnect_mount()
                for client in clients: action(client, 'sync')
                deadline = time.monotonic() + 120
                while time.monotonic() < deadline:
                    for client in clients: action(client, 'sync')
                    temporary = output / 'conflict-model'
                    if temporary.exists(): shutil.rmtree(temporary)
                    snapshot = output / 'conflict.one'
                    snapshot.write_bytes(snapshot_file('synthetic.one'))
                    subprocess.run([ROOT / 'target/debug/examples/document', snapshot, temporary], check=True)
                    model = json.loads((temporary / 'document.json').read_text())
                    values = [node['kind'].get('text') for space in model['spaces'].values() for revision in space['revisions'].values() for node in revision['nodes'].values()]
                    if 'Client A concurrent edit.' in values and 'Client B concurrent edit.' in values: break
                    time.sleep(1)
                else: raise AssertionError('Both concurrent edits were not retained')
                checkpoint('conflict')
                ssh('sudo systemctl restart smbd && systemctl is-active smbd')
                reconnect_mount()
                for client in clients: wait_text(client, ['Native disjoint edit.'])
                checkpoint('server-restart')
                vm.qmp(a['name'], 'set_link', {'name': 'lab', 'up': False})
                try:
                    action(a, 'edit', expected='Native disjoint edit.', text='Client A offline edit.')
                    assert 'Client A offline edit.' in action(a, 'snapshot')
                    action(b, 'edit', expected='Left cell', text='Client B online cell.')
                    wait_text(b, ['Client B online cell.', 'Native disjoint edit.'])
                    checkpoint('transport-disconnected')
                finally:
                    vm.qmp(a['name'], 'set_link', {'name': 'lab', 'up': True})
                for client in clients: wait_text(client, ['Client A offline edit.', 'Client B online cell.'])
                checkpoint('transport-reconnected')
                with os.fdopen(os.open(shared / 'synthetic.one', os.O_RDONLY | os.O_EXLOCK | os.O_NONBLOCK), 'rb') as locked:
                    before = locked.read()
                    action(a, 'edit', expected='Client A offline edit.', text='Native lock recovery.')
                    action(a, 'sync')
                    assert 'Native lock recovery.' in action(a, 'snapshot')
                    locked.seek(0)
                    assert locked.read() == before, 'The shared file changed while exclusively locked'
                    (output / 'contention.json').write_text(json.dumps({'unchanged_sha256': hashlib.sha256(before).hexdigest(), 'locks': ssh('sudo smbstatus --locks')}, indent=2))
                for client in clients: wait_text(client, ['Native lock recovery.', 'Client B online cell.'])
                checkpoint('lock-released')
                action(a, 'kill-process')
                for client in clients: wait_text(client, ['Native lock recovery.', 'Client B online cell.'])
                checkpoint('process-restarted')
                vm.qmp(a['name'], 'quit')
                deadline = time.monotonic() + 10
                while vm.running(a['name']) and time.monotonic() < deadline: time.sleep(.1)
                assert not vm.running(a['name']), 'The owned clone did not terminate'
                vm.start_instance(a['name'])
                vm.wait_instance(a['name'], 300)
                command(a['name'], 'powershell -NoProfile -ExecutionPolicy Bypass -File C:\\one-tests\\network.ps1 -LabMac ' + vm.lab_mac(a['name']), a['folder'])
                start_controller(a, resume=True)
                for client in clients: wait_text(client, ['Native lock recovery.', 'Client B online cell.'])
                checkpoint('vm-restarted')
                final = json.loads((output / 'vm-restarted/model/document.json').read_text())
                (output / 'retention.json').write_text(json.dumps(verify_final_state(final), indent=2))
            for client in clients:
                action(client, 'close')
                collect_artifacts(client['name'], client['folder'], '*')
            if abrupt and not conflict_clients:
                checkpoint('crash-closed')
                model = json.loads((output / 'crash-closed/model/document.json').read_text())
                actual = reachable_page_text(model)
                assert actual.pop(main_space) == sorted([final_text, *native_text]), 'Application closure changed recovered edits'
                assert actual == conflicts, 'Application closure changed recovered conflict pages'
            elif stress_clients:
                checkpoint('stress-closed')
            elif conflict_clients:
                checkpoint('conflict-closed')
                model = json.loads((output / 'conflict-closed/model/document.json').read_text())
                retained = reachable_page_text(model)
                assert sorted(expected) == sorted(text for texts in retained.values() for text in texts), 'Closing native clients changed the competing results'
                (output / 'result.json').write_text(json.dumps({'native_writers': conflict_clients,
                    'rust_writers': rust_writers, 'rust_readers': rust_readers, 'rust_commits': len(commits),
                    'competing_results': sorted(expected), 'reachable_page_text': retained}, indent=2))
        if not (stress_clients or conflict_clients or abrupt): (output / 'result.json').write_text(json.dumps({'disjoint': True, 'conflict_retained': True, 'server_restart': True, 'transport_reconnect': True, 'lock_contention': True, 'process_restart': True, 'vm_restart': True}, indent=2))
    except BaseException:
        if linux_vm.instance_path(server).exists() and linux_vm.running(server):
            failure = output / 'failure-after-client-shutdown'
            failure.mkdir(exist_ok=True)
            try:
                for name in ('synthetic.one', 'Open Notebook.onetoc2'):
                    with (failure / name).open('wb') as stream:
                        subprocess.run(linux_vm.ssh_argv(server, f'cat /srv/agent/m6-collaboration/"{name}"'),
                                       stdout=stream, stderr=subprocess.PIPE, check=True, timeout=30)
            except Exception as error:
                (failure / 'capture-error.txt').write_text(str(error))
        raise
    finally:
        if embedded_smb and linux_vm.instance_path(server).exists() and linux_vm.running(server):
            try:
                with (output / 'smb-trace.jsonl').open('wb') as trace:
                    subprocess.run(linux_vm.ssh_argv(server, 'cat /tmp/smb-trace.jsonl'), stdout=trace, check=True, timeout=30)
            except Exception as error:
                (output / 'trace-error.txt').write_text(str(error))
        if os.path.ismount(mount): unmount()
        if linux_vm.instance_path(server).exists():
            try:
                if linux_vm.running(server): linux_vm.shutdown(server, 60)
            finally:
                if linux_vm.running(server): linux_vm.qmp(server, 'quit')
                deadline = time.monotonic() + 10
                while linux_vm.running(server) and time.monotonic() < deadline: time.sleep(.1)
                linux_vm.delete_instance(server)
        (output / 'teardown.json').write_text(json.dumps({'linux_absent': not linux_vm.instance_path(server).exists()}, indent=2))
    if embedded_smb:
        from verify_smb_overlap import verify
        with (output / 'smb-trace.jsonl').open() as trace:
            overlap = verify(json.loads(line) for line in trace)
        (output / 'overlap.json').write_text(json.dumps(overlap, indent=2))
        if offline_lost_reply:
            from offline_outage import verify_lost_reply
            (output / 'offline-lost-reply-verification.json').write_text(json.dumps(verify_lost_reply(output), indent=2))
        if offline_outage:
            from offline_outage import verify_outage
            (output / 'offline-outage-verification.json').write_text(json.dumps(verify_outage(output), indent=2))
        if disconnect:
            from native_disconnect import verify_disconnect
            (output / 'disconnect-verification.json').write_text(json.dumps(verify_disconnect(output), indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--linux', required=True, help='Disposable Linux VM owned by this run; it is deleted on exit.')
    parser.add_argument('--stress-clients', type=int, default=0)
    parser.add_argument('--conflict-clients', type=int, default=0, help='Native clients editing the same paragraph offline while Rust writes the server copy.')
    parser.add_argument('--stress-operations', type=int, default=30)
    parser.add_argument('--sync-every', type=int, default=1, help='Native edits between explicit sync requests; zero uses background sync.')
    parser.add_argument('--rust-writers', type=int, default=4)
    parser.add_argument('--rust-readers', type=int, default=3)
    parser.add_argument('--edit', action='store_true', help='Use random text replacements in every writer.')
    parser.add_argument('--seed', type=int, default=710)
    parser.add_argument('--abrupt', action='store_true', help='Abrupt server and client stops with preserved-disk recovery and intent accounting.')
    parser.add_argument('--embedded-smb', action='store_true', help='Run Rust stress clients through the embedded SMB adapter.')
    parser.add_argument('--maintenance', action='store_true', help='Pause the workload for the owned maintenance controller.')
    parser.add_argument('--offline', action='store_true', help='Use durable local queues and traced offline workers for Rust writers.')
    parser.add_argument('--document-operations', action='store_true', help='Queue paragraph/outline creation and formatting alongside offline text edits.')
    parser.add_argument('--record-writes', action='store_true', help='Retain owned lab write payloads for revision replay.')
    parser.add_argument('--offline-lost-reply', action='store_true', help='Drop a publication reply and require confirmation of its original revision.')
    parser.add_argument('--offline-client-reply', action='store_true', help='Disconnect only the formatting writer and require peer publication before it reconciles.')
    parser.add_argument('--offline-outage', action='store_true', help='Queue local edits during an owned SMB outage, then require recovery.')
    parser.add_argument('--disconnect', action='store_true', help='Interrupt and reconnect the embedded append workload twice.')
    parser.add_argument('--client-profile', choices=('debug', 'release'), default='debug', help='Cargo build profile for Rust stress clients')
    parser.add_argument('--client-timeout', type=float, default=600, help='Maximum seconds for the Rust workload, including its start barrier.')
    parser.add_argument('--fixture', type=Path, help='Copy this fixture directory into the disposable stress notebook.')
    args = parser.parse_args()
    if not 0 < args.client_timeout < 2**64 / 1000: parser.error('Choose a finite positive client timeout.')
    if args.maintenance and not (args.embedded_smb and args.stress_clients): parser.error('--maintenance requires embedded SMB stress mode.')
    if args.document_operations and not args.offline: parser.error('--document-operations requires --offline.')
    if args.record_writes and not args.embedded_smb: parser.error('--record-writes requires --embedded-smb.')
    if args.offline_client_reply and not (args.offline_lost_reply and args.document_operations): parser.error('--offline-client-reply requires --offline-lost-reply and --document-operations.')
    if args.offline_lost_reply and (not args.offline or args.offline_outage or args.sync_every or args.stress_operations < 8): parser.error('--offline-lost-reply requires --offline, --sync-every 0, at least eight operations and no --offline-outage.')
    if args.offline_outage and (not args.offline or args.sync_every or args.stress_operations < 8): parser.error('--offline-outage requires --offline, --sync-every 0 and at least eight operations.')
    if args.offline and (not args.embedded_smb or not args.stress_clients or args.edit or args.disconnect or args.maintenance): parser.error('--offline requires embedded append stress without disconnect or maintenance.')
    if args.embedded_smb and not args.stress_clients: parser.error('--embedded-smb requires --stress-clients.')
    if args.disconnect and (not args.embedded_smb or args.edit or args.maintenance or args.sync_every): parser.error('--disconnect requires embedded append stress with --sync-every 0 and no maintenance.')
    if args.rust_writers < 2 or args.rust_readers < 1: parser.error('Use at least two Rust writers and one reader.')
    if args.sync_every < 0 or args.stress_operations <= 0: parser.error('Use a nonnegative sync interval and positive operation count.')
    if args.stress_clients and args.stress_clients < 3: parser.error('Stress mode requires at least three native clients.')
    if args.conflict_clients < 0 or (args.conflict_clients and args.stress_clients): parser.error('Choose a positive conflict client count without stress mode.')
    if args.abrupt and (args.stress_clients or args.edit): parser.error('Abrupt recovery uses append intents or the offline-conflict workload.')
    def interrupted(_signal, _frame): raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    replay(args.output, args.linux, args.stress_clients, args.stress_operations, args.sync_every, args.rust_writers, args.rust_readers, args.edit, args.seed, args.conflict_clients, args.abrupt, args.embedded_smb, args.maintenance, args.fixture, args.disconnect, args.client_timeout, args.offline, args.offline_outage, args.offline_lost_reply, args.client_profile, args.document_operations, args.record_writes, args.offline_client_reply)
