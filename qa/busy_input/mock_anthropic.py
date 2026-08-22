#!/usr/bin/env python3
"""Deterministic Anthropic-protocol stub for real-machine QA.

The point is TIMING, not language. A scenario needs to know, to the second,
when an assistant turn commits and how long a run stays alive — neither of
which a real provider will tell you, and both of which every busy-input
behaviour is defined in terms of.

Two properties do the work:

  * Every turn but the last ends in a `tool_use`, which keeps the run alive
    across the assistant-message commit. That is what lets a scenario prove a
    redelivery was caused by the *burst draining* and not by the run slot
    freeing — the slot is still held.
  * Think-time before the first byte is scripted per turn, so "the steers land
    inside turn #3" is a fact about the plan rather than a hope.

Turn plans are named so scenarios can pick their own pacing:

  burst-drain    3,30,45,45,end   — Round-9: a long run with two mid-run commits
  long-run       3,90,end         — one turn alive for a minute and a half
  quick          1,1,end          — barely-alive run, for arrival-ordering checks
  channel-burst  2, then 20 x15   — several runs in flight at once (interrupt/queue)
  single-shot    end, end, end…   — every turn answers immediately with no tool
                                    call, so each `chat.send` is exactly ONE
                                    priced LLM call. Round-7 (per-principal spend
                                    budget): a `quick`-style plan's second "tool"
                                    turn lets the metering floor's mid-run check
                                    fire on turn 2 once turn 1's cost crosses a
                                    tiny ceiling — a DIFFERENT denial path
                                    (`ExecutionError::Failed`, generic) than the
                                    run-admission arm's `SpendExhausted` this
                                    plan exists to isolate. One call in, one
                                    priced call out, nothing else moves.

Two optional trailing arguments let a scenario say WHAT the turn calls and
capture WHAT THE MODEL SAW coming back:

  tool_spec   path to `{"name": ..., "input": {...}}` — the tool call every
              `tool` turn emits, instead of the default `file_read` probe.
  request_log path to append each incoming request body to, one JSON object
              per line. Turn N+1's `messages` carry turn N's `tool_result`
              verbatim, so this file is the only oracle for what a tool
              actually handed the model — the tool's own RPC reply is a
              different thing on a different path.

Usage:  mock_anthropic.py [port] [probe_path] [plan_name] [tool_spec] [request_log]
"""
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18991
PROBE = sys.argv[2] if len(sys.argv) > 2 else "/etc/hostname"
PLAN_NAME = sys.argv[3] if len(sys.argv) > 3 else "burst-drain"
TOOL_SPEC_PATH = sys.argv[4] if len(sys.argv) > 4 else ""
REQUEST_LOG = sys.argv[5] if len(sys.argv) > 5 else ""

# The tool every `tool` turn calls. Default keeps the historical probe so the
# busy-input scenarios are byte-for-byte unaffected.
TOOL_SPEC = {"name": "file_read", "input": {"path": PROBE}}
if TOOL_SPEC_PATH:
    with open(TOOL_SPEC_PATH) as _fh:
        TOOL_SPEC = json.load(_fh)

PLANS = {
    "burst-drain": [(3, "tool"), (30, "tool"), (45, "tool"), (45, "tool"), (0, "end")],
    "long-run": [(3, "tool"), (90, "tool"), (0, "end")],
    "quick": [(1, "tool"), (1, "tool"), (0, "end")],
    # For scenarios that put SEVERAL runs in flight (interrupt / queue bursts).
    # The turn counter is global, not per-run, so a plan that ends after a few
    # entries would have the second run finish the moment it started and leave
    # the third message with nothing to interrupt. A long flat tail keeps every
    # run in the scenario alive; teardown, not the plan, ends them.
    "channel-burst": [(2, "tool")] + [(20, "tool")] * 15 + [(0, "end")],
    # See the module doc's "single-shot" entry. 200 turns is far more than any
    # scenario needs; the global turn counter (see PLAN[turn - 1] below) means
    # every one of them must answer "end" for the guarantee to hold across a
    # whole fixture run, not just the first call.
    "single-shot": [(0, "end")] * 200,
}
PLAN = PLANS.get(PLAN_NAME, PLANS["burst-drain"])

T0 = time.monotonic()
_n = [0]
_lock = threading.Lock()


def log(*a):
    print(f"{time.monotonic() - T0:7.2f}s [mock]", *a, flush=True)


def sse(payload):
    return f"event: {payload['type']}\ndata: {json.dumps(payload)}\n\n".encode()


def describe(msgs):
    """What the harness is carrying into this turn.

    The message count and the trailing user text are the cheapest available
    evidence for whether a message was *steered into the live loop* (it shows
    up appended to an existing conversation) or *ran separately* (it opens a
    conversation of its own).
    """
    if not msgs:
        return "no messages"
    last_user = next(
        (m for m in reversed(msgs) if m.get("role") == "user"),
        None,
    )
    text = ""
    if last_user:
        content = last_user.get("content")
        if isinstance(content, str):
            text = content
        elif isinstance(content, list):
            text = " ".join(
                b.get("text", "") for b in content if isinstance(b, dict)
            )
    return f"{len(msgs)} messages, last user text: {text.strip()[:120]!r}"


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_POST(self):
        # A cancelled run drops the SSE connection mid-stream, so a broken pipe
        # here is not an error — it is the clearest evidence available that the
        # cancellation actually reached the in-flight provider call. Report it
        # as such instead of dumping a traceback that reads like a fixture bug.
        try:
            self._do_post()
        except (BrokenPipeError, ConnectionResetError):
            log("client disconnected mid-stream (run cancelled) — expected under interrupt")

    def _do_post(self):
        n = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        with _lock:
            _n[0] += 1
            turn = _n[0]
        think, kind = PLAN[turn - 1] if turn <= len(PLAN) else (0, "end")
        msgs = body.get("messages", [])
        if REQUEST_LOG:
            # Append under the same lock that hands out turn numbers, so a
            # scenario reading this file can trust the ordering.
            with _lock, open(REQUEST_LOG, "a") as fh:
                fh.write(json.dumps({"turn": turn, "body": body}) + "\n")
        log(f"turn #{turn} request ({describe(msgs)}) -> thinking {think}s, then {kind}")
        time.sleep(think)

        if not body.get("stream"):
            content = [{"type": "text", "text": f"mock turn {turn}"}]
            if kind == "tool":
                content.append(
                    {
                        "type": "tool_use",
                        "id": f"toolu_{turn}",
                        "name": TOOL_SPEC["name"],
                        "input": TOOL_SPEC["input"],
                    }
                )
            payload = {
                "id": f"msg_{turn}",
                "type": "message",
                "role": "assistant",
                "model": body.get("model", "qa-mock"),
                "content": content,
                "stop_reason": "tool_use" if kind == "tool" else "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 10},
            }
            raw = json.dumps(payload).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)
            log(f"turn #{turn} answered (non-streaming, {kind})")
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
                        "id": f"msg_{turn}",
                        "type": "message",
                        "role": "assistant",
                        "model": body.get("model", "qa-mock"),
                        "content": [],
                        "stop_reason": None,
                        "stop_sequence": None,
                        "usage": {"input_tokens": 10, "output_tokens": 1},
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
                    "delta": {
                        "type": "text_delta",
                        "text": f"mock turn {turn}: still working.",
                    },
                }
            )
        )
        chunk(sse({"type": "content_block_stop", "index": 0}))
        if kind == "tool":
            chunk(
                sse(
                    {
                        "type": "content_block_start",
                        "index": 1,
                        "content_block": {
                            "type": "tool_use",
                            "id": f"toolu_{turn}",
                            "name": TOOL_SPEC["name"],
                            "input": {},
                        },
                    }
                )
            )
            chunk(
                sse(
                    {
                        "type": "content_block_delta",
                        "index": 1,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": json.dumps(TOOL_SPEC["input"]),
                        },
                    }
                )
            )
            chunk(sse({"type": "content_block_stop", "index": 1}))
        chunk(
            sse(
                {
                    "type": "message_delta",
                    "delta": {
                        "stop_reason": "tool_use" if kind == "tool" else "end_turn",
                        "stop_sequence": None,
                    },
                    "usage": {"output_tokens": 12},
                }
            )
        )
        chunk(sse({"type": "message_stop"}))
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()
        log(f"turn #{turn} ASSISTANT TURN STREAMED ({kind})")

    def do_GET(self):
        raw = json.dumps({"data": [{"id": "qa-mock", "type": "model"}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


log(f"listening on 127.0.0.1:{PORT} (plan {PLAN_NAME}: {PLAN}, pid {os.getpid()})")
ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
