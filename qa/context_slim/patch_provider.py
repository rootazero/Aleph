#!/usr/bin/env python3
"""Re-point the `[providers.qa-mock]` block `qa/busy_input/patch_config.py`
wrote, for one context-slim phase.

The busy-input patch leaves exactly one provider (`qa-mock`, anthropic,
127.0.0.1) and one agent bound to it. A phase here needs a different wire or
a different host CLASS for that same provider, so this rewrites the block in
place rather than adding a second provider the agent would not use.

  --protocol P            `anthropic` | `openai`
  --base-url URL          e.g. http://api.deepseek.com (reached via HTTP_PROXY)
  --model M               the one model (also the agent's)
  --server-context-editing  enable Anthropic server-side context editing
  --context-budget-off    add `[context_budget] enabled = false` — the opt-out.
                          A generated config has no such section, and a
                          missing section is ON, so this is the only way to
                          get a run that builds no ContextBudget.
"""
import argparse
import re

ap = argparse.ArgumentParser()
ap.add_argument("path")
ap.add_argument("--protocol", required=True)
ap.add_argument("--base-url", required=True)
ap.add_argument("--model", required=True)
ap.add_argument("--server-context-editing", action="store_true")
ap.add_argument("--context-budget-off", action="store_true")
a = ap.parse_args()

src = open(a.path).read()
head = "[providers.qa-mock]"
start = src.index(head)
end = src.find("\n[", start + len(head))
end = len(src) if end < 0 else end
block = src[start:end]


def set_line(text, key, value):
    pat = re.compile(rf"^{re.escape(key)}\s*=.*$", re.M)
    line = f"{key} = {value}"
    return pat.sub(line, text) if pat.search(text) else text.rstrip("\n") + f"\n{line}\n"


block = set_line(block, "protocol", f'"{a.protocol}"')
block = set_line(block, "base_url", f'"{a.base_url}"')
block = set_line(block, "models", f'["{a.model}"]')
if a.server_context_editing:
    block = set_line(block, "server_context_editing", "true")
src = src[:start] + block + src[end:]
src = re.sub(r'^model = "qa-mock-model"$', f'model = "{a.model}"', src, flags=re.M)
if a.context_budget_off:
    if re.search(r"^\[context_budget\]", src, re.M):
        raise SystemExit("config already has [context_budget]; refusing to add a second header")
    src = src.rstrip("\n") + "\n\n[context_budget]\nenabled = false\n"
open(a.path, "w").write(src)
print(f"provider qa-mock -> {a.protocol} {a.base_url} {a.model}"
      f"{' +server_context_editing' if a.server_context_editing else ''}")
