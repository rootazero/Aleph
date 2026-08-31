#!/usr/bin/env python3
"""Assert on WHAT THE MODEL WAS HANDED, not on what the server logged.

`resume_coordinator.rs`'s own unit tests already pin the exact bytes of
`boundary_repair_text` for both provenances. This script does not repeat
that — it reads `$REQUEST_LOG`, the mock provider's record of every request
body it actually received, and checks that the repair text (a) reached the
model at all and (b) still carries its four semantic points once it got
there. `--stage attribute` additionally checks that the two dangles in that
scenario were NOT both attributed to this restart — the exact defect this
round's design spec (§1.4) fixes, and the one the `attribute` stage exists to
falsify on the pre-round tree.
"""
import argparse
import json
import pathlib
import sys

FOUR_POINTS = [
    "OUTCOME UNKNOWN",
    "NOT a report that the call failed",
    "side effects",
]
THIS_RESTART = "the server restarted"
EARLIER_RUN = "an earlier run in this session"


def request_bodies(path):
    out = []
    for line in pathlib.Path(path).read_text().splitlines():
        if not line.strip():
            continue
        try:
            out.append(json.loads(line)["body"])
        except (json.JSONDecodeError, KeyError):
            continue
    return out


def repair_texts(bodies):
    """Every distinct chunk of text carrying "OUTCOME UNKNOWN", across every
    request body — a raw substring scan rather than a fixed schema walk, so
    this does not care whether a tool_result's content is a bare string or a
    nested content-block list."""
    out = []
    for body in bodies:
        for msg in body.get("messages", []):
            content = msg.get("content")
            if isinstance(content, list):
                for block in content:
                    text = json.dumps(block, ensure_ascii=False)
                    if "OUTCOME UNKNOWN" in text:
                        out.append(text)
            elif isinstance(content, str) and "OUTCOME UNKNOWN" in content:
                out.append(content)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--request-log", required=True)
    ap.add_argument("--stage", required=True, choices=["crash", "attribute"])
    args = ap.parse_args()

    bodies = request_bodies(args.request_log)
    if not bodies:
        print(f"FAIL: {args.request_log} holds no request bodies at all", file=sys.stderr)
        return 1

    texts = repair_texts(bodies)
    if not texts:
        print("FAIL: no OUTCOME UNKNOWN reached the model", file=sys.stderr)
        return 1

    failures = []
    for text in texts:
        for point in FOUR_POINTS:
            if point not in text:
                failures.append(f"missing {point!r} in: {text[:200]}")

    if args.stage == "attribute":
        this = [t for t in texts if THIS_RESTART in t]
        earlier = [t for t in texts if EARLIER_RUN in t]
        if not earlier:
            failures.append(
                "FAIL: the dangle left by the EARLIER run was blamed on this restart "
                "(no 'an earlier run in this session' text reached the model). "
                "This is the pre-fix behaviour §1.4 describes."
            )
        if not this:
            failures.append("FAIL: this run's own dangle did not say 'the server restarted'")

    for f in failures:
        print(f, file=sys.stderr)
    if failures:
        return 1

    print(f"PASS ({len(texts)} repair text chunk(s) reached the model)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
