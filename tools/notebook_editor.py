#!/usr/bin/env python3
"""Serve the HTML diagnostic editor on a new notebook copy."""
import argparse
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import mimetypes
import os
from pathlib import Path
import re
import shutil
import subprocess
from threading import Lock
import time
from urllib.parse import parse_qs, urlsplit
import uuid

from document_model import walk
from notebook_report import generate

ROOT = Path(__file__).resolve().parent.parent
BRIDGE = ROOT / 'target/debug/onestore-diagnostic'


def bridge(mode, source, destination, edit=None):
    try:
        result = subprocess.run([BRIDGE, mode, source, destination],
                                input=json.dumps(edit) if edit is not None else None,
                                capture_output=True, text=True, timeout=120)
        if result.returncode:
            raise RuntimeError(result.stderr or f'Diagnostic process exited {result.returncode}')
        return json.loads(result.stdout)
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        return {'ok': False, 'state': 'Unknown' if mode == 'commit' else 'NotCommitted',
                'kind': 'Process', 'error': str(error)}


class Session:
    def __init__(self, source, output):
        source = source.resolve(strict=True)
        self.output = output.resolve()
        if self.output.is_relative_to(source):
            raise ValueError('Choose a session directory outside the source notebook.')
        self.output.mkdir(parents=True, exist_ok=False)
        self.lock = Lock()
        hashes = {p.relative_to(source).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                  for p in source.rglob('*') if p.is_file()}
        shutil.copytree(source, self.output / 'notebook')
        for name, digest in hashes.items():
            copied = self.output / 'notebook' / name
            if hashlib.sha256(copied.read_bytes()).hexdigest() != digest or hashlib.sha256((source / name).read_bytes()).hexdigest() != digest:
                raise ValueError('The source changed during copying; start a fresh session.')
            copied.chmod(copied.stat().st_mode | 0o600)
        (self.output / 'source.json').write_text(json.dumps({'root': str(source), 'sha256': hashes}, indent=2))
        (self.output / 'g').mkdir()
        (self.output / 'objects').mkdir()
        self.snapshot()

    def snapshot(self):
        number = max((int(p.name) for p in (self.output / 'g').iterdir() if p.name.isdecimal()), default=-1) + 1
        pending = self.output / 'g' / (str(number) + '-' + uuid.uuid4().hex + '.building')
        source = pending / 'snapshot'
        source.mkdir(parents=True)
        for path in sorted((self.output / 'notebook').rglob('*')):
            if path.suffix.lower() not in ('.one', '.onetoc2'): continue
            saved = source / path.relative_to(self.output / 'notebook')
            saved.parent.mkdir(parents=True, exist_ok=True)
            result = bridge('snapshot', path, saved)
            if not result['ok']: raise RuntimeError(result['error'])
            digest = hashlib.sha256(saved.read_bytes()).hexdigest()
            blob = self.output / 'objects' / digest
            if blob.exists(): saved.unlink()
            else:
                saved.rename(blob)
                blob.chmod(0o444)
            os.link(blob, saved)
        previous = self.output / 'g' / str(number - 1) / 'report' if number else None
        generate(source, pending / 'report', editable=True, previous=previous)
        pending.rename(self.output / 'g' / str(number))
        return number

    def page(self, generation, page):
        if type(generation) is not int or generation < 0:
            raise ValueError('Choose a page from this report.')
        folder = self.output / 'g' / str(generation)
        pages = json.loads((folder / 'report/pages.json').read_text())
        row = next(p for p in pages if p['report'] == page)
        if row['category'] != 'Page':
            raise ValueError('Choose an active page. Conflicts, templates, deleted pages and history are read-only here.')
        sources = json.loads((folder / 'report/source.json').read_text())
        index = next(i for i, source in enumerate(sources) if source['path'] == row['section'])
        model = folder / 'report/model' / str(index)
        document = json.loads((model / 'document.json').read_text())
        revision = document['spaces'][row['space']]['revisions'][row['revision']]
        return row, folder / 'snapshot' / row['section'], revision, model

    def selection(self, generation, page, oid, run):
        if type(run) is not int or run < 0:
            raise ValueError('Choose a text run from this report.')
        row, source, revision, model = self.page(generation, page)
        node = next(node for key, node in walk(revision, row['object']) if key == oid)
        if node['kind']['type'] != 'RichText': raise ValueError('Choose a text run.')
        selected = node['kind']['runs'][run]
        text = json.loads((model / 'text.json').read_text())[row['space']][row['revision']][oid][run]['text']
        request = {'space': row['space'], 'object': oid,
                   'action': {'type': 'Text', 'start': selected['start'], 'end': selected['end'], 'replacement': text}}
        return row, source, request

    def location(self, generation, row=None):
        page = 'index.html'
        if row:
            pages = json.loads((self.output / 'g' / str(generation) / 'report/pages.json').read_text())
            page = next((p['report'] for p in pages if (p['section'], p['space'], p['object'], p['context']) ==
                         (row['section'], row['space'], row['object'], row['context'])), page)
        return f'/g/{generation}/report/{page}'

    def save(self, data):
        fields = {'text': {'run', 'replacement'}, 'format': {'run', 'start', 'end', 'attributes'},
                  'paragraph': {'before', 'text', 'author'}, 'outline': {'x', 'y', 'text', 'author'}}
        action = data.get('action')
        if action not in fields or set(data) != {'generation', 'page', 'object', 'action'} | fields[action]:
            raise ValueError('Choose text, formatting, a paragraph or an outline to save.')
        if action in ('text', 'format'):
            row, source, edit = self.selection(data['generation'], data['page'], data['object'], data['run'])
            selected = edit['action']
            if action == 'text':
                if not isinstance(data['replacement'], str): raise ValueError('Enter replacement text.')
                selected['replacement'] = data['replacement']
            else:
                if (type(data['start']) is not int or type(data['end']) is not int
                        or not 0 <= data['start'] <= data['end'] <= selected['end'] - selected['start']):
                    raise ValueError('Select text within this run.')
                edit['action'] = {'type': 'Format', 'start': selected['start'] + data['start'],
                                  'end': selected['start'] + data['end'], 'attributes': data['attributes']}
        else:
            row, source, revision, _ = self.page(data['generation'], data['page'])
            if data['object'] not in {oid for oid, _ in walk(revision, row['object'])}:
                raise ValueError('Choose a container on this page.')
            edit = {'space': row['space'], 'object': data['object'],
                    'action': {'type': action.title(), **{key: data[key] for key in fields[action]}}}
        operation = uuid.uuid4().hex
        result = {'ok': False, 'state': 'NotCommitted'}
        try:
            with (self.output / 'operations.jsonl').open('a') as log:
                intent = {'event': 'intent', 'operation': operation, 'started_ms': time.time_ns() // 1000000,
                          'selection': {key: data[key] for key in ('generation', 'page', 'object', 'action')},
                          'edit': edit, 'section': row['section']}
                log.write(json.dumps(intent, ensure_ascii=True) + '\n')
                log.flush(); os.fsync(log.fileno())
                result = {'ok': False, 'state': 'Unknown'}
                result = bridge('commit', self.output / 'notebook' / row['section'], source, edit)
                log.write(json.dumps({'event': 'outcome', 'operation': operation, 'finished_ms': time.time_ns() // 1000000, **result}) + '\n')
                log.flush(); os.fsync(log.fileno())
        except Exception as error:
            result = {**result, 'ok': False, 'error': str(error)}
        try:
            generation = self.snapshot()
            result['location'] = self.location(generation, row)
        except Exception as error:
            result = {**result, 'ok': False, 'report_error': str(error)}
        return result


class Handler(BaseHTTPRequestHandler):
    def redirect(self, location):
        self.send_response(302)
        self.send_header('Location', location)
        self.send_header('Cache-Control', 'no-store')
        self.send_header('Content-Length', '0')
        self.end_headers()

    def reply(self, status, value):
        content = json.dumps(value, ensure_ascii=True).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(content)))
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        self.wfile.write(content)

    def do_GET(self):
        session = self.server.session
        url = urlsplit(self.path)
        try:
            if url.path == '/':
                latest = max(int(p.name) for p in (session.output / 'g').iterdir() if p.name.isdecimal())
                self.redirect(session.location(latest))
                return
            if url.path in ('/api/run', '/api/page', '/latest'):
                query = parse_qs(url.query, strict_parsing=True)
                if url.path == '/latest':
                    row = session.page(int(query['generation'][0]), query['page'][0])[0] if query else None
                    with session.lock:
                        self.redirect(session.location(session.snapshot(), row))
                    return
                if url.path == '/api/page':
                    row, _, revision, _ = session.page(int(query['generation'][0]), query['page'][0])
                    def label(oid):
                        node = revision['nodes'][oid]
                        text = ' '.join(n['kind']['text'] for _, n in walk(revision, oid) if n['kind']['type'] == 'RichText')
                        return node['kind']['type'] + (' · ' + text[:80] if text else '')
                    targets = []
                    pending = [row['object']]
                    while pending:
                        oid = pending.pop()
                        node = revision['nodes'][oid]
                        if node['kind']['type'] == 'Title': continue
                        if node['kind']['type'] in ('Outline', 'Paragraph', 'OutlineGroup', 'Cell'):
                            targets.append({'object': oid, 'label': label(oid),
                                            'children': [{'object': child, 'label': label(child)} for child in node['children']]})
                        pending.extend(reversed(node['structure'] + node['content'] + node['children']))
                    self.reply(200, {'ok': True, 'object': row['object'], 'targets': targets})
                    return
                row, source, edit = session.selection(int(query['generation'][0]), query['page'][0], query['object'][0], int(query['run'][0]))
                result = bridge('check', source, '-', edit)
                self.reply(200 if result['ok'] else 422, {**result, 'text': edit['action']['replacement']})
                return
            if url.path in ('/editor.js', '/editor.css'):
                path = ROOT / 'tools/diagnostic' / url.path[1:]
            else:
                match = re.fullmatch(r'/g/(\d+)/report/(.+)', url.path)
                if not match: raise FileNotFoundError()
                root = session.output / 'g' / match[1] / 'report'
                path = (root / match[2]).resolve(strict=True)
                if not path.is_relative_to(root): raise FileNotFoundError()
            with path.open('rb') as file:
                self.send_response(200)
                self.send_header('Content-Type', mimetypes.guess_type(path)[0] or 'application/octet-stream')
                self.send_header('Content-Length', str(os.fstat(file.fileno()).st_size))
                self.send_header('Cache-Control', 'no-store')
                self.end_headers()
                shutil.copyfileobj(file, self.wfile)
        except (ValueError, KeyError, IndexError, StopIteration) as error:
            self.reply(422, {'ok': False, 'error': str(error) or 'The selected text is unavailable.'})
        except OSError:
            self.reply(404, {'ok': False, 'error': 'Report unavailable. Open the notebook index.'})
        except Exception as error:
            self.reply(503, {'ok': False, 'error': str(error)})

    def do_POST(self):
        session = self.server.session
        host = self.headers.get('Host')
        allowed = {f'127.0.0.1:{self.server.server_port}', f'localhost:{self.server.server_port}'}
        if host not in allowed or self.headers.get('Origin', 'http://' + host) != 'http://' + host or self.headers.get('X-OneNote-Diagnostic') != '1':
            self.reply(403, {'ok': False, 'error': 'Open this editor on its local address.'})
            return
        if self.path != '/api/save':
            self.reply(404, {'ok': False, 'error': 'Unknown diagnostic action.'})
            return
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if not 0 < length <= 65536 or self.headers.get('Content-Type') != 'application/json':
                raise ValueError('Send a JSON edit smaller than 64 KiB.')
            data = json.loads(self.rfile.read(length))
            with session.lock:
                result = session.save(data)
            status = 200 if result['ok'] else 409 if result.get('kind') == 'ResourceBusy' else 422 if result.get('kind') in ('InvalidData', 'Input') else 503
            self.reply(status, result)
        except (ValueError, KeyError, IndexError, StopIteration, TypeError) as error:
            self.reply(422, {'ok': False, 'state': 'NotCommitted', 'error': str(error) or 'The selected text is unavailable.'})
        except Exception as error:
            self.reply(503, {'ok': False, 'state': 'Unknown' if self.path == '/api/save' else 'NotCommitted', 'error': str(error)})


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('session', type=Path)
    parser.add_argument('--port', type=int, default=8782)
    args = parser.parse_args()
    server = ThreadingHTTPServer(('127.0.0.1', args.port), Handler)
    server.session = Session(args.source, args.session)
    print(f'Diagnostic editor: http://127.0.0.1:{server.server_port}/', flush=True)
    print(f'Editable notebook copy: {server.session.output / "notebook"}', flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
