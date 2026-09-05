#!/usr/bin/env python3
"""Rewrite `[search]` so the stack talks to this fixture's mocks and nothing else.

Every existing `[search…]` table is dropped first rather than edited. A
generated config already carries `[search]`, and appending a second header of
the same name is `duplicate key 'search'` — the server then refuses to boot
*after* printing its banner, which reads like the fixture started on the wrong
port rather than like a config error.

`web_fetch_fallback` is off for every phase. It is on by default in
production, and it is the one branch that would take a phase off this machine:
"every backend came back empty" is exactly when it scrapes DuckDuckGo, and the
`empty` phase manufactures that state on purpose.

`min_request_interval_ms = 0` because the provider otherwise spaces requests
2s apart — tuned for a real instance's upstream engines, and pure latency
against a mock that has none.

`[ssrf] allow_private_network = true` because every mock binds 127.0.0.1, and
the construction-time SSRF check on a provider's `base_url` refuses
loopback/private targets unless the operator opts in. This is the operator
switch, set the way a real self-hosted LAN deployment sets it — the fixture
proves the wiring works under the configuration that makes it legal, not by
disabling the check. Any pre-existing `[ssrf]` table is dropped first, same
reason as `[search]`: a duplicated header is a config error at load.
"""
import argparse
import re

p = argparse.ArgumentParser()
p.add_argument("path")
p.add_argument(
    "--searxng",
    action="append",
    default=[],
    metavar="NAME=PORT",
    help="a mock SearXNG backend, named NAME, on 127.0.0.1:PORT",
)
p.add_argument(
    "--exa",
    metavar="NAME",
    help="an Exa backend with a deliberately invalid key. It is the only "
    "backend in reach that declares domain_filter and does not need a "
    "credential we have, so it is how the ordering claim gets a candidate "
    "that outranks searxng. It is expected to fail: the claim is about who "
    "was ASKED first, which the answer's notes report.",
)
p.add_argument("--default", required=True, help="default_provider")
p.add_argument("--fallback", action="append", default=[])
p.add_argument("--max-results", default="5")
p.add_argument("--timeout-seconds", default="10")
args = p.parse_args()

src = open(args.path, encoding="utf-8").read()

# Drop every `[search]` / `[search.*]` / `[ssrf]` / `[ssrf.*]` table, header
# through last line before the next top-level header.
out, dropping = [], False
for line in src.splitlines(keepends=True):
    header = re.match(r"\s*\[+([^\]]+)\]", line)
    if header:
        name = header.group(1)
        dropping = (
            name == "search"
            or name.startswith("search.")
            or name == "ssrf"
            or name.startswith("ssrf.")
        )
    if not dropping:
        out.append(line)
src = "".join(out).rstrip() + "\n"

fallbacks = ", ".join(f'"{f}"' for f in args.fallback)
block = [
    "",
    "[ssrf]",
    "allow_private_network = true",
    "",
    "[search]",
    "enabled = true",
    f'default_provider = "{args.default}"',
    f"fallback_providers = [{fallbacks}]",
    f"max_results = {args.max_results}",
    f"timeout_seconds = {args.timeout_seconds}",
    "web_fetch_fallback = false",
]

for spec in args.searxng:
    name, port = spec.split("=", 1)
    block += [
        "",
        f"[search.backends.{name}]",
        'provider_type = "searxng"',
        f'base_url = "http://127.0.0.1:{port}"',
        # Required since the boot-time SSRF host check landed: without it
        # every backend here is refused at construction, `from_config` reports
        # "no provider was constructable", and the whole fixture silently
        # falls back to the TAVILY_API_KEY env var. The mock IS a private
        # upstream, so saying so is the honest config, not a workaround.
        "allow_private_upstream = true",
        "min_request_interval_ms = 0",
    ]

if args.exa:
    block += [
        "",
        f"[search.backends.{args.exa}]",
        'provider_type = "exa"',
        'api_key = "qa-invalid-exa-key"',
    ]

open(args.path, "w", encoding="utf-8").write(src + "\n".join(block) + "\n")
print(f"[search] rewritten: default={args.default} fallbacks={args.fallback}")
