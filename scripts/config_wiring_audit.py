#!/usr/bin/env python3
"""Config wiring parity audit — catch "inert" policy config.

A `[policies.*]` section is a severed wire when its type is declared,
deserialized, and shipped to the Panel (users can set it) but NO core code
consumes it — the knob does nothing. The robust signal (used by the
2026-07-15 audit) is TYPE reachability: a policy type referenced ONLY inside
src/config/ has no consumer. This is far less noisy than per-field grep
(field names like `enable_warnings` collide across the tree).

  DECLARED — every `pub struct/enum` in src/config/types/policies/**
  CONSUMED — that type name referenced anywhere in src/ OUTSIDE src/config/
             (tests stripped)

Inert = DECLARED with zero CONSUMED references. Grep-level guard, no compile.
Run from repo root:  python3 scripts/config_wiring_audit.py
Exit non-zero when the inert set exceeds KNOWN_INERT (triaged baseline).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

ROOT = Path(__file__).resolve().parent.parent
# Test code is stripped by the shared brace-matching helper: cutting at the
# first `#[cfg(test)]` (the old inline version) discarded most of the 108 files
# whose first such attribute is an inline helper above production code.
from wiring_strip import strip_tests  # noqa: E402  (sibling script, not a package)


TYPE_RE = re.compile(r"pub (?:struct|enum) (\w+)")

# Not user-facing `[policies.*]` sections, so not subject to the inertness
# check: aggregator containers (consumed via the parent Config) and
# section-internal helper/sub-element types (consumed within their own
# section's logic inside src/config/). Verified 2026-07-15.
NOT_A_SECTION = {
    "PoliciesConfig",        # top aggregator — consumed as a field of Config
    "MemoryPolicies",        # [policies.memory] group — consumed via Config
    "TierPreset",            # helper of the LIVE exec_tier section (builtin_tiers())
    "ModePreset",            # id-only mirror of `SessionMode` exposed to UI /
                             # CLI surfaces (same role as `TierPreset`); the live
                             # consumer is `SessionMode` itself.
}

# Policy sections declared but never consumed core-side.
#
# Baseline is EMPTY: the 2026-07-15 batch-B fix resolved all seven the audit
# found. Six were DELETED (R10 corpses from dissolved middleware) — tool_safety
# (the R7-forbidden name classifier ExecTier replaced, incl. its test-only
# `infer_safety_level` reader), intent, keyword, experimental, text,
# memory.ai_retrieval. One was WIRED: MetricsPolicy is now bound in
# `Config::load` → `metrics::init_metrics_runtime`, so its knobs reach the live
# StageTimer. A NEW inert policy section now fails CI.
KNOWN_INERT: set[str] = set()


def main() -> int:
    policy_dir = ROOT / "src/config/types/policies"
    declared: dict[str, str] = {}
    for f in policy_dir.rglob("*.rs"):
        text = strip_tests(f.read_text(encoding="utf-8", errors="replace"))
        rel = f.relative_to(ROOT).as_posix()
        for m in TYPE_RE.finditer(text):
            declared.setdefault(m.group(1), f"{rel}:{text.count(chr(10), 0, m.start()) + 1}")

    # Concatenate all core source OUTSIDE src/config/, tests stripped.
    consumer_text_parts: list[str] = []
    for f in (ROOT / "src").rglob("*.rs"):
        if "config" in f.relative_to(ROOT).parts[:2]:  # skip src/config/**
            continue
        consumer_text_parts.append(strip_tests(f.read_text(encoding="utf-8", errors="replace")))
    consumer_text = "\n".join(consumer_text_parts)

    def consumed(type_name: str) -> bool:
        return re.search(rf"\b{re.escape(type_name)}\b", consumer_text) is not None

    inert = sorted(t for t in declared if t not in NOT_A_SECTION and not consumed(t))

    print(f"DECLARED policy types: {len(declared)}")
    print(f"\n=== Policy types with zero core-side consumer (inert knob) ({len(inert)}) ===")
    for t in inert:
        tag = "  [known]" if t in KNOWN_INERT else "  [NEW!] "
        print(f"{tag}{t}  {declared[t]}")

    unexpected = set(inert) - KNOWN_INERT
    if unexpected:
        print(f"\nFAIL: {len(unexpected)} NEW inert policy type(s) beyond the triaged baseline:")
        for t in sorted(unexpected):
            print(f"  - {t}  {declared[t]}")
        return 1
    print("\nOK: no inert policy types beyond the triaged baseline.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
