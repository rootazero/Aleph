#!/usr/bin/env python3
"""Seed the fixture through the production writers, over the gateway.

Curated entries come from `remember`, notes from `note_manage` — the same two
tools an agent turn reaches, dispatched by `tools.invoke` so no model is
involved. Nothing here touches the database or the agent directory: the whole
point of both claims is that the Panel's face and the tool's face resolve to
ONE store, and a fixture that wrote that store itself would be asserting
against its own idea of where it lives.

The last curated call is a deliberate duplicate. It is refused, and the refusal
is what puts a row in `memory_write_decisions` with a server-side reason —
which is the only thing item 5's ledger can be checked against. A ledger with
no refused row in it cannot tell "refusals are shown" from "there were none".

## Both partitions are reported, and neither is assumed

A loopback session carries `Personal(u-owner)` scope, so `session_write_id`
composes `main__u-owner` and BOTH writers land there — while the Panel asks
`memory.listFacts` for whatever `agents.list` calls the default agent. Those
are not the same string, and the fixture must not paper over that: it asks
`agents.list` for the id the Panel would use, reports what the store answers
for it, and writes both that id and the curated file's own self-reported
destination to `<ALEPH_HOME>/qa-seeded.json` for `probe.py` to read. Recomputing the server's
scope resolution here would make this fixture a second answer to the exact
question the round is about.
"""
import asyncio
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc  # noqa: E402

import websockets  # noqa: E402

WS = sys.argv[1]
ALEPH_HOME = Path(sys.argv[2])
NOTE_COUNT = int(sys.argv[3]) if len(sys.argv) > 3 else 1040

# Entry 3 is Chinese so that item 2's budget reading is falsifiable: the store
# bills characters, and a byte count would show this one at three times its
# real cost.
ENTRIES = [
    "QA-1 the operator runs Aleph on a Mac Studio and prefers terse replies",
    "QA-2 the operator's timezone is Asia/Shanghai",
    "QA-3 用户偏好中文回复，代码注释用英文",
]

L = Ledger()


async def main():
    seeded = {}
    async with websockets.connect(WS, max_size=32 * 1024 * 1024) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-memory-curated-seed")

        # The id the PANEL will use — read from the same RPC the console reads
        # it from, not spelled here.
        msg = await rpc.call("agents.list", {})
        panel_agent = msg.get("result", {}).get("default_id")
        L.check("agents.list names a default agent", bool(panel_agent), str(panel_agent))
        seeded["panel_agent"] = panel_agent

        for e in ENTRIES:
            ok, body = await rpc.invoke("remember", {"action": "add", "content": e})
            L.check(f"remember add {e[:24]!r}", ok, json.dumps(body, ensure_ascii=False)[:120])
            # The tool reports where it wrote. That string is the server's own
            # answer to "which partition is this session's", and the only
            # honest way for the fixture to find the file.
            if isinstance(body, dict) and body.get("destination"):
                seeded["destination"] = body["destination"]

        # The refusal that gives the ledger something to show.
        ok, body = await rpc.invoke("remember", {"action": "add", "content": ENTRIES[0]})
        blob = json.dumps(body, ensure_ascii=False)
        L.check(
            "a duplicate add is refused (this is the ledger's seeded row)",
            "duplicate" in blob.lower(),
            blob[:160],
        )

        L.log(f"seeding {NOTE_COUNT} notes via note_manage…")
        failed = 0
        for i in range(1, NOTE_COUNT + 1):
            ok, body = await rpc.invoke(
                "note_manage",
                {
                    "action": "create",
                    "category": "reference",
                    "filename": f"qa-note-{i:04d}",
                    "title": f"QA Note {i:04d}",
                    "content": f"Seeded note {i} for the note-window growth check.",
                },
            )
            if not ok:
                failed += 1
                if failed <= 3:
                    L.log(f"  note {i} failed: {json.dumps(body, ensure_ascii=False)[:200]}")
            if i % 200 == 0:
                L.log(f"  {i}/{NOTE_COUNT}")
        L.check("every seeded note was created", failed == 0, f"{failed} failed")

        # What the store answers for the id the Panel asks about. NOT asserted
        # to equal NOTE_COUNT: that equality is one of the things the browser
        # half is here to find out, and pre-judging it in the fixture would
        # turn a product finding into a fixture failure.
        msg = await rpc.call("memory.listFacts", {"agent_id": panel_agent, "limit": 1})
        panel_total = msg.get("result", {}).get("total")
        seeded["panel_note_total"] = panel_total
        L.log(f"memory.listFacts(agent_id={panel_agent!r}).total = {panel_total}")

    # Where the curated file actually landed, per the tool's own report.
    dest = seeded.get("destination", "")
    # `destination` is a human-facing sentence ("~/.aleph/agents/X/MEMORY.md
    # (curated hot zone — …)"), so take the first token and expand `~` against
    # the SCRATCH home this fixture is running under.
    rel = dest.split()[0] if dest else ""
    if rel.startswith("~/.aleph/"):
        path = ALEPH_HOME / rel[len("~/.aleph/") :]
    else:
        path = Path(rel)
    seeded["curated_path"] = str(path)
    # The partition the WRITERS composed, taken from the tool's own report of
    # where it wrote (".../agents/<partition>/MEMORY.md"). `note_manage`
    # composes through the same `session_write_id`, so this one value names
    # both corpora.
    seeded["write_partition"] = path.parent.name
    L.check("the curated file the tool named exists", path.is_file(), str(path))
    if path.is_file():
        body = path.read_text(encoding="utf-8")
        L.check(
            "all three seeded entries are in it",
            all(e in body for e in ENTRIES),
            f"{len(body)} chars",
        )

    # Inside the scratch home, not the repo: this is run state, and it goes
    # away with the rest of the throwaway root.
    out = ALEPH_HOME / "qa-seeded.json"
    out.write_text(json.dumps(seeded, ensure_ascii=False, indent=2), encoding="utf-8")
    L.log("wrote " + str(out))
    return L.verdict()


sys.exit(asyncio.run(main()))
