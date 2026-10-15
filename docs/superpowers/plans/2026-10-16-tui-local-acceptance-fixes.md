# Local terminal acceptance fixes / 本地终端验收修复

Date: 2026-10-16. Branch: `feat/tui-streaming-refactor`.

## Changes / 修复

- Remove the Chat rectangle and title. Header, transcript, scrolling and mouse regions use the full chat viewport; no residual one-cell border offset. Keep the narrow one-line latest-user chip and its measured jump target.
- Draw the composer between two thin horizontal rules, without corners or side borders. Empty input occupies three rows; multiline input remains capped.
- The failed run's final `RunError` now takes its sequence from the existing event emitter. The independent active-run bookkeeping counter could place it before an already emitted `RunComplete`, causing clients to discard the error as old.
- A TUI error receipt for a known, settled run stays visible without re-adopting that run and scheduling history replacement which would erase the receipt.
- Security-store v14 migration probes each `user_id` column before ALTER. A partially completed old upgrade must reopen the persisted key store, preserving its vault master key, rather than falling back to an unrelated in-memory key store. Existing encrypted provider credentials are retained.

中文：无回复问题包含凭据加载失败与错误回执被吞掉两层。修复保留现有保险库和主密钥，不要求先更换 MiniMax key；运行前已备份加密保险库和一致性 SQLite 快照。

## Verification / 验证

- `cargo test -p alephcore --lib v14_retry_preserves_master_token_when_identity_columns_already_exist -- --test-threads=1`: 1 passed. Fixture is memory SQLite with a synthetic master key; no real credentials used.
- `cargo test -p aleph-tui --lib -- --test-threads=1`: 546 passed, 1 existing live-Gateway test ignored. Tests cover borderless viewport coordinates, composer rules, sticky jump, and visible post-completion failures.
- Each Cargo command remains serialized behind the available-physical-memory guard, with one build job and dev debug information disabled.
- Runtime build, redeployment and a real model-reply probe are tracked separately below; compile success alone does not establish provider connectivity.

## Runtime acceptance / 运行验收

Pending deployment and real reply check. Manual appearance, mouse and opacity inspection remain the user's terminal acceptance.
