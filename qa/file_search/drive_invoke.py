#!/usr/bin/env python3
"""`grep` / `find` driven through the real gateway, on a real tree.

Two claim groups, both reached over `tools.invoke` because that surface
dispatches straight off the live `ToolRegistry` — which is precisely what the
in-process tests cannot do. Every unit test next to these tools calls
`GrepTool::run` directly, so all of them stay green for a tool that is
registered three times and dispatched zero (the `plugin_manage` shape). One
call over the wire is the narrowest thing that says otherwise.

  floor  — what the walk refuses to read, and whether it says so. The unit
           tests hand `denied_paths` in by hand, so they prove the predicate
           and nothing about the wiring; only a booted server proves that
           `[sandbox] deny_read_globs` from a config file reaches
           `get_denied_paths()`. The sharp assertion is the third one:
           `no_ignore: true` lifts the ignore rules and must NOT lift the
           protected-location floor.

  page   — the window says how big the thing was that it was cut from, and
           consecutive pages are disjoint. Over the wire this also exercises
           `next_offset`'s `skip_serializing_if`, which is the difference
           between "last page" and "field lost in transit".
"""
import asyncio
import json
import sys

import websockets

URL, PHASE, TREE, EXPECT_JSON = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
EXPECT = json.loads(EXPECT_JSON)
NEEDLE = EXPECT["needle"]

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
            await ws.send(
                json.dumps({"jsonrpc": "2.0", "id": n[0], "method": method, "params": params})
            )
            while True:
                msg = json.loads(await asyncio.wait_for(ws.recv(), timeout=60))
                if msg.get("id") == n[0]:
                    return msg

        async def tool(name, arguments):
            r = await call("tools.invoke", {"tool_name": name, "arguments": arguments})
            if "error" in r:
                return None, json.dumps(r["error"])[:300]
            return r["result"]["result"], ""

        await call("connect", {"client": "qa-file-search", "version": "1"})

        if PHASE == "floor":
            await floor(tool)
        elif PHASE == "page":
            await page(tool)
        else:
            check(False, f"unknown phase {PHASE}")


async def floor(tool):
    probe = f"{TREE}/probe"

    # The dispatch proof rides on this call: a missing arm in
    # `ToolRegistry::execute_tool` answers `Unknown tool` here, and no amount
    # of registration-face testing in-process would have said so.
    out, err = await tool("grep", {"pattern": NEEDLE, "path": probe})
    if out is None:
        check(False, "grep reached the registry over the wire", err)
        return
    check(True, "grep reached the registry over the wire")

    matches = out["matches"]
    check(
        out["total_matches"] == EXPECT["visible_matches"]
        and out["files_with_matches"] == EXPECT["visible_files"],
        "ignore rules bound the search",
        f"{out['total_matches']} match(es) in {out['files_with_matches']} file(s), "
        f"expected {EXPECT['visible_matches']}/{EXPECT['visible_files']}",
    )
    check("src/alpha.rs:" in matches, "a tracked source file was found", matches[:120])
    check("generated/" not in matches, "the .gitignore'd build output stayed out")
    check("node_modules/" not in matches, "the generated-dir floor held with no .gitignore entry")
    check(".pem" not in matches, "the protected location stayed out")

    msg = out["message"]
    check("no_ignore=true" in msg, "the message names the ignore lever", msg)
    # This clause is the live-config oracle: it can only be non-zero if
    # `[sandbox] deny_read_globs` from the config file reached
    # `get_denied_paths()` at tool construction.
    check(
        f"{EXPECT['withheld']} path(s) withheld" in msg,
        "the message names what the deny floor withheld",
        msg,
    )

    # The sharp one. `no_ignore` is the lever for "search the generated trees
    # too"; a caller who pulls it must not thereby pull the credential floor.
    out, err = await tool("grep", {"pattern": NEEDLE, "path": probe, "no_ignore": True})
    if out is None:
        check(False, "grep no_ignore reached the registry", err)
        return
    check(
        out["total_matches"] == EXPECT["no_ignore_matches"]
        and out["files_with_matches"] == EXPECT["no_ignore_files"],
        "no_ignore reached the ignored and generated trees",
        f"{out['total_matches']} match(es) in {out['files_with_matches']} file(s), "
        f"expected {EXPECT['no_ignore_matches']}/{EXPECT['no_ignore_files']}",
    )
    check(
        ".pem" not in out["matches"]
        and f"{EXPECT['withheld']} path(s) withheld" in out["message"],
        "no_ignore did NOT lift the protected-location floor",
        out["message"],
    )

    # `find` is the other face of the same walk, and it binds the same floor.
    out, err = await tool("find", {"pattern": "*.pem", "path": probe, "no_ignore": True})
    if out is None:
        check(False, "find reached the registry over the wire", err)
        return
    check(out["total"] == 0, "find does not list the protected location", out["paths"][:200])

    out, _ = await tool("find", {"pattern": "*.rs", "path": probe})
    check(
        out is not None and out["total"] == EXPECT["rs_files"],
        "find still lists the files it should",
        "" if out is None else out["paths"].replace("\n", " "),
    )


async def page(tool):
    pages = f"{TREE}/pages"
    total = EXPECT["page_matches"]

    first, err = await tool("grep", {"pattern": NEEDLE, "path": pages, "limit": 60})
    if first is None:
        check(False, "grep page 1", err)
        return
    check(
        first["returned"] == 60 and first["total_matches"] == total,
        "page 1 is a window that reports the whole",
        f"returned={first['returned']} total={first['total_matches']} expected total={total}",
    )
    check(first.get("next_offset") == 60, "page 1 carries the cursor", str(first.get("next_offset")))

    second, err = await tool(
        "grep", {"pattern": NEEDLE, "path": pages, "limit": 60, "offset": first["next_offset"]}
    )
    if second is None:
        check(False, "grep page 2", err)
        return
    a = set(first["matches"].splitlines())
    b = set(second["matches"].splitlines())
    check(not (a & b), "consecutive pages are disjoint", f"{len(a & b)} shared line(s)")
    check(
        second["total_matches"] == total,
        "the reported whole does not move between pages",
        str(second["total_matches"]),
    )

    last, err = await tool(
        "grep", {"pattern": NEEDLE, "path": pages, "limit": 60, "offset": 120}
    )
    if last is None:
        check(False, "grep last page", err)
        return
    check(
        last["returned"] == total - 120 and "next_offset" not in last,
        "the last page says it is the last",
        f"returned={last['returned']} next_offset={last.get('next_offset')}",
    )
    check(
        len(a | b | set(last["matches"].splitlines())) == total,
        "the three pages together are the whole",
        str(len(a | b | set(last["matches"].splitlines()))),
    )

    files, err = await tool("grep", {"pattern": NEEDLE, "path": pages, "files_only": True})
    if files is None:
        check(False, "grep files_only", err)
        return
    check(
        files["returned"] == EXPECT["page_files"]
        and len(files["matches"].splitlines()) == EXPECT["page_files"],
        "files_only pages over files, not matches",
        f"returned={files['returned']} lines={len(files['matches'].splitlines())}",
    )


asyncio.run(main())
sys.exit(rc)
