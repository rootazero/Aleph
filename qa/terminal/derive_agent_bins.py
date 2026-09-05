#!/usr/bin/env python3
"""Print `<agent label>\t<interactive executable>` for every agent the engine
knows, DERIVED from `crates/agent-detect/src/engine.rs`.

The `real` stage needs to find an installed agent on PATH and then say which
label the product should answer with. Both facts already exist, in
`agent_label` and `interactive_agent_executable`, and they disagree for two
agents (`antigravity` -> `agy`, `github-copilot` -> `copilot`) — so a hand
list in run.sh would be a second copy of a roster that is already wrong in two
places the day it is written (判据 §1).

A variant this cannot parse is simply omitted, which costs the fixture a
candidate. That direction is the safe one: fewer candidates can only end in
the stage SKIPPING, loudly, never in it asserting something false.

Usage:  derive_agent_bins.py path/to/engine.rs
"""
import re
import sys


def arms(src: str, fn: str) -> dict:
    """`Agent::X => "y"` arms of one function's `match`, including the arm
    Cursor writes as a `cfg!(windows)` block (its non-Windows string is the
    one this fixture runs under)."""
    start = src.index(f"pub fn {fn}(")
    # The next `pub fn` after this one bounds the body; the last function in
    # the file is bounded by end-of-file.
    nxt = src.find("\npub fn ", start + 1)
    body = src[start : nxt if nxt != -1 else len(src)]
    out = {}
    for variant, value in re.findall(r'Agent::(\w+) => "([^"]+)"', body):
        out.setdefault(variant, value)
    for variant, block in re.findall(r"Agent::(\w+) => \{(.*?)\n        \}", body, re.S):
        if variant in out:
            continue
        strings = re.findall(r'"([^"]+)"', block)
        # `if cfg!(windows) { "a.cmd" } else { "a" }` — take the else branch.
        if len(strings) == 2:
            out[variant] = strings[1]
    return out


def main() -> int:
    src = open(sys.argv[1], encoding="utf-8").read()
    labels = arms(src, "agent_label")
    bins = arms(src, "interactive_agent_executable")
    if not labels or not bins:
        print("could not parse engine.rs", file=sys.stderr)
        return 1
    for variant in sorted(set(labels) & set(bins)):
        print(f"{labels[variant]}\t{bins[variant]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
