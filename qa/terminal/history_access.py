"""Linux: exercise terminal history admission with an actual paired device."""

import argparse
import asyncio
import json
import os
from contextlib import asynccontextmanager

import websockets
from panel_live import disposable_gateway, file_path


@asynccontextmanager
async def connected(url, params):
    async with websockets.connect(url, open_timeout=2, proxy=None) as websocket:
        sequence = 0

        async def call(method, params):
            nonlocal sequence
            sequence += 1
            await websocket.send(json.dumps({
                'jsonrpc': '2.0', 'id': sequence, 'method': method, 'params': params,
            }))
            async with asyncio.timeout(5):
                while (reply := json.loads(await websocket.recv())).get('id') != sequence:
                    pass
            return reply

        hello = await call('connect', params)
        yield call, require_result(hello, 'connect')


def require_result(response, step):
    assert 'result' in response, (step, response.get('error', {}).get('code'))
    return response['result']


async def check_history(operator, session):
    request = {**session, 'limit': 32}
    data = "i=0; while [ $i -lt 160 ]; do printf '%s%03d\\n' 'qa-principal-' \"$i\"; i=$((i+1)); done\n"
    require_result(await operator('pty.input', {**session, 'data': data}), 'seed history')
    async with asyncio.timeout(8):
        while require_result(await operator('pty.attach', session), 'owner attach')['scrollback_len'] < 64:
            await asyncio.sleep(0.025)

    page = require_result(await operator('pty.history', request), 'owner history')
    rows = page['rows']
    assert len(rows) == 32, 'owner history must include one full page'
    assert all(isinstance(row['index'], str) and row['index'].isascii() and row['index'].isdecimal()
               for row in rows), 'history indices must be decimal strings'
    start = int(rows[0]['index'])
    assert [int(row['index']) for row in rows] == list(range(start, start + 32))
    assert any('qa-principal-' in ''.join(run['text'] for run in row['runs']) for row in rows)
    encoded = json.dumps(page, ensure_ascii=False, separators=(',', ':')).encode()
    assert len(encoded) <= 256 * 1024, 'history result exceeds its serialized byte bound'
    assert page['older_before'] == rows[0]['index'], 'backward cursor must anchor the oldest returned row'
    older = require_result(await operator('pty.history', {**request, 'before': page['older_before']}),
                           'owner older history')
    older_indices = list(range(start - 32, start))
    assert [int(row['index']) for row in older['rows']] == older_indices

    for fields in ({'limit': 0}, {'limit': 65}, {'before': 'not-a-cursor'}):
        rejected = await operator('pty.history', {**request, **fields})
        assert rejected.get('error', {}).get('code') == -32602, 'invalid history request must be rejected'
    future = await operator('pty.history', {**request, 'before': str(2**64 - 1)})
    assert 'error' in future and 'result' not in future, 'future cursor must not silently return an empty page'

    async def switch_grid(alternate):
        escape_suffix = 'h' if alternate else 'l'
        data = f"printf '\\033[?1049{escape_suffix}'\n"
        require_result(await operator('pty.input', {**session, 'data': data}), 'switch screen')
        async with asyncio.timeout(8):
            while True:
                live = require_result(await operator('pty.attach', session), 'observe screen mode')
                if live['patch']['alt_screen'] is alternate:
                    return
                await asyncio.sleep(0.025)

    await switch_grid(True)
    rejected = await operator('pty.history', request)
    assert 'error' in rejected and 'result' not in rejected, 'alternate grid must not expose history'
    await switch_grid(False)
    restored = require_result(await operator('pty.history', {**request, 'before': page['older_before']}),
                              'owner history after alternate screen')
    assert [int(row['index']) for row in restored['rows']] == older_indices, 'primary cursor changed'


async def check_device(operator, port, role, session):
    principal = require_result(await operator('users.create', {
        'display_name': f'QA Terminal {role}', 'role': role,
    }), 'create fixture principal')
    ticket = require_result(await operator('gateway.ticket.create', {
        'user_id': principal['user']['user_id'],
    }), 'mint fixture ticket')
    async with connected(f'ws://192.0.2.23:{port}/ws', {
        'client_type': 'panel', 'bootstrap_ticket': ticket['ticket'],
        'device_id': f'qa-terminal-{role}', 'device_name': f'QA Terminal {role}',
    }) as (device, hello):
        assert hello.get('device_token'), 'the device must consume a real pairing ticket'
        visible = await device('pty.list', {})
        if role == 'admin':
            assert require_result(visible, 'second admin list')['sessions'] == []
        else:
            assert 'error' in visible, 'member received operator-only pty.list'
        attach = await device('pty.attach', session)
        assert 'error' in attach, 'other principal could attach to owner session'
        absent_id = '00000000-0000-4000-8000-000000000001'
        owned = (await device('pty.history', {**session, 'limit': 32})).get('error')
        absent = (await device('pty.history', {'session_id': absent_id, 'limit': 32})).get('error')
        assert owned and absent and owned['code'] == attach['error']['code'], \
            'history and attach must use the same admission boundary'
        # The existing not-found error may echo the requested ID, not its owner.
        equivalent = {**owned, 'message': owned['message'].replace(session['session_id'], absent_id)}
        assert equivalent == absent, 'denial must not reveal whether another principal owns an ID'
        print(f'Paired {role}: terminal history denied ({owned["code"]})', flush=True)


async def probe(server):
    async with (
        disposable_gateway(server, paired_device=True) as (port, _, workspace, _),
        connected(f'ws://127.0.0.1:{port}/ws', {'client_type': 'cli'}) as (operator, _),
    ):
        spawned = require_result(await operator('pty.spawn', {
            'command': '/bin/sh', 'cwd': str(workspace),
        }), 'spawn fixture shell')
        session = {'session_id': spawned['session_id']}
        try:
            await check_history(operator, session)
            for role in ('admin', 'member'):
                await check_device(operator, port, role, session)
            print('Owner paged retained rows and restored a primary cursor; invalid reads and foreign callers were denied.',
                  flush=True)
        finally:
            require_result(await operator('pty.close', session), 'close fixture shell')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=file_path, required=True)
    args = parser.parse_args()
    if not os.access(args.server, os.X_OK):
        parser.error(f'not executable: {args.server}')
    asyncio.run(probe(args.server))
