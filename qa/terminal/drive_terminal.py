#!/usr/bin/env python3
"""The four `qa/terminal` stages, driven against a real booted gateway.

Every assertion here is on an EFFECT — a field value in `runtime.agents.list`
or in a `terminal{...}` answer — never on "the call happened". The reason is
specific to this round: phase 1 shipped an agent panel that identified
sessions from the `$SHELL` recorded at `pty.spawn` time, so every row in
production read `Unknown` while twenty-one detection manifests and their unit
tests stayed green. A test that calls the sampler with the agent's name in its
hand cannot see that; only a shell that is spawned as `sh` and has `claude`
typed into it afterwards can.

  identify  the probe names the foreground program and the manifest names the
            agent, for a session whose SPAWN LABEL is `sh` — with a control
            session that ran no agent, so a green cannot come from
            "everything is claude"
  wait      `terminal{wait}` blocks on the table's watch and returns `reached`
            when the state arrives — with the negative arm, a state the
            session never enters, which must answer `timeout` and the CURRENT
            entry rather than dressing the last one up as a final state
  quiet     30 s of silence publishes `quiet_since` and does NOT move `state`
            (spec R2-3), and a frame clears it again
  cwd       the merged cwd order — OSC 7 › foreground probe › spawn dir — over
            three directories that are actually different, so the winner is
            identifiable
"""
import asyncio
import json
import os
import sys
import time

import websockets

URL = sys.argv[1]
STAGE = sys.argv[2]
BIN_DIR = sys.argv[3]
WORK = sys.argv[4]
CHROME = json.load(open(sys.argv[5]))

rc = 0
SPAWNED: list[str] = []


def check(ok, label, detail=""):
    global rc
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}" + (f" — {detail}" if detail else ""))
    if not ok:
        rc = 1


def note(msg):
    print(f"  ... {msg}")


class Conn:
    """One websocket, with the JSON-RPC id bookkeeping every call needs."""

    def __init__(self, ws, name):
        self.ws = ws
        self.n = 0
        self.name = name

    async def call(self, method, params, timeout=60):
        self.n += 1
        mine = self.n
        await self.ws.send(
            json.dumps({"jsonrpc": "2.0", "id": mine, "method": method, "params": params})
        )
        while True:
            msg = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=timeout))
            if msg.get("id") == mine:
                return msg

    async def tool(self, arguments, timeout=60):
        r = await self.call(
            "tools.invoke", {"tool_name": "terminal", "arguments": arguments}, timeout=timeout
        )
        if "error" in r:
            return None, json.dumps(r["error"])[:400]
        out = r["result"]["result"]
        if not out.get("success"):
            return None, out.get("message", "<no message>")
        return out.get("data"), ""

    async def spawn(self, **params):
        params.setdefault("rows", 24)
        params.setdefault("cols", 100)
        r = await self.call("pty.spawn", params)
        if "error" in r:
            raise RuntimeError(f"pty.spawn failed: {json.dumps(r['error'])[:400]}")
        res = r["result"]
        SPAWNED.append(res["session_id"])
        return res

    async def send(self, session_id, data):
        r = await self.call("pty.input", {"session_id": session_id, "data": data})
        if "error" in r:
            raise RuntimeError(f"pty.input failed: {json.dumps(r['error'])[:400]}")

    async def agents(self):
        r = await self.call("runtime.agents.list", {})
        if "error" in r:
            raise RuntimeError(f"runtime.agents.list failed: {json.dumps(r['error'])[:400]}")
        return {e["session_id"]: e for e in r["result"]["agents"]}

    async def sessions(self):
        r = await self.call("pty.list", {})
        if "error" in r:
            raise RuntimeError(f"pty.list failed: {json.dumps(r['error'])[:400]}")
        return {s["session_id"]: s for s in r["result"]["sessions"]}

    async def entry(self, session_id):
        return (await self.agents()).get(session_id)

    async def until(self, session_id, pred, seconds, what):
        """Poll one row until `pred` holds. Returns (entry, elapsed_seconds).

        Returns the LAST entry seen either way, so a failing assertion can
        print what the row actually said instead of `None`.
        """
        started = time.monotonic()
        last = None
        while time.monotonic() - started < seconds:
            last = await self.entry(session_id)
            if last is not None and pred(last):
                return last, time.monotonic() - started
            await asyncio.sleep(0.3)
        note(f"timed out after {seconds}s waiting for {what}; last row: {last}")
        return last, time.monotonic() - started


async def connect(name):
    ws = await websockets.connect(URL, max_size=None)
    c = Conn(ws, name)
    await c.call("connect", {"client": f"qa-terminal-{name}", "version": "1"})
    return c


# --------------------------------------------------------------------------
# identify
# --------------------------------------------------------------------------


async def stage_identify(c):
    """A shell spawned as `sh`, with an agent typed into it afterwards.

    The spawn label is asserted to be `sh` in the same breath as the agent
    name, because "identified from the spawn label" and "identified from the
    probe" produce the same row when the label happens to be right — which is
    exactly why phase 1's defect survived. Here the label CANNOT be right.
    """
    agent_s = (await c.spawn(command="sh", cwd=f"{WORK}/spawn"))["session_id"]
    plain_s = (await c.spawn(command="sh", cwd=f"{WORK}/spawn"))["session_id"]
    note(f"agent session {agent_s}, control session {plain_s}")

    # A shell that has not painted has no row at all; nudge both so the first
    # frame is not something we are merely hoping for.
    await c.send(plain_s, "echo qa-control-shell\n")
    await c.send(agent_s, "echo qa-agent-shell\n")
    await c.until(agent_s, lambda e: True, 15, "the agent session's first frame")
    await c.until(plain_s, lambda e: True, 15, "the control session's first frame")

    await c.send(agent_s, f'export PATH="{BIN_DIR}:$PATH"\n')
    await c.send(agent_s, "claude\n")

    want = CHROME["screens"]["working"]
    entry, took = await c.until(
        agent_s,
        lambda e: e.get("agent") == "claude"
        and e.get("program") == "claude"
        and e.get("state") == "working",
        40,
        "the typed agent to be identified and reach working",
    )
    entry = entry or {}
    note(f"identified after {took:.1f}s: {entry}")

    rows = await c.sessions()
    label = rows.get(agent_s, {}).get("shell")
    # If this were `claude`, every row below would be satisfied by the
    # spawn-label path phase 1 already had, and the stage would prove nothing.
    check(
        label == "sh",
        "the agent session's SPAWN LABEL is still `sh`",
        f"pty.list shell={label!r}",
    )
    check(
        entry.get("program") == "claude",
        "the foreground probe put the PROGRAM on the wire",
        f"program={entry.get('program')!r}",
    )
    check(
        entry.get("agent") == "claude",
        "the manifest identified the AGENT from that program",
        f"agent={entry.get('agent')!r}",
    )
    check(
        entry.get("state") == want["state"],
        f"the screen rules were reachable — state is {want['state']}",
        f"state={entry.get('state')!r}",
    )

    data, err = await c.tool({"action": "explain", "session_id": agent_s})
    if data is None:
        check(False, "terminal{explain} answered", err)
    else:
        rule = (data.get("matched_rule") or {}).get("id")
        check(
            rule == want["rule"],
            "explain names the manifest rule the fixture's screen was built from",
            f"matched_rule={rule!r}, expected {want['rule']!r}; "
            f"screen_tail={json.dumps(data.get('inputs', {}).get('screen_tail', ''))[:200]}",
        )
        check(
            data.get("source") == "bundled"
            and data.get("manifest_version") == CHROME["manifest_version"],
            "explain reports the bundled manifest at the version the screens came from",
            f"source={data.get('source')!r} version={data.get('manifest_version')!r} "
            f"expected {CHROME['manifest_version']!r}",
        )

    # The falsifying half. Without it, a sampler that answered `claude` for
    # every session would satisfy every assertion above.
    control = await c.entry(plain_s)
    control = control or {}
    note(f"control row: {control}")
    # `program: null` is "we could not look", not "no agent is running": the
    # two arms below would both be satisfied by a probe that never answered.
    check(
        control.get("program") is not None,
        "the probe ANSWERED for the control session too",
        f"program={control.get('program')!r}",
    )
    check(
        control.get("program") != "claude",
        "the control session's program is not the agent's",
        f"program={control.get('program')!r}",
    )
    check(
        control.get("agent") is None,
        "no manifest matched the control session, and none was guessed",
        f"agent={control.get('agent')!r}",
    )
    check(
        control.get("state") == "unknown",
        "an unidentified program is `unknown`, never `idle`",
        f"state={control.get('state')!r}",
    )


# --------------------------------------------------------------------------
# wait
# --------------------------------------------------------------------------


async def stage_wait(c):
    """`terminal{wait}` really blocks on the state, and really gives up.

    The blocking call goes out on its OWN connection: a gateway that handles
    one request at a time per socket would otherwise serialise the poll behind
    the wait, and the stage would be measuring the transport rather than the
    watch.
    """
    session = (await c.spawn(command="sh", cwd=f"{WORK}/spawn"))["session_id"]
    note(f"session {session}")
    await c.send(session, "echo qa-wait-shell\n")
    await c.until(session, lambda e: True, 15, "the first frame")
    await c.send(session, f'export PATH="{BIN_DIR}:$PATH"\n')
    # 8 s per screen: the wait has to be ISSUED while the session is working,
    # and a 2 s working phase can be over before the poll notices it.
    await c.send(session, "PHASE_SECS=8 claude\n")

    entry, _ = await c.until(
        session,
        lambda e: e.get("agent") == "claude" and e.get("state") == "working",
        40,
        "the agent to reach working",
    )
    if (entry or {}).get("state") != "working":
        check(False, "the session reached working before the wait was issued", str(entry))
        return
    note(f"working: {entry}")

    waiter = await connect("waiter")
    started = time.monotonic()
    data, err = await waiter.tool(
        {"action": "wait", "session_id": session, "until": ["blocked"], "timeout_ms": 30000},
        timeout=60,
    )
    took_ms = (time.monotonic() - started) * 1000
    if data is None:
        check(False, "terminal{wait} answered", err)
    else:
        note(f"wait returned after {took_ms:.0f} ms: {json.dumps(data)[:300]}")
        check(
            data.get("outcome") == "reached",
            "a wait whose state arrives answers `reached`",
            f"outcome={data.get('outcome')!r}",
        )
        check(
            (data.get("agent") or {}).get("state") == "blocked",
            "the answer carries the entry that says so",
            f"agent.state={(data.get('agent') or {}).get('state')!r}",
        )
        check(
            500 <= took_ms < 30000,
            "it BLOCKED and then woke — it neither returned instantly nor "
            "burned the whole window",
            f"{took_ms:.0f} ms",
        )

    # The negative arm. The session is holding `blocked`; `idle` never comes.
    started = time.monotonic()
    data, err = await waiter.tool(
        {"action": "wait", "session_id": session, "until": ["idle"], "timeout_ms": 4000},
        timeout=60,
    )
    took_ms = (time.monotonic() - started) * 1000
    if data is None:
        check(False, "terminal{wait} answered on the negative arm", err)
    else:
        note(f"negative wait returned after {took_ms:.0f} ms: {json.dumps(data)[:300]}")
        check(
            data.get("outcome") == "timeout",
            "a state the session never enters answers `timeout`, not `reached`",
            f"outcome={data.get('outcome')!r}",
        )
        check(
            (data.get("agent") or {}).get("state") == "blocked",
            "the timeout carries the CURRENT entry, not a manufactured final state",
            f"agent.state={(data.get('agent') or {}).get('state')!r}",
        )
        check(
            took_ms >= 3800,
            "the window was actually spent",
            f"{took_ms:.0f} ms for a 4000 ms window",
        )
    await waiter.ws.close()


# --------------------------------------------------------------------------
# quiet
# --------------------------------------------------------------------------


async def stage_quiet(c):
    """Silence is a fact about output, never a state change (spec R2-3).

    Takes ~45 s: the fake goes silent for 35 s and `QUIET_AFTER_MS` is 30 s.
    """
    session = (await c.spawn(command="sh", cwd=f"{WORK}/spawn"))["session_id"]
    note(f"session {session}")
    await c.send(session, "echo qa-quiet-shell\n")
    await c.until(session, lambda e: True, 15, "the first frame")
    await c.send(session, f'export PATH="{BIN_DIR}:$PATH"\n')
    await c.send(session, "QUIET=1 claude\n")

    working, _ = await c.until(
        session,
        lambda e: e.get("agent") == "claude" and e.get("state") == "working",
        40,
        "the agent to reach working",
    )
    working = working or {}
    note(f"working: {working}")
    if working.get("state") != "working":
        check(False, "the session reached working before the silence began", str(working))
        return
    # Without this the stage proves nothing: a row that was ALREADY marked
    # quiet would satisfy the assertion below without any clock running.
    check(
        working.get("quiet_since") is None,
        "a session that just painted is not quiet",
        f"quiet_since={working.get('quiet_since')!r}",
    )

    started = time.monotonic()
    quiet, took = await c.until(
        session,
        lambda e: e.get("quiet_since") is not None,
        60,
        "the 30 s quiet clock to publish",
    )
    quiet = quiet or {}
    note(f"quiet after {took:.1f}s: {quiet}")
    check(
        quiet.get("quiet_since") is not None,
        "silence is published as `quiet_since`",
        f"quiet_since={quiet.get('quiet_since')!r}",
    )
    check(
        quiet.get("state") == "working",
        "SILENCE IS NOT IDLE — the state the working screen established stands",
        f"state={quiet.get('state')!r}",
    )
    check(
        quiet.get("agent") == "claude" and quiet.get("program") == "claude",
        "the identification survives the silence",
        f"agent={quiet.get('agent')!r} program={quiet.get('program')!r}",
    )
    check(
        25 <= took <= 45,
        "the mark appeared on the 30 s clock, not immediately",
        f"{took:.1f}s after the working screen",
    )

    # A frame ends it. Without this the mark could be a sticky flag that
    # nothing ever clears, and the stage above would not know the difference.
    cleared, _ = await c.until(
        session,
        lambda e: e.get("quiet_since") is None,
        30,
        "the next frame to clear the quiet mark",
    )
    cleared = cleared or {}
    note(f"after the next paint: {cleared}")
    check(
        cleared.get("quiet_since") is None,
        "a real frame clears the quiet mark",
        f"quiet_since={cleared.get('quiet_since')!r}",
    )
    check(
        cleared.get("state") == "blocked",
        "and the screen that broke the silence is the one now reported",
        f"state={cleared.get('state')!r}",
    )


# --------------------------------------------------------------------------
# cwd
# --------------------------------------------------------------------------


async def stage_cwd(c):
    """OSC 7 › foreground probe › spawn dir, over three real directories.

    Two sessions, because one cannot distinguish "OSC 7 won" from "the probe
    had nothing to say": the second runs the same binary in the same spawn
    directory and emits no OSC 7, so its answer can only have come from the
    probe. Both sessions spawn the fake DIRECTLY rather than typing it into a
    shell — an interactive shell may emit OSC 7 of its own (macOS's
    `/etc/bashrc_Apple_Terminal` does), which would put a fourth answer into a
    stage whose whole subject is which answer wins.

    What this stage does NOT prove: that the spawn directory is the third
    tier. Reaching it needs a probe that fails, which cannot be arranged from
    the wire — it only shows the spawn dir is the answer NEITHER session gave.
    """
    osc_dir = f"{WORK}/osc"
    probe_dir = f"{WORK}/probe"
    probe2_dir = f"{WORK}/probe2"
    spawn_dir = f"{WORK}/spawn"
    fake = os.path.join(BIN_DIR, "claude")

    both = await c.spawn(
        command=fake,
        cwd=spawn_dir,
        env={"QA_FAKE_CD": probe_dir, "QA_FAKE_OSC7": osc_dir, "PHASE_SECS": "2"},
    )
    osc_s = both["session_id"]
    probe_s = (
        await c.spawn(
            command=fake,
            cwd=spawn_dir,
            env={"QA_FAKE_CD": probe2_dir, "PHASE_SECS": "2"},
        )
    )["session_id"]
    note(f"osc session {osc_s}, probe-only session {probe_s}")

    a, _ = await c.until(
        osc_s, lambda e: e.get("program") == "claude", 30, "the OSC session to be probed"
    )
    b, _ = await c.until(
        probe_s, lambda e: e.get("program") == "claude", 30, "the probe-only session to be probed"
    )
    a, b = a or {}, b or {}
    note(f"osc row:   {a}")
    note(f"probe row: {b}")

    rows = await c.sessions()
    note(f"pty.list spawn dirs: {[(k[:8], v['cwd']) for k, v in rows.items()]}")

    # The three directories must actually differ, or nothing below discriminates.
    check(
        len({osc_dir, probe_dir, spawn_dir}) == 3,
        "the three cwd tiers are three different directories",
        f"{osc_dir} / {probe_dir} / {spawn_dir}",
    )
    check(
        a.get("program") == "claude" and b.get("program") == "claude",
        "the probe answered for BOTH sessions",
        f"programs={a.get('program')!r} / {b.get('program')!r}",
    )
    check(
        b.get("cwd") == probe2_dir,
        "with no OSC 7, the live cwd is the FOREGROUND PROCESS's, not the spawn dir",
        f"cwd={b.get('cwd')!r}, spawned in {spawn_dir}",
    )
    check(
        a.get("cwd") == osc_dir,
        "OSC 7 outranks both the probe's cwd and the spawn dir",
        f"cwd={a.get('cwd')!r}, probe was in {probe_dir}, spawned in {spawn_dir}",
    )
    check(
        rows.get(osc_s, {}).get("cwd") == spawn_dir,
        "`pty.list` still reports the SPAWN directory — the two cwds are "
        "different facts, not two spellings of one",
        f"pty.list cwd={rows.get(osc_s, {}).get('cwd')!r}",
    )


STAGES = {
    "identify": stage_identify,
    "wait": stage_wait,
    "quiet": stage_quiet,
    "cwd": stage_cwd,
}


async def main():
    if STAGE not in STAGES:
        print(f"unknown stage {STAGE}", file=sys.stderr)
        return 64
    c = await connect("main")
    try:
        await STAGES[STAGE](c)
    finally:
        for s in SPAWNED:
            try:
                await c.call("pty.close", {"session_id": s}, timeout=10)
            except Exception as exc:  # noqa: BLE001 - cleanup must not mask the verdict
                print(f"  ... could not close {s}: {exc}")
        await c.ws.close()
    return rc


sys.exit(asyncio.run(main()))
