# Aggregate Review — 6 Modules (2026-XX-XX)

**Source commits**: all branches forked from `bb708bdfc` (main HEAD)
**Review agents**: 6 (parallel)
**Worktrees**: `.worktrees/review-{wizard,workflow,desktop,interfaces,shared,mobile}/`
**Per-module reports**: `review-notes.md` in each worktree

## Headline counts

| Module | P0 | P1 | P2 | Hard redline |
|--------|----|----|----|--------------|
| src/wizard | 0 | 2 | 5 | — |
| src/workflow | 0 | 4 | 3 | R8 gray (lexer) |
| desktop | 0 | 1 | 1 | — |
| interfaces | 0 | 0 | 0 | — |
| shared | 0 | 2 | 3 | — |
| mobile | 0 | 3 | 4 | — |
| **total** | **0** | **12** | **16** | **0 hard, 1 gray** |

## Cross-module findings (deferred or contained)

1. **wizard::onboarding no-op** — `flows/onboarding.rs:271-287` silently drops user-collected `OnboardingData`; persistence is unimplemented by acknowledged "KNOWN GAP (deferred)" comment. **Cannot fix inside wizard module alone**; needs gateway contract change. **Defer** with explicit note in CHANGELOG.
2. **shared::58-enum non_exhaustive gap** — All public enums in `shared/protocol/` lack `#[non_exhaustive]`. Adding it is a 1-line attribute per enum but breaks every exhaustive `match` site across `alephcore`, `aleph-cli`, `aleph-tui`, `aleph-panel`, `shared-ui-logic`. **Single-pass audit would touch 50+ match arms in crate dependencies.** Recommend a separate, dedicated change with a `try_match!` migration plan.

## Fix plan (worktree order)

### worktree: `review/mobile` (smallest, isolated Swift)
- **P1** `ReachabilityProbe.swift:141-149` — restrict `AcceptAnyServerTrust` to LAN/local-network hosts
- **P1** `AppState.swift:139-141` / `ConnectionStore.swift:36-43` — surface Keychain save failure to user
- **P1** `PanelWebView.swift:177-186` — document SAN-not-shown limitation in SECURITY.md

### worktree: `review/desktop`
- **P1** `desktop/macos/bridge/Sources/AlephBridge/RPC/Handlers.swift:17` — fix comment that mis-describes IPC auth (real model is OS-private stdin/stdout pipe + setpgid + ParentWatch)

### worktree: `review/workflow`
- **P1** `store.rs:128-176` `save_at` — document concurrent-save lost-update risk; add opt-in lockfile path
- **P1** `import/mod.rs` `parse_workflow_js` — add MAX_IMPORT_BYTES limit (default 16 MiB) to prevent DoS via huge file (this also caps `parallel_watch` unbounded growth)
- **P1** `import/mod.rs:60-69` — improve parse-error hint when string literal contains `*/`

### worktree: `review/wizard`
- **P1** `prompter.rs:140-166` + `session.rs:281-296` — `RpcPrompter::prompt` timeout race vs `answer` lock contention — fix by always attempting `rx.try_recv()` before declaring timeout
- Onboarding no-op: **defer** (cross-module)

### worktree: `review/shared`
- **P1** `providers/wire.rs:198` `ProviderInfo.api_key` — add `#[serde(skip_serializing)]` to prevent credential leak (least-disruptive fix; preserves API surface, blocks serialization)
- 58-enum non_exhaustive: **defer** to dedicated change

### worktree: `review/interfaces`
- No findings. No changes. Branch merges empty commit + report reference.

## Post-merge verification

Per-crate cargo check (each worktree, before merge):
- `cargo check -p alephcore --lib` (covers wizard, shared/../alephcore integrations)
- `cargo check -p aleph-protocol` (covers shared fix)
- `cargo check -p aleph-desktop-macos` (covers desktop)
- `cargo check -p aleph-panel --lib` (interfaces, no changes — sanity)
- Mobile: Swift (no cargo check possible on Linux)

After all merges:
- `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo check --workspace` (memory-limited)

## State-of-negative (explicit non-actions)

- **Not** running `cargo test`. Compile-only verification per AGENTS.md "无需cargo check" plan (we override to per-crate check, then workspace).
- **Not** adding `#[non_exhaustive]` to 58 shared/protocol enums (deferred — needs crate-spanning migration).
- **Not** fixing `wizard::onboarding` no-op (cross-module, needs gateway contract change).
- **Not** running `cargo clippy --workspace --all-targets` (would OOM at 11GB available).
- **Not** building mobile (no Xcode on Linux; fixes reviewed statically).