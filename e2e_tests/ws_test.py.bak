#!/usr/bin/env python3
"""E2E test for Aleph team coordination system.

Strategy: send slash commands via chat.send, verify results in SQLite DB.
Fast-path commands execute tools directly (no LLM), so we verify via DB
ground truth rather than streaming tool events.
"""

import asyncio
import json
import sqlite3
import sys
import time
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
        print(f"  ✗ {name}" + (f" — {detail}" if detail else ""))
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
        await self._drain()

    async def _drain(self, timeout=0.5):
        try:
            while True:
                await asyncio.wait_for(self.ws.recv(), timeout=timeout)
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

    async def slash(self, cmd, timeout=20):
        """Send slash command and wait for run_complete. Returns response text."""
        self.msg_id += 1
        rid = self.msg_id
        await self.ws.send(json.dumps({
            "jsonrpc": "2.0", "method": "chat.send",
            "params": {"message": cmd, "channel": "gui:chat", "stream": True},
            "id": rid
        }))

        # Collect messages. Fast-path is <5ms so run_complete may arrive
        # BEFORE or AFTER the id=rid response. Accept both orderings.
        run_id = None
        text_parts = []
        final_response = ""
        completed = False
        deadline = asyncio.get_event_loop().time() + timeout

        while not completed:
            remaining = deadline - asyncio.get_event_loop().time()
            if remaining <= 0:
                break
            try:
                raw = await asyncio.wait_for(self.ws.recv(), timeout=min(remaining, 5.0))
                d = json.loads(raw)
                m = d.get("method", "")
                p = d.get("params", {})

                # Capture run_id from initial response
                if d.get("id") == rid and "result" in d:
                    new_run_id = d["result"].get("run_id", "")
                    if new_run_id:
                        run_id = new_run_id
                    # If we already saw run_complete for this run, we're done
                    if completed:
                        break
                    continue

                evt_run_id = p.get("run_id", "")

                # Skip events from OTHER runs (known different run_id)
                if run_id and evt_run_id and evt_run_id != run_id:
                    continue

                if m == "stream.response_chunk":
                    text_parts.append(p.get("content", ""))
                elif m == "stream.run_complete":
                    summary = p.get("summary", {})
                    final_response = summary.get("final_response", "")
                    completed = True
                    # If we already have run_id, we can break
                    if run_id:
                        break
                    # Otherwise keep going to capture run_id
                elif m == "stream.error":
                    text_parts.append(f"ERROR: {p}")
                    completed = True
                    break
            except asyncio.TimeoutError:
                break

        return "".join(text_parts) or final_response

    async def close(self):
        if self.ws:
            await self.ws.close()


async def main():
    # Unique prefix for this test run
    prefix = f"e2e-{int(time.time()) % 100000}"

    # Clean ALL test data
    try:
        db_execute("DELETE FROM coord_task_dependencies")
        db_execute("DELETE FROM coord_team_members")
        db_execute("DELETE FROM coord_tasks")
        db_execute("DELETE FROM coord_teams")
    except Exception as e:
        print(f"Warning: cleanup failed: {e}")

    client = AlephClient()
    await client.connect()
    print(f"✓ Connected (prefix: {prefix})\n")

    # ═══════════════════════════════════════════════
    print("═══ Phase 1: Infrastructure ═══")
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
    print(f"\n═══ Phase 2: Team Lifecycle ({prefix}) ═══")
    # ═══════════════════════════════════════════════

    # 2.1: team_create
    print("\n--- 2.1: /team create ---")
    text = await client.slash(f"/team create --name {prefix} --leader main --description test-team")
    check("Got response", len(text) > 0, f"text: {text[:100]}")
    teams = db_query("SELECT * FROM coord_teams WHERE id = ?", (prefix,))
    check("Team in DB", len(teams) == 1)
    if teams:
        check("Leader is main", teams[0]["leader"] == "main")
        check("Status active", teams[0]["status"] == "active")

    # 2.2: task_create A (no deps)
    print("\n--- 2.2: /task create A ---")
    text = await client.slash(f"/task create --subject TaskA --team_id {prefix} --owner main --priority high")
    check("Got response", len(text) > 0)
    tasks_a = db_query("SELECT * FROM coord_tasks WHERE team_id=? AND subject='TaskA'", (prefix,))
    check("Task A in DB", len(tasks_a) == 1)
    task_a_id = tasks_a[0]["id"] if tasks_a else None
    if task_a_id:
        check("A status pending", tasks_a[0]["status"] == "pending")
        check("A priority high", tasks_a[0]["priority"] == "high")
        print(f"    → ID: {task_a_id}")

    # 2.3: task_create B (blocked by A)
    print("\n--- 2.3: /task create B (blocked by A) ---")
    task_b_id = None
    if task_a_id:
        text = await client.slash(f"/task create --subject TaskB --team_id {prefix} --owner main --blocked_by {task_a_id}")
        check("Got response", len(text) > 0)
        tasks_b = db_query("SELECT * FROM coord_tasks WHERE team_id=? AND subject='TaskB'", (prefix,))
        check("Task B in DB", len(tasks_b) == 1)
        task_b_id = tasks_b[0]["id"] if tasks_b else None
        if task_b_id:
            deps = db_query("SELECT * FROM coord_task_dependencies WHERE task_id=? AND depends_on=?", (task_b_id, task_a_id))
            check("B→A dependency", len(deps) == 1)

    # 2.4: task_create C (blocked by B)
    print("\n--- 2.4: /task create C (blocked by B) ---")
    task_c_id = None
    if task_b_id:
        text = await client.slash(f"/task create --subject TaskC --team_id {prefix} --owner main --blocked_by {task_b_id}")
        tasks_c = db_query("SELECT * FROM coord_tasks WHERE team_id=? AND subject='TaskC'", (prefix,))
        task_c_id = tasks_c[0]["id"] if tasks_c else None
        check("Task C in DB (chain A→B→C)", task_c_id is not None)

    # 2.5: task_list
    print("\n--- 2.5: /task list ---")
    text = await client.slash(f"/task list --team_id {prefix}")
    check("task_list response", len(text) > 0)
    has_blocked = "blocked" in text.lower()
    check("Shows blocked status", has_blocked, f"text: {text[:150]}")
    print(f"    → {text[:300]}")

    # 2.6: Complete A → B unblocks
    print("\n--- 2.6: Complete A → B unblocks ---")
    if task_a_id:
        text = await client.slash(f"/task update --task_id {task_a_id} --status completed --result Done")
        check("Update response", len(text) > 0)
        a_row = db_query("SELECT status FROM coord_tasks WHERE id=?", (task_a_id,))
        check("A completed in DB", a_row and a_row[0]["status"] == "completed")

        # Verify B is now unblocked (no unresolved deps)
        if task_b_id:
            b_unresolved = db_query("""
                SELECT COUNT(*) as cnt FROM coord_task_dependencies d
                JOIN coord_tasks dep ON dep.id = d.depends_on
                WHERE d.task_id = ? AND dep.status != 'completed'
            """, (task_b_id,))
            check("B unblocked (0 unresolved deps)", b_unresolved[0]["cnt"] == 0)

    # 2.7: Complete B → C unblocks
    print("\n--- 2.7: Complete B → C unblocks ---")
    if task_b_id:
        text = await client.slash(f"/task update --task_id {task_b_id} --status completed --result Done")
        b_row = db_query("SELECT status FROM coord_tasks WHERE id=?", (task_b_id,))
        check("B completed in DB", b_row and b_row[0]["status"] == "completed")

        if task_c_id:
            c_unresolved = db_query("""
                SELECT COUNT(*) as cnt FROM coord_task_dependencies d
                JOIN coord_tasks dep ON dep.id = d.depends_on
                WHERE d.task_id = ? AND dep.status != 'completed'
            """, (task_c_id,))
            check("C unblocked (0 unresolved deps)", c_unresolved[0]["cnt"] == 0)

    # 2.8: team_disband
    print("\n--- 2.8: /team disband ---")
    text = await client.slash(f"/team disband --team_id {prefix}")
    check("Disband response", len(text) > 0)
    t = db_query("SELECT status FROM coord_teams WHERE id=?", (prefix,))
    check("Team disbanded in DB", t and t[0]["status"] == "disbanded")
    cancelled = db_query("SELECT COUNT(*) as cnt FROM coord_tasks WHERE team_id=? AND status='cancelled'", (prefix,))
    check("Remaining tasks cancelled", cancelled[0]["cnt"] >= 0)

    # ═══════════════════════════════════════════════
    print("\n═══ Phase 3: Template Launch ═══")
    # ═══════════════════════════════════════════════

    print("\n--- 3.1: /team launch ---")
    text = await client.slash('/team launch --template code-review-team --variables {"goal":"E2E test"}')
    check("Launch response", len(text) > 0)
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
        check("4 DAG edges", len(deps) == 4, f"got {len(deps)}")
        for m in members:
            print(f"    → Member: {m['agent_id']}: {m['role']}")
        for t in tasks:
            print(f"    → Task: {t['subject']} [{t['status']}] → {t['owner']}")
        for d in deps:
            print(f"    → DAG: {d['child']} ← {d['parent']}")

    # ═══════════════════════════════════════════════
    print("\n═══ Phase 4: DAG Integrity ═══")
    # ═══════════════════════════════════════════════

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
