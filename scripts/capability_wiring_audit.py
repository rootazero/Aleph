#!/usr/bin/env python3
"""Severed-wire guard for the capability / canvas / clarification modules.

What it detects
---------------
A **severed wire** is a publicly declared item in the audited modules that has
no production consumer: the producer (the declaration) exists, but nothing in
the production tree ever reads it. This is the "no consumer" form of
severance; the other forms (stub far-end, inert config, client ghost,
name/path drift, never-compiled far-end) are covered by the sibling guards:

* `scripts/rpc_wiring_audit.py`  - RPC client / handler / classifier parity
* `scripts/tool_wiring_audit.py` - tool definition vs dispatch parity
* `scripts/config_wiring_audit.py` - configuration readers vs writers
* `scripts/wiring_seams.py`      - module registration seams

The guard is a **detector, not a judge** (same rule the sibling guards
follow). It cannot decide whether a missing consumer is a bug, a deliberate
deferral, or dead code; it only reports the seam and leaves the verdict to a
human or to the triaged `KNOWN_SEVERED` baseline below.

Baseline rules
--------------
`KNOWN_SEVERED` pins the seams that were reviewed and deliberately left
unmounted (deferred capability phases, surfaces the constitution reserves for
a later phase). The baseline may only **shrink**: an entry whose declaration
disappears from the tree is a stale baseline entry and fails the run, so a
fixed seam cannot stay pinned forever.

Test code is stripped before scanning (`scripts/wiring_strip.py`), so a
`#[cfg(test)]` consumer never keeps a severed wire alive, and a
`#[cfg(test)]` declaration is never reported.

Usage
-----
    python3 scripts/capability_wiring_audit.py [--json]
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import wiring_strip  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
SCAN_DIRS = ("src/capability", "src/canvas", "src/clarification")
CONSUMER_ROOTS = ("src", "tests", "benches")

# A declaration is `pub` (public API) - `pub(crate)` / `pub(super)` are
# crate-internal and therefore not part of the external surface this guard
# watches.
DECL_RE = re.compile(
    r"^[ \t]*pub (?:async )?(?:unsafe )?(?:fn|struct|enum|trait|const|static|type|mod)\s+"
    r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)"
)

# Reviewed, deliberately unmounted seams. Each entry is pinned so that the
# surface cannot grow silently; deleting the declaration fails the run.
KNOWN_SEVERED: dict[str, str] = {
    # --- deferred capability phases (kept on purpose) -------------------
    "src/capability/mod.rs:effect_claim": (
        "EffectClaim producer integration is deferred (AGENTS.md Capability Phase 4); "
        "no production SessionEvent::EffectClaim* construction point exists yet"
    ),
    "src/capability/effect_claim.rs:reconcile_session_events": (
        "same deferred EffectClaim phase as the module itself"
    ),
    "src/capability/grant.rs:create_grant": "capability-grant phase is not mounted",
    "src/capability/grant.rs:get_grant": "capability-grant phase is not mounted",
    "src/capability/grant.rs:grants_by_grantor": "capability-grant phase is not mounted",
    "src/capability/grant.rs:grants_for_grantee": "capability-grant phase is not mounted",
    "src/capability/grant.rs:revoke_grant": "capability-grant phase is not mounted",
    "src/capability/grant.rs:validate_grant_chain": "capability-grant phase is not mounted",
    "src/capability/grant.rs:migrate_session": "capability-grant phase is not mounted",
    "src/capability/lifecycle.rs:renew_lease": (
        "lease TTL renewal has no production renewal policy yet; kept as the "
        "lifecycle contract"
    ),
}


def production_lines(path: Path) -> list[str]:
    """Return the file's lines with every `#[cfg(test)]` region removed."""
    text = path.read_text()
    stripped = wiring_strip.strip_tests(text)
    original = text.split("\n")
    kept = stripped.split("\n")
    out: list[str] = []
    idx = 0
    for line in original:
        if idx < len(kept) and kept[idx] == line:
            out.append(line)
            idx += 1
        else:
            out.append("")
    if idx != len(kept):
        # Alignment failed: fall back to the stripped text so the run reports
        # something rather than silently dropping declarations.
        return kept
    return out


def declarations(files: list[Path]) -> dict[tuple[str, str], dict]:
    found: dict[tuple[str, str], dict] = {}
    for path in files:
        rel = path.relative_to(ROOT).as_posix()
        for lineno, line in enumerate(production_lines(path), start=1):
            match = DECL_RE.match(line)
            if not match:
                continue
            found[(rel, match.group("name"))] = {
                "file": rel,
                "line": lineno,
                "name": match.group("name"),
            }
    return found


def production_corpus(files: list[Path]) -> dict[str, list[tuple[str, int]]]:
    corpus: dict[str, list[tuple[str, int]]] = {}
    for path in files:
        rel = path.relative_to(ROOT).as_posix()
        for lineno, line in enumerate(production_lines(path), start=1):
            if not line:
                continue
            for token in re.findall(r"[A-Za-z_][A-Za-z0-9_]*", line):
                corpus.setdefault(token, []).append((rel, lineno))
    return corpus


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    scan_files: list[Path] = []
    for directory in SCAN_DIRS:
        scan_files.extend(sorted((ROOT / directory).rglob("*.rs")))
    consumer_files: list[Path] = []
    for directory in CONSUMER_ROOTS:
        root = ROOT / directory
        if root.exists():
            consumer_files.extend(sorted(root.rglob("*.rs")))

    declared = declarations(scan_files)
    corpus = production_corpus(consumer_files)

    severed: list[dict] = []
    internal_only: list[dict] = []
    for (rel, name), info in sorted(declared.items()):
        occurrences = [(f, l) for (f, l) in corpus.get(name, []) if not (f == rel and l == info["line"])]
        if not occurrences:
            severed.append(info)
        elif all(f == rel for f, _ in occurrences):
            internal_only.append(info)

    baseline_keys = set(KNOWN_SEVERED)
    severed_keys = {f"{info['file']}:{info['name']}" for info in severed}
    declared_keys = {f"{info['file']}:{info['name']}" for info in declared.values()}
    new_severed = [info for info in severed if f"{info['file']}:{info['name']}" not in baseline_keys]
    # A baseline entry is stale when the declaration disappeared (the seam was
    # deleted) or when the seam got wired (the item now has a consumer outside
    # its own module). Either way the baseline may only shrink.
    stale_baseline = [
        key
        for key in sorted(baseline_keys)
        if key not in severed_keys and key not in declared_keys
    ]
    wired_baseline = [
        key for key in sorted(baseline_keys) if key in declared_keys and key not in severed_keys
    ]

    report = {
        "declared": len(declared),
        "severed": severed,
        "internal_only": internal_only,
        "new_severed": new_severed,
        "stale_baseline": stale_baseline,
        "wired_baseline": wired_baseline,
    }

    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(f"DECLARED public items in {', '.join(SCAN_DIRS)}: {len(declared)}")
        print(f"SEVERED (no production consumer): {len(severed)}")
        for info in severed:
            key = f"{info['file']}:{info['name']}"
            mark = "NEW! " if key in {f"{i['file']}:{i['name']}" for i in new_severed} else "  -  "
            print(f"  {mark}{info['name']}  {info['file']}:{info['line']}")
        print(
            "INTERNAL-ONLY (public surface consumed only inside its own module - "
            "informational, not a severed wire): "
            f"{len(internal_only)}"
        )
        for info in internal_only:
            print(f"  -  {info['name']}  {info['file']}:{info['line']}")
        if wired_baseline:
            print(f"BASELINE ENTRIES THAT GOT WIRED (remove them): {len(wired_baseline)}")
            for key in wired_baseline:
                print(f"  ~  {key}")
        if stale_baseline:
            print(f"STALE BASELINE ENTRIES (must be removed): {len(stale_baseline)}")
            for key in stale_baseline:
                print(f"  !  {key}")

    if new_severed:
        print(f"FAIL: {len(new_severed)} newly severed wire(s) not in the baseline")
        return 1
    if stale_baseline:
        print(f"FAIL: {len(stale_baseline)} stale baseline entr(ies) - the baseline may only shrink")
        return 1
    if wired_baseline:
        print(f"FAIL: {len(wired_baseline)} baseline entr(ies) are wired now - remove them")
        return 1
    print("OK: no newly severed wires")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
