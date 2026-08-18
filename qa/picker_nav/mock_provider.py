#!/usr/bin/env python3
"""Minimal Anthropic-protocol stub that answers a connectivity probe.

A fourth job, distinct from the three stubs already in `qa/`:

  * `busy_input/mock_anthropic.py` scripts **timing** — when a turn commits.
  * `plan_handoff/mock_plan.py` scripts **which tool** and records the tool
    surface the server sent.
  * `announce/mock_announce.py` records what the model was handed on turns
    **nobody's client asked for**.

This one answers exactly one question — "can this endpoint answer a ping?" —
because that is the entire content of `providers::probe::probe_provider`: one
`UnifiedMessage::user("ping")` round-trip, no tools, no turn plan. Scripting a
plan here would be scaffolding for a loop that never runs.

What it *does* own is the oracle for **which row the button dialled**. The
probe carries no provider label on the wire; it carries the model from that
provider's stored config. So the fixture gives every mock-backed provider a
distinct model id and this stub appends every request's model to a log — which
is how "I pressed Test on qa-mock" is told apart from "I pressed Test on some
other row and the UI attributed it to qa-mock". The button's own reply cannot
answer that: it is the same shape either way.

The failure arm deliberately has no mode here. A provider pointed at a **closed
port** produces a real connection refusal from the real client stack; a stub
that returns HTTP 500 on demand would instead be testing the stub's idea of
what failure looks like.

Usage:  mock_provider.py PORT [REQUEST_LOG]
"""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18992
REQUEST_LOG = sys.argv[2] if len(sys.argv) > 2 else ""

T0 = time.monotonic()
_lock = threading.Lock()
_n = [0]


def log(*a):
    print(f"{time.monotonic() - T0:7.2f}s [mock-provider]", *a, flush=True)


def sse(payload):
    return f"event: {payload['type']}\ndata: {json.dumps(payload)}\n\n".encode()


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_POST(self):
        try:
            self._do_post()
        except (BrokenPipeError, ConnectionResetError):
            log("client disconnected mid-response")

    def _do_post(self):
        n = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        model = body.get("model", "?")
        with _lock:
            _n[0] += 1
            seq = _n[0]
            if REQUEST_LOG:
                with open(REQUEST_LOG, "a") as fh:
                    fh.write(json.dumps({"seq": seq, "model": model}) + "\n")
        log(f"probe #{seq} model={model!r} stream={bool(body.get('stream'))}")

        if not body.get("stream"):
            payload = {
                "id": f"msg_{seq}",
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [{"type": "text", "text": "pong"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 4, "output_tokens": 1},
            }
            raw = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)
            return

        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

        def chunk(b):
            self.wfile.write(f"{len(b):X}\r\n".encode() + b + b"\r\n")
            self.wfile.flush()

        chunk(
            sse(
                {
                    "type": "message_start",
                    "message": {
                        "id": f"msg_{seq}",
                        "type": "message",
                        "role": "assistant",
                        "model": model,
                        "content": [],
                        "stop_reason": None,
                        "stop_sequence": None,
                        "usage": {"input_tokens": 4, "output_tokens": 1},
                    },
                }
            )
        )
        chunk(
            sse(
                {
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""},
                }
            )
        )
        chunk(
            sse(
                {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "pong"},
                }
            )
        )
        chunk(sse({"type": "content_block_stop", "index": 0}))
        chunk(
            sse(
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                    "usage": {"output_tokens": 1},
                }
            )
        )
        chunk(sse({"type": "message_stop"}))
        chunk(b"")


if __name__ == "__main__":
    log(f"listening on 127.0.0.1:{PORT}")
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
