#!/usr/bin/env python3
"""Exercise Memory controls and raw/chat continuity against one owned server."""
import argparse
import asyncio
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "qa/browser_managed"))
from qa_rpc import Rpc, ran, ws_connect

IS_WINDOWS = os.name == "nt"
if IS_WINDOWS:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from windows_console import deliver_control_event, prepare_console_parent, spawn_creation_flags


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


class Probe:
    def __init__(self, args):
        self.args = args
        self.root = Path(tempfile.mkdtemp(prefix="aleph-memory-lifecycle-", dir=args.artifacts))
        self.home = self.root / "home"
        self.aleph = self.home / ".aleph"
        self.aleph.mkdir(parents=True)
        self.port = free_port()
        self.provider_port = free_port()
        self.checks = []
        self.first_note = None
        self.completed = False
        self.process = None
        self.provider = None
        self.streams = []
        self.binary = self.root / "aleph-server"
        self.env = {
            key: value for key, value in os.environ.items()
            if not any(part in key.upper() for part in ("API_KEY", "AUTH_TOKEN", "ANTHROPIC", "OPENAI", "ALEPH_"))
        }
        self.env.update({
            "RUSTUP_HOME": os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup")),
            "CARGO_HOME": os.environ.get("CARGO_HOME", str(Path.home() / ".cargo")),
            "HOME": str(self.home), "ALEPH_HOME": str(self.aleph),
            "RUST_LOG": "info", "NO_PROXY": "127.0.0.1,localhost",
        })
        self.env.pop("RUST_MIN_STACK", None)

    def deliver_shutdown_signal(self, sig):
        """Ask the owned server to shut down through the platform's real control path.

        Windows has no SIGTERM: `Popen.send_signal(SIGTERM)` is `TerminateProcess`
        (a hard kill) and `send_signal(SIGINT)` raises `ValueError`. The gateway
        only implements a graceful Windows shutdown for Ctrl-C, so the Windows
        path delivers a real `CTRL_C_EVENT` console control event instead.
        """
        if not IS_WINDOWS:
            self.process.send_signal(sig)
            return {"signal": int(sig)}
        result = deliver_control_event(self.process.pid, "ctrl_c")
        self.check("Windows console control event delivered", bool(result.get("generated")), result)
        return result

    def check(self, name, condition, detail=""):
        self.checks.append({"name": name, "pass": bool(condition), "detail": detail})
        print(("PASS " if condition else "FAIL ") + name, str(detail)[:350], flush=True)
        if not condition:
            raise AssertionError(name)

    def prepare(self):
        self.check("input artifact digest", digest(self.args.binary) == self.args.sha256)
        shutil.copyfile(self.args.binary, self.binary)
        self.binary.chmod(0o755)
        self.check("private artifact digest", digest(self.binary) == self.args.sha256)
        provenance = {"sha256": self.args.sha256, "input": str(self.args.binary)}
        if self.args.build_record:
            built = json.loads(self.args.build_record.read_text())
            self.check("build provenance digest", built["sha256"] == self.args.sha256)
            provenance["build"] = built
        (self.root / "artifact.json").write_text(json.dumps(provenance, indent=2) + "\n", encoding="utf-8")
        provider_config = "" if self.args.providerless else f'''[general]
default_provider = "memory-qa"
[providers.memory-qa]
enabled = true
verified = true
protocol = "anthropic"
base_url = "http://127.0.0.1:{self.provider_port}"
api_key = "qa-dummy-not-a-real-key"
models = ["memory-qa-model"]
timeout_seconds = 30
'''
        (self.aleph / "config.toml").write_text(provider_config + f'''[gateway]
host = "127.0.0.1"
port = {self.port}
[agents]
[[agents.list]]
id = "main"
default = true
name = "Main Agent"
[memory]
enabled = false
[memory.dreaming]
enabled = false
[memory.profile]
enabled = false
[memory.embedding]
providers = []
active_provider_id = ""
[cron]
enabled = false
[heartbeat]
enabled = false
[mcp]
enabled = false
[acp]
enabled = false
[evolution]
enabled = false
[skills]
enabled = false
''', encoding="utf-8")
        if self.args.providerless:
            return
        stream = (self.root / "provider.log").open("w")
        self.streams.append(stream)
        self.provider = subprocess.Popen([
            sys.executable, str(ROOT / "qa/memory_lifecycle/provider.py"),
            str(self.provider_port), str(self.root / "provider-requests.jsonl"),
        ], stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)

    async def boot(self, name):
        self.offsets = {path: path.stat().st_size for path in (self.aleph / "logs").glob("*") if path.is_file()}
        self.log = self.root / (name + ".log")
        stream = self.log.open("w")
        self.streams.append(stream)
        spawn_options = {}
        if IS_WINDOWS:
            # The gateway's graceful Windows path is Ctrl-C, and a console
            # control event only reaches processes that share one console.
            prepare_console_parent()
            spawn_options["creationflags"] = spawn_creation_flags()
        else:
            spawn_options["start_new_session"] = True
        self.process = subprocess.Popen([
            str(self.binary), "--port", str(self.port), "start",
        ], cwd=self.root, env=self.env, stdout=stream, stderr=subprocess.STDOUT, **spawn_options)
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline and self.process.poll() is None:
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/health", timeout=1) as response:
                    if response.status == 200:
                        break
            except OSError:
                pass
            await asyncio.sleep(0.1)
        else:
            self.check(name + ": listener ready", False, self.process.poll())
        self.check(name + ": owned process alive", self.process.poll() is None, self.process.pid)
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            self.endpoints = list(self.home.rglob(".ipc-endpoint.json"))
            if self.endpoints:
                break
            await asyncio.sleep(0.05)
        self.check(name + ": endpoint published", bool(self.endpoints))

    async def tool(self, rpc, name, arguments=None):
        ok, value = await rpc.invoke(name, arguments or {})
        self.check(name + ": production tool succeeded", ran(ok, value), value)
        return value

    async def chat_off(self, rpc, label):
        marker = "Memory lifecycle raw chat " + label
        sent = await rpc.call("chat.send", {"message": marker, "channel": "gui:memory-lifecycle-" + label, "agent_id": "main"})
        self.check(label + ": chat admitted Off", "result" in sent, sent)
        key = sent["result"]["session_key"]
        deadline = time.monotonic() + 45
        messages = []
        def has_reply(rows):
            return any(
                row.get("role") == "assistant"
                and (bool(str(row.get("content", "")).strip()) if self.args.providerless else "pong" in str(row.get("content", "")))
                for row in rows
            )
        while time.monotonic() < deadline:
            history = await rpc.call("sessions.history", {"session_key": key, "limit": 50})
            messages = history.get("result", {}).get("messages", [])
            if has_reply(messages):
                break
            await asyncio.sleep(0.1)
        self.check(label + (": providerless reply persisted" if self.args.providerless else ": real provider reply persisted"), has_reply(messages), messages)
        self.check(label + ": user message persisted", any(marker in str(row.get("content", "")) for row in messages))
        deleted = await rpc.call("sessions.delete", {"session_key": key})
        # Windows note: the file backend archives a session by RENAMING its
        # directory. Windows refuses that rename while the directory still has
        # an open child handle (ERROR_ACCESS_DENIED / os error 5), and the
        # store only retries NAME collisions, not this transient denial. Retry
        # here so one transient sharing violation cannot decide the whole
        # lifecycle verdict; every attempt is recorded in the check detail.
        attempts = 1
        while "result" not in deleted and attempts < 6 and "Archive session failed" in str(deleted):
            await asyncio.sleep(0.5)
            deleted = await rpc.call("sessions.delete", {"session_key": key})
            attempts += 1
        self.check(
            label + ": session-tail capture requested",
            "result" in deleted,
            {"attempts": attempts, "response": deleted},
        )
        deadline = time.monotonic() + 10
        found = False
        while time.monotonic() < deadline:
            for path in self.aleph.rglob("*.md"):
                if marker in path.read_text(encoding="utf-8", errors="replace"):
                    found = True
                    break
            if found:
                break
            await asyncio.sleep(0.05)
        self.check(label + ": real raw Markdown archive", found)

    def provider_command(self, method, body=None):
        request = urllib.request.Request(
            f"http://127.0.0.1:{self.provider_port}/" + method,
            data=None if body is None else json.dumps(body).encode(),
            headers={"content-type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=2) as response:
            return json.load(response)

    async def hold_memory_during_signal(self, rpc, name, sig):
        marker = "heldmemorydrainatlas" + name.replace("-", "")
        await asyncio.to_thread(self.provider_command, "arm", {"marker": marker})
        await self.tool(rpc, "note_manage", {
            "action": "create", "category": "reference", "filename": "held-" + name,
            "title": marker, "content": marker + " retains actual Memory work through shutdown.",
        })
        work = asyncio.create_task(rpc.invoke("memory_reflect", {"query": marker}))
        try:
            deadline = time.monotonic() + 15
            entered = False
            while time.monotonic() < deadline:
                status = await asyncio.to_thread(self.provider_command, "status")
                if status["entered"]:
                    entered = True
                    break
                if work.done():
                    self.check(name + ": real Memory provider entry", False, work.result())
                await asyncio.sleep(0.05)
            self.check(name + ": real Memory provider entry", entered)
            self.deliver_shutdown_signal(sig)
            try:
                code = await asyncio.to_thread(self.process.wait, timeout=5.5)
            except subprocess.TimeoutExpired:
                self.check(name + ": shutdown retains work beyond former five-second exit", True)
            else:
                self.check(name + ": shutdown retains work beyond former five-second exit", False, code)
        finally:
            await asyncio.to_thread(self.provider_command, "release", {})
            work.cancel()
            await asyncio.gather(work, return_exceptions=True)
        deadline = time.monotonic() + 10
        completed = False
        while time.monotonic() < deadline:
            status = await asyncio.to_thread(self.provider_command, "status")
            if status["completed"]:
                completed = True
                break
            await asyncio.sleep(0.05)
        self.check(name + ": released real provider request completed", completed)

    async def scenario(self, name, sig, first):
        await self.boot(name)
        async with ws_connect(f"ws://127.0.0.1:{self.port}/ws") as ws:
            rpc = Rpc(ws)
            self.check(name + ": connected", "result" in await rpc.connect("memory-lifecycle"))
            status = await self.tool(rpc, "memory_runtime_status")
            self.check(name + ": initially disabled", status.get("state") == "off", status)
            await self.chat_off(rpc, name + "-cold")
            if first:
                self.check(name + ": disabled boot created no orientation", not any(self.aleph.rglob("SCHEMA.md")))
            started = await self.tool(rpc, "memory_runtime_start")
            self.check(name + ": activation completed", started.get("state") == "running", started)
            await self.tool(rpc, "note_manage", {
                "action": "create", "category": "reference", "filename": "runtime-" + name,
                "title": "Runtime lifecycle " + name, "content": "Persistent runtime lifecycle marker " + name,
            })
            stopped = await self.tool(rpc, "memory_runtime_stop")
            self.check(name + ": source stopped", stopped.get("state") == "off", stopped)
            rejected = await rpc.call("memory.stats", {})
            error = rejected.get("error", {})
            self.check(name + ": explicit Memory RPC refused", error.get("code") == -32099 and error.get("message") == "memory_unavailable" and error.get("data", {}).get("state") == "off", rejected)
            rejected_name = "rejected-" + name
            ok, body = await rpc.invoke("note_manage", {
                "action": "create", "category": "reference", "filename": rejected_name,
                "title": "Rejected lifecycle note", "content": "This write must remain unavailable.",
            })
            self.check(name + ": Memory tool refused Off", not ran(ok, body), body)
            await self.chat_off(rpc, name + "-stopped")
            self.check(name + ": rejected write had no Markdown effect", not any(self.aleph.rglob(rejected_name + ".md")))
            restarted = await self.tool(rpc, "memory_runtime_start")
            self.check(name + ": reactivation completed", restarted.get("state") == "running", restarted)
            stats = await rpc.call("memory.stats", {})
            self.check(name + ": fresh Memory RPC succeeds", "result" in stats, stats)
            if not self.args.providerless:
                await self.hold_memory_during_signal(rpc, name, sig)
        self.check(name + ": real note persisted", any(self.aleph.rglob("runtime-" + name + ".md")))
        if first:
            self.first_note = "runtime-" + name
        if not first:
            self.check(name + ": previous boot note survived", any(self.aleph.rglob(self.first_note + ".md")))
        if self.args.providerless:
            self.deliver_shutdown_signal(sig)
        try:
            code = await asyncio.to_thread(self.process.wait, timeout=45)
        except subprocess.TimeoutExpired:
            self.process.kill()
            code = await asyncio.to_thread(self.process.wait)
            self.check(name + ": signal drain completed", False, "external 45-second guard expired; owned process killed")
        text = self.log.read_text(encoding="utf-8", errors="replace")
        for path in (self.aleph / "logs").glob("*"):
            if path.is_file():
                with path.open("rb") as stream:
                    stream.seek(self.offsets.get(path, 0))
                    text += stream.read().decode("utf-8", errors="replace")
        (self.root / (name + "-combined.log")).write_text(text, encoding="utf-8")
        self.check(name + ": orderly process exit", code == 0, code)
        if IS_WINDOWS:
            # Windows has no SIGTERM at all; the graceful path is the Ctrl-C
            # handler, and the gateway records it as `signal=ctrl_c`.
            self.check(name + ": signal path observed", "signal=ctrl_c" in text)
        else:
            self.check(name + ": signal path observed", "SIGTERM" in text if sig == signal.SIGTERM else "ctrl_c" in text)
        self.check(name + ": watchdog not forced", "forcing exit" not in text and "graceful shutdown did not complete" not in text)
        self.check(name + ": endpoint removed", all(not path.exists() for path in self.endpoints))
        with socket.socket() as client:
            client.settimeout(1)
            self.check(name + ": listener removed", client.connect_ex(("127.0.0.1", self.port)) != 0)
        for path in self.aleph.rglob("*.db"):
            with sqlite3.connect(path.resolve().as_uri() + "?mode=ro", uri=True) as database:
                self.check(name + ": SQLite integrity " + path.name, database.execute("PRAGMA integrity_check").fetchone()[0] == "ok")
        self.process = None

    def cleanup(self):
        for process in [self.process, self.provider]:
            if process is not None and process.poll() is None:
                if IS_WINDOWS and process is self.process:
                    deliver_control_event(process.pid, "ctrl_c")
                else:
                    process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        for stream in self.streams:
            stream.close()
        if not self.completed:
            self.checks.append({"name": "complete lifecycle scenario", "pass": False, "detail": "scenario ended before all required stages completed"})
        (self.root / "results.json").write_text(json.dumps({"complete": self.completed, "providerless": self.args.providerless, "checks": self.checks}, indent=2) + "\n", encoding="utf-8")
        print(f"Checks: {sum(row['pass'] for row in self.checks)}/{len(self.checks)}; artifacts: {self.root}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--build-record", type=Path)
    parser.add_argument("--providerless", action="store_true", help="Use the actual providerless registry and simulated chat path; start no model stub.")
    parser.add_argument("--artifacts", type=Path, default=Path(tempfile.gettempdir()))
    args = parser.parse_args()
    probe = Probe(args)
    try:
        probe.prepare()
        if IS_WINDOWS:
            # SIGTERM does not exist on Windows, so the two scenarios are named
            # after the control event that is actually delivered.
            first, second = "ctrlc", "ctrlc-restart"
        else:
            first, second = "sigterm", "sigint-restart"
        asyncio.run(probe.scenario(first, signal.SIGINT, True))
        asyncio.run(probe.scenario(second, signal.SIGINT, False))
        probe.check("private binary remained pinned", digest(probe.binary) == args.sha256)
        probe.completed = True
    finally:
        probe.cleanup()


if __name__ == "__main__":
    main()
