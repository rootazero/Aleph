#!/usr/bin/env python3
"""Deterministic Anthropic-protocol stub for the run-halt real-machine QA.

A fourth job, distinct from the three stubs already here:

  * `busy_input/mock_anthropic.py` scripts **timing** — when a turn commits.
  * `plan_handoff/mock_plan.py` scripts **which tool**.
  * `announce/mock_announce.py` scripts a **side effect that outlives the run**.

This one scripts **how the run ends**, which is the only thing a fixture can do
that a unit test cannot: `terminate_reason` is written by the harness, adjusted
by the orchestrator bridge, settled by the runner, synthesized-or-forwarded by
the gateway drain and rendered by four clients. Every one of those had a test
that passed while the chain was broken end to end (§3.17c: the whole terminal
settle sat below `run_result.map_err(..)?`, so a run that failed reported
`completed`, 0 tokens, 0 loops).

Two plans, and the second is not a variation of the first — they take opposite
arms of the flow:

  crash   burn some turns and tokens, then refuse with HTTP 401. The provider
          chain classifies that as fatal, the run ends on the FAILURE arm, and
          the claim is that the receipt still carries the work that was done.
  cap     never stop asking for tools, so `max_iterations` trips. That is the
          SUCCESS arm — the run finished, it just finished early — and because
          each turn also emits text, `escalate_partial_result` folds the cap
          into the `budget_exhausted_partial_result` umbrella and puts the real
          cap in `terminate_detail`. That is the field three of the five
          rendering surfaces used to ignore.

Turn counting follows `mock_plan.py`'s rule and for the same reason: only
requests carrying a tool surface are turns. Titling and compaction fire with no
tools, and a plan indexed by "nth HTTP request" desyncs the moment one does.

Usage:  mock_halt.py PORT PLAN_NAME OBSERVATIONS_PATH [TURNS_BEFORE_CRASH]
"""
import json
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
PLAN_NAME = sys.argv[2] if len(sys.argv) > 2 else "crash"
OBS_PATH = sys.argv[3] if len(sys.argv) > 3 else "/tmp/run_halt_observations.jsonl"
# How many tool-calling turns happen before the provider refuses. It must be
# >= 1 or the claim degenerates: a run that fails on its very first request has
# no work to lose, and "reports 0 tokens" would then be the truth.
BURN_TURNS = int(sys.argv[4]) if len(sys.argv) > 4 else 2

# A tool that always exists, always succeeds, and touches nothing. `bash` is not
# idempotent, so `run.sh` grants it explicitly the way an operator would.
IDLE_TOOL = ("bash", {"cmd": "true"})

T0 = time.monotonic()
_turn = [0]
_lock = threading.Lock()


def log(*a):
    print(f"{time.monotonic() - T0:7.2f}s [mock]", *a, flush=True)


def sse(payload):
    return f"event: {payload['type']}\ndata: {json.dumps(payload)}\n\n".encode()


def observe(record):
    with _lock, open(OBS_PATH, "a") as fh:
        fh.write(json.dumps(record) + "\n")


def action_for(turn):
    """`("tool", spec)` / `("crash", None)` — what turn number `turn` does."""
    if PLAN_NAME == "cap":
        # No terminal arm at all: the cap is what has to stop this, and a plan
        # that ends on its own would prove nothing about the cap.
        return ("tool", IDLE_TOOL)
    return ("tool", IDLE_TOOL) if turn <= BURN_TURNS else ("crash", None)


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_POST(self):
        try:
            self._do_post()
        except (BrokenPipeError, ConnectionResetError):
            log("client disconnected mid-stream")

    def _do_post(self):
        n = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")

        if not body.get("tools"):
            observe({"kind": "side-channel", "messages": len(body.get("messages", []))})
            log("side-channel request (no tool surface) -> plain text")
            self._answer(body, 0, None)
            return

        with _lock:
            _turn[0] += 1
            turn = _turn[0]
        kind, tool = action_for(turn)
        observe({"kind": "turn", "turn": turn, "action": kind,
                 "messages": len(body.get("messages", []))})
        log(f"turn #{turn} -> {kind}")

        if kind == "crash":
            self._refuse()
            return
        self._answer(body, turn, tool)

    def _refuse(self):
        """The provider says no, in the one way that is never retried.

        401 rather than 500 on purpose: a 5xx is transient and the retry ladder
        would spend the run's budget on it, which turns a deterministic fixture
        into a slow one. `authentication_error` is terminal at the first
        attempt, so the run reaches its failure arm immediately and the fixture
        measures the settle, not the ladder.
        """
        raw = json.dumps(
            {
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "QA fixture: the provider refuses this key",
                },
            }
        ).encode()
        self.send_response(401)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _answer(self, body, turn, tool):
        content = [{"type": "text", "text": f"mock turn {turn}"}]
        if tool:
            content.append(
                {
                    "type": "tool_use",
                    "id": f"toolu_{turn}_{int(time.monotonic() * 1000) % 100000}",
                    "name": tool[0],
                    "input": tool[1],
                }
            )
        payload = {
            "id": f"msg_{turn}",
            "type": "message",
            "role": "assistant",
            "model": body.get("model", "qa-mock"),
            "content": content,
            "stop_reason": "tool_use" if tool else "end_turn",
            # Non-zero and asymmetric so a receipt that reports "0 tokens" is
            # provably wrong rather than plausibly empty.
            "usage": {"input_tokens": 137, "output_tokens": 41},
        }

        if not body.get("stream"):
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

        start = dict(payload)
        start["content"] = []
        start["stop_reason"] = None
        chunk(sse({"type": "message_start", "message": start}))
        chunk(sse({"type": "content_block_start", "index": 0,
                   "content_block": {"type": "text", "text": ""}}))
        chunk(sse({"type": "content_block_delta", "index": 0,
                   "delta": {"type": "text_delta", "text": f"mock turn {turn}"}}))
        chunk(sse({"type": "content_block_stop", "index": 0}))
        if tool:
            chunk(sse({"type": "content_block_start", "index": 1,
                       "content_block": {"type": "tool_use",
                                         "id": payload["content"][1]["id"],
                                         "name": tool[0], "input": {}}}))
            chunk(sse({"type": "content_block_delta", "index": 1,
                       "delta": {"type": "input_json_delta",
                                 "partial_json": json.dumps(tool[1])}}))
            chunk(sse({"type": "content_block_stop", "index": 1}))
        chunk(sse({"type": "message_delta",
                   "delta": {"stop_reason": "tool_use" if tool else "end_turn"},
                   "usage": {"output_tokens": 41}}))
        chunk(sse({"type": "message_stop"}))
        chunk(b"")


if __name__ == "__main__":
    open(OBS_PATH, "w").close()
    log(f"plan={PLAN_NAME} burn_turns={BURN_TURNS} obs={OBS_PATH}")
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
