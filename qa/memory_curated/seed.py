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

The two ids being different is the NORMAL state of a stock install, not a
defect: `read_partitions` resolves the base id the picker holds into the union
`[org tier, this session's partition]`, so the Panel finds what the writers
wrote without either side hard-coding the other's answer. This file therefore
asserts the equality of the COUNTS while still reporting both ids — the
identity that matters is "the reader reached the writer's rows", not "the two
strings match".

An earlier revision shipped a `relocate_notes.py` that re-keyed the corpus into
the partition the readers looked in. It was deleted with the fix: it re-keyed
`notes_index` and `notes_links` only, leaving the FTS and vector rows where the
writer put them, so every retrieval-facing surface in this fixture reported an
honest 0 → 0 funnel through a partition whose index rows had moved out from
under it. Seeding where the writer actually writes is what makes items 10-11
answerable at all.
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

# Distinctive enough that the x-ray (item 11) can be asked for it by name and
# the funnel's output count means something.
CORRECTION = "QA-FIX stop reformatting the changelog when I only asked for a version bump"

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

        # Seed one correction, so the fix queue (item 10) has a pending row to
        # show. `flag_user_correction` composes the session scope exactly like
        # `remember` and `note_manage`, which is the whole point: it is the
        # third writer whose partition the reader has to reach.
        ok, body = await rpc.invoke(
            "flag_user_correction",
            {
                # `content`, not `correction`, and the severity token is `med`
                # — both taken from `FlagUserCorrectionArgs`, which is the only
                # thing that gets a vote. Guessing either produced a Validation
                # error, which is at least loud; a fixture that guessed a
                # *valid* wrong value would have been silent.
                "content": CORRECTION,
                "severity": "med",
                "suggested_rule": "Ask before rewriting a file the user just edited.",
            },
        )
        L.check(
            "a correction was flagged (the fix queue's pending row)",
            ok,
            json.dumps(body, ensure_ascii=False)[:200],
        )
        seeded["correction"] = CORRECTION

        # What the store answers for the id the PANEL asks about. This is now an
        # assertion rather than a report: `memory.listFacts` resolves the base id
        # through `memory_scope::read_partitions`, so it must reach the rows
        # `note_manage` just composed a partition for. It answered 0 for a store
        # holding every one of these notes until that was fixed, and 0 is exactly
        # what a regression here would produce again — silently, because an empty
        # list renders as an empty list.
        msg = await rpc.call("memory.listFacts", {"agent_id": panel_agent, "limit": 1})
        panel_total = msg.get("result", {}).get("total")
        seeded["panel_note_total"] = panel_total
        L.check(
            "the Panel's note reader reaches the partition the writer wrote to",
            panel_total == NOTE_COUNT,
            f"memory.listFacts(agent_id={panel_agent!r}).total = {panel_total}, "
            f"seeded {NOTE_COUNT}",
        )

        # The stat cards read the same union. Asserted here rather than only in
        # the browser because a stat card disagreeing with its own list is the
        # phantom-page family, and a number is cheaper to check than a card.
        msg = await rpc.call("memory.stats", {"agent_id": panel_agent})
        stats = msg.get("result", {})
        L.check(
            "memory.stats agrees with the note list",
            stats.get("totalFacts") == NOTE_COUNT,
            f"totalFacts={stats.get('totalFacts')} vs {NOTE_COUNT}",
        )

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
