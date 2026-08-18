#!/usr/bin/env python3
"""Configure the channel-reachability QA daemon.

Four channel blocks, each carrying a distinct claim:

  feishu   — CONNECTed 2026-08-18. `domain` points at the mock Lark, so this
             one actually starts: token, bot info, webhook server.
             `connection_mode = "webhook"` because it is the mode that can be
             driven from localhost; note the Panel's Feishu card cannot
             produce it (it offers no connection_mode / webhook_* field), so
             a Panel-configured feishu is always the websocket mode.
  line     — configurable since long before it had a Panel card. Credentials
             are placeholders and there is no mock for LINE, so the claim here
             is *construction and registration*, not a successful start.
  qq       — written in the FLAT single-account spelling that the new Panel
             card produces. This is the only place `QQConfig::from_wire` is
             exercised on the real boot path.
  msteams  — the CONTROL. Its adapter was severed 2026-08-17, so it must be
             dropped by `resolved_channels()` with a named warning. Without it
             the "no failures were logged" assertions prove nothing: an empty
             log and a working probe look identical.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--gateway-port", required=True)
p.add_argument("--mock-port", required=True)
p.add_argument("--lark-port", required=True)
p.add_argument("--feishu-webhook-port", required=True)
p.add_argument("--feishu-webhook-path", default="/feishu/events")
p.add_argument("--feishu-token", default="qa-feishu-verification-token")
# `validate()` demands an encrypt_key in webhook mode, but `resolve_payload`
# only *uses* it when the body carries an `encrypt` field — so a plaintext
# event still goes through, and the fixture does not have to implement Lark's
# AES envelope to exercise the inbound path.
p.add_argument("--feishu-encrypt-key", default="qa-feishu-encrypt-key-0123456789")
args = p.parse_args()

src = open(args.path).read()


def drop_sections(text, pred):
    out, keep = [], True
    for line in text.splitlines():
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            keep = not pred(m.group(1))
        if keep:
            out.append(line)
    return "\n".join(out) + "\n"


src = drop_sections(src, lambda s: s.startswith(("channels", "providers", "agents")))


def set_key(text, section, key, value):
    lines = text.splitlines()
    out, cur, inserted = [], None, False
    for line in lines:
        m = re.match(r"^\[+([^\]]+)\]+\s*$", line)
        if m:
            cur = m.group(1)
            out.append(line)
            if cur == section:
                out.append(f"{key} = {value}")
                inserted = True
            continue
        if cur == section and re.match(rf"^\s*{re.escape(key)}\s*=", line):
            continue
        out.append(line)
    text = "\n".join(out) + "\n"
    if not inserted:
        text += f"\n[{section}]\n{key} = {value}\n"
    return text


for section, key, value in [
    ("gateway", "host", '"127.0.0.1"'),
    ("gateway", "port", args.gateway_port),
    # Pinned rather than inherited: `drive_lark_errors.py` asserts "throttled
    # twice, then accepted", which is only the expected shape while the budget
    # is >= 2. Riding on `SendRetryPolicy::default()` would let a change there
    # turn a real regression into a fixture that quietly asserts something else.
    ("gateway.send_retry", "max_rate_limit_retries", "2"),
    ("gateway.send_retry", "max_retry_after_secs", "30"),
    ("cron", "enabled", "false"),
    ("heartbeat", "enabled", "false"),
    ("mcp", "enabled", "false"),
    ("acp", "enabled", "false"),
    ("evolution", "enabled", "false"),
    ("memory", "enabled", "false"),
    ("skills", "enabled", "false"),
]:
    src = set_key(src, section, key, value)

src += f"""
[providers.qa-mock]
enabled = true
protocol = "anthropic"
base_url = "http://127.0.0.1:{args.mock_port}"
api_key = "qa-dummy-not-a-real-key"
models = ["qa-mock-model"]
timeout_seconds = 600
stream_idle_timeout_secs = 0

[[agents.list]]
id = "main"
name = "QA Main"
default = true
model = "qa-mock-model"
provider = "qa-mock"
system_prompt = "QA fixture."

# ── feishu: the one that really starts ────────────────────────────────────
# The instance id must equal the channel type: every factory hardcodes the
# runtime channel id to the type and discards the configured instance id,
# while subsystems.rs registers policy under the instance id. See qa/README.md.
[channels.feishu]
enabled = true
app_id = "cli_qa_app"
app_secret = "qa-app-secret"
domain = "http://127.0.0.1:{args.lark_port}"
connection_mode = "webhook"
webhook_host = "127.0.0.1"
webhook_port = {args.feishu_webhook_port}
webhook_path = "{args.feishu_webhook_path}"
# Without this every webhook body is rejected: `verify_payload_token` falls
# through to `_ => false` when no token is configured.
verification_token = "{args.feishu_token}"
encrypt_key = "{args.feishu_encrypt_key}"
groups_allowed = true
require_mention = false
# Streaming ON so the run exercises `FeishuEventEmitter`. With it off the
# reply still goes out (plain ReplyEmitter -> channel -> MessageOps) and the
# emitter's reachability is unobservable — which is how it stayed broken.
streaming = true
typing_indicator = false
permission_level = "config"

# ── line: registration only (no mock for the LINE API) ────────────────────
[channels.line]
enabled = true
channel_access_token = "qa-line-token"
channel_secret = "qa-line-secret"
webhook_host = "127.0.0.1"
webhook_port = 18991
webhook_path = "/line/webhook"

# ── qq: the FLAT spelling the new Panel card writes ───────────────────────
[channels.qq]
enabled = true
app_id = "102000000"
client_secret = "qa-qq-secret"
group_policy = "mention_only"

# ── msteams: CONTROL — severed, must be dropped and say so ────────────────
[channels.msteams]
enabled = true
app_id = "qa-msteams"
"""

open(args.path, "w").write(src)
print(f"patched {args.path}: gateway {args.gateway_port}, lark mock {args.lark_port}")
