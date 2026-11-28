"""Offline sanity for the fixture itself, never a substitute for a Gateway run."""
from collections import Counter
from http.client import HTTPConnection
import json
from pathlib import Path
import tempfile
import threading
import time
import tomllib
import unittest
from unittest.mock import patch
import wave

from run import environment, write_config
from fixture import MAX_BODY, Provider, RATE, VOICED_SECONDS, write_clip


class TestVoiceFixture(unittest.TestCase):
    def test_write_clip_is_repeatable_mono_with_expected_sample_count(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for profile, seconds in VOICED_SECONDS.items():
                first, second = root / 'first.wav', root / 'second.wav'
                write_clip(first, profile)
                write_clip(second, profile)
                self.assertEqual(first.read_bytes(), second.read_bytes())
                with wave.open(str(first)) as clip:
                    self.assertEqual((clip.getnchannels(), clip.getsampwidth(), clip.getframerate()), (1, 2, RATE))
                    self.assertEqual(clip.getnframes(), round(RATE * (seconds + 2.5)))
                    self.assertEqual(clip.readframes(RATE // 2), bytes(RATE))
                    self.assertNotEqual(clip.readframes(500), bytes(1000))

    def test_environment_and_write_config_never_inherit_credentials_or_proxies(self):
        canaries = {'ANTHROPIC_API_KEY': 'private', 'ALEPH_HOME': 'real', 'HTTP_PROXY': 'proxy', 'NODE_OPTIONS': 'options'}
        with tempfile.TemporaryDirectory() as tmp, Provider(b'media') as provider, patch.dict('os.environ', canaries):
            root = Path(tmp)
            env = environment(root)
            self.assertEqual(set(env).intersection(canaries), {'ALEPH_HOME'})
            aleph = Path(env['ALEPH_HOME'])
            self.assertEqual(aleph, root / 'aleph')
            for enabled in (False, True):
                write_config(aleph, 24680, root / 'Panel build', provider, enabled)
                data = tomllib.loads((aleph / 'config.toml').read_text(encoding='utf-8'))
                self.assertTrue(data['gateway']['tls']['enabled'])
                self.assertEqual(data['gateway']['host'], '127.0.0.1')
                self.assertFalse(data['voice']['streaming']['enabled'])
                self.assertEqual('transcription_providers' in data['generation'], enabled)
                self.assertEqual(data['generation']['speech_providers']['qa-tts']['api_key'], provider.token)
                self.assertEqual(data['providers']['phase9-qa']['base_url'], f'http://127.0.0.1:{provider.server_port}')

    def test_provider_credentials_routes_stream_and_value_only_counters(self):
        with Provider(b'fake mp3 bytes') as provider:
            worker = threading.Thread(target=provider.serve_forever, daemon=True)
            worker.start()
            bearer = {'Authorization': f'Bearer {provider.token}'}

            def post(route, payload, headers):
                client = HTTPConnection('127.0.0.1', provider.server_port, timeout=2)
                try:
                    client.request('POST', route, payload, headers)
                    reply = client.getresponse()
                    return reply.status, reply.getheader('content-type'), reply.read()
                finally:
                    client.close()

            try:
                self.assertEqual(post('/v1/audio/speech', b'{}', {})[0], 401)
                self.assertEqual(post('/v1/audio/speech', b'', bearer | {'Content-Length': str(MAX_BODY + 1)})[0], 413)
                self.assertEqual(post('/v1/audio/transcriptions', b'--part\r\nRIFF1234', bearer | {
                    'Content-Type': 'multipart/form-data; boundary=part',
                })[0], 200)
                self.assertEqual(post('/v1/audio/speech', b'{"input":"private fixture"}', bearer),
                                 (200, 'audio/mpeg', b'fake mp3 bytes'))
                status, mime, sse = post('/v1/messages', b'{"stream":true,"tools":[{"name":"artifact_publish"}]}',
                                         {'x-api-key': provider.token})
                self.assertEqual((status, mime), (200, 'text/event-stream'))
                self.assertIn(b'event: message_stop', sse)
                self.assertNotIn(b'tool_use', sse)
            finally:
                provider.shutdown()
                worker.join(timeout=2)
            self.assertEqual(Counter(row['kind'] for row in provider.attempts), {'stt': 1, 'tts': 2, 'chat': 1})
            self.assertEqual(provider.attempts[0]['request_body_bytes'], 0)
            self.assertEqual(provider.attempts[0]['response_body_bytes_written'], 0)
            self.assertNotIn('private fixture', json.dumps(provider.attempts))
            self.assertNotIn(provider.token, json.dumps(provider.attempts))

    def test_queue_marker_comes_from_latest_turn_not_retained_history(self):
        with tempfile.TemporaryDirectory() as tmp, Provider(b'synthetic', queue_control=Path(tmp)) as provider:
            body = {'messages': [
                {'role': 'user', 'content': 'VOICE_QA_SIBLING'},
                {'role': 'assistant', 'content': 'VOICE_QA_RUNNING'},
                {'role': 'user', 'content': 'VOICE_QA_QUEUED_SUCCESSOR'},
            ]}
            self.assertTrue(provider.admit_queue_request(body))
            self.assertEqual(provider.queue_markers, ['QUEUED_SUCCESSOR'])
            self.assertEqual(json.loads((Path(tmp) / 'status.json').read_text()),
                             {'markers': ['QUEUED_SUCCESSOR']})
            (Path(tmp) / 'release-RUNNING').touch()
            body['messages'][-1]['content'] = 'VOICE_QA_SIBLING then VOICE_QA_RUNNING'
            self.assertTrue(provider.admit_queue_request(body))
            self.assertEqual(provider.queue_markers, ['QUEUED_SUCCESSOR', 'RUNNING'])

    def test_delayed_provider_marks_actual_inflight_request_before_reply(self):
        with tempfile.TemporaryDirectory() as tmp, Provider(b'synthetic', 'tts', Path(tmp)) as provider:
            worker = threading.Thread(target=provider.serve_forever, daemon=True)
            worker.start()
            client = HTTPConnection('127.0.0.1', provider.server_port, timeout=2)
            try:
                client.request('POST', '/v1/audio/speech', b'{"input":"fixture"}',
                               {'Authorization': f'Bearer {provider.token}'})
                mark = Path(tmp) / 'in-flight-tts'
                deadline = time.monotonic() + 2
                while not mark.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(mark.exists())
                with provider.lock:
                    self.assertEqual(len(provider.attempts), 1)
                    self.assertGreater(provider.attempts[0]['request_body_bytes'], 0)
                    self.assertEqual(provider.attempts[0]['response_body_bytes_written'], 0)
            finally:
                provider.done.set()
                client.close()
                provider.shutdown()
                worker.join(timeout=2)


if __name__ == '__main__':
    unittest.main()
