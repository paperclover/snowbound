#!/usr/bin/env python3
"""Trace a dedicated test SMB session and cut selected responses before delivery."""
import argparse
import asyncio
import json
from pathlib import Path
import struct
import time


async def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('control', type=Path)
    parser.add_argument('--port', type=int, default=11445)
    parser.add_argument('--server', default='10.0.0.1')
    args = parser.parse_args()
    writers = set()
    state = {'mode': 'up'}
    previous = None
    blocked = False
    matched = 0

    def record(**fields):
        print(json.dumps({'time': time.time(), **fields}), flush=True)

    async def controls():
        nonlocal state, previous, blocked, matched
        while True:
            try:
                raw = args.control.read_bytes()
                if raw != previous:
                    state = json.loads(raw)
                    previous, matched = raw, 0
                    blocked = state.get('mode') == 'down'
                    record(control=state)
                    if blocked:
                        for writer in list(writers):
                            writer.transport.abort()
            except (FileNotFoundError, json.JSONDecodeError):
                pass
            await asyncio.sleep(0.05)

    async def connection(client, client_writer):
        nonlocal blocked, matched
        if blocked:
            client_writer.close()
            return
        server_writer = None
        try:
            server, server_writer = await asyncio.open_connection(args.server, 445)
            writers.update((client_writer, server_writer))

            async def forward(reader, writer, direction):
                nonlocal blocked, matched
                while True:
                    prefix = await reader.readexactly(4)
                    frame = await reader.readexactly(int.from_bytes(prefix[1:], 'big'))
                    at = 0
                    while frame[at:at+4] == b'\xfeSMB':
                        command = struct.unpack_from('<H', frame, at+12)[0]
                        entry = {'direction': direction, 'command': command,
                                 'message': struct.unpack_from('<Q', frame, at+24)[0]}
                        if direction == 'response':
                            entry['status'] = hex(struct.unpack_from('<I', frame, at+8)[0])
                        elif command in (8, 9):
                            entry['length'], entry['offset'] = struct.unpack_from('<IQ', frame, at+68)
                            if command == 9 and entry['offset'] <= 96 and entry['offset'] + entry['length'] >= 100:
                                data_offset = struct.unpack_from('<H', frame, at+66)[0]
                                entry['transactions'] = struct.unpack_from('<I', frame, at+data_offset+96-entry['offset'])[0]
                        elif command == 10:
                            count = struct.unpack_from('<H', frame, at+66)[0]
                            entry['locks'] = [struct.unpack_from('<QQI', frame, at+88+24*i) for i in range(count)]
                        record(**entry)
                        if state.get('cut') == command and direction == 'response' and entry['status'] == state.get('status', '0x0'):
                            matched += 1
                            if matched == state.get('occurrence', 1):
                                blocked = True
                                record(cut=entry)
                                for stream in list(writers):
                                    stream.transport.abort()
                                return
                        next_command = struct.unpack_from('<I', frame, at+20)[0]
                        if not next_command:
                            break
                        at += next_command
                    if frame[:4] == b'\xfdSMB':
                        record(encrypted=True, direction=direction)
                    writer.write(prefix + frame)
                    await writer.drain()

            tasks = [asyncio.create_task(forward(client, server_writer, 'request')),
                     asyncio.create_task(forward(server, client_writer, 'response'))]
            done, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for task in pending:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
        except (OSError, asyncio.IncompleteReadError) as error:
            record(error=str(error))
        finally:
            for writer in (client_writer, server_writer):
                if writer:
                    writers.discard(writer)
                    writer.close()

    server = await asyncio.start_server(connection, '127.0.0.1', args.port)
    async with server:
        record(listening=args.port)
        await asyncio.gather(server.serve_forever(), controls())


if __name__ == '__main__':
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
