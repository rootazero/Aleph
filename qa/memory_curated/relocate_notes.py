#!/usr/bin/env python3
"""Move the seeded note corpus into the partition the Panel's note list reads.

## Why this step exists at all — it is a finding, not a convenience

Every memory WRITER composes the session's scope (`project_scope::session_write_id`),
and a loopback Panel session resolves to `Personal(u-owner)`. So notes written
from a stock single-machine install land in `main__u-owner`.

The Panel's note readers do not compose: `memory.listFacts`, `memory.stats` and
the graph handlers take the base id the agent picker holds (`main`) and query
that partition exactly. On the machine this fixture just built, `note_manage`
created 1040 notes and `memory.listFacts(agent_id="main").total` answered **0**
— the Vault's note list, fact/feedback/lesson facets, graph and stat cards are
all structurally empty on a stock install.

That is the same defect family the tool face was swept for twice already
(FEATURE_LOCATOR §5.22 round-3 ②, §5.22 ⑪): *the read path must compose the
same way the write path does*. The gateway RPC readers were never in either
sweep. Fixing them is a sweep of its own — the established answer there is
`session_read_ids` (the union of base and scope), which needs multi-partition
list/count support in the backend and a note identity that is `(agent_id, path)`
rather than `path` alone, because the same path can exist in two partitions.

So this fixture does NOT paper over the defect by pretending the count is fine.
It re-keys the corpus the real writer produced into the partition the reader
looks in, so the *pagination* claim (items 7-9) can be tested at all. The rows
are exactly what `index_note` wrote; only the partition column and the corpus
directory move.

## What this deliberately does NOT move, and what that costs

Only the two tables the LIST face reads (`notes_index`, `notes_links`) and the
markdown corpus. The FTS and vector rows keep their original partition, so
every RETRIEVAL-facing surface — `memory.search`, `graph.search`, the retrieval
x-ray — probes a partition whose index rows have moved out from under it and
honestly reports finding nothing. A run of this fixture therefore says nothing
about retrieval quality; do not read an empty funnel here as a product defect.
Items 7-9 are unaffected: `memory.listFacts` reads `notes_index` alone.
"""
import argparse
import shutil
import sqlite3
import sys
from pathlib import Path

ap = argparse.ArgumentParser()
ap.add_argument("aleph_home")
ap.add_argument("--from-agent", required=True)
ap.add_argument("--to-agent", required=True)
args = ap.parse_args()

home = Path(args.aleph_home)
db_path = home / "data" / "memory.db"
if not db_path.is_file():
    print(f"no memory db at {db_path}", file=sys.stderr)
    sys.exit(1)

conn = sqlite3.connect(str(db_path))
before = conn.execute(
    "SELECT COUNT(*) FROM notes_index WHERE agent_id = ?", (args.from_agent,)
).fetchone()[0]
if before == 0:
    print(f"nothing indexed under {args.from_agent!r} — refusing a silent no-op", file=sys.stderr)
    sys.exit(1)

conn.execute(
    "UPDATE notes_index SET agent_id = ? WHERE agent_id = ?", (args.to_agent, args.from_agent)
)
conn.execute(
    "UPDATE notes_links SET agent_id = ? WHERE agent_id = ?", (args.to_agent, args.from_agent)
)
conn.commit()
after = conn.execute(
    "SELECT COUNT(*) FROM notes_index WHERE agent_id = ?", (args.to_agent,)
).fetchone()[0]
conn.close()

# The markdown corpus moves with the index rows: a row whose file is not where
# its partition says it is would make the drawer's "open this note" fail for a
# reason that has nothing to do with what is being tested.
src = home / "memory" / "note" / args.from_agent
dst = home / "memory" / "note" / args.to_agent
if src.is_dir():
    dst.parent.mkdir(parents=True, exist_ok=True)
    if dst.is_dir():
        for child in src.iterdir():
            shutil.move(str(child), str(dst / child.name))
        src.rmdir()
    else:
        shutil.move(str(src), str(dst))

print(f"relocated {before} note rows {args.from_agent} -> {args.to_agent} (now {after})")
