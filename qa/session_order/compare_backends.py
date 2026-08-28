#!/usr/bin/env python3
"""Judge the four captures. Exit code = number of failed claims.

Per backend:
  * the drive produced `2 x turns` rows, user/assistant alternating, assistant
    texts ascending — the control. Without it every "order unchanged" claim
    below is satisfiable by a store that returns nothing.
  * `total` equals the row count for an unwindowed read.
  * a DESCENDING stamp scramble does not change the served order. A store that
    ranks the stamps returns this reversed.
  * `session.truncate` succeeds — not "does not crash": it must carry a result,
    because the SQLite half answered INTERNAL_ERROR to every call ever made.
  * it keeps the HEAD of that order, which is the file store's
    `drain(keep_count..)` and the cut `/undo` means.

Across backends: identical rows before, after the scramble, and after the cut.
That is the claim of the round — one conversation cannot have two orders, and
the DELETE makes disagreement destructive rather than cosmetic.

Usage:  compare_backends.py FILE_BEFORE FILE_AFTER SQLITE_BEFORE SQLITE_AFTER TURNS KEEP
"""
import json
import re
import sys

fb, fa, sb, sa, turns, keep = sys.argv[1:7]
turns, keep = int(turns), int(keep)
load = lambda p: json.load(open(p))
caps = {
    "file": (load(fb), load(fa)),
    "sqlite": (load(sb), load(sa)),
}

failures = []


def check(ok, claim, evidence=""):
    print(("  PASS  " if ok else "  FAIL  ") + claim)
    if evidence:
        print(f"          {evidence}")
    if not ok:
        failures.append(claim)
    return ok


def rows(cap, field="history"):
    return [tuple(r) for r in cap[field]["rows"]]


for backend, (before, after) in caps.items():
    print(f"\n--- {backend}")
    b = rows(before)
    check(
        len(b) == 2 * turns,
        f"[{backend}] the drive recorded {2 * turns} rows",
        f"got {len(b)}: {b[:4]}",
    )
    check(
        [r[0] for r in b] == ["user", "assistant"] * turns,
        f"[{backend}] rows alternate user/assistant",
        f"roles={[r[0] for r in b]}",
    )
    # `mock turn N: still working.` — the `single-shot` plan's streamed text,
    # not the plain `mock turn N` the module doc's summary suggests. N is also
    # NOT contiguous across sends (measured: 2, 4, 5, 6 for four sends): the
    # counter advances for every request that carries a tool surface, and a run
    # makes side-channel calls that do. Ascending is the property; consecutive
    # is not, and asserting it would be asserting something about the mock.
    assistants = [c for role, c in b if role == "assistant"]
    ns = [int(m.group(1)) for c in assistants if (m := re.search(r"mock turn (\d+)", c))]
    check(
        len(ns) == len(assistants),
        f"[{backend}] every assistant row is a recognisable mock turn",
        f"parsed {len(ns)} of {len(assistants)}: {assistants}",
    ) and check(
        ns == sorted(ns),
        f"[{backend}] the assistant turns are in ascending order",
        f"{ns}",
    )
    check(
        before["history"]["total"] == len(b),
        f"[{backend}] total equals the row count for an unwindowed read",
        f'total={before["history"]["total"]} count={len(b)}',
    )

    a = rows(after)
    check(
        a == b,
        f"[{backend}] a descending stamp scramble did not reorder the "
        f"transcript",
        f"before={[c for _, c in b]}\n          after ={[c for _, c in a]}",
    )

    tr = after.get("truncate") or {}
    check(
        "error" not in tr,
        f"[{backend}] session.truncate reached the database",
        f"{tr}",
    )
    check(
        tr.get("messages_removed") == 2 * turns - keep,
        f"[{backend}] session.truncate removed the {2 * turns - keep} rows past "
        f"the keep line",
        f"{tr}",
    )
    cut = rows(after, "history_after_truncate")
    check(
        cut == b[:keep],
        f"[{backend}] session.truncate kept the HEAD of the recorded order",
        f"kept  ={[c for _, c in cut]}\n          wanted={[c for _, c in b[:keep]]}",
    )

print("\n--- across backends")
fbc, fac = caps["file"]
sbc, sac = caps["sqlite"]
check(
    rows(fbc) == rows(sbc),
    "the same driven conversation reads identically on both backends",
    f'file  ={[c for _, c in rows(fbc)]}\n          sqlite={[c for _, c in rows(sbc)]}',
)
check(
    rows(fac) == rows(sac),
    "and still does after the stamps disagree with the recording order",
)
check(
    rows(fac, "history_after_truncate") == rows(sac, "history_after_truncate"),
    "session.truncate destroys the same rows on both backends",
    f'file  ={[c for _, c in rows(fac, "history_after_truncate")]}\n'
    f'          sqlite={[c for _, c in rows(sac, "history_after_truncate")]}',
)

print(f"\n{len(failures)} failure(s)")
for f in failures:
    print(f"  - {f}")
sys.exit(len(failures))
