# Server-Centric Architecture Reframing Design

> Date: 2026-02-23
> Status: Approved
> Supersedes: 2026-02-06-server-client-architecture-design.md, 2026-02-06-server-client-implementation.md

## Motivation

The current "Server-Client" framing implies Clients have local execution capabilities (tool execution, shell commands, file system operations). In reality, all Interfaces (macOS App, Tauri Desktop, CLI, Telegram Bot, Discord Bot, WebChat) are **pure I/O layers** — they send user input and display Server responses, nothing more.

This misalignment between terminology and reality risks misleading future development. This design reframes the architecture to match its true nature: **Aleph is a self-contained, privately deployable AI Server**.

## Core Definition Change

**Before:**
```
Server = Brain (Agent Loop, LLM)
Client = Hands (Local tool execution, Shell, File system)
```

**After:**
```
Aleph Server = Complete AI Engine (Perception + Thinking + Execution, all server-side)
Interface = Human interaction endpoint (Pure I/O, zero business logic)
```

## Terminology Table

| Old Term | New Term | Notes |
|----------|----------|-------|
| Client | Interface | Any connection endpoint to the Server |
| Server-Client architecture | Server-centric architecture | All logic centralized in Server |
| ClientManifest | (deleted) | Interfaces don't declare capabilities |
| ExecutionPolicy | (deleted) | No routing decisions, all Server execution |
| ReverseRpc | (deleted) | Server never calls Interface for execution |
| channels/ | interfaces/ | Message channels → interaction interfaces |
| clients/ | apps/ | Client apps → application shells |

## Interface Classification

| Type | Examples | Communication |
|------|----------|---------------|
| **Native App** | macOS App, Tauri Desktop | WebSocket |
| **CLI** | aleph-cli | WebSocket |
| **Bot** | Telegram, Discord | Platform APIs |
| **Web** | Dashboard, WebChat | HTTP/WebSocket |

Common characteristics of all Interfaces:
- Send user input to Server
- Receive and display Server responses
- Execute NO tools or business logic
- Declare NO execution capabilities

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────────────┐
│                       INTERFACE LAYER                            │
│   macOS App │ Tauri App │ CLI │ Telegram │ Discord │ WebChat    │
│              (Pure I/O — input user messages, display responses) │
└───────────────────────────────┬─────────────────────────────────┘
                                │ WebSocket (JSON-RPC 2.0)
                                │ ws://127.0.0.1:18789
┌───────────────────────────────┴─────────────────────────────────┐
│                    ALEPH SERVER (Self-contained)                  │
│                                                                   │
│  ┌──────────────────────────────────────────────────────────┐    │
│  │ GATEWAY — Router │ Session │ Event Bus │ Interfaces      │    │
│  └──────────────────────────────┬───────────────────────────┘    │
│                                 │                                 │
│  ┌──────────────────────────────┴───────────────────────────┐    │
│  │ AGENT — Observe → Think → Act → Feedback → Compress      │    │
│  └──────────────────────────────┬───────────────────────────┘    │
│                                 │                                 │
│  ┌──────────────────────────────┴───────────────────────────┐    │
│  │ EXECUTION — Providers │ Executor │ Tool Server │ MCP      │    │
│  └──────────────────────────────┬───────────────────────────┘    │
│                                 │                                 │
│  ┌──────────────────────────────┴───────────────────────────┐    │
│  │ STORAGE — Memory (LanceDB) │ State (SQLite) │ Config      │    │
│  └──────────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────────┘
```

## Implementation Plan

### Phase 1 — Semantic Refactoring (Directory rename + Documentation)

#### Directory Renames
- `clients/` → `apps/`
- `core/src/gateway/channels/` → `core/src/gateway/interfaces/`

#### Affected References for `clients/` → `apps/`
- `Cargo.toml` workspace members
- `.gitignore` paths
- `build-macos.sh` script
- `CLAUDE.md` all references
- All documentation path references

#### Affected References for `channels/` → `interfaces/`
- `core/src/gateway/mod.rs` — mod declarations
- Feature flags references
- `core/src/lib.rs` re-exports
- All `use crate::gateway::channels::` import statements

#### Documentation Updates
| Document | Changes |
|----------|---------|
| **CLAUDE.md** | Replace architecture diagram, remove Server-Client section, update project structure, update terminology |
| **docs/ARCHITECTURE.md** | Replace architecture diagram, remove ToolRouter/ExecutionPolicy descriptions, update to Server-centric |
| **docs/GATEWAY.md** | Remove ReverseRpc, ClientManifest descriptions, update Interface access documentation |
| **docs/TOOL_SYSTEM.md** | Remove ExecutionPolicy content, simplify execution path description |
| **README.md** | Update architecture overview and terminology |

#### Design Documents to Mark as Superseded
- `docs/plans/2026-02-06-server-client-architecture-design.md` — add SUPERSEDED header
- `docs/plans/2026-02-06-server-client-implementation.md` — add SUPERSEDED header

### Phase 2 — Code Deletion (Distributed execution infrastructure)

#### Files to Delete
| File | Content |
|------|---------|
| `shared/protocol/src/policy.rs` | ExecutionPolicy enum |
| `shared/protocol/src/manifest.rs` | ClientManifest, ClientCapabilities, ClientEnvironment |
| `core/src/gateway/reverse_rpc.rs` | ReverseRpcManager |
| `core/src/gateway/client_manifest.rs` | Re-exports of manifest types |
| `core/src/executor/router.rs` | ToolRouter |
| `core/src/executor/routed_executor.rs` | RoutedExecutor |
| `core/src/dispatcher/types/execution_policy.rs` | ExecutionPolicy re-export |
| `apps/cli/src/executor.rs` | LocalExecutor |

#### Files to Modify (Remove related fields/references)
| File | Changes |
|------|---------|
| `core/src/gateway/server.rs` | Remove `ConnectionState.manifest` field |
| `core/src/gateway/handlers/auth.rs` | Remove `ConnectParams.manifest` parameter |
| `core/src/dispatcher/types/unified.rs` | Remove `execution_policy` field from UnifiedTool |
| `core/src/dispatcher/types/mod.rs` | Remove ExecutionPolicy exports |
| `core/src/executor/mod.rs` | Remove router/routed exports |
| `apps/cli/src/client.rs` | Remove tool.call reverse RPC handler |
| `apps/shared/src/client.rs` | Remove tool execution capability |

#### Protocol Simplification
**Remove:**
- `tool.call` (Server→Interface) — no longer needed
- `tool.result` (Interface→Server) — no longer needed
- Capability negotiation messages

**Retain:**
- `connect` / `disconnect` — connection management
- `chat.send` / `chat.stream` — message communication
- `session.*` — session management
- All other existing RPC methods

## Success Criteria

1. All distributed execution code removed (ExecutionPolicy, ClientManifest, ReverseRpc, ToolRouter, RoutedExecutor)
2. Directory renames applied (`clients/` → `apps/`, `channels/` → `interfaces/`)
3. All documentation updated with new terminology
4. `cargo build` and `cargo test` pass after each phase
5. No remaining references to "Client" in execution context (only "Interface" or "App")
