# Docs Refresh: README.md + CLAUDE.md Overhaul

> Date: 2026-03-08

## Goal

Align README.md and CLAUDE.md with current codebase reality. Make README professional and GitHub-standard. Slim CLAUDE.md to pure coding rules.

## Decisions

| Decision | Choice |
|----------|--------|
| README audience | Users + contributors, user-first |
| Philosophy section | Remove entirely |
| Language | Bilingual: `README.md` (EN) + `README_CN.md` (CN), interlinked |
| CLAUDE.md scope | Pure AI coding guide — rules only, no architecture descriptions |
| Roadmap | Remove |
| LICENSE file | Create (MIT) |

## Factual Corrections Needed

| Item | Current (wrong) | Actual |
|------|-----------------|--------|
| Start command | `cargo run -p alephcore --features gateway --bin aleph-gateway -- start` | `cargo run --bin aleph` |
| macOS app path | `apps/macos/` | `apps/macos-native/` |
| Desktop description | "Tauri + React" | "Tauri" (UI is Leptos/WASM in `apps/panel/`) |
| Built-in tools count | "19+" | 33 files |
| Gateway interfaces | Telegram, Discord only | 15 interfaces (Telegram, Discord, Slack, WhatsApp, IRC, Matrix, Signal, etc.) |
| Config path | `~/.config/aleph/` | `~/.aleph/` |
| Gateway file count (CLAUDE.md) | "34 files" | ~60 items |
| Providers file count (CLAUDE.md) | "21 files" | ~19 items |
| Builtin tools count (CLAUDE.md) | "19 files" | 33 files |
| Project structure | Missing apps/panel, apps/webchat, crates/, shared/ | Add all |
| LICENSE reference | Points to non-existent file | Create LICENSE file |

## README.md Structure (EN)

```
# Aleph (ℵ)
> One-line description
> [badges] [language switch link]

## What is Aleph?
  3-4 sentences: positioning, core capabilities, differentiator

## Architecture
  Updated ASCII diagram (5-layer: Interface → Gateway → Agent → Execution → Storage)
  Link to docs/reference/ARCHITECTURE.md

## Features
  Core (6 bullets) + Developer Experience (5 bullets)

## Getting Started
  ### Prerequisites
  ### Quick Start (corrected commands)
  ### Configuration (~/.aleph/ structure)

## Building
  just command table (dev / build / test / wasm / macos)

## Project Structure
  Updated directory tree

## Documentation
  Reference docs link table

## Contributing
  Branch strategy + commit convention

## License
  MIT

## Acknowledgments
  2-3 items
```

## README_CN.md Structure

Mirror of README.md in Chinese. Top of each file links to the other.

## CLAUDE.md Structure (~200 lines)

```
# CLAUDE.md

## OpenSpec block (keep as-is)

## Architecture Redlines (R1-R7)
  Keep all 7 rules verbatim

## Design Principles (P1-P7)
  Keep all 7 principles verbatim

## Development Guide
  ### Build Commands (cargo / just quick reference)
  ### Feature Flags (loom, test-helpers only)
  ### Commit Convention
  ### Branch Strategy
  ### Language Convention
  ### Git Worktree Caveats (condensed)

## Documentation Index
  Name + link only, no descriptions

## Session Context
  Key context (1-2 lines)
  Memory Prompt rule
  Language: 中文对话
```

### Removed from CLAUDE.md

- 五层涌现架构 (Five Layers of Emergence) → ARCHITECTURE.md
- 1-2-3-4 架构模型 complete description → ARCHITECTURE.md
- POE 架构 / DDD 建模 → respective reference docs
- 核心子系统表 → ARCHITECTURE.md
- 技术栈表 → ARCHITECTURE.md
- 项目结构目录树 → README.md
- 北极星章节 → delete
- Opening quote and philosophy → delete

## New Files

1. `LICENSE` — MIT license text
2. `README_CN.md` — Chinese translation of README.md

## Modified Files

1. `README.md` — complete rewrite
2. `CLAUDE.md` — restructure and slim down
