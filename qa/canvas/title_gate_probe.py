#!/usr/bin/env python3
"""Wire probe for the canvas title gate — the arm the Panel cannot reach.

`check_title` refuses three things: blank, over `MAX_TITLE_BYTES`, and control
characters. A browser can only produce the first two: `<input type="text">`
runs the DOM's value-sanitization algorithm, which **strips CR and LF from the
value outright**, so a person typing into the rename box can never submit a
newline. The control-character arm therefore exists for the other two writers
— the `canvas` tool and any raw JSON-RPC client — and this probe is what
exercises it on a live wire.

It also pins the property that makes the gate worth having: a refused
`SetDocMeta` leaves the document *and its revision* exactly as they were, so
nothing half-lands and a rejected batch costs nobody a revision.

    python3 qa/canvas/title_gate_probe.py [port] [--tls]

Exits non-zero and prints the first failing assertion; prints one PASS line
per case otherwise. Loopback is always operator (trust model), so no
credentials are needed.
"""

import asyncio
import json
import ssl
import sys

try:
    import websockets
except ImportError:  # pragma: no cover - operator-facing message
    sys.exit("needs the `websockets` package (pip install websockets)")

PORT = int(sys.argv[1]) if len(sys.argv) > 1 and sys.argv[1].isdigit() else 18798
TLS = "--tls" in sys.argv

# Keep in step with `aleph_protocol::canvas::MAX_TITLE_BYTES`. Deliberately not
# read from the source: this probe is meant to fail loudly if the cap moves
# without anyone revisiting what it protects.
MAX_TITLE_BYTES = 200

failures = []


def check(name, ok, detail=""):
    if ok:
        print(f"PASS  {name}")
    else:
        print(f"FAIL  {name}  {detail}")
        failures.append(name)


class Rpc:
    def __init__(self, ws):
        self.ws = ws
        self._id = 200

    async def call(self, method, params):
        """Return (result, error) — errors are data here, not exits."""
        self._id += 1
        rid = self._id
        await self.ws.send(
            json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        )
        while True:
            msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=30))
            if msg.get("id") == rid:
                return msg.get("result", {}), msg.get("error")


async def main():
    scheme = "wss" if TLS else "ws"
    ssl_ctx = None
    if TLS:
        ssl_ctx = ssl.create_default_context()
        ssl_ctx.check_hostname = False
        ssl_ctx.verify_mode = ssl.CERT_NONE

    async with websockets.connect(
        f"{scheme}://127.0.0.1:{PORT}/ws", ssl=ssl_ctx, max_size=2**24
    ) as ws:
        rpc = Rpc(ws)
        connect, err = await rpc.call("connect", {"client_info": {"name": "title-gate-probe"}})
        if err or not connect.get("authorized"):
            sys.exit(f"loopback connect not authorized: {json.dumps(err or connect)[:200]}")

        # --- writer 1: canvas.create ---------------------------------------
        _, err = await rpc.call("canvas.create", {"title": "x" * (MAX_TITLE_BYTES + 1)})
        check("create refuses an over-cap title", err is not None, json.dumps(err)[:120])

        _, err = await rpc.call("canvas.create", {"title": "one\ntwo"})
        check("create refuses a newline title", err is not None, json.dumps(err)[:120])

        _, err = await rpc.call("canvas.create", {"title": "   "})
        check("create refuses a blank title", err is not None, json.dumps(err)[:120])

        made, err = await rpc.call("canvas.create", {"title": "Title gate probe"})
        if err:
            sys.exit(f"canvas.create failed: {json.dumps(err)}")
        doc = made["canvas"]
        cid, rev = doc["id"], doc["revision"]
        check("create accepts an admissible title", doc["title"] == "Title gate probe")

        # --- writer 2: SetDocMeta ------------------------------------------
        async def set_title(title, base):
            return await rpc.call(
                "canvas.apply",
                {
                    "canvas_id": cid,
                    "base_revision": base,
                    "ops": [{"op": "set_doc_meta", "title": title}],
                },
            )

        for label, bad in (
            ("newline", "one\ntwo"),
            ("tab", "one\ttwo"),
            ("over cap", "y" * (MAX_TITLE_BYTES + 1)),
            ("blank", "  "),
        ):
            _, err = await set_title(bad, rev)
            check(f"set_doc_meta refuses a {label} title", err is not None, json.dumps(err)[:120])

        # The refusal must have cost nothing: same title, same revision.
        env, err = await rpc.call("canvas.get", {"canvas_id": cid})
        if err:
            sys.exit(f"canvas.get failed: {json.dumps(err)}")
        after = env["canvas"]
        check(
            "a refused batch leaves the document untouched",
            after["title"] == "Title gate probe" and after["revision"] == rev,
            f'title={after["title"]!r} revision={after["revision"]} (expected {rev})',
        )

        # …and an admissible one still works, from this same face.
        res, err = await set_title("Renamed over the wire", rev)
        check("set_doc_meta accepts an admissible title", err is None, json.dumps(err)[:120])
        if not err:
            check("an accepted rename bumps the revision", res["revision"] == rev + 1)

        # A title exactly at the cap is admissible — the bound is inclusive,
        # and an off-by-one here would be invisible from the UI.
        _, err = await set_title("z" * MAX_TITLE_BYTES, rev + 1)
        check("a title exactly at the cap is accepted", err is None, json.dumps(err)[:120])

        await rpc.call("canvas.delete", {"canvas_id": cid})

    print()
    print("FAILURES:", failures if failures else "none")
    sys.exit(1 if failures else 0)


asyncio.run(main())
