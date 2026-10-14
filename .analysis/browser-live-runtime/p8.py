from ed import apply
p='D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256/docs/reports/verify-20261007/browser-live-task48.md'
apply(p,[('测试后仅对 `manager.rs`/`process.rs` 做了 rustfmt（纯格式）。','测试后对 `manager.rs`/`process.rs` 做了 rustfmt（纯格式），并重跑同一过滤器：`task48-hardening-test2.log` 同为 491 passed / 0 failed（check/clippy 未在 fmt 后重跑）。'),
('rustfmt (format only) was applied to `manager.rs`/`process.rs` after that test run.','rustfmt (format only) was applied to `manager.rs`/`process.rs` after that run and the same filter re-run: `task48-hardening-test2.log` also 491 passed / 0 failed (check/clippy not re-run after fmt).')])
