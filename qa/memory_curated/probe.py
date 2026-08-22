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

The two retrieval phases (`xray`, `fixes`) are the exception that proves the
rule: there is no second face to ask, because the retrieval funnel and the
correction queue each have exactly one reader. So they assert against what the
SEED wrote — a specific note body and a specific correction string, both put
there by the production writers — rather than against the reader's own idea of
what it should have found.

Phases:
  baseline                      what the seed left behind
  after-edit --new T --old T    item 3
  after-remove --gone T         item 4
  ledger                        item 5/6 — how many write-decision rows exist
  notes                         item 7/8 — the store's note total
  xray                          item 11 — the retrieval funnel, stage by stage
  fixes                         item 10 — the correction queue's pending row
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
ap.add_argument(
    "phase",
    choices=[
        "baseline", "after-edit", "after-remove", "ledger",
        "notes", "xray", "fixes", "addressing",
    ],
)
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

        elif args.phase == "xray":
            # `memory.retrieve_with_trace` is the ONLY reader of this funnel, so
            # there is no second face to cross-check against. The oracle is the
            # seed instead: it created `qa-note-0001` through `note_manage`, so a
            # search for its body text has a known-present target. A funnel whose
            # every stage reports 0 in / 0 out here means the reader is probing a
            # partition the writer never wrote to — which is exactly what it did
            # before `read_partitions`, and exactly what a regression looks like.
            msg = await rpc.call(
                "memory.retrieve_with_trace",
                {"agent_id": AGENT, "query": "Seeded note for the note-window growth check", "limit": 10},
            )
            res = msg.get("result", {})
            stages = res.get("trace", {}).get("stages", [])
            results = res.get("results", [])
            L.log(f"stages: {len(stages)}")
            for st in stages:
                L.log(
                    f"  {st.get('name')}: {st.get('input_count')} -> "
                    f"{st.get('output_count')} ({st.get('duration_ms')}ms)"
                )
            L.check(
                "the funnel reports at least one stage",
                bool(stages),
                json.dumps(res.get("trace", {}), ensure_ascii=False)[:200],
            )
            L.check(
                "some stage actually retrieved something (not a 0 -> 0 funnel)",
                any(int(st.get("output_count") or 0) > 0 for st in stages),
                "; ".join(
                    f"{st.get('name')}={st.get('input_count')}->{st.get('output_count')}"
                    for st in stages
                ),
            )
            L.check(
                "the seeded notes are what came back",
                any("Seeded note" in (r.get("content") or "") for r in results),
                json.dumps(results[:2], ensure_ascii=False)[:300],
            )
            print(f"XRAY_STAGES={len(stages)} XRAY_RESULTS={len(results)}", flush=True)

        elif args.phase == "fixes":
            correction = seeded().get("correction", "")
            msg = await rpc.call(
                "memory.list_corrections",
                {"agent_id": AGENT, "limit": 50, "include_distilled": True},
            )
            rows = msg.get("result", {}).get("corrections", [])
            L.log(f"corrections: {len(rows)}")
            for r in rows[:5]:
                L.log(f"  [{r.get('status')}] {r.get('severity')} / {r.get('content', '')[:70]}")
            L.check(
                "the correction the seed flagged is in the queue",
                any(correction in (r.get("content") or "") for r in rows),
                f"looking for {correction[:50]!r} in {len(rows)} rows",
            )
            L.check(
                "it is pending, not distilled (no dream cycle has run)",
                any(
                    correction in (r.get("content") or "") and r.get("status") == "pending"
                    for r in rows
                ),
                json.dumps(rows[:2], ensure_ascii=False)[:300],
            )
            print(f"FIX_ROWS={len(rows)}", flush=True)

        elif args.phase == "addressing":
            # Checklist item 12 asks whether the DRAWER used the row's own
            # partition or the agent picker's id. You cannot see which argument
            # the client sent -- so instead find a server verb where the two
            # candidate arguments give DIFFERENT answers, and ask it both ways.
            #
            # `graph.node_detail` is that verb: it is an ADDRESSING verb, so it
            # takes the partition verbatim and never resolves a union (that is
            # the whole distinction `memory_scope::read_partitions` exists to
            # draw). The picker's bare id therefore cannot reach a note the
            # writers composed into a session partition, and if the drawer
            # rendered a body, the id it sent can only have come from the row.
            #
            # Keep this asymmetric on purpose: if BOTH ids started answering,
            # the control stops separating them and item 12 becomes untestable
            # -- so the failure of the picker id is an assertion, not a bug.
            node = "reference/qa-note-0001"
            picker = AGENT
            row = seeded().get("write_partition", "")
            L.log(f"picker id {picker!r} vs row id {row!r}")
            L.check(
                "the two ids actually differ (else this control proves nothing)",
                bool(row) and row != picker,
                f"{picker!r} vs {row!r}",
            )

            miss = await rpc.call("graph.node_detail", {"node_id": node, "agent_id": picker})
            hit = await rpc.call("graph.node_detail", {"node_id": node, "agent_id": row})

            def body_of(msg):
                return ((msg.get("result") or {}).get("content") or "")

            L.check(
                "the picker's bare id cannot reach the note (addressing is verbatim)",
                "error" in miss or not body_of(miss),
                json.dumps(miss.get("error") or miss.get("result"), ensure_ascii=False)[:160],
            )
            L.check(
                "the row's own partition reaches it",
                bool(body_of(hit)),
                body_of(hit)[:80].replace("\n", " "),
            )
            print(f"ADDRESSING picker={'miss' if not body_of(miss) else 'hit'} "
                  f"row={'hit' if body_of(hit) else 'miss'}", flush=True)

    return L.verdict()


sys.exit(asyncio.run(main()))
