# Review Summary — Batch 4

**Date**: 2026-07-21
**Modules reviewed**: 6 (`src/routing`, `src/runtimes`, `src/sandbox`, `src/search`, `src/secrets`, `src/security`)
**Reviewer**: AI static review (rust-logic-audit skill, 5-phase method)
**Subagents**: 6 parallel reviews

## Module Totals

| Module           | Files | Critical | Warning | Suggested Test |
|------------------|------:|---------:|--------:|---------------:|
| routing+runtimes |    17 |        1 |       9 |              5 |
| search+secrets   |    30 |        1 |       8 |              5 |
| security         |    19 |        2 |      23 |             10 |
| sandbox core     |    18 |        4 |      16 |              7 |
| sandbox approval |    15 |        0 |       7 |              8 |
| sandbox network  |    24 |        0 |       5 |              7 |
| **TOTAL**        |   123 |    **8** |     **68** |          **42** |

## Top Priorities (Critical)

1. **runtimes/bootstrap.rs:200-282** — critical — `enrich_path_for_reprobe` 并发调用下 read-modify-write 丢失 prepended 路径
2. **secrets/mod.rs:43 vs secrets/placeholder.rs:42-44** — critical — secret-name charset mismatch: validator 允许 `:`,parser 拒绝 → 路径不可达
3. **security/ssrf/mod.rs:101-106 + media_send/understand** — critical — 同步 `validate_url` 不做 DNS 解析,允许 hostname→私网 IP 的 SSRF 旁路
4. **security/runtime_guard.rs:11,13** — critical — `runtime_guard.rs` 锁类型不一致:std sync `RwLock` 与 tokio async `Mutex` 混用,future-proof foot-gun
5. **sandbox/command_policy/mod.rs:228-245** — critical — 中间窗口扫描漏检: ≥ 2*MAX_SCAN_BYTES + 1 字节 payload 中段被跳过
6. **sandbox/capabilities.rs:130-142** — critical — `is_within` 在子路径不存在时静默回退到词法归一化,symlink 逃逸空窗
7. **sandbox/worktree.rs:294-311** — critical — `WorktreeSandbox` timeout 丢弃 `partial_stdout/stderr`
8. **sandbox/policy.rs:82-94** — critical — `fs_read + fs_write` 同时存在时 `fs_read` 被静默丢弃,跨驱动语义不一致

## Cross-module Themes

- **Wiring Completeness violations**: 多个 `pub fn` / `pub mod` 无生产 caller (`ContextIdHasher`, `sanitize_label`, `safe_regex`, `RuntimeCapability`, `outcome_from_session_completed`)
- **Sync Primitives Rule violations**: `routing/*` 全部用 `std::sync::Arc` 而非 `crate::sync_primitives::Arc`
- **Audit pipeline gaps**: `wrap_external_content_with_report` patterns 不被生产路径消费,3 个 `AuditEventType` variants 死代码
- **Fail-closed vs fail-open 漂移**: 部分路径依赖 OS sandbox 兜底,policy 层 fail-open 是可接受的纵深防御

详细报告见各 subagent 输出(本目录 batch4-routing-runtimes.md 等)