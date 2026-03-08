# Docs Refresh Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rewrite README.md (EN), create README_CN.md (CN), restructure CLAUDE.md to pure coding rules, create LICENSE file.

**Architecture:** Four independent documentation files. README.md is the canonical English version; README_CN.md mirrors it in Chinese. CLAUDE.md is slimmed from ~600 lines to ~200 lines, keeping only architectural redlines, design principles, and development rules. LICENSE is standard MIT text.

**Tech Stack:** Markdown, no code changes.

**Design doc:** `docs/plans/2026-03-08-docs-refresh-design.md`

---

### Task 1: Create LICENSE file

**Files:**
- Create: `LICENSE`

**Step 1: Write MIT license**

Create `LICENSE` with standard MIT text. Copyright holder: use the name from `Cargo.toml` repository field (`rootazero`). Year: 2026.

```
MIT License

Copyright (c) 2026 rootazero

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

**Step 2: Commit**

```bash
git add LICENSE
git commit -m "docs: add MIT license file"
```

---

### Task 2: Rewrite README.md (English)

**Files:**
- Modify: `README.md` (complete rewrite)

**Step 1: Write the new README.md**

Key facts to use (verified from codebase):
- Start command: `cargo run --bin aleph`
- Config path: `~/.aleph/`
- macOS native app: `apps/macos-native/`
- Builtin tools: 30+
- Gateway interfaces: 15 (Telegram, Discord, Slack, WhatsApp, IRC, Matrix, Signal, Nostr, Email, Webhook, iMessage, XMPP, Mattermost, CLI)
- Workspace members: core, crates/desktop, crates/logging, shared/protocol, shared/ui_logic, apps/cli, apps/panel, apps/desktop/src-tauri, apps/webchat
- Rust MSRV: 1.92
- Repo: `https://github.com/rootazero/Aleph`
- Panel UI: Leptos/WASM (`apps/panel/`)
- Build tool: `just` (see `justfile`)
- No feature flags needed for production builds

Structure (in order):

```markdown
# Aleph (ℵ)

> Self-hosted personal AI assistant — one core, many shells.

[badges: Rust 1.92+, MIT License, Platform macOS|Linux|Windows]
[Link to README_CN.md: 中文文档]

## What is Aleph?

3-4 sentences:
- Self-hosted personal AI assistant running entirely on your devices
- Connects through a unified Gateway to 15+ messaging channels
- Rust core with Observe-Think-Act-Feedback agent loop
- Native macOS app, cross-platform Tauri app, CLI, web panel

## Architecture

Updated 5-layer ASCII diagram matching actual codebase:
- Interface Layer: macOS Native | Tauri | CLI | Panel (WASM) | WebChat | Telegram | Discord | ...
- Gateway Layer: Router | Session Manager | Event Bus | Channel Registry | Hot Reload
- Agent Layer: Agent Loop | Thinker | Dispatcher | Task Planner | Compressor
- Execution Layer: Providers | Executor | Tool Server | MCP | Extensions | Exec
- Storage Layer: Memory (LanceDB) | State (SQLite) | Config

Link to docs/reference/ARCHITECTURE.md

## Features

### Core
- Multi-provider LLM support (Claude, GPT-4, Gemini, DeepSeek, Ollama, Moonshot)
- 15+ messaging channel interfaces
- 30+ built-in tools
- Memory system with hybrid search (vector ANN + full-text)
- MCP protocol support for external tool integration
- POE (Principle-Operation-Evaluation) agent architecture

### Developer Experience
- Hot reload for configuration changes
- Plugin system (WASM + Node.js)
- `just` build pipeline with one-command workflows
- 58+ Gateway JSON-RPC handlers
- JSON Schema auto-generation (schemars)

## Getting Started

### Prerequisites
- Rust 1.92+ (install via rustup.rs)
- just (install via `cargo install just`)
- For WASM panel: wasm-bindgen-cli, npm
- For macOS native: Xcode, XcodeGen

### Quick Start
git clone https://github.com/rootazero/Aleph.git
cd Aleph
cargo run --bin aleph

### Configuration
~/.aleph/
├── config.toml       # Main configuration
├── providers.toml    # AI provider credentials  (VERIFY THIS EXISTS)
├── logs/             # Server logs
└── ...

## Building

| Command | Description |
|---------|-------------|
| `just dev` | Run server (debug, rebuilds WASM) |
| `just build` | Build server (release) |
| `just test` | Run core tests |
| `just wasm` | Build WASM Panel UI |
| `just macos` | Build macOS native app (release) |
| `just check` | Quick compile check |
| `just clippy` | Lint with clippy |
| `just deps` | Verify build dependencies |

See `just --list` for all recipes.

## Project Structure

aleph/
├── core/                    # Rust Core (alephcore crate)
│   └── src/
│       ├── gateway/         # WebSocket control plane + channel interfaces
│       ├── agent_loop/      # Observe-Think-Act-Feedback loop
│       ├── thinker/         # LLM interaction layer
│       ├── dispatcher/      # Task orchestration (DAG scheduling)
│       ├── executor/        # Tool execution engine
│       ├── builtin_tools/   # 30+ built-in tools
│       ├── memory/          # LanceDB hybrid search
│       ├── providers/       # Multi-LLM provider adapters
│       ├── extension/       # WASM + Node.js plugin runtime
│       ├── intent/          # Intent detection pipeline
│       ├── exec/            # Shell execution security
│       ├── mcp/             # MCP protocol client
│       └── ...
├── crates/
│   ├── desktop/             # Desktop capability traits + native impl
│   └── logging/             # Structured logging
├── apps/
│   ├── cli/                 # Command-line interface
│   ├── macos-native/        # Native Swift/Xcode app
│   ├── desktop/             # Cross-platform Tauri app
│   ├── panel/               # Leptos/WASM control panel
│   └── webchat/             # Web chat UI (React)
├── shared/                  # Shared protocol + UI logic crates
├── docs/reference/          # Architecture & system documentation
├── justfile                 # Build pipeline recipes
└── Cargo.toml               # Workspace configuration

## Documentation

| Document | Topic |
|----------|-------|
| [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) | System architecture |
| [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) | Agent loop, thinker, dispatcher |
| [GATEWAY.md](docs/reference/GATEWAY.md) | WebSocket protocol, RPC methods |
| [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) | Tool development |
| [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) | Memory and search |
| [EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) | Plugin development |
| [SECURITY.md](docs/reference/SECURITY.md) | Execution security |
| [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) | DDD domain modeling |
| [SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) | Server dev & deployment |

## Contributing

Single-branch development on `main`. Commit format: `<scope>: <description>` (English).

Example: `gateway: add WebSocket reconnection logic`

## License

MIT License. See [LICENSE](LICENSE).

## Acknowledgments

- Ghost in the Shell — the vision of human-AI symbiosis
- Jorge Luis Borges — the Aleph metaphor
```

**Step 2: Verify links**

Check that all `docs/reference/*.md` links resolve. All 12 files confirmed to exist during exploration phase.

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: rewrite README.md to match codebase reality"
```

---

### Task 3: Create README_CN.md (Chinese)

**Files:**
- Create: `README_CN.md`

**Step 1: Translate README.md to Chinese**

Full Chinese translation of the README.md created in Task 2. Key differences:
- Title line: same (`# Aleph (ℵ)`)
- Subtitle: `> 自托管个人 AI 助手 — 一核多端。`
- Language link points back to README.md: `[English](README.md)`
- All section headings in Chinese
- Technical terms keep English (Rust, WASM, WebSocket, JSON-RPC, etc.)
- Commands and code blocks stay in English

**Step 2: Add cross-links**

- In `README.md` header area, add: `[中文文档](README_CN.md)`
- In `README_CN.md` header area, add: `[English](README.md)`

**Step 3: Commit**

```bash
git add README_CN.md README.md
git commit -m "docs: add Chinese README and cross-language links"
```

---

### Task 4: Restructure CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (restructure, ~600 → ~200 lines)

**Step 1: Read current CLAUDE.md fully**

Read the complete file to identify exact line ranges for each section to keep/remove.

**Step 2: Write the new CLAUDE.md**

Sections to KEEP (verbatim or lightly edited):
- OpenSpec block at top (lines ~1-12 of current file)
- Architecture Redlines R1-R7 (currently under "🛑 架构红线")
- Design Principles P1-P7 (currently under "🧬 软件设计原则")
- Git Worktree caveats (condensed version)
- Commit convention, branch strategy, language convention

Sections to REMOVE:
- Opening quote ("这是人类历史上第一次...")
- 核心哲学 section (五层涌现, POE, DDD)
- 1-2-3-4 架构模型 complete description
- 核心子系统 table
- 技术栈 table
- 项目结构 directory tree
- 北极星 section
- Detailed Environment section

Sections to ADD/UPDATE:
- Build commands table (corrected, matching justfile)
- Feature flags (loom, test-helpers only — no mention of removed flags)
- Documentation index (name + link only, no descriptions)
- Session context (condensed to 2-3 lines + memory prompt rule)

Target structure:

```markdown
# CLAUDE.md

<!-- OPENSPEC:START -->
[keep as-is]
<!-- OPENSPEC:END -->

## 🛑 架构红线 (Architectural Redlines)

[R1-R7 verbatim from current file]

## 🧬 设计原则 (Design Principles)

[P1-P7 verbatim from current file]

## 🔧 开发指南

### 构建命令

| Command | Description |
|---------|-------------|
| `cargo run --bin aleph` | Start server (debug) |
| `cargo check -p alephcore` | Quick compile check |
| `cargo test -p alephcore --lib` | Run core tests |
| `just dev` | Dev server (rebuilds WASM first) |
| `just build` | Release build (WASM + server) |
| `just test-all` | All tests (core + desktop + proptest) |
| `just clippy` | Lint |

### Feature Flags

Production: all features always compiled, no flags needed.
Test-only: `loom` (concurrency testing), `test-helpers` (integration test utilities).

### 提交规范

English commit messages. Format: `<scope>: <description>`
Example: `gateway: add WebSocket server foundation`

### 分支策略

Single-branch development on `main`.

### 语言规范

- Reply in Chinese
- Code comments in English
- Documentation in both

### Git Worktree 注意事项

`EnterWorktree` locks CWD to worktree directory. Cannot delete worktree from same session.
Use `(cd /main/repo && commands)` subshell or clean up in a new session.

## 📚 文档索引

| Document | Link |
|----------|------|
| Architecture | [ARCHITECTURE.md](docs/reference/ARCHITECTURE.md) |
| Agent System | [AGENT_SYSTEM.md](docs/reference/AGENT_SYSTEM.md) |
| Gateway | [GATEWAY.md](docs/reference/GATEWAY.md) |
| Tool System | [TOOL_SYSTEM.md](docs/reference/TOOL_SYSTEM.md) |
| Memory System | [MEMORY_SYSTEM.md](docs/reference/MEMORY_SYSTEM.md) |
| Extension System | [EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) |
| Security | [SECURITY.md](docs/reference/SECURITY.md) |
| Design Patterns | [DESIGN_PATTERNS.md](docs/reference/DESIGN_PATTERNS.md) |
| Code Organization | [CODE_ORGANIZATION.md](docs/reference/CODE_ORGANIZATION.md) |
| Domain Modeling | [DOMAIN_MODELING.md](docs/reference/DOMAIN_MODELING.md) |
| Agent Design Philosophy | [AGENT_DESIGN_PHILOSOPHY.md](docs/reference/AGENT_DESIGN_PHILOSOPHY.md) |
| Server Development | [SERVER_DEVELOPMENT.md](docs/reference/SERVER_DEVELOPMENT.md) |

## 📝 Session Context

- **项目**: 自托管个人 AI 助手，Rust Core + 多端架构
- **核心循环**: Observe → Think → Act → Feedback → Compress
- **语言**: 使用中文对话

### Memory Prompt

When token is low to 10%, summarize this session to generate a "memory prompt" for next session inheritance.
```

**Step 3: Verify line count**

Run `wc -l CLAUDE.md` — target is ~200 lines (acceptable range: 180-250).

**Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: restructure CLAUDE.md to pure coding rules (~200 lines)"
```

---

### Task 5: Final verification

**Step 1: Verify all files**

```bash
# Check files exist
ls -la README.md README_CN.md CLAUDE.md LICENSE

# Check line counts
wc -l README.md README_CN.md CLAUDE.md

# Check cross-links work
grep -n "README_CN.md" README.md
grep -n "README.md" README_CN.md
grep -n "LICENSE" README.md README_CN.md
```

**Step 2: Verify no broken doc links**

```bash
# All referenced docs should exist
for f in ARCHITECTURE AGENT_SYSTEM GATEWAY TOOL_SYSTEM MEMORY_SYSTEM EXTENSION_SYSTEM SECURITY DESIGN_PATTERNS CODE_ORGANIZATION DOMAIN_MODELING AGENT_DESIGN_PHILOSOPHY SERVER_DEVELOPMENT; do
  ls docs/reference/${f}.md
done
```

**Step 3: Read through each file once**

Skim-read all 4 files to catch typos, broken formatting, or inconsistencies.
