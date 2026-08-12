#!/usr/bin/env python3
"""Deterministic Anthropic-protocol stub that drives the plan→build handoff.

Different job from `busy_input/mock_anthropic.py`, which scripts *timing*. This
one scripts *which tool the model reaches for on each turn*, and — the part no
unit test can reach — **records the tool surface the server actually sent**.

That recording is the point. The read-only floor has two halves that a unit test
can only assert about separately:

  * `PlanPhase::hides` removes a wholly-refused tool from the turn's tool list,
  * releasing the latch bumps `ScopedToolService::cache_generation`, so the NEXT
    turn's list is rebuilt with the floor lifted.

Only a real server assembles that list, and only a provider sees it. So every
request's `tools[]` names go to `observations.jsonl`, alongside the tool_result
blocks the previous turn's call produced — which is the other thing a unit test
cannot see: what the model was actually told when the floor refused it.

The turn counter advances **only for requests that carry tools**. A run makes
side-channel provider calls (titling, compaction, summarisation) that carry no
tool surface, and a plan indexed by "nth HTTP request" would silently desync the
moment one fired. Those are answered with plain text and logged as such.

Usage:  mock_plan.py PORT PLAN_NAME OBSERVATIONS_PATH
"""
import json
import os
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
PLAN_NAME = sys.argv[2] if len(sys.argv) > 2 else "handoff"
OBS_PATH = sys.argv[3] if len(sys.argv) > 3 else "/tmp/plan_handoff_observations.jsonl"

PROBE_PLANNING = "plan_handoff_probe_while_planning.txt"
PROBE_BUILDING = "plan_handoff_probe_after_approval.txt"

# A tool call the floor must refuse, one it must admit, and the handoff verb.
WRITE_WHILE_PLANNING = (
    "file_write",
    {"file_path": PROBE_PLANNING, "content": "the floor did not hold\n"},
)
WRITE_AFTER_APPROVAL = (
    "file_write",
    {"file_path": PROBE_BUILDING, "content": "the floor lifted\n"},
)
SET_PLAN = (
    "scratchpad",
    {
        "action": "set_plan",
        "items": ["read the code", "write the patch", "run the tests"],
    },
)
REQUEST_BUILD = ("scratchpad", {"action": "request_build"})

# Every plan opens with a CONTROL turn — a separate, ordinary `building`
# session that calls nothing. Its only job is to record the tool surface of a
# session that is NOT planning, on this same server, with this same config and
# session mode. Without it, "`bash` is absent while planning" is satisfied just
# as well by "`bash` was never offered in this mode at all", and the scenario
# would pass while testing nothing. (判据 §0: 空配置下「成功」与「默认值」长得一样.)
CONTROL = None

PLANS = {
    # The main end-to-end. Every step is a separate claim; see qa/README.md.
    "handoff": [
        CONTROL,  # 1. building: the A of the A/B
        WRITE_WHILE_PLANNING,  # 2. refused by the floor at dispatch
        SET_PLAN,  # 3. planning tools work
        REQUEST_BUILD,  # 4. raises the card; the driver approves
        WRITE_AFTER_APPROVAL,  # 5. must SUCCEED — same run, floor lifted
        None,  # 6. end
    ],
    # Same shape, opposite answer: the person declines.
    "deny": [
        CONTROL,
        SET_PLAN,
        REQUEST_BUILD,  # driver denies
        WRITE_WHILE_PLANNING,  # must STILL be refused
        None,
    ],
    # The floor sits above the explicit `[policies.tool_permissions]` layer and
    # above the `full` tier. `bash = "allow"` is in the config for this one.
    "floor": [
        CONTROL,
        ("bash", {"cmd": "echo the floor did not hold"}),  # explicit allow loses
        ("file_ops", {"operation": "list", "path": "~"}),  # read-only: admitted
        ("file_ops", {"operation": "delete", "path": PROBE_PLANNING}),  # refused
        None,
    ],
}
PLAN = PLANS[PLAN_NAME]

T0 = time.monotonic()
_n = [0]
_lock = threading.Lock()
_obs = open(OBS_PATH, "w")


def log(*a):
    print(f"{time.monotonic() - T0:7.2f}s [mock]", *a, flush=True)


def record(obj):
    _obs.write(json.dumps(obj) + "\n")
    _obs.flush()


def sse(payload):
    return f"event: {payload['type']}\ndata: {json.dumps(payload)}\n\n".encode()


def tool_results(msgs):
    """Every tool_result block in the conversation, oldest first.

    This is the model's own view of what happened to its calls — the only
    place a refusal's *wording* is observable end to end.
    """
    out = []
    for m in msgs:
        content = m.get("content")
        if not isinstance(content, list):
            continue
        for b in content:
            if isinstance(b, dict) and b.get("type") == "tool_result":
                c = b.get("content")
                if isinstance(c, list):
                    text = " ".join(
                        x.get("text", "") for x in c if isinstance(x, dict)
                    )
                else:
                    text = c if isinstance(c, str) else json.dumps(c)
                out.append(
                    {
                        "tool_use_id": b.get("tool_use_id"),
                        "is_error": bool(b.get("is_error")),
                        "text": text,
                    }
                )
    return out


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
        tools = [t.get("name") for t in (body.get("tools") or [])]

        # Side-channel calls (titling, compaction) carry no tool surface. They
        # must not consume a scripted turn — see the module doc.
        if not tools:
            log(f"side-channel call ({len(body.get('messages', []))} messages) -> plain text")
            record({"kind": "side_channel", "messages": len(body.get("messages", []))})
            self._answer(body, None, "acknowledged")
            return

        with _lock:
            _n[0] += 1
            turn = _n[0]
        step = PLAN[turn - 1] if turn <= len(PLAN) else None
        results = tool_results(body.get("messages", []))
        record(
            {
                "kind": "turn",
                "turn": turn,
                "tools_visible": sorted(tools),
                "tool_results": results,
                "will_call": step[0] if step else None,
            }
        )
        log(
            f"turn #{turn}: {len(tools)} tools visible "
            f"(file_write={'file_write' in tools}, bash={'bash' in tools}) "
            f"-> {'call ' + step[0] if step else 'end_turn'}"
        )
        if results:
            log(f"    last tool_result: {results[-1]['text'][:160]!r}")
        self._answer(body, step, f"mock turn {turn}")

    def _answer(self, body, step, text):
        turn_id = _n[0]
        if not body.get("stream"):
            content = [{"type": "text", "text": text}]
            if step:
                content.append(
                    {
                        "type": "tool_use",
                        "id": f"toolu_{turn_id}",
                        "name": step[0],
                        "input": step[1],
                    }
                )
            payload = {
                "id": f"msg_{turn_id}",
                "type": "message",
                "role": "assistant",
                "model": body.get("model", "qa-mock"),
                "content": content,
                "stop_reason": "tool_use" if step else "end_turn",
                "usage": {"input_tokens": 10, "output_tokens": 10},
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
                        "id": f"msg_{turn_id}",
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
                    "delta": {"type": "text_delta", "text": text},
                }
            )
        )
        chunk(sse({"type": "content_block_stop", "index": 0}))
        if step:
            chunk(
                sse(
                    {
                        "type": "content_block_start",
                        "index": 1,
                        "content_block": {
                            "type": "tool_use",
                            "id": f"toolu_{turn_id}",
                            "name": step[0],
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
                            "partial_json": json.dumps(step[1]),
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
                        "stop_reason": "tool_use" if step else "end_turn",
                        "stop_sequence": None,
                    },
                    "usage": {"output_tokens": 12},
                }
            )
        )
        chunk(sse({"type": "message_stop"}))
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def do_GET(self):
        raw = json.dumps({"data": [{"id": "qa-mock", "type": "model"}]}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)


log(f"listening on 127.0.0.1:{PORT} (plan {PLAN_NAME}, {len(PLAN)} steps, pid {os.getpid()})")
log(f"observations -> {OBS_PATH}")
ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
