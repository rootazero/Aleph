"""Loopback-only synthetic providers and media for the NSH Voice QA runner."""

import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import secrets
import struct
import threading
import wave

from nsh_panel_provider import response as chat_response

RATE = 16_000
VOICED_SECONDS = {"short": 0.8, "long": 8, "limit": 22}
TRANSCRIPT = "Fixture voice input."
MAX_BODY = 2 * 1024 * 1024
MAX_ATTEMPTS = 64
ROUTES = {"/v1/messages": "chat", "/v1/audio/transcriptions": "stt", "/v1/audio/speech": "tts"}


def write_clip(path, profile):
    """0.5 s silence, deterministic voiced tone, 2 s silence; never records."""
    n = round(RATE * VOICED_SECONDS[profile])
    tone = b"".join(struct.pack("<h", round(9000 * math.sin(2 * math.pi * 320 * i / RATE))) for i in range(n))
    with wave.open(str(path), "wb") as out:
        out.setparams((1, 2, RATE, 0, "NONE", "not compressed"))
        out.writeframes(bytes(RATE) + tone + bytes(4 * RATE))


def chat_events(answer):
    text = answer["content"][0]["text"]
    events = [
        ("message_start", {"message": answer | {"content": [], "stop_reason": None}}),
        ("content_block_start", {"index": 0, "content_block": {"type": "text", "text": ""}}),
        ("content_block_delta", {"index": 0, "delta": {"type": "text_delta", "text": text}}),
        ("content_block_stop", {"index": 0}),
        ("message_delta", {"delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 8}}),
        ("message_stop", {}),
    ]
    return "".join(f"event: {kind}\ndata: {json.dumps(body | {'type': kind})}\n\n" for kind, body in events).encode()


class Provider(ThreadingHTTPServer):
    def __init__(self, audio, delayed=None, scratch=None):
        self.audio, self.delayed, self.scratch = audio, delayed, scratch
        self.token = secrets.token_hex(32)
        self.attempts, self.lock, self.done = [], threading.Lock(), threading.Event()
        self.overflow = False
        super().__init__(("127.0.0.1", 0), Handler)

    def get_request(self):
        sock, address = super().get_request()
        sock.settimeout(5)
        return sock, address

    def handle_error(self, request, client_address):
        # Never emit request data or exception payloads.
        pass


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_POST(self):
        kind = ROUTES.get(self.path)
        if kind is None:
            self.send_error(404)
            return
        header = "x-api-key" if kind == "chat" else "authorization"
        expected = self.server.token if kind == "chat" else f"Bearer {self.server.token}"
        if not hmac.compare_digest(self.headers.get(header, "").encode(), expected.encode()):
            self.send_error(401)
            return
        row = {"kind": kind, "request_body_bytes": 0, "response_body_bytes_written": 0}
        with self.server.lock:
            overflow = len(self.server.attempts) >= MAX_ATTEMPTS
            self.server.overflow |= overflow
            if not overflow:
                self.server.attempts.append(row)
        if overflow:
            self.send_error(429)
            return
        size = self.headers.get("Content-Length", "")
        if not size.isascii() or not size.isdecimal() or len(size) > 7 or not 0 < int(size) <= MAX_BODY:
            self.send_error(413)
            return
        size = int(size)
        raw = self.rfile.read(size)
        with self.server.lock:
            row["request_body_bytes"] = len(raw)
        if len(raw) != size:
            return
        mime = "application/json"
        try:
            if kind == "stt":
                if not self.headers.get("Content-Type", "").startswith("multipart/form-data;") or b"RIFF" not in raw:
                    raise ValueError
                payload = json.dumps({"text": TRANSCRIPT}).encode()
            elif kind == "tts":
                if not isinstance(json.loads(raw).get("input"), str):
                    raise ValueError
                mime, payload = "audio/mpeg", self.server.audio
            else:
                body = json.loads(raw)
                # Reuse the S9 provider's deterministic reply without its artifact-tool branch.
                answer = chat_response({"model": body.get("model", "phase9-qa")})
                if body.get("stream"):
                    mime, payload = "text/event-stream", chat_events(answer)
                else:
                    payload = json.dumps(answer).encode()
        except (AttributeError, ValueError):
            self.send_error(400)
            return
        if kind == self.server.delayed:
            if self.server.scratch is not None:
                (self.server.scratch / f"in-flight-{kind}").touch()
            if self.server.done.wait(8):
                return
        try:
            self.send_response(200)
            self.send_header("Content-Type", mime)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)
        except (ConnectionError, TimeoutError):
            return
        with self.server.lock:
            row["response_body_bytes_written"] = len(payload)
