#!/usr/bin/env python3
"""Items 8 + 9: the model-facing marketplace verbs, driven through `tools.invoke`.

The Panel fixture's provider is a dead port on purpose, so an agent turn cannot
complete — and an agent turn is not what the claim is about anyway. The claim is
that the tool face and the RPC face answer the same question with the same
answer, and that the tool refuses to install. `tools.invoke` reaches the real
registry with the real arguments, which is the narrowest thing that can say so.
"""
import asyncio
import json
import sys

import websockets

URL = sys.argv[1]
MARKET = sys.argv[2]

rc = 0


def check(ok, label, detail=""):
    global rc
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" — {detail}" if detail else ""))
    if not ok:
        rc = 1


async def main():
    async with websockets.connect(URL, max_size=None) as ws:
        n = [0]

        async def call(method, params):
            n[0] += 1
            await ws.send(json.dumps({"jsonrpc": "2.0", "id": n[0], "method": method, "params": params}))
            while True:
                msg = json.loads(await ws.recv())
                if msg.get("id") == n[0]:
                    return msg

        async def tool(args):
            r = await call("tools.invoke", {"tool_name": "plugin_manage", "arguments": args})
            return r

        await call("connect", {"client": "qa", "version": "1"})

        print("item 8: the tool face agrees with the RPC face")

        rpc = await call("plugin.marketplace.list", {})
        rpc_rows = {r["name"]: r for r in rpc["result"]["marketplaces"]}

        t = await tool({"action": "marketplace_list"})
        if "error" in t:
            check(False, "marketplace_list reached the tool", json.dumps(t["error"])[:300])
            return
        payload = t["result"]
        body = payload.get("content") or payload
        text = json.dumps(body)
        # tools.invoke wraps the tool output; find the marketplaces array wherever it landed.
        def find_rows(o):
            if isinstance(o, dict):
                if "marketplaces" in o and isinstance(o["marketplaces"], list):
                    return o["marketplaces"]
                for v in o.values():
                    f = find_rows(v)
                    if f is not None:
                        return f
            if isinstance(o, list):
                for v in o:
                    f = find_rows(v)
                    if f is not None:
                        return f
            if isinstance(o, str):
                try:
                    return find_rows(json.loads(o))
                except Exception:
                    return None
            return None

        tool_rows_list = find_rows(payload)
        check(tool_rows_list is not None, "the tool answered with marketplace rows", text[:200])
        if tool_rows_list is None:
            return
        tool_rows = {r["name"]: r for r in tool_rows_list}

        check(set(tool_rows) == set(rpc_rows),
              "both faces list exactly the same registrations",
              f"tool={sorted(tool_rows)} rpc={sorted(rpc_rows)}")
        for name, row in rpc_rows.items():
            tr = tool_rows.get(name, {})
            check(tr.get("removable") == row["removable"],
                  f"'{name}': the removable bit matches the RPC face",
                  f"tool={tr.get('removable')} rpc={row['removable']}")
            check(tr.get("unremovable_reason") == row.get("unremovable_reason"),
                  f"'{name}': and so does the reason it carries")

        print("item 8b: marketplace_add both registers AND fetches")
        added = await tool({"action": "marketplace_add", "source": MARKET})
        atext = json.dumps(added)
        check("error" not in added, "add succeeded", atext[:300])
        check('"fetched":true' in atext.replace(" ", ""), "the tool reports it fetched the contents", atext[:400])

        after = await call("plugin.marketplace.list", {})
        names = [r["name"] for r in after["result"]["marketplaces"]]
        check("qa-market" in names, "the RPC face sees what the tool registered", str(names))

        print("item 8c: the classifier is the same one on the tool face")
        win = await tool({"action": "marketplace_add", "source": r"C:\dir\mk"})
        wtext = json.dumps(win)
        check("Local marketplace path does not exist" in wtext,
              "a Windows-shaped path is classified local here too",
              wtext[:300])
        bad = await tool({"action": "marketplace_add", "source": ".."})
        btext = json.dumps(bad)
        check("Invalid marketplace name" in btext,
              "and a traversal source is refused, not stored",
              btext[:300])

        print("item 8d: browse names whose install it is talking about")
        br = await tool({"action": "marketplace_browse", "marketplace": None})
        btxt = json.dumps(br)
        check("operator_can_install" in btxt,
              "the browse row says `operator_can_install`, not a bare `installable`",
              btxt[:250])
        check('"installable"' not in btxt,
              "and does not offer a bit named for an action this tool lacks")

        print("item 9: the tool will not install")
        inst = await tool({"action": "install", "name": "qa-mk-string"})
        itext = json.dumps(inst)
        check("error" in inst or "unknown variant" in itext.lower(),
              "there is no install action to call",
              itext[:250])
        # `tools.list` does not exist (-32601); the advertised catalogue is
        # `tools.catalog`. A membership assertion against a method that is not
        # there fails exactly like a missing sentence would.
        desc = await call("tools.catalog", {})
        check("error" not in desc, "the tool catalogue answered", json.dumps(desc)[:200])
        dtext = json.dumps(desc)
        check("plugin_manage" in dtext, "and plugin_manage is advertised in it")
        check("cannot install or uninstall plugins" in dtext,
              "and the advertised description says so where the model reads it")
        check("never runs anything from it" in dtext,
              "while also saying registering a catalogue executes nothing")

        # leave the fixture as we found it
        for name in ("qa-market", "mk"):
            await call("plugin.marketplace.remove", {"name": name})

    print(f"\nVERDICT: {'PASS' if rc == 0 else 'FAIL'}")
    sys.exit(rc)


asyncio.run(main())
