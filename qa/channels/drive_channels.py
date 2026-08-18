#!/usr/bin/env python3
"""Assertions for the channel-reachability real-machine QA.

Each check prints PASS/FAIL with the evidence it read. The exit code is the
number of failures, so `run.sh` can be used from CI.

Usage:
  drive_channels.py <server-stdout> <log-dir> <lark-base> <feishu-webhook-url>
                    <verification-token>
"""
import json
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

STDOUT, LOGDIR, LARK, HOOK, TOKEN = sys.argv[1:6]

FAILURES = []
T0 = time.monotonic()


def check(name, ok, evidence):
    tag = "PASS" if ok else "FAIL"
    print(f"[{tag}] {name}\n       {evidence}", flush=True)
    if not ok:
        FAILURES.append(name)


def stdout_text():
    return Path(STDOUT).read_text(errors="replace")


def file_log_text():
    d = Path(LOGDIR)
    if not d.is_dir():
        return ""
    return "".join(
        p.read_text(errors="replace") for p in sorted(d.glob("aleph-server.log*"))
    )


def observations():
    with urllib.request.urlopen(f"{LARK}/__observations", timeout=5) as r:
        raw = r.read().decode()
    return [json.loads(l) for l in raw.splitlines() if l.strip()]


def wait_for_observation(pred, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for obs in observations():
            if pred(obs):
                return obs
        time.sleep(0.5)
    return None


# ── 1-3. the factory table actually contains these three ──────────────────
out = stdout_text()
for chan in ("feishu", "line", "qq"):
    line = f"Registered channel: {chan} ({chan})"
    check(
        f"{chan} is constructed from [channels.{chan}]",
        line in out,
        f"looked for {line!r} in server stdout",
    )

# ── 4. the CONTROL: prove the probe can see a channel that is NOT there ───
check(
    "control: msteams is not registered",
    "Registered channel: msteams" not in out,
    "severed 2026-08-17; a line here would mean the CUT did not take",
)
flog = file_log_text()
check(
    "control: msteams is dropped *and says so*",
    "Channel 'msteams' has no 'type' field" in flog,
    "without this line the three assertions above prove nothing — an empty "
    "log and a broken probe look identical",
)
for chan in ("feishu", "line", "qq"):
    check(
        f"{chan} is not in the dropped-channel warnings",
        f"Channel '{chan}' has no 'type' field" not in flog
        and f"Failed to create channel '{chan}'" not in flog,
        "resolved_channels() kept it and the factory table answered",
    )

# ── 4b. qq got past config parsing: the flat spelling really deserialised ──
# There is no QQ mock, so `start()` must fail — but *where* it fails is the
# claim. A ConfigError would mean `QQConfig::from_wire` rejected the flat
# single-account spelling the Panel card writes; an auth failure means it
# parsed, validated and constructed, and only the far end is missing.
qq_fail = next((l for l in out.splitlines() if "Channel qq failed" in l), "")
check(
    "qq's flat spelling parsed — it fails at auth, not at config",
    "Authentication failed" in qq_fail,
    qq_fail.strip() or "no `Channel qq failed` line at all",
)

# ── 5-7. start() really dialled Lark ──────────────────────────────────────
check(
    "feishu start() succeeded against the mock",
    "✓ Channel feishu started" in out,
    "this is the half that had never run: construction was never the broken part",
)
obs = observations()
check(
    "start() fetched an app access token",
    any(o["path"] == "/open-apis/auth/v3/app_access_token/internal" for o in obs),
    f"{len(obs)} request(s) reached the mock Lark",
)
tok = next(
    (o for o in obs if o["path"] == "/open-apis/auth/v3/app_access_token/internal"),
    None,
)
check(
    "the token request carried the configured credentials",
    bool(tok) and tok["body"].get("app_id") == "cli_qa_app",
    f"body={tok['body'] if tok else None}",
)
check(
    "start() fetched bot info with that token",
    any(o["path"] == "/open-apis/bot/v3/info" for o in obs),
    "proves the token was accepted and latched, not just requested",
)

# ── 8. inbound webhook -> agent turn -> outbound back to Lark ─────────────
event = {
    "schema": "2.0",
    "header": {
        "event_id": "qa-evt-1",
        "event_type": "im.message.receive_v1",
        "token": TOKEN,
        "create_time": str(int(time.time() * 1000)),
    },
    "event": {
        "sender": {"sender_id": {"open_id": "ou_qa_human"}, "sender_type": "user"},
        "message": {
            "message_id": "om_qa_inbound_1",
            "chat_id": "oc_qa_group",
            "chat_type": "group",
            "message_type": "text",
            "create_time": str(int(time.time() * 1000)),
            "content": json.dumps({"text": "ping from qa"}),
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
    with urllib.request.urlopen(req, timeout=10) as r:
        status, body = r.status, r.read().decode()
except urllib.error.HTTPError as e:
    status, body = e.code, e.read().decode()
except Exception as e:  # noqa: BLE001 - the failure text is the evidence
    status, body = -1, repr(e)
check(
    "the feishu webhook server accepted a signed event",
    status == 200,
    f"POST {HOOK} -> {status} {body[:120]}",
)

sent = wait_for_observation(lambda o: o["path"] == "/open-apis/im/v1/messages")
check(
    "the reply travelled back out through the real Feishu send path",
    sent is not None,
    f"looked for POST /open-apis/im/v1/messages within 45s; "
    f"{'got ' + json.dumps(sent['body'])[:160] if sent else 'nothing arrived'}",
)
if sent:
    check(
        "the outbound message is addressed to the chat the event came from",
        sent["body"].get("receive_id") == "oc_qa_group",
        f"receive_id={sent['body'].get('receive_id')}",
    )

# ── 9. the streaming emitter is reachable at all ──────────────────────────
# `try_create_feishu_emitter` rebuilt `FeishuConfig` from `Config.channels`,
# where `app_secret` no longer is once the vault migration has run — so it
# returned `None` on every deployment that had ever saved a channel, and the
# streaming / typing-indicator emitter was dead. Nothing said so: the reply
# still went out through the plain path. A card create is the observable that
# separates the two.
card = wait_for_observation(lambda o: o["path"] == "/open-apis/cardkit/v1/cards", timeout=20)
check(
    "the streaming emitter was constructed (its config came from the channel)",
    card is not None,
    "looked for POST /open-apis/cardkit/v1/cards; absent means "
    "try_create_feishu_emitter returned None and nobody was told",
)

# ── 10. the inbound path reuses the channel's authenticated client ────────
# `try_create_feishu_emitter` runs once per inbound feishu message and used to
# build its own `TokenManager`, then call `refresh_token()` — the one method
# that never consults the cache. So the token count is the observable: with the
# shared handle it stays at the single request `start()` made; without it, it
# is 1 + one per message. Nothing else about the reply distinguishes the two.
tokens = [
    o for o in observations()
    if o["path"] == "/open-apis/auth/v3/app_access_token/internal"
]
check(
    "the per-message emitter reuses start()'s client instead of re-authenticating",
    len(tokens) == 1,
    f"{len(tokens)} token request(s) after a full inbound->outbound round trip; "
    "1 = the shared Arc<FeishuApi>, 2 = a second TokenManager per message",
)

print(f"\n{len(FAILURES)} failure(s) in {time.monotonic() - T0:.1f}s"
      + (": " + ", ".join(FAILURES) if FAILURES else ""))
sys.exit(len(FAILURES))
