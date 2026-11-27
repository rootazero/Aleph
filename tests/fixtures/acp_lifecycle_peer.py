import json
import os
from pathlib import Path
import select
import sys
import threading
import time

cwd = Path.cwd()
pid = os.getpid()
held_prompt = None

def respond(request, result):
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)

def record(line):
    with (cwd / 'frames').open('a') as frames:
        frames.write(f'{pid} {line}\n')

for line in sys.stdin:
    frame = json.loads(line)
    method = frame['method']
    record(method)
    if method == 'initialize':
        while (cwd / 'hold-initialize').exists():
            time.sleep(0.01)
        result = {'protocolVersion': 1, 'agentCapabilities': {'loadSession': True}}
    elif method == 'session/new':
        predecessor = cwd / 'predecessor'
        if predecessor.exists():
            try:
                os.kill(int(predecessor.read_text()), 0)
                record('predecessor-still-running')
            except ProcessLookupError:
                pass
        result = {'sessionId': f'peer-{pid}'}
    elif method == 'session/load':
        record('load:' + frame['params']['sessionId'])
        if (cwd / 'reject-load').exists():
            print(json.dumps({'jsonrpc': '2.0', 'id': frame['id'],
                'error': {'code': -32000, 'message': 'test peer refused saved session'}}), flush=True)
            continue
        result = {}
    elif method == 'session/prompt':
        text = frame['params']['prompt'][0]['text']
        if text == 'stop reading':
            threading.Event().wait()
        if text == 'pause after response':
            respond(frame, {'content': text})
            select.select([sys.stdin], [], [])
            record('stdin-readable')
            threading.Event().wait()
        if text == 'wait for cancel':
            held_prompt = frame
            continue
        result = {'content': text}
    elif method == 'session/cancel':
        record('cancel:' + frame['params']['sessionId'])
        if held_prompt is not None:
            respond(held_prompt, {'content': 'canceled'})
            held_prompt = None
        continue
    else:
        result = {}
    respond(frame, result)
