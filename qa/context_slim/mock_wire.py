#!/usr/bin/env python3
"""Deterministic LLM endpoint for the context-slim fixture, driven by STATE.

It answers two wires on one port:

  * `…/chat/completions` — DeepSeek's OpenAI-compatible wire in thinking
    mode. It enforces the rule the real API enforces: on a request that
    carries `tools`, every earlier assistant message must carry
    `reasoning_content`, or the answer is a 400. (api-docs.deepseek.com
    /guides/thinking_mode: "the reasoning_content must be fully passed back
    to the API in all subsequent requests … the API will return a 400
    error".)
  * `…/v1/messages` — the Anthropic wire, streaming.

The server reaches this mock either directly (a `127.0.0.1` base_url — which
Aleph classifies as a Custom host) or as an HTTP PROXY for a first-party
hostname (`base_url = "http://api.deepseek.com"` + `HTTP_PROXY` pointing
here): reqwest then sends an absolute-form request line and this handler
serves it. That is the only way a local mock can sit behind a host Aleph
classifies as DeepSeek-native or Anthropic first-party — both classifiers
key on the hostname.

What each answer is depends on the conversation, never on a turn counter: a
run makes side-channel calls (strategy planning, titling) that carry no tool
surface, so "turn N" means nothing. A request without `tools` gets a short
text answer and nothing else.

Every request is appended to REQUEST_LOG as
`{"n", "path", "headers", "body", "rejected"}`.

Usage:  mock_wire.py PORT SCENARIO REQUEST_LOG [--require-field NAME]

  SCENARIO   replay | ingress | gate | firstparty
  --require-field  the assistant-message field the 400 rule checks
             (default `reasoning_content`). Pointing it at a field the code
             never sends is the fixture's own break-it switch.
"""
import argparse
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ap = argparse.ArgumentParser()
ap.add_argument("port", type=int)
ap.add_argument("scenario", choices=["replay", "ingress", "gate", "firstparty"])
ap.add_argument("request_log")
ap.add_argument("--require-field", default="reasoning_content")
ARGS = ap.parse_args()

NEEDLE = "QA_NEEDLE_LINE unicorn marmalade"
GATE_LINES = int(__import__("os").environ.get("QA_GATE_LINES", "2600"))
_n = [0]
_lock = threading.Lock()
T0 = time.monotonic()


def log(*a):
    print(f"{time.monotonic() - T0:7.2f}s [mock]", *a, flush=True)


def record(path, headers, body, rejected):
    with _lock:
        _n[0] += 1
        n = _n[0]
        with open(ARGS.request_log, "a") as fh:
            fh.write(
                json.dumps(
                    {
                        "n": n,
                        "path": path,
                        "headers": {k.lower(): v for k, v in headers.items()},
                        "body": body,
                        "rejected": rejected,
                    }
                )
                + "\n"
            )
    return n


def called_since_driver_message(msgs):
    """Whether an assistant tool call follows the driver's latest message.

    "The last user message" is not the driver's: the harness appends
    user-role reminders after tool results, so a rule keyed on the last user
    message never sees its own tool call and calls again every step. Every
    message the driver sends starts with `QA `, and reminders do not.
    """
    last = max(
        (i for i, m in enumerate(msgs)
         if m.get("role") == "user" and "QA " in json.dumps(m.get("content", ""))),
        default=-1,
    )
    return any(m.get("role") == "assistant" and m.get("tool_calls") for m in msgs[last + 1:])


# ── DeepSeek (OpenAI chat) ────────────────────────────────────────────────────


def deepseek_missing(body):
    """Assistant messages that lack the required field (only when tools)."""
    if not body.get("tools"):
        return []
    return [
        i
        for i, m in enumerate(body.get("messages", []))
        if m.get("role") == "assistant" and ARGS.require_field not in m
    ]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _json(self, code, obj):
        raw = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _sse_start(self):
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

    def _chunk(self, b):
        self.wfile.write(f"{len(b):X}\r\n".encode() + b + b"\r\n")
        self.wfile.flush()

    def _end(self):
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def do_GET(self):
        self._json(200, {"data": [{"id": "qa-model", "object": "model", "type": "model"}]})

    def do_POST(self):
        try:
            n = int(self.headers.get("content-length", 0))
            body = json.loads(self.rfile.read(n) or b"{}")
            if self.path.endswith("/chat/completions"):
                self._deepseek(body)
            elif self.path.endswith("/v1/messages"):
                self._anthropic(body)
            else:
                record(self.path, self.headers, body, False)
                self._json(404, {"error": {"message": f"mock: no route {self.path}"}})
        except (BrokenPipeError, ConnectionResetError):
            log("client disconnected")

    # ── DeepSeek ──

    def _deepseek(self, body):
        missing = deepseek_missing(body)
        n = record(self.path, self.headers, body, bool(missing))
        if missing:
            log(f"#{n} 400: assistant messages {missing} lack {ARGS.require_field}")
            self._json(
                400,
                {
                    "error": {
                        "message": "The reasoning_content in the thinking mode must be "
                        "passed back to the API.",
                        "type": "invalid_request_error",
                    }
                },
            )
            return
        msgs = body.get("messages", [])
        call = bool(body.get("tools")) and not called_since_driver_message(msgs)
        reasoning = f"QA_REASONING_{n} deciding what to do"
        self._sse_start()

        def data(obj):
            self._chunk(f"data: {json.dumps(obj)}\n\n".encode())

        def delta(d, finish=None):
            data({"id": f"c{n}", "object": "chat.completion.chunk",
                  "choices": [{"index": 0, "delta": d, "finish_reason": finish}]})

        if body.get("tools"):
            delta({"role": "assistant", "reasoning_content": reasoning})
        if call:
            delta({"tool_calls": [{"index": 0, "id": f"call_{n}", "type": "function",
                                   "function": {"name": "file_read",
                                                "arguments": json.dumps({"path": "/etc/hosts"})}}]})
            delta({}, "tool_calls")
        else:
            delta({"content": f"QA answer {n}."})
            delta({}, "stop")
        data({"id": f"c{n}", "object": "chat.completion.chunk", "choices": [],
              "usage": {"prompt_tokens": 50, "completion_tokens": 10, "total_tokens": 60}})
        self._chunk(b"data: [DONE]\n\n")
        self._end()
        log(f"#{n} deepseek {'tool_call' if call else 'answer'}")

    # ── Anthropic ──

    def _anthropic(self, body):
        n = record(self.path, self.headers, body, False)
        msgs = body.get("messages", [])

        def is_tool_result(m):
            c = m.get("content")
            return m.get("role") == "user" and isinstance(c, list) and any(
                isinstance(b, dict) and b.get("type") == "tool_result" for b in c
            )

        done = sum(1 for m in msgs if is_tool_result(m))
        tool = None
        if body.get("tools"):
            if ARGS.scenario == "ingress" and done == 0:
                # One process is enough for this stage; compound commands also
                # work (a `| cat` variant passed 2026-09-26). The exit 71 once
                # blamed on process-fork was the Homebrew-bash resolution defect.
                tool = ("bash", {"cmd": "awk 'BEGIN{for(i=1;i<=90000;i++){print i; "
                                        f"if(i==60000) print \"{NEEDLE}\"}}}}'"})
            elif ARGS.scenario == "ingress" and done == 1:
                tool = ("ctx_search", {"queries": ["unicorn marmalade"]})
            elif ARGS.scenario == "gate" and done == 0:
                # Sized to land between DEFAULT_RESULT_BUDGET_TOKENS and
                # MAX_RESULT_BUDGET_TOKENS; `drive.py gate` measures it the way Layer 2 does
                # before trusting the arm.
                tool = ("bash", {"cmd": f"awk 'BEGIN{{for(i=1;i<={GATE_LINES};i++) print i}}'"})
            elif ARGS.scenario == "firstparty" and done == 0:
                tool = ("file_read", {"path": "/etc/hosts"})
        thinking = ARGS.scenario == "firstparty" and bool(body.get("tools"))
        self._sse_start()

        def ev(obj):
            self._chunk(f"event: {obj['type']}\ndata: {json.dumps(obj)}\n\n".encode())

        ev({"type": "message_start", "message": {
            "id": f"msg_{n}", "type": "message", "role": "assistant",
            "model": body.get("model", "qa"), "content": [], "stop_reason": None,
            "stop_sequence": None, "usage": {"input_tokens": 50, "output_tokens": 1}}})
        idx = 0
        if thinking:
            ev({"type": "content_block_start", "index": idx,
                "content_block": {"type": "thinking", "thinking": "", "signature": ""}})
            ev({"type": "content_block_delta", "index": idx,
                "delta": {"type": "thinking_delta", "thinking": f"QA_REASONING_{n} plan"}})
            ev({"type": "content_block_delta", "index": idx,
                "delta": {"type": "signature_delta", "signature": f"qa_sig_{n}"}})
            ev({"type": "content_block_stop", "index": idx})
            idx += 1
        ev({"type": "content_block_start", "index": idx,
            "content_block": {"type": "text", "text": ""}})
        ev({"type": "content_block_delta", "index": idx,
            "delta": {"type": "text_delta", "text": f"QA turn {n}."}})
        ev({"type": "content_block_stop", "index": idx})
        idx += 1
        if tool:
            ev({"type": "content_block_start", "index": idx, "content_block": {
                "type": "tool_use", "id": f"toolu_{n}", "name": tool[0], "input": {}}})
            ev({"type": "content_block_delta", "index": idx, "delta": {
                "type": "input_json_delta", "partial_json": json.dumps(tool[1])}})
            ev({"type": "content_block_stop", "index": idx})
        ev({"type": "message_delta", "delta": {
            "stop_reason": "tool_use" if tool else "end_turn", "stop_sequence": None},
            "usage": {"output_tokens": 12}})
        ev({"type": "message_stop"})
        self._end()
        log(f"#{n} anthropic {tool[0] if tool else 'answer'}")


log(f"listening on 127.0.0.1:{ARGS.port} scenario={ARGS.scenario} "
    f"require={ARGS.require_field}")
ThreadingHTTPServer(("127.0.0.1", ARGS.port), Handler).serve_forever()
