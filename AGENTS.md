# AGENTS.md — Aleph Development Guide

1. Ask > Assume: Never make silent assumptions about intent or architecture. If unattended, pick the most reasonable path and explicitly log the assumption before proceeding.

2. Trade-offs > Blind Simplicity: Before writing code, state your approach and explicitly call out what this specific design makes harder down the line. Avoid naive solutions that paint us into an architectural corner.

3. Pragmatic Scope: Stay on task, but execute or propose sensible refactorings/abstractions that prevent tech debt. Always surface bad code or design smells for separate discussion.

4. Flag Uncertainty: Confidence without certainty causes damage. Never guess. If unsure, propose a small, localized, low-risk experiment to validate the hypothesis first.

5. High-Signal Pushback: Challenge my ideas only if they introduce significant architectural risk, waste effort, or violate settled industry practices. Offer a forward-thinking, correct alternative. Ignore minor stylistic preferences.

6. State the Negative: End every task completion by explicitly listing what you did not do, unhandled edge cases, or skipped validations.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

---

## Language

使用中文进行对话

---

## Build & Test

```bash
cargo check -p alephcore              # Fast compile
cargo clippy -p alephcore -- -D warnings  # Lint
cargo test -p alephcore --lib          # Unit tests
cargo test -p alephcore --lib test_name  # Single test
just test-all                          # All tests (core + desktop + proptest)
```

**内存受限机器（<16GB）**：alephcore lib test 单 rustc 进程可吃 8GB+（已被 OOM killer 击杀过，报错伪装成「could not compile; N warnings emitted」）。限流编译：

```bash
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p alephcore --lib
```

---

## Code Style

**rustfmt** (4-space indent, 100 char width) + **clippy** (`-D warnings`).

| Item | Convention |
|------|-----------|
| Modules/functions/variables | `snake_case` |
| Types/traits/enums | `PascalCase` |
| Constants | `SCREAMING_SNAKE_CASE` |
| Visibility | Default private, `pub(crate)` for internal sharing |

**Error handling**: Libraries use `thiserror`; applications use `anyhow`. Use `?` for propagation. Never `unwrap()` in production.

**Immutability**: Variables are immutable by default. Use `let mut` only when required.

---

## Architecture Redlines

| Rule | Description |
|------|-------------|
| **R1** | Core never calls platform APIs (AppKit, Vision, CoreGraphics). Core defines trait contracts; platform impl via IPC |
| **R2** | Complex business UI in Leptos/WASM only. Native shells = window container + animations |
| **R3** | Core minimalism — no heavy deps for non-core features; implement as Skill/MCP |
| **R4** | Interface layers (App/Bot/CLI) are pure I/O — no business logic |
| **R5** | Menu bar first, window on demand — lightweight entry + expand when needed |
| **R6** | AI comes to you — minimize context switching; Halo, notifications, inline |
| **R7** | One core, many shells — Rust Core is the only brain |
| **R8** | LLM handles intent/routing. Regex only for machine formats (JSON, URLs) |
| **R9** | All configurability exposed as tools — natural language drives everything |
| **R10** | Intelligence lives in the prompt — zero middleware tax |

---

## Commit Format

`<scope>: <description>` (English). Example: `gateway: add WebSocket server foundation`

---

## Process Management (CRITICAL)

**Before restarting Aleph:**
```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
```
Multiple processes → HMAC failure → **vault data loss**. Runtime sentinel: doctor's `core/duplicate-instance` check (`src/diagnostics/checks/duplicate_instance.rs`) warns when other live `aleph-server` processes exist.

---

## Version

- **CalVer**: `YYYY.MM.DD`
- **VERSION file** is the only source. Use `env!("ALEPH_VERSION")` in code.
- Release: write CHANGELOG → `just release YYYY.MM.DD`

---

## Key References

| Document | Purpose |
|----------|---------|
| `CLAUDE.md` | Architecture redlines, design principles |
| `justfile` | All build commands |
| `docs/reference/ARCHITECTURE.md` | Full architecture |
| `docs/reference/CODE_ORGANIZATION.md` | Module/file patterns |

---

## Workspace Members

```
desktop/shared       # DesktopCapability trait + IPC
desktop/macos        # macOS native implementation
desktop/linux        # Linux native implementation
desktop/windows      # Windows native implementation
shared/logging       # Logging infrastructure
shared/protocol      # Shared protocol types
shared/ui_logic      # Shared UI logic
shared/client        # Shared client utilities
interfaces/cli       # CLI client
interfaces/tui       # TUI client
interfaces/webchat   # Web chat interface
```

---

## 工具

搜索用 grep/find 内置工具，多 OR 词用一次 multi_grep，必须走 bash 时用 rg
定位后用 read 的 offset/limit 只读命中附近，工作区外已知文件直接 read

---

*Last updated: 2026-04-22*
