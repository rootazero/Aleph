#!/usr/bin/env python3
"""Aggregate harness validation evidence into a single REPORT.md.

Reads:
  $EVIDENCE_DIR/run-meta.json          # build commit, version, timestamps
  $EVIDENCE_DIR/<scenario>/evidence.json  # per-scenario results

Writes:
  $EVIDENCE_DIR/REPORT.md

Spec: docs/superpowers/specs/2026-04-25-harness-production-validation-design.md §6.5
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

MODULE_LABELS: dict[str, str] = {
    "M1": "Orchestration Loop",
    "M2": "Tools",
    "M3": "Memory",
    "M4": "Context Management",
    "M5": "Prompt Assembly",
    "M6": "Tool Calling Schema",
    "M7": "State / Checkpoint",
    "M8": "Error Handling",
    "M9": "Guardrails",
    "M10": "Verification",
    "M11": "Subagent Orchestration",
    "M12": "Init / Boot",
    "gateway": "Gateway",
    "skill_prefetch": "Skill Prefetch (cross-cut)",
}

STATUS_EMOJI: dict[str, str] = {
    "pass": "✅",
    "fail": "❌",
    "skip": "⏭️",
    "blocked": "⏸️",
}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--evidence-dir", required=True, type=Path)
    ap.add_argument("--output", required=True, type=Path)
    args = ap.parse_args()

    meta_path = args.evidence_dir / "run-meta.json"
    if not meta_path.exists():
        print(f"FATAL: {meta_path} not found", file=sys.stderr)
        return 1
    meta = json.loads(meta_path.read_text())

    # Collect scenarios
    scenarios: list[dict] = []
    for ej in sorted(args.evidence_dir.glob("*/evidence.json")):
        try:
            scenarios.append(json.loads(ej.read_text()))
        except json.JSONDecodeError as e:
            print(f"WARN: skipping {ej}: {e}", file=sys.stderr)

    # Tally
    by_status: dict[str, list] = {"pass": [], "fail": [], "skip": [], "blocked": []}
    by_severity_fail: dict[str, int] = {"critical": 0, "high": 0, "medium": 0, "low": 0}
    for s in scenarios:
        st = s.get("status", "unknown")
        by_status.setdefault(st, []).append(s)
        if st == "fail":
            sev = s.get("severity_on_fail", "unknown")
            by_severity_fail[sev] = by_severity_fail.get(sev, 0) + 1

    lines: list[str] = []
    lines.append("# Harness Production Validation Report")
    lines.append("")
    lines.append(f"**Run**: {meta.get('started_at', 'unknown')}  ")
    lines.append(f"**Build**: just build (release) — commit `{meta.get('build_commit', 'unknown')}`  ")
    lines.append(f"**Aleph version**: {meta.get('aleph_version', 'unknown')}  ")
    lines.append(f"**Test HOME**: `{meta.get('test_home', 'unknown')}`  ")
    lines.append(f"**RUST_LOG**: `{meta.get('rust_log', 'unknown')}`")
    lines.append("")
    lines.append("---")
    lines.append("")

    # Verdict
    lines.append("## Verdict")
    lines.append("")
    lines.append("| | Count | Status |")
    lines.append("|---|---|---|")
    total = len(scenarios)
    pass_count = len(by_status.get("pass", []))
    fail_count = len(by_status.get("fail", []))
    blocked_count = len(by_status.get("blocked", []))
    skip_count = len(by_status.get("skip", []))
    lines.append(f"| Total scenarios | {total} | |")
    lines.append(f"| Passed | {pass_count} | {'✅' if pass_count == total else ''} |")
    lines.append(f"| Failed (Critical) | {by_severity_fail['critical']} | {'❌' if by_severity_fail['critical'] > 0 else '✅'} |")
    lines.append(f"| Failed (High) | {by_severity_fail['high']} | {'🟡' if by_severity_fail['high'] > 0 else '✅'} |")
    lines.append(f"| Failed (Medium) | {by_severity_fail['medium']} | {'🟡' if by_severity_fail['medium'] > 0 else '✅'} |")
    lines.append(f"| Failed (Low) | {by_severity_fail['low']} | {'🟡' if by_severity_fail['low'] > 0 else '✅'} |")
    lines.append(f"| Blocked | {blocked_count} | {'⏸️' if blocked_count > 0 else ''} |")
    lines.append(f"| Skipped | {skip_count} | {'⏭️' if skip_count > 0 else ''} |")
    lines.append("")

    # Verdict statement
    if by_severity_fail["critical"] > 0:
        verdict = "**❌ FAIL** — Critical regression detected. Refactor not validated."
    elif by_severity_fail["high"] > 0:
        verdict = "**🟡 PASS WITH WARNINGS** — High-severity gaps. Module functional but not fully exercised."
    elif fail_count > 0 or blocked_count > 0:
        verdict = "**🟡 PASS** — Medium/low gaps and known blockers; refactor introduced no regressions."
    else:
        verdict = "**✅ PASS** — All scenarios green. 12-module ontology functional under post-dissolution topology."
    lines.append(f"**Conclusion**: {verdict}")
    lines.append("")

    # Module matrix
    lines.append("## 12-Module Coverage Matrix")
    lines.append("")
    lines.append("| Module | Scenarios | Status |")
    lines.append("|---|---|---|")
    by_module: dict[str, list[dict]] = {}
    for s in scenarios:
        by_module.setdefault(s.get("module", "?"), []).append(s)
    # Sort modules: M1..M12 first, then others alphabetically
    def module_sort_key(m: str) -> tuple[int, str]:
        if m.startswith("M") and m[1:].isdigit():
            return (0, f"{int(m[1:]):02d}")
        return (1, m)
    for mid in sorted(by_module.keys(), key=module_sort_key):
        scns = by_module[mid]
        statuses = {s.get("status", "?") for s in scns}
        if statuses == {"pass"}:
            emoji = "✅"
        elif "fail" in statuses:
            emoji = "❌"
        elif "blocked" in statuses:
            emoji = "⏸️"
        else:
            emoji = "🟡"
        ids = ", ".join(s.get("scenario_id", "?") for s in scns)
        label = MODULE_LABELS.get(mid, mid)
        lines.append(f"| {mid} {label} | {ids} | {emoji} |")
    lines.append("")

    # Per-scenario detail
    lines.append("## Per-Scenario Detail")
    lines.append("")
    for s in scenarios:
        sid = s.get("scenario_id", "?")
        mod = s.get("module", "?")
        st = s.get("status", "?")
        sev = s.get("severity_on_fail", "?")
        ic = STATUS_EMOJI.get(st, "❓")
        lines.append(f"### {sid} — {mod} (severity={sev})  {ic}")
        lines.append("")
        for c in s.get("checks", []):
            mark = "✅" if c.get("passed") else "❌"
            cid = c.get("id", "?")
            kind = c.get("kind", "?")
            exp = c.get("expected", "")
            act = c.get("actual", "")
            lines.append(f"- {mark} **{cid}** ({kind}): expected `{exp}`, got `{act}`")
        lines.append("")
        # Started/ended for duration if present
        if "started_at" in s and "ended_at" in s:
            lines.append(f"_Started: {s['started_at']}, ended: {s['ended_at']}_")
            lines.append("")

    args.output.write_text("\n".join(lines) + "\n")
    print(f"REPORT.md written to {args.output}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
