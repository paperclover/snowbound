#!/usr/bin/env python3
"""Trace a dedicated test SMB session and cut selected requests or responses."""
import argparse
import asyncio
import json
from pathlib import Path
import struct
import time


def lease_state(frame, at, contexts):
    """The lease state of a CREATE's `RqLs` context, if any (MS-SMB2 2.2.13.2.8, 2.2.14.2.10)."""
    offset, length = struct.unpack_from('<II', frame, at + contexts)
    cursor = at + offset if length else None
    while cursor is not None:
        following, name_offset, name_length, _, data_offset = struct.unpack_from('<IHHHH', frame, cursor)
        if frame[cursor + name_offset:cursor + name_offset + name_length] == b'RqLs':
            return struct.unpack_from('<I', frame, cursor + data_offset + 16)[0]
        cursor = cursor + following if following else None
    return None


def header_fields(offset, data):
    result = {}
    for name, start, length in [('transactions', 96, 4), ('version', 212, 16),
                                 ('generation', 228, 8), ('deny_read', 236, 16)]:
        if offset <= start and start + length <= offset + len(data):
            value = data[start - offset:start - offset + length]
            result[name] = value.hex() if length == 16 else int.from_bytes(value, 'little')
    return result


async def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('control', type=Path)
    parser.add_argument('--port', type=int, default=11445)
    parser.add_argument('--server', default='10.0.0.1')
    parser.add_argument('--server-port', type=int, default=445)
    parser.add_argument('--bind', default='127.0.0.1')
    args = parser.parse_args()
    writers = set()
    state = {'mode': 'up'}
    previous = None
    blocked = False
    matched = 0
    connections = 0
    record_writes = False

    def record(**fields):
        print(json.dumps({'time': time.time(), **fields}), flush=True)

    async def controls():
        nonlocal state, previous, blocked, matched, record_writes
        while True:
            try:
                raw = args.control.read_bytes()
                if raw != previous:
                    state = json.loads(raw)
                    if 'record_writes' in state:
                        record_writes = bool(state['record_writes'])
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
        nonlocal blocked, matched, connections
        connections += 1
        connection_id = connections
        record(connection=connection_id, peer=client_writer.get_extra_info("peername"), opened=True)
        if blocked:
            client_writer.close()
            return
        server_writer = None
        try:
            server, server_writer = await asyncio.open_connection(args.server, args.server_port)
            writers.update((client_writer, server_writer))

            async def forward(reader, writer, direction):
                nonlocal blocked, matched
                while True:
                    prefix = await reader.readexactly(4)
                    frame = await reader.readexactly(int.from_bytes(prefix[1:], 'big'))
                    at = 0
                    while frame[at:at+4] == b'\xfeSMB':
                        command = struct.unpack_from('<H', frame, at+12)[0]
                        entry = {'connection': connection_id, 'direction': direction, 'command': command, 'frame': frames[direction],
                                 'message': struct.unpack_from('<Q', frame, at+24)[0],
                                 'credit_charge': struct.unpack_from('<H', frame, at+6)[0],
                                 'credits': struct.unpack_from('<H', frame, at+14)[0]}
                        if direction == 'response':
                            entry['status'] = hex(struct.unpack_from('<I', frame, at+8)[0])
                        elif command in (8, 9):
                            entry['length'], entry['offset'] = struct.unpack_from('<IQ', frame, at+68)
                            if command == 9:
                                data_offset = struct.unpack_from('<H', frame, at+66)[0]
                                data = frame[at+data_offset:at+data_offset+entry['length']]
                                entry.update(header_fields(entry['offset'], data))
                                if record_writes:
                                    entry['data'] = data.hex()
                        elif command == 10:
                            count = struct.unpack_from('<H', frame, at+66)[0]
                            entry['locks'] = [struct.unpack_from('<QQI', frame, at+88+24*i) for i in range(count)]
                        if command == 18 and (direction == 'request' or entry['status'] == '0x0'):
                            size = struct.unpack_from('<H', frame, at+64)[0]
                            entry['oplock_body'] = frame[at+64:at+64+size].hex()
                        if direction == 'request':
                            if command in (6, 7, 8, 9, 10):
                                offset = 80 if command in (8, 9) else 72
                                entry['file_id'] = frame[at+offset:at+offset+16].hex()
                            elif command == 17:
                                entry['info_type'], entry['info_class'] = struct.unpack_from('<BB', frame, at+66)
                                if record_writes:
                                    entry['file_id'] = frame[at+80:at+96].hex()
                                    length, offset = struct.unpack_from('<IH', frame, at+68)
                                    entry['data'] = frame[at+offset:at+offset+length].hex()
                            elif command == 5:
                                entry['oplock'] = frame[at+67]
                                entry['access'] = struct.unpack_from('<I', frame, at+88)[0]
                                entry['share'], entry['disposition'], entry['options'] = struct.unpack_from('<III', frame, at+96)
                                offset, length = struct.unpack_from('<HH', frame, at+108)
                                entry['path'] = frame[at+offset:at+offset+length].decode('utf-16-le')
                                if entry['oplock'] == 0xff:
                                    entry['lease'] = lease_state(frame, at, 112)
                        elif command == 5 and entry['status'] == '0x0':
                            entry['oplock'] = frame[at+66]
                            entry['file_id'] = frame[at+128:at+144].hex()
                            if entry['oplock'] == 0xff:
                                entry['lease'] = lease_state(frame, at, 144)
                        elif command == 9 and entry['status'] == '0x0':
                            entry['written'] = struct.unpack_from('<I', frame, at+68)[0]
                        if direction == 'request':
                            requests[entry['message']] = entry
                            request = entry
                        else:
                            request = requests.get(entry['message'], {})
                            if entry['status'] != '0x103': requests.pop(entry['message'], None)
                            if command == 8 and entry['status'] == '0x0' and request.get('offset', 252) < 252:
                                data_offset = frame[at+66]
                                length = struct.unpack_from('<I', frame, at+68)[0]
                                entry.update(header_fields(request['offset'], frame[at+data_offset:at+data_offset+length]))
                        record(**entry)
                        if (state.get('cut') == command and direction == state.get('direction', 'response')
                                and (direction == 'request' or entry['status'] == state.get('status', '0x0'))
                                and ('peer' not in state or state['peer'] == client_writer.get_extra_info('peername')[0])
                                and ('offset' not in state or state['offset'] == request.get('offset'))):
                            matched += 1
                            if matched == state.get('occurrence', 1):
                                local = state.get('scope') == 'connection'
                                blocked = not local
                                record(cut=entry)
                                for stream in (client_writer, server_writer) if local else list(writers):
                                    stream.transport.abort()
                                return
                        next_command = struct.unpack_from('<I', frame, at+20)[0]
                        if not next_command:
                            break
                        at += next_command
                    frames[direction] += 1
                    if frame[:4] == b'\xfdSMB':
                        record(encrypted=True, direction=direction)
                    if state.get('delay_ms'):
                        # Half the round trip each way, as on a wide-area link: every frame
                        # arrives that much later, in order, without queueing behind others.
                        loop = asyncio.get_running_loop()
                        loop.call_at(loop.time() + state['delay_ms'] / 2000, writer.write, prefix + frame)
                    else:
                        writer.write(prefix + frame)
                        await writer.drain()

            requests = {}
            frames = {'request': 0, 'response': 0}
            tasks = [asyncio.create_task(forward(client, server_writer, 'request')),
                     asyncio.create_task(forward(server, client_writer, 'response'))]
            done, pending = await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for task in pending:
                task.cancel()
            results = await asyncio.gather(*tasks, return_exceptions=True)
            for result in results:
                if isinstance(result, Exception) and not isinstance(result, (OSError, asyncio.IncompleteReadError)):
                    record(connection=connection_id, trace_error=repr(result))
        except (OSError, asyncio.IncompleteReadError) as error:
            record(error=str(error))
        finally:
            record(connection=connection_id, closed=True)
            for writer in (client_writer, server_writer):
                if writer:
                    writers.discard(writer)
                    writer.close()

    server = await asyncio.start_server(connection, args.bind, args.port)
    async with server:
        record(listening=server.sockets[0].getsockname()[1])
        await asyncio.gather(server.serve_forever(), controls())


if __name__ == '__main__':
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
