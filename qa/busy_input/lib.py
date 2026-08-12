"""Shared helpers for the busy-input real-machine QA scenarios.

Everything here exists to serve one rule, learned the expensive way in
§4.8 Round-9:

    THE SESSION LOG IS THE CLOCK, NOT THE WALL.

`count_pending_steering` reads the session event log, so a scenario that
paces itself off `time.sleep` is pacing off something the code under test
cannot see. The first version of the Round-9 driver sent both steers within
the window where the log still held no assistant turn at all (pending = 0),
so it never reached the backpressure branch — and it looked completely
healthy while doing it, because both messages *were* accepted.

Anything that needs to happen "after the model has answered once" must wait
on `SessionLog.wait_for("assistant_message", ...)`, never on a duration.
"""

import asyncio
import hashlib
import hmac
import json
import sqlite3
import time
import urllib.error
import urllib.request

T0 = time.monotonic()


def log(*a):
    """Timestamped progress line, offset from process start."""
    print(f"{time.monotonic() - T0:7.2f}s", *a, flush=True)


class SessionLog:
    """Read-only view of `sessions.db`, the authoritative clock.

    Opened read-only per query rather than held open: the server is writing
    to this database concurrently, and a long-lived reader is a good way to
    collect `SQLITE_BUSY` at exactly the wrong moment.
    """

    def __init__(self, path, session_id=None):
        self.path = path
        self.session_id = session_id

    def _query(self, sql, args=()):
        try:
            con = sqlite3.connect(f"file:{self.path}?mode=ro", uri=True, timeout=2)
            try:
                return con.execute(sql, args).fetchall()
            finally:
                con.close()
        except sqlite3.Error:
            # The server may not have created the file yet, or may hold the
            # write lock. Both are "no answer yet", never "no such event".
            return []

    def _scope(self):
        if self.session_id:
            return " and session_id = ?", (self.session_id,)
        return "", ()

    def rows(self, event_type):
        """Committed rows of one event type, oldest first: (seq, created_at)."""
        clause, args = self._scope()
        return self._query(
            "select seq, created_at from session_events "
            f"where event_type = ?{clause} order by seq",
            (event_type,) + args,
        )

    def payloads(self, event_type):
        """Decoded payloads of one event type, oldest first."""
        clause, args = self._scope()
        rows = self._query(
            "select payload_json from session_events "
            f"where event_type = ?{clause} order by seq",
            (event_type,) + args,
        )
        out = []
        for (raw,) in rows:
            try:
                out.append(json.loads(raw))
            except json.JSONDecodeError:
                pass
        return out

    def sessions(self):
        """Every session id that has events, oldest first by first event."""
        return [
            r[0]
            for r in self._query(
                "select session_id from session_events "
                "group by session_id order by min(seq)"
            )
        ]

    def runs(self):
        """`run_id -> outcome` for finished runs; `None` outcome = still open.

        `RunStarted`/`RunFinished` are the authoritative record of what the
        engine did with each message — which is what tells an Interrupt
        apart from a Queue without guessing from timing.
        """
        out = {}
        for p in self.payloads("run_started"):
            if "run_id" in p:
                out.setdefault(p["run_id"], None)
        for p in self.payloads("run_finished"):
            if "run_id" in p:
                out[p["run_id"]] = p.get("outcome")
        return out

    async def wait_for(self, event_type, count, budget):
        """Block until `count` rows of `event_type` exist. Returns the newest
        such row, or `None` on timeout — never raises, so a scenario can
        report a specific failure instead of a traceback."""
        end = time.monotonic() + budget
        while time.monotonic() < end:
            r = self.rows(event_type)
            if len(r) >= count:
                return r[count - 1]
            await asyncio.sleep(0.2)
        return None


# --- Gateway JSON-RPC over WebSocket -----------------------------------------


async def rpc(ws, method, params, rid):
    await ws.send(
        json.dumps({"jsonrpc": "2.0", "method": method, "params": params, "id": rid})
    )


async def reply(ws, rid, budget=30):
    """Await the reply with this id, skipping the event frames interleaved
    with it (the socket carries both)."""
    end = time.monotonic() + budget
    while time.monotonic() < end:
        remaining = max(0.1, end - time.monotonic())
        m = json.loads(await asyncio.wait_for(ws.recv(), timeout=remaining))
        if m.get("id") == rid:
            return m
    raise TimeoutError(f"no reply to id {rid}")


# --- Channel inbound (generic webhook channel) -------------------------------


def webhook_signature(secret, body):
    """`X-Webhook-Signature` value: `WebhookReceiver::compute_signature`."""
    digest = hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return f"sha256={digest}"


def webhook_post(base_url, path, secret, payload, timeout=10):
    """Deliver one message through the real channel inbound path.

    This is a genuine channel arrival, not a simulation: the POST enters
    `WebhookReceiver::router()` (merged into the gateway server), is HMAC
    verified, parsed into an `InboundMessage`, and handed to the
    `InboundMessageRouter` — the same path a Telegram or Slack message
    takes once its adapter has parsed it. That is what makes per-channel
    `busy_input_mode` reachable at all: the RPC face never carries it.

    Returns `(status, body_text)`.
    """
    body = json.dumps(payload).encode()
    req = urllib.request.Request(
        f"{base_url}{path}",
        data=body,
        headers={
            "content-type": "application/json",
            "X-Webhook-Signature": webhook_signature(secret, body),
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read().decode(errors="replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(errors="replace")
    except OSError as e:
        return 0, str(e)


def channel_message(text, conversation, sender="qa-sender", message_id=None):
    """A `WebhookPayload` carrying one inbound message.

    All messages in a scenario share `conversation_id`, which is what makes
    them land on the same session — and therefore what makes the second one
    arrive while the first is still running, which is the whole point.
    """
    payload = {
        "sender_id": sender,
        "sender_name": "QA Sender",
        "message": text,
        "conversation_id": conversation,
        "is_group": False,
    }
    if message_id:
        payload["message_id"] = message_id
    return payload
