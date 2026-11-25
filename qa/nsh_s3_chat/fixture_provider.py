"""Loopback-only Anthropic fixture for real tool and Markdown transcript QA."""

import itertools
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import threading
import time

FILE = 's3-transcript-fixture.txt'
WEB_FILE = 's3-web-transcript-fixture.txt'
WEB_BEFORE = 'S3 Web before tool'
WEB_AFTER = 'S3 Web after tool'
REPLY = 'S3 scripted response'
RICH_REPLY = f'''{REPLY}

> [!NOTE]
> S3 transcript callout

https://example.com/s3-transcript

```mermaid
flowchart LR
A[S3 Start] --> B[S3 Finish]
```
'''


def install(aleph, desktop_ready, desktop_release, web_release, log):
    serial = itertools.count(1)

    class Handler(BaseHTTPRequestHandler):
        protocol_version = 'HTTP/1.1'

        def log_message(self, *_):
            pass

        def do_POST(self):
            if self.path.split('?', 1)[0] != '/v1/messages':
                self.send_error(404)
                return
            body = json.loads(self.rfile.read(int(self.headers['content-length'])))
            request = next(serial)
            messages = body.get('messages', [])
            tools = {tool['name'] for tool in body.get('tools', [])}
            history = json.dumps(messages)
            # Match only fixture prompts, not injected system context or tool results.
            fixture_turn = next((marker for message in reversed(messages)
                                 if message.get('role') == 'user'
                                 for marker in ('Stop probe', '你好', 'desktophello')
                                 if marker in json.dumps(message.get('content'), ensure_ascii=False)), None)
            step = None
            release = None
            text = REPLY
            if fixture_turn == '你好' and 'file_write' in tools:
                if '"qa_s3_web_write"' not in history:
                    text = WEB_BEFORE
                    step = ('qa_s3_web_write', 'file_write', {'file_path': WEB_FILE, 'content': 's3 web line\n'})
                else:
                    text = WEB_AFTER
                    release = web_release
            elif fixture_turn == 'desktophello' and '"qa_s3_edit"' not in history and {'file_write', 'file_edit'} <= tools:
                if '"qa_s3_write"' not in history:
                    text = 'S3 before writing'
                    step = ('qa_s3_write', 'file_write', {'file_path': FILE, 'content': 's3 original line\n'})
                else:
                    text = 'S3 before editing'
                    step = ('qa_s3_edit', 'file_edit', {
                        'file_path': FILE, 'old_string': 's3 original line', 'new_string': 's3 edited line',
                    })
            elif fixture_turn == 'desktophello' and '"qa_s3_edit"' in history:
                text = RICH_REPLY
                release = desktop_release
            with log.open('a', encoding='utf-8') as stream:
                print(f'request {request}: turn={fixture_turn!r}, tool={step[1] if step else None}, '
                      f'wait={release.name if release else None}', file=stream)
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Transfer-Encoding', 'chunked')
            self.end_headers()

            def event(kind, **fields):
                payload = json.dumps({'type': kind, **fields})
                data = f'event: {kind}\ndata: {payload}\n\n'.encode()
                self.wfile.write(f'{len(data):X}\r\n'.encode() + data + b'\r\n')
                self.wfile.flush()

            event('message_start', message={
                'id': f'msg_s3_{request}', 'type': 'message', 'role': 'assistant',
                'model': body.get('model', 'qa-s3-model'), 'content': [],
                'stop_reason': None, 'stop_sequence': None, 'usage': {'input_tokens': 100, 'output_tokens': 1},
            })
            event('content_block_start', index=0, content_block={'type': 'text', 'text': ''})
            event('content_block_delta', index=0, delta={'type': 'text_delta', 'text': text})
            event('content_block_stop', index=0)
            if step:
                call_id, name, arguments = step
                time.sleep(0.3)
                event('content_block_start', index=1, content_block={
                    'type': 'tool_use', 'id': call_id, 'name': name, 'input': {},
                })
                event('content_block_delta', index=1, delta={
                    'type': 'input_json_delta', 'partial_json': json.dumps(arguments),
                })
                event('content_block_stop', index=1)
            if release:
                if release == desktop_release:
                    desktop_ready.touch()
                # Keep the provider response open until the host captures the live rows.
                deadline = time.monotonic() + 25
                while not release.exists() and time.monotonic() < deadline:
                    time.sleep(0.05)
            event('message_delta', delta={
                'stop_reason': 'tool_use' if step else 'end_turn', 'stop_sequence': None,
            }, usage={'output_tokens': 12})
            event('message_stop')
            self.wfile.write(b'0\r\n\r\n')

    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    config = aleph / 'config.toml'
    original = config.read_text(encoding='utf-8')
    config.write_text(original.replace('name = "Main Agent"', '''name = "Main Agent"
provider = "qa-s3"
model = "qa-s3-model"''', 1) + f'''
[agents.defaults]
workspace_root = {json.dumps(str(aleph.parent / 'workspaces'))}
agents_root = {json.dumps(str(aleph.parent / 'agents'))}

[providers.qa-s3]
enabled = true
protocol = "anthropic"
base_url = "http://127.0.0.1:{server.server_port}"
api_key = "qa-dummy-not-a-real-key"
models = ["qa-s3-model"]
''', encoding='utf-8')
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server
