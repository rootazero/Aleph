#!/usr/bin/env python3
"""Check DSH discovery, stdio activation and the remote MCP network policy."""
import asyncio
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "browser_managed"))
from qa_rpc import Ledger, ws_connect  # noqa: E402
from drive_command import Conn  # noqa: E402

SELECTOR = "g:qa-dsh"
HTTP_SELECTOR = "g:qa-dsh-http"
STDIO_TOOL = "plugin_qa-dsh_stdio__qa_echo"
HTTP_TOOL = "plugin_qa-dsh-http_http__qa_echo"


def named_tools(value):
    if isinstance(value, dict):
        names = {value["name"]} if isinstance(value.get("name"), str) else set()
        for item in value.values():
            names.update(named_tools(item))
        return names
    if isinstance(value, list):
        return set().union(*(named_tools(item) for item in value))
    return set()


async def row(rpc, selector=SELECTOR):
    response = await rpc.call("plugins.list", {})
    return next((plugin for plugin in response["result"]["plugins"] if plugin.get("selector") == selector), {})


async def wait_tools(rpc, status, expected):
    for _ in range(80):
        response = await rpc.call("tools.catalog", {})
        found = named_tools(response["result"]) & {STDIO_TOOL, HTTP_TOOL}
        if found == expected and (await row(rpc)).get("status") == status:
            return True
        await asyncio.sleep(0.25)
    return False


async def switch(rpc, action, selector=SELECTOR):
    reply = await rpc.call(f"plugins.{action}", {"selector": selector})
    return "error" not in reply


async def main(ws_url, root, phase):
    log = Ledger()
    stdio_log = root / "dsh-stdio.jsonl"
    http_log = root / "dsh-http.jsonl"
    async with ws_connect(ws_url) as ws:
        rpc = Conn(ws)
        hello = await rpc.call("connect", {"client_type": "cli"})
        if "error" in hello:
            raise RuntimeError(f"Gateway connection refused: {hello['error']}")
        if phase == "install":
            reply = await rpc.call("plugins.install", {"url": str(root / "source/qa-dsh")})
            installed = (reply.get("result") or {}).get("plugin") or {}
            log.check("local install returns the exact DSH instance",
                      installed.get("selector") == SELECTOR, str(reply))
            reply = await rpc.call("plugins.install", {"url": str(root / "source/qa-dsh-http")})
            http = (reply.get("result") or {}).get("plugin") or {}
            log.check("local install normalizes the literal Streamable HTTP server",
                      http.get("selector") == HTTP_SELECTOR and http.get("mcp_servers_count") == 1,
                      str(reply))
            log.check("disabled staged install did not open either transport",
                      not stdio_log.exists() and not http_log.exists())
            return log.verdict()
        record = await row(rpc)
        locations = record.get("import_diagnostics", [])
        diagnostics = json.dumps(locations)
        log.check("canonical plugin row locates both unsupported nodes in the package patch",
                  any(item.get("path") == "cordis.patch.yml" for item in locations)
                  and "failOnStartupError" in diagnostics and "!!js" in diagnostics
                  and "patch[1].insert[2]" in diagnostics and "patch[2].insert[1]" in diagnostics,
                  diagnostics)
        if phase == "disabled":
            log.check("disabled bundle publishes neither server", await wait_tools(rpc, "disabled", set()))
            log.check("discovery initiated no subprocess or HTTP traffic",
                      not stdio_log.exists() and not http_log.exists())
            log.check("exact HTTP enable accepted", await switch(rpc, "enable", HTTP_SELECTOR))
            for _ in range(80):
                http = await row(rpc, HTTP_SELECTOR)
                if http.get("status") == "error":
                    break
                await asyncio.sleep(0.25)
            log.check("HTTP loopback is refused at readiness with no transport traffic",
                      http.get("status") == "error" and "SSRF blocked" in json.dumps(http)
                      and not http_log.exists(), str(http))
            log.check("exact HTTP disable accepted", await switch(rpc, "disable", HTTP_SELECTOR))
            log.check("exact stdio enable accepted", await switch(rpc, "enable"))
        log.check("literal stdio server reached live readiness",
                  await wait_tools(rpc, "loaded", {STDIO_TOOL}))
        run = await rpc.call("chat.send", {
            "message": "Exercise the DSH tools", "session_key": f"qa-dsh-{phase}", "stream": True,
        })
        run_id = (run.get("result") or {}).get("run_id")
        outcome, details = await rpc.terminal(run_id) if run_id else (None, run)
        log.check("Gateway turn completes", outcome == "stream.run_complete", str(details))
        request_log = root / "dsh-model.jsonl"
        results = []
        if request_log.exists():
            for line in request_log.read_text().splitlines():
                body = json.loads(line)["body"]
                for message in body.get("messages", []):
                    content = message.get("content", [])
                    if isinstance(content, list):
                        results.extend(block for block in content if block.get("type") == "tool_result")
        evidence = json.dumps(results)
        log.check("real stdio tool result reaches the model",
                  "dsh-stdio" in evidence and "mcp-stdio-roundtrip" in evidence)
        if phase == "disabled":
            log.check("exact disable accepted", await switch(rpc, "disable"))
            log.check("stdio tool row withdraws", await wait_tools(rpc, "disabled", set()))
            log.check("install did not invoke the package script or tagged node",
                      not (root / "dsh-postinstall-marker").exists() and not (root / "dsh-tag-marker").exists())
            log.check("exact re-enable accepted", await switch(rpc, "enable"))
            log.check("stdio tool publishes again from the re-enabled instance",
                      await wait_tools(rpc, "loaded", {STDIO_TOOL}))
    return log.verdict()


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main(sys.argv[1], Path(sys.argv[2]), sys.argv[3])))
