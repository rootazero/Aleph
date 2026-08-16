#!/usr/bin/env python3
"""RPC wiring parity audit — catch "severed wire" RPC methods.

Aleph's JSON-RPC method names live in THREE independent, hand-maintained
places across THREE crates, with no single source of truth binding them:

  1. HANDLED   — handler registrations in `src/gateway/**` + `src/bin/aleph-server/**`
                 (`.register("m", ..)`, `register_handler!(server, "m", ..)`).
  2. CLASSIFIER — `rate_limiter::scope_for_method`, `lane::override_for`,
                 `sandbox/rate_limit.rs` (method literals used for rate/lane policy).
  3. CLIENT    — Panel/webchat `rpc_call("m", ..)` sites.

A "severed wire" is a method named in CLIENT or CLASSIFIER that no HANDLED
site serves: the Panel calls it and gets METHOD_NOT_FOUND, or a rate/lane
policy guards a method that doesn't exist (usually a rename that never
propagated). This script recomputes the diffs and prints the severed set.

It is deliberately a grep-level guard (no compile): fast, cross-crate,
re-runnable in CI. Run from repo root:  python3 scripts/rpc_wiring_audit.py
Exit code is non-zero when the severed set exceeds KNOWN_SEVERED (the
baseline of already-triaged findings), so a NEW severance fails CI while
the in-flight fixes stay listed.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

ROOT = Path(__file__).resolve().parent.parent

# ---------------------------------------------------------------------------
# Extraction
# ---------------------------------------------------------------------------

# Dotted method name (config.get, memory.search) — used for CLIENT/CLASSIFIER
# where every entry must be a real method literal.
METHOD_RE = re.compile(r"^[a-z][a-zA-Z0-9_]*(?:\.[a-zA-Z0-9_]+)+$")
# A registered handler name — dotted OR a bare word (health, echo, version).
# Excludes axum URL paths ("/secrets") and other non-method strings.
HANDLED_METHOD_RE = re.compile(r"^[a-z][a-zA-Z0-9_.]*$")

# Cut a file's `#[cfg(test)]` module before scanning: test assertions name
# lots of methods (`scope_for_method("unknown.method")`) that are not policy.
# Test code is stripped by the shared brace-matching helper: cutting at the
# first `#[cfg(test)]` (the old inline version) discarded most of the 108 files
# whose first such attribute is an inline helper above production code.
from wiring_strip import strip_tests  # noqa: E402  (sibling script, not a package)


def rs_files(*rel_dirs: str) -> list[Path]:
    files: list[Path] = []
    for rel in rel_dirs:
        base = ROOT / rel
        if base.is_file():
            files.append(base)
        elif base.is_dir():
            files.extend(base.rglob("*.rs"))
    return files


def scan(files: list[Path], patterns: list[re.Pattern[str]]) -> dict[str, list[str]]:
    """Return {method: [locations]} for every literal captured by any pattern."""
    found: dict[str, list[str]] = {}
    for f in files:
        try:
            text = f.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        rel = f.relative_to(ROOT).as_posix()
        text = strip_tests(text)
        for pat in patterns:
            for m in pat.finditer(text):
                name = m.group(1)
                line = text.count("\n", 0, m.start()) + 1
                found.setdefault(name, []).append(f"{rel}:{line}")
    return found


# HANDLED: any registration site. Broad on purpose — missing a registration
# mechanism would false-positive a real handler as "severed", so err toward
# capturing more.
HANDLED_PATTERNS = [
    re.compile(r'\.register(?:_query|_mutate|_notify)?\(\s*"([^"]+)"'),
    re.compile(r'register_handler!\(\s*\w+\s*,\s*"([^"]+)"'),
    re.compile(r'\.route\(\s*"([^"]+)"'),  # admin_api axum routers (path, not rpc — filtered below)
]

# CLASSIFIER: method literals inside the rate/lane policy surfaces only.
CLASSIFIER_PATTERNS = [re.compile(r'"([a-z][a-zA-Z0-9_.]*)"')]

# CLIENT: Panel rpc_call sites. DOTALL so a method split onto the next line
# after `rpc_call(` is still captured.
CLIENT_PATTERNS = [
    re.compile(r'rpc_call\(\s*"([^"]+)"', re.DOTALL),
    re.compile(r'\.call(?:::<.*?>)?\(\s*"([^"]+)"', re.DOTALL),
]

# Methods served OUTSIDE the .register/register_handler! path — protocol-level
# special-cases dispatched directly in server/handler.rs. Not severed.
PROTOCOL_ALLOWLIST = {
    "connect",
    "events.subscribe",
    "events.unsubscribe",
    "events.list",
    "node.approval.request",
}

# Already-triaged severed wires (the 2026-07-15 wire audit). Being fixed;
# listed so the script stays green on known work-in-progress and only fails
# on NEW severances. Remove an entry as its fix lands.
# CLIENT phantoms resolved 2026-07-15 (batch A): skills.add repointed to
# skills.install; config.set / config.list / sessions.set_pinned were dead client
# wrappers (zero callers, no server-side feature) and were deleted.
# CLASSIFIER phantoms resolved 2026-07-15 (batch F): config.apply / config.set /
# memory.store deleted from rate_limiter's RpcWrite list (no handler); session.delete
# repointed to the real plural sessions.delete so the destructive delete lands in the
# strict write bucket instead of the loose default; connect.challenge removed from
# lane's Query overrides (an unbuilt nonce handshake). Empty = no known gaps.
KNOWN_SEVERED: set[str] = set()


def main() -> int:
    handled_raw = scan(
        rs_files("src/gateway", "src/bin/aleph-server"), HANDLED_PATTERNS
    )
    handled = {m for m in handled_raw if HANDLED_METHOD_RE.match(m)}
    handled |= PROTOCOL_ALLOWLIST

    classifier = {
        m
        for m in scan(
            # The two authoritative gateway RPC classifiers. (The sandbox
            # command-policy limiter in src/sandbox/rate_limit.rs also names a
            # few RPC methods, but they're a subset of these — and scanning it
            # picks up its non-method hook name — so it's intentionally omitted.)
            rs_files(
                "src/gateway/rate_limiter.rs",
                "src/gateway/lane.rs",
            ),
            CLASSIFIER_PATTERNS,
        )
        if METHOD_RE.match(m)
    }

    client_raw = scan(
        rs_files("interfaces/webchat", "interfaces/cli", "interfaces/tui", "shared/client"),
        CLIENT_PATTERNS,
    )
    client = set(client_raw)

    client_severed = sorted(client - handled)
    classifier_severed = sorted(classifier - handled)

    def show(title: str, methods: list[str], locs: dict[str, list[str]] | None) -> None:
        print(f"\n=== {title} ({len(methods)}) ===")
        for m in methods:
            tag = "  [known]" if m in KNOWN_SEVERED else "  [NEW!] "
            where = ""
            if locs is not None:
                where = "  " + ", ".join(locs.get(m, [])[:2])
            print(f"{tag}{m}{where}")

    print(f"HANDLED methods: {len(handled)} | CLASSIFIER: {len(classifier)} | CLIENT: {len(client)}")
    show("CLIENT calls with no handler (Panel → METHOD_NOT_FOUND)", client_severed, client_raw)
    show("CLASSIFIER phantoms (rate/lane guards a non-existent method)", classifier_severed, None)

    unexpected = (set(client_severed) | set(classifier_severed)) - KNOWN_SEVERED
    if unexpected:
        print(f"\nFAIL: {len(unexpected)} NEW severed wire(s) beyond the triaged baseline:")
        for m in sorted(unexpected):
            print(f"  - {m}")
        return 1
    print("\nOK: no severed wires beyond the triaged baseline.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
