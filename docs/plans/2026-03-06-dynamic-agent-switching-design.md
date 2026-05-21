# Dynamic Agent Switching Design

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Enable natural language agent switching with dynamic agent creation at the router layer.

**Architecture:** InboundMessageRouter intercepts switch intent via hybrid detection (keyword match + LLM fallback), dynamically creates agents if needed (with LLM-generated SOUL.md), and switches the active agent for the channel/peer.

## Context

Currently only `/switch <agent_id>` command works, and only for pre-configured agents. Users want to say "帮我切换到交易助手" and have the system:
1. Detect the switch intent
2. Create the agent if it doesn't exist
3. Generate an initial persona (SOUL.md)
4. Switch the active agent for the channel

## Architecture

```
User Message -> InboundMessageRouter
                  |
                  +-- 1. /switch command (existing, unchanged)
                  |
                  +-- 2. Keyword fast match (zero latency)
                  |     "切换到X" / "switch to X" / "换成X"
                  |     -> matched -> extract name -> step 4
                  |
                  +-- 3. LLM intent classification (fallback)
                  |     -> {intent: "switch", id: "trading", name: "交易助手"}
                  |     -> {intent: "normal"} -> normal routing
                  |
                  +-- 4. Agent exists?
                  |     +-- yes -> switch (set_active_agent)
                  |     +-- no  -> dynamic create
                  |           +-- LLM generates SOUL.md
                  |           +-- Create ~/.aleph/workspaces/{id}/
                  |           +-- Register in AgentRegistry
                  |           +-- Switch
                  |
                  +-- 5. Reply confirmation to user
```

## Key Decisions

| Item | Decision |
|------|----------|
| Trigger | Hybrid: keyword match -> LLM fallback |
| agent_id | LLM returns `{id: "trading", name: "交易助手"}` |
| Persona | LLM generates initial SOUL.md; user can edit later |
| Directory | `~/.aleph/workspaces/{id}/` |
| Switch back | Same intent detection path (main already exists) |
| channel/peer | Router has context natively, no args needed |
| Workspace files | Only SOUL.md generated; other 6 files use defaults |

## Workspace Files (7 total, from parallel branch)

| File | Required | Created on dynamic agent? |
|------|----------|--------------------------|
| SOUL.md | No | Yes - LLM generated |
| IDENTITY.md | No | No - uses default |
| AGENTS.md | No | No - no project context |
| TOOLS.md | No | No - uses builtin descriptions |
| MEMORY.md | No | No - no memory injection |
| HEARTBEAT.md | No | No - uses default heartbeat |
| BOOTSTRAP.md | No | No - skip bootstrap |

## Keyword Match Rules

Chinese patterns:
- `切换到(.+)`
- `换成(.+)`
- `我想(和|跟)?(.+)(聊|说|谈)`

English patterns:
- `switch to (.+)`
- `change to (.+)`

When keyword matches, the extracted name still needs a lightweight LLM call to get the English `id`:
```
Given agent name "{name}", return a short English snake_case id.
Examples: "交易助手" -> "trading", "健康顾问" -> "health"
Return only the id, nothing else.
```

## LLM Intent Classification (keyword miss fallback)

Prompt:
```
Classify this message. If the user wants to switch to a different AI agent/persona,
return JSON: {"intent":"switch","id":"english_snake_case","name":"display name"}
Otherwise return: {"intent":"normal"}
Message: {msg}
```

## SOUL.md Generation

Prompt:
```
Generate a concise AI persona description for an agent named "{name}" (id: {id}).
Write 3-5 sentences in the user's language describing this agent's expertise,
communication style, and personality. Be specific to the domain.
```

Output saved to `~/.aleph/workspaces/{id}/SOUL.md`.

## Changes Required

1. **`InboundMessageRouter`** - Add intent detection layer (keyword + LLM)
2. **`AgentRegistry`** - Add `create_dynamic(id, name, soul_content)` method
3. **Remove `switch_agent` tool** - No longer needed (router handles everything)
4. **`WorkspaceManager`** - Reuse existing `set_active_agent()`

## Not Changing

- `aleph.toml` config structure
- SessionKey mechanism
- Agent loop / thinker internals
- `/switch` command (still works as before)
