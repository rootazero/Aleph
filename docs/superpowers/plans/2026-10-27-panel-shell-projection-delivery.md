# Panel Shell Projection Delivery / Panel Shell Projection 阶段交付记录

日期：2026-10-27
分支：`panel-shell`
基线：`34a3c63bbb735a112529d73dfbf0ed968cadfa3d` (`test(panel): add terminal host acceptance fixtures`)

## Intent / 意图

本阶段把已通过 production host/wire acceptance 的 Terminal 一次性 host 特例收口为可复用的 host contract：host-owned descriptor projection 初始化 registry，route 选择 active key，`PanelLoader` 负责 module lifecycle。它为下一阶段选择第二个 built-in Tab 提供稳定边界，但不迁移第二个真实 Tab。

This stage generalizes the accepted Terminal host into a reusable host contract: a host-owned descriptor projection initializes the registry, route selection chooses the active key, and `PanelLoader` owns module lifecycle. It prepares the boundary for a later second built-in Tab migration without migrating that Tab here.

## Delivered / 已交付

- Added `interfaces/webchat/src/panel/projection.rs::PanelProjection` as the host-owned built-in descriptor projection entry.
- Moved the Terminal descriptor construction out of `interfaces/webchat/src/panel/shell.rs`; feature-specific pilot/production entry, API version, trust, permissions, status, and validation inputs remain explicit.
- Kept `TabRegistry` as the host projection registry and made it cloneable for Shell ownership without introducing a second authority.
- Changed `PanelShell` to consume a projection and route-selected `initial_tab`; shell identity, active tab, nav row, load/error/retry surface, and mount target are derived from registry/active state rather than feature flags. `PanelProjection::select_route` returns the generic `Legacy` / `Modular` / `Unavailable` route state, so `app.rs` has no per-Tab modular route branch.
- Changed `interfaces/webchat/src/app.rs` so a malformed built-in projection cannot silently mount legacy Terminal on `/terminal`; the route renders an unavailable boundary, while unknown routes still use the legacy host.
- Preserved loader teardown semantics: UI resources and module handles are released, while `pty.close` remains an explicit user operation only.
- Added projection tests for the complete Terminal descriptor contract, canonical route selection, trailing-slash normalization, unknown route fail-closed behavior, duplicate/invalid-entry/API mismatch rejection, and disabled/incompatible/failed descriptor fail-closed behavior. Existing registry/loader tests continue to cover ordering, duplicates, shadowing, replacement, active removal fallback, one active mount, and cleanup semantics.

## Scope and trade-offs / 范围与取舍

The projection currently contains only the host-owned Terminal descriptor. A future plugin projection can be added at this host boundary, but this change does not create a community installation, trust, update, uninstall, or browser-owned registry path. `panel-modular` remains opt-in and the legacy Terminal wiring remains the rollback path.

本阶段没有修改 `src/harness/`、Capability/ZahirFacade runtime、PTY server business logic、community lifecycle、其他 built-in Tab、默认 release、发布资产或 live Gateway 接入。真实 Gateway/live parity remains BLOCKED because no authorized disposable Gateway/project/session/credential was available; the production host fixture is explicitly loopback evidence.

## Validation / 验证

All cargo commands were preceded by `python qa/browser_live/check_memory.py` from the worktree root, and cargo was run from `interfaces/webchat` serially:

- `cargo fmt -- --check` — pass.
- `cargo test --lib --no-default-features --features panel-modular panel:: -- --nocapture` — 49 passed, 0 failed.
- `cargo test --lib --no-default-features --features panel-tab-spike panel:: -- --nocapture` — 49 passed, 0 failed.
- `cargo clippy --lib --no-default-features --features panel-modular --all-targets -- -D warnings -A dead_code -A unused_imports` — pass.
- `cargo clippy --lib --no-default-features --features panel-tab-spike --all-targets -- -D warnings -A dead_code -A unused_imports` — pass.
- `node --test tests/panel-build-memory-gate.test.mjs tests/panel-terminal-host.test.mjs tests/panel-terminal-protocol.test.mjs` — 3 passed, 0 failed.
  - `production_host_fixture: passed` against the loopback WebSocket/HTTP Gateway-shaped fixture.
  - `protocol_fixture: passed` for `/` and `/panel/` bases, with the existing PTY protocol and lifecycle assertions.
  - `memory_gate_rejection: passed`.

The staged `.panel-tabs/` assets are ignored build output. The worktree-only Playwright package symlink used for the browser run is also ignored state; no package manifest, lockfile, or tracked dependency changed.

## Deferred / 未完成

- Second real built-in Tab migration.
- Real Aleph Gateway/live parity and PTY/session interaction against a disposable authorized target.
- Default `panel-modular` release flip or legacy wiring removal.
- Community UI installation/trust/integrity/update/uninstall lifecycle.
- Mobile/tablet parity, full accessibility/leak/release matrix, and release rollback hardening.
- Capability/ZahirFacade runtime mount or any PTY business-logic changes.
