#!/usr/bin/env python3
"""Install three formats, then verify their contributions on one Gateway."""
import asyncio
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "browser_managed"))
from qa_rpc import Ledger, ws_connect  # noqa: E402
from drive_command import Conn  # noqa: E402
from plant_visibility import plant as plant_claude  # noqa: E402

PROJECTS = ("project-a", "project-b", "project-c")
CLAUDE_TOOL = "plugin_qa-vis_vis__qa_echo"
CHILD_TASK = "QA_COMPOSITION_CHILD_TASK"
HOOK_MARK = "composition-hook-fired"


def plant(root, mock):
    source = root / "source"
    claude = plant_claude(source, "global", mock)
    for project in PROJECTS[:2]:
        plant_claude(root / project / ".aleph/plugins", project, mock)
    (root / PROJECTS[2]).mkdir()
    manifest = claude / ".claude-plugin/plugin.json"
    definition = json.loads(manifest.read_text())
    hook_script = claude / "hooks/record-call.sh"
    hook_script.parent.mkdir()
    hook_script.write_text(
        '#!/bin/sh\nif [ "${ALEPH_HOOKS_TEST:-}" != 1 ]; then\n'
        f'  touch "{root / HOOK_MARK}"\nfi\n'
    )
    definition["hooks"] = {"PostToolUse": [{"hooks": [{
        "type": "command", "command": 'sh "${CLAUDE_PLUGIN_ROOT}/hooks/record-call.sh"',
    }]}]}
    manifest.write_text(json.dumps(definition))
    (claude / "agents/qa-vis-global-agent.md").write_text(
        "---\nname: qa-vis-global-agent\ndescription: Composition delegate\n---\n"
        "QA_COMPOSITION_CHILD_PROMPT\n"
    )
    pi = source / "qa-comp-pi"
    (pi / "prompts").mkdir(parents=True)
    (pi / "skills/selected").mkdir(parents=True)
    (pi / "package.json").write_text(json.dumps({
        "name": "qa-comp-pi", "version": "1.0.0",
        "scripts": {"postinstall": f"touch {root / 'pi-ran-postinstall'}"},
        "pi": {"prompts": ["prompts"], "skills": ["skills/selected"],
               "extensions": ["ignored.ts"]},
    }))
    (pi / "ignored.ts").write_text(
        f"import {{writeFileSync}} from 'node:fs'; writeFileSync({json.dumps(str(root / 'pi-ran-js'))}, 'ran');\n"
        "export default function () {}\n"
    )
    (pi / "prompts/review.md").write_text("---\ndescription: QA review\n---\nQA_COMP_PI=$1|${2:-fallback}\n")
    (pi / "skills/selected/SKILL.md").write_text(
        "---\nname: qa-comp-pi-selected\ndescription: Composition Pi skill\n---\nQA_COMP_PI_SKILL_BODY\n"
    )
    (root / "composition-prime.json").write_text(json.dumps([
        {"name": CLAUDE_TOOL, "input": {"probe": "prime-claude"}}, None,
    ]))
    (root / "composition-tools.json").write_text(json.dumps([
        {"name": CLAUDE_TOOL, "input": {"probe": "mcp-claude-roundtrip"}},
        {"name": "subagent", "input": {"task": CHILD_TASK, "agent_type": "qa-vis-global-agent"}},
        None,
        None,
    ]))


async def turn(rpc, label, message, workspace=None):
    params = {"message": message, "session_key": f"qa-composition-{label}", "stream": True}
    if workspace is not None:
        params["project_root"] = str(workspace)
    reply = await rpc.call("chat.send", params)
    run_id = (reply.get("result") or {}).get("run_id")
    return await rpc.terminal(run_id) if run_id else (None, reply)


def requests(path, marker=None):
    bodies = [json.loads(line)["body"] for line in path.read_text().splitlines()] if path.exists() else []
    if marker is None:
        return bodies
    return [body for body in bodies if marker in json.dumps(body.get("messages", []))]


async def wait_installed(rpc):
    observed = {}
    for _ in range(100):
        reply = await rpc.call("plugins.list", {})
        observed = {p["selector"]: p for p in reply["result"]["plugins"]}
        vis = [p for p in observed.values() if p["name"] == "qa-vis"]
        if (len(vis) == 3 and all(p["status"] == "loaded" for p in vis)
                and all(observed.get(g, {}).get("status") == "loaded" for g in ("g:qa-comp-pi", "g:qa-dsh"))):
            break
        await asyncio.sleep(0.25)
    return observed


async def verify_scope(rpc, log, root):
    by_copy = {}
    for plugin in (await wait_installed(rpc)).values():
        if plugin["name"] == "qa-vis":
            by_copy[plugin.get("description", "").removeprefix("QA visibility ").removesuffix(" plugin")] = plugin
    log.check("two project instances and global restart as independent live packages",
              set(by_copy) == {*PROJECTS[:2], "global"}
              and len({p["selector"] for p in by_copy.values()}) == 3
              and all(p["status"] == "loaded" for p in by_copy.values()), str(by_copy))
    if set(by_copy) != {*PROJECTS[:2], "global"}:
        return
    for label, expected in (("project-a", "project-a"), ("project-b", "project-b"), ("project-c", "global")):
        await check_command(rpc, log, root, label, expected, root / label)
    a = by_copy["project-a"]["selector"]
    provider = scoped_echo_name(a)
    rejected = await rpc.call("tools.invoke", {"tool_name": provider, "arguments": {"probe": "foreign"}})
    log.check("sessionless raw call cannot use a live project provider key", "error" in rejected, str(rejected))
    result = await rpc.call("plugins.disable", {"selector": a})
    log.check("exact project A disable succeeded", "result" in result, str(result))
    await check_command(rpc, log, root, "a-disabled", None, root / "project-a")
    await check_command(rpc, log, root, "b-still-live", "project-b", root / "project-b")
    await check_command(rpc, log, root, "global-still-live", "global", None)


def scoped_echo_name(selector):
    import hashlib
    runtime_key = f"p_{selector[2:]}_qa-vis"
    prefix = hashlib.sha256(f"mcp-server\0plugin:{runtime_key}/vis".encode()).hexdigest()[:30]
    tool = hashlib.sha256(b"mcp-tool\0qa_echo").hexdigest()[:32]
    return f"p_{prefix}{tool}"


async def check_command(rpc, log, root, label, expected, workspace):
    marker = f"QA_COMP_SCOPE_{label}"
    outcome, detail = await turn(rpc, label, f"/qa-vis:echo {marker}", workspace)
    body = json.dumps(requests(root / "composition-model.jsonl", marker))
    observed = {copy for copy in (*PROJECTS[:2], "global") if f"QA_COMMAND_{copy}: {marker}" in body}
    log.check(f"{label}: correct package handles the bare command", outcome == "stream.run_complete"
              and observed == ({expected} if expected else set()), str((observed, detail)))


async def drive(url, root, phase):
    log = Ledger()
    async with ws_connect(url) as ws:
        rpc = Conn(ws)
        hello = await rpc.call("connect", {"client_type": "cli"})
        if "error" in hello:
            raise RuntimeError(hello["error"])
        if phase == "install":
            for project in PROJECTS:
                reply = await rpc.call("projects.add", {"path": str(root / project), "name": project})
                log.check(f"register {project}", "result" in reply, str(reply))
            for plugin in ("qa-vis", "qa-comp-pi"):
                reply = await rpc.call("plugins.install", {"url": str(root / "source" / plugin)})
                row = (reply.get("result") or {}).get("plugin") or {}
                log.check(f"local install returns exact selector for {plugin}",
                          row.get("selector") == f"g:{plugin}", str(reply))
        elif phase == "prime":
            outcome, details = await turn(rpc, "prime", "QA_COMP_PRIME")
            log.check("Claude tool turn primes the hook consent request", outcome == "stream.run_complete", str(details))
            log.check("hook requires approval before running", not (root / HOOK_MARK).exists())
        elif phase == "effects":
            outcome, detail = await turn(rpc, "effects", "QA_COMP_EFFECTS")
            log.check("Claude MCP and delegate turn completed", outcome == "stream.run_complete", str(detail))
            bodies = requests(root / "composition-effects.jsonl")
            log.check("Claude MCP result returned to model", "mcp-claude-roundtrip" in json.dumps([
                b for b in bodies if "tool_result" in json.dumps(b.get("messages", []))
            ]))
            child = requests(root / "composition-effects.jsonl", CHILD_TASK)
            log.check("Claude Agent executed with its authored prompt",
                      any("QA_COMPOSITION_CHILD_PROMPT" in json.dumps(b.get("system")) for b in child))
            for _ in range(40):
                if (root / HOOK_MARK).is_file():
                    break
                await asyncio.sleep(0.1)
            log.check("approved hook fired on the live tool call", (root / HOOK_MARK).is_file())
        elif phase == "scope":
            command = "qa-comp-pi:review"
            listing = await rpc.call("commands.list", {})
            log.check("Pi prompt is listed after restart", command in json.dumps(listing), str(listing)[:500])
            outcome, details = await turn(rpc, "pi", f'/{command} "two words"')
            log.check("Pi prompt arguments reached the model",
                      outcome == "stream.run_complete" and "QA_COMP_PI=two words|fallback" in json.dumps(
                          requests(root / "composition-model.jsonl")), str(details))
            claude_skill = await rpc.call("tools.invoke", {
                "tool_name": "skill_read", "arguments": {"skill_id": "qa-vis-global-skill"},
            })
            log.check("Claude skill is callable through the normal skill reader",
                      "QA skill." in json.dumps(claude_skill), str(claude_skill))
            response = await rpc.call("tools.invoke", {"tool_name": "skill_read", "arguments": {"skill_id": "qa-comp-pi-selected"}})
            log.check("Pi skill is callable through the normal skill reader", "QA_COMP_PI_SKILL_BODY" in json.dumps(response), str(response))
            log.check("Pi extension code was not executed", not (root / "pi-ran-js").exists())
            log.check("npm postinstall did not run", not (root / "pi-ran-postinstall").exists())
            await verify_scope(rpc, log, root)
        else:
            raise ValueError(phase)
    return log.verdict()


if __name__ == "__main__":
    if sys.argv[1] == "plant":
        plant(Path(sys.argv[2]), Path(sys.argv[3]))
    else:
        sys.exit(asyncio.run(drive(sys.argv[2], Path(sys.argv[3]), sys.argv[1])))
