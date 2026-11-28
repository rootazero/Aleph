"""Loopback-only synthetic providers and media for the NSH Voice QA runner."""

import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import secrets
import struct
import threading
import time
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


def chat_events(answer, first_sentence=None):
    text = answer["content"][0]["text"]
    events = [
        ("message_start", {"message": answer | {"content": [], "stop_reason": None}}),
        ("content_block_start", {"index": 0, "content_block": {"type": "text", "text": ""}}),
        ("content_block_delta", {"index": 0, "delta": {"type": "text_delta", "text": text}}),
        ("content_block_stop", {"index": 0}),
        ("message_delta", {"delta": {"stop_reason": "end_turn", "stop_sequence": None}, "usage": {"output_tokens": 8}}),
        ("message_stop", {}),
    ]
    if first_sentence:
        events.insert(2, ("content_block_delta", {"index": 0, "delta": {"type": "text_delta", "text": first_sentence}}))
    return [f"event: {kind}\ndata: {json.dumps(body | {'type': kind})}\n\n".encode() for kind, body in events]


class Provider(ThreadingHTTPServer):
    QUEUE_MARKERS = ("SIBLING", "QUEUED_SUCCESSOR", "QUEUED", "RUNNING_SUCCESSOR", "RUNNING")

    def __init__(self, audio, delayed=None, scratch=None, progressive=False, empty_transcript=False,
                 queue_control=None):
        self.audio, self.delayed, self.scratch = audio, delayed, scratch
        self.progressive = progressive
        self.empty_transcript = empty_transcript
        self.queue_control = queue_control
        self.queue_markers = []
        self.token = secrets.token_hex(32)
        self.attempts, self.lock, self.done = [], threading.Lock(), threading.Event()
        self.overflow = False
        super().__init__(("127.0.0.1", 0), Handler)
        if queue_control is not None:
            (queue_control / "status.json").write_text('{"markers": []}\n', encoding="utf-8")

    def admit_queue_request(self, body):
        """Attribute only the latest user turn, never a marker retained in Chat history."""
        if self.queue_control is None:
            return True
        # Core may wrap the latest turn in a structured content envelope. The
        # last fixed marker in the complete request still identifies the new
        # input; selecting the first marker would misattribute retained history.
        encoded = json.dumps(body).encode()
        occurrences = [(encoded.rfind(f"VOICE_QA_{name}".encode()), name) for name in self.QUEUE_MARKERS]
        occurrences = [(position, name) for position, name in occurrences if position >= 0]
        marker = max(occurrences)[1] if occurrences else None
        if marker is None:
            return True
        with self.lock:
            self.queue_markers.append(marker)
            temporary = self.queue_control / f"status-{threading.get_ident()}.tmp"
            temporary.write_text(json.dumps({"markers": self.queue_markers}) + "\n", encoding="utf-8")
            temporary.replace(self.queue_control / "status.json")
        if marker not in ("SIBLING", "RUNNING"):
            return True
        deadline = time.monotonic() + 15
        release = self.queue_control / f"release-{marker}"
        while not release.exists() and time.monotonic() < deadline:
            if self.done.wait(0.05):
                return False
        return release.exists()

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
        cutoff = None
        try:
            if kind == "stt":
                if not self.headers.get("Content-Type", "").startswith("multipart/form-data;") or b"RIFF" not in raw:
                    raise ValueError
                payload = json.dumps({"text": "" if self.server.empty_transcript else TRANSCRIPT}).encode()
            elif kind == "tts":
                if not isinstance(json.loads(raw).get("input"), str):
                    raise ValueError
                mime, payload = "audio/mpeg", self.server.audio
            else:
                body = json.loads(raw)
                if not self.server.admit_queue_request(body):
                    self.send_error(503)
                    return
                # Reuse the S9 provider's deterministic reply without its artifact-tool branch.
                answer = chat_response({"model": body.get("model", "phase9-qa")})
                if body.get("stream"):
                    first = "The first reply is ready. " if self.server.progressive else None
                    chunks = chat_events(answer, first)
                    mime, payload = "text/event-stream", b"".join(chunks)
                    if first:
                        cutoff = sum(map(len, chunks[:3]))
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
            if cutoff is not None:
                self.wfile.write(payload[:cutoff])
                self.wfile.flush()
                if self.server.done.wait(2):
                    return
                self.wfile.write(payload[cutoff:])
            else:
                self.wfile.write(payload)
        except (ConnectionError, TimeoutError):
            return
        with self.server.lock:
            row["response_body_bytes_written"] = len(payload)
