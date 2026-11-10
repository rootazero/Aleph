# FIX-IF2 — region-splice track (raw API)

Worktree: `/home/zou/data/workspace/Aleph-wt-fix-extfx`. Model: `88apiclaude/claude-sonnet-5-5`. One API call per finding.

- IF2-01: FIXED
- IF2-02: FIXED
- IF2-03: FIXED
- IF2-04: FIXED
## IF2-05 — FIXED — 在共享 `audit_guild_permissions` 入口拒绝零 ID，先于 `GuildId` 构造和 HTTP 调用；保留启动循环原有跳过逻辑，新增 `test_audit_guild_permissions_rejects_zero_before_http`。 Reject zero guild IDs at the shared audit API before `GuildId` construction or HTTP; retain startup filtering and add a focused regression test.

本轮只修改审计 API、相邻测试与本记录；不改签名或非零 ID 路径。继续保留 `u64` 接口，因此仍需入口检查，而非类型级非零保证。 Only the audit API, adjacent test, and this note changed; the signature and non-zero path are preserved. Keeping `u64` requires boundary validation rather than a type-level guarantee.

验证：静态检查与格式解析；按要求未运行 Cargo、回归测试或 Discord 真机验证；未改 git 历史，也未重审其他编号。 Validation: static checks and formatting parse only; Cargo, regression-test execution, and live Discord validation are intentionally deferred. No git history changes or re-audit of other findings.
- IF2-08: FIXED
- IF2-06: FIXED
- IF2-09: FIXED
- IF2-07: FIXED
- IF2-10: FIXED
- IF2-13: FIXED
- IF2-12: FIXED
