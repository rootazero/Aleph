All fixes applied and verified. Here's the final report:

---

# Module: exec

## Summary
- Files reviewed: **39**
- Issues found: **18** (across 4 parallel review agents)
- Issues fixed: **13** (all high-confidence issues, confidence ≥ 80)

## Fixes Applied

### Critical Security (4 fixes)

1. **`approval/escalation.rs:45-57`** — Path scope bypass via string prefix confusion
   → 改用 `Path::starts_with`（组件级比较）替代 `str::starts_with`，防止 `/tmp-evil/` 匹配 `/tmp/*` 的绕过攻击。同时增加了空路径的显式拒绝。

2. **`sandbox/platforms/macos.rs:100-104`** — Seatbelt profile 注入（domain）
   → 增加 domain 字符白名单验证（仅允许 `[a-zA-Z0-9._-]`），非法字符直接返回错误。

3. **`sandbox/platforms/macos.rs:75-78`** — Seatbelt profile 注入（path）
   → 对 `path.display()` 中的双引号进行转义，防止路径中的 `"` 字符破坏 Seatbelt 表达式结构。

4. **`ipc.rs:204-216,284-288`** — HMAC 时序旁道攻击 + panic in async task
   → 使用 `hmac::verify_slice()`（内部常量时间比较）替代 `!=`；将 `compute_hmac` 从 `expect` 改为返回 `Result<Vec<u8>, IpcError>`。

### Important Logic/Security (9 fixes)

5. **`sandbox/executor.rs:106`** — Cleanup 错误覆盖执行结果
   → 改为 `tracing::warn` 记录 cleanup 失败，不再用 `?` 传播覆盖原始执行结果和审计日志。

6. **`sandbox/executor.rs:91`** — `timeout_secs * 1000` 溢出
   → 改用 `saturating_mul(1000)`。

7. **`approval/audit.rs:175`** — 未知 escalation reason 静默映射为 `FirstExecution`
   → 增加 `tracing::warn` 日志记录未知值，保留 fallback 行为但不再完全静默。

8. **`parser.rs:315-326`** — PATH 查找逻辑 bug
   → `.map(|_| "")` 丢弃系统 PATH 值后又重新读取。简化为单一 `unwrap_or_else` 链。

9. **`risk.rs:122-124`** — `env`/`set`/`printenv` 和 `git fetch`/`remote` 错误标记为 Safe
   → 移除 `env`/`set`/`printenv` 和 `git fetch`/`git remote`（它们会泄露环境变量或执行网络 I/O），降级为默认的 Caution。

10. **`manager.rs:180`** — `wait_for_decision` 使用完整超时而非剩余时间
    → 改为从当前时间计算剩余时间 `expires_at_ms - now_ms`，防止延迟调用时额外授予完整超时窗口。

11. **`manager.rs:491,520,531`** — 测试代码 `lock().unwrap()` 违反 P7 锁安全规范
    → 改为 `.unwrap_or_else(|e| e.into_inner())`。

12. **`sandbox/profile.rs:15`** — `TempDir::keep()` 已弃用
    → 改用 `into_path()` 正确消费 TempDir guard 并禁止自动删除。

13. **`bridge.rs:74-79`** — `split(':')` 脆弱解析
    → 改用 `split_once` + `rsplit_once`，允许 approval_id 包含冒号（future-proof）。

## Verification
- `cargo check -p alephcore --lib` — **通过**（零错误）
- `cargo test -p alephcore --lib -- exec::` — **261 tests 全部通过**

## Notes

以下为已知但未修复的低优先级问题（置信度 < 80 或设计层面需要更大重构）：

- **`sandbox/presets.rs`** — `${WORKSPACE}` 等模板变量未被展开，生成无效的 Seatbelt 规则。需要在 `generate_profile` 时增加变量替换逻辑（架构变更）。
- **`approval/binding.rs:75-85`** — 范围解析器对负数范围静默失败（`split_once('-')` 在 `-100-100` 上错误分割）。建议未来采用 `range:start:end` 格式。
- **`forwarder.rs:99`** — 正则每次调用重编译。建议在构造函数中预编译缓存。
- **`approval/escalation.rs` vs `path_canonicalize.rs`** — 两个路径检查 API 不一致（一个做 percent-decode，一个不做）。建议统一入口。
