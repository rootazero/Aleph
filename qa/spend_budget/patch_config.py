#!/usr/bin/env python3
"""Turn a freshly generated Aleph config into an inert QA daemon for the
per-principal spend budget fixture.

Three jobs, same shape as `qa/busy_input/patch_config.py` and
`qa/multiuser_audit`'s inline patch, but tailored to this fixture:

1. **Make it inert.** Drop every channel, provider, and agent the generator
   wrote (and any stray `[policies]` table — none is expected on a fresh
   generate, but dropping it keeps this idempotent), then add back exactly
   one provider (`anthropic`, pointed at the local mock — see
   `qa/busy_input/mock_anthropic.py`) and one agent whose DEFAULT model is
   deliberately UNPRICED (`qa-mock-model` prefix-matches nothing in
   `src/pricing.rs::PRICE_TABLE`). Assertion 11 needs an unpriced default;
   every priced call in this fixture instead pins `model_override` to
   `claude-haiku-4-5` on the SAME provider, so the pricing table's real
   `anthropic` vendor entry resolves — see `src/providers/http_provider.rs`'s
   `serving_provider_hint`: the provider's CONFIG KEY (not its base_url) is
   what the price table and model catalog are keyed on.

2. **Deliberately no `[policies.spend]`.** Assertion 1 needs a box that has
   never had a spend ceiling — `configured: false` AND zero `spend_ledger`
   rows after a real (priced) run. The policy is added live via
   `config.patch` after that assertion runs (assertion 7's own mechanism),
   not written into the static config.

3. **Make the member-pairing path reachable.** `gateway.host = "0.0.0.0"` +
   `allow_insecure_remote = true`, exactly as `qa/multiuser_audit/run.sh`
   documents: `resolve_connect_auth` authorises a loopback peer on its FIRST
   line, before it ever reads a bootstrap ticket, so a ticket redeemed over
   127.0.0.1 creates no device row — silently and successfully. The exposure
   is a scratch server with no real provider, no vault content, and a random
   port, for the lifetime of one run.

Also sets `[general] language = "en"` — assertion 5 needs the spend-ceiling
receipt rendered in English, and `build_run_request` stamps `metadata["locale"]`
from this key FRESH on every run (see `src/gateway/i18n.rs`'s module doc: this
reader is more live than the `INSTALLED_LOCALE` `OnceLock` promises), so this
takes effect without any live-reload step of its own.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument("--gateway-port", required=True)
p.add_argument("--mock-port", required=True)
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


src = drop_sections(
    src, lambda s: s.startswith(("channels", "providers", "agents", "policies"))
)


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
    ("gateway", "port", args.gateway_port),
    ("gateway", "host", '"0.0.0.0"'),
    ("gateway", "allow_insecure_remote", "true"),
    ("general", "language", '"en"'),
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
[providers.anthropic]
enabled = true
protocol = "anthropic"
base_url = "http://127.0.0.1:{args.mock_port}"
api_key = "qa-dummy-not-a-real-key"
models = ["qa-mock-model", "claude-haiku-4-5"]
timeout_seconds = 600
stream_idle_timeout_secs = 0

[[agents.list]]
id = "main"
name = "QA Main"
default = true
model = "qa-mock-model"
provider = "anthropic"
system_prompt = "QA fixture."
"""

open(args.path, "w").write(src)
print(f"patched {args.path}: gateway {args.gateway_port}, mock {args.mock_port}")
