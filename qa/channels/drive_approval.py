#!/usr/bin/env python3
"""Phase 3: the notify-and-wait ruling on the CHANNEL leg.

The 2026-08-28 ruling ("不要使用超时，应该使用通知+永久等待") was verified live on
the Panel leg only. That leg parks in `OperatorApprovalRequester`; a channel turn
parks somewhere else entirely — `ChannelApprovalBridgeAdapter`, which delivers the
card through the real Feishu send path and waits on a different call site. Both
read the timeout from one function, so the *decision* is shared; nothing had ever
shown that the channel leg's `await_registered` actually receives it, that the
card survives the old 120 s deadline out here, or that a human on the channel can
still answer one that has.

Nine checks, in the order the failures matter:

  1.   the gated turn actually entered through the channel
  2.   a channel turn parked on an approval at all
  3-4. the card carries the no-expiry sentinel, AND the field is really on the
       wire — an absent key would satisfy `== 0` just as well, and this phase
       exists to avoid exactly that kind of green
  5.   the card reached the human: a real POST to Feishu, not just a record
  6-7. THE REGRESSION: still parked well past 120 s, with nothing in the log
       having timed it out
  8-9. a human on the channel can still answer a card that outlived the old
       deadline, and the answer lands

Exit code is the number of failures.

Usage:
  drive_approval.py <ws-url> <lark-base> <feishu-webhook-url> <token> <log-dir>
"""
import asyncio
import json
import sys
import time
import urllib.request
from pathlib import Path

import websockets

WS, LARK, HOOK, TOKEN, LOGDIR = sys.argv[1:6]

# Comfortably past `DEFAULT_APPROVAL_TIMEOUT_MS` (120 s) — the deadline this
# phase exists to prove is gone. Sampled rather than slept through, so the
# failure says WHEN the card vanished instead of only that it did.
PAST_OLD_DEADLINE_SECS = 150

FAILURES = []
T0 = time.monotonic()


def check(name, ok, evidence):
    tag = "PASS" if ok else "FAIL"
    print(f"{time.monotonic() - T0:7.2f}s [{tag}] {name}\n       {evidence}", flush=True)
    if not ok:
        FAILURES.append(name)


def observations():
    with urllib.request.urlopen(f"{LARK}/__observations", timeout=5) as r:
        raw = r.read().decode()
    return [json.loads(l) for l in raw.splitlines() if l.strip()]


def server_log():
    d = Path(LOGDIR)
    if not d.is_dir():
        return ""
    return "".join(
        p.read_text(errors="replace") for p in sorted(d.glob("aleph-server.log*"))
    )


def feishu_message(text, message_id):
    """One inbound Feishu message through the real webhook path."""
    event = {
        "schema": "2.0",
        "header": {
            "event_id": f"qa-evt-{message_id}",
            "event_type": "im.message.receive_v1",
            "token": TOKEN,
            "create_time": str(int(time.time() * 1000)),
        },
        "event": {
            "sender": {
                "sender_id": {"open_id": "ou_qa_human"},
                "sender_type": "user",
            },
            "message": {
                "message_id": f"om_{message_id}",
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
    with urllib.request.urlopen(req, timeout=10) as r:
        return r.status


_rid = [1]


def next_id():
    _rid[0] += 1
    return _rid[0]


async def rpc(ws, method, params, rid):
    await ws.send(
        json.dumps({"jsonrpc": "2.0", "method": method, "params": params, "id": rid})
    )
    end = time.monotonic() + 30
    while time.monotonic() < end:
        m = json.loads(
            await asyncio.wait_for(ws.recv(), timeout=max(0.1, end - time.monotonic()))
        )
        if m.get("id") == rid:
            return m
    raise TimeoutError(f"no reply to {method}")


async def pending(ws, rid):
    r = await rpc(ws, "exec.approvals.pending", {}, rid)
    return r.get("result", {}).get("pending", [])


async def main():
    async with websockets.connect(WS, max_size=None) as ws:
        await rpc(ws, "connect", {"client_info": {"name": "qa-approval"}}, 1)

        # A channel turn whose tool call is behind the confirmation gate
        # (`[policies.tool_permissions] file_read = "ask"`).
        check(
            "the feishu webhook accepted the message that triggers the gate",
            feishu_message("please read something", "approval_1") == 200,
            f"POST {HOOK}",
        )

        card = None
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline and card is None:
            parked = await pending(ws, next_id())
            card = parked[0] if parked else None
            if card is None:
                await asyncio.sleep(1)

        check(
            "a channel turn parked on an approval",
            card is not None,
            "polled exec.approvals.pending for 90s; "
            + (json.dumps(card["record"])[:200] if card else "nothing parked"),
        )
        if card is None:
            return len(FAILURES)

        record = card["record"]
        approval_id = record["id"]

        # The sentinel must be PRESENT, not merely falsy. `record.get(k, 0) == 0`
        # would pass just as well against a wire that never carried the field —
        # the exact shape of vacuous assertion this whole phase exists to avoid.
        check(
            "the no-expiry sentinel is on the wire, not absent-and-defaulted",
            "expires_at_ms" in record and "created_at_ms" in record,
            f"record keys: {sorted(record)}",
        )
        check(
            "a channel-routed card carries the no-expiry sentinel (expires_at_ms == 0)",
            record.get("expires_at_ms") == 0 and record.get("created_at_ms", 0) > 0,
            f"expires_at_ms={record.get('expires_at_ms')!r} "
            f"created_at_ms={record.get('created_at_ms')!r} "
            "— a real record with no deadline, not an empty one",
        )

        # The card must have REACHED the human. A pending record proves the gate
        # fired; only an outbound POST proves anybody was told.
        sent = [
            o
            for o in observations()
            if o["path"] in ("/open-apis/im/v1/messages", "/open-apis/cardkit/v1/cards")
        ]
        check(
            "the approval prompt travelled out through the real Feishu send path",
            bool(sent),
            f"{len(sent)} outbound call(s); last={json.dumps(sent[-1]['body'])[:200] if sent else 'none'}",
        )

        # ── THE REGRESSION ────────────────────────────────────────────────
        # Under the old 120 s deadline this card is gone by now and the tool
        # call has failed with "nobody answered".
        waited = 0
        alive_at = []
        while waited < PAST_OLD_DEADLINE_SECS:
            await asyncio.sleep(10)
            waited += 10
            ids = [p["record"]["id"] for p in await pending(ws, next_id())]
            if approval_id in ids:
                alive_at.append(waited)
        check(
            f"the card is STILL parked {PAST_OLD_DEADLINE_SECS}s later "
            "(past the old 120s deadline)",
            alive_at and alive_at[-1] >= PAST_OLD_DEADLINE_SECS,
            f"alive at t+{alive_at}s; the old deadline would have retired it at t+120s",
        )
        log = server_log()
        check(
            "nothing timed out the parked approval",
            "Approval timed out" not in log,
            "searched the server log for the manager's timeout line",
        )

        # ── the human on the channel answers ──────────────────────────────
        # Through the channel, not through `exec.approval.resolve`: the RPC face
        # is the Panel's answer path and was already covered. What was never
        # shown is that a card which outlived the deadline is still answerable
        # by the person it was delivered to.
        check(
            "the human replied /approve on the channel",
            feishu_message("/approve", "approval_2") == 200,
            f"POST {HOOK}",
        )
        gone = False
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            ids = [p["record"]["id"] for p in await pending(ws, next_id())]
            if approval_id not in ids:
                gone = True
                break
            await asyncio.sleep(1)
        check(
            "the channel reply resolved the card that had been parked past the deadline",
            gone,
            f"approval {approval_id} left the pending set"
            if gone
            else f"approval {approval_id} still pending 60s after /approve",
        )

    return len(FAILURES)


if __name__ == "__main__":
    rc = asyncio.run(main())
    print(f"\n{len(FAILURES)} failure(s): {FAILURES}" if FAILURES else "\nall checks passed")
    sys.exit(rc)
