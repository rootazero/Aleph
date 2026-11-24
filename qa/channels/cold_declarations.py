#!/usr/bin/env python3
"""Probe a freshly built aleph-server through its real RPC and Signal transport.

Usage: python3 qa/channels/cold_declarations.py /path/to/aleph-server
Requires Python 3.11+ and websockets. Uses only loopback and a disposable home.
"""

import asyncio
from collections import Counter
import json
import os
from pathlib import Path
import socket
import sys
import tempfile
import tomllib

import websockets

ADMIN = "+15550000090"
SENDER = "+15550000091"
GROUP = "qa-signal-group"
NUMBERS = {
    "cold-boot": "+15550000001",
    "true-boot": "+15550000002",
    "default-boot": "+15550000003",
    "cold-create": "+15550000004",
    "true-create": "+15550000005",
    "default-create": "+15550000006",
    "cold-delete": "+15550000007",
}


async def wait_for(predicate):
    async with asyncio.timeout(20):
        while not predicate():
            await asyncio.sleep(0.05)


async def probe(binary, root):
    upstream = Counter()
    outbound = []
    emit_first = {NUMBERS[name] for name in ("cold-boot", "cold-create")}

    async def signal(reader, writer):
        async with asyncio.timeout(60):
            try:
                request = (await reader.readline()).decode().split()
                if not request:
                    return
                method, path, _ = request
                headers = {}
                while (line := await reader.readline()) not in (b"\r\n", b""):
                    name, value = line.decode().split(":", 1)
                    headers[name.lower()] = value.strip()
                upstream[path] += 1
                print(f"upstream {method} {path}", flush=True)
                if method == "GET" and path.startswith("/v1/receive/"):
                    writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")
                    if path.rsplit("/", 1)[-1] in emit_first:
                        event = {"envelope": {"source": SENDER, "dataMessage": {
                            "message": "/new @aleph", "timestamp": 1795464000000,
                            "groupInfo": {"groupId": GROUP},
                        }}}
                        writer.write(f"data: {json.dumps(event)}\n\n".encode())
                    await writer.drain()
                    await reader.read()
                elif method == "POST" and path == "/v2/send":
                    body = await reader.readexactly(int(headers["content-length"]))
                    outbound.append(json.loads(body))
                    payload = b'{"timestamp":1795464000001}'
                    writer.write(b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\n"
                                 + f"Content-Length: {len(payload)}\r\nConnection: close\r\n\r\n".encode()
                                 + payload)
                    await writer.drain()
                else:
                    writer.write(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                    await writer.drain()
            finally:
                writer.close()
                await writer.wait_closed()

    listener = await asyncio.start_server(signal, "127.0.0.1", 0)
    upstream_port = listener.sockets[0].getsockname()[1]
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    home = root / "home"
    aleph_home = home / ".aleph"
    aleph_home.mkdir(parents=True)
    config = aleph_home / "config.toml"
    api_url = f"http://127.0.0.1:{upstream_port}"
    config_text = f'''[gateway]
host = "127.0.0.1"
port = {port}

[providers.qa-local]
enabled = true
protocol = "anthropic"
base_url = "{api_url}"
api_key = "qa-not-a-real-key"
models = ["qa-model"]

[[agents.list]]
id = "main"
name = "QA"
default = true
model = "qa-model"
provider = "qa-local"
'''
    for section in ("cron", "heartbeat", "mcp", "acp", "evolution", "memory", "skills"):
        config_text += f"\n[{section}]\nenabled = false\n"
    for name, enabled in (("cold-boot", "false"), ("true-boot", "true"), ("default-boot", None)):
        config_text += f'\n[channels.{name}]\ntype = "signal"\napi_url = "{api_url}"\nphone_number = "{NUMBERS[name]}"\n'
        if enabled is not None:
            config_text += f"enabled = {enabled}\n"
    config_text += f'''
[channels.imessage]
enabled = false
transport = "bluebubbles"
[channels.imessage.bluebubbles]
server_url = "{api_url}"
password = "qa-not-a-real-password"
webhook_port = 0

[channels.cold-telegram]
type = "telegram"
enabled = false
bot_token = "12345:dummy-qa-value"

[channels.cold-feishu]
type = "feishu"
enabled = false
app_id = "cli_dummy_qa"
app_secret = "dummy-qa-value"
'''
    config.write_text(config_text)
    env = dict(os.environ, HOME=str(home), ALEPH_HOME=str(aleph_home),
               XDG_CONFIG_HOME=str(home / ".config"), XDG_DATA_HOME=str(home / ".local/share"),
               XDG_CACHE_HOME=str(home / ".cache"), NO_PROXY="127.0.0.1,localhost",
               no_proxy="127.0.0.1,localhost", RUST_MIN_STACK="268435456",
               RUST_LOG="info,alephcore::gateway::interfaces::signal=debug,"
                        "alephcore::gateway::handlers::channel=debug")
    with (root / "server.log").open("wb") as log:
        process = await asyncio.create_subprocess_exec(str(binary), "--port", str(port), "start",
                                                       cwd=root, env=env, stdout=log, stderr=log)
        try:
            async with asyncio.timeout(90):
                while True:
                    if process.returncode is not None:
                        raise RuntimeError(f"aleph-server exited {process.returncode}")
                    try:
                        ws = await websockets.connect(f"ws://127.0.0.1:{port}/ws", open_timeout=2)
                        break
                    except (OSError, TimeoutError):
                        await asyncio.sleep(0.2)
            async with ws:
                next_id = 0

                async def rpc(method, params=None, *, error=False):
                    nonlocal next_id
                    next_id += 1
                    await ws.send(json.dumps({"jsonrpc": "2.0", "id": next_id,
                                              "method": method, "params": params or {}}))
                    async with asyncio.timeout(15):
                        while (frame := json.loads(await ws.recv())).get("id") != next_id:
                            pass
                    assert ("error" in frame and frame["error"] is not None) == error, (method, frame)
                    return frame["error" if error else "result"]

                def params(name):
                    return {"channel_id": name}

                def receives(name):
                    return upstream[f"/v1/receive/{NUMBERS[name]}"]

                def observed(name, event):
                    logs = [root / "server.log", *(aleph_home / "logs").glob("aleph-server.log*")]
                    # Non-TTY logs are file-only, but support either logging sink.
                    return max(sum(event in line and f"channel_id={name}" in line
                                   for line in source.read_text(errors="replace").splitlines()) for source in logs)

                def constructs(name):
                    return observed(name, "Signal HTTP client constructed")

                def hydrations(name):
                    return observed(name, "Resolving channel credentials from vault")

                async def create(name, enabled=None):
                    cfg = {"api_url": api_url, "phone_number": NUMBERS[name]}
                    if enabled is not None:
                        cfg["enabled"] = enabled
                    if name == "cold-create":
                        cfg["group_allow_admin_from"] = [ADMIN]
                    return await rpc("channel.create", {"id": name, "type": "signal", "config": cfg})

                await rpc("connect", {"client": "qa-cold-declarations", "version": "1"})
                for name in ("cold-boot", "imessage", "cold-telegram", "cold-feishu"):
                    assert (await rpc("channels.status", params(name)))["status"] == "disabled"
                await wait_for(lambda: all(receives(n) and constructs(n) == 1 for n in ("true-boot", "default-boot")))
                assert receives("cold-boot") == constructs("cold-boot") == 0
                assert hydrations("true-boot") > 0 and hydrations("default-boot") > 0
                assert all(hydrations(name) == 0 for name in ("cold-boot", "cold-telegram", "cold-feishu"))
                assert not any(path.startswith("/api/v1/") for path in upstream), upstream
                await rpc("channels.status", params("missing"), error=True)
                await rpc("channel.send", params("cold-boot") | {"to": SENDER, "text": "must not send"}, error=True)
                print("PASS cold boot, disabled BlueBubbles, enabled/default boot controls", flush=True)

                assert (await create("cold-create", False))["status"] == "disabled"
                await rpc("channel.create", {"id": "cold-create", "type": "signal", "config": {"enabled": False}}, error=True)
                assert (await create("cold-delete", False))["status"] == "disabled"
                for name, enabled in (("true-create", True), ("default-create", None)):
                    assert (await create(name, enabled))["status"] == "started"
                await wait_for(lambda: all(receives(n) and constructs(n) == 1 for n in ("true-create", "default-create")))
                listed = await rpc("channels.list")
                assert listed["summary"]["disabled"] == 6, listed
                assert listed["summary"]["total"] == len(listed["channels"]) == 10, listed
                assert receives("cold-create") == constructs("cold-create") == hydrations("cold-create") == 0
                assert not outbound
                await rpc("channel.delete", {"id": "cold-delete"})
                await rpc("channels.status", params("cold-delete"), error=True)
                assert receives("cold-delete") == constructs("cold-delete") == 0
                print("PASS cold create/list/delete; enabled/default create controls", flush=True)

                await rpc("config.patch", {"path": "channels.cold-boot", "patch": {"group_allow_admin_from": "invalid"}})
                await rpc("channel.start", params("cold-boot"), error=True)
                assert (await rpc("channels.status", params("cold-boot")))["status"] == "disabled"
                assert receives("cold-boot") == constructs("cold-boot") == 0
                await rpc("config.patch", {"path": "channels.cold-boot", "patch": {"group_allow_admin_from": [ADMIN]}})
                for name in ("cold-boot", "cold-create"):
                    assert (await rpc("channel.start", params(name)))["status"] == "started"
                    await wait_for(lambda: any(body["number"] == NUMBERS[name] for body in outbound))
                    reply = next(body for body in outbound if body["number"] == NUMBERS[name])
                    assert reply["recipients"] == [GROUP] and "not enabled for your account" in reply["message"], reply
                    assert receives(name) == constructs(name) == 1
                    assert (await rpc("channels.status", params(name)))["status"] == "connected"
                    assert tomllib.loads(config.read_text())["channels"][name]["enabled"] is False
                print("PASS malformed policy rejection; first inbound uses policy on both activation paths", flush=True)
        finally:
            if process.returncode is None:
                process.terminate()
                try:
                    await asyncio.wait_for(process.wait(), 20)
                except TimeoutError:
                    process.kill()
                    await process.wait()
            listener.close()
            await listener.wait_closed()


if __name__ == "__main__":
    base = Path(tempfile.mkdtemp(prefix="aleph-cold-declarations-"))
    print(f"QA artifacts: {base}", flush=True)
    asyncio.run(probe(Path(sys.argv[1]).resolve(strict=True), base))
