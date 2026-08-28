#!/usr/bin/env python3
"""Rewrite a driven transcript's stamps so they DISAGREE with the order the
rows were recorded — the shape an import, a backfill or a reconciler produces,
and the only shape that can tell the two candidate orders apart.

Run with the server stopped, between two `chat.history` calls. Everything the
server itself writes is monotonic (`MessageProjector` stamps `created_at_ms`,
which follows `seq`), so a conversation the fixture merely *drives* is served
identically by a store that ranks the stamps and a store that serves recording
order. Without this step the `order` scenario is green either way — it would be
a fixture that reports on nothing.

Descending by design: row *i* is stamped `BASE - i*60s`, so ranking the stamps
would return the transcript exactly REVERSED and truncate would cut from the
other end. A subtle scramble would leave a diff nobody can read.

Same edit, two spellings, because that is the whole subject:
  * file    — `$ROOT/data/sessions/<key>/transcript.jsonl`, one JSON object per
              line, rewritten in place preserving line order.
  * sqlite  — `$ROOT/data/sessions.db`, `UPDATE messages SET timestamp` keyed by
              `id`, which is the recording order.

Usage:  scramble_stamps.py ALEPH_HOME {file|sqlite} SESSION_KEY
"""
import glob
import json
import os
import sqlite3
import sys

# 2021-01-01T00:00:00Z in ms. Comfortably past `SECONDS_MILLIS_BOUNDARY`, so
# `stamp_millis` reads every value written here as milliseconds and the
# scramble cannot be mistaken for a units artefact.
BASE_MS = 1_609_459_200_000
STEP_MS = 60_000

home, backend, session_key = sys.argv[1], sys.argv[2], sys.argv[3]


def scramble_file():
    root = os.path.join(home, "data", "sessions")
    safe = session_key.replace("/", "_").replace("\\", "_")
    path = os.path.join(root, safe, "transcript.jsonl")
    if not os.path.exists(path):
        cand = glob.glob(os.path.join(root, "*", "transcript.jsonl"))
        raise SystemExit(f"no transcript at {path}; found {cand}")
    rows = [json.loads(l) for l in open(path) if l.strip()]
    for i, r in enumerate(rows):
        r["timestamp"] = BASE_MS - i * STEP_MS
    with open(path, "w") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    return len(rows)


def scramble_sqlite():
    db = os.path.join(home, "data", "sessions.db")
    con = sqlite3.connect(db, timeout=10)
    try:
        ids = [
            r[0]
            for r in con.execute(
                "SELECT id FROM messages WHERE session_key = ? ORDER BY id ASC",
                (session_key,),
            )
        ]
        if not ids:
            raise SystemExit(f"no messages for {session_key} in {db}")
        for i, rid in enumerate(ids):
            con.execute(
                "UPDATE messages SET timestamp = ? WHERE id = ?",
                (BASE_MS - i * STEP_MS, rid),
            )
        con.commit()
        return len(ids)
    finally:
        con.close()


n = scramble_file() if backend == "file" else scramble_sqlite()
print(f"scrambled {n} stamps on the {backend} backend (descending, {STEP_MS}ms apart)")
if n < 4:
    raise SystemExit(f"only {n} rows — too few for the truncate half to mean anything")
