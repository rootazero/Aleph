# AGENTS.md — Aleph Development Guide

> Instructions for AI agents. Read before touching code.

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

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

*Last updated: 2026-04-22*
