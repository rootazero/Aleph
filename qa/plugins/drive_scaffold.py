#!/usr/bin/env python3
"""Does the server load what `aleph plugin init` wrote?

`interfaces/cli` may not depend on `alephcore`, so the scaffolder and the
loader are two authors with no compiler between them. Round 1 found them
disagreeing: `--type nodejs` wrote `kind = "nodejs"`, which the server's
`PluginKind` rejects with `unknown variant`, so the first example in the
development guide produced a plugin that could never be installed -- while
`plugin validate` and `plugin pack` both said it was fine, because the CLI
carried its own weaker schema.

The fix gave both sides one vocabulary. What that fix still cannot prove from
inside either crate is that a scaffold, once written to disk by the real CLI,
is a document the real registry accepts. That needs both binaries.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc  # noqa: E402

import websockets  # noqa: E402

WS = sys.argv[1]
RUNTIMES = sys.argv[2].split()

L = Ledger()


def rows(payload):
    found = []

    def walk(node):
        if isinstance(node, list):
            if node and all(isinstance(x, dict) and "name" in x for x in node):
                found.append(node)
            for v in node:
                walk(v)
        elif isinstance(node, dict):
            for v in node.values():
                walk(v)

    walk(payload)
    return found[0] if found else []


async def main():
    async with websockets.connect(WS, max_size=32 * 1024 * 1024) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-scaffold")

        msg = await rpc.call("plugins.list", {})
        listing = rows(msg.get("result", {}))
        L.check("plugins.list returns rows", bool(listing), f"{len(listing)} rows")

        by_name = {r.get("name"): r for r in listing}
        L.log(f"  runtimes under test: {RUNTIMES}")

        for rt in RUNTIMES:
            name = f"qa-scaffold-{rt}"
            row = by_name.get(name)
            L.check(f"[{rt}] the scaffolded plugin is registered", row is not None,
                    json.dumps(row)[:200] if row else f"{name} absent from {sorted(by_name)}")
            if row is None:
                continue
            status = str(row.get("status", "")).lower()
            # The exact shape of the round-1 bug: the row exists, and it is an
            # Error row carrying `unknown variant`.
            L.check(f"[{rt}] it did not land as an Error row",
                    "error" not in status, f"status={status!r} row={json.dumps(row)[:200]}")
            kind = str(row.get("kind", ""))
            L.check(f"[{rt}] the server reports a runtime it knows",
                    kind in RUNTIMES, f"kind={kind!r}")
            # The scaffolder chose the runtime; the loader must agree it is
            # that one, not merely that it is *some* known one.
            L.check(f"[{rt}] the loaded runtime is the one the scaffolder asked for",
                    kind == rt, f"asked {rt!r}, loaded {kind!r}")

    return L.verdict()


if __name__ == "__main__":
    import asyncio
    sys.exit(asyncio.run(main()))
