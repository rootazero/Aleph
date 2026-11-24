#!/usr/bin/env python3
"""Unit-only diagnostics tests; no Gateway, HTTP fixture or paid provider."""
import asyncio
import contextlib
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import AsyncMock, Mock, patch

SPEC = importlib.util.spec_from_file_location('capability_i3_probe', Path(__file__).with_name('probe.py'))
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)
SECRET = 'fixture-ticket-SECRET-123456'
DEVICE = 'fixture-device-SECRET-987654'
EXPECTED_TESTS = 22


class FakeSocket:
    def __init__(self, result):
        self.result = result
        self.queue = asyncio.Queue()

    def __aiter__(self):
        return self

    async def __anext__(self):
        value = await self.queue.get()
        if value is None:
            raise StopAsyncIteration
        return value

    async def send(self, raw):
        request = json.loads(raw)
        await self.queue.put(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': self.result}))

    async def close(self):
        await self.queue.put(None)


class EvidenceTests(unittest.TestCase):
    def make_case(self):
        case = probe.Probe(None)
        root = Path(tempfile.mkdtemp(prefix='unit-evidence-', dir=os.environ['TMPDIR']))
        case.prepare = lambda: setattr(case, 'root', root)
        case.boot = AsyncMock()
        case.stop = AsyncMock()
        case.scenarios = AsyncMock()
        return case, root

    def run_case(self, case, root):
        with contextlib.redirect_stdout(io.StringIO()):
            code = asyncio.run(case.run())
        return code, json.loads((root / 'receipt.json').read_text())

    def error_rpc(self, message):
        rpc = object.__new__(probe.Rpc)
        rpc.secrets = {SECRET, DEVICE, 'qa-dummy-not-a-real-key'}
        rpc.call = AsyncMock(return_value={'error': {'code': -32099, 'message': message,
                                                   'data': {'private_model_body': 'MUST_NOT_PERSIST'}}})
        return rpc

    def start_child(self, script, root):
        env = {'HOME': os.environ['HOME'], 'TMPDIR': os.environ['TMPDIR'],
               'PYTHONDONTWRITEBYTECODE': '1', 'PATH': '/usr/bin:/bin'}
        child = subprocess.Popen([sys.executable, '-I', '-B', '-c', script], cwd=root, env=env,
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                 start_new_session=True)
        self.assertEqual(os.getpgid(child.pid), child.pid)
        return child

    def settle_child(self, child, root):
        if child.poll() is None:
            child.terminate()
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.assertEqual(os.getpgid(child.pid), child.pid)
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
        for stream in (child.stdin, child.stdout, child.stderr):
            if stream is not None and not stream.closed:
                stream.close()
        (root / 'helper-child.json').write_text(json.dumps({
            'pid': child.pid, 'pgid_at_creation': child.pid, 'returncode': child.returncode,
            'reaped': child.poll() is not None, 'kind': 'owned stdio unit helper only'}) + '\n')
        self.assertIsNotNone(child.returncode)

    def release_child(self, child):
        child.stdin.write(b'x')
        child.stdin.flush()
        child.stdin.close()

    def test_actual_rpc_error_reaches_receipt(self):
        case, root = self.make_case()
        reached = []

        async def scenario():
            rpc = object.__new__(probe.Rpc)
            rpc.secrets = {SECRET}
            rpc.call = AsyncMock(return_value={'jsonrpc': '2.0', 'id': 1, 'error': {
                'code': -32099, 'message': 'session unavailable; ticket=' + SECRET,
                'data': {'private_model_body': 'MUST_NOT_PERSIST'}}})
            reached.append('actual Rpc.result -> Probe.run -> receipt.json')
            await rpc.result('capabilities.ui.attach', {'private_request': 'MUST_NOT_PERSIST'})

        case.scenarios = scenario
        with contextlib.redirect_stdout(io.StringIO()):
            exit_code = asyncio.run(case.run())
        saved = json.loads((root / 'receipt.json').read_text())
        self.assertEqual(exit_code, 1)
        self.assertEqual(reached, ['actual Rpc.result -> Probe.run -> receipt.json'])
        self.assertEqual(saved['failure_code'], 'rpc_failed_capabilities.ui.attach_-32099')
        self.assertIn('rpc_error', saved, 'Actual protocol error message is lost at the existing receipt boundary')
        diagnostic = saved['rpc_error']
        self.assertEqual(diagnostic['method'], 'capabilities.ui.attach')
        self.assertEqual(diagnostic['code'], -32099)
        self.assertIn('session unavailable', diagnostic['message'])
        self.assertTrue(diagnostic['redacted'])
        self.assertNotIn(SECRET, json.dumps(saved))
        self.assertNotIn('MUST_NOT_PERSIST', json.dumps(saved))

    def test_primary_rpc_failure_survives_cleanup_failure(self):
        case, root = self.make_case()
        case.secrets.add(SECRET)
        rpc = self.error_rpc('session unavailable ' + SECRET)
        case.scenarios = lambda: rpc.result('capabilities.ui.attach', {})
        case.cleanup = AsyncMock(side_effect=OSError('cleanup ' + SECRET))
        code, saved = self.run_case(case, root)
        self.assertEqual(code, 1)
        self.assertEqual(saved['failure_type'], 'ProbeFailure')
        self.assertEqual(saved['failure_code'], 'rpc_failed_capabilities.ui.attach_-32099')
        self.assertEqual(saved['cleanup_errors'][0]['stage'], 'cleanup')
        self.assertEqual(saved['cleanup_errors'][0]['error_type'], 'OSError')
        self.assertNotIn(SECRET, json.dumps(saved))

    def test_success_compatibility(self):
        case, root = self.make_case()
        value = {'ok': True, 'private_success_body': 'MUST_NOT_PERSIST'}
        rpc = object.__new__(probe.Rpc)
        rpc.secrets = set()
        rpc.call = AsyncMock(return_value={'result': value})

        async def scenario():
            self.assertIs(await rpc.result('tools.invoke', {}), value)

        case.scenarios = scenario
        code, saved = self.run_case(case, root)
        self.assertEqual(code, 0)
        self.assertEqual(saved['status'], 'PASS_FOR_LISTED_SCENARIOS_ONLY')
        self.assertNotIn('rpc_error', saved)
        self.assertNotIn('MUST_NOT_PERSIST', json.dumps(saved))

    def test_cleanup_only_failure_remains_nonzero(self):
        case, root = self.make_case()
        case.secrets.add(SECRET)
        case.cleanup = AsyncMock(side_effect=RuntimeError('cleanup ' + SECRET))
        code, saved = self.run_case(case, root)
        self.assertEqual(code, 1)
        self.assertEqual(saved['status'], 'FAIL')
        self.assertEqual(saved['failure_code'], 'cleanup_failed')
        self.assertNotIn(SECRET, json.dumps(saved))

    def test_rpc_message_bounded_and_safe(self):
        rpc = self.error_rpc('x' * (probe.RPC_MESSAGE_LIMIT - 7) + SECRET + ' trailing')
        with self.assertRaises(probe.ProbeFailure) as caught:
            asyncio.run(rpc.result('capabilities.ui.attach', {}))
        diagnostic = caught.exception.diagnostic
        self.assertEqual(str(caught.exception), 'rpc_failed_capabilities.ui.attach_-32099')
        self.assertEqual(diagnostic['code'], -32099)
        self.assertTrue(diagnostic['truncated'])
        self.assertTrue(diagnostic['redacted'])
        self.assertLessEqual(len(diagnostic['message'].encode()), probe.RPC_MESSAGE_LIMIT)
        self.assertNotIn(SECRET[:7], diagnostic['message'])
        self.assertNotIn('MUST_NOT_PERSIST', json.dumps(diagnostic))

    def test_redaction_at_every_chunk_split(self):
        value = SECRET.encode()
        for split in range(len(value) + 1):
            with self.subTest(split=split):
                text = probe.BoundedText(512)
                text.feed(b'prefix:' + value[:split])
                text.feed(value[split:] + b':suffix')
                safe = text.safe({SECRET})
                self.assertEqual(safe['text'], 'prefix:[REDACTED]:suffix')
                self.assertTrue(safe['redacted'])
                self.assertFalse(safe['truncated'])

    def test_redaction_at_every_retention_boundary(self):
        for kept in range(1, len(SECRET) + 1):
            with self.subTest(kept=kept):
                text = probe.BoundedText(8 + kept)
                payload = b'prefix::' + SECRET.encode() + b'excess'
                for offset in range(0, len(payload), 3):
                    text.feed(payload[offset:offset + 3])
                safe = text.safe({SECRET})
                self.assertEqual(safe['text'], ('prefix::[REDACTED]')[:8 + kept])
                self.assertTrue(safe['redacted'])
                self.assertTrue(safe['truncated'])
                self.assertLessEqual(len(text.data), 8 + kept)

    def test_oversized_memory_and_utf8_bounds(self):
        text = probe.BoundedText(17)
        for _ in range(64):
            text.feed(b'\xff' * 4096)
        safe = text.safe(set())
        self.assertEqual(text.total, 64 * 4096)
        self.assertEqual(len(text.data), 17)
        self.assertLessEqual(len(safe['text'].encode()), 17)
        self.assertTrue(safe['truncated'])

    def test_credentials_collected_without_success_bodies(self):
        secrets = set()
        result = {'role': 'operator', 'identity': {'device_token': DEVICE},
                  'ticket': SECRET, 'private_success_body': 'MUST_NOT_PERSIST'}

        async def exchange():
            rpc = probe.Rpc(FakeSocket(result), secrets)
            try:
                self.assertEqual(await rpc.result('connect', {'bootstrap_ticket': SECRET}), result)
            finally:
                await rpc.close()

        asyncio.run(exchange())
        self.assertEqual(secrets, {SECRET, DEVICE})
        self.assertNotIn('MUST_NOT_PERSIST', secrets)

    def test_real_stdio_independent_bounded_eof_and_reaped(self):
        _, root = self.make_case()
        script = ('import os,sys;sys.stdin.buffer.read(1);'
                  'os.write(1,b"stdout:"+' + repr(SECRET.encode()[:9]) + ');'
                  'os.write(1,' + repr(SECRET.encode()[9:]) + '+b"x"*200000);'
                  'os.write(2,b"stderr:"+' + repr(DEVICE.encode()) + '+b"\\n")')
        child = self.start_child(script, root)
        capture = probe.ServerOutput(child)
        try:
            self.release_child(child)
            self.assertEqual(child.wait(timeout=10), 0)
            saved = capture.finish({SECRET, DEVICE})
            self.assertTrue(saved['joined'])
            self.assertIsNone(saved['join_error'])
            self.assertEqual(saved['pid'], child.pid)
            self.assertEqual(saved['pgid_at_capture'], child.pid)
            stdout, stderr = saved['streams']['stdout'], saved['streams']['stderr']
            self.assertEqual(stdout['source_bytes'], 7 + len(SECRET) + 200000)
            self.assertEqual(stderr['source_bytes'], 7 + len(DEVICE) + 1)
            self.assertTrue(stdout['truncated'])
            self.assertFalse(stderr['truncated'])
            self.assertTrue(stdout['eof'] and stderr['eof'])
            self.assertTrue(stdout['redacted'] and stderr['redacted'])
            self.assertNotIn('stderr:', stdout['text'])
            self.assertNotIn('stdout:', stderr['text'])
            self.assertNotIn(SECRET, json.dumps(saved))
            self.assertNotIn(DEVICE, json.dumps(saved))
            self.assertLessEqual(len(capture.buffers['stdout'].data), probe.SERVER_STREAM_LIMIT)
            (root / 'stdio-safe-evidence.json').write_text(json.dumps(saved) + '\n')
        finally:
            capture.stopping.set()
            capture.thread.join(timeout=2)
            self.settle_child(child, root)
        self.assertFalse(capture.thread.is_alive())

    def test_read_failure_recorded_without_hanging(self):
        _, root = self.make_case()
        child = self.start_child('import os,sys;sys.stdin.buffer.read(1);os.write(1,b"out");os.write(2,b"err")', root)
        stdout_fd = child.stdout.fileno()
        real_read = os.read

        def read(fd, count):
            if fd == stdout_fd:
                raise OSError('unit read failure')
            return real_read(fd, count)

        capture = None
        try:
            with patch.object(probe.os, 'read', side_effect=read):
                capture = probe.ServerOutput(child)
                self.release_child(child)
                child.wait(timeout=10)
                saved = capture.finish(set())
            self.assertTrue(saved['joined'])
            self.assertEqual(saved['streams']['stdout']['read_error'], 'OSError')
            self.assertFalse(saved['streams']['stdout']['eof'])
            self.assertTrue(saved['streams']['stderr']['eof'])
            (root / 'read-error-evidence.json').write_text(json.dumps(saved) + '\n')
        finally:
            if capture is not None:
                capture.stopping.set()
                capture.thread.join(timeout=2)
            self.settle_child(child, root)

    def test_reader_shutdown_without_eof_is_bounded(self):
        _, root = self.make_case()
        child = self.start_child('import sys;sys.stdin.buffer.read(1)', root)
        capture = probe.ServerOutput(child)
        try:
            start = time.monotonic()
            saved = capture.finish(set())
            self.assertLess(time.monotonic() - start, 4)
            self.assertTrue(saved['joined'])
            self.assertTrue(saved['stop_requested'])
            self.assertFalse(any(stream['eof'] for stream in saved['streams'].values()))
            (root / 'incomplete-eof-evidence.json').write_text(json.dumps(saved) + '\n')
        finally:
            capture.stopping.set()
            capture.thread.join(timeout=2)
            self.settle_child(child, root)

    def test_capture_finish_failure_preserves_primary(self):
        case, root = self.make_case()
        case.secrets.add(SECRET)
        case.scenarios = AsyncMock(side_effect=probe.ProbeFailure('primary_failure'))
        case.server_output = Mock()
        case.server_output.finish.side_effect = RuntimeError('join ' + SECRET)
        code, saved = self.run_case(case, root)
        self.assertEqual(code, 1)
        self.assertEqual(saved['failure_code'], 'primary_failure')
        self.assertEqual(saved['cleanup_errors'][0]['stage'], 'server_output')
        self.assertNotIn(SECRET, json.dumps(saved))

    def test_client_failure_still_settles_owned_child(self):
        case, root = self.make_case()
        case.secrets.add(SECRET)
        child = self.start_child('import sys;sys.stdin.buffer.read(1)', root)
        case.provider = child
        case.clients = [Mock(close=AsyncMock(side_effect=OSError('close ' + SECRET)))]
        try:
            code, saved = self.run_case(case, root)
            self.assertEqual(code, 1)
            self.assertEqual(saved['cleanup_errors'][0]['stage'], 'client_close')
            self.assertTrue(saved['owned_process_cleanup'][0]['wait_completed'])
            self.assertEqual(saved['owned_process_cleanup'][0]['pid'], child.pid)
            self.assertIsNotNone(child.poll())
            self.assertNotIn(SECRET, json.dumps(saved))
        finally:
            self.settle_child(child, root)

    def test_non_rpc_primary_type_survives_cleanup_error(self):
        case, root = self.make_case()
        case.scenarios = AsyncMock(side_effect=ValueError('private body not copied'))
        case.cleanup = AsyncMock(side_effect=OSError('cleanup failed'))
        code, saved = self.run_case(case, root)
        self.assertEqual(code, 1)
        self.assertEqual(saved['failure_type'], 'ValueError')
        self.assertNotIn('failure_code', saved)
        self.assertNotIn('rpc_error', saved)
        self.assertNotIn('private body not copied', json.dumps(saved))

    def test_non_string_rpc_message_is_not_serialized(self):
        rpc = self.error_rpc({'private_model_body': 'MUST_NOT_PERSIST'})
        with self.assertRaises(probe.ProbeFailure) as caught:
            asyncio.run(rpc.result('capabilities.ui.attach', {}))
        self.assertEqual(caught.exception.diagnostic['message'], '')
        self.assertNotIn('MUST_NOT_PERSIST', json.dumps(caught.exception.diagnostic))

    def run_at_receipt_boundary(self, case, root):
        captured = []
        original_write = Path.write_text
        known = {SECRET, DEVICE, 'qa-dummy-not-a-real-key', SECRET[:16], DEVICE[:16]}

        def write(path, data, *args, **kwargs):
            if path == root / 'receipt.json':
                captured.append(data)
                # A failing privacy RED observes the actual serializer without writing its raw leak.
                for value in sorted(known, key=len, reverse=True):
                    data = data.replace(value, '[REDACTED-BY-TEST-SINK]')
            return original_write(path, data, *args, **kwargs)

        with patch.object(Path, 'write_text', write), contextlib.redirect_stdout(io.StringIO()):
            code = asyncio.run(case.run())
        self.assertEqual(len(captured), 1)
        raw = captured[0]
        observed = json.loads(raw)
        flags = {
            'rpc_full_credential_present': any(value in observed.get('rpc_error', {}).get('message', '') for value in (SECRET, DEVICE)),
            'cleanup_full_credential_present': any(value in entry.get('text', '') for entry in observed.get('cleanup_errors', []) for value in (SECRET, DEVICE)),
            'rpc_cut_prefix_present': SECRET[:16] in observed.get('rpc_error', {}).get('message', ''),
            'cleanup_cut_prefix_present': any(DEVICE[:16] in entry.get('text', '') for entry in observed.get('cleanup_errors', [])),
            'stdout_cut_prefix_present': SECRET[:16] in observed.get('server_output', {}).get('streams', {}).get('stdout', {}).get('text', ''),
        }
        (root / 'receipt-boundary-observation.json').write_text(json.dumps({
            'actual_write_boundary_reached': True, 'unsafe_raw_payload_persisted': False,
            'serialized_payload_sha256': hashlib.sha256(raw.encode()).hexdigest(),
            'serialized_bytes': len(raw.encode()), 'privacy_flags': flags,
            'saved_file_identical_to_actual_serializer': (root / 'receipt.json').read_text() == raw,
            'sink_limit': 'Only the regression sink replaces known fixture values before disk; assertions use original in-memory serialization. GREEN must require no sink replacements.'}) + '\n')
        return code, observed, raw

    def late_discovery_case(self, cut=False):
        case, root = self.make_case()
        rpc = object.__new__(probe.Rpc)
        rpc.secrets = case.secrets
        dummy = 'qa-dummy-not-a-real-key'
        if cut:
            prefix = 'dummy=' + dummy + ';'
            rpc_message = prefix + 'x' * (probe.RPC_MESSAGE_LIMIT - 16 - len(prefix)) + SECRET + ';end'
            cleanup_message = 'x' * (probe.RPC_MESSAGE_LIMIT - 16) + DEVICE + ';end'
        else:
            rpc_message = 'dummy=' + dummy + ';late=' + SECRET
            cleanup_message = 'late-device=' + DEVICE
        rpc.call = AsyncMock(return_value={'error': {'code': -32099, 'message': rpc_message}})
        case.scenarios = lambda: rpc.result('capabilities.ui.attach', {})

        async def cleanup():
            case.cleanup_error('before_discovery', OSError(cleanup_message))
            late = probe.Rpc(FakeSocket({'ticket': SECRET, 'identity': {'device_token': DEVICE}}), case.secrets)
            try:
                await late.result('gateway.ticket.create', {})
            finally:
                await late.close()

        case.cleanup = cleanup
        code, saved, raw = self.run_at_receipt_boundary(case, root)
        self.assertTrue({SECRET, DEVICE}.issubset(case.secrets))
        self.assertEqual(code, 1)
        self.assertEqual(saved['status'], 'FAIL')
        self.assertEqual(saved['failure_type'], 'ProbeFailure')
        self.assertEqual(saved['failure_code'], 'rpc_failed_capabilities.ui.attach_-32099')
        self.assertEqual(saved['cleanup_errors'][0]['stage'], 'before_discovery')
        self.assertEqual(saved['rpc_error']['source_bytes'], len(rpc_message.encode()))
        self.assertEqual(saved['cleanup_errors'][0]['source_bytes'], len(cleanup_message.encode()))
        self.assertEqual(saved['rpc_error']['retained_bytes'], min(len(rpc_message.encode()), probe.RPC_MESSAGE_LIMIT))
        self.assertEqual(saved['cleanup_errors'][0]['retained_bytes'], min(len(cleanup_message.encode()), probe.RPC_MESSAGE_LIMIT))
        self.assertLessEqual(len(saved['rpc_error']['message'].encode()), probe.RPC_MESSAGE_LIMIT)
        self.assertLessEqual(len(saved['cleanup_errors'][0]['text'].encode()), probe.RPC_MESSAGE_LIMIT)
        return root, saved, raw

    def test_late_credentials_rescrub_rpc_and_cleanup_at_receipt(self):
        root, saved, raw = self.late_discovery_case()
        leaks = [SECRET in saved['rpc_error']['message'], DEVICE in saved['cleanup_errors'][0]['text']]
        self.assertFalse(any(leaks), 'Late known credential remained in the actual selected receipt serialization')
        self.assertTrue(saved['rpc_error']['redacted'])
        self.assertTrue(saved['cleanup_errors'][0]['redacted'])
        self.assertIn('dummy=[REDACTED]', saved['rpc_error']['message'])
        self.assertTrue((root / 'receipt.json').read_text() == raw, 'GREEN still needed test-sink redaction')

    def test_late_credentials_prior_cut_and_flags_preserved(self):
        root, saved, raw = self.late_discovery_case(cut=True)
        leaks = [SECRET[:16] in saved['rpc_error']['message'], DEVICE[:16] in saved['cleanup_errors'][0]['text']]
        self.assertFalse(any(leaks), 'A prior message cut retained a late-known credential prefix')
        self.assertTrue(saved['rpc_error']['redacted'] and saved['rpc_error']['truncated'])
        self.assertTrue(saved['cleanup_errors'][0]['redacted'] and saved['cleanup_errors'][0]['truncated'])
        self.assertIn('dummy=[REDACTED]', saved['rpc_error']['message'])
        self.assertTrue((root / 'receipt.json').read_text() == raw, 'GREEN still needed test-sink redaction')

    def partial_capture_case(self, read_error=False):
        case, root = self.make_case()
        case.secrets.add(SECRET)
        script = ('import os,sys;sys.stdin.buffer.read(1);os.write(1,' + repr(b'ticket=' + SECRET.encode()[:16]) + ');'
                  'sys.stdin.buffer.read(1);os.write(1,b"!");sys.stdin.buffer.read(1)')
        child = self.start_child(script, root)
        stdout_fd = child.stdout.fileno()
        ready, failed = threading.Event(), threading.Event()
        original_read = os.read
        capture = None

        def read(fd, size):
            if fd == stdout_fd and ready.is_set():
                failed.set()
                raise OSError('injected read failure after nonempty prefix')
            return original_read(fd, size)

        try:
            manager = patch.object(probe.os, 'read', side_effect=read) if read_error else contextlib.nullcontext()
            with manager:
                capture = probe.ServerOutput(child)
                original_feed = capture.buffers['stdout'].feed

                def feed(chunk):
                    original_feed(chunk)
                    if capture.buffers['stdout'].total >= 23:
                        ready.set()

                capture.buffers['stdout'].feed = feed
                child.stdin.write(b'x')
                child.stdin.flush()
                self.assertTrue(ready.wait(timeout=5), 'Owned reader did not receive the intended nonempty prefix')
                if read_error:
                    child.stdin.write(b'y')
                    child.stdin.flush()
                    self.assertTrue(failed.wait(timeout=5), 'Injected post-prefix read failure was not reached')
                case.server_output = capture
                case.cleanup = AsyncMock()
                case.scenarios = AsyncMock(side_effect=probe.ProbeFailure('partial_stream_primary'))
                code, saved, raw = self.run_at_receipt_boundary(case, root)
            self.assertEqual(code, 1)
            self.assertEqual(saved['failure_code'], 'partial_stream_primary')
            self.assertEqual(saved['cleanup_errors'][0]['stage'], 'server_output')
            output = saved['server_output']
            self.assertTrue(output['joined'])
            self.assertTrue(output['stop_requested'])
            stream = output['streams']['stdout']
            self.assertEqual(stream['source_bytes'], 23)
            self.assertEqual(stream['retained_bytes'], 23)
            self.assertEqual(stream['limit_bytes'], probe.SERVER_STREAM_LIMIT)
            self.assertFalse(stream['truncated'])
            self.assertFalse(stream['eof'])
            self.assertEqual(stream['read_error'], 'OSError' if read_error else None)
            return root, saved, raw
        finally:
            if capture is not None:
                capture.stopping.set()
                capture.thread.join(timeout=2)
                self.assertFalse(capture.thread.is_alive())
            self.settle_child(child, root)

    def test_nonempty_incomplete_prefix_scrubbed_at_receipt(self):
        root, saved, raw = self.partial_capture_case()
        self.assertFalse(SECRET[:16] in saved['server_output']['streams']['stdout']['text'],
                         'Known under-limit prefix reached the incomplete-stream receipt serialization')
        self.assertTrue(saved['server_output']['streams']['stdout']['redacted'])
        self.assertTrue((root / 'receipt.json').read_text() == raw, 'GREEN still needed test-sink redaction')

    def test_nonempty_read_error_prefix_scrubbed_at_receipt(self):
        root, saved, raw = self.partial_capture_case(read_error=True)
        self.assertFalse(SECRET[:16] in saved['server_output']['streams']['stdout']['text'],
                         'Known under-limit prefix reached the read-error receipt serialization')
        self.assertTrue(saved['server_output']['streams']['stdout']['redacted'])
        self.assertTrue((root / 'receipt.json').read_text() == raw, 'GREEN still needed test-sink redaction')

    def complete_capture_case(self, incomplete_stderr=False):
        case, root = self.make_case()
        case.secrets.update({SECRET, DEVICE})
        stdout_payload = b'prefix:' + SECRET.encode() + b':suffix'
        stderr_payload = b'device=' + DEVICE.encode()[:16] if incomplete_stderr else b'healthy-stderr'
        script = ('import os,sys;sys.stdin.buffer.read(1);os.write(1,' + repr(stdout_payload) + ');'
                  'os.close(1);os.write(2,' + repr(stderr_payload) + ')'
                  + (';sys.stdin.buffer.read(1)' if incomplete_stderr else ''))
        child = self.start_child(script, root)
        stdout_fd, stderr_fd = child.stdout.fileno(), child.stderr.fileno()
        stdout_eof, stderr_ready = threading.Event(), threading.Event()
        original_read = os.read
        capture = None

        def read(fd, size):
            chunk = original_read(fd, size)
            if fd == stdout_fd and not chunk:
                stdout_eof.set()
            if fd == stderr_fd and chunk:
                stderr_ready.set()
            return chunk

        try:
            with patch.object(probe.os, 'read', side_effect=read):
                capture = probe.ServerOutput(child)
                if incomplete_stderr:
                    child.stdin.write(b'x')
                    child.stdin.flush()
                else:
                    self.release_child(child)
                    self.assertEqual(child.wait(timeout=10), 0)
                self.assertTrue(stdout_eof.wait(timeout=5), 'Native stdout EOF was not observed')
                self.assertTrue(stderr_ready.wait(timeout=5), 'Native stderr payload was not observed')
                case.server_output = capture
                case.cleanup = AsyncMock()
                if incomplete_stderr:
                    case.scenarios = AsyncMock(side_effect=probe.ProbeFailure('mixed_stream_primary'))
                code, saved, raw = self.run_at_receipt_boundary(case, root)
            output = saved['server_output']
            self.assertTrue(output['joined'])
            self.assertIsNone(output['join_error'])
            self.assertEqual(output['stop_requested'], incomplete_stderr)
            stdout, stderr = output['streams']['stdout'], output['streams']['stderr']
            self.assertTrue(stdout['eof'])
            self.assertIsNone(stdout['read_error'])
            self.assertIsNone(stdout['close_error'])
            self.assertEqual(stdout['source_bytes'], len(stdout_payload))
            self.assertEqual(stdout['retained_bytes'], len(stdout_payload))
            self.assertEqual(stdout['limit_bytes'], probe.SERVER_STREAM_LIMIT)
            self.assertTrue(stdout['redacted'])
            self.assertFalse(stdout['truncated'])
            self.assertEqual(stderr['eof'], not incomplete_stderr)
            self.assertIsNone(stderr['read_error'])
            self.assertEqual(stderr['source_bytes'], len(stderr_payload))
            self.assertEqual(stderr['retained_bytes'], len(stderr_payload))
            self.assertFalse(stderr['truncated'])
            self.assertEqual(code, 1 if incomplete_stderr else 0)
            if incomplete_stderr:
                self.assertEqual(saved['failure_code'], 'mixed_stream_primary')
                self.assertEqual(saved['cleanup_errors'][0]['stage'], 'server_output')
                self.assertTrue(stderr['redacted'])
                self.assertFalse(DEVICE[:16] in stderr['text'], 'Incomplete stderr retained a known prefix')
            else:
                self.assertEqual(saved['status'], 'PASS_FOR_LISTED_SCENARIOS_ONLY')
                self.assertNotIn('cleanup_errors', saved)
                self.assertEqual(stderr['text'], 'healthy-stderr')
            (root / 'complete-stream-observation.json').write_text(json.dumps({
                'native_stdout_eof_observed': stdout_eof.is_set(), 'native_stderr_payload_observed': stderr_ready.is_set(),
                'stdout_receipt_eof': stdout['eof'], 'stderr_receipt_eof': stderr['eof'],
                'stop_requested': output['stop_requested'], 'stdout_complete_suffix_preserved': stdout['text'] == 'prefix:[REDACTED]:suffix',
                'stdout_source_bytes': stdout['source_bytes'], 'stderr_source_bytes': stderr['source_bytes'],
                'saved_file_identical_to_actual_serializer': (root / 'receipt.json').read_text() == raw}) + '\n')
            return root, saved, raw
        finally:
            if capture is not None:
                capture.stopping.set()
                capture.thread.join(timeout=2)
                self.assertFalse(capture.thread.is_alive())
            self.settle_child(child, root)

    def test_complete_eof_suffix_preserved_at_receipt(self):
        root, saved, raw = self.complete_capture_case()
        self.assertTrue(saved['server_output']['streams']['stdout']['text'] == 'prefix:[REDACTED]:suffix',
                        'Complete native EOF stream lost its benign suffix at the receipt boundary')
        self.assertTrue((root / 'receipt.json').read_text() == raw, 'GREEN still needed test-sink redaction')

    def test_complete_stdout_independent_of_incomplete_stderr_at_receipt(self):
        root, saved, raw = self.complete_capture_case(incomplete_stderr=True)
        self.assertTrue(saved['server_output']['streams']['stdout']['text'] == 'prefix:[REDACTED]:suffix',
                        'Incomplete stderr or stop request over-redacted healthy complete stdout')
        self.assertTrue((root / 'receipt.json').read_text() == raw, 'GREEN still needed test-sink redaction')


if __name__ == '__main__':
    selected = sys.argv[1:]
    suite = (unittest.defaultTestLoader.loadTestsFromNames(selected, sys.modules[__name__]) if selected
             else unittest.defaultTestLoader.loadTestsFromTestCase(EvidenceTests))
    count = suite.countTestCases()
    expected = len(selected) if selected else EXPECTED_TESTS
    if count == 0 or count != expected:
        raise SystemExit('TEST_COUNT_MISMATCH: ' + str((expected, count)))
    print('INTENDED_TEST_COUNT=' + str(expected), flush=True)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    print('ACTUAL_TEST_COUNT=' + str(result.testsRun), flush=True)
    raise SystemExit(0 if result.wasSuccessful() and result.testsRun == expected else 1)
