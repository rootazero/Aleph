#!/usr/bin/env python3
"""Real-machine QA for the leftovers round.

Three claim groups, run against a live `aleph-server` with a relocated
HOME/ALEPH_HOME and `[agents.defaults]` pointed at non-default roots:

  A. the converged tool DESCRIPTIONs are what the wire actually carries;
  B. the hooks writer and the hooks reader agree under a relocated ALEPH_HOME;
  C. provisioning writes where the resolver will look, i.e. under the
     configured `[agents.defaults]` roots and NOT under the default layout.

B and C are the discriminating ones: both were fixed in the previous round
with in-process assertions only. C in particular is invisible on any install
that leaves the two keys unset, because then both sides fall back to the same
default — which is why this harness configures them.
"""
import asyncio
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger, Rpc  # noqa: E402

import websockets  # noqa: E402

WS = sys.argv[1]
ALEPH_HOME = Path(sys.argv[2])
AGENTS_ROOT = Path(sys.argv[3])
WORKSPACE_ROOT = Path(sys.argv[4])
AGENT_ID = sys.argv[5] if len(sys.argv) > 5 else "qa-leftover-agent"

L = Ledger()


def descriptions_from(payload):
    """Pull {name: description} out of a tools.* response, shape-agnostically.

    The two faces do not have to agree on their envelope for the claim to
    hold; they have to agree on the bytes. Walking the JSON rather than
    hardcoding a path keeps this from silently finding nothing when the
    envelope changes — `assert_found` below turns that into a failure.
    """
    found = {}

    def walk(node):
        if isinstance(node, dict):
            name, desc = node.get("name"), node.get("description")
            if isinstance(name, str) and isinstance(desc, str):
                found.setdefault(name, desc)
            for v in node.values():
                walk(v)
        elif isinstance(node, list):
            for v in node:
                walk(v)

    walk(payload)
    return found


async def main():
    async with websockets.connect(WS, max_size=32 * 1024 * 1024) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-leftovers")

        # ---------------------------------------------------------------- A
        L.log("\n--- A. converged tool descriptions on the wire ---")
        cat = await rpc.call("tools.catalog", {})
        cat_desc = descriptions_from(cat.get("result", {}))
        L.check(
            "tools.catalog serves a non-trivial tool list",
            len(cat_desc) > 20,
            f"{len(cat_desc)} named+described entries",
        )

        pdf = cat_desc.get("pdf_generate", "")
        L.check("pdf_generate is served", bool(pdf), f"{len(pdf)}B")
        # The inherent copy carried a three-line `Examples:` block the catalog
        # copy never had. Convergence means the block is gone everywhere, not
        # that one face still holds it.
        L.check(
            "pdf_generate description carries no stale Examples block",
            "Examples:" not in pdf,
            repr(pdf[-70:]),
        )
        L.check(
            "pdf_generate description is the catalog text",
            pdf.rstrip().endswith('for generated PDFs.'),
            repr(pdf[-70:]),
        )

        img = cat_desc.get("image_generate", "")
        L.check(
            "image_generate serves the single converged text",
            img == "Generate images from text prompts using AI image generation providers.",
            repr(img),
        )

        eff = await rpc.call("tools.effective", {"agent_id": "main"})
        eff_desc = descriptions_from(eff.get("result", {}))
        if eff_desc:
            overlap = set(cat_desc) & set(eff_desc)
            disagree = sorted(n for n in overlap if cat_desc[n] != eff_desc[n])
            L.check(
                "every tool named by both faces carries identical bytes on each",
                not disagree,
                f"{len(overlap)} shared; disagreeing: {disagree[:5]}",
            )
        else:
            L.log("  [skip] tools.effective returned no described entries")

        # ---------------------------------------------------------------- B
        # Driven through `hooks.add` / the runtime list rather than the
        # `hooks_manage` tool: `hooks.add` is the writer that was resolving its
        # own home-rooted path while `load_user_hooks` read `ALEPH_HOME`, and
        # the runtime list is the reader that could not see what it wrote. The
        # `hooks_manage` tool's `add` raises an approval this surface cannot
        # answer, which is correct behaviour and not the thing under test.
        L.log("\n--- B. hooks writer and reader agree under a relocated ALEPH_HOME ---")
        marker = "echo aleph-qa-leftovers-marker"
        added = await rpc.call(
            "hooks.add", {"event": "PostToolUse", "command": marker, "matcher": "*"}
        )
        ok = "error" not in added
        L.check("hooks.add succeeds", ok, json.dumps(added.get("error", ""))[:200])

        reported = json.dumps(added.get("result", {}))
        path_field = (added.get("result") or {}).get("path", "")
        L.check(
            "hooks.add reports a path inside the relocated ALEPH_HOME",
            bool(path_field) and Path(path_field).resolve().is_relative_to(ALEPH_HOME.resolve()),
            path_field or reported[:160],
        )

        on_disk = sorted(p for p in ALEPH_HOME.rglob("hooks*.json"))
        wrote_marker = [p for p in on_disk if marker in p.read_text(errors="ignore")]
        L.check(
            "the hook is on disk under the relocated ALEPH_HOME",
            bool(wrote_marker),
            ", ".join(str(p.relative_to(ALEPH_HOME)) for p in wrote_marker) or f"searched {len(on_disk)} file(s)",
        )

        # The failure this pair exists to catch writes to the developer's real
        # home instead. That is invisible from the writer's own success flag.
        real_home = Path(os.environ["REAL_HOME"]) / ".aleph" if "REAL_HOME" in os.environ else None
        if real_home and real_home.is_dir():
            stray = [
                p for p in real_home.glob("hooks*.json")
                if marker in p.read_text(errors="ignore")
            ]
            L.check("the developer's real ~/.aleph was not written to", not stray, str(stray[:2]))

        async def runtime_sees_marker():
            ok, body = await rpc.invoke("hooks_manage", {"action": "list"})
            return ok, marker in json.dumps(body), json.dumps(body)

        ok, seen, body = await runtime_sees_marker()
        L.check("the runtime hooks view is readable", ok, body[:120])
        if not seen:
            # Hot-load is a separate promise from writer/reader agreement.
            # Reload explicitly and re-ask, so a miss says which of the two
            # failed instead of collapsing both into one red.
            await rpc.call("hooks.reload", {})
            ok, seen, body = await runtime_sees_marker()
            L.log("  [note] marker appeared only after an explicit hooks.reload" if seen
                  else "  [note] marker absent even after hooks.reload")
        L.check(
            "the reader sees the hook the writer just wrote",
            seen,
            "" if seen else "runtime view does not contain the marker — writer/reader split",
        )

        # ---------------------------------------------------------------- C
        L.log("\n--- C. provisioning honours [agents.defaults] ---")
        created, body = await rpc.invoke(
            "agent_create",
            {"id": AGENT_ID, "name": "QA Leftover", "description": "qa"},
        )
        L.check("agent_create succeeds", created, json.dumps(body)[:220])

        state_dir = AGENTS_ROOT / AGENT_ID
        workspace = WORKSPACE_ROOT / AGENT_ID
        L.check(
            "the agent state dir landed under the configured agents_root",
            state_dir.is_dir(),
            str(state_dir),
        )
        L.check(
            "SOUL.md was provisioned there",
            (state_dir / "SOUL.md").is_file(),
            str(state_dir / "SOUL.md"),
        )
        L.check(
            "the workspace landed under the configured workspace_root",
            workspace.is_dir(),
            str(workspace),
        )
        # The discriminating half: before the roots were threaded through,
        # provisioning wrote the default layout while the resolver rebuilt the
        # agent from the configured one. Both sides "worked"; they never met.
        #
        # Gated on the create having actually happened — a refusal leaves the
        # default layout empty too, and an ungated assertion would report that
        # as a pass. A vacuous green here is the exact shape this QA exists to
        # stop trusting.
        if created:
            for stray in (ALEPH_HOME / "agents" / AGENT_ID, ALEPH_HOME / "workspaces" / AGENT_ID):
                L.check(
                    f"nothing was provisioned into the default layout ({stray.parent.name}/{stray.name})",
                    not stray.exists(),
                    str(stray),
                )
        else:
            L.log("  [skip] default-layout claims are vacuous unless the create ran")

    return L.verdict()


sys.exit(asyncio.run(main()))
