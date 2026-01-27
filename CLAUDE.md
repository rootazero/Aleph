<!-- OPENSPEC:START -->
# OpenSpec Instructions

These instructions are for AI assistants working in this project.

Always open `@/openspec/AGENTS.md` when the request:
- Mentions planning or proposals (words like proposal, spec, change, plan)
- Introduces new capabilities, breaking changes, architecture shifts, or big performance/security work
- Sounds ambiguous and you need the authoritative spec before coding

Use `@/openspec/AGENTS.md` to learn:
- How to create and apply change proposals
- Spec format and conventions
- Project structure and guidelines

Keep this managed block so 'openspec update' can refresh the instructions.

<!-- OPENSPEC:END -->

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Aether** is a system-level AI middleware for macOS (native) and cross-platform (Tauri). It acts as an invisible "ether" connecting user intent with AI models through a frictionless interface.

**Current Status**: Phase 9 Complete (Agent Loop Hardening)

### Core Philosophy: "Ghost" Aesthetic

- **Invisible First**: No dock icon, no permanent window. Only background process + menu bar
- **De-GUI**: Ephemeral UI that appears at cursor, then dissolves
- **Frictionless**: AI intelligence directly at cursor without context switching
- **Native-First**: 100% native code - Rust core with platform-specific UI

### ⚠️ Critical: Aether is an AI Agent

**Aether 是 AI Agent，不是简单的工具路由器。** 必须支持多步骤任务：
- **任务分解**: 复杂请求分解为子任务
- **依赖管理**: DAG 调度处理依赖
- **上下文传递**: 任务间输出传递
- **错误恢复**: 失败处理策略

**关键模块**: `agent_loop`, `dispatcher/planner`, `dispatcher/scheduler`, `agents/sub_agents`

---

## Technical Stack

| Layer | Technology |
|-------|------------|
| **Rust Core** | AetherTool system, UniFFI 0.31+, tokio, reqwest |
| **macOS UI** | Swift + SwiftUI (Native) |
| **Cross-Platform** | Tauri 2.0 + React + TypeScript |

See [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) for complete technical documentation.

---

## Project Structure

```
aether/
├── core/                      # Rust Core (~35 modules)
│   └── src/
│       ├── agent_loop/        # Core observe-think-act-feedback
│       ├── agents/            # Agent system + sub-agents
│       ├── components/        # 8 agentic loop components
│       ├── dispatcher/        # Task orchestration (16 sub-modules)
│       ├── extension/         # Plugin system (Claude Code compatible)
│       └── ...
├── platforms/
│   ├── macos/                 # Swift + SwiftUI
│   └── tauri/                 # Cross-platform (Windows, Linux)
└── docs/                      # Documentation
```

See [docs/DIRECTORY_STRUCTURE.md](./docs/DIRECTORY_STRUCTURE.md) for detailed structure.

---

## Quick Commands

```bash
# Rust Core
cd core && cargo build && cargo test

# macOS
cd platforms/macos && xcodegen generate && open Aether.xcodeproj

# Tauri
cd platforms/tauri && pnpm install && pnpm tauri dev
```

See [docs/BUILD_COMMANDS.md](./docs/BUILD_COMMANDS.md) for complete reference.

---

## Key Architecture Components

| Component | Description |
|-----------|-------------|
| **agent_loop** | Core observe-think-act-feedback cycle with doom loop detection, retry, multi-tool execution |
| **dispatcher** | Multi-layer routing: planner, scheduler, executor, model_router, etc. |
| **extension** | Claude Code compatible plugins with async FFI (UniFFI 0.31+) |
| **MCP** | Model Context Protocol with stdio/HTTP/SSE, OAuth 2.0 |
| **thinker** | LLM decision-making with model routing, prompt building |
| **three_layer** | Control architecture: Orchestrator (FSM) / Skill (DAG) / Tools |

See [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md), [docs/DISPATCHER.md](./docs/DISPATCHER.md), [docs/AGENT_LOOP.md](./docs/AGENT_LOOP.md) for details.

---

## Conversation Modes

Aether supports two conversation modes with different lifecycle and scope:

### 🔒 Single-Turn (单轮对话) - FROZEN

**定位**: Fast, stateless responses for ephemeral tasks.

**Use Cases**:
- Input method spell checking / correction
- Quick queries without context
- Temporary lookups or translations

**Implementation**:
- Location: `core/src/ffi/processing/orchestration.rs` (single-turn branch)
- Identifier: `ProcessOptions.topic_id = None`
- Memory: In-memory only, no persistence
- History: No context injection

**Development Constraints**:
- ⚠️ **FROZEN**: Feature-locked as of Phase 9
- ❌ No new features or capabilities
- ❌ No modifications to execution logic (except critical bug fixes)
- ✅ Performance optimizations allowed (without changing behavior)

**Rationale**: Single-turn functionality is stable and sufficient. Future efforts focus on multi-turn AI agent capabilities.

---

### ✅ Multi-Turn (多轮对话) - ACTIVE DEVELOPMENT

**定位**: AI Agent with persistent sessions for complex, multi-step tasks.

**Use Cases**:
- Complex task decomposition (DAG scheduling)
- Multi-step workflows with dependency management
- Context-aware conversations with history
- Agent Loop with doom loop detection

**Implementation**:
- Location: `core/src/ffi/processing/` (agent_loop.rs, dag_executor.rs)
- Identifier: `ProcessOptions.topic_id = Some(uuid)`
- Persistence: SQLite (`ConversationStore`) + in-memory cache
- History: Full context injection from previous turns

**Development Focus** (Active):
- ✅ Agent Loop enhancements (retry strategies, tool orchestration)
- ✅ DAG scheduler optimization (parallel execution, dependency resolution)
- ✅ Session management (compression, cross-device sync)
- ✅ Advanced AI capabilities (multi-model routing, adaptive planning)

**Key Modules**: `agent_loop`, `dispatcher/planner`, `dispatcher/scheduler`, `agents/sub_agents`

---

### Mode Distinction in Code

| Aspect | Single-Turn | Multi-Turn |
|--------|-------------|------------|
| **topic_id** | `None` (constant "single-turn" in memory) | `Some(uuid)` |
| **Working Directory** | System default (cleared) | `output_dir/<topic_id>/` |
| **History Injection** | ❌ None | ✅ From `conversation_histories` |
| **Persistence** | ❌ None | ✅ SQLite + memory |
| **Agent Loop** | Lightweight (skipped if possible) | Full cycle (observe-think-act-feedback) |
| **DAG Scheduling** | N/A | ✅ For multi-step tasks |

**Critical**: All future feature development should target **multi-turn mode**. Single-turn is intentionally kept minimal and stable.

---

## Key Constraints

- **macOS Entry**: Use `main.swift` + `NSApplicationMain()` (not SwiftUI @main)
- **Halo Window**: Never call `makeKeyAndOrderFront()` - zero focus theft
- **Business Logic**: Rust core only, Swift is UI layer
- **FFI**: Use UniFFI, never manual bindings

See [docs/DESIGN_CONSTRAINTS.md](./docs/DESIGN_CONSTRAINTS.md) for full constraints.

---

## Documentation Index

| Category | Documents |
|----------|-----------|
| **Architecture** | [ARCHITECTURE](./docs/ARCHITECTURE.md), [DISPATCHER](./docs/DISPATCHER.md), [AGENT_LOOP](./docs/AGENT_LOOP.md) |
| **Development** | [BUILD_COMMANDS](./docs/BUILD_COMMANDS.md), [DIRECTORY_STRUCTURE](./docs/DIRECTORY_STRUCTURE.md), [DEVELOPMENT_SETUP](./docs/DEVELOPMENT_SETUP.md) |
| **Platform** | [PLATFORM_NOTES](./docs/PLATFORM_NOTES.md), [DESIGN_CONSTRAINTS](./docs/DESIGN_CONSTRAINTS.md) |

---

## Development

### Branch Strategy

**单分支开发模式**：所有开发工作直接在 main 分支进行。仅在需要隔离测试时临时创建分支。

### Git Commit

After completing a task, use `git add` and `git commit` with English commit messages.

### Environment

See [docs/DEVELOPMENT_SETUP.md](./docs/DEVELOPMENT_SETUP.md) for Python, Xcode, and other environment setup.

---

## Session

### Memory Prompt

When token is low to 10%, summarize this session to generate a "memory prompt" for next session inheritance.

### Language

- Reply in Chinese
- Program comments in English

### Skills

Use skills from: `~/.claude/skills/build-macos-apps`
