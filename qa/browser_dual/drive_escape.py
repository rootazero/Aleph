#!/usr/bin/env python3
"""The escape hatch, on the host this branch was built for: no `playwright-cli`.

**Why this stage exists.** The branch's headline arrangement is "obscura is the
default kernel, Chromium is the escape hatch", and `src/tools/probes/browser.rs`
names the target host out loud: *"a machine provisioned exactly the way the
dual-engine branch intends — obscura installed, no `playwright-cli`, no `npx`."*
Every other fixture here runs on a machine that HAS `playwright-cli` on `PATH`,
so every `browser_open` in them passed through a `managed_cli_path` that
answered `Some`. 284 green assertions could not see this axis, which is why it
took a source reading to find it and why a source reading is not where it should
have been settled.

**The axis is the whole fixture, and it took two attempts to make it real.**
`managed_cli_path` reads two places — a `which` walk of `PATH`, and the runtime
ledger — so the first draft scrubbed `PATH` and stopped there. The escape hatch
then OPENED, and for a moment that read like "the defect does not exist". It was
the stage's own *is-my-axis-real* assertion that said otherwise: the scratch
ledger had a `playwright-cli` entry anyway, because the ledger is filled by
`runtimes::probe`, which searches **well-known install directories that no PATH
contains** — fnm's among them, located from `$FNM_DIR`. A host with fnm exported
still has a playwright-cli as far as Aleph is concerned.

So the axis is three things, each asserted rather than assumed:

  * every `PATH` entry holding a `playwright-cli` is dropped;
  * `$FNM_DIR` / `$FNM_MULTISHELL_PATH` / `npm_config_prefix` / `$ASDF_DATA_DIR`
    are unset, so the probe's directory search finds nothing either;
  * `$ALEPH_HOME` is a scratch dir, and the ledger is read **at both ends** of
    the run — `runtimes::probe` writes lazily, so a single reading at startup
    cannot rule out an entry that appeared during the launch.

That first failure is the reason this docstring is long: a fixture that hides a
tool badly reports the absence of a defect, and reads exactly like a fixture
that hid it well.

**The control is in the stage.** `default` stays on obscura and must still open:
without that, "chromium refused" and "this whole server is broken because the
PATH surgery removed something it needed" are the same reading. A fixture whose
negative result has two explanations has measured nothing.
"""
import argparse
import asyncio
import json
import os
import shutil
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "browser_managed"))
from qa_rpc import Ledger, Rpc, ws_connect  # noqa: E402

sys.path.insert(0, os.path.dirname(__file__))
from drive import main_processes  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("ws")
ap.add_argument("--page-url", required=True)
ap.add_argument("--aleph-home", required=True)
ap.add_argument("--chromium-udd", required=True)
ap.add_argument("--chrome-bin", required=True)
args = ap.parse_args()

led = Ledger()
check = led.check


def ledger_state():
    """`(ok, detail)` for "the runtime ledger hands out no playwright-cli".

    ⚠️ **The predicate is `executable()`, not "the name appears".** The first
    draft asserted the string `playwright-cli` was absent from the file, and it
    went red on a run where `managed_cli_path` had correctly answered `None` —
    because the ledger carries an entry for every capability it knows about,
    most of them `Missing`, and `CapabilityLedger::executable` hands back a path
    only for `status == "ready"`. An instrument measuring a different predicate
    than the code does reports a difference that is not there (判据 §18); the
    claim now reads the same field the launcher reads.

    Returned as a pair so the SAME derivation answers at both ends of the run —
    two call sites reading one fact, rather than two spellings of it.
    """
    path = os.path.join(args.aleph_home, "runtimes", "ledger.json")
    if not os.path.exists(path):
        return True, f"{path}: <absent>"
    with open(path, encoding="utf-8") as fh:
        raw = fh.read()
    try:
        entries = (json.loads(raw) or {}).get("entries") or {}
    except ValueError as exc:
        # Unparsed is UNKNOWN, and an unknown may only say so (判据 §8).
        return False, f"{path}: unparseable ({exc}): {raw[:160]}"
    entry = entries.get("playwright-cli")
    if entry is None:
        return True, f"{path}: no playwright-cli entry at all"
    ready = str(entry.get("status", "")).lower() == "ready"
    return not ready, f"{path}: playwright-cli entry = {json.dumps(entry)[:200]}"


async def main():
    # ---------------------------------------------------------------- the axis
    #
    # Asserted, not assumed. `managed_cli_path` reads exactly two places — a
    # `which` walk of `PATH` and the runtime ledger — so those are the two
    # claims. A stage that merely BELIEVED it had hidden the CLI would report a
    # green escape hatch on a host that still had one, which is the shape that
    # kept this defect invisible in the first place.
    check(
        "no playwright-cli is reachable on this process's PATH",
        shutil.which("playwright-cli") is None,
        str(os.environ.get("PATH", ""))[:300],
    )
    check(
        "…and the runtime ledger hands out no executable for it, BEFORE any browser opens",
        *ledger_state(),
    )

    async with ws_connect(args.ws) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-browser-dual-escape")

        # ------------------------------------------------------------ control
        ok, res = await rpc.invoke(
            "browser_open", {"url": args.page_url, "profile": "default"}
        )
        check(
            "CONTROL: the default profile still opens on obscura without any playwright-cli",
            ok and res.get("success") is True and res.get("engine") == "obscura",
            json.dumps(res)[:300],
        )

        # ------------------------------------------------------------ subject
        #
        # `resolve_binary` has three routes and this host is configured so the
        # FIRST one wins: `[general.browser.runtime] binary_path` names a real
        # Chrome, and `prefer_system_browser = false` keeps route 2 from being
        # the thing that answers. Route 3 — `playwright_managed` — is the only
        # consumer of a CLI binary, and it is never reached. So a refusal here
        # is a refusal for the absence of a tool this resolution would not have
        # consulted.
        ok, res = await rpc.invoke(
            "browser_open", {"url": args.page_url, "profile": "escape"}
        )
        check(
            "THE ESCAPE HATCH OPENS WITH A PINNED CHROME AND NO PLAYWRIGHT-CLI",
            ok and res.get("success") is True,
            json.dumps(res)[:400],
        )
        check(
            "…and it reports the engine it landed on",
            res.get("engine") == "chromium",
            str(res.get("engine")),
        )

        # The load-bearing half: a tool that answered `success: true` without a
        # browser would look identical from here (判据 §11).
        chrome = main_processes(f"--user-data-dir={args.chromium_udd}")
        pinned = [p for p in chrome if args.chrome_bin in p[1]]
        check(
            "…and the process running is the PINNED binary, not one discovery found",
            bool(pinned),
            f"chrome_bin={args.chrome_bin} live={chrome}",
        )

        ok, res = await rpc.invoke(
            "browser_evaluate", {"profile": "escape", "script": "document.title"}
        )
        check(
            "…and the page really loaded in it",
            ok and "dual-engine QA" in json.dumps(res),
            json.dumps(res)[:300],
        )

        # The axis again, at the OTHER end. The state that matters is the one
        # during the launch, and a ledger read once at startup cannot rule out
        # an entry that appeared in between — `runtimes::probe` writes lazily.
        # Two readings of one fact at two moments is not duplication; it is the
        # only way to see a value that changed half-way (判据 §18).
        check(
            "…and the ledger STILL handed out none afterwards, so the launch really had none",
            *ledger_state(),
        )

    return led.verdict()


sys.exit(asyncio.run(main()))
