#!/usr/bin/env python3
"""Owner trust policy, end to end on a real daemon.

`OwnerTrustPolicy` shipped complete and unreachable: a full implementation, a
`PluginStatus::Blocked` variant, and a `PluginRuntimeStatus::Blocked` consumer
chain across three crates — with zero producers, because the only production
constructor passed `ExtensionConfig::default()`. Every install was permissive
and `Blocked` could not occur.

Connecting it means the interesting claims are all about *state that crosses a
process boundary*, which is exactly what an in-process test cannot show:

  1. the default posture is unchanged (every plugin still loads);
  2. enforcing it makes previously-loading plugins report `blocked`;
  3. vouching for one lets that one through and no others;
  4. all of it survives a restart, since the policy is re-derived from
     `plugins.toml` at construction.

Run in four phases so the harness can restart the server between them: the
policy is a LOAD gate, so its effect is visible on the next load, not on the
call that changed it.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc  # noqa: E402

import websockets  # noqa: E402

WS = sys.argv[1]
ALEPH_HOME = Path(sys.argv[2])
PHASE = sys.argv[3]

L = Ledger()
PLANTED = ["qa-inline", "qa-pathform", "qa-vars"]


def rows(payload):
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


async def statuses(rpc):
    msg = await rpc.call("plugins.list", {})
    return {r.get("name"): str(r.get("status", "")).lower() for r in rows(msg.get("result", {}))}


async def phase_baseline(rpc):
    L.log("\n--- 1. default posture: enforcement off, everything loads ---")
    ok, res = await rpc.invoke("plugin_manage", {"action": "trust_status"})
    L.check("trust_status answers", ok, json.dumps(res)[:200])
    L.check("enforcement is OFF on a fresh install",
            ok and res.get("data", {}).get("enforced") is False,
            json.dumps(res.get("data", {}))[:200])

    st = await statuses(rpc)
    for name in PLANTED:
        L.check(f"[{name}] loads with enforcement off",
                st.get(name) == "loaded", f"status={st.get(name)!r}")

    L.log("\n--- 2. turn enforcement on, and vouch for exactly one ---")
    ok, res = await rpc.invoke("plugin_manage", {"action": "trust_enforce", "enforce": True})
    L.check("trust_enforce accepted", ok, json.dumps(res)[:220])

    # A LOAD gate must not silently tear down running plugins; that is
    # `disable`'s job. The summary has to say so, because a caller who assumes
    # otherwise tells an operator the plugin has been stopped when it has not.
    L.check("the reply says enforcement does not stop running plugins",
            ok and "not stopped" in json.dumps(res).lower(),
            json.dumps(res.get("summary", ""))[:220])

    ok, res = await rpc.invoke("plugin_manage", {"action": "trust", "name": "qa-inline"})
    L.check("trust accepted for qa-inline", ok, json.dumps(res)[:220])

    disk = ALEPH_HOME / "data" / "plugins.toml"
    text = disk.read_text() if disk.exists() else ""
    L.check("enforcement is in the durable document",
            "enforce = true" in text, text[:300])
    L.check("the vouch is in the durable document",
            "trusted = true" in text, text[:300])


async def phase_enforced(rpc):
    L.log("\n--- 3. after restart: only the vouched plugin loads ---")
    st = await statuses(rpc)

    L.check("the vouched plugin still loads",
            st.get("qa-inline") == "loaded", f"status={st.get('qa-inline')!r}")

    for name in ("qa-pathform", "qa-vars"):
        # `blocked` and absent are very different answers: the operator needs
        # the id in order to vouch for it, which is why the refusal registers a
        # row instead of dropping the plugin.
        L.check(f"[{name}] is refused by the policy",
                st.get(name) == "blocked",
                f"status={st.get(name)!r} (absent would mean the row was dropped, "
                f"leaving no id to vouch for)")

    ok, res = await rpc.invoke("plugin_manage", {"action": "trust_status"})
    data = res.get("data", {}) if ok else {}
    L.check("trust_status reports enforcement survived the restart",
            data.get("enforced") is True, json.dumps(data)[:200])
    L.check("trust_status lists the vouched plugin",
            data.get("trusted") == ["qa-inline"], json.dumps(data)[:200])


async def phase_untrust(rpc):
    L.log("\n--- 4. withdraw the vouch ---")
    ok, res = await rpc.invoke("plugin_manage", {"action": "untrust", "name": "qa-inline"})
    L.check("untrust accepted", ok, json.dumps(res)[:220])
    # The distinction `untrust` vs `disable` is not inferable from the names,
    # so the reply must carry it.
    L.check("the reply says untrust does not stop a running plugin",
            ok and "disable" in json.dumps(res).lower(),
            json.dumps(res.get("summary", ""))[:250])


async def phase_all_blocked(rpc):
    L.log("\n--- 5. after restart: nothing planted loads ---")
    st = await statuses(rpc)
    for name in PLANTED:
        L.check(f"[{name}] is refused once its vouch is gone",
                st.get(name) == "blocked", f"status={st.get(name)!r}")

    # Bundled plugins are trusted by construction — enforcement is about
    # directories anyone can drop a tree into, and an install where turning the
    # policy on removed the shipped plugins would be a policy nobody keeps on.
    hist = {}
    for v in st.values():
        hist[v] = hist.get(v, 0) + 1
    L.log(f"  status histogram: {hist}")

    # The scoping claim. `collect_plugin_dirs` unions the skill, command and
    # agent directories into this walk, so on a stock install ~88 of these ~91
    # rows are bundled SKILLS, not plugins. Gating them on a policy called
    # *plugin* owner trust meant switching enforcement on took the entire
    # shipped skill library with it — which is what the first run of this
    # scenario showed, and why `trust_gated` exists.
    #
    # Asserting "something is still loaded" was the wrong shape: those skill
    # rows have never been `loaded` (they are not plugin manifests, so they
    # report `error`, before and after this change alike). The claim that
    # actually discriminates is that enforcement moved EXACTLY the plugin rows
    # and nothing else.
    blocked = {name for name, v in st.items() if v == "blocked"}
    L.check("exactly the planted plugins are blocked, nothing else",
            blocked == set(PLANTED),
            f"blocked={sorted(blocked)} expected={sorted(PLANTED)}")
    L.check("no skill/command/agent directory was caught by the plugin policy",
            hist.get("blocked", 0) == len(PLANTED),
            f"{hist.get('blocked', 0)} blocked rows vs {len(PLANTED)} planted plugins; "
            f"a larger number means the union of non-plugin dirs is being gated")


async def main():
    async with websockets.connect(WS, max_size=32 * 1024 * 1024) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-trust")
        if PHASE == "baseline":
            await phase_baseline(rpc)
        elif PHASE == "enforced":
            await phase_enforced(rpc)
            await phase_untrust(rpc)
        elif PHASE == "blocked":
            await phase_all_blocked(rpc)
        else:
            print(f"unknown phase {PHASE}")
            return 2
    return L.verdict()


if __name__ == "__main__":
    import asyncio
    sys.exit(asyncio.run(main()))
