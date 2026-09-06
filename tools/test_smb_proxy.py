import importlib.util
import asyncio
import json
from pathlib import Path
import socket
import struct
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('smb_proxy', Path(__file__).with_name('smb-proxy.py'))
proxy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proxy)


class HeaderTrace(unittest.TestCase):
    def test_fields_follow_complete_payload_ranges(self):
        header = bytearray(1024)
        header[96:100] = (257).to_bytes(4, 'little')
        header[212:228] = bytes(range(16))
        header[228:236] = (999).to_bytes(8, 'little')
        header[236:252] = bytes(range(16, 32))
        expected = {'transactions': 257, 'version': bytes(range(16)).hex(),
                    'generation': 999, 'deny_read': bytes(range(16, 32)).hex()}
        self.assertEqual(proxy.header_fields(0, header), expected)
        self.assertEqual(proxy.header_fields(212, header[212:252]), {k:v for k,v in expected.items() if k != 'transactions'})
        self.assertEqual(proxy.header_fields(96, header[96:99]), {})
        self.assertEqual(proxy.header_fields(100, header[100:212]), {})
        self.assertEqual(proxy.header_fields(228, header[228:251]), {'generation':999})
        self.assertEqual(proxy.header_fields(252, bytes(1024)), {})


class WriteTrace(unittest.IsolatedAsyncioTestCase):
    async def test_opt_in_payload_and_partial_write_response_are_traced(self):
        frames = []

        async def respond(reader, writer):
            try:
                for _ in range(3):
                    prefix = await reader.readexactly(4)
                    frame = await reader.readexactly(int.from_bytes(prefix[1:], 'big'))
                    frames.append(frame)
                    reply = bytearray(80)
                    reply[:64] = frame[:64]
                    struct.pack_into('<I', reply, 16, 1)
                    struct.pack_into('<I', reply, 68, 3)
                    writer.write(len(reply).to_bytes(4, 'big') + reply)
                    await writer.drain()
            finally:
                writer.close()
                await writer.wait_closed()

        server = await asyncio.start_server(respond, '127.0.0.1', 0)
        server_port = server.sockets[0].getsockname()[1]
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            proxy_port = reservation.getsockname()[1]
        with tempfile.TemporaryDirectory() as directory:
            control = Path(directory) / 'control.json'
            control.write_text('{}')
            process = await asyncio.create_subprocess_exec(
                sys.executable, str(Path(proxy.__file__)), str(control),
                '--port', str(proxy_port), '--server', '127.0.0.1', '--server-port', str(server_port),
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
            records = []
            writer = None
            try:
                while not any('control' in row for row in records):
                    records.append(json.loads(await asyncio.wait_for(process.stdout.readline(), 5)))
                reader, writer = await asyncio.open_connection('127.0.0.1', proxy_port)
                sent = []
                for message, command in enumerate((9, 9, 17)):
                    if message == 1:
                        control.write_text('{"record_writes":true}')
                        while not any(row.get('control', {}).get('record_writes') for row in records):
                            records.append(json.loads(await asyncio.wait_for(process.stdout.readline(), 5)))
                    if message == 2:
                        control.write_text('{}')
                        while True:
                            row = json.loads(await asyncio.wait_for(process.stdout.readline(), 5))
                            records.append(row)
                            if row.get('control') == {}:
                                break
                    data = b'abcde' if command == 9 else (4096).to_bytes(8, 'little')
                    offset = 112 if command == 9 else 96
                    frame = bytearray(offset)
                    frame[:4] = b'\xfeSMB'
                    struct.pack_into('<H', frame, 12, command)
                    struct.pack_into('<Q', frame, 24, message)
                    frame[80:96] = bytes(range(16))
                    if command == 9:
                        struct.pack_into('<HIQ', frame, 66, offset, len(data), 4096)
                    else:
                        struct.pack_into('<BBIH', frame, 66, 1, 20, len(data), offset)
                    frame += data
                    sent.append(bytes(frame))
                    writer.write(len(frame).to_bytes(4, 'big') + frame)
                    await writer.drain()
                    prefix = await asyncio.wait_for(reader.readexactly(4), 5)
                    await asyncio.wait_for(reader.readexactly(int.from_bytes(prefix[1:], 'big')), 5)
                    while not any(row.get('direction') == 'response' and row['message'] == message for row in records):
                        records.append(json.loads(await asyncio.wait_for(process.stdout.readline(), 5)))
                self.assertEqual(frames, sent)
                requests = [row for row in records if row.get('direction') == 'request']
                self.assertNotIn('data', requests[0])
                self.assertEqual(requests[1]['data'], b'abcde'.hex())
                self.assertEqual(requests[1]['offset'], 4096)
                self.assertEqual(requests[2]['data'], (4096).to_bytes(8, 'little').hex())
                self.assertEqual(requests[2]['file_id'], bytes(range(16)).hex())
                self.assertEqual([row['written'] for row in records if row.get('direction') == 'response' and row['command'] == 9], [3, 3])
            finally:
                if writer is not None:
                    writer.close()
                    await writer.wait_closed()
                if process.returncode is None:
                    process.terminate()
                await asyncio.wait_for(process.wait(), 5)
                server.close()
                await server.wait_closed()

    async def test_connection_cut_keeps_existing_and_new_peers_usable(self):
        async def respond(reader, writer):
            try:
                while True:
                    prefix = await reader.readexactly(4)
                    frame = await reader.readexactly(int.from_bytes(prefix[1:], 'big'))
                    reply = bytearray(80)
                    reply[:64] = frame[:64]
                    struct.pack_into('<I', reply, 16, 1)
                    struct.pack_into('<I', reply, 68, 4)
                    writer.write(len(reply).to_bytes(4, 'big') + reply)
                    await writer.drain()
            except (OSError, asyncio.IncompleteReadError):
                pass
            finally:
                writer.close()
                await writer.wait_closed()

        async def send(writer, message):
            frame = bytearray(116)
            frame[:4] = b'\xfeSMB'
            struct.pack_into('<H', frame, 12, 9)
            struct.pack_into('<Q', frame, 24, message)
            struct.pack_into('<HIQ', frame, 66, 112, 4, 96)
            writer.write(len(frame).to_bytes(4, 'big') + frame)
            await writer.drain()

        async def receive(reader):
            prefix = await asyncio.wait_for(reader.readexactly(4), 5)
            return await asyncio.wait_for(reader.readexactly(int.from_bytes(prefix[1:], 'big')), 5)

        for scope in ('connection', 'all'):
            with self.subTest(scope=scope), tempfile.TemporaryDirectory() as directory:
                server = await asyncio.start_server(respond, '127.0.0.1', 0)
                control = Path(directory) / 'control.json'
                control.write_text('{}')
                process = await asyncio.create_subprocess_exec(
                    sys.executable, str(Path(proxy.__file__)), str(control), '--port', '0',
                    '--server', '127.0.0.1', '--server-port', str(server.sockets[0].getsockname()[1]),
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
                clients, records = [], []
                try:
                    while not any('control' in row for row in records):
                        records.append(json.loads(await asyncio.wait_for(process.stdout.readline(), 5)))
                    port, = [row['listening'] for row in records if 'listening' in row]
                    for index in range(2):
                        reader, writer = await asyncio.open_connection('127.0.0.1', port)
                        clients.append((reader, writer))
                        await send(writer, index)
                        self.assertEqual(struct.unpack_from('<I', await receive(reader), 68)[0], 4)
                    setting = dict(cut=9, offset=96, scope=scope)
                    control.write_text(json.dumps(setting))
                    while not any(row.get('control') == setting for row in records):
                        records.append(json.loads(await asyncio.wait_for(process.stdout.readline(), 5)))
                    await send(clients[0][1], 2)
                    with self.assertRaises(asyncio.IncompleteReadError): await receive(clients[0][0])
                    while not any('cut' in row for row in records):
                        records.append(json.loads(await asyncio.wait_for(process.stdout.readline(), 5)))
                    cut, = [row['cut'] for row in records if 'cut' in row]
                    self.assertEqual((cut['direction'], cut['status'], cut['written']), ('response', '0x0', 4))
                    reader, writer = await asyncio.open_connection('127.0.0.1', port)
                    clients.append((reader, writer))
                    for reader, writer in clients[1:]:
                        if scope == 'connection':
                            await send(writer, 3)
                            self.assertEqual(struct.unpack_from('<I', await receive(reader), 68)[0], 4)
                        else:
                            with self.assertRaises(asyncio.IncompleteReadError): await receive(reader)
                finally:
                    for _, writer in clients:
                        writer.close()
                        await writer.wait_closed()
                    if process.returncode is None: process.terminate()
                    await asyncio.wait_for(process.wait(), 5)
                    server.close()
                    await server.wait_closed()
