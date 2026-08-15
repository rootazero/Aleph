#!/usr/bin/env python3
"""Deterministic Anthropic-protocol stub for the background-bash announce QA.

A third job, distinct from the two stubs already here:

  * `busy_input/mock_anthropic.py` scripts **timing** — when a turn commits.
  * `plan_handoff/mock_plan.py` scripts **which tool** and records the tool
    surface the server sent.

This one scripts a tool call whose *effect outlives the run that made it*, and
records what the model was handed on every later turn. That recording is the
whole oracle: the claim under test is "a background job that finishes after its
run ended still reaches somebody", and the only place that is observable is a
provider request that **nobody's client asked for**.

Turn counting follows `mock_plan.py`'s rule, and for the same reason: the turn
counter advances only for requests that carry a tool surface. A run makes
side-channel provider calls (titling, compaction, summarisation) with no tools,
and a plan indexed by "nth HTTP request" desyncs the moment one fires.

`$LAST_PROCESS_ID` in a tool input is replaced by the process id parsed out of
the most recent `tool_result` in the incoming conversation (`bash`'s spawn
receipt reads "Started background process N"). It is substituted as a NUMBER:
`BashArgs::process_id` is `Option<u64>`, and a string there is a validation
error, not a poll.

Usage:  mock_announce.py PORT PLAN_NAME OBSERVATIONS_PATH [SLEEP_SECS]
"""
import json
import re
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
PLAN_NAME = sys.argv[2] if len(sys.argv) > 2 else "outlive"
OBS_PATH = sys.argv[3] if len(sys.argv) > 3 else "/tmp/announce_observations.jsonl"
SLEEP_SECS = int(sys.argv[4]) if len(sys.argv) > 4 else 12

# A marker distinctive enough that finding it in a later request proves the
# job's OUTPUT travelled, not merely that some turn happened.
MARKER = "QA_ANNOUNCE_MARKER_7f3a"

SPAWN = (
    "bash",
    {"cmd": f"sleep {SLEEP_SECS}; echo {MARKER}", "background": True},
)
# Terminal collection: `wait` blocks to completion and stamps
# `ProcessRegistry::is_reported`, which is the announce's dedup predicate.
# `poll` on a still-running job is NOT terminal and would not stamp it.
WAIT_LAST = ("bash", {"process_action": "wait", "process_id": "$LAST_PROCESS_ID"})
# A harmless read that keeps a run alive without touching the job.
IDLE_TOOL = ("bash", {"cmd": "true"})
# Same-process CONTROL, run before every background spawn. Without it, "the
# background job failed" and "bash cannot run here at all" are the same
# observation — and this fixture's first run could not tell them apart.
FG_PROBE = ("bash", {"cmd": f"echo {MARKER}_FG"})

PLANS = {
    # The flagship: the job outlives its run. Turn 1 spawns, turn 2 ends the
    # run, and the job has SLEEP_SECS still to go. Anything after that is a
    # turn nobody's client asked for.
    "outlive": [(0, FG_PROBE), (0, SPAWN), (0, "end")],
    # The dedup claim: the same job, collected by the model itself. A turn
    # spent re-stating a result already folded into the context is exactly the
    # cost `already_delivered` exists to avoid.
    "collected": [(0, FG_PROBE), (0, SPAWN), (0, WAIT_LAST), (0, "end")],
    # The mid-run claim: the run is still alive when the job finishes, so the
    # notice must be absorbed into it at the next turn boundary rather than
    # opening a second run.
    "midrun": [(0, FG_PROBE), (0, SPAWN), (SLEEP_SECS + 8, IDLE_TOOL), (0, "end")],
}
PLAN = PLANS.get(PLAN_NAME, PLANS["outlive"])

T0 = time.monotonic()
_turn = [0]
_lock = threading.Lock()


def log(*a):
    print(f"{time.monotonic() - T0:7.2f}s [mock]", *a, flush=True)


def sse(payload):
    return f"event: {payload['type']}\ndata: {json.dumps(payload)}\n\n".encode()


def text_blocks(msg):
    """Every text-ish string in one message, tool_result payloads included."""
    out = []
    content = msg.get("content")
    if isinstance(content, str):
        return [content]
    if isinstance(content, list):
        for b in content:
            if not isinstance(b, dict):
                continue
            if isinstance(b.get("text"), str):
                out.append(b["text"])
            inner = b.get("content")
            if isinstance(inner, str):
                out.append(inner)
            elif isinstance(inner, list):
                for ib in inner:
                    if isinstance(ib, dict) and isinstance(ib.get("text"), str):
                        out.append(ib["text"])
    return out


def last_process_id(msgs):
    """The newest background process id the conversation mentions, or None."""
    for m in reversed(msgs):
        for t in reversed(text_blocks(m)):
            hit = re.findall(r"[Bb]ackground process (\d+)", t)
            if hit:
                return int(hit[-1])
    return None


def resolve(spec_input, msgs):
    """Substitute `$LAST_PROCESS_ID` (as a number) into a tool input."""
    out = {}
    for k, v in spec_input.items():
        if v == "$LAST_PROCESS_ID":
            pid = last_process_id(msgs)
            if pid is None:
                # Answering with a string here would produce a validation
                # error that reads like a product bug. Say so instead.
                log("WARNING: no process id in the conversation to substitute")
                out[k] = 0
            else:
                out[k] = pid
        else:
            out[k] = v
    return out


def last_user_text(msgs):
    """The newest user text that is not the harness's trailing reminder.

    The harness appends a `<system-reminder>` as its own user message, so
    "the last user message" is that reminder on every single turn — the first
    version of this oracle read it and reported `announce=no` for a request
    that was carrying the announce three messages up.
    """
    for m in reversed(msgs):
        if m.get("role") != "user":
            continue
        t = " ".join(text_blocks(m)).strip()
        if t.startswith("<system-reminder>"):
            continue
        return t
    return ""


def whole_conversation(msgs):
    """Every text byte in the request — what the model can actually see."""
    return "\n".join(t for m in msgs for t in text_blocks(m))


def observe(record):
    with _lock, open(OBS_PATH, "a") as fh:
        fh.write(json.dumps(record) + "\n")


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def do_POST(self):
        try:
            self._do_post()
        except (BrokenPipeError, ConnectionResetError):
            log("client disconnected mid-stream (run cancelled)")

    def _do_post(self):
        n = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        msgs = body.get("messages", [])
        has_tools = bool(body.get("tools"))

        if not has_tools:
            # Side-channel call (titling / compaction / summarisation). It is
            # not a turn; answering it as one would desync every plan index.
            observe({"kind": "side-channel", "messages": len(msgs)})
            log("side-channel request (no tool surface) -> plain text")
            self._answer(body, turn=0, kind="end", tool=None)
            return

        with _lock:
            _turn[0] += 1
            turn = _turn[0]
        think, action = PLAN[turn - 1] if turn <= len(PLAN) else (0, "end")

        text = last_user_text(msgs)
        # Membership is asked of the WHOLE request, never of one message: the
        # announce arrives as a user message that later turns keep carrying,
        # and a tool_result is not a "user text" at all.
        whole = whole_conversation(msgs)
        observe(
            {
                "kind": "turn",
                "turn": turn,
                "messages": len(msgs),
                "last_user_text": text[:400],
                "carries_announce": "[system] Background process" in whole,
                "carries_marker": MARKER in whole,
                "fg_control_ok": f"{MARKER}_FG" in whole,
                "tool_results": [t[:300] for m in msgs for t in text_blocks(m)
                                 if "exit_code" in t or "Capability denied" in t][-2:],
            }
        )
        log(
            f"turn #{turn}: {len(msgs)} messages, "
            f"announce={'YES' if '[system] Background process' in whole else 'no'}, "
            f"last user text {text[:90]!r} -> think {think}s then "
            f"{action if isinstance(action, str) else action[0]}"
        )
        time.sleep(think)

        tool = None
        if action != "end":
            name, spec_input = action
            tool = (name, resolve(spec_input, msgs))
        self._answer(body, turn, "tool" if tool else "end", tool)

    def _answer(self, body, turn, kind, tool):
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
            "usage": {"input_tokens": 10, "output_tokens": 10},
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
        idx = 0
        chunk(
            sse(
                {
                    "type": "content_block_start",
                    "index": idx,
                    "content_block": {"type": "text", "text": ""},
                }
            )
        )
        chunk(
            sse(
                {
                    "type": "content_block_delta",
                    "index": idx,
                    "delta": {"type": "text_delta", "text": f"mock turn {turn}"},
                }
            )
        )
        chunk(sse({"type": "content_block_stop", "index": idx}))
        if tool:
            idx += 1
            chunk(
                sse(
                    {
                        "type": "content_block_start",
                        "index": idx,
                        "content_block": {
                            "type": "tool_use",
                            "id": payload["content"][1]["id"],
                            "name": tool[0],
                            "input": {},
                        },
                    }
                )
            )
            chunk(
                sse(
                    {
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": {
                            "type": "input_json_delta",
                            "partial_json": json.dumps(tool[1]),
                        },
                    }
                )
            )
            chunk(sse({"type": "content_block_stop", "index": idx}))
        chunk(
            sse(
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "tool_use" if tool else "end_turn"},
                    "usage": {"output_tokens": 10},
                }
            )
        )
        chunk(sse({"type": "message_stop"}))
        chunk(b"")


if __name__ == "__main__":
    open(OBS_PATH, "w").close()
    log(f"plan={PLAN_NAME} sleep={SLEEP_SECS}s obs={OBS_PATH}")
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
