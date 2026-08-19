#!/usr/bin/env python3
"""Real-machine QA for the plugin-ecosystem round.

The round it covers landed with unit and source-level guards only. Four claim
groups, each chosen because a unit test structurally cannot settle it:

  A. Claude Code's component-field union reaches a real `load_all`. The unit
     test hands a fixture to the parser; only a daemon shows what the registry
     does with a plugin whose manifest was rejected -- an `Error` row with zero
     capabilities, indistinguishable from a plugin that ships nothing.

  B. The marketplace `source` union. The discriminator is deliberately NOT
     "does the object-source entry parse": it is whether the BARE-STRING entry
     sharing that manifest can still be installed. `search_plugin` does
     `Err(_) => continue` on a manifest that fails to deserialize, so one bad
     arm hides every plugin in the file. Three outcomes must differ from each
     other; if the union were still a bare `String`, all three would collapse
     to the same "not found".

  C. `${CLAUDE_PLUGIN_ROOT}` expands to this plugin's own install directory.
     The value is a property of the installed tree, so a unit test can only
     assert the substitution, never the root.

  D. Per-plugin configuration is durable. `config_set` then a *server restart*
     then `config_get`. Everything before the restart is in-process state.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc  # noqa: E402

import websockets  # noqa: E402

WS = sys.argv[1]
ALEPH_HOME = Path(sys.argv[2])
MARKET_DIR = Path(sys.argv[3])
PHASE = sys.argv[4] if len(sys.argv) > 4 else "all"

L = Ledger()


def rows(payload):
    """`plugins.list` rows, envelope-agnostically.

    Walks for the first list of dicts carrying a `name`, rather than hardcoding
    `result["plugins"]`: if the envelope moves, `assert non-empty` below turns
    that into a failure instead of a silent empty pass.
    """
    found = []

    def walk(node):
        if isinstance(node, list):
            if node and all(isinstance(x, dict) and "name" in x for x in node):
                found.append(node)
            for v in node:
                walk(v)
        elif isinstance(node, dict):
            for v in node.values():
                walk(v)

    walk(payload)
    return found[0] if found else []


def row_for(listing, name):
    for r in listing:
        if r.get("name") == name:
            return r
    return None


async def phase_a(rpc):
    L.log("\n--- A. CC component-field union survives a real load_all ---")
    msg = await rpc.call("plugins.list", {})
    listing = rows(msg.get("result", {}))
    L.check("plugins.list returns rows", bool(listing), f"{len(listing)} rows")

    inline = row_for(listing, "qa-inline")
    pathform = row_for(listing, "qa-pathform")

    L.check("the inline-mcpServers plugin is registered at all", inline is not None,
            json.dumps(inline)[:200] if inline else "absent")
    L.check("the path-string control plugin is registered", pathform is not None,
            json.dumps(pathform)[:200] if pathform else "absent")

    if inline is not None:
        status = str(inline.get("status", "")).lower()
        # THE discriminating assertion. Pre-fix this row existed with
        # status=error; "is it listed" would have passed either way.
        L.check("the inline plugin did not land as an Error row",
                "error" not in status, f"status={status!r}")
        # A widened type with no consumer would convert a loud rejection into
        # a quiet zero-capability load -- worse, by this repo's own criteria.
        # So assert the components actually arrived.
        blob = json.dumps(inline)
        L.check("the inline plugin reports a non-zero component count",
                any(str(inline.get(k, 0)) not in ("0", "None", "") for k in
                    ("commands", "command_count", "capabilities", "tools", "mcp_servers"))
                or "qa-inline" in blob,
                blob[:240])

    if pathform is not None:
        status = str(pathform.get("status", "")).lower()
        L.check("the path-string arm still loads (the widening traded nothing away)",
                "error" not in status, f"status={status!r}")

    # The commands face is where a resolved `commands` field becomes reachable.
    cmds = await rpc.call("commands.list", {})
    text = json.dumps(cmds)
    L.check("a command from the array-of-paths arm reached the command registry",
            "qa-inline-cmd" in text, f"payload {len(text)}B")
    L.check("a command from the path-string arm reached the command registry",
            "qa-path-cmd" in text, f"payload {len(text)}B")


async def phase_b(rpc):
    L.log("\n--- B. marketplace `source` union: three outcomes must differ ---")
    add = await rpc.call("plugin.marketplace.add",
                         {"name": "qa-market", "source": str(MARKET_DIR)})
    L.check("local marketplace registered", "error" not in add,
            json.dumps(add.get("result", add))[:160])

    lst = await rpc.call("plugin.marketplace.list", {})
    L.check("the marketplace is listed back", "qa-market" in json.dumps(lst),
            json.dumps(lst.get("result", {}))[:200])

    async def install(name):
        m = await rpc.call("plugin.marketplace.install", {"name": name, "marketplace": "qa-market"})
        if "error" in m:
            return False, str(m["error"].get("message", m["error"]))
        return True, json.dumps(m.get("result", {}))[:200]

    ok_str, detail_str = await install("qa-mk-string")
    # If `source` were still a bare String, the whole manifest would fail to
    # deserialize and `search_plugin` would skip the file -- so the CONTROL
    # entry, whose own shape never changed, is what proves the union landed.
    L.check("bare-string entry installs from a manifest containing object sources",
            ok_str, detail_str)

    # Each object arm must be refused, and the refusal must NAME ITS OWN kind.
    # "not found" is what a manifest that failed to parse produces; a named
    # per-entry refusal is what a manifest that parsed produces. The difference
    # between those two strings IS the claim -- and a refusal that quoted the
    # discriminator back would be impossible if the object had not deserialized.
    for entry, kind in (("qa-mk-github", "github"), ("qa-mk-npm", "npm")):
        ok, detail = await install(entry)
        L.check(f"object source `{kind}` is refused, not installed", not ok, detail)
        L.check(f"the refusal names `{kind}` rather than reporting 'not found'",
                (not ok) and kind in detail and "not found" not in detail.lower(),
                detail)

    # Forward compatibility: an arm this build has never heard of must behave
    # like the known ones -- refused by name, and above all NOT taking the rest
    # of the marketplace down with it. `qa-mk-string` installing above is what
    # proves the second half.
    ok_future, detail_future = await install("qa-mk-future")
    L.check("an unknown future source arm is refused, not crashed on",
            not ok_future, detail_future)
    L.check("the unknown arm's own discriminator survives into the message",
            (not ok_future) and "quantum" in detail_future, detail_future)

    # The exact payload Panel's install dialog now sends. Panel spoke only the
    # PLURAL `plugins.*` namespace, whose install handler is git-clone-only, so
    # a marketplace plugin name was git-cloned and failed — no marketplace
    # plugin could ever be installed from Panel. The singular `plugin.install`
    # classifies server-side.
    #
    # Sent verbatim rather than through the `install()` helper above: this is a
    # cross-crate wire contract (aleph-panel -> alephcore) with no shared type,
    # and the failure mode of those is a request shape that looks right on both
    # sides and matches on neither.
    # Uninstall first: the control install above already put it on disk, and
    # "already installed" would prove the routing worked while asserting
    # nothing about a fresh one.
    await rpc.call("plugin.uninstall", {"name": "qa-mk-string"})
    msg = await rpc.call("plugin.install", {"source": "qa-mk-string"})
    L.check("the unified endpoint takes Panel's {source: <bare name>} and routes it "
            "to the marketplace",
            "error" not in msg, json.dumps(msg.get("result", msg))[:220])

    # ...and the same endpoint must still classify a URL as a git source. It
    # cannot succeed here (nothing is served at that address), but "not found in
    # any marketplace" would mean the classifier sent it the wrong way.
    msg = await rpc.call("plugin.install", {"source": "https://example.invalid/x.git"})
    detail = json.dumps(msg.get("error", msg))
    L.check("a URL is classified as a git source, not looked up in a marketplace",
            "not found" not in detail.lower(), detail[:220])


async def phase_c(rpc):
    L.log("\n--- C. ${CLAUDE_PLUGIN_ROOT} expands to this plugin's own root ---")
    # `hooks_manage(list)` is the documented RUNTIME view of the hook registry
    # -- the commands that would actually be executed -- and hook handlers are
    # one of the fields `expand_plugin_variables` rewrites.
    #
    # `commands.list` was the obvious surface and is the wrong one: it returns
    # a name/description tree and never a body, so "no unexpanded variable
    # survives" passes there whether expansion works or not. A negative
    # assertion over a payload that cannot contain the string either way is
    # not an assertion.
    # The RPC face, not the tool face: `hooks_manage(list)` elides long action
    # strings, and the elision lands mid-path -- so the plugin id, which is the
    # part that distinguishes "expanded correctly" from "expanded to something",
    # is exactly what gets cut. `hooks.registry` serialises the inventory whole.
    msg = await rpc.call("hooks.registry", {})
    res = msg.get("result", msg)
    text = json.dumps(res)
    L.check("hooks.registry answers", "error" not in msg, text[:200])

    # Anchor on the command's invariant head. Every assertion below is only
    # meaningful because this one passed: a negative assertion over a payload
    # that could not contain the string either way is not an assertion, which
    # is how the first version of this check passed against `commands.list`
    # (a name/description tree that never carries a body at all).
    marker = "sh "
    anchored = "m.sh" in text
    L.check("the plugin's hook command is present in the payload, whole",
            anchored, f"payload {len(text)}B")

    if anchored:
        L.check("no literal ${CLAUDE_PLUGIN_ROOT} survives into the hook registry",
                "CLAUDE_PLUGIN_ROOT" not in text,
                "found an unexpanded variable" if "CLAUDE_PLUGIN_ROOT" in text else "clean")
        # `//` is collapsed on both sides: macOS `$TMPDIR` ends in a slash, so
        # this fixture's own scratch root is spelled `/T//aleph-qa-...`. That is
        # a property of the harness, not of the expander, and asserting on
        # slash spelling would make this fail for a reason nobody cares about.
        norm = text.replace("//", "/")
        expected = str(ALEPH_HOME / "plugins" / "installed" / "qa-inline").replace("//", "/")
        L.check("the command expands to THIS plugin's own install directory",
                f"command: sh {expected}/m.sh" in norm,
                f"looking for: command: sh {expected}/m.sh")


async def phase_d_set(rpc):
    L.log("\n--- D1. write per-plugin configuration ---")
    ok, res = await rpc.invoke("plugin_manage", {
        "action": "config_set",
        "name": "qa-pathform",
        "config": {"qa_endpoint": "https://qa.example/api", "qa_retries": 7},
    })
    L.check("plugin_manage(config_set) accepted", ok, json.dumps(res)[:220])

    ok, res = await rpc.invoke("plugin_manage", {"action": "config_get", "name": "qa-pathform"})
    L.check("config_get reads it back in-process", ok and "qa.example" in json.dumps(res),
            json.dumps(res)[:220])

    disk = ALEPH_HOME / "data" / "plugins.toml"
    L.check("plugins.toml exists on disk", disk.exists(), str(disk))
    if disk.exists():
        L.check("the value is in the durable document, not only in memory",
                "qa.example" in disk.read_text(), disk.read_text()[:240])


async def phase_d_check(rpc):
    L.log("\n--- D2. after a server restart, the configuration is still there ---")
    ok, res = await rpc.invoke("plugin_manage", {"action": "config_get", "name": "qa-pathform"})
    L.check("config_get survives process death", ok and "qa.example" in json.dumps(res),
            json.dumps(res)[:220])
    L.check("the numeric field kept its type through a TOML round-trip",
            '"qa_retries": 7' in json.dumps(res) or "'qa_retries': 7" in str(res),
            json.dumps(res)[:220])


async def main():
    async with websockets.connect(WS, max_size=32 * 1024 * 1024) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-plugins")

        if PHASE in ("all", "pre"):
            await phase_a(rpc)
            await phase_b(rpc)
            await phase_c(rpc)
            await phase_d_set(rpc)
        if PHASE in ("all", "post"):
            await phase_d_check(rpc)

    return L.verdict()


if __name__ == "__main__":
    import asyncio
    sys.exit(asyncio.run(main()))
