#!/usr/bin/env python3
"""Fail-fast poller for the aleph-app-release.yml CI run.

Why this exists: `gh run watch --exit-status` only returns when the WHOLE run
finishes, so a single platform job that fails early (e.g. a `#[cfg(target_os)]`
const-fn that only compiles on the host) does not surface until the other two
platforms have also ground through their ~30-minute builds. This poller checks
job-level status every 60s and exits the instant ANY job reports
failure/cancelled/timed_out, printing which platform broke — turning a 33-minute
round-trip into an early bail-out. It also guards transient empty/timeout `gh`
responses (the dev network has had brief DNS/proxy drops) instead of crashing.

Reuse on every release:
    python3 scripts/poll_release_run.py            # auto-picks latest run
    python3 scripts/poll_release_run.py <run-id>   # explicit run
    python3 scripts/poll_release_run.py <run-id> 540   # custom budget (seconds)

Exit output (parse the last line):
    RESULT=COMPLETED conclusion=success   -> all platforms green
    RESULT=FAILED                         -> one+ platform broke (names printed)
    RESULT=STILL_RUNNING                  -> budget elapsed; just run it again
"""
import sys
import json
import subprocess
import time

WORKFLOW = "aleph-app-release.yml"
TERMINAL_BAD = ("failure", "cancelled", "timed_out")


def latest_run_id():
    out = subprocess.run(
        ["gh", "run", "list", "--workflow", WORKFLOW, "--limit", "1",
         "--json", "databaseId", "--jq", ".[0].databaseId"],
        capture_output=True, text=True, timeout=45,
    )
    rid = out.stdout.strip()
    if not rid:
        raise SystemExit("could not resolve latest %s run id" % WORKFLOW)
    return rid


def main():
    rid = sys.argv[1] if len(sys.argv) > 1 else latest_run_id()
    budget = int(sys.argv[2]) if len(sys.argv) > 2 else 540
    print("polling run %s (budget %ss)" % (rid, budget))
    deadline = time.time() + budget

    while time.time() < deadline:
        try:
            out = subprocess.run(
                ["gh", "run", "view", rid, "--json", "status,conclusion,jobs"],
                capture_output=True, text=True, timeout=45,
            )
        except Exception as e:  # network/proxy drop — tolerate, retry
            print(time.strftime("%H:%M:%S"), "gh-timeout", repr(e))
            time.sleep(30)
            continue
        if out.returncode != 0 or not out.stdout.strip():
            print(time.strftime("%H:%M:%S"), "gh-empty rc=%s" % out.returncode)
            time.sleep(30)
            continue
        try:
            d = json.loads(out.stdout)
        except Exception as e:
            print(time.strftime("%H:%M:%S"), "json-fail", repr(e))
            time.sleep(20)
            continue

        overall = d.get("status")
        concl = d.get("conclusion") or "running"
        jobs = d.get("jobs", [])
        failed = [j for j in jobs if j.get("conclusion") in TERMINAL_BAD]
        if failed:
            print("RESULT=FAILED")
            for j in failed:
                print("  FAILED_JOB:", j.get("name"), "->", j.get("conclusion"))
            return
        if overall == "completed":
            print("RESULT=COMPLETED conclusion=" + concl)
            return
        states = ",".join(
            (j.get("name", "?").split("(")[-1].rstrip(")")[:7] + ":" + j.get("status", "?")[:4])
            for j in jobs
        )
        print(time.strftime("%H:%M:%S"), "running [" + states + "]")
        time.sleep(60)

    print("RESULT=STILL_RUNNING")


if __name__ == "__main__":
    main()
