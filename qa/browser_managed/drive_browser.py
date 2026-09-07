#!/usr/bin/env python3
"""Drive the managed browser stack against a REAL browser over `tools.invoke`.

Why this fixture exists
-----------------------
Every browser unit test in the tree runs against a fake backend or asserts a
"degrades without a running browser" path. That is the shape of test whose
green is a property of the machine, and it hid four defects for four rounds:
the managed driver — the DEFAULT driver — never issued `playwright-cli open`,
so every tool answered "the browser is not open"; `--headed` was prepended to
`tab-new`, which rejects it outright; no line of a real `tab-list` parsed, so
the post-navigation SSRF audit ran over an empty listing; and the PDF engine
drove the same never-opened session.

So the claims here are deliberately the ones a fake backend cannot make.

The oracle
----------
`playwright-cli list` is read out of band, with the scenario's scratch HOME
(the CLI's session store is HOME-scoped). It reports whether a session is open
at all.

That is as far as it goes now. Since the launch chain flipped — Aleph spawns
Chromium itself and the CLI joins over `attach --cdp` — the CLI no longer owns
the profile directory, so `playwright-cli list` has no `user-data-dir:` line
to echo (it used to, back when the CLI launched the browser itself; the second
oracle below replaced that reading). "The `--config` file Aleph generated
actually reached the browser" is now proven by `DevToolsActivePort` appearing
INSIDE `--expect-user-data-dir` plus a live `/json/version` on the port it
names — the port file is written by Chrome itself, into the directory Aleph
chose, which is a claim about the browser rather than about the CLI's copy of
our config.

The control group
-----------------
"`browser_open` succeeded" is satisfied just as well by a session that was
already open, which would make the whole fixture a tautology. So every scenario
first drives a NON-launching verb (`browser_navigate`) against the same fresh
profile and requires it to FAIL — that is both the proof the session started
closed and the proof that only the launching verb opens one.
"""
import argparse
import asyncio
import json
import os
import sys

from qa_rpc import Ledger, Rpc, cli_sessions, http_json, read_devtools_port_file, ws_connect

ap = argparse.ArgumentParser()
ap.add_argument("url")
ap.add_argument("scenario", choices=["open", "ambient", "headed"])
ap.add_argument("--page-url", required=True)
ap.add_argument("--marker", required=True)
ap.add_argument("--home", required=True, help="scratch HOME, for the CLI oracle")
ap.add_argument("--cli", required=True)
ap.add_argument("--expect-user-data-dir", required=True)
ap.add_argument("--planted-user-data-dir", default="")
ap.add_argument("--cwd", required=True, help="the server's cwd, which must stay clean")
ap.add_argument(
    "--output-dir-root",
    required=True,
    help="~/.aleph/data/browser/cli-output, where page snapshots must land instead",
)
args = ap.parse_args()

# The RPC plumbing and the out-of-band `playwright-cli` oracle live in
# `qa_rpc.py`, shared with `drive_tools.py`: the two drivers are meant to
# disagree about what to assert, never about how they talk to the gateway.
_led = Ledger()
log = Ledger.log
check = _led.check
_rpc = [None]


async def invoke(ws, tool, arguments):
    return await _rpc[0].invoke(tool, arguments)


def sessions():
    return cli_sessions(args.cli, args.home)


async def scenario(ws):
    log(f"\n--- control: a non-launching verb must NOT open a browser ---")
    # `NavigateAction` is an externally-tagged enum: `{"goto": {"url": ...}}`.
    # The first version of this fixture sent `{"action": "goto", "url": ...}`,
    # which fails deserialization — the control "passed" while proving nothing
    # about the browser at all, which is the exact tautology qa/README warns
    # about. So the rejection reason is asserted, not just the rejection.
    ok, res = await invoke(
        ws,
        "browser_navigate",
        {"action": {"goto": {"url": args.page_url}}, "profile": "default"},
    )
    # The tool degrades to success:false rather than erroring, so accept either
    # an RPC error or an unsuccessful body — what must NOT happen is success.
    navigated = ok and isinstance(res, dict) and res.get("success")
    check(
        "browser_navigate on a closed session does not succeed",
        not navigated,
        f"result={json.dumps(res)[:180]}",
    )
    blob = json.dumps(res).lower()
    check(
        "…and it was refused for the absence of a browser, not for bad arguments",
        ("not open" in blob or "no active browser" in blob or "open/goto first" in blob)
        and "invalid arguments" not in blob,
        f"result={json.dumps(res)[:220]}",
    )
    before = sessions()
    check(
        "and it left no open session behind",
        "status: open" not in before,
        f"list={before.strip()[:160]!r}",
    )

    log(f"\n--- browser_open must launch a real browser ---")
    ok, res = await invoke(ws, "browser_open", {"url": args.page_url, "profile": "default"})
    check("browser_open reports success", ok and res.get("success"), json.dumps(res)[:220])
    tab_id = (res or {}).get("tab_id")
    # `"last"` is the sentinel the code falls back to when the listing parses to
    # nothing — which is what every real listing did before this round. A
    # numeric id is the proof the real `- 0: [](url)` format now parses.
    check(
        "the tab id is a parsed id, not the 'last' sentinel",
        tab_id is not None and str(tab_id).isdigit(),
        f"tab_id={tab_id!r}",
    )

    after = sessions()
    check("the CLI now reports an open session", "status: open" in after, after.strip()[:200])

    log(f"\n--- the generated --config must have reached the browser ---")
    # Not `playwright-cli list` echoing `user-data-dir:` — under `attach --cdp`
    # the CLI does not own the profile dir, so it has nothing to echo (see the
    # docstring's "The oracle"). The stronger claim: Chrome itself wrote
    # `DevToolsActivePort` inside the directory Aleph's config named, and the
    # port it names is a live CDP endpoint — proof the browser was launched
    # with our user-data-dir, not merely that the CLI was told about one.
    port_file = os.path.join(args.expect_user_data_dir, "DevToolsActivePort")
    endpoint = read_devtools_port_file(port_file)
    check(
        "the launch honored user_data_dir from the profile (DevToolsActivePort appeared there)",
        endpoint is not None,
        port_file,
    )
    if endpoint:
        port, _browser_path = endpoint
        try:
            status, version = http_json(port, "/json/version")
        except Exception as e:  # noqa: BLE001 - the failure IS the claim
            status, version = 0, {"error": str(e)}
        check(
            "…and that user-data-dir's Chrome is a live, answering CDP endpoint",
            status == 200 and "Chrome/" in version.get("Browser", ""),
            json.dumps(version)[:200],
        )
    if args.scenario == "headed":
        # `--headed` is an option of `open`; it used to be prepended to the
        # `tab-new` argv, where the CLI rejects it outright (`Unknown option:
        # --headed`, exit 1). So headed mode was a hard failure on every call,
        # not a degraded one — and this is the claim only this scenario makes.
        check(
            "a headless=false profile really launched headed",
            "headed: true" in after,
            f"list={after.strip()[:220]!r}",
        )
    if args.scenario == "ambient":
        # A `.playwright/cli.config.json` planted in the server's cwd must not
        # be consulted: the CLI auto-loads it relative to the process cwd, and
        # the driver never sets one, so the child inherits the server's. That
        # schema also carries `initScript` (JS in every page) and `cdpEndpoint`.
        check(
            "an ambient .playwright/cli.config.json is NOT honored",
            args.planted_user_data_dir not in after,
            f"planted {args.planted_user_data_dir!r} must be absent",
        )

    log(f"\n--- the page content must come back through the read path ---")
    ok, res = await invoke(ws, "browser_snapshot", {"profile": "default"})
    snap = (res or {}).get("snapshot") or ""
    check("browser_snapshot succeeds", ok and res.get("success"), json.dumps(res)[:200])
    check(
        "the snapshot carries the fixture's marker",
        args.marker in snap,
        f"marker={args.marker!r} snapshot[:200]={snap[:200]!r}",
    )

    # The CLI writes page snapshots and console logs to `.playwright-cli/`
    # relative to the process cwd, and the driver sets no cwd — so without an
    # explicit `outputDir` the browsed page's accessibility tree lands in
    # whatever directory the server was started in. (It did: a full snapshot of
    # a visited site turned up in a git checkout.)
    log(f"\n--- browsed page content must not land in the server's cwd ---")
    litter = os.path.join(args.cwd, ".playwright-cli")
    strays = sorted(os.listdir(litter)) if os.path.isdir(litter) else []
    check(
        "the server's cwd has no .playwright-cli/ litter",
        not os.path.exists(litter),
        f"{litter} exists, containing {strays[:5]}" if strays else f"{litter} absent",
    )
    # Absence alone is a vacuous pass — it also describes a CLI that wrote
    # nothing at all. The containment claim needs the positive half: the
    # snapshots exist, under Aleph's own storage.
    landed = []
    for root, _dirs, files in os.walk(args.output_dir_root):
        landed += [os.path.join(root, f) for f in files]
    check(
        "…and the snapshots did land under ~/.aleph instead",
        any(f.endswith((".yml", ".yaml", ".log")) for f in landed),
        f"output root {args.output_dir_root} holds {[os.path.basename(f) for f in landed][:5]}",
    )

    log(f"\n--- a second tab proves tab addressing on real listings ---")
    ok, res = await invoke(ws, "browser_open", {"url": args.page_url, "profile": "default"})
    second = (res or {}).get("tab_id")
    check("a second browser_open succeeds", ok and res.get("success"), json.dumps(res)[:200])
    check(
        "the second tab gets a distinct parsed id",
        second is not None and str(second).isdigit() and second != tab_id,
        f"first={tab_id!r} second={second!r}",
    )
    # A second open must NOT have relaunched the browser: `playwright-cli open`
    # is destructive (new pid, every tab dropped), which is precisely why the
    # launch is lazy and gated on the CLI's own refusal.
    both_numeric = all(x is not None and str(x).isdigit() for x in (tab_id, second))
    check(
        "the browser was not relaunched (tab ids keep increasing)",
        both_numeric and int(second) > int(tab_id),
        f"ids {tab_id!r} -> {second!r} should be increasing within one browser",
    )


async def main():
    async with ws_connect(args.url) as ws:
        _rpc[0] = Rpc(ws)
        await _rpc[0].connect("qa-browser-managed")
        await scenario(ws)
    return _led.verdict()


sys.exit(asyncio.run(main()))
