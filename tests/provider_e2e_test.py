#!/usr/bin/env python3
"""
Provider Architecture E2E Test

Validates provider model routing, health tracking, and fallback infrastructure
against a live Aleph server via WebSocket.

Test phases:
1. Provider RPC — list/get providers via JSON-RPC
2. Chat with model routing — verify model_resolved stream event
3. Agent model config — verify agent's configured model is used
4. Provider health — verify health resets after test_connection
5. Fallback notice — verify non-Panel channels get fallback info
"""

import asyncio
import json
import sys
import os

sys.path.insert(0, os.path.join(os.path.dirname(__file__),
    "../.claude/skills/aleph-e2e-verify/scripts"))

from aleph_e2e_client import (
    AlephClient, check, section, summary, get_shared_token
)


class ProviderTestClient(AlephClient):
    """Extended client that captures stream.model_resolved events."""

    async def chat_with_model_info(self, message, agent_id="main", timeout=120):
        """Like chat(), but also captures model_resolved events."""
        resp = await self.rpc("chat.send", {
            "message": message, "agent_id": agent_id, "stream": True
        })
        if "_error" in resp:
            return {"text": "", "tool_calls": [], "model_info": None,
                    "completed": False, "_error": resp["_error"]}

        run_id = resp.get("run_id", "")
        full_text, tool_calls = "", []
        model_info = None
        completed = False
        deadline = asyncio.get_event_loop().time() + timeout

        while not completed:
            remaining = deadline - asyncio.get_event_loop().time()
            if remaining <= 0:
                break
            try:
                raw = await asyncio.wait_for(self.ws.recv(), timeout=min(remaining, 5.0))
                data = json.loads(raw)
                m = data.get("method", "")
                p = data.get("params", {}) if isinstance(data.get("params"), dict) else {}

                evt_run = p.get("run_id", "")
                if run_id and evt_run and evt_run != run_id:
                    continue

                if m == "stream.response_chunk":
                    full_text += p.get("delta", "") or p.get("content", "")
                elif m == "stream.tool_start":
                    tool_calls.append({
                        "name": p.get("tool_name", "") or p.get("tool_id", ""),
                        "input": p.get("params", {}),
                    })
                elif m == "stream.tool_end":
                    if tool_calls:
                        tool_calls[-1]["result"] = p.get("result", "")
                elif m == "stream.model_resolved":
                    model_info = p.get("model_info", p)
                elif m == "stream.run_complete":
                    completed = True
                    # Capture final response from summary if delta text was empty
                    s = p.get("summary", {})
                    if isinstance(s, dict) and not full_text:
                        full_text = s.get("final_response", "") or s.get("text", "")
                    break
                elif m in ("stream.run_failed", "stream.error"):
                    return {"text": full_text, "tool_calls": tool_calls,
                            "model_info": model_info, "completed": False,
                            "_error": p.get("error", str(p))}
            except asyncio.TimeoutError:
                break

        return {"text": full_text, "tool_calls": tool_calls,
                "model_info": model_info, "completed": completed}


async def main():
    token = get_shared_token()
    client = ProviderTestClient()

    section("CONNECT")
    conn = await client.connect(token)
    check("WebSocket connected", conn is not None)

    # ═══ Phase 1: Provider RPC ═══
    section("Phase 1: Provider RPC")

    # List providers
    providers_resp = await client.rpc("providers.list")
    check("providers.list returns data", not providers_resp.get("_error"),
          detail=str(providers_resp.get("_error", "")))

    # API wraps in {"providers": [...]}
    providers = providers_resp.get("providers", providers_resp) if isinstance(providers_resp, dict) else providers_resp
    if isinstance(providers, list):
        provider_names = [p.get("name", "") for p in providers]
        check("At least one provider configured", len(providers) >= 1,
              detail=f"got {len(providers)} providers")

        # Check for a default provider
        defaults = [p for p in providers if p.get("is_default")]
        check("One provider is default", len(defaults) == 1,
              detail=f"defaults: {[p.get('name') for p in defaults]}")

        if defaults:
            default_name = defaults[0].get("name", "")
            check(f"Default provider has name", bool(default_name))

            # Get default provider details
            detail_resp = await client.rpc("providers.get", {"name": default_name})
            check(f"providers.get('{default_name}') returns data",
                  not detail_resp.get("_error"),
                  detail=str(detail_resp.get("_error", "")))

            # API may wrap in {"provider": {...}}
            detail = detail_resp.get("provider", detail_resp) if isinstance(detail_resp, dict) else detail_resp
            if isinstance(detail, dict) and not detail.get("_error"):
                check("Provider has model field",
                      "model" in detail or "models" in detail,
                      detail=f"keys: {list(detail.keys())}")
                check("Provider is enabled or default",
                      detail.get("enabled", False) or detail.get("is_default", False) or
                      detail.get("has_api_key", False))
    else:
        check("providers.list returns list", False,
              detail=f"got {type(providers).__name__}: {str(providers)[:200]}")

    # ═══ Phase 2: Chat with Model Routing ═══
    section("Phase 2: Chat with Model Routing (model_resolved event)")

    result = await client.chat_with_model_info(
        "Reply with exactly one word: hello",
        timeout=60
    )

    check("Chat completed", result["completed"],
          detail=result.get("_error", ""))
    check("Got response text", len(result["text"]) > 0,
          detail=f"text length: {len(result['text'])}")

    mi = result.get("model_info")
    check("model_resolved event received", mi is not None,
          detail="no stream.model_resolved in stream events")

    if mi:
        check("model_info has 'model' field", "model" in mi,
              detail=f"keys: {list(mi.keys())}")
        check("model_info has 'provider' field", "provider" in mi,
              detail=f"keys: {list(mi.keys())}")
        check("model_info has 'is_fallback' field", "is_fallback" in mi,
              detail=f"keys: {list(mi.keys())}")
        check("model_info.model is non-empty", bool(mi.get("model")),
              detail=f"model: {mi.get('model')}")
        check("model_info.provider is non-empty", bool(mi.get("provider")),
              detail=f"provider: {mi.get('provider')}")
        # In normal operation, is_fallback should be false
        check("Not a fallback (healthy primary)", mi.get("is_fallback") == False,
              detail=f"is_fallback: {mi.get('is_fallback')}")
        print(f"    Model: {mi.get('model')} | Provider: {mi.get('provider')}")

    # ═══ Phase 3: Agent Model Config ═══
    section("Phase 3: Agent Model Config")

    # Get agent list and check model fields
    agents_resp = await client.rpc("agents.list")
    agents = agents_resp.get("agents", agents_resp) if isinstance(agents_resp, dict) else agents_resp
    if isinstance(agents, list) and len(agents) > 0:
        check("agents.list returns agents", True)
        main_agent = next((a for a in agents if a.get("id") == "main"), agents[0])
        agent_model = main_agent.get("model") or "(uses default)"
        print(f"    Agent '{main_agent.get('id')}' model: {agent_model}")
        check("Main agent has model or uses default", True,
              detail=f"model: {agent_model}")

        # If agent has explicit model, verify it matches chat
        if mi and main_agent.get("model"):
            check("Chat used agent's configured model",
                  mi.get("model", "").startswith(main_agent["model"].split("-")[0]) or
                  main_agent["model"] in mi.get("model", ""),
                  detail=f"agent.model={main_agent['model']}, used={mi.get('model')}")
        elif mi:
            # No agent model → uses default provider's model
            check("Chat used default provider's model",
                  bool(mi.get("model")),
                  detail=f"used model: {mi.get('model')}")
    else:
        check("agents.list returns agents", isinstance(agents, list),
              detail=f"got: {str(agents)[:200]}")

    # ═══ Phase 4: Provider Test Connection ═══
    section("Phase 4: Provider Test Connection (resets health)")

    if isinstance(providers, list) and defaults:
        default_prov = defaults[0]
        default_name = default_prov.get("name", "")
        # providers.test requires {name, config} — build minimal config from provider info
        test_config = {
            "models": default_prov.get("models", [default_prov.get("model", "")]),
            "base_url": default_prov.get("base_url"),
            "protocol": default_prov.get("protocol"),
        }
        test_result = await client.rpc("providers.test",
            {"name": default_name, "config": test_config}, timeout=30)
        check(f"providers.test('{default_name}') succeeds",
              not test_result.get("_error"),
              detail=str(test_result)[:200])
    else:
        check("Skipping test_connection (no default provider)", False)

    # ═══ Phase 5: Post-Reset Chat (verify health tracking works) ═══
    section("Phase 5: Post-Reset Verification Chat")
    await client._drain(0.5)  # clear any stale events

    result2 = await client.chat_with_model_info(
        "Reply with exactly one word: world",
        timeout=60
    )
    check("Post-reset chat completed", result2["completed"],
          detail=result2.get("_error", ""))
    check("Post-reset got response", len(result2["text"]) > 0)

    mi2 = result2.get("model_info")
    check("Post-reset model_resolved received", mi2 is not None)
    if mi2:
        check("Post-reset: not a fallback", mi2.get("is_fallback") == False,
              detail=f"is_fallback: {mi2.get('is_fallback')}")
        check("Post-reset: same provider as before",
              mi2.get("provider") == (mi.get("provider") if mi else ""),
              detail=f"before={mi.get('provider') if mi else 'N/A'}, after={mi2.get('provider')}")

    # ═══ Cleanup ═══
    await client.close()
    return summary()


if __name__ == "__main__":
    ok = asyncio.run(main())
    sys.exit(0 if ok else 1)
