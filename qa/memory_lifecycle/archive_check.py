"""Live FileSessionStore archive behaviour check for one Windows-native server.

Focus: the archive directory tree produced by `sessions.delete` for one reused
session key. The unit test
`gateway::session_store::file_backend::archive_tests::delete_session_reuses_a_key_without_colliding_with_its_archive`
covers the rename loop in isolation; this script checks the same contract
against a running server, on the platform where the session key contains
characters that the directory name sanitizes away.

The script starts the supplied server binary, drives it over the same JSON-RPC
surface the lifecycle probe uses, and reports the archive directory names it
observes. It never builds a server and never touches an installed daemon.
"""

import argparse
import asyncio
import hashlib
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "qa/browser_managed"))
from qa_rpc import Rpc, ran, ws_connect  # noqa: E402

IS_WINDOWS = os.name == "nt"
if IS_WINDOWS:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from windows_console import deliver_control_event, prepare_console_parent, spawn_creation_flags


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_healthy(port, timeout=90):
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2) as response:
                if response.status == 200:
                    return True
        except (urllib.error.URLError, OSError):
            time.sleep(0.3)
    return False


def archive_dirs(aleph_home):
    base = Path(aleph_home) / "data" / "sessions" / ".archive"
    if not base.exists():
        return {}
    return {
        date.name: sorted(entry.name for entry in date.iterdir())
        for date in sorted(base.iterdir())
        if date.is_dir()
    }


def archive_snapshots(aleph_home):
    base = Path(aleph_home) / "data" / "sessions" / ".archive"
    if not base.exists():
        return {}
    return {
        str(archive.relative_to(base)): {
            str(path.relative_to(archive)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in archive.rglob("*")
            if path.is_file()
        }
        for date in base.iterdir()
        if date.is_dir()
        for archive in date.iterdir()
        if archive.is_dir()
    }


async def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, default=Path(tempfile.gettempdir()))
    parser.add_argument("--channel", default="gui:memory-archive-check")
    parser.add_argument("--cycles", type=int, default=3)
    args = parser.parse_args()
    if args.cycles < 1:
        parser.error("--cycles must be positive")

    root = Path(tempfile.mkdtemp(prefix="aleph-memory-archive-", dir=args.artifacts))
    home = root / "home"
    aleph = home / ".aleph"
    aleph.mkdir(parents=True)
    port = free_port()
    binary = root / ("aleph-server.exe" if IS_WINDOWS else "aleph-server")
    shutil.copyfile(args.binary, binary)
    binary.chmod(0o755)

    (aleph / "config.toml").write_text(
        f"""[general]
session_store_backend = "file"

[gateway]
host = "127.0.0.1"
port = {port}
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
enabled = false

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
""",
        encoding="utf-8",
    )

    env = dict(os.environ)
    env["HOME"] = str(home)
    env["ALEPH_HOME"] = str(aleph)
    env["RUST_LOG"] = "info"
    env["NO_PROXY"] = "127.0.0.1,localhost"

    spawn_options = {}
    if IS_WINDOWS:
        prepare_console_parent()
        spawn_options["creationflags"] = spawn_creation_flags()
    else:
        spawn_options["start_new_session"] = True
    log = (root / "server.log").open("w")
    process = subprocess.Popen(
        [str(binary), "--port", str(port), "start"],
        cwd=root,
        env=env,
        stdout=log,
        stderr=subprocess.STDOUT,
        **spawn_options,
    )

    results = []
    key = None
    snapshots = {}
    endpoints = []
    completed = False
    try:
        if not wait_healthy(port):
            raise RuntimeError("server never became healthy")
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            endpoints = list(home.rglob(".ipc-endpoint.json"))
            if endpoints:
                break
            await asyncio.sleep(0.05)
        if not endpoints:
            raise RuntimeError("server never published its endpoint")
        async with ws_connect(f"ws://127.0.0.1:{port}/ws") as socket_:
            rpc = Rpc(socket_)
            await rpc.connect("memory-archive-check")
            for cycle in range(args.cycles):
                sent = await rpc.call(
                    "chat.send",
                    {
                        "message": f"archive check cycle {cycle}",
                        "channel": args.channel,
                        "agent_id": "main",
                    },
                )
                if "result" not in sent:
                    results.append({"cycle": cycle, "stage": "chat", "response": sent})
                    raise RuntimeError(f"chat failed on cycle {cycle}: {sent}")
                current_key = sent["result"]["session_key"]
                if key is not None and current_key != key:
                    raise RuntimeError(f"session key changed: {key} -> {current_key}")
                key = current_key
                # Wait for the persisted reply before requesting archival.
                deadline = time.monotonic() + 45
                while time.monotonic() < deadline:
                    history = await rpc.call("sessions.history", {"session_key": key, "limit": 50})
                    messages = history.get("result", {}).get("messages", [])
                    if any(row.get("role") == "assistant" and str(row.get("content", "")).strip() for row in messages):
                        break
                    await asyncio.sleep(0.1)
                else:
                    results.append({"cycle": cycle, "stage": "history", "response": history})
                    raise RuntimeError(f"no persisted reply on cycle {cycle}")
                deleted = await rpc.call("sessions.delete", {"session_key": key})
                observed = archive_dirs(aleph)
                row = {
                    "cycle": cycle,
                    "session_key": key,
                    "delete_result": deleted.get("result"),
                    "delete_error": deleted.get("error"),
                    "attempts": [{"attempt": 0, "response": deleted}],
                    "archive": observed,
                }
                results.append(row)
                # Never retry an archive failure here: that would mask the
                # production failure this check is meant to expose.
                if not deleted.get("result", {}).get("deleted"):
                    raise RuntimeError(f"delete failed on attempt 0, cycle {cycle}: {deleted}")
                current = archive_snapshots(aleph)
                row["previous_archives_unchanged"] = all(
                    current.get(path) == files for path, files in snapshots.items()
                )
                row["one_archive_added"] = len(current) == cycle + 1 and len(current.keys() - snapshots.keys()) == 1
                # Numbering restarts on a new UTC archive date. Each date must
                # contain exactly the unsuffixed base followed by 1..N.
                base = "agent_main_main_s1" if IS_WINDOWS else "agent:main:main:s1"
                row["names_contiguous"] = all(
                    names == sorted([base] + [f"{base}-{i}" for i in range(1, len(names))])
                    for names in observed.values()
                )
                if not all(row[name] for name in ("previous_archives_unchanged", "one_archive_added", "names_contiguous")):
                    raise RuntimeError(f"archive contract failed on cycle {cycle}: {row}")
                snapshots = current
                await asyncio.sleep(0.5)
        completed = True
    finally:
        forced_cleanup = False
        delivery = None
        if process.poll() is None:
            if IS_WINDOWS:
                delivery = deliver_control_event(process.pid, "ctrl_c")
            else:
                process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                forced_cleanup = True
                process.kill()
                process.wait()
        log.close()
        text = (root / "server.log").read_text(encoding="utf-8", errors="replace")
        for path in (aleph / "logs").glob("*"):
            if path.is_file():
                text += path.read_text(encoding="utf-8", errors="replace")
        with socket.socket() as client:
            client.settimeout(1)
            listener_closed = client.connect_ex(("127.0.0.1", port)) != 0
        shutdown = {
            "exit_code": process.returncode,
            "forced_cleanup": forced_cleanup,
            "signal_delivery": delivery,
            "signal_observed": "[SHUTDOWN] signal=ctrl_c" in text if IS_WINDOWS else "[SHUTDOWN] signal=SIGTERM" in text,
            "endpoint_removed": bool(endpoints) and all(not path.exists() for path in endpoints),
            "listener_closed": listener_closed,
        }
        shutdown_ok = (
            shutdown["exit_code"] == 0
            and not forced_cleanup
            and all(shutdown[name] for name in ("signal_observed", "endpoint_removed", "listener_closed"))
        )
        summary = {
            "complete": completed and shutdown_ok,
            "session_key": key,
            "cycles": results,
            "final_archive": archive_dirs(aleph),
            "archive_sha256": snapshots,
            "shutdown": shutdown,
        }
        (root / "archive-check.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
        print(json.dumps(summary, indent=2))
        print("artifacts:", root)
        if not shutdown_ok:
            raise RuntimeError(f"server cleanup failed: {shutdown}")


if __name__ == "__main__":
    asyncio.run(main())
