#!/usr/bin/env python3
"""Turn a freshly generated Aleph config into an inert QA daemon.

Three jobs:

1. **Make it inert.** Drop every channel, provider and agent the generator
   wrote, then add back exactly one of each. A QA daemon that dials out is a
   QA daemon whose results you cannot trust.

2. **Point it at the mock.** `ProviderConfig.api_key` is `skip_serializing`
   but still *deserializes*, so an inline key in the config file is enough —
   the QA server never touches the real `~/.aleph/data/secrets.vault`, and
   the run costs nothing and reaches no network.

3. **Make the wake edge observable.** `max_pending_steering = 1` puts the
   backpressure branch one message away, and a 600 s fallback tick means any
   redelivery inside a few seconds is provably the real wake edge and not the
   safety net.

With `--channel-busy-mode`, also emits a generic webhook channel — the only
locally drivable channel inbound, and therefore the only way to exercise
per-channel `busy_input_mode` at all (the RPC face never carries it).

  ⚠️  The channel instance id is fixed to `webhook` on purpose. Every
  `register_plain_channel!` factory hardcodes its runtime channel id to the
  channel *type* and discards the configured instance id, while
  `subsystems.rs` registers the policy under the *instance* id. They only
  agree when the instance is named after its type. See qa/README.md.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--gateway-port", required=True)
p.add_argument("--mock-port", required=True)
p.add_argument("--max-pending-steering", default="1")
p.add_argument("--wake-fallback-secs", default="600")
p.add_argument(
    "--channel-busy-mode",
    choices=["steer", "interrupt", "queue"],
    help="emit a generic webhook channel with this busy_input_mode",
)
p.add_argument("--channel-secret", default="qa-webhook-secret")
p.add_argument("--channel-path", default="/webhook/generic")
p.add_argument(
    "--channel-id",
    default="webhook",
    help="instance id; anything but `webhook` proves the id-divergence bug",
)
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
            continue  # replaced by the line inserted at the header
        out.append(line)
    text = "\n".join(out) + "\n"
    if not inserted:
        text += f"\n[{section}]\n{key} = {value}\n"
    return text


for section, key, value in [
    ("gateway", "host", '"127.0.0.1"'),
    ("gateway", "port", args.gateway_port),
    ("execution", "max_pending_steering", args.max_pending_steering),
    ("execution", "busy_queue_wake_fallback_secs", args.wake_fallback_secs),
    ("execution", "busy_queue_max_wait_secs", "1800"),
    ("execution", "mid_turn_steering", "true"),
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
"""

if args.channel_busy_mode:
    # `Config.channels` is a MAP keyed by instance id, not an array of tables —
    # the `[[channels]] id = ...` form in `webhook/mod.rs`'s doc comment does
    # not parse ("invalid type: sequence, expected a map"). The map KEY is the
    # instance id, and the flat policy keys sit alongside the channel's own
    # config keys in the same table.
    src += f"""
[channels.{args.channel_id}]
type = "webhook"
enabled = true
secret = "{args.channel_secret}"
callback_url = "http://127.0.0.1:1/qa-sink"
path = "{args.channel_path}"
allowed_senders = []
busy_input_mode = "{args.channel_busy_mode}"
permission_level = "config"
"""

open(args.path, "w").write(src)
print(
    f"patched {args.path}: gateway {args.gateway_port}, mock {args.mock_port}, "
    f"channel {args.channel_busy_mode or 'none'}"
)
