# AGENTS.md — Aleph Development Guide

> Instructions for AI agents. Read before touching code.

---

## Build & Test

```bash
cargo check -p alephcore              # Fast compile
cargo clippy -p alephcore -- -D warnings  # Lint
cargo test -p alephcore --lib          # Unit tests
cargo test -p alephcore --lib test_name  # Single test
just test-all                          # All tests (core + desktop + proptest)
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
| **R4** | Interface layers (App/Bot/CLI) are pure I/O — no business logic |
| **R8** | LLM handles intent/routing. Regex only for machine formats (JSON, URLs) |
| **R9** | All configurability exposed as tools — natural language drives everything |

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
Multiple processes → HMAC failure → **vault data loss**.

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

*Last updated: 2026-04-04*
