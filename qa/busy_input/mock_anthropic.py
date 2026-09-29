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
              `tool` turn emits, instead of the default `file_read` probe. A
              JSON *list* of those is also accepted: turn N emits entry N,
              cycling, for claims that need two different calls in one
              conversation. N here counts *run* turns, not every request —
              planning calls (see `n_planning` below) are answered with
              text-only and do not advance the index. If the scenario does
              not set `n_planning`, the index matches the global request
              counter, which is the historical behaviour every existing
              busy-input scenario was written against.
  request_log path to append each incoming request body to, one JSON object
              per line. Each entry carries `turn` (global request count),
              `is_planning` (bool), `run_turn` (1-based run index, what
              `tool_spec` keys off), and the request `body`. Turn N+1's
              `messages` carry turn N's `tool_result` verbatim, so this
              file is the only oracle for what a tool actually handed the
              model — the tool's own RPC reply is a different thing on a
              different path.
  n_planning   integer (default 0) — the first N requests this mock answers
              are tagged `is_planning: true` and return text-only
              responses (no `tool_use`). Scenarios that exercise the
              server's side-channel planning call pass the number of
              planning calls they issue. Because the mock has no session
              affinity, this is a process-wide counter: a scenario that
              opens K conversations each with one planning call sets it
              to K.

Usage:  mock_anthropic.py [port] [probe_path] [plan_name] [tool_spec] [request_log] [n_planning]
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
#
# A spec file may hold either one object (every `tool` turn calls the same
# thing — what every scenario before 2026-08-29 wanted) or a LIST of them, in
# which case turn N calls entry N, cycling. The list form exists because some
# claims are about what the *previous* turn's tool_result carried, and a
# control arm for those has to be a different call in the same conversation:
# `qa/file_search` asserts that a shell `grep -r` comes back with a steer and
# that an `rg` does not, which is one assertion about two adjacent turns.
_spec = {"name": "file_read", "input": {"path": PROBE}}
if TOOL_SPEC_PATH:
    with open(TOOL_SPEC_PATH) as _fh:
        _spec = json.load(_fh)
TOOL_SPECS = _spec if isinstance(_spec, list) else [_spec]

# Per-connection request counter — the protocol doesn't carry one. We can't
# tell which request is the first of a conversation from headers alone, so
# the server's side-channel planning call (the FIRST request of every
# conversation, before the run's own turn 1) would silently shift the
# `tool_spec [N, ...]` indexing by one if we counted every request as a run
# turn.
#
# Two knobs keep the indexing stable:
#
#  * `--n-planning=N` (sixth positional arg, default 0): the FIRST N requests
#    this mock answers are tagged as planning calls. They return text-only
#    answers (no `tool_use`) and do not advance `tool_spec` indexing — the
#    N-th *run* turn still gets entry N. Scenarios that exercise the
#    planning call set this to the number of conversations they open, not
#    the number of planning calls per conversation.
#
#  * `request_log` (seventh positional arg) carries `turn`, `is_planning`,
#    `n_answered_so_far`, and `body` per request, so scenarios that need
#    exact turn→entry pairing derive the mapping themselves instead of
#    trusting the mock's local counter.
N_PLANNING = int(sys.argv[6]) if len(sys.argv) > 6 else 0

_n = [0]
_n_planning_seen = [0]
_n_run_seen = [0]
_lock = threading.Lock()

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
    # Several fast tool turns and then an ending. For scenarios whose claim is
    # about a tool RESULT rather than about timing: a run makes side-channel
    # provider calls (strategy planning, titling, compaction) that carry no
    # tool surface, and this counter advances for those too — so a scenario
    # cannot assume "turn 2 holds turn 1's result" and needs slack plus a
    # content-based oracle. See `qa/file_search/drive_turn.py`.
    "tool-chain": [(1, "tool")] * 9 + [(0, "end")],
}
PLAN = PLANS.get(PLAN_NAME, PLANS["burst-drain"])

T0 = time.monotonic()


def log(*a):
    print(f"{time.monotonic() - T0:7.2f}s [mock]", *a, flush=True)


def spec_for(run_turn):
    """The tool call the N-th *run* turn (1-based) emits.

    A planning call (see `_do_post`) does not call this — it returns text-only
    and does not advance the run-turn counter, so the N-th run turn keeps
    getting entry N even when N_PLANNING > 0.
    """
    return TOOL_SPECS[(run_turn - 1) % len(TOOL_SPECS)]


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
            is_planning = _n_planning_seen[0] < N_PLANNING
            if is_planning:
                _n_planning_seen[0] += 1
                run_turn = _n_run_seen[0]  # does not advance on planning
            else:
                _n_run_seen[0] += 1
                run_turn = _n_run_seen[0]
            n_answered = _n[0]
            log_entry = {
                "turn": n_answered,
                "is_planning": is_planning,
                "n_answered_so_far": n_answered,
                "run_turn": run_turn,
                "body": body,
            }
        # `turn` (the index into PLAN) follows the global request counter so
        # think-time/plan-shape behaviour stays stable; `run_turn` (the
        # index into `tool_spec`) ignores planning calls so `tool_spec [N]`
        # still lands on the N-th *run* turn.
        turn = n_answered
        think, kind = PLAN[turn - 1] if turn <= len(PLAN) else (0, "end")
        msgs = body.get("messages", [])
        if REQUEST_LOG:
            # Append under the same lock that hands out turn numbers, so a
            # scenario reading this file can trust the ordering. Includes
            # `is_planning` + `run_turn` so scenarios can derive the actual
            # mapping without trusting the mock's local counter.
            with _lock, open(REQUEST_LOG, "a") as fh:
                fh.write(json.dumps(log_entry) + "\n")
        role = "planning" if is_planning else "run"
        log(f"turn #{turn} ({role}, run #{run_turn}) ({describe(msgs)}) -> thinking {think}s, then {kind}")
        time.sleep(think)

        if not body.get("stream"):
            content = [{"type": "text", "text": f"mock turn {turn}"}]
            if kind == "tool" and not is_planning:
                spec = spec_for(run_turn)
                content.append(
                    {
                        "type": "tool_use",
                        "id": f"toolu_{turn}",
                        "name": spec["name"],
                        "input": spec["input"],
                    }
                )
            payload = {
                "id": f"msg_{turn}",
                "type": "message",
                "role": "assistant",
                "model": body.get("model", "qa-mock"),
                "content": content,
                "stop_reason": "tool_use" if kind == "tool" and not is_planning else "end_turn",
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
        if kind == "tool" and not is_planning:
            chunk(
                sse(
                    {
                        "type": "content_block_start",
                        "index": 1,
                        "content_block": {
                            "type": "tool_use",
                            "id": f"toolu_{turn}",
                            "name": spec_for(run_turn)["name"],
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
                            "partial_json": json.dumps(spec_for(run_turn)["input"]),
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
                        "stop_reason": "tool_use" if kind == "tool" and not is_planning else "end_turn",
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
