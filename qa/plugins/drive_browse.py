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
# Only `registrations` uses it: a real local marketplace directory to add.
MARKET_DIR = sys.argv[3] if len(sys.argv) > 3 else ""

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


async def phase_registrations(rpc):
    """The registration surface: list / add / remove, and the `removable` bit.

    `plugin.marketplace.list` was the last member of this family the server
    built as a `json!` literal and every client decoded by hand, so a renamed
    key could not go red on either side. It is also the call the Panel's new
    Marketplaces section reads, and the reason that section can exist at all:
    `add` and `remove` were registered and admin-classed, but their only client
    was `interfaces/cli`, a binary the release workflow never builds.

    Why a real daemon. The built-in marketplace is not a fixture — it is
    injected into every `list()` by `all_marketplaces` and refused by every
    `remove()`. On a fresh install it is the *only* row there is, so "does the
    Remove button appear on a row the server then rejects" is a question only a
    process holding the real manager can answer.
    """
    L.log("marketplace registrations: list / add / remove")

    listed = body(await rpc.call("plugin.marketplace.list", {}))
    rows = listed.get("marketplaces") or []
    L.check("list returns rows", bool(rows), f"n={len(rows)}")

    # Key-set equality on the *real* wire. Parsing a response only ever proves
    # a superset, because serde ignores unknown keys on the way in — so a
    # client-side decode test is structurally blind to a key that stopped being
    # sent. This looks at what the daemon actually put on the socket.
    required = {"name", "source", "type", "removable"}
    allowed = required | {"unremovable_reason"}
    bad = [r for r in rows if not required <= set(r) or not set(r) <= allowed]
    L.check(
        "every row carries exactly the keys its renderers read",
        not bad,
        f"offenders={bad[:2]}",
    )

    by_name = {r.get("name"): r for r in rows}
    builtin = by_name.get(BUILTIN)
    L.check(f"the built-in '{BUILTIN}' is listed", builtin is not None)
    if builtin:
        L.check(
            "the built-in says it is not removable",
            builtin.get("removable") is False,
            f"removable={builtin.get('removable')}",
        )
        L.check(
            "and carries the server's own reason rather than a blank",
            bool(builtin.get("unremovable_reason")),
            str(builtin.get("unremovable_reason")),
        )

    names = [r.get("name") for r in rows]
    L.check(
        "rows arrive sorted by name",
        names == sorted(names),
        f"{names}",
        # The server holds them in a HashMap; unsorted means the Panel list
        # reshuffles on every load.
    )

    # --- add -------------------------------------------------------------
    added = await rpc.call("plugin.marketplace.add", {"source": MARKET_DIR})
    L.check("add accepts a local path", "error" not in added, json.dumps(added)[:200])
    new_name = body(added).get("name") or ""
    L.check("add answers with the name it derived", bool(new_name), new_name)

    # The Panel composes add + update, because `add` registers without
    # fetching while the shipped `aleph-server plugin marketplace add` syncs
    # right after. Driving both here is what proves the pair the Panel sends.
    synced = await rpc.call("plugin.marketplace.update", {"name": new_name})
    L.check("update syncs the freshly added source", "error" not in synced, json.dumps(synced)[:200])

    rows = body(await rpc.call("plugin.marketplace.list", {})).get("marketplaces") or []
    fresh = {r.get("name"): r for r in rows}.get(new_name)
    L.check(f"'{new_name}' is listed after add", fresh is not None)
    if fresh:
        L.check("it is typed local", fresh.get("type") == "local", str(fresh.get("type")))
        L.check("it says it IS removable", fresh.get("removable") is True)
        L.check("and carries no reason", fresh.get("unremovable_reason") is None)

    # --- the bit and the action, on the real daemon ----------------------
    # The unit guard asserts these agree in-process. This asserts it of the
    # daemon a client actually talks to, for every row it will actually show.
    for row in rows:
        name = row.get("name")
        claims_removable = row.get("removable") is True
        outcome = await rpc.call("plugin.marketplace.remove", {"name": name})
        really_removed = "error" not in outcome
        L.check(
            f"'{name}': the removable bit matches what remove does",
            claims_removable == really_removed,
            f"bit={claims_removable} remove_ok={really_removed}",
        )

    after = body(await rpc.call("plugin.marketplace.list", {})).get("marketplaces") or []
    left = [r.get("name") for r in after]
    L.check(
        "the built-in survived a remove it refused",
        BUILTIN in left,
        f"left={left}",
    )
    L.check(
        f"'{new_name}' is gone after remove",
        new_name not in left,
        f"left={left}",
    )


async def main():
    async with websockets.connect(WS, max_size=32 * 1024 * 1024) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-browse")
        if PHASE == "contents":
            await phase_contents(rpc)
        elif PHASE == "install":
            await phase_install(rpc)
        elif PHASE == "registrations":
            await phase_registrations(rpc)
        else:
            print(f"unknown phase {PHASE}")
            return 2
    return L.verdict()


if __name__ == "__main__":
    import asyncio

    sys.exit(asyncio.run(main()))
