#!/usr/bin/env python3
"""Real-binary capability-projection diagnostics QA driver.

Every positive receipt below comes from a live gateway/run-loop effect.  The
provider log is used only to prove the tool list handed to the real AgentLoop;
the disposable stdio MCP server writes the handler-effect receipt.
"""
from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time
from pathlib import Path

QA = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(QA / "browser_managed"))
from qa_rpc import Ledger, ws_connect, ran  # noqa: E402

DIAG = "capability_projection_diagnostics"


class Conn:
    def __init__(self, ws):
        self.ws = ws
        self._id = 100
        self.frames = []

    async def call(self, method, params, timeout=180, sent=None):
        self._id += 1
        rid = self._id
        await self.ws.send(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
        if sent is not None:
            sent.set()
        while True:
            msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=timeout))
            if msg.get("id") == rid:
                return msg
            if "method" in msg:
                self.frames.append(msg)

    async def batch(self, calls, timeout=180):
        ids = {}
        for method, params in calls:
            self._id += 1
            ids[self._id] = (method, params)
            await self.ws.send(json.dumps({"jsonrpc": "2.0", "id": self._id, "method": method, "params": params}))
        out = {}
        while len(out) < len(ids):
            msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=timeout))
            if msg.get("id") in ids:
                out[msg["id"]] = msg
            elif "method" in msg:
                self.frames.append(msg)
        return [out[rid] for rid in ids]

    async def connect(self):
        return await self.call("connect", {"client_info": {"name": "qa-capability-hpre"}})

    async def invoke(self, tool, args, sent=None):
        msg = await self.call("tools.invoke", {"tool_name": tool, "arguments": args}, sent=sent)
        if "error" in msg:
            return False, {"rpc_error": msg["error"]}
        result = msg.get("result", {})
        return bool(result.get("ok")), result.get("result", result)

    async def terminal(self, run_id, timeout=120):
        def match(m):
            return m.get("method") in ("stream.run_complete", "stream.run_error") and (m.get("params") or {}).get("run_id") == run_id
        for frame in self.frames:
            if match(frame):
                return frame
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=1))
            except asyncio.TimeoutError:
                continue
            if match(msg):
                return msg
            if "method" in msg:
                self.frames.append(msg)
        return {"method": "stream.timeout"}


class QA(Ledger):
    def __init__(self):
        super().__init__()
        self.receipts = 0
        self.unverified = []

    def receipt(self, claim, ok, detail=""):
        self.receipts += 1
        return self.check(claim, ok, detail)

    def gap(self, claim, detail):
        self.unverified.append(f"{claim}: {detail}")
        print(f"  [UNVERIFIED] {claim} — {detail}", flush=True)


def names(value, out=None):
    # An explicitly supplied empty set is a caller-owned accumulator.
    if out is None:
        out = set()
    if isinstance(value, dict):
        if isinstance(value.get("name"), str):
            out.add(value["name"])
        for child in value.values():
            names(child, out)
    elif isinstance(value, list):
        for child in value:
            names(child, out)
    return out


def provider_bodies(path: Path, marker: str):
    if not path.exists():
        return []
    out = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        body = row.get("body", row)
        if isinstance(body, dict) and marker in json.dumps(body, sort_keys=True):
            out.append(body)
    return out


def provider_tools(body):
    return {x.get("name") for x in body.get("tools", []) if isinstance(x, dict) and isinstance(x.get("name"), str)}


def effect_markers(path: Path, marker: str):
    """Return only the fixture's exact ``qa_echo`` tool-call effect."""
    if not path.exists():
        return []
    out = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        arguments = row.get("arguments")
        if (
            row.get("method") == "tools/call"
            and row.get("name") == "qa_echo"
            and isinstance(arguments, dict)
            and arguments.get("marker") == marker
        ):
            out.append(row)
    return out


async def gateway(conn, q, method, params):
    msg = await conn.call(method, params)
    if "error" in msg:
        q.check(f"gateway {method} accepted", False, json.dumps(msg["error"])[:300])
        return False, {"rpc_error": msg["error"]}
    return True, msg.get("result", {})


async def catalog(conn):
    ok, body = await gateway(conn, QA(), "tools.catalog", {})
    return names(body) if ok else set()


async def wait_for_catalog(conn, prefix, present=True, timeout=35):
    end = time.monotonic() + timeout
    latest = set()
    while time.monotonic() < end:
        latest = await catalog(conn)
        found = sorted(x for x in latest if x.startswith(prefix + "__"))
        if bool(found) == present:
            return found, latest
        await asyncio.sleep(0.25)
    return sorted(x for x in latest if x.startswith(prefix + "__")), latest


async def wait_effect(path, marker, timeout=35):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        found = effect_markers(path, marker)
        if found:
            return found
        await asyncio.sleep(0.25)
    return effect_markers(path, marker)


def marker_for(args, label):
    """Unique request marker; the mock's tool input has a separate fixture marker."""
    return f"QA_HPRE_{label.upper()}_{args.run_id}_{os.getpid()}"


def effect_marker_for(args, label):
    """Marker emitted by run.sh's static mock tool specification."""
    wire_label = {"replace": "replacement", "replacement": "replacement"}.get(label, label)
    return f"QA_HPRE_{wire_label.upper()}_{args.run_id}"


def fixture_args(script, effect_log, temporary_count=1):
    """MCP argv: argv[1] is the receipt log, argv[2] is temporary-tool count."""
    return [str(script), str(effect_log), str(int(temporary_count))]


def fixture_config(args, temporary_count=1):
    return {
        "command": args.python,
        "args": fixture_args(args.fixture_script, args.effect_log, temporary_count),
        "env": {},
        "enabled": True,
    }


def fixture_tool_names(temporary_count):
    return ["qa_echo"] + [f"temporaryqa_echo_{i}" for i in range(int(temporary_count))]


def temporary_tool_name(server, index):
    return f"{server_id(server)}__temporaryqa_echo_{index}"


def _wire_text(value):
    return json.dumps(value, sort_keys=True).lower()


def is_hold_already_active(ok, body):
    """Match the concrete structured hold error, not merely any failed call."""
    if ok or not isinstance(body, dict):
        return False
    if body.get("error") == "hold_already_active":
        return True
    rpc_error = body.get("rpc_error")
    # Real wire text wraps the Display string: "tool '<DIAG>' failed: Aleph error:
    # capability_projection_diagnostics: diagnostics hold already active".
    message = rpc_error.get("message") if isinstance(rpc_error, dict) else None
    if message == "diagnostics hold already active" or (
        isinstance(message, str) and message.endswith(f"{DIAG}: diagnostics hold already active")
    ):
        return True
    return _wire_text(body).find('"error": "hold_already_active"') >= 0


def is_canonical_host_closed(ok, body):
    """Recognize only the diagnostic HostClosed wire/error boundary."""
    if not isinstance(body, dict):
        return False
    text = _wire_text(body)
    return (not ok) and ("host_closed" in text or "diagnostics host is closed" in text)


def is_closed_status(ok, body, applied_before=None):
    """Accept a closed status snapshot, or the canonical closed read error."""
    if ok:
        status = status_body(body)
        if status.get("lifecycle") != "closed":
            return False
        return applied_before is None or status.get("applied_tool_ids") == applied_before
    return is_canonical_host_closed(ok, body)


def is_canonical_closed_chat_terminal(frame):
    """Require a real run_error/error boundary for a post-close chat refusal."""
    if not isinstance(frame, dict) or frame.get("method") != "stream.run_error":
        return False
    text = _wire_text(frame)
    return "host_closed" in text or "diagnostics host is closed" in text or "closed projection" in text


class HoldRaceError(Exception):
    """The two-connection hold race did not produce an independent armed proof.

    The accepted proof is narrow: one task must complete with the canonical
    ``HoldAlreadyActive`` wire error AND the other must still be pending
    inside its 5-second timer. Any other shape (both done, neither done
    within the bounded wait, first completion carrying anything other than
    HoldAlreadyActive) is rejected here; the caller treats this as
    "no armed hold proven, retry".
    """


async def classify_hold_race(task_a, task_b, *, timeout=3.5):
    """Resolve two independent hold RPCs into (winner_task, loser_task).

    ``winner_task`` is the still-pending 5-second hold that the host owns;
    ``loser_task`` has already returned ``HoldAlreadyActive`` and is the
    independent armed proof. The pending task is NOT cancelled on the
    success path; the caller owns its lifecycle so it can be awaited only
    after the burst/final dispatch is in flight. On every error path
    both tasks are cancelled and ``HoldRaceError`` is raised.
    """
    done, pending = await asyncio.wait(
        {task_a, task_b},
        return_when=asyncio.FIRST_COMPLETED,
        timeout=timeout,
    )
    if not done:
        for t in (task_a, task_b):
            if not t.done():
                t.cancel()
        raise HoldRaceError("neither hold RPC completed within the bounded wait")
    if len(done) == 2:
        raise HoldRaceError("both hold RPCs returned without HoldAlreadyActive; no host-owned hold proven")
    finished = next(iter(done))
    ok, body = finished.result()
    if not is_hold_already_active(ok, body):
        for t in (task_a, task_b):
            if not t.done():
                t.cancel()
        raise HoldRaceError(f"first completion was not HoldAlreadyActive: {body!r}")
    return next(iter(pending)), finished


async def _cancel_and_reap(*tasks):
    """Cancel still-running tasks and retrieve every result/exception."""
    live = [t for t in tasks if t is not None]
    for t in live:
        if not t.done():
            t.cancel()
    if live:
        await asyncio.gather(*live, return_exceptions=True)


async def _safe_close_ws(ws):
    try:
        await ws.close()
    except Exception:
        pass


def provider_bodies_exclude(bodies, *excluded_ids):
    """Every recorded provider body must omit every excluded tool id.

    Returns ``False`` for empty bodies, non-list inputs, or any body
    whose ``tools`` list carries one of the excluded names. Returning
    ``False`` on the empty case is intentional: a closed-state runloop
    that simply never reaches the provider is NOT proof that the
    canonical tool set excludes the identities. The caller pairs this
    with a separate terminal-match check.
    """
    if not bodies:
        return False
    for body in bodies:
        if not isinstance(body, dict):
            return False
        present = {
            x.get("name")
            for x in body.get("tools", [])
            if isinstance(x, dict) and isinstance(x.get("name"), str)
        }
        for excluded in excluded_ids:
            if excluded in present:
                return False
    return True


def is_rejection(msg):
    """Accept only a real JSON-RPC error or an explicit ``ok: false``."""
    error = msg.get("error") if isinstance(msg, dict) else None
    if isinstance(error, dict) and ("code" in error or "message" in error):
        return True
    result = msg.get("result") if isinstance(msg, dict) else None
    return isinstance(result, dict) and result.get("ok") is False


async def _chat_run(conn, q, marker):
    msg = await conn.call("chat.send", {"message": marker, "stream": True})
    if "error" in msg:
        q.receipt("chat.send reached the real AgentLoop", False, json.dumps(msg["error"])[:300])
        return None
    run_id = (msg.get("result") or {}).get("run_id")
    q.receipt("chat.send reached the real AgentLoop", bool(run_id), json.dumps(msg.get("result", {}))[:240])
    if not run_id:
        return None
    terminal = await conn.terminal(run_id)
    q.receipt(
        "real run-loop reached a terminal frame",
        terminal.get("method") in ("stream.run_complete", "stream.run_error"),
        json.dumps(terminal)[:240],
    )
    return terminal


async def _recorded_provider_bodies(path, marker, timeout=35):
    bodies = []
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        bodies = provider_bodies(path, marker)
        if bodies:
            return bodies
        await asyncio.sleep(0.25)
    return bodies


async def send_run(conn, q, marker, provider_log, effect_log, tool_name=None, effect_marker=None):
    """Positive path: provider request, mounted tool, and exact handler effect."""
    effect_marker = effect_marker or marker
    terminal = await _chat_run(conn, q, marker)
    if terminal is None:
        return [], []
    q.receipt(
        "positive run completed successfully",
        terminal.get("method") == "stream.run_complete",
        json.dumps(terminal)[:240],
    )
    bodies = await _recorded_provider_bodies(provider_log, marker)
    if tool_name:
        tools = set().union(*(provider_tools(b) for b in bodies)) if bodies else set()
        q.receipt("provider request body contains the mounted tool", tool_name in tools, str(sorted(tools)))
    q.receipt("provider request was recorded with a nonempty body", bool(bodies) and any(bool(b) for b in bodies), f"{len(bodies)} request(s)")
    effects = await wait_effect(effect_log, effect_marker)
    q.receipt("invocation handler produced an attributed effect receipt", bool(effects), str(effects[:1]))
    return bodies, effects


async def send_negative_run(conn, q, marker, provider_log, removed_tool):
    """Negative path: real provider/terminal receipt, but no effect wait."""
    terminal = await _chat_run(conn, q, marker)
    if terminal is None:
        return []
    bodies = await _recorded_provider_bodies(provider_log, marker)
    tools = set().union(*(provider_tools(b) for b in bodies)) if bodies else set()
    q.receipt("negative provider request was recorded with a nonempty body", bool(bodies) and any(bool(b) for b in bodies), f"{len(bodies)} request(s)")
    q.receipt("negative provider request excludes the removed tool", bool(bodies) and removed_tool not in tools, str(sorted(tools)))
    return bodies


async def setup(conn, q, args, name, temporary_count=1):
    ok, body = await gateway(conn, q, "mcp_config.create", {"name": name, "config": fixture_config(args, temporary_count)})
    q.receipt(f"MCP mutation route created {name}", ok and ran(ok, body), json.dumps(body)[:240])
    return ok


def server_id(name):
    return "".join(c if (c.isascii() and (c.isalnum() or c in "_- .")) else "_" for c in name).replace(" ", "")


async def diag(conn, q, operation, _sent=None, **fields):
    ok, body = await conn.invoke(DIAG, {"operation": operation, **fields}, sent=_sent)
    return ok, body


def status_body(body):
    return body if isinstance(body, dict) and "registry_cursor" in body else next((v for v in body.values() if isinstance(v, dict) and "registry_cursor" in v), {}) if isinstance(body, dict) else {}


async def initial(conn, q, args):
    name = args.prefix + "_initial"
    await setup(conn, q, args, name)
    hits, _ = await wait_for_catalog(conn, name)
    q.receipt("positive initial fixture reaches tools.catalog", bool(hits), str(hits))
    if hits:
        await send_run(conn, q, marker_for(args, "initial"), args.provider_log, args.effect_log, hits[0], effect_marker_for(args, "initial"))


async def replacement(conn, q, args):
    old, new = args.prefix + "_replace_old", args.prefix + "_replace_new"
    await setup(conn, q, args, old)
    old_hits, _ = await wait_for_catalog(conn, old)
    q.receipt("normal registry replacement has old fixture", bool(old_hits), str(old_hits))
    if not old_hits:
        return
    old_id = server_id(old)
    ok, _ = await gateway(conn, q, "mcp_config.delete", {"id": old_id})
    q.receipt("normal registry removal route accepted", ok)
    removed, _ = await wait_for_catalog(conn, old, False)
    q.receipt("normal registry removal removes old tool", not removed, str(removed))
    await setup(conn, q, args, new)
    new_hits, _ = await wait_for_catalog(conn, new)
    q.receipt("normal registry replacement reaches new fixture", bool(new_hits), str(new_hits))
    if new_hits:
        await send_run(conn, q, marker_for(args, "replacement"), args.provider_log, args.effect_log, new_hits[0], effect_marker_for(args, "replacement"))
        replacement_bodies = provider_bodies(args.provider_log, marker_for(args, "replacement"))
        q.receipt(
            "replacement provider tool set excludes removed identity",
            bool(replacement_bodies) and all(old_hits[0] not in provider_tools(b) for b in replacement_bodies),
            str(old_hits),
        )


async def owner_fixture(conn, q, args, kind):
    name = args.prefix + "_" + kind
    await setup(conn, q, args, name)
    hits, _ = await wait_for_catalog(conn, name)
    q.receipt(f"{kind} fixture reaches applied catalog", bool(hits), str(hits))
    return name, hits[0] if hits else ""


async def ownership(conn, q, args):
    name, tool = await owner_fixture(conn, q, args, "owner")
    if not tool:
        return
    ok, before_raw = await diag(conn, q, "status")
    before = status_body(before_raw)
    baseline_cursor = before.get("registry_cursor")
    baseline_gen = max((int(x[1]) for x in before.get("applied_owner_generations", []) if len(x) == 2 and x[0] == tool), default=-1)
    q.receipt("owner status is a real applied snapshot", ok and tool in before.get("applied_tool_ids", []), json.dumps(before)[:300])
    ok, bump = await diag(conn, q, "bump_runtime")
    bump_generation = int(bump.get("owner_generation", -1)) if isinstance(bump, dict) else -1
    q.receipt("owner generation bump control applied", ok and bump_generation > baseline_gen, json.dumps(bump))
    end = time.monotonic() + 30
    after = {}
    while time.monotonic() < end:
        _, raw = await diag(conn, q, "status")
        after = status_body(raw)
        gens = {x[0]: int(x[1]) for x in after.get("applied_owner_generations", []) if len(x) == 2}
        if gens.get(tool, -1) == bump_generation:
            break
        await asyncio.sleep(0.25)
    after_gens = {x[0]: int(x[1]) for x in after.get("applied_owner_generations", []) if len(x) == 2}
    q.receipt(
        "bump applied exact owner generation without registry revision",
        after.get("registry_cursor") == baseline_cursor and after_gens.get(tool) == bump_generation and bump_generation > baseline_gen,
        json.dumps(after),
    )
    ok, _ = await diag(conn, q, "revoke_tool", tool_name=tool)
    q.receipt("owner-only revoke control accepted", ok)
    end = time.monotonic() + 30
    revoked = {}
    while time.monotonic() < end:
        _, raw = await diag(conn, q, "status")
        revoked = status_body(raw)
        if tool not in revoked.get("applied_tool_ids", []):
            break
        await asyncio.sleep(0.25)
    q.receipt("owner-only revoke removes applied tool without registry revision", tool not in revoked.get("applied_tool_ids", []) and revoked.get("registry_cursor") == baseline_cursor, json.dumps(revoked))
    await send_negative_run(conn, q, marker_for(args, "owner_revoked"), args.provider_log, tool)
    _, dispose_tool = await owner_fixture(conn, q, args, "dispose")
    if dispose_tool:
        _, pre_raw = await diag(conn, q, "status")
        pre = status_body(pre_raw)
        cursor = pre.get("registry_cursor")
        ok, _ = await diag(conn, q, "dispose_runtime")
        q.receipt("owner dispose control accepted", ok)
        end = time.monotonic() + 30
        post = {}
        while time.monotonic() < end:
            _, post_raw = await diag(conn, q, "status")
            post = status_body(post_raw)
            if dispose_tool not in post.get("applied_tool_ids", []):
                break
            await asyncio.sleep(0.25)
        q.receipt("dispose removes tools without registry revision", dispose_tool not in post.get("applied_tool_ids", []) and post.get("registry_cursor") == cursor, json.dumps(post))
        await send_negative_run(conn, q, marker_for(args, "dispose_removed"), args.provider_log, dispose_tool)


async def _armed_hold(args, q, plane):
    """Arm a 5-second hold only when one of two independent connections
    proves the other is currently host-owned.

    The host's ``hold`` RPC only returns success after the timer expires
    or an explicit release. Awaiting first-success therefore proves
    nothing about an *active* hold. The accepted proof: send the same
    ``hold(plane, 5000)`` on two independent physical connections and
    observe which one completes first with the canonical
    ``HoldAlreadyActive`` wire error. That connection is independent
    confirmation that the OTHER connection is currently inside its
    5-second host-owned hold, regardless of which side won the race.
    """
    last_error = None
    for attempt in range(3):
        sockets = []
        tasks = []
        armed = None
        try:
            # ws_connect() is an async context manager; we hold the raw
            # socket and close it explicitly via _safe_close_ws.
            first_ws = await ws_connect(args.ws).__aenter__()
            sockets.append(first_ws)
            second_ws = await ws_connect(args.ws).__aenter__()
            sockets.append(second_ws)
            first = Conn(first_ws)
            second = Conn(second_ws)
            await first.connect()
            await second.connect()
            # No "request sent" barrier: socket-send readiness is not an
            # armed proof. classify_hold_race bounds the whole proof to
            # <= 3.5s, leaving the rest of the host's 5s timer for the burst.
            task1 = asyncio.create_task(diag(first, q, "hold", plane=plane, duration_ms=5000))
            task2 = asyncio.create_task(diag(second, q, "hold", plane=plane, duration_ms=5000))
            tasks = [task1, task2]
            try:
                winner_task, loser_task = await classify_hold_race(task1, task2, timeout=3.5)
            except HoldRaceError as exc:
                last_error = str(exc)
                print(f"  [INFO] hold active proof retry {attempt + 1}: {last_error}", flush=True)
                continue
            winner_ws = second_ws if winner_task is task2 else first_ws
            armed = (winner_ws, winner_task)
            return armed
        finally:
            if armed is None:
                # Retry or unexpected failure (incl. cancellation): cancel
                # and reap both RPC tasks, close both sockets. No host
                # control task is left behind.
                await _cancel_and_reap(*tasks)
                for ws in sockets:
                    await _safe_close_ws(ws)
            else:
                # Success: the loser socket is released; the winner's
                # pending task and socket stay owned by the caller.
                for ws in sockets:
                    if ws is not armed[0]:
                        await _safe_close_ws(ws)
    q.check(
        f"{plane} hold active proof",
        False,
        f"three bounded attempts never produced HoldAlreadyActive: {last_error}",
    )
    return None


APPLIED_RECEIPT_FIELDS = (
    "applied_invalidation_count",
    "applied_replacement_count",
    "last_replacement_registry_cursor",
    "last_replacement_tool_ids",
)


def applied_receipt_missing(status):
    """Names of the four applier receipt fields absent from a status wire body."""
    return [k for k in APPLIED_RECEIPT_FIELDS if k not in status]


def _intval(value):
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def applied_recovery_verdict(before, after, base_tool, temporaries=()):
    """Judge real applier-side recovery receipts between two status snapshots.

    Returns ``(verdict, detail)``; verdict is ``"pass"``, ``"fail"`` or
    ``"unverified"``. Only the applier fields (``applied_*`` and
    ``last_replacement_*``) count; the source-side ``replacement_count`` and the
    pending depth do not.  ``last_replacement_*`` is the replacement snapshot the
    applier consumed, which need not equal the final snapshot (later normal
    deltas may follow), so it is only required to come from a registry cursor
    past the baseline and not past the final applied cursor.  The registry
    cursor is a separate domain from owner generation / session seq.
    """
    missing = applied_receipt_missing(before) + [k for k in applied_receipt_missing(after) if k not in applied_receipt_missing(before)]
    if missing:
        return "unverified", f"status wire lacks applier receipt fields: {sorted(set(missing))}"
    b_inv, a_inv = _intval(before["applied_invalidation_count"]), _intval(after["applied_invalidation_count"])
    b_rep, a_rep = _intval(before["applied_replacement_count"]), _intval(after["applied_replacement_count"])
    if None in (b_inv, a_inv, b_rep, a_rep):
        return "fail", "applier counters are not integers"
    problems = []
    if not a_inv > b_inv:
        problems.append(f"applied_invalidation_count did not increase ({b_inv}->{a_inv})")
    if not a_rep > b_rep:
        problems.append(f"applied_replacement_count did not increase ({b_rep}->{a_rep})")
    pair_cursor = _intval(after["last_replacement_registry_cursor"])
    pair_ids = after["last_replacement_tool_ids"]
    base_cursor, final_cursor = _intval(before.get("registry_cursor")), _intval(after.get("registry_cursor"))
    if pair_cursor is None:
        problems.append("last_replacement_registry_cursor is absent after a replacement")
    else:
        if base_cursor is not None and not pair_cursor > base_cursor:
            problems.append(f"replacement cursor {pair_cursor} not past baseline registry cursor {base_cursor}")
        if final_cursor is not None and pair_cursor > final_cursor:
            problems.append(f"replacement cursor {pair_cursor} is past final applied cursor {final_cursor}")
    if not isinstance(pair_ids, list) or not pair_ids or not all(isinstance(x, str) for x in pair_ids):
        problems.append("last_replacement_tool_ids is not a nonempty list of ids")
    elif base_tool not in pair_ids:
        problems.append(f"replacement payload lacks base tool {base_tool}")
    applied_ids = after.get("applied_tool_ids", [])
    if base_tool not in applied_ids:
        problems.append("final applied snapshot lacks the surviving base tool")
    leaked = sorted(set(temporaries) & set(applied_ids))
    if leaked:
        problems.append(f"final applied snapshot still carries {len(leaked)} temporary tool(s)")
    if problems:
        return "fail", "; ".join(problems)
    return "pass", (
        f"inv {b_inv}->{a_inv}; repl {b_rep}->{a_rep}; pair_cursor={pair_cursor}; "
        f"pair_ids={len(pair_ids)}; final_cursor={final_cursor}"
    )


async def hold_and_updates(conn, q, args, plane, count, prefix):
    name, tool = await owner_fixture(conn, q, args, prefix)
    if not tool:
        return
    _, before_raw = await diag(conn, q, "status")
    before = status_body(before_raw)
    capacity = int(before.get("pending_capacity", 0))
    burst_count = max(capacity + 2, count, 300)
    armed = await _armed_hold(args, q, plane)
    if armed is None:
        return
    hold_ws, hold_task = armed

    # One bridge update registers the complete temporary set; this is not a
    # restart-per-event loop. The final zero-count update removes only those
    # temporary entries and leaves the base qa_echo fixture mounted.
    burst_ok, burst_body = await gateway(
        conn,
        q,
        "mcp_config.update",
        {"id": server_id(name), "config": fixture_config(args, burst_count)},
    )
    q.receipt(
        f"{plane} hold bulk registration route accepted",
        burst_ok and ran(burst_ok, burst_body),
        f"temporary_count={burst_count}; pending_capacity={capacity}; {json.dumps(burst_body)[:180]}",
    )
    if not burst_ok:
        await hold_task
        await hold_ws.close()
        return

    overflow = False
    overflow_status = {}
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        _, raw = await diag(conn, q, "status")
        overflow_status = status_body(raw)
        pending = int(overflow_status.get("pending_depth", 0))
        replacement_delta = int(overflow_status.get("replacement_count", 0)) - int(before.get("replacement_count", 0))
        overflow = replacement_delta > 0 or pending > capacity
        if plane == "delivery" and overflow:
            break
        if plane != "delivery":
            # Source hold intentionally defers the lag observation until the
            # normal timer release, but still records the real queued depth.
            if pending > 0:
                break
        await asyncio.sleep(0.1)
    if plane == "delivery":
        # Source-side overflow is only the cause here; the consumed Invalidated
        # and the applied replacement are proven below by the applier's own
        # applied_invalidation_count / applied_replacement_count receipts.
        q.receipt(
            "delivery hold proves bounded overflow before convergence",
            overflow,
            json.dumps(overflow_status),
        )
    else:
        q.receipt(
            "source-intake hold records real queued bulk registration",
            int(overflow_status.get("pending_depth", 0)) > 0,
            json.dumps(overflow_status),
        )
    if plane == "delivery" and not overflow:
        q.check("delivery burst completed within the five-second proof window", False, json.dumps(overflow_status))

    final_ok, final_body = await gateway(
        conn,
        q,
        "mcp_config.update",
        {"id": server_id(name), "config": fixture_config(args, 0)},
    )
    q.receipt(
        f"{plane} final zero-temporary update accepted",
        final_ok and ran(final_ok, final_body),
        json.dumps(final_body)[:240],
    )
    hold_ok, hold_body = await hold_task
    q.receipt(f"{plane} hold autorelease completed", hold_ok, json.dumps(hold_body)[:240])
    await hold_ws.close()

    end = time.monotonic() + 35
    temporary = {temporary_tool_name(name, i) for i in range(burst_count)}
    # Wait for REAL applier receipts (not the source-side enqueue counter), the
    # final registry state (no temporary tools) and a stable applied cursor.
    after = {}
    stable = 0
    last_cursor = None
    while time.monotonic() < end:
        _, raw = await diag(conn, q, "status")
        after = status_body(raw)
        advanced = (
            _intval(after.get("applied_invalidation_count")) is not None
            and _intval(after.get("applied_invalidation_count")) > int(before.get("applied_invalidation_count", 0) or 0)
            and _intval(after.get("applied_replacement_count")) is not None
            and _intval(after.get("applied_replacement_count")) > int(before.get("applied_replacement_count", 0) or 0)
        )
        converged = (
            advanced
            and tool in after.get("applied_tool_ids", [])
            and temporary.isdisjoint(after.get("applied_tool_ids", []))
            and after.get("registry_cursor", 0) > before.get("registry_cursor", 0)
        )
        if converged and after.get("registry_cursor") == last_cursor:
            stable += 1
        else:
            stable = 0
        last_cursor = after.get("registry_cursor")
        if stable >= 3:
            break
        if len(applied_receipt_missing(after)) == len(APPLIED_RECEIPT_FIELDS):
            break
        await asyncio.sleep(0.25)
    final_applied = tool in after.get("applied_tool_ids", [])
    final_cursor = after.get("registry_cursor", 0) > before.get("registry_cursor", 0)
    lag_observed = after.get("lag_count", 0) > before.get("lag_count", 0)
    verdict, detail = applied_recovery_verdict(before, after, tool, temporary)
    summary = json.dumps({k: after.get(k) for k in ("lifecycle", "registry_cursor", "applied_invalidation_count", "applied_replacement_count", "last_replacement_registry_cursor", "lag_count", "replacement_count")})
    if verdict == "unverified":
        q.gap(f"{plane} applied recovery receipts", detail)
        return
    q.receipt(f"{plane} real applier receipts: invalidation+replacement counted, cursor/payload from the applied snapshot", verdict == "pass", f"{detail}; {summary}")
    if plane != "delivery":
        q.receipt("source-intake recovery observed lag on the real source", lag_observed, summary)
    q.receipt(f"{plane} final applied snapshot has base tool and advanced cursor", final_applied and final_cursor, summary)
    if verdict != "pass" or not (final_applied and final_cursor):
        return
    bodies, _ = await send_run(
        conn,
        q,
        marker_for(args, f"{prefix}_final"),
        args.provider_log,
        args.effect_log,
        tool,
        effect_marker_for(args, plane.split("_")[0]),
    )
    provider_final = set().union(*(provider_tools(b) for b in bodies)) if bodies else set()
    q.receipt(
        f"{plane} final provider exposes surviving base only",
        bool(bodies) and temporary.isdisjoint(provider_final) and tool in provider_final,
        str(sorted(x for x in provider_final if "temporaryqa_echo" in x)),
    )


async def close_case(conn, q, args):
    name, tool = await owner_fixture(conn, q, args, "close")
    if not tool:
        return
    pre_ok, pre_raw = await diag(conn, q, "status")
    pre = status_body(pre_raw)
    q.receipt(
        "close pre-status is active with applied fixture",
        pre_ok and pre.get("lifecycle") == "active" and tool in pre.get("applied_tool_ids", []),
        json.dumps(pre)[:300],
    )
    ok, close = await diag(conn, q, "close")
    q.receipt(
        "close completes shared source/applier join",
        ok and close.get("source_joined") and close.get("applier_joined") and not close.get("source_failed") and not close.get("applier_failed"),
        json.dumps(close),
    )
    post_ok, post_raw = await diag(conn, q, "status")
    q.receipt(
        "post-close host status is closed or canonical closed refusal",
        is_closed_status(post_ok, post_raw, pre.get("applied_tool_ids", [])),
        json.dumps(post_raw)[:400],
    )

    # The shared registry is intentionally independent of the projection host:
    # create may be accepted and its catalog entry may become visible after
    # close. Neither is used as the close oracle.
    later = args.prefix + "_post_close"
    created = await setup(conn, q, args, later)
    hits, _ = await wait_for_catalog(conn, later, True, timeout=10)
    q.receipt("post-close shared-registry create remains accepted", created, str(hits))
    q.receipt("post-close shared-registry catalog may remain visible", created and bool(hits), str(hits))
    after_create_ok, after_create_raw = await diag(conn, q, "status")
    q.receipt(
        "post-close host status/applied snapshot remains closed and unchanged",
        is_closed_status(after_create_ok, after_create_raw, pre.get("applied_tool_ids", [])),
        json.dumps(after_create_raw)[:400],
    )

    # The installed closed-state runloop fails closed on an empty MCP
    # entry list but still exposes the built-in canonical tools
    # (``get_tool_schema``, ``subagent``). A real chat therefore reaches a
    # matched terminal (``stream.run_complete``/``stream.run_error``) with
    # the provider carrying that canonical-only tool surface. The closed
    # identities (the original close fixture AND the post-close fixture)
    # must be absent from EVERY recorded provider body, not just the
    # first one and not "all([]) implies refusal".
    marker = marker_for(args, "post_close_chat")
    chat = await conn.call("chat.send", {"message": marker, "stream": True})
    terminal = None
    if "error" not in chat:
        run_id = (chat.get("result") or {}).get("run_id")
        if run_id:
            terminal = await conn.terminal(run_id)
    terminal_matched = (
        isinstance(terminal, dict)
        and terminal.get("method") in ("stream.run_complete", "stream.run_error")
    )
    q.receipt(
        "post-close chat reached a matched real terminal",
        terminal_matched,
        json.dumps(terminal or chat)[:500],
    )
    post_bodies = await _recorded_provider_bodies(args.provider_log, marker, timeout=2)
    excluded = [tool]
    if hits:
        excluded.append(hits[0])
    q.receipt(
        "post-close provider tool set excludes closed and after-close identities",
        provider_bodies_exclude(post_bodies, *excluded),
        f"excluded={excluded}; bodies={len(post_bodies)}",
    )
    # Extra observability only: a real canonical closed-host error still
    # counts when it is actually present, but it is not required for the
    # close contract — the closed-state runloop may terminate via a
    # tool_not_found boundary on the canonical builtins instead.
    canonical_refusal = is_canonical_host_closed(False, chat) or is_canonical_closed_chat_terminal(terminal)
    if canonical_refusal:
        q.receipt(
            "post-close canonical closed-host refusal observed",
            True,
            json.dumps(terminal or chat)[:500],
        )


async def negative(conn, q, args):
    before_ok, before_raw = await diag(conn, q, "status")
    before = status_body(before_raw)
    msg = await conn.call("tools.invoke", {"tool_name": DIAG, "arguments": {"operation": "status", "role": "operator", "loopback": True, "connection_id": "spoof"}})
    q.receipt("spoofed diagnostic identity args are denied before mutation", is_rejection(msg), json.dumps(msg)[:300])
    _, after_raw = await diag(conn, q, "status")
    after = status_body(after_raw)
    q.receipt("spoofed args leave diagnostic state unchanged", before.get("registry_cursor") == after.get("registry_cursor"), json.dumps(after)[:300])
    # Member / physical non-loopback / missing-ambient denials need a real TLS
    # LAN peer and ordinary-agent route; they live in tls_negative.py (run.sh
    # "tls" phase) and are judged there, never assumed here.


async def disabled(conn, q, args):
    body = await conn.call("tools.catalog", {})
    visible = DIAG in names(body.get("result", {}))
    q.receipt("diagnostic tool is absent when ALEPH_CAPABILITY_DIAGNOSTICS is not exactly 1", not visible, str(sorted(x for x in names(body.get("result", {})) if "diagnostic" in x)))
    msg = await conn.call("tools.invoke", {"tool_name": DIAG, "arguments": {"operation": "status"}})
    q.receipt("disabled diagnostic invocation is refused", is_rejection(msg), json.dumps(msg)[:300])


async def main(args):
    q = QA()
    try:
        async with ws_connect(args.ws) as ws:
            conn = Conn(ws)
            await conn.connect()
            if args.scenario == "disabled":
                await disabled(conn, q, args)
            elif args.scenario == "initial":
                await initial(conn, q, args)
            elif args.scenario == "replacement":
                await replacement(conn, q, args)
            elif args.scenario == "ownership":
                await ownership(conn, q, args)
            elif args.scenario == "delivery":
                await hold_and_updates(conn, q, args, "delivery", 0, "delivery")
            elif args.scenario == "source":
                await hold_and_updates(conn, q, args, "source_intake", 258, "source")
            elif args.scenario == "close":
                await close_case(conn, q, args)
            elif args.scenario == "negative":
                await negative(conn, q, args)
    except Exception as exc:
        q.check("driver completed without transport/runtime error", False, repr(exc))
    print(f"RECEIPT_COUNT: {q.receipts}", flush=True)
    if q.unverified:
        print("UNVERIFIED_GAPS:", flush=True)
        for gap in q.unverified:
            print(f"  - {gap}", flush=True)
    if q.failures:
        print(f"VERDICT: FAIL ({len(q.failures)} claim(s))", flush=True)
        return 1
    if q.unverified:
        print("VERDICT: UNVERIFIED (no PASS claimed)", flush=True)
        return 3
    print("VERDICT: PASS", flush=True)
    return 0


def parse_args():
    p = argparse.ArgumentParser()
    p.add_argument("--ws", required=True)
    p.add_argument("--provider-log", type=Path, required=True)
    p.add_argument("--effect-log", type=Path, required=True)
    p.add_argument("--fixture-script", type=Path, required=True)
    p.add_argument("--python", default=sys.executable)
    p.add_argument("--scenario", choices=("disabled", "initial", "replacement", "ownership", "delivery", "source", "close", "negative"), required=True)
    p.add_argument("--prefix", required=True)
    p.add_argument("--run-id", default=str(os.getpid()))
    return p.parse_args()


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main(parse_args())))
