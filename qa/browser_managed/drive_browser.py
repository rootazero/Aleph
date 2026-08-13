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
and which `user-data-dir` it launched with — the second is what proves the
`--config` file Aleph generated actually reached the browser, rather than the
browser merely being up.

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
import subprocess
import sys

import websockets

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

failures = []
_id = [100]


def log(*a):
    print(*a, flush=True)


def check(claim, ok, detail=""):
    log(f"  [{'PASS' if ok else 'FAIL'}] {claim}" + (f" — {detail}" if detail else ""))
    if not ok:
        failures.append(f"{claim} ({detail})" if detail else claim)
    return ok


async def rpc(ws, method, params):
    _id[0] += 1
    rid = _id[0]
    await ws.send(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
    while True:
        msg = json.loads(await asyncio.wait_for(ws.recv(), timeout=180))
        if msg.get("id") == rid:
            return msg


async def invoke(ws, tool, arguments):
    """One `tools.invoke`. Returns (ok, result_or_error_text)."""
    msg = await rpc(ws, "tools.invoke", {"tool_name": tool, "arguments": arguments})
    if "error" in msg:
        return False, json.dumps(msg["error"])
    res = msg["result"]
    return bool(res.get("ok")), res.get("result", res)


def cli_sessions():
    """`playwright-cli list`, read with the scratch HOME. Out-of-band oracle."""
    out = subprocess.run(
        [args.cli, "list"],
        capture_output=True,
        text=True,
        timeout=60,
        # HOME is overridden (the CLI's session store is HOME-scoped, and the
        # developer's own sessions are not ours to read) but PATH is inherited:
        # `playwright-cli` is a node script and a hand-made PATH without `node`
        # turns the oracle into `env: node: No such file or directory` — which
        # reads exactly like "no sessions" and would have passed the check.
        env={**os.environ, "HOME": args.home},
    )
    return out.stdout + out.stderr


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
    before = cli_sessions()
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

    after = cli_sessions()
    check("the CLI now reports an open session", "status: open" in after, after.strip()[:200])

    log(f"\n--- the generated --config must have reached the browser ---")
    check(
        "the launch honored user_data_dir from the profile",
        args.expect_user_data_dir in after,
        f"expected {args.expect_user_data_dir!r} in list output",
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
    async with websockets.connect(args.url, max_size=None) as ws:
        msg = await rpc(ws, "connect", {"client_info": {"name": "qa-browser-managed"}})
        log("connect ->", json.dumps(msg.get("result", msg))[:160])
        await scenario(ws)

    log("")
    if failures:
        log(f"VERDICT: FAIL ({len(failures)} claim(s))")
        for f in failures:
            log(f"  - {f}")
        return 1
    log("VERDICT: PASS")
    return 0


sys.exit(asyncio.run(main()))
