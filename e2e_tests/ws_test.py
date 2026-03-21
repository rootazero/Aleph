#!/usr/bin/env python3
"""E2E test for Aleph team coordination system.

Strategy: use slash commands via chat.send, track tool_start/tool_end events,
and verify results in both streaming output AND SQLite database.
"""

import asyncio
import json
import sqlite3
import sys
from pathlib import Path
import websockets

TOKEN = "e93e1b11-0c29-43e4-8164-590852d2d45d:545048137890cd796382dfb9f880f0b82b216da3f8a122b0ed4d83974aad4c90"
WS_URL = "ws://127.0.0.1:18790/ws"
COORD_DB = Path.home() / ".aleph" / "data" / "coord.db"

passed = 0
failed = 0


def check(name, condition, detail=""):
    global passed, failed
    if condition:
        print(f"  ✓ {name}")
        passed += 1
    else:
        print(f"  ✗ {name} — {detail}" if detail else f"  ✗ {name}")
        failed += 1
    return condition


def db_query(sql, params=()):
    conn = sqlite3.connect(str(COORD_DB))
    conn.row_factory = sqlite3.Row
    rows = conn.execute(sql, params).fetchall()
    conn.close()
    return [dict(r) for r in rows]


def db_execute(sql, params=()):
    conn = sqlite3.connect(str(COORD_DB))
    conn.execute("PRAGMA foreign_keys = ON")
    conn.execute(sql, params)
    conn.commit()
    conn.close()


class AlephClient:
    def __init__(self):
        self.ws = None
        self.msg_id = 0

    async def connect(self):
        self.ws = await websockets.connect(WS_URL, max_size=10 * 1024 * 1024)
        self.msg_id += 1
        await self.ws.send(json.dumps({
            "jsonrpc": "2.0", "method": "connect",
            "params": {"token": TOKEN}, "id": self.msg_id
        }))
        resp = json.loads(await self.ws.recv())
        assert "result" in resp, f"Connect failed: {resp}"
        # Drain events
        try:
            while True:
                await asyncio.wait_for(self.ws.recv(), timeout=0.5)
        except Exception:
            pass

    async def rpc(self, method, params=None, timeout=10):
        self.msg_id += 1
        rid = self.msg_id
        await self.ws.send(json.dumps({
            "jsonrpc": "2.0", "method": method, "params": params or {}, "id": rid
        }))
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            remaining = deadline - asyncio.get_event_loop().time()
            if remaining <= 0:
                return {"error": "timeout"}
            raw = await asyncio.wait_for(self.ws.recv(), timeout=remaining)
            resp = json.loads(raw)
            if resp.get("id") == rid:
                return resp

    async def slash(self, cmd, timeout=30, new_session=False):
        """Send slash command, collect tool_start/tool_end pairs and text."""
        # Reconnect with fresh WebSocket to avoid context pollution
        if new_session:
            await self.close()
            await self.connect()

        self.msg_id += 1
        rid = self.msg_id
        await self.ws.send(json.dumps({
            "jsonrpc": "2.0", "method": "chat.send",
            "params": {"message": cmd, "channel": "gui:chat", "stream": True},
            "id": rid
        }))

        tools = {}  # seq -> {name, output, error}
        text_parts = []
        deadline = asyncio.get_event_loop().time() + timeout

        while True:
            remaining = deadline - asyncio.get_event_loop().time()
            if remaining <= 0:
                break
            try:
                raw = await asyncio.wait_for(self.ws.recv(), timeout=min(remaining, 2.0))
                d = json.loads(raw)
                m = d.get("method", "")
                p = d.get("params", {})

                if d.get("id") == rid:
                    continue
                if m == "stream.tool_start":
                    name = p.get("tool_name", "") or p.get("tool_id", "")
                    tools[name] = {"name": name, "output": None, "error": None}
                elif m == "stream.tool_end":
                    name = p.get("tool_id", "") or p.get("tool_name", "")
                    result = p.get("result", {}) or {}
                    tools[name] = {
                        "name": name,
                        "output": result.get("output", ""),
                        "error": result.get("error"),
                    }
                elif m == "stream.response_chunk":
                    text_parts.append(p.get("content", ""))
                elif m in ("stream.run_complete", "stream.done", "stream.error"):
                    # Wait a moment for any trailing events after run_complete
                    try:
                        while True:
                            await asyncio.wait_for(self.ws.recv(), timeout=1.0)
                    except (asyncio.TimeoutError, Exception):
                        pass
                    break
            except asyncio.TimeoutError:
                # Only break on silence if we already saw run_complete
                continue

        return {
            "tools": list(tools.values()),
            "text": "".join(text_parts),
        }

    async def close(self):
        if self.ws:
            await self.ws.close()


async def main():
    global passed, failed

    # Clean ALL test data (full reset)
    try:
        db_execute("DELETE FROM coord_task_dependencies")
        db_execute("DELETE FROM coord_team_members")
        db_execute("DELETE FROM coord_tasks")
        db_execute("DELETE FROM coord_teams")
    except Exception as e:
        print(f"Warning: cleanup failed: {e}")

    # Generate unique test prefix to avoid session history conflicts
    import time
    prefix = f"e2e-{int(time.time()) % 100000}"

    client = AlephClient()
    await client.connect()
    print(f"✓ Connected (prefix: {prefix})\n")

    # ═══════════════════════════════════════════════
    print("═══ Phase 1: Infrastructure Verification ═══")
    # ═══════════════════════════════════════════════

    resp = await client.rpc("agents.tools_schema")
    groups = resp.get("result", {}).get("groups", [])
    gids = [g["id"] for g in groups]
    check("Tool categories loaded", len(groups) > 0)
    check("'team' category", "team" in gids)
    check("'spawn' category", "spawn" in gids)
    check("'delegate' category", "delegate" in gids)
    team_cat = next((g for g in groups if g["id"] == "team"), {})
    tnames = [t["name"] for t in team_cat.get("tools", [])]
    check("8 tools in team category", len(tnames) == 8, f"{len(tnames)}: {tnames}")

    resp = await client.rpc("commands.list")
    cmds = json.dumps(resp.get("result", {}))
    check("/team namespace", "team" in cmds)
    check("/task namespace", "task" in cmds)

    # ═══════════════════════════════════════════════
    print("\n═══ Phase 2: Team Lifecycle ═══")
    # ═══════════════════════════════════════════════

    # -- 2.1: team_create --
    print("\n--- 2.1: /team create ---")
    r = await client.slash(f"/team create --name {prefix} --leader main --description DAG-test")
    tl = [t["name"] for t in r["tools"]]
    # LLM may call extra tools (team_list) alongside team_create — check DB as ground truth
    teams = db_query("SELECT * FROM coord_teams WHERE id = ?", (prefix,))
    check("team_create executed (DB verified)", len(teams) == 1, f"tools called: {tl}")

    # -- 2.2: task_create A (no deps) --
    print("\n--- 2.2: /task create A ---")
    r = await client.slash(f"/task create --subject TaskA --team_id {prefix} --owner main --priority high")
    tl = [t["name"] for t in r["tools"]]
    check("task_create called (A)", "task_create" in tl, f"tools: {tl}")
    tasks_a = db_query("SELECT id, status, priority FROM coord_tasks WHERE team_id=? AND subject='TaskA'", (prefix,))
    check("Task A in DB", len(tasks_a) == 1)
    task_a_id = tasks_a[0]["id"] if tasks_a else None
    if task_a_id:
        check("Task A pending", tasks_a[0]["status"] == "pending")
        check("Task A high priority", tasks_a[0]["priority"] == "high")

    # -- 2.3: task_create B (blocked by A) --
    print("\n--- 2.3: /task create B (blocked by A) ---")
    task_b_id = None
    if task_a_id:
        r = await client.slash(f"/task create --subject TaskB --team_id {prefix} --owner main --blocked_by {task_a_id}")
        tl = [t["name"] for t in r["tools"]]
        check("task_create called (B)", "task_create" in tl, f"tools: {tl}")
        tasks_b = db_query("SELECT id FROM coord_tasks WHERE team_id=? AND subject='TaskB'", (prefix,))
        task_b_id = tasks_b[0]["id"] if tasks_b else None
        check("Task B in DB", task_b_id is not None)
        if task_b_id:
            deps = db_query("SELECT * FROM coord_task_dependencies WHERE task_id=? AND depends_on=?", (task_b_id, task_a_id))
            check("B→A dependency edge", len(deps) == 1)

    # -- 2.4: task_create C (blocked by B) --
    print("\n--- 2.4: /task create C (blocked by B) ---")
    task_c_id = None
    if task_b_id:
        r = await client.slash(f"/task create --subject TaskC --team_id {prefix} --owner main --blocked_by {task_b_id}")
        tasks_c = db_query("SELECT id FROM coord_tasks WHERE team_id=? AND subject='TaskC'", (prefix,))
        task_c_id = tasks_c[0]["id"] if tasks_c else None
        check("Task C in DB (chain A→B→C)", task_c_id is not None)

    # -- 2.5: task_list — verify blocked --
    print("\n--- 2.5: /task list ---")
    r = await client.slash(f"/task list --team_id {prefix}")
    tl = [t["name"] for t in r["tools"]]
    check("task_list called", "task_list" in tl, f"tools: {tl}")
    if r["tools"]:
        out = r["tools"][0].get("output", "")
        check("Blocked status in output", "blocked" in out.lower(), f"output: {out[:150]}")
        print(f"    → Board: {out[:300]}")

    # -- 2.6: Complete A, verify B unblocks --
    print("\n--- 2.6: Complete A → B unblocks ---")
    if task_a_id:
        r = await client.slash(f"/task update --task_id {task_a_id} --status completed --result Done")
        tl = [t["name"] for t in r["tools"]]
        check("task_update called", "task_update" in tl, f"tools: {tl}")
        if r["tools"]:
            # After completing A, the output should show A as completed and B should no longer be blocked
            all_output = " ".join(t.get("output", "") for t in r["tools"])
            check("Completion reflected in output",
                  "completed" in all_output.lower() or "unblock" in all_output.lower(),
                  f"output: {all_output[:150]}")

        a_row = db_query("SELECT status FROM coord_tasks WHERE id=?", (task_a_id,))
        check("A completed in DB", a_row and a_row[0]["status"] == "completed")
    else:
        print("  (skipped — no task A)")

    # -- 2.7: Complete B, verify C unblocks --
    print("\n--- 2.7: Complete B → C unblocks ---")
    if task_b_id:
        r = await client.slash(f"/task update --task_id {task_b_id} --status completed --result Done")
        tl = [t["name"] for t in r["tools"]]
        check("task_update called (B)", "task_update" in tl, f"tools: {tl}")
    else:
        print("  (skipped — no task B)")

    # -- 2.8: team_disband --
    print("\n--- 2.8: /team disband ---")
    r = await client.slash(f"/team disband --team_id {prefix}")
    tl = [t["name"] for t in r["tools"]]
    check("team_disband called", "team_disband" in tl, f"tools: {tl}")
    t = db_query("SELECT status FROM coord_teams WHERE id=?", (prefix,))
    check("Team disbanded in DB", t and t[0]["status"] == "disbanded")

    # ═══════════════════════════════════════════════
    print("\n═══ Phase 3: Template Launch ═══")
    # ═══════════════════════════════════════════════

    print("\n--- 3.1: /team launch ---")
    r = await client.slash('/team launch --template code-review-team --variables {"goal":"E2E test"}', timeout=15)
    tl = [t["name"] for t in r["tools"]]
    check("team_launch called", "team_launch" in tl, f"tools: {tl}")
    if r["tools"]:
        err = r["tools"][0].get("error")
        check("team_launch no error", err is None, f"error: {err}")

    tteams = db_query("SELECT id FROM coord_teams WHERE id LIKE '%code-review%'")
    check("Template team in DB", len(tteams) > 0)
    if tteams:
        tid = tteams[0]["id"]
        members = db_query("SELECT agent_id, role FROM coord_team_members WHERE team_id=?", (tid,))
        tasks = db_query("SELECT subject, owner, status FROM coord_tasks WHERE team_id=?", (tid,))
        deps = db_query("""
            SELECT t1.subject AS child, t2.subject AS parent
            FROM coord_task_dependencies d
            JOIN coord_tasks t1 ON t1.id = d.task_id
            JOIN coord_tasks t2 ON t2.id = d.depends_on
            WHERE t1.team_id = ?
        """, (tid,))
        check("4 members", len(members) == 4, f"got {len(members)}")
        check("4 tasks", len(tasks) == 4, f"got {len(tasks)}")
        check("4 dependency edges", len(deps) == 4, f"got {len(deps)}")
        print("    Members:")
        for m in members:
            print(f"      → {m['agent_id']}: {m['role']}")
        print("    Tasks:")
        for t in tasks:
            print(f"      → {t['subject']} [{t['status']}] owner={t['owner']}")
        print("    DAG:")
        for d in deps:
            print(f"      → {d['child']} ← {d['parent']}")

    # ═══════════════════════════════════════════════
    print("\n═══ Phase 4: DAG Integrity ═══")
    # ═══════════════════════════════════════════════

    # Immutable edges: {prefix} dependencies still exist even after completion
    if task_a_id and task_b_id:
        deps = db_query("SELECT * FROM coord_task_dependencies WHERE task_id=? AND depends_on=?", (task_b_id, task_a_id))
        check("Edge B→A preserved after completion", len(deps) == 1)
    if task_b_id and task_c_id:
        deps = db_query("SELECT * FROM coord_task_dependencies WHERE task_id=? AND depends_on=?", (task_c_id, task_b_id))
        check("Edge C→B preserved after completion", len(deps) == 1)

    # ═══════════════════════════════════════════════
    print(f"\n{'═' * 50}")
    print(f"RESULTS: {passed} passed, {failed} failed")
    print(f"{'═' * 50}")

    await client.close()
    return failed == 0


if __name__ == "__main__":
    ok = asyncio.run(main())
    sys.exit(0 if ok else 1)
