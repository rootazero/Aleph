#!/usr/bin/env python3
"""Prove spec §5.3/§5.5 on two real engines: the login survives the switch.

Every claim here is one a fake engine cannot make. The load-bearing one is the
LAST pair: the source engine's process is gone and a chromium is in its place.
A switch that leaves the old browser running looks identical to a correct one
from every RPC angle.
"""
import argparse
import asyncio
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "browser_managed"))
from qa_rpc import Ledger, Rpc, ws_connect  # noqa: E402

# `token_processes` and `main_processes` carry two pieces of reasoning this
# file must not re-derive (判据 §1): the `--` that BSD pgrep needs before a
# pattern starting with `-` (without it it matches nothing, which reads exactly
# like "the process is gone"), and the whole-word match that stops
# `--storage-dir=<X>` from also finding `--storage-dir=<X>-neighbour`.
sys.path.insert(0, os.path.dirname(__file__))
from drive import main_processes, token_processes  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("ws")
ap.add_argument("--page-url", required=True)
ap.add_argument("--aleph-home", required=True)
ap.add_argument("--obscura-storage", required=True)
args = ap.parse_args()

led = Ledger()
log, check = Ledger.log, led.check
COOKIE = "aleph-qa-switch"
VALUE = "carried-%d" % os.getpid()
# The obscura the `default` profile runs: the fixture names that directory
# explicitly (run.sh's `--user-data-dir "$OBSCURA_STORAGE"`), so this is the
# path the launcher actually passes and not one this script guessed.
OBSCURA_PAT = f"--storage-dir={args.obscura_storage}"
# The chromium the switch starts, on the other hand, is NOT that directory: a
# `user_data_dir` written for obscura is in obscura's format, so
# `ProfileManager::request_from` hands the target engine the managed path under
# its own `data_subdir` instead. That derivation is
# `browser_state_dir("chromium")/sanitize_session_key("default")`.
CHROMIUM_UDD = os.path.join(args.aleph_home, "data", "browser", "chromium", "default")


async def main():
    async with ws_connect(args.ws) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-browser-dual-switch")

        ok, res = await rpc.invoke(
            "browser_open", {"url": args.page_url, "profile": "default"}
        )
        check(
            "browser_open on the default profile succeeds",
            ok and res.get("success") is True,
            json.dumps(res)[:200],
        )
        check(
            "browser_open reports obscura",
            res.get("engine") == "obscura",
            str(res.get("engine")),
        )

        before = token_processes(OBSCURA_PAT)
        check("an obscura owns our storage dir before the switch", bool(before), str(before))

        ok, res = await rpc.invoke(
            "browser_cookies",
            {
                "profile": "default",
                "action": "set",
                "name": COOKIE,
                "value": VALUE,
                "domain": "127.0.0.1",
                "path": "/",
            },
        )
        check(
            "a cookie is set on obscura",
            ok and res.get("success") is True,
            json.dumps(res)[:200],
        )

        ok, res = await rpc.invoke("browser_tabs", {"profile": "default", "action": "list"})
        check(
            "the tab list shows the page before the switch",
            args.page_url in json.dumps(res),
            json.dumps(res)[:200],
        )

        ok, res = await rpc.invoke(
            "browser_session",
            {
                "profile": "default",
                "action": "switch_engine",
                "engine": "chromium",
                "migrate": True,
            },
        )
        # `ok` alone is not the claim: every arm of this tool returns
        # `success: false` inside an `ok` envelope for a refusal, so reading
        # only `ok` would call a refused switch a success and blame the wrong
        # step three claims later.
        check(
            "switch_engine succeeds",
            ok and res.get("success") is True,
            json.dumps(res)[:400],
        )
        check(
            "it reports the engine it landed on",
            res.get("switched_to") == "chromium",
            str(res.get("switched_to")),
        )
        check(
            "it reports at least one cookie carried",
            (res.get("cookies_moved") or 0) >= 1,
            str(res.get("cookies_moved")),
        )
        check(
            "it returns the new page tree in the same call",
            bool(res.get("snapshot_text")),
            str(res.get("snapshot_text"))[:120],
        )

        ok, res = await rpc.invoke("browser_cookies", {"profile": "default", "action": "list"})
        listed = json.dumps(res)
        check(
            "the cookie set on obscura is readable on chromium",
            COOKIE in listed and VALUE in listed,
            listed[:300],
        )

        ok, res = await rpc.invoke("browser_tabs", {"profile": "default", "action": "list"})
        check(
            "the tab is on the same URL it was on before the switch",
            args.page_url in json.dumps(res),
            json.dumps(res)[:200],
        )

        # Every verb above this line went through `get_backend`, which resolves
        # the engine from the PROFILE CONFIG. That they answered at all is the
        # only evidence that the switch moved the config as well as the
        # registry — without it each one would have answered `EngineMismatch`.
        chrome = main_processes(f"--user-data-dir={CHROMIUM_UDD}")
        check("a chromium is now running", bool(chrome), str(chrome))

        # The load-bearing claim, and the only one invisible from the RPC face.
        after = token_processes(OBSCURA_PAT)
        check(
            "no obscura with our storage dir survives the switch",
            not after,
            f"before={before} after={after}",
        )

        ok, res = await rpc.invoke(
            "browser_session",
            {
                "profile": "default",
                "action": "switch_engine",
                "engine": "chromium",
                "migrate": True,
            },
        )
        check(
            "switching to the engine already running is refused, not reported as success",
            res.get("success") is False,
            json.dumps(res)[:200],
        )

    return led.verdict()


sys.exit(asyncio.run(main()))
