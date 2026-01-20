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

**Aether** is a system-level AI middleware for macOS (primary), Windows, and Linux. It acts as an invisible "ether" connecting user intent with AI models through a frictionless, native interface with zero webview dependencies.

**Current Status**: Phase 8 Completed (Runtime Manager) | Phase 9 Planned (Production Hardening)

### Core Philosophy: "Ghost" Aesthetic

- **Invisible First**: No dock icon, no permanent window. Only background process + menu bar/system tray
- **De-GUI**: Ephemeral UI that appears at cursor, then dissolves
- **Frictionless**: Brings AI intelligence directly to the cursor without context switching
- **Native-First**: 100% native code - Rust core with platform-specific UI (Swift, C#, GTK)

### ⚠️ Critical: Aether is an AI Agent

**Aether 是 AI Agent，不是简单的工具路由器。**

复杂多步骤任务是 Agent 的核心能力，必须支持：
- **任务分解**: 将复杂请求分解为多个子任务（如："分析文档 → 生成prompt → 绘制图像"）
- **依赖管理**: 子任务之间的依赖关系（DAG 调度）
- **上下文传递**: 前一个任务的输出作为后一个任务的输入
- **错误恢复**: 单个子任务失败时的处理策略

**关键模块**:
- `UnifiedPlanner`: LLM 驱动的任务分解，生成 ExecutionPlan
- `UnifiedExecutor`: DAG 调度执行器，处理 TaskGraph
- `RequestOrchestrator`: 请求入口，需要集成 Planner 支持复杂任务

**没有多步骤任务支持，Aether 就不是真正的 Agent。**

---

## Technical Stack

**Architecture**: Rust Core + rig-core + UniFFI + Native UI

**NO WEBVIEWS. NO TAURI. NO ELECTRON.**

| Layer | Technology |
|-------|------------|
| **Rust Core** | rig-core 0.28, rig-sqlite, UniFFI, tokio, reqwest |
| **macOS UI** | Swift + SwiftUI, NSApplicationMain() entry |
| **Windows UI** | C# + WinUI 3 (Future) |
| **Linux UI** | Rust + GTK4 (Future) |

See [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) for complete technical documentation.

---

## Project Structure

```
aether/
├── Cargo.toml                 # Workspace root
├── VERSION                    # Single version source
├── core/                      # Rust Core (44 modules)
│   ├── src/
│   │   ├── lib.rs             # UniFFI/C ABI exports
│   │   ├── agent/             # Agent execution
│   │   ├── components/        # 8 agentic loop components
│   │   ├── dispatcher/        # Multi-layer routing
│   │   ├── memory/            # Dual-layer memory
│   │   └── ...
│   └── Cargo.toml             # Features: uniffi, cabi
├── platforms/
│   ├── macos/                 # Swift + SwiftUI
│   │   ├── project.yml        # XcodeGen config
│   │   └── Aether/Sources/
│   └── windows/               # C# + WinUI 3
├── scripts/                   # Build scripts
└── docs/                      # Documentation
```

See [docs/DIRECTORY_STRUCTURE.md](./docs/DIRECTORY_STRUCTURE.md) for detailed tree.

---

## Development Workflow

```
┌─────────────────────────────────────────────────────────────────┐
│                       开发工作流                                 │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  1. 修改 Rust Core                                              │
│     └─ cd core && cargo test                                    │
│                                                                  │
│  2. 构建平台特定库                                               │
│     ├─ macOS:   ./scripts/build-core.sh macos                   │
│     └─ Windows: .\scripts\build-windows.ps1                     │
│                                                                  │
│  3. 开发 UI                                                      │
│     ├─ macOS:   cd platforms/macos && xcodegen && open *.xcodeproj │
│     └─ Windows: cd platforms/windows && dotnet watch            │
│                                                                  │
│  4. 提交                                                         │
│     └─ git commit (触发对应平台的 CI)                            │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Branch Strategy

**单分支开发模式**：所有开发工作直接在 main 分支进行。

```
main                    # 唯一的长期分支，所有开发直接在此进行
└── hotfix/xxx          # 临时分支：仅在需要紧急修复时创建，完成后立即合并删除
```

**原则**：
- 日常开发直接在 main 分支提交
- 仅在需要隔离测试或紧急修复时临时创建分支
- 临时分支完成后立即合并并删除
- 避免长期存在的 feature 分支导致配置不同步

### Key Decisions Summary

| 决策点 | 推荐方案 |
|--------|----------|
| 代码组织 | Monorepo |
| Rust 核心 | Workspace 成员，feature flags 区分平台 |
| FFI 绑定 | macOS: UniFFI, Windows: csbindgen |
| CI/CD | 按路径触发，平台独立构建 |
| 版本管理 | 单一 VERSION 文件 |
| 本地化 | JSON 主文件 → 转换脚本 → 平台格式 |

---

## Quick Commands

```bash
# Rust Core
cd core && cargo build           # Build
cd core && cargo test            # Test
cd core && cargo build --release # Release build

# macOS
cd platforms/macos && xcodegen generate
open Aether.xcodeproj

# Build scripts
./scripts/build-core.sh macos    # Build core for macOS
./scripts/build-macos.sh release # Full macOS build
```

See [docs/BUILD_COMMANDS.md](./docs/BUILD_COMMANDS.md) for complete build reference.

---

## Key Architecture Components

| Component | Description |
|-----------|-------------|
| **Agentic Loop** | 8 components: IntentAnalyzer, TaskPlanner, ToolExecutor, etc. |
| **rig-core** | AI provider abstraction (OpenAI, Anthropic, Gemini) |
| **Dual-Layer Memory** | Raw history + AI-extracted facts |
| **Cowork** | DAG task orchestration with model routing |
| **Runtime Managers** | Auto-install uv, fnm, yt-dlp |
| **MCP** | Model Context Protocol (stdio transport) |

See individual docs: [ARCHITECTURE](./docs/ARCHITECTURE.md), [DISPATCHER](./docs/DISPATCHER.md), [COWORK](./docs/COWORK.md)

---

## Key Constraints (Brief)

- **macOS Entry**: Use `main.swift` + `NSApplicationMain()` (not SwiftUI @main)
- **Settings Window**: Use `NSPanel` (not NSWindow)
- **Halo Window**: Never call `makeKeyAndOrderFront()` - zero focus theft
- **Business Logic**: Rust core only, Swift is UI layer
- **FFI**: Use UniFFI, never manual bindings

See [docs/DESIGN_CONSTRAINTS.md](./docs/DESIGN_CONSTRAINTS.md) for full constraints and anti-patterns.

---

## Documentation Index

| Category | Documents |
|----------|-----------|
| **Architecture** | [ARCHITECTURE](./docs/ARCHITECTURE.md), [DISPATCHER](./docs/DISPATCHER.md), [COWORK](./docs/COWORK.md) |
| **Configuration** | [CONFIGURATION](./docs/CONFIGURATION.md), [PERMISSIONS](./docs/PERMISSIONS.md) |
| **Development** | [BUILD_COMMANDS](./docs/BUILD_COMMANDS.md), [DIRECTORY_STRUCTURE](./docs/DIRECTORY_STRUCTURE.md) |
| **Platform** | [PLATFORM_NOTES](./docs/PLATFORM_NOTES.md), [MACOS26_WINDOW_DESIGN](./docs/MACOS26_WINDOW_DESIGN.md) |
| **Testing** | [TESTING_GUIDE](./docs/TESTING_GUIDE.md), [manual-testing-checklist](./docs/manual-testing-checklist.md) |
| **Design** | [ui-design-guide](./docs/ui-design-guide.md), [DESIGN_CONSTRAINTS](./docs/DESIGN_CONSTRAINTS.md) |

---

## Skills

Use skills from: `~/.claude/skills/build-macos-apps`

---

## Environment

```bash
# Python
~/.uv/python3/bin/python
source ~/.uv/python3/bin/activate
cd ~/.uv/python3 && uv pip install <package>

# Xcode
cd platforms/macos && xcodegen generate

# Syntax validation
~/.uv/python3/bin/python Scripts/verify_swift_syntax.py <file.swift>
```

---

## Git Commit

After completing a task or fixing an issue, use `git add` and `git commit` to submit this modification use English.

---

## Memory Prompt

When the token is low to 10% of the limit, summarize this session at the end of the session to generate a "memory prompt" that can be directly copied and used, so that the next session can be inherited.

---

## Language

- Reply language in Chinese
- Program comments in English
