#!/usr/bin/env python3
"""Prove the §6.4 `launch` sentence on a real machine.

Aleph starts Chrome, the port file appears, `attach --cdp` connects,
`browser_snapshot` carries `[ref=eN]`, and `close` leaves Chrome alive.

Every claim here is one a fake backend cannot make. The two that matter most
are the ones the flip created: the port file is written into the user-data-dir
ALEPH chose (so the browser is ours, not the CLI's), and the endpoint is still
serving after `browser_profile(close)` — which under `attach --cdp` is a
disconnect, not a shutdown (measured in the spike; nine Chrome processes before
and after).
"""
import argparse
import asyncio
import json
import os
import signal
import subprocess
import sys

from qa_rpc import Ledger, Rpc, cli_sessions, http_json, read_devtools_port_file, ws_connect

ap = argparse.ArgumentParser()
ap.add_argument("url")
ap.add_argument("--page-url", required=True)
ap.add_argument("--marker", required=True)
ap.add_argument("--home", required=True, help="scratch HOME, for the CLI oracle")
ap.add_argument("--cli", required=True)
ap.add_argument("--expect-user-data-dir", required=True,
                help="where DevToolsActivePort must appear — Aleph's choice, not the CLI's")
ap.add_argument("--server-pid", type=int, required=True,
                help="the aleph-server to stop, so the exit-time browser kill can be observed")
args = ap.parse_args()

_led = Ledger()
log = Ledger.log
check = _led.check

PORT_FILE = os.path.join(args.expect_user_data_dir, "DevToolsActivePort")
# The registry, NOT a file inside the udd: a profile may point its
# `user_data_dir` anywhere, so the record that lets a boot sweep find the
# browser lives in one place derived from ALEPH_HOME.
SIDECAR = os.path.join(
    os.environ["ALEPH_HOME"], "data", "browser", "chromium", "default.json"
)


def read_endpoint():
    return read_devtools_port_file(PORT_FILE)


def chrome_pids(udd):
    """Chrome processes carrying OUR user-data-dir. The `pgrep -f` pattern is
    the flag AND its value: a bare `Chrome` would count the developer's own
    browser and the claim would pass on any machine with Chrome open."""
    # `--` before the pattern: the pattern itself starts with `--user-data-dir=`,
    # and BSD pgrep (macOS) — unlike GNU pgrep — has no getopt-style handling
    # for a pattern that looks like an option; without `--` it replies
    # `pgrep: illegal option -- -` and this always-empty result silently makes
    # every claim built on it vacuously true (measured: it hid a real leaked
    # Chrome process behind a passing "no Chrome left" claim).
    out = subprocess.run(
        ["pgrep", "-f", "--", f"--user-data-dir={udd}"],
        capture_output=True, text=True,
    )
    return [p for p in out.stdout.split() if p.strip()]


def chrome_main_pid(udd):
    """Of the pids carrying our --user-data-dir, the ONE that is Aleph's own
    top-level launch — not a renderer/GPU/utility/zygote helper. Every Chrome
    CHILD process is invoked with --type=<kind>; only the top-level browser
    process (built from ChromiumLaunchSpec::argv) has no --type= at all, so
    that is what distinguishes it. Returns (pid, full command line) or
    (None, "") if none of the matched pids qualifies.

    M4 (round 2): the previous version asserted on pids[0] — whichever pid
    `pgrep` happened to list first, unordered — and helper processes carry
    the SAME --user-data-dir flag as the main process but a DIFFERENT argv
    (no --use-mock-keychain). This machine has had as many as nine stray
    Chromes alive at once; pids[0] being the main process was a coin flip
    the fixture happened to keep landing heads on, not a guarantee.

    Queued fix: a pid that exits between `pgrep` (chrome_pids) and this `ps
    -p` gives an EMPTY command line — which does not contain "--type=" either,
    so the old check read that as "found the main process" instead of "this
    pid is gone, learned nothing". An empty `ps` result means unknown; it
    must never be spent as a match."""
    for pid in chrome_pids(udd):
        proc = subprocess.run(
            ["ps", "-p", pid, "-o", "command="],
            capture_output=True, text=True,
        )
        if not proc.stdout.strip():
            continue
        if "--type=" not in proc.stdout:
            return pid, proc.stdout
    return None, ""


async def main():
    async with ws_connect(args.url) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-attach")

        # CONTROL. A non-launching verb must FAIL first, or every claim below is
        # satisfied just as well by a browser that was already running.
        # `NavigateAction` is an externally-tagged enum: `{"goto": {"url": ...}}`,
        # not the flat `{"url": ...}` the brief's first draft used — that shape
        # fails deserialization with "missing field `action`" (verified: the
        # RED path taken WAS the wrong one, an "invalid arguments" failure
        # rather than a "browser not open" one, until this was fixed to match
        # `drive_browser.py`'s working shape).
        ok, body = await rpc.invoke(
            "browser_navigate",
            {"action": {"goto": {"url": args.page_url}}, "profile": "default"},
        )
        # Every browser tool's output carries its OWN `success: bool`
        # (`BrowserNavigateOutput` etc.) — the RPC wrapper's `ok` only means
        # "the tool ran without the gateway itself erroring", and this tool
        # DEGRADES (returns `success: false`) rather than raising when there is
        # no session. Gating on `not ok` alone (the brief's first draft) is
        # therefore always false here and the check fails no matter what the
        # body says — verified: it failed with a body that plainly states the
        # browser is not open, until fixed to check the inner flag too, the
        # same way `drive_browser.py`'s `navigated` does.
        navigated = ok and isinstance(body, dict) and body.get("success")
        # Attributable, not merely failing: the refusal has to be ABOUT the
        # missing browser. `browser_navigate` on a fresh profile would fail for
        # want of a tab too, and a control that passes for the wrong reason is
        # not a control.
        text = json.dumps(body)
        check(
            "a non-launching verb fails on a fresh profile, and says the browser is not open (control)",
            (not navigated)
            and ("not open" in text.lower() or "no active browser" in text.lower()
                 or "open/goto first" in text.lower() or "no tabs" in text.lower()),
            text[:200],
        )
        check("no port file before anything launched (control)", read_endpoint() is None, PORT_FILE)
        check("no sidecar before anything launched (control)", not os.path.exists(SIDECAR), SIDECAR)

        # 1. Aleph starts Chrome.
        ok, body = await rpc.invoke("browser_open", {"profile": "default", "url": args.page_url})
        check("browser_open succeeds", ok and isinstance(body, dict) and body.get("success"),
              json.dumps(body)[:300])

        # 2. The port file appears, in ALEPH's user-data-dir.
        endpoint = None
        for _ in range(60):
            endpoint = read_endpoint()
            if endpoint:
                break
            await asyncio.sleep(0.5)
        check("DevToolsActivePort appeared in Aleph's user-data-dir", endpoint is not None, PORT_FILE)
        if not endpoint:
            return _led.verdict()
        port, browser_path = endpoint
        log(f"  endpoint: http://127.0.0.1:{port}{browser_path}")

        # 3. It is a real, serving CDP endpoint.
        try:
            status, version = http_json(port, "/json/version")
        except Exception as e:  # noqa: BLE001 - the failure IS the claim
            status, version = 0, {"error": str(e)}
        check("the endpoint answers /json/version", status == 200, json.dumps(version)[:200])
        check("it is a Chromium-family browser", "Chrome/" in version.get("Browser", ""),
              version.get("Browser", "<none>"))

        # 4. attach --cdp connected: the CLI can drive the page Aleph opened.
        ok, body = await rpc.invoke("browser_snapshot", {"profile": "default", "max_chars": 4000})
        text = json.dumps(body)
        check("browser_snapshot succeeds", ok and isinstance(body, dict) and body.get("success"), text[:200])
        check("the snapshot carries playwright refs ([ref=eN])", "[ref=e" in text, text[:200])
        check("the snapshot is of the page we asked for", args.marker in text, text[:200])

        # 5. Aleph launched it, not the CLI: a process carrying our udd exists.
        pids = chrome_pids(args.expect_user_data_dir)
        check("a Chrome process carries Aleph's --user-data-dir", len(pids) > 0, " ".join(pids))

        # 5a. `--use-mock-keychain` is really in that process's argv — not just
        #     "a Chrome process exists" (5), which a stalled, half-navigated
        #     Chrome satisfies exactly as well as a healthy one (the whole
        #     shape of the hang: it keeps answering `/json/version` while the
        #     first navigation in every page never dispatches). This is the
        #     hang-rootcause report's own falsifier for `ChromiumLaunchSpec`'s
        #     argv fix: revert the two-line fix and this goes red on a scratch
        #     HOME while checks 1-5 above stay green, because the missing flag
        #     does not stop the process from existing — only from ever
        #     answering `Page.navigate`.
        #     M4 (round 2): must be THE main process, not `pids[0]` — an
        #     arbitrary member of the pgrep match set that can just as well be
        #     a renderer/GPU helper (same --user-data-dir, different argv).
        main_pid, argv_text = chrome_main_pid(args.expect_user_data_dir)
        check(
            "Aleph's chromium argv carries --use-mock-keychain",
            main_pid is not None and "--use-mock-keychain" in argv_text,
            argv_text.strip()[:300],
        )

        # 5b. The record that makes an orphan reclaimable. No unit test can see
        #     this: `write_sidecar` is best-effort and a launch that skipped it
        #     still reports success, so the only place the omission shows up is
        #     here (the plan's Task 1 says so, and this is that claim).
        sidecar = {}
        try:
            with open(SIDECAR) as fh:
                sidecar = json.load(fh)
        except OSError as e:
            log("  sidecar unreadable:", e)
        check("the sidecar registry holds this profile's record", bool(sidecar), SIDECAR)
        check(
            "its pid is one of the live Chrome processes",
            str(sidecar.get("pid")) in pids,
            f"sidecar pid={sidecar.get('pid')} pgrep={pids}",
        )
        check(
            "it records the user-data-dir, which is how the boot sweep matches argv",
            sidecar.get("user_data_dir") == args.expect_user_data_dir,
            str(sidecar.get("user_data_dir")),
        )

        # 6. `close` is a DISCONNECT under attach --cdp.
        #    Driven OUT OF BAND, with the scenario's scratch HOME, because that
        #    is the same command `ProfileManager::reap_idle` runs — and because
        #    `browser_profile` has no close action (its ProfileAction is List |
        #    GetState, verified). Killing Aleph's Chromium is the reaper's other
        #    half and belongs to the `reap` scenario, not to this claim.
        closed = subprocess.run(
            [args.cli, "-s=default", "close"],
            capture_output=True, text=True, timeout=60,
            env={**os.environ, "HOME": args.home},
        )
        log("  playwright-cli close ->", (closed.stdout + closed.stderr).strip()[:200])
        await asyncio.sleep(2)
        try:
            status_after, _ = http_json(port, "/json/version")
        except Exception:  # noqa: BLE001
            status_after = 0
        check("the endpoint still serves after close (close only disconnects)", status_after == 200,
              f"status={status_after}")
        check("the Chrome processes are still there after close",
              len(chrome_pids(args.expect_user_data_dir)) > 0, "")

        # 7. And the CLI can find its way back, which is what makes a reaped or
        #    crashed CLI cost nothing.
        #
        #    Split in two (M5, round 2), so this suite does not carry a
        #    standing-red claim: the re-attach reaching the SAME browser
        #    process — not a relaunch — is what Piece 4 (`59dc20cce`) actually
        #    delivers, and that is what is asserted here. Which TAB that
        #    re-attached session treats as "current" is a separate,
        #    undelivered question — a real subprocess with a real marker page
        #    proved the CLI's tab-listing order differs between the first
        #    attach and this re-attach (neither "always first" nor "always
        #    last") — so it is a named, booked gap (FEATURE_LOCATOR §3.12,
        #    qa/README.md), not an assertion here. Asserting `args.marker in
        #    body` would leave this claim permanently red, which trains
        #    readers to scroll past red rather than fix it.
        ok, body = await rpc.invoke("browser_snapshot", {"profile": "default", "max_chars": 2000})
        reattached_pids = chrome_pids(args.expect_user_data_dir)
        check(
            "a later tool call re-attaches to the SAME browser process (not a relaunch)",
            ok
            and isinstance(body, dict)
            and body.get("success")
            and str(sidecar.get("pid")) in reattached_pids,
            json.dumps(body)[:200],
        )

        log("\n  playwright-cli list (recorded, not asserted — the attach-session shape is a new reading):")
        log(cli_sessions(args.cli, args.home))

    # 8. spec §3.6 「退出时杀」. The websocket is closed by now; stop the daemon
    #    the way an operator does and require the browser to go with it.
    #    `Child` does not kill on drop, so without the explicit shutdown hook
    #    this claim fails and the browser survives every restart.
    os.kill(args.server_pid, signal.SIGTERM)
    for _ in range(60):
        if not chrome_pids(args.expect_user_data_dir):
            break
        await asyncio.sleep(0.5)
    # ⚠️ SCOPE. This claim covers the ORDERLY exit path only: SIGTERM to an idle
    #    server returns from `run_until_shutdown` well inside the 5 s failsafe,
    #    so the wedged path (`start/helpers.rs`, the one that ends in
    #    `process::exit(0)`) is never taken here and this pgrep says NOTHING
    #    about it. That half is pinned by the source census
    #    `both_daemon_exit_paths_reap_background_jobs_and_browsers`, not by this
    #    fixture — stated rather than left to be inferred, because "the QA is
    #    green" would otherwise read as covering both.
    check(
        "SIGTERM to aleph-server leaves no Chrome carrying its user-data-dir (ORDERLY path only)",
        not chrome_pids(args.expect_user_data_dir),
        " ".join(chrome_pids(args.expect_user_data_dir)),
    )
    check(
        "and the sidecar record is gone with it",
        not os.path.exists(SIDECAR),
        SIDECAR,
    )

    return _led.verdict()


if __name__ == "__main__":
    # Not just style: without this guard, importing this module (as
    # test_drive_attach.py does, to unit-test chrome_main_pid without a real
    # machine) launches the whole QA driver — connecting to a real gateway —
    # as a side effect of `import`. `run.sh` always invokes this file as
    # `python3 drive_attach.py ...`, so `__name__` is `"__main__"` there and
    # nothing about the real run changes.
    sys.exit(asyncio.run(main()))
