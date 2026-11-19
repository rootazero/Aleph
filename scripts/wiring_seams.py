#!/usr/bin/env python3
"""Severed-wire guard for the audited protocol/agent modules.

A wire is reachable only when a symbol is BOTH defined (produced on side A)
AND consumed (called on side B). A public API nobody calls is a severed wire:
the code exists, may even have tests, but its far end is never reached.
`dead_code` lints cannot see this — the producer is not dead, the CONSUMER is
missing, and the producer is `pub` so the compiler stays quiet.

This is the form-1 detector of the severed-wire audit (2026-11-19) applied to
the three audited modules: `src/a2a`, `src/acp`, `src/agents`.

    DEFINED  — every `pub fn` / `pub async fn` in the module
    CONSUMED — the same name used in production code anywhere in the tree
               (call site, method receiver, function pointer)
    SEVERED  = DEFINED - CONSUMED

The consumption side deliberately counts ANY production mention of the name,
not only `name(...)` call syntax: `spawn_blocking(load_persisted_sessions)`
is a real consumer that a call-shaped regex would miss. Comments and test
code are stripped first, so a symbol that is only mentioned in a doc comment
or only exercised by a unit test still reads as severed.

`KNOWN_SEVERED` is the triaged baseline: the wires that are knowingly left
open, each with the reason it is not connected. It only ever SHRINKS — fix a
wire, delete its baseline entry, and a NEW severance turns the check red.
A guard is a DETECTOR, not a judge: it says a wire is severed, not whether to
reconnect or delete it.

Run from the repo root:  python3 scripts/wiring_seams.py
Exit non-zero when a seam has a severance beyond its baseline.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from wiring_audit import SeamSpec, audit_seam  # noqa: E402  (sibling script)

ROOT = Path(__file__).resolve().parent.parent

# A `pub fn` declaration (the producer side).
DEFINED_RE = r"^[ \t]*pub(?:\([a-z]+\))?\s+(?:async\s+)?(?:unsafe\s+)?fn\s+([a-z_][a-z0-9_]*)"

# Any production mention of an identifier (the consumer side).
CONSUMED_RE = r"(?<![A-Za-z0-9_])([a-z_][a-z0-9_]*)"

# Every Rust tree that can hold a consumer of the audited modules' API.
CONSUMER_GLOBS = "src/**/*.rs,crates/**/*.rs,plugins/**/*.rs,bin/**/*.rs,sdk/**/*.rs"

# Triaged baseline — the wires that are knowingly left open.
#
# `infer_from_addr` (src/a2a/domain/security.rs): classifies a peer address
# into a trust tier. The auth path CANNOT use it — the local bypass must
# accept loopback only, while `infer_from_addr` would classify a LAN peer as
# `Trusted` and hand it the Local tier's admin permissions. The classification
# therefore lives inline in the authenticator, and this helper stays a
# protocol-surface utility. Deliberately not wired.
#
# `check_no_cycle_sync` (src/agents/swarm/tasks/dag.rs): the guard for the
# FIRST path that mutates the edges of an EXISTING task. Today every
# `coord_task_dependencies` row is inserted with a freshly minted task id,
# which makes the check structurally constant-true (it was CUT at that call
# site as vacuous — see `store/crud.rs::create_task`). Deliberately not wired.
KNOWN_SEVERED: dict[str, set[str]] = {
    "a2a-public-api": {"infer_from_addr"},
    "acp-public-api": set(),
    "agents-public-api": {"check_no_cycle_sync"},
}

SPECS = [
    SeamSpec(
        name="a2a-public-api",
        defined_glob="src/a2a/**/*.rs",
        defined_re=DEFINED_RE,
        consumed_glob=CONSUMER_GLOBS,
        consumed_re=CONSUMED_RE,
        known_severed=KNOWN_SEVERED["a2a-public-api"],
        exclude_decl=True,
    ),
    SeamSpec(
        name="acp-public-api",
        defined_glob="src/acp/**/*.rs",
        defined_re=DEFINED_RE,
        consumed_glob=CONSUMER_GLOBS,
        consumed_re=CONSUMED_RE,
        known_severed=KNOWN_SEVERED["acp-public-api"],
        exclude_decl=True,
    ),
    SeamSpec(
        name="agents-public-api",
        defined_glob="src/agents/**/*.rs",
        defined_re=DEFINED_RE,
        consumed_glob=CONSUMER_GLOBS,
        consumed_re=CONSUMED_RE,
        known_severed=KNOWN_SEVERED["agents-public-api"],
        exclude_decl=True,
    ),
]


def main() -> int:
    failed = False
    for spec in SPECS:
        unexpected, defined = audit_seam(spec, ROOT)
        # Baseline entries that are no longer severed are stale: the wire was
        # fixed or the symbol was deleted. Prune them so the baseline keeps
        # describing reality (it may only shrink).
        stale = sorted(
            name for name in spec.known_severed if name not in defined
        )
        for name in stale:
            print(f"  [stale baseline] {name} — no longer defined; prune it")
        if unexpected:
            failed = True
    if failed:
        print(
            "\nFAIL: new severed wire(s) beyond the triaged baseline.\n"
            "      Either wire the far end, or delete the producer."
        )
        return 1
    print("\nOK: no severed wires beyond the triaged baseline.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
