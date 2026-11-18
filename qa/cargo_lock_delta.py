#!/usr/bin/env python3
"""Compare frozen, retained and candidate Cargo graphs without resolving them."""
import argparse
import hashlib
import json
import subprocess
import tomllib
from pathlib import Path


def identity(package):
    return package["name"], package["version"], package.get("source", "")


def descriptor(package):
    return {key: package.get(key) for key in ["name", "version", "source", "checksum"]}


def compare(old, new):
    before = {identity(package): descriptor(package) for package in old}
    after = {identity(package): descriptor(package) for package in new}
    names = sorted({package["name"] for package in old + new})
    changed = []
    for name in names:
        previous = [before[key] for key in sorted(before) if key[0] == name]
        current = [after[key] for key in sorted(after) if key[0] == name]
        if previous and current and previous != current:
            changed.append({"name": name, "old": previous, "new": current})
    return {
        "added": [after[key] for key in sorted(after.keys() - before.keys())],
        "removed": [before[key] for key in sorted(before.keys() - after.keys())],
        "changed": changed,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline-rev", required=True)
    parser.add_argument("--retained-lock", type=Path, required=True)
    parser.add_argument("--candidate-lock", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    data = {
        "frozen": subprocess.check_output(["git", "show", args.baseline_rev + ":Cargo.lock"]),
        "retained": args.retained_lock.read_bytes(),
        "candidate": args.candidate_lock.read_bytes(),
    }
    graphs = {key: tomllib.loads(value.decode())["package"] for key, value in data.items()}
    report = {
        "schema_version": 1,
        "identity": ["name", "version", "source"],
        "changed_semantics": "names present on both sides whose complete version/source/checksum sets differ",
        "baseline_revision": args.baseline_rev,
        "inputs": {key: {"sha256": hashlib.sha256(value).hexdigest(), "entries": len(graphs[key])} for key, value in data.items()},
        "frozen_to_retained": compare(graphs["frozen"], graphs["retained"]),
        "retained_to_candidate": compare(graphs["retained"], graphs["candidate"]),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"inputs": report["inputs"], "deltas": {key: {kind: len(values) for kind, values in report[key].items()} for key in ["frozen_to_retained", "retained_to_candidate"]}}, indent=2))


if __name__ == "__main__":
    main()
