"""Own an isolated production Gateway and deterministic model endpoint for QA."""
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
from urllib.request import Request, urlopen

from nsh_panel_launcher import file_sha256


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def capture_panel_assets(url, output=None):
    """Bind debug HTTP bytes, which are not necessarily embedded in the binary."""
    if output is not None:
        output.mkdir(parents=True, exist_ok=True)
    assets = {}
    for name in ["index.html", "aleph_panel.js", "aleph_panel_bg.wasm", "tailwind.css"]:
        digest = hashlib.sha256()
        size = 0
        destination = (output / name).open("wb") if output is not None else None
        try:
            with urlopen(Request(url + name, headers={"Accept-Encoding": "identity"}), timeout=30) as response:
                if response.headers.get("Content-Encoding", "identity") != "identity":
                    raise ValueError("panel_asset_identity_encoding_required")
                while chunk := response.read(1048576):
                    digest.update(chunk)
                    size += len(chunk)
                    if destination is not None:
                        destination.write(chunk)
        finally:
            if destination is not None:
                destination.close()
        assets[name] = {"sha256": digest.hexdigest(), "bytes": size}
        if output is not None:
            assets[name]["capture"] = str(output / name)
    return assets


def panel_assets_unchanged(url, before):
    after = capture_panel_assets(url)
    return all(after[name] == {key: value for key, value in item.items() if key != "capture"}
               for name, item in before.items())


@contextmanager
def isolated_gateway(binary, output, *, mid_turn_steering=True):
    output.mkdir(parents=True, exist_ok=True)
    children = []
    streams = []
    record = {"status": "STARTING", "children": []}
    fixture = tempfile.TemporaryDirectory(prefix="p9-gateway-", dir="/tmp")
    with fixture as directory:
        root = Path(directory)
        home, aleph = root / "home", root / "aleph"
        home.mkdir()
        aleph.mkdir()
        port, provider_port = free_port(), free_port()
        while provider_port == port:
            provider_port = free_port()
        (aleph / "config.toml").write_text(f'''[execution]
mid_turn_steering = {str(mid_turn_steering).lower()}
[general]
default_provider = "phase9-qa"
[providers.phase9-qa]
enabled = true
verified = true
protocol = "anthropic"
base_url = "http://127.0.0.1:{provider_port}"
api_key = "qa-not-a-real-key"
models = ["phase9-qa"]
timeout_seconds = 30
[gateway]
host = "127.0.0.1"
port = {port}
[agents]
[[agents.list]]
id = "main"
default = true
name = "Phase9 QA"
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
''')
        try:
            commands = [
                [sys.executable, str(Path(__file__).with_name("nsh_panel_provider.py")), "--port", str(provider_port), "--record", str(output / "model-requests.jsonl")],
                [str(binary), "--port", str(port), "start"],
            ]
            for name, argv in zip(["model", "gateway"], commands):
                stream = (output / f"{name}.log").open("wb")
                streams.append(stream)
                child = subprocess.Popen(argv, cwd=root, env=os.environ | {"HOME": str(home), "ALEPH_HOME": str(aleph), "TMPDIR": str(root)}, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
                children.append(child)
                record["children"].append({"name": name, "argv": argv, "pid": child.pid, "cwd": str(root)})
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline and children[-1].poll() is None:
                try:
                    with urlopen(f"http://127.0.0.1:{port}/health", timeout=1) as response:
                        if response.status == 200:
                            break
                except OSError:
                    pass
                time.sleep(0.1)
            else:
                raise RuntimeError("owned_gateway_did_not_become_ready")
            gateway = {"kind": "production_gateway", "isolated": True, "url": f"http://127.0.0.1:{port}/", "binary": str(binary), "binary_sha256": file_sha256(binary), "pid": children[-1].pid, "model_endpoint": "deterministic_protocol_fixture"}
            gateway["panel_assets"] = capture_panel_assets(gateway["url"], output / "served-assets")
            receipt = output / "gateway.json"
            receipt.write_text(json.dumps(gateway, indent=2) + "\n")
            record["status"] = "READY"
            yield receipt
        finally:
            primary_error = sys.exc_info()[0] is not None
            if record.get("status") == "READY":
                try:
                    record["panel_assets_unchanged"] = panel_assets_unchanged(gateway["url"], gateway["panel_assets"])
                except (OSError, ValueError) as error:
                    record["panel_assets_unchanged"] = False
                    record["panel_assets_error"] = {"type": type(error).__name__, "reason": str(error)}
            for child, metadata in reversed(list(zip(children, record["children"]))):
                forced = False
                if child.poll() is None:
                    try:
                        os.killpg(child.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    try:
                        child.wait(timeout=30)
                    except subprocess.TimeoutExpired:
                        forced = True
                        try:
                            os.killpg(child.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        child.wait()
                metadata.update(exit_code=child.returncode, forced=forced)
            for stream in streams:
                stream.close()
            fixture.cleanup()
            record["fixture_removed"] = not root.exists()
            (output / "gateway-cleanup.json").write_text(json.dumps(record, indent=2) + "\n")
            if not primary_error and record.get("status") == "READY" and not record.get("panel_assets_unchanged"):
                raise RuntimeError("served_panel_assets_changed_during_runtime")
