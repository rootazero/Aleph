#!/usr/bin/env python3
"""Marketplace browsing, end to end on a real daemon.

Installing a plugin by name worked; *finding* the name did not. `search_plugin`
matched an exact id and nothing listed a marketplace's contents, so the only
operator who could use the install field was one who already knew what to type.
`plugin.marketplace.browse` is the surface that was missing, and the Panel
dialog and `aleph plugin marketplace browse` are its two clients.

Why a real daemon rather than a unit test. The built-in marketplace
(`aleph-official`) does not exist as a fixture: its content is extracted from
the binary into `<ALEPH_HOME>/plugins/cache/aleph-official/` on startup. Its
`source` field is the sentinel string `"bundled"` — not a path — and the cache
resolver every *lookup* went through handed that sentinel to the local-path
resolver, which resolved it against the process working directory, found
nothing, and skipped the marketplace. Silently, because skipping an unreadable
cache is the right thing for a lookup to do.

The blast radius of that skip was the whole install-by-name path for anything
Aleph ships: `plugin.install {source: "<official name>"}` → marketplace install
→ `search_plugin` → zero results → "not found, try marketplace update first",
advice that could not work because update wrote to a directory the lookup never
read. Only a process that has actually run the extractor can tell the fixed
version from the broken one, which is why this scenario exists.

Phases:
  contents — browse returns the built-in marketplace's plugins; `list` (the
             registrations) and `browse` (the contents) are different answers;
             a query narrows; every row can be acted on or says why not
  install  — a name discovered by browsing actually installs, and shows up in
             `plugins.list` afterwards
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc  # noqa: E402

import websockets  # noqa: E402

WS = sys.argv[1]
PHASE = sys.argv[2]

L = Ledger()
BUILTIN = "aleph-official"


def body(msg):
    """The result object, or `{}` when the call errored."""
    if "error" in msg:
        return {}
    return msg.get("result") or {}


async def phase_contents(rpc):
    L.log("browse the built-in marketplace")

    listed = await rpc.call("plugin.marketplace.list", {})
    registrations = body(listed).get("marketplaces") or []
    reg_names = {m.get("name") for m in registrations}
    L.check("the built-in marketplace is registered", BUILTIN in reg_names,
            f"registrations={sorted(n for n in reg_names if n)}")

    browsed = await rpc.call("plugin.marketplace.browse", {})
    if "error" in browsed:
        L.check("browse answers at all", False, json.dumps(browsed["error"])[:300])
        return
    result = body(browsed)
    plugins = result.get("plugins") or []
    problems = result.get("problems") or []

    # The headline. Before the cache-resolution fix this list was empty and
    # `problems` carried "Local marketplace path does not exist: bundled" —
    # a sentinel being reported as a missing directory.
    L.check("browse returns the built-in marketplace's contents",
            any(p.get("marketplace") == BUILTIN for p in plugins),
            f"{len(plugins)} row(s), problems={json.dumps(problems)[:300]}")
    L.check("and reports no problem for a marketplace it could read",
            not any(p.get("marketplace") == BUILTIN for p in problems),
            json.dumps(problems)[:300])

    # `list` and `browse` are different questions. A caller reading `list`
    # looking for plugin names finds none and concludes the marketplace is
    # empty — that is the shape this surface exists to end.
    L.check("list answers registrations, browse answers contents",
            reg_names != {p.get("name") for p in plugins},
            f"registrations={sorted(n for n in reg_names if n)} "
            f"contents={sorted(p.get('name') or '' for p in plugins)[:5]}")

    names = [p.get("name") for p in plugins]
    L.log(f"  contents: {sorted(n for n in names if n)[:12]}")

    # Every row must be actionable or say why not — a catalogue that offers an
    # Install button the install call refuses is worse than one that says so.
    bad = [
        p for p in plugins
        if bool(p.get("installable")) == bool(p.get("unavailable_reason"))
    ]
    L.check("every row is installable xor carries a reason it is not",
            not bad, json.dumps(bad)[:300])

    # And every row must name its marketplace, or install cannot address it
    # unambiguously when two marketplaces carry the same name.
    L.check("every row names the marketplace it came from",
            all(p.get("marketplace") for p in plugins),
            json.dumps([p for p in plugins if not p.get("marketplace")])[:200])

    if not names:
        return
    target = sorted(n for n in names if n)[0]

    narrowed = body(await rpc.call("plugin.marketplace.browse", {"query": target}))
    narrowed_names = {p.get("name") for p in narrowed.get("plugins") or []}
    L.check("a query narrows the catalogue rather than being ignored",
            target in narrowed_names and len(narrowed_names) <= len(set(names)),
            f"query={target!r} → {sorted(n for n in narrowed_names if n)}")

    # Case-insensitive, and matched against description as well as name — a
    # searcher who remembers what a plugin does but not what it is called.
    upper = body(await rpc.call("plugin.marketplace.browse", {"query": target.upper()}))
    L.check("the query is case-insensitive",
            target in {p.get("name") for p in upper.get("plugins") or []},
            f"query={target.upper()!r}")

    missing = body(await rpc.call("plugin.marketplace.browse", {"query": "zzz-no-such-plugin"}))
    L.check("a query that matches nothing returns an empty catalogue, not an error",
            missing.get("plugins") == [],
            json.dumps(missing)[:200])

    # Narrowing to a marketplace nobody registered is an empty result, not a
    # broken marketplace: reporting it as a problem would send the operator to
    # debug a cache that was never supposed to exist.
    elsewhere = body(await rpc.call("plugin.marketplace.browse", {"marketplace": "no-such-market"}))
    L.check("narrowing to an unregistered marketplace is empty, not a problem",
            elsewhere.get("plugins") == [] and elsewhere.get("problems") == [],
            json.dumps(elsewhere)[:200])


async def phase_install(rpc):
    L.log("install a name that browsing found")

    result = body(await rpc.call("plugin.marketplace.browse", {}))
    candidates = [
        p for p in (result.get("plugins") or [])
        if p.get("marketplace") == BUILTIN and p.get("installable")
    ]
    if not L.check("browsing offers at least one installable built-in plugin",
                   bool(candidates), json.dumps(result)[:300]):
        return
    target = sorted(candidates, key=lambda p: p["name"])[0]["name"]
    L.log(f"  installing {target!r}")

    installed_before = body(await rpc.call("plugins.list", {})).get("plugins") or []
    already = {p.get("name") for p in installed_before}

    # The bare-name form: exactly what the Panel field and `aleph plugin
    # install <name>` send. Before the fix this answered "Plugin '<name>' not
    # found. Try 'aleph plugin marketplace update' first." for every plugin
    # Aleph ships.
    outcome = await rpc.call("plugin.install", {"source": target})
    L.check("installing a browsed built-in plugin by bare name succeeds",
            "error" not in outcome,
            json.dumps(outcome.get("error", {}))[:300])

    after = body(await rpc.call("plugins.list", {})).get("plugins") or []
    now = {p.get("name") for p in after}
    L.check("and the registry lists it afterwards",
            target in now,
            f"was_present_before={target in already} rows_now={len(now)}")


async def main():
    async with websockets.connect(WS, max_size=32 * 1024 * 1024) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-browse")
        if PHASE == "contents":
            await phase_contents(rpc)
        elif PHASE == "install":
            await phase_install(rpc)
        else:
            print(f"unknown phase {PHASE}")
            return 2
    return L.verdict()


if __name__ == "__main__":
    import asyncio

    sys.exit(asyncio.run(main()))
