#!/usr/bin/env python3
"""Out-of-band oracle for the checklist's checkpoints.

Everything here deliberately avoids the two RPCs under test
(`memory.curated.*`): an assertion that the Panel wrote something, checked by
asking the same handler that did the write, only proves the handler is
self-consistent. So the questions are put to the two faces that are NOT the
one being driven —

  * the FILE on disk (`MEMORY.md`, found by walking the scratch home rather
    than by recomputing the server's scope resolution), and
  * the TOOL face (`remember`), whose store must be the same object the Panel
    just mutated. A duplicate `add` is the cheapest way to ask it: the tool
    can only call the text a duplicate if it can already see it.

Phases:
  baseline                      what the seed left behind
  after-edit --new T --old T    item 3
  after-remove --gone T         item 4
  ledger                        item 5/6 — how many write-decision rows exist
  notes                         item 7/8 — the store's note total
"""
import argparse
import asyncio
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc  # noqa: E402

import websockets  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("ws")
ap.add_argument("home")
ap.add_argument("phase", choices=["baseline", "after-edit", "after-remove", "ledger", "notes"])
ap.add_argument("--new", default=None)
ap.add_argument("--old", default=None)
ap.add_argument("--gone", default=None)
args = ap.parse_args()

L = Ledger()

# The id the Panel sends, recorded by the seed from `agents.list`.
AGENT = json.loads(
    (Path(args.home) / "qa-seeded.json").read_text(encoding="utf-8")
)["panel_agent"]


def seeded() -> dict:
    """What the seed recorded — the server's own answers, not this file's.

    In particular the curated path: a fresh loopback install has TWO
    `MEMORY.md` files (`agents/main/` from provisioning, `agents/main__u-owner/`
    from the personal-scope composition), and picking between them here would
    be a second, hand-rolled answer to the scope question the round is about.
    """
    return json.loads((Path(args.home) / "qa-seeded.json").read_text(encoding="utf-8"))


def curated_file() -> Path:
    return Path(seeded()["curated_path"])


async def main():
    body = curated_file().read_text(encoding="utf-8")
    L.log(f"agent id (as the Panel resolves it): {AGENT!r}")

    async with websockets.connect(args.ws, max_size=32 * 1024 * 1024) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-memory-curated-probe")

        if args.phase == "baseline":
            L.log(f"--- {curated_file()} ---")
            L.log(body)
            msg = await rpc.call(
                "memory.trace",
                {"agent_id": AGENT, "target": "", "kind": "write_decision", "max_results": 50},
            )
            rows = msg.get("result", {}).get("write_decisions", [])
            L.log(f"ledger rows: {len(rows)}")
            for r in rows:
                L.log(f"  {r.get('action')} / {r.get('reason')} / {r.get('subject', '')[:60]}")
            msg = await rpc.call("memory.listFacts", {"agent_id": AGENT, "limit": 1})
            L.log(f"note total: {msg.get('result', {}).get('total')}")

        elif args.phase == "after-edit":
            L.check("the new text is in MEMORY.md", args.new in body, args.new[:60])
            L.check("the old text is gone from MEMORY.md", args.old not in body, args.old[:60])
            # The tool face, asked about the text the PANEL wrote. A refusal
            # naming it a duplicate is only possible if `remember` resolved to
            # the same store the RPC handler mutated.
            _, res = await rpc.invoke("remember", {"action": "add", "content": args.new})
            blob = json.dumps(res, ensure_ascii=False)
            L.check(
                "the `remember` tool sees the Panel's edit (refuses it as a duplicate)",
                "duplicate" in blob.lower(),
                blob[:200],
            )

        elif args.phase == "after-remove":
            L.check("the removed text is gone from MEMORY.md", args.gone not in body, args.gone[:60])
            # And the tool can add it back — i.e. it is genuinely absent from
            # the tool's view too, not merely absent from the file.
            ok, res = await rpc.invoke("remember", {"action": "add", "content": args.gone})
            L.check(
                "the `remember` tool no longer knows it (a re-add is accepted)",
                ok,
                json.dumps(res, ensure_ascii=False)[:200],
            )
            # Put the store back the way the checklist left it.
            await rpc.invoke("remember", {"action": "remove", "old_text": args.gone})

        elif args.phase == "ledger":
            msg = await rpc.call(
                "memory.trace",
                {"agent_id": AGENT, "target": "", "kind": "write_decision", "max_results": 100},
            )
            rows = msg.get("result", {}).get("write_decisions", [])
            print(f"LEDGER_ROWS={len(rows)}", flush=True)
            for r in rows[:10]:
                L.log(f"  {r.get('action')} / {r.get('reason')} / {r.get('subject', '')[:60]}")
            return 0

        elif args.phase == "notes":
            msg = await rpc.call("memory.listFacts", {"agent_id": AGENT, "limit": 1})
            total = msg.get("result", {}).get("total")
            print(f"NOTE_TOTAL={total}", flush=True)
            return 0

    return L.verdict()


sys.exit(asyncio.run(main()))
