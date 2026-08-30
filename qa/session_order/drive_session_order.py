#!/usr/bin/env python3
"""Real-machine QA for the transcript's order and for `session.truncate`.

Two claims a unit test structurally cannot make, both about a booted server:

  1. **The server's `session_store_backend` really selects a backend, and both
     serve one conversation identically.** The unit tests build both stores in
     one process — the right way to compare them, and blind to the config that
     picks one. `default_session_store_backend()` returned `"file"` while the
     doc beside it said `"sqlite" (default)`; that gap is this shape.

  2. **`session.truncate` reaches the database on the SQLite backend.** It had
     two `unchecked_transaction()` calls, the first shadowed rather than
     committed, so the second `BEGIN` ran inside it and SQLite refused —
     `cannot start a transaction within a transaction`. Every call returned
     INTERNAL_ERROR; `/undo` had never once succeeded there. Only the RPC face
     proves the handler path, and only a sqlite-configured server reaches it.

Run per backend; `run.sh` diffs the two JSON outputs.

The middle step is what makes this discriminating. Everything the server
writes is stamped monotonically, so a stamp-ranking store and a
recording-order store agree on any conversation this fixture merely drives.
`scramble_stamps.py` runs between the two `chat.history` calls, with the server
stopped, and rewrites the stamps DESCENDING — after which a store that ranks
them serves the transcript reversed and truncates from the wrong end.

Usage:  drive_session_order.py WS_URL DB OUT_JSON {before|after}
"""
import argparse
import asyncio
import json
import sys

import websockets

sys.path.insert(0, __file__.rsplit("/", 1)[0] + "/../busy_input")
from lib import SessionLog, log, reply, rpc  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("url")
ap.add_argument("db")
ap.add_argument("out")
ap.add_argument("phase", choices=["before", "after"])
ap.add_argument("--session-key", default="")
ap.add_argument("--turns", type=int, default=4)
ap.add_argument("--keep", type=int, default=4)
args = ap.parse_args()

CHANNEL = "gui:qa-session-order"


async def history(ws, key, rid):
    await rpc(ws, "chat.history", {"session_key": key, "limit": 200}, rid)
    r = await reply(ws, rid, budget=30)
    if "result" not in r:
        return {"error": r.get("error")}
    res = r["result"]
    rows = res.get("messages") or res.get("history") or []
    return {
        "total": res.get("total"),
        "count": len(rows),
        # role+content only: `id` is a row id on one backend and the producer's
        # string on the other, and comparing those would report a divergence
        # that is not one.
        "rows": [(m.get("role"), m.get("content")) for m in rows],
    }


async def drive(ws, slog):
    """`--turns` sends, each answered by the `single-shot` plan with exactly one
    priced call, so the transcript is `user, assistant` repeated and every
    assistant row carries a different `mock turn N`."""
    key = ""
    for i in range(args.turns):
        rid = 100 + i
        params = {"message": f"probe {i}", "channel": CHANNEL}
        if key:
            params["session_key"] = key
        await rpc(ws, "chat.send", params, rid)
        r = await reply(ws, rid, budget=60)
        if "result" not in r:
            raise SystemExit(f"chat.send {i} rejected: {r}")
        key = r["result"]["session_key"]
        if await slog.wait_for("run_finished", i + 1, 180) is None:
            raise SystemExit(f"run {i} never finished")
    return key


async def main():
    out = {"phase": args.phase}
    async with websockets.connect(args.url, max_size=None) as ws:
        await rpc(ws, "connect", {"client_info": {"name": "qa-session-order"}}, 1)
        r = await reply(ws, 1)
        log("connect ->", r["result"]["role"])
        slog = SessionLog(args.db)

        if args.phase == "before":
            key = await drive(ws, slog)
            out["session_key"] = key
            out["history"] = await history(ws, key, 200)
        else:
            key = args.session_key
            if not key:
                raise SystemExit("--session-key is required for the `after` phase")
            out["session_key"] = key
            out["history"] = await history(ws, key, 200)

            await rpc(
                ws,
                "session.truncate",
                {"session_key": key, "keep_count": args.keep},
                300,
            )
            t = await reply(ws, 300, budget=30)
            out["truncate"] = t.get("result") or {"error": t.get("error")}
            out["history_after_truncate"] = await history(ws, key, 301)

    json.dump(out, open(args.out, "w"), indent=2)
    log(json.dumps(out, indent=2)[:2000])


asyncio.run(main())
