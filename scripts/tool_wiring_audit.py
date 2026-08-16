#!/usr/bin/env python3
"""Tool wiring parity audit — catch "severed wire" builtin tools.

A builtin tool is reachable only when its `AlephTool::NAME` is BOTH defined
(the tool type exists) AND dispatchable (an arm in `execute_tool` routes the
name to the tool). A tool that is fully implemented but absent from the
dispatch match is a severed wire: the code exists, has tests, but the LLM
can never call it (e.g. `vision`, `sessions_spawn` in the 2026-07-15 audit).

  DEFINED    — every `const NAME: &'static str = "x"` in src/builtin_tools/**
  DISPATCHED — every `"x" =>` arm in the execute_tool match

Severed = DEFINED - DISPATCHED. Grep-level guard, no compile. Run from repo
root:  python3 scripts/tool_wiring_audit.py
Exit non-zero when the severed set exceeds KNOWN_SEVERED (triaged baseline).
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


def scan(files: list[Path], pat: re.Pattern[str]) -> dict[str, str]:
    found: dict[str, str] = {}
    for f in files:
        try:
            text = strip_tests(f.read_text(encoding="utf-8", errors="replace"))
        except OSError:
            continue
        rel = f.relative_to(ROOT).as_posix()
        for m in pat.finditer(text):
            line = text.count("\n", 0, m.start()) + 1
            found.setdefault(m.group(1), f"{rel}:{line}")
    return found


DEFINED_RE = re.compile(r'const NAME:\s*&(?:\'static\s+)?str\s*=\s*"([^"]+)"')
DISPATCH_RE = re.compile(r'^\s*"([a-z][a-zA-Z0-9_]*)"\s*=>', re.MULTILINE)

DISPATCH_FILE = "src/executor/builtin_registry/registry/tool_registry_impl.rs"

# Tools defined but intentionally not dispatched via the builtin match. The
# 2026-07-15 wire audit drained this baseline to empty:
#   - vision (VisionTool): redundant wrapper over VisionPipeline (media_understand
#     already routes through it) — CUT.
#   - sessions_spawn (SessionsSpawnTool): never-wired, partly-stub delegation tool
#     covered by team_delegate / a2a / acp — CUT.
#   - invalid (InvalidTool): repair.rs fallback that was never registered, so the
#     branch was dead; the error path + list_tools already covers it — CUT.
# A NEW severed tool now means a genuinely defined-but-undispatched wire → fix it.
KNOWN_SEVERED: set[str] = set()


def main() -> int:
    defined = scan(list((ROOT / "src/builtin_tools").rglob("*.rs")), DEFINED_RE)
    dispatched = set(scan([ROOT / DISPATCH_FILE], DISPATCH_RE))

    severed = sorted(set(defined) - dispatched)

    print(f"DEFINED tool NAMEs: {len(defined)} | DISPATCHED arms: {len(dispatched)}")
    print(f"\n=== Defined tools with no dispatch arm (LLM can never call them) ({len(severed)}) ===")
    for name in severed:
        tag = "  [known]" if name in KNOWN_SEVERED else "  [NEW!] "
        print(f"{tag}{name}  {defined[name]}")

    unexpected = set(severed) - KNOWN_SEVERED
    if unexpected:
        print(f"\nFAIL: {len(unexpected)} NEW severed tool(s) beyond the triaged baseline:")
        for name in sorted(unexpected):
            print(f"  - {name}  {defined[name]}")
        return 1
    print("\nOK: no severed tools beyond the triaged baseline.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
