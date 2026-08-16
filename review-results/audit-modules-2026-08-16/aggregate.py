#!/usr/bin/env python3
"""Aggregate all module summary.json files into one unified findings list."""
import json, sys, os

BASE = os.path.dirname(os.path.abspath(__file__))
MODULES = ["memory", "metrics", "orchestrator", "pii", "providers", "resilience", "routing", "runtimes"]

def norm_finding(mod, f):
    """Normalize a finding dict from any agent schema."""
    loc = f.get("path") or f.get("file") or f.get("location") or "?"
    sev = f.get("severity", "low").lower()
    if sev not in ("critical", "high", "medium", "low"):
        sev = "low"
    return {
        "module": mod,
        "id": f.get("id", "?"),
        "severity": sev,
        "category": f.get("category", "?"),
        "confidence": f.get("confidence", "?"),
        "location": loc,
        "title": f.get("title", f.get("summary", "?")),
        "description": f.get("description", f.get("summary", "")),
        "fix": f.get("suggested_fix") or f.get("fix") or "",
    }

all_findings = []
for mod in MODULES:
    d = os.path.join(BASE, mod)
    if not os.path.isdir(d):
        continue
    for fn in sorted(os.listdir(d)):
        if not fn.startswith("summary") or not fn.endswith(".json"):
            continue
        try:
            data = json.load(open(os.path.join(d, fn)))
        except Exception as e:
            print(f"WARN: cannot parse {fn}: {e}", file=sys.stderr)
            continue
        for f in data.get("findings", []):
            all_findings.append(norm_finding(mod, f))

# sort by severity then module
sev_order = {"critical": 0, "high": 1, "medium": 2, "low": 3}
all_findings.sort(key=lambda f: (sev_order.get(f["severity"], 9), f["module"], f["location"]))

counts = {"critical": 0, "high": 0, "medium": 0, "low": 0}
for f in all_findings:
    counts[f["severity"]] += 1

print(f"TOTAL: {len(all_findings)} findings  C={counts['critical']} H={counts['high']} M={counts['medium']} L={counts['low']}")
print()
for f in all_findings:
    if f["severity"] in ("critical", "high"):
        print(f"[{f['severity'].upper()}] {f['module']}/{f['id']} {f['location']} — {f['title']}")

with open(os.path.join(BASE, "_aggregated.json"), "w") as out:
    json.dump({"counts": counts, "findings": all_findings}, out, indent=1, ensure_ascii=False)
print("\nWritten: _aggregated.json")
