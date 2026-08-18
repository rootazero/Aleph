#!/usr/bin/env python3
"""Assertions for the Lark failure paths — the half a happy-path mock cannot reach.

Runs against the server `run.sh` already booted, after `drive_channels.py` has
established that the plain round trip works. Everything here is driven by
`mock_lark.py`'s `/__inject` queue: the mock answers a bounded number of calls
with a canned failure and then goes back to answering normally, which is what
lets one assertion span "rejected twice, then accepted". A mock that always
failed could only ever show that the client gives up.

Why this exists at all: `feishu_inbound/crypto.rs` recorded, in as many words,
that "how the client behaves against a live 429 or a live permission error" was
not covered and could not be without an app credential. That was true of a real
credential and false of a controllable far end — and the uncovered half was
where the client was wrong.

Usage:
  drive_lark_errors.py <lark-base> <feishu-webhook-url> <verification-token>
                       <log-dir>
"""
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

LARK, HOOK, TOKEN, LOGDIR = sys.argv[1:5]

MESSAGES = "/open-apis/im/v1/messages"
FAILURES = []
T0 = time.monotonic()


def check(name, ok, evidence):
    tag = "PASS" if ok else "FAIL"
    print(f"[{tag}] {name}\n       {evidence}", flush=True)
    if not ok:
        FAILURES.append(name)


def observations():
    with urllib.request.urlopen(f"{LARK}/__observations", timeout=5) as r:
        raw = r.read().decode()
    return [json.loads(l) for l in raw.splitlines() if l.strip()]


def sends_since(mark):
    """POSTs the channel made to the send endpoint after observation `mark`."""
    return [o for o in observations()[mark:] if o["path"] == MESSAGES]


def inject(directives):
    body = json.dumps(directives).encode()
    req = urllib.request.Request(
        f"{LARK}/__inject",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=5) as r:
        return json.loads(r.read().decode())


def pending():
    with urllib.request.urlopen(f"{LARK}/__pending", timeout=5) as r:
        return json.loads(r.read().decode())


def reset():
    """Clear the queue and return whatever was still in it.

    Every case ends with this, and its return value is that case's own
    evidence. Leaving a leftover for the next case to trip over is how one
    regression prints as three — the mutation run that proved this file can go
    red showed exactly that: case 1 gave up after a single throttle and the
    unserved second one was then eaten by case 3's message.
    """
    req = urllib.request.Request(f"{LARK}/__reset", data=b"", method="POST")
    with urllib.request.urlopen(req, timeout=5) as r:
        return json.loads(r.read().decode())


def file_log_text():
    d = Path(LOGDIR)
    if not d.is_dir():
        return ""
    return "".join(
        p.read_text(errors="replace") for p in sorted(d.glob("aleph-server.log*"))
    )


_seq = [0]


def send_event(text):
    """Push one signed inbound message and return the observation mark before it."""
    _seq[0] += 1
    n = _seq[0]
    mark = len(observations())
    event = {
        "schema": "2.0",
        "header": {
            "event_id": f"qa-err-evt-{n}",
            "event_type": "im.message.receive_v1",
            "token": TOKEN,
            "create_time": str(int(time.time() * 1000)),
        },
        "event": {
            "sender": {"sender_id": {"open_id": "ou_qa_human"}, "sender_type": "user"},
            "message": {
                "message_id": f"om_qa_err_{n}",
                "chat_id": "oc_qa_group",
                "chat_type": "group",
                "message_type": "text",
                "create_time": str(int(time.time() * 1000)),
                "content": json.dumps({"text": text}),
            },
        },
    }
    req = urllib.request.Request(
        HOOK,
        data=json.dumps(event).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        urllib.request.urlopen(req, timeout=10).read()
    except urllib.error.HTTPError as e:  # the body is the evidence, not a crash
        e.read()
    return mark


def wait_for_sends(mark, want, timeout=60):
    """Block until `want` send attempts have landed after `mark`, or time out."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        got = sends_since(mark)
        if len(got) >= want:
            return got
        time.sleep(0.25)
    return sends_since(mark)


def drain(seconds=6):
    """Let any in-flight retries land before the next case queues its own."""
    time.sleep(seconds)


# ── 1-2. Lark's legacy throttle shape is retried, not dropped ─────────────
#
# The shape: `HTTP 400` + `code: 99991400` + `x-ogw-ratelimit-reset`. Lark's
# modern gateway answers 429, a documented set of legacy OpenAPI endpoints
# answers 400, and both carry the code — so a classifier that reads only the
# status sees the second one as a generic failure. Downstream that is
# `ChannelError::SendFailed`, which `channel_registry::send` never retries and
# `delivery_queue::should_enqueue` never re-enqueues: the reply is dropped and
# nothing above the channel is told. This case is the one that was RED.
THROTTLE = {
    "path": MESSAGES,
    "times": 2,
    "status": 400,
    "headers": {"x-ogw-ratelimit-reset": "1"},
    "body": {"code": 99991400, "msg": "request trigger frequency limit"},
}
inject([THROTTLE])
t_start = time.monotonic()
mark = send_event("throttle me")
got = wait_for_sends(mark, 3)
throttled = [o for o in got if o.get("injected_status") == 400]
accepted = [o for o in got if "injected_status" not in o]
elapsed = time.monotonic() - t_start

left = reset()
check(
    "a legacy-shaped throttle (400 + 99991400) is retried and the reply lands",
    len(throttled) == 2 and len(accepted) >= 1 and not left,
    f"{len(throttled)} throttled + {len(accepted)} accepted attempt(s) at {MESSAGES}; "
    f"unserved={left or 'none'}. Two throttled and none accepted means the "
    f"client gave up — the classifier read only the HTTP status.",
)

# The wait itself is only observable as elapsed time, so the assertion is a
# band rather than a value. It separates the two implementations cleanly:
# honouring `x-ogw-ratelimit-reset: 1` twice is ~2s, while the hard-coded
# fallback this replaced (5s, because it read `retry-after`, which Lark does
# not send) is ~10s. Anything under ~1s means no back-off happened at all.
check(
    "the back-off came from Lark's own x-ogw-ratelimit-reset, not the fallback",
    len(accepted) >= 1 and 1.0 <= elapsed <= 8.0,
    f"round trip took {elapsed:.1f}s for 2 retries at 1s each; "
    f"<1s = no wait, >8s = the 5s fallback was used twice",
)

drain()

# ── 3. a refusal is diagnosable ───────────────────────────────────────────
#
# A 403 whose body is the gateway's HTML used to surface as `Send response
# parse failed: error decoding response body` — a sentence that names neither
# the status nor the endpoint, and that reads identically for an expired app
# secret, a proxy 502 and a genuinely malformed payload.
inject([{
    "path": MESSAGES,
    "times": 1,
    "status": 403,
    "raw_body": "<html><head><title>403</title></head><body>Forbidden</body></html>",
}])
mark = send_event("refuse me")
got = wait_for_sends(mark, 1)
refused = [o for o in got if o.get("injected_status") == 403]
time.sleep(2)
left_403 = reset()
log = file_log_text()

check(
    "a 403 with a non-JSON body reports the status",
    "HTTP 403" in log,
    "looked for `HTTP 403` in the server log; without it the operator sees only "
    "`error decoding response body` and cannot tell a refusal from a bad proxy",
)

# ── 4. and it is not mistaken for a throttle ──────────────────────────────
#
# The widening in case 1 has a false-positive direction: if "any 4xx" had been
# taken for a throttle, a permission error would burn the whole retry budget
# against a wall and delay every reply behind it.
check(
    "a 403 is not retried — a refusal is not a throttle",
    len(refused) == 1 and not left_403,
    f"{len(refused)} attempt(s) answered 403, unserved={left_403 or 'none'}; more "
    f"than one attempt means the widened predicate swallowed a permanent error, "
    f"and an unserved directive means the send never happened at all",
)

drain()

# ── 5. a bare 400 is still terminal ───────────────────────────────────────
#
# The narrow half of the same predicate, on a live wire. `400` is the generic
# bad-request status; treating it alone as a throttle would retry malformed
# calls until the budget ran out and report a rate limit that never happened.
# The in-process test asserts this about the function; this asserts it about
# the channel.
inject([{
    "path": MESSAGES,
    "times": 1,
    "status": 400,
    "body": {"code": 230020, "msg": "bot is not in the chat"},
}])
mark = send_event("reject me")
got = wait_for_sends(mark, 1)
rejected = [o for o in got if o.get("injected_status") == 400]
time.sleep(2)
left_400 = reset()

check(
    "a 400 without the rate-limit code is terminal, not retried",
    len(rejected) == 1 and not left_400,
    f"{len(rejected)} attempt(s) answered a plain 400, unserved={left_400 or 'none'}; "
    f"more than one attempt means the predicate widened to the status instead of "
    f"to Lark's code",
)

# ── 6. the fixture's own control ──────────────────────────────────────────
#
# Every case above reads "an injection fired", and each now clears its own
# queue, so "unserved" is folded into the case it belongs to. What is left to
# check is the mock itself: that a directive queued right now is still visible
# a moment later. If `/__inject` silently dropped what it was given — a typo'd
# key, a handler that never consulted the queue — every count above would be
# zero and several checks would read as passes for the wrong reason.
inject([{"path": "/__never_called", "times": 1, "status": 500}])
queued = pending()
check(
    "the mock really queues what it is handed",
    queued.get("/__never_called") == 1,
    f"queued a directive for a path nothing calls and read back {queued}; "
    f"an empty answer means the cases above asserted against a mock that was "
    f"answering normally the whole time",
)
reset()

print(f"\n{len(FAILURES)} failure(s) in {time.monotonic() - T0:.1f}s"
      + (": " + ", ".join(FAILURES) if FAILURES else ""))
sys.exit(len(FAILURES))
