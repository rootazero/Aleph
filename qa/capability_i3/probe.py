#!/usr/bin/env python3
"""Opt-in, isolated production integration probe; importing never starts services."""
from __future__ import annotations

import argparse
import asyncio
import contextlib
import hashlib
import json
import os
from pathlib import Path
import selectors
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlparse
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[2]
TOPICS = ["capability.event", "capability.event.status", "capabilities.ui.display"]


RPC_MESSAGE_LIMIT = 2048
SERVER_STREAM_LIMIT = 65536
CREDENTIAL_FIELDS = {"ticket", "bootstrap_ticket", "token", "device_token", "access_token", "refresh_token", "api_key", "authorization"}


class ProbeFailure(RuntimeError):
    def __init__(self, code, diagnostic=None, diagnostic_text=None, diagnostic_secrets=None):
        super().__init__(code)
        self.diagnostic = diagnostic
        self.diagnostic_text = diagnostic_text
        self.diagnostic_secrets = diagnostic_secrets


def remember_credentials(value, secrets):
    if isinstance(value, dict):
        for key, child in value.items():
            if key.lower() in CREDENTIAL_FIELDS and isinstance(child, str) and child:
                secrets.add(child)
            elif isinstance(child, (dict, list)):
                remember_credentials(child, secrets)
    elif isinstance(value, list):
        for child in value:
            remember_credentials(child, secrets)


class BoundedText:
    """Retain a byte prefix in memory; scrub known credentials before persistence."""
    def __init__(self, limit):
        self.limit = limit
        self.data = bytearray()
        self.total = 0

    def feed(self, chunk):
        self.total += len(chunk)
        self.data.extend(chunk[:max(0, self.limit - len(self.data))])

    def safe(self, secrets, incomplete=False):
        data = bytes(self.data)
        redacted = False
        truncated = self.total > len(data)
        for secret in sorted((item.encode("utf-8") for item in secrets if item), key=len, reverse=True):
            replaced = data.replace(secret, b"[REDACTED]")
            redacted |= replaced != data
            data = replaced
            if truncated or incomplete:
                # Uncertain boundaries can cut credentials; matching benign tails may also be redacted.
                for overlap in range(min(len(secret) - 1, len(data)), 0, -1):
                    if data.endswith(secret[:overlap]):
                        data = data[:-overlap] + b"[REDACTED]"
                        redacted = True
                        break
        rendered = data.decode("utf-8", "replace").encode("utf-8")
        text = rendered[:self.limit].decode("utf-8", "ignore")
        return {"text": text, "redacted": redacted, "truncated": truncated or len(rendered) > self.limit,
                "source_bytes": self.total, "retained_bytes": len(self.data), "limit_bytes": self.limit}


class ServerOutput:
    """Drain only the pipes returned by this probe's owned server Popen."""
    def __init__(self, process):
        self.pid = process.pid
        self.pgid = os.getpgid(process.pid)
        require(self.pgid == self.pid, "server_capture_own_process_group")
        self.streams = {"stdout": process.stdout, "stderr": process.stderr}
        self.buffers = {name: BoundedText(SERVER_STREAM_LIMIT) for name in self.streams}
        self.state = {name: {"eof": False, "read_error": None, "close_error": None} for name in self.streams}
        self.stopping = threading.Event()
        self.thread = threading.Thread(target=self.read, name="capability-i3-server-output", daemon=True)
        self.thread.start()

    def read(self):
        with selectors.DefaultSelector() as selector:
            try:
                for name, stream in self.streams.items():
                    os.set_blocking(stream.fileno(), False)
                    selector.register(stream, selectors.EVENT_READ, name)
                while selector.get_map() and not self.stopping.is_set():
                    for key, _ in selector.select(timeout=0.05):
                        name = key.data
                        try:
                            chunk = os.read(key.fd, 4096)
                        except BlockingIOError:
                            continue
                        except OSError as error:
                            self.state[name]["read_error"] = type(error).__name__
                            selector.unregister(key.fileobj)
                            continue
                        if chunk:
                            self.buffers[name].feed(chunk)
                        else:
                            self.state[name]["eof"] = True
                            selector.unregister(key.fileobj)
            except Exception as error:
                for state in self.state.values():
                    if not state["eof"] and state["read_error"] is None:
                        state["read_error"] = type(error).__name__
            finally:
                for name, stream in self.streams.items():
                    try:
                        stream.close()
                    except OSError as error:
                        self.state[name]["close_error"] = type(error).__name__

    def finish(self, secrets):
        join_error = None
        try:
            self.thread.join(timeout=1)
            if self.thread.is_alive():
                self.stopping.set()
                self.thread.join(timeout=1)
        except Exception as error:
            join_error = type(error).__name__
            self.stopping.set()
        joined = not self.thread.is_alive()
        return {"pid": self.pid, "pgid_at_capture": self.pgid, "joined": joined, "join_error": join_error,
                "stop_requested": self.stopping.is_set(), "streams": {
                    name: {**self.buffers[name].safe(secrets, incomplete=not state["eof"] or state["read_error"] is not None),
                           **state} for name, state in self.state.items()}}


def require(condition, name):
    if not condition:
        raise ProbeFailure(name)


def digest(path):
    value = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def topic_frame(message):
    if "topic" in message:
        return message
    params = message.get("params")
    return params if isinstance(params, dict) and "topic" in params else None


def source_record(frame, namespace, name):
    event = frame.get("data", {}).get("event", {})
    return frame.get("topic") == "capability.event" and event.get("id") == {"namespace": namespace, "name": name}


def audited_frames(frames):
    return [frame for frame in frames if frame.get("topic") in TOPICS[:2]]


class Rpc:
    """A single reader preserves notifications even while several RPCs await replies."""
    def __init__(self, socket_, secrets=None):
        self.socket = socket_
        self.secrets = secrets if secrets is not None else set()
        self.pending = {}
        self.notifications = []
        self.changed = asyncio.Condition()
        self.send_lock = asyncio.Lock()
        self.next_id = 0
        self.reader = asyncio.create_task(self.read())

    async def read(self):
        try:
            async for raw in self.socket:
                message = json.loads(raw)
                future = self.pending.get(message.get("id"))
                if future is not None and not future.done():
                    future.set_result(message)
                else:
                    async with self.changed:
                        require(len(self.notifications) < 10000, "notification_buffer_bound")
                        self.notifications.append(topic_frame(message) or message)
                        self.changed.notify_all()
        finally:
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(ProbeFailure("connection_closed"))

    async def call(self, method, params, timeout=45):
        remember_credentials(params, self.secrets)
        async with self.send_lock:
            self.next_id += 1
            request_id = self.next_id
            future = asyncio.get_running_loop().create_future()
            self.pending[request_id] = future
            await self.socket.send(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}))
        try:
            response = await asyncio.wait_for(future, timeout)
            remember_credentials(response.get("result"), self.secrets)
            return response
        finally:
            self.pending.pop(request_id, None)

    async def result(self, method, params):
        response = await self.call(method, params)
        if "error" in response:
            error = response["error"]
            message = BoundedText(RPC_MESSAGE_LIMIT)
            if isinstance(error.get("message"), str):
                message.feed(error["message"].encode("utf-8"))
            diagnostic = message.safe(self.secrets)
            diagnostic["message"] = diagnostic.pop("text")
            diagnostic.update(method=method, code=error.get("code") if type(error.get("code")) is int else None)
            raise ProbeFailure("rpc_failed_" + method + "_" + str(error.get("code")), diagnostic, message, self.secrets)
        return response["result"]

    async def tool(self, name, arguments=None):
        response = await self.result("tools.invoke", {"tool_name": name, "arguments": arguments or {}})
        require(response.get("ok") is True, "tool_failed_" + name)
        value = response.get("result", response)
        if isinstance(value, str):
            with contextlib.suppress(json.JSONDecodeError):
                value = json.loads(value)
        require(not isinstance(value, dict) or (value.get("success") is not False and value.get("ok") is not False), "tool_body_failed_" + name)
        return value

    async def wait_event(self, predicate, start=0, timeout=10):
        async def wait():
            async with self.changed:
                while True:
                    for frame in self.notifications[start:]:
                        if predicate(frame):
                            return frame
                    await self.changed.wait()
        return await asyncio.wait_for(wait(), timeout)

    async def close(self):
        await self.socket.close()
        self.reader.cancel()
        await asyncio.gather(self.reader, return_exceptions=True)


class Probe:
    def __init__(self, args):
        self.args = args
        self.root = None
        self.process = None
        self.provider = None
        self.server_output = None
        self.secrets = {"qa-dummy-not-a-real-key"}
        self.cleanup_errors = []
        self._diagnostic_texts = []
        self.clients = []
        self.checks = []
        self.receipt = {"status": "NOT_RUN", "checks": self.checks, "limitations": [
            "Loopback only: not a LAN/source-IP or browser rendering result.",
            "Typed Zahir delivery and same-sequence persistence are proved by Rust integration tests, not inferred from WebSocket frames.",
            "No remote audit-read endpoint exists or is invented by this probe.",
            "Panel manifest hashes are provenance inputs, not a browser/compressed-delivery attestation.",
            "Entire owning start_server force-drop is not proved by a process signal test.",
            "Diagnostics redact exact known generated credential fields and the dummy provider key, not arbitrary unknown secrets or a universal private-body detector.",
            "Server streams retain bounded prefixes in memory, scrubbed only before persistence; truncation and incomplete EOF/join are explicit.",
        ]}

    def check(self, name, condition):
        self.checks.append({"name": name, "pass": bool(condition)})
        require(condition, name)

    def prepare(self):
        require(self.args.artifacts.is_dir(), "artifacts_parent_must_exist")
        require(self.args.binary.is_file() and digest(self.args.binary) == self.args.sha256.lower(), "input_binary_sha256")
        panel = self.args.panel_output.resolve()
        for name in ("index.html", "build.json", "build-inputs.json"):
            require((panel / name).is_file(), "panel_input_" + name)
        panel_files = {}
        for path in sorted(panel.rglob("*")):
            if path.is_file():
                require(path.resolve().is_relative_to(panel), "panel_symlink_outside_input")
                panel_files[str(path.relative_to(panel))] = {"sha256": digest(path), "bytes": path.stat().st_size}
        # Never clean or delete the caller's artifact parent, which may also contain a build target.
        self.root = Path(tempfile.mkdtemp(prefix="capability-i3-", dir=self.args.artifacts.resolve()))
        self.home = self.root / "home"
        self.aleph = self.home / ".aleph"
        self.aleph.mkdir(parents=True)
        self.binary = self.root / ("aleph-server.exe" if os.name == "nt" else "aleph-server")
        shutil.copyfile(self.args.binary, self.binary)
        self.binary.chmod(0o755)
        self.check("private_binary_sha256", digest(self.binary) == self.args.sha256.lower())
        self.port, self.provider_port = free_port(), free_port()
        require(self.port != self.provider_port, "distinct_listener_ports")
        self.env = {name: os.environ[name] for name in ("PATH", "SYSTEMROOT", "WINDIR", "LANG", "LC_ALL", "TERM", "TMPDIR") if name in os.environ}
        self.env.update(HOME=str(self.home), ALEPH_HOME=str(self.aleph), USER="capability-i3-qa", RUST_LOG="error")
        self.config = self.aleph / "config.toml"
        self.config.write_text(f'''[general]
default_provider = "memory-qa"
[providers.memory-qa]
enabled = true
verified = true
protocol = "anthropic"
base_url = "http://127.0.0.1:{self.provider_port}"
api_key = "qa-dummy-not-a-real-key"
models = ["memory-qa-model"]
timeout_seconds = 30
[gateway]
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
[policies.terminal]
enabled = true
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
        self.receipt.update(binary_sha256=self.args.sha256.lower(), panel_files=panel_files)
        self.provider = subprocess.Popen([sys.executable, str(ROOT / "qa/memory_lifecycle/provider.py"), str(self.provider_port), os.devnull], cwd=self.root, env=self.env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
        self.process = subprocess.Popen([str(self.binary), "--config", str(self.config), "--bind", "127.0.0.1", "--port", str(self.port), "--webchat-dir", str(panel), "start"], cwd=self.root, env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        self.receipt["owned_pid"] = self.process.pid
        self.server_output = ServerOutput(self.process)

    async def boot(self):
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            require(self.process.poll() is None, "owned_server_early_exit")
            try:
                def health():
                    with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/health", timeout=1) as response:
                        return response.status == 200
                if await asyncio.to_thread(health):
                    break
            except OSError:
                pass
            await asyncio.sleep(0.1)
        else:
            raise ProbeFailure("owned_server_health_timeout")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            endpoints = list(self.home.rglob(".ipc-endpoint.json"))
            if endpoints:
                endpoint = json.loads(endpoints[0].read_text())
                parsed = urlparse(endpoint["url"])
                self.check("endpoint_is_own_pid", endpoint.get("pid") == self.process.pid)
                self.check("endpoint_is_own_loopback_listener", parsed.hostname in ("127.0.0.1", "localhost", "::1") and parsed.port == self.port)
                self.url = f"ws://127.0.0.1:{self.port}/ws"
                return
            await asyncio.sleep(0.05)
        raise ProbeFailure("private_endpoint_timeout")

    async def client(self, connect=None):
        import websockets
        client = Rpc(await websockets.connect(self.url, max_size=None, ping_interval=None), self.secrets)
        self.clients.append(client)
        if connect is not None:
            result = await client.result("connect", connect)
            self.check("explicit_connection_authorized", result.get("role") == "operator")
        return client

    async def identity(self, admin, label, user_id=None):
        if user_id is None:
            result = await admin.result("users.create", {"display_name": "capability-i3-" + label, "role": "admin"})
            user_id = result["user"]["user_id"]
        ticket = (await admin.result("gateway.ticket.create", {"user_id": user_id, "ttl_seconds": 900}))["ticket"]
        client = await self.client({"client_type": "panel", "bootstrap_ticket": ticket, "device_id": str(uuid.uuid4()), "device_name": "capability-i3-" + label})
        # Tickets and device tokens remain only in memory and are never copied into the receipt.
        await client.result("events.subscribe", {"topics": TOPICS})
        return client, user_id

    async def scenarios(self):
        admin = await self.client({"client_info": {"name": "capability-i3-bootstrap"}})
        guest = await self.client()
        self.check("guest_cannot_subscribe", "error" in await guest.call("events.subscribe", {"topics": TOPICS}))
        alice, alice_id = await self.identity(admin, "alice")
        alice_other, _ = await self.identity(admin, "alice-second-connection", alice_id)
        bob, _ = await self.identity(admin, "bob")
        chat = await alice.result("chat.send", {"message": "capability i3 private session", "channel": "gui:capability-i3-" + str(uuid.uuid4()), "agent_id": "main"})
        attachment = await alice.result("capabilities.ui.attach", {"session_key": chat["session_key"]})
        surface = attachment["surface_id"]
        is_source = lambda frame: source_record(frame, "aleph/ui", surface)
        is_activated = lambda frame: is_source(frame) and frame["data"]["event"]["type"] == "capability_activated"
        await alice.wait_event(is_activated)
        await alice_other.wait_event(is_activated)
        marker = "capability-i3-do-not-audit-" + str(uuid.uuid4())
        display = asyncio.create_task(alice.result("capabilities.ui.invoke", {**attachment, "operation": {"operation": "display", "text": marker}}))
        try:
            frame = await alice.wait_event(lambda item: item.get("topic") == "capabilities.ui.display" and item.get("data", {}).get("surface_id") == surface)
            message = frame["data"]
            await alice.result("capabilities.ui.ack", {**attachment, "request_id": message["request_id"], "rendered": True})
            await display
        finally:
            if not display.done():
                display.cancel()
            await asyncio.gather(display, return_exceptions=True)
        await alice.result("capabilities.ui.detach", attachment)
        await alice.wait_event(lambda frame: is_source(frame) and frame["data"]["event"]["type"] == "capability_revoked")
        await alice_other.wait_event(lambda frame: is_source(frame) and frame["data"]["event"]["type"] == "capability_revoked")
        # This bounded quiet window is a negative observation, not a proof of all possible scheduler interleavings.
        await asyncio.sleep(0.4)
        self.check("other_principal_has_no_private_source_frames", not any(is_source(frame) for frame in bob.notifications))
        self.check("guest_has_no_private_source_frames", not any(is_source(frame) for frame in guest.notifications))
        self.check("same_human_other_connection_audit_not_functional_display", not any(frame.get("topic") == "capabilities.ui.display" and frame.get("data", {}).get("surface_id") == surface for frame in alice_other.notifications))
        for client in (alice, alice_other):
            events = [frame["data"]["event"]["type"] for frame in client.notifications if is_source(frame)]
            self.check("actual_private_commit_and_withdraw_once", events.count("capability_activated") == 1 and events.count("capability_revoked") == 1)
        self.check("audit_excludes_display_body", all(marker not in json.dumps(audited_frames(client.notifications)) for client in self.clients))
        await alice.tool("memory_runtime_start")
        await alice.tool("note_manage", {"action": "create", "category": "reference", "filename": "capability-i3", "title": "Capability I3", "content": "Synthetic integration note"})
        await alice.tool("memory_runtime_stop")
        self.check("memory_real_source_withdrawn", bool(await alice.wait_event(lambda frame: source_record(frame, "aleph/resources", "memory.runtime") and frame["data"]["event"]["type"] == "capability_revoked")))
        pty = await alice.result("pty.spawn", {"command": "cmd.exe" if os.name == "nt" else "cat", "rows": 24, "cols": 80})
        await alice.result("pty.close", {"session_id": pty["session_id"]})
        self.check("pty_real_source_withdrawn", bool(await alice.wait_event(lambda frame: source_record(frame, "aleph/terminal", "session-" + pty["session_id"]) and frame["data"]["event"]["type"] == "capability_revoked")))
        selected = await self.identity(admin, "selection", alice_id)
        selection = selected[0]
        await selection.result("events.unsubscribe", {"topics": ["capability.event", "capability.event.status"]})
        before = len(selection.notifications)
        await alice.tool("memory_runtime_start")
        await alice.tool("memory_runtime_stop")
        await asyncio.sleep(0.4)
        self.check("websocket_subscription_selection_is_separate", not any(frame.get("topic") == "capability.event" for frame in selection.notifications[before:]))
        for client in self.clients:
            records = [frame["data"] for frame in client.notifications if frame.get("topic") == "capability.event"]
            sequence = [record["sequence"] for record in records]
            self.check("websocket_sequence_monotonic_without_duplicates", sequence == sorted(set(sequence)))
        self.receipt["received_record_counts"] = [len(audited_frames(client.notifications)) for client in self.clients]

    async def stop(self):
        if self.process is not None and self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            code = await asyncio.to_thread(self.process.wait, timeout=45)
            self.check("owned_process_signal_exit", code == 0)
        elif self.process is not None:
            self.check("owned_process_survived_scenarios", False)

    def cleanup_error(self, stage, error):
        message = BoundedText(RPC_MESSAGE_LIMIT)
        message.feed(str(error).encode("utf-8"))
        diagnostic = {"stage": stage, "error_type": type(error).__name__, **message.safe(self.secrets)}
        self.cleanup_errors.append(diagnostic)
        self._diagnostic_texts.append((diagnostic, "text", message))

    async def cleanup(self):
        for client in self.clients:
            try:
                await asyncio.wait_for(client.close(), timeout=5)
            except Exception as error:
                self.cleanup_error("client_close", error)
        settled = []
        for name, process in (("server", self.process), ("provider", self.provider)):
            if process is None:
                continue
            record = {"name": name, "pid": process.pid, "returncode": None, "wait_completed": False}
            try:
                if process.poll() is None:
                    process.terminate()
                    try:
                        await asyncio.to_thread(process.wait, timeout=5)
                    except subprocess.TimeoutExpired:
                        # Never signal a group unless it still belongs to this launched child.
                        if os.name == "posix" and os.getpgid(process.pid) == process.pid:
                            os.killpg(process.pid, signal.SIGKILL)
                        else:
                            process.kill()
                        await asyncio.to_thread(process.wait, timeout=5)
                record["returncode"] = process.wait(timeout=0)
                record["wait_completed"] = True
            except Exception as error:
                self.cleanup_error(name + "_cleanup", error)
            settled.append(record)
        self.receipt["owned_process_cleanup"] = settled

    async def run(self):
        try:
            require(os.name == "posix", "this_probe_signal_scenario_requires_posix")
            self.prepare()
            await self.boot()
            await self.scenarios()
            await self.stop()
            self.receipt["status"] = "PASS_FOR_LISTED_SCENARIOS_ONLY"
        except Exception as error:
            self.receipt["status"] = "FAIL"
            self.receipt["failure_type"] = type(error).__name__
            if isinstance(error, ProbeFailure):
                self.receipt["failure_code"] = str(error)
                if error.diagnostic is not None:
                    self.receipt["rpc_error"] = error.diagnostic
                    if error.diagnostic_text is not None:
                        self.secrets.update(error.diagnostic_secrets or ())
                        self._diagnostic_texts.append((error.diagnostic, "message", error.diagnostic_text))
        finally:
            try:
                await self.cleanup()
            except Exception as error:
                self.cleanup_error("cleanup", error)
            if self.server_output is not None:
                try:
                    output = self.server_output.finish(self.secrets)
                    self.receipt["server_output"] = output
                    if not output["joined"] or output["join_error"] or any(
                        not stream["eof"] or stream["read_error"] or stream["close_error"] for stream in output["streams"].values()
                    ):
                        self.cleanup_error("server_output", ProbeFailure("server_output_incomplete"))
                except Exception as error:
                    self.cleanup_error("server_output", error)
            if self.cleanup_errors:
                self.receipt["cleanup_errors"] = self.cleanup_errors
                if self.receipt["status"] != "FAIL":
                    self.receipt.update(status="FAIL", failure_type="ProbeFailure", failure_code="cleanup_failed")
            if self.root is not None:
                # Cleanup awaits can discover credentials after an earlier bounded diagnostic was rendered.
                for diagnostic, field, message in self._diagnostic_texts:
                    refreshed = message.safe(self.secrets)
                    text = refreshed.pop("text")
                    refreshed["redacted"] |= diagnostic["redacted"]
                    refreshed["truncated"] |= diagnostic["truncated"]
                    diagnostic.update(refreshed)
                    diagnostic[field] = text
                (self.root / "receipt.json").write_text(json.dumps(self.receipt, indent=2) + "\n", encoding="utf-8")
        print(json.dumps({"status": self.receipt["status"], "artifact_directory": str(self.root) if self.root else None}))
        return 0 if self.receipt["status"].startswith("PASS_") else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--sha256", required=True)
    parser.add_argument("--panel-output", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    require(len(args.sha256) == 64 and all(character in "0123456789abcdefABCDEF" for character in args.sha256), "invalid_sha256")
    return asyncio.run(Probe(args).run())


if __name__ == "__main__":
    sys.exit(main())
