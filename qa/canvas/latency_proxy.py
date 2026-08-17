#!/usr/bin/env python3
"""Upstream-latency TCP proxy — the instrument that makes the real conflict
window drivable (checklist item 3).

Why it exists
-------------
Two browser tabs driven serially over MCP can never lose the optimistic-lock
race: frame propagation on loopback is <100 ms, so by the time the "other"
tab commits, the first tab has already reconciled the broadcast and sends a
fresh `base_revision`. The 2026-08-17 QA round therefore verified item 3's
convergence but never saw the conflict arm fire on a real wire (spec §8).

This proxy stretches exactly the half of the wire that creates the window:
**client→server (upstream) traffic is delayed by a fixed amount; the
server→client half passes through untouched.** Open tab A through the proxy
and tab B directly, edit the same shape in A then immediately in B — B lands
first, A's already-in-flight `canvas.apply` arrives carrying the now-stale
revision, and the server refuses it with `REVISION_CONFLICT` for real. A's
recovery (refetch + replay + resend) then runs on the genuine wire.

Delaying upstream only is deliberate: A still receives B's broadcast
instantly, which pins the other correctness claim — an in-flight batch is
NOT rebased by a broadcast that arrives after send (`base_revision` is read
at send time; the doc signal's revision is server truth).

Oracle
------
Server→client WebSocket text frames are unmasked and (with this gateway)
uncompressed, so the refusal is greppable in the raw stream: the proxy
prints `CONFLICT FRAME SEEN` the moment the downstream stream carries the
`REVISION_CONFLICT` code. That line is the positive proof the conflict arm
fired — pair it with the effect assertions (both edits present in both tabs
and in doc.json afterwards).

The scan is over the *stream*, not over each TCP chunk: `ConflictScanner`
carries `len(marker) - 1` bytes across reads, so a marker split across a
64 KiB boundary is still found exactly once. (Carrying one byte fewer than
the marker is what makes it exactly once — no complete marker can be
re-formed out of the carry alone, so an occurrence cannot be double-counted.)
`--self-test` drives that boundary case directly.

What absence still does NOT prove: the oracle reads plaintext, so it is blind
to a downstream frame that was compressed (`permessage-deflate`) or otherwise
re-encoded. If the line never appears, cross-check `doc.json` revisions
before concluding no conflict occurred.

Origin note: the gateway's `/ws` origin policy allows any loopback origin
regardless of port (`origin_policy.rs::is_loopback_host`), so a page served
from the proxy port needs no config change.

Usage
-----
    python3 qa/canvas/latency_proxy.py <listen_port> <gateway_port> [delay_ms]

    # e.g. gateway on 18798, proxy on 18799, 2.5 s upstream delay:
    python3 qa/canvas/latency_proxy.py 18799 18798 2500
    # tab A: http://127.0.0.1:18799   tab B: http://127.0.0.1:18798

Every request from tab A (page assets included) pays the delay — sluggish by
design; drive A's edit first, then B's within the delay window.
"""

import asyncio
import sys
import time

_ARGS = [a for a in sys.argv[1:] if not a.startswith("--")]
LISTEN_PORT = int(_ARGS[0]) if len(_ARGS) > 0 else 18799
TARGET_PORT = int(_ARGS[1]) if len(_ARGS) > 1 else 18798
DELAY_S = (float(_ARGS[2]) if len(_ARGS) > 2 else 2500.0) / 1000.0

# `"code":-32031` — aleph_protocol::jsonrpc::REVISION_CONFLICT as it appears
# in a JSON-RPC error response. Scanned only on the unmasked downstream half.
CONFLICT_MARKER = b"-32031"


def log(line: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {line}", flush=True)


async def pump_delayed(reader, writer, delay: float) -> None:
    """client→server: every chunk is released `delay` seconds after arrival.

    A single queue + one writer task keeps chunk order — a per-chunk
    `create_task(sleep; write)` would let reordering corrupt the TCP stream.
    """
    queue: asyncio.Queue = asyncio.Queue()

    async def drain_queue() -> None:
        loop = asyncio.get_event_loop()
        while True:
            due, chunk = await queue.get()
            if chunk is None:
                break
            now = loop.time()
            if due > now:
                await asyncio.sleep(due - now)
            writer.write(chunk)
            try:
                await writer.drain()
            except (ConnectionError, OSError):
                break
        try:
            writer.close()
            await writer.wait_closed()
        except (ConnectionError, OSError):
            pass

    writer_task = asyncio.create_task(drain_queue())
    loop = asyncio.get_event_loop()
    try:
        while True:
            chunk = await reader.read(65536)
            if not chunk:
                break
            await queue.put((loop.time() + delay, chunk))
    except (ConnectionError, OSError):
        pass
    finally:
        await queue.put((0.0, None))
        await writer_task


class ConflictScanner:
    """Counts `marker` occurrences in a byte *stream* read in arbitrary chunks.

    A TCP read boundary is not a frame boundary, so scanning each chunk in
    isolation misses any marker that straddles one — which for a 6-byte needle
    is rare enough to never show up in a QA run and still leave the oracle
    quietly unsound. Keeping the trailing `len(marker) - 1` bytes as context
    makes the scan exact: every occurrence is found, and none is found twice
    (a complete marker cannot fit inside the carry alone).
    """

    def __init__(self, marker: bytes) -> None:
        self.marker = marker
        self.carry = b""
        self.count = 0

    def feed(self, chunk: bytes) -> int:
        """Returns how many new occurrences this chunk completed."""
        window = self.carry + chunk
        found = window.count(self.marker)
        self.count += found
        keep = len(self.marker) - 1
        self.carry = window[-keep:] if keep else b""
        return found


async def pump_direct(reader, writer) -> None:
    """server→client: pass through untouched, watching for the conflict code."""
    scanner = ConflictScanner(CONFLICT_MARKER)
    try:
        while True:
            chunk = await reader.read(65536)
            if not chunk:
                break
            if scanner.feed(chunk):
                log(
                    f"CONFLICT FRAME SEEN (downstream carries "
                    f"{CONFLICT_MARKER.decode()}; {scanner.count} so far)"
                )
            writer.write(chunk)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        try:
            writer.close()
            await writer.wait_closed()
        except (ConnectionError, OSError):
            pass


async def handle(client_reader, client_writer) -> None:
    try:
        server_reader, server_writer = await asyncio.open_connection("127.0.0.1", TARGET_PORT)
    except OSError as exc:
        log(f"cannot reach 127.0.0.1:{TARGET_PORT}: {exc}")
        client_writer.close()
        return
    await asyncio.gather(
        pump_delayed(client_reader, server_writer, DELAY_S),
        pump_direct(server_reader, client_writer),
    )


async def main() -> None:
    server = await asyncio.start_server(handle, "127.0.0.1", LISTEN_PORT)
    log(
        f"latency proxy on 127.0.0.1:{LISTEN_PORT} → 127.0.0.1:{TARGET_PORT}, "
        f"upstream +{DELAY_S * 1000:.0f} ms (downstream direct)"
    )
    async with server:
        await server.serve_forever()


def self_test() -> None:
    """Prove the boundary case the chunk-local scan used to miss."""
    m = CONFLICT_MARKER
    cases = [
        ("whole marker in one chunk", [b'{"code":' + m + b"}"], 1),
        ("split down the middle", [b'{"code":' + m[:3], m[3:] + b"}"], 1),
        ("one byte at a time", [bytes([b]) for b in b'{"code":' + m + b"}"], 1),
        ("two occurrences, one split", [b"x" + m + b"y" + m[:2], m[2:] + b"z"], 2),
        ("no marker", [b'{"code":-32030}', b'{"code":-320311}'[:8]], 0),
        ("carry cannot self-complete", [m, b""], 1),
    ]
    failures = 0
    for name, chunks, expected in cases:
        scanner = ConflictScanner(m)
        for c in chunks:
            scanner.feed(c)
        status = "ok " if scanner.count == expected else "FAIL"
        if scanner.count != expected:
            failures += 1
        print(f"  [{status}] {name}: counted {scanner.count}, expected {expected}")
    if failures:
        sys.exit(f"{failures} self-test case(s) failed")
    print("conflict oracle self-test passed")


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        self_test()
        sys.exit(0)
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
