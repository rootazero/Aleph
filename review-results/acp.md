
ib compiles clean. The bin error is pre-existing and unrelated to the ACP module.

---

# Module: acp

## Summary
- Files reviewed: 10
- Issues found: 2
- Issues fixed: 2

## Fixes

1. **session.rs:189** State machine leak in `prompt()` — `create_acp_session()` 失败时 state 留在 `Busy` 不会恢复 → 改为失败时设置 `Error` 状态并返回错误
2. **session.rs:241** State machine leak in `cancel()` — `transport.send()` 失败时 state 不会重置 → 改为失败时设置 `Error` 状态并返回错误

## Notes

- **代码质量整体较高**：无 UTF-8 字节切片、无 lock poisoning（使用 tokio RwLock）、无 SQL 注入、无 static mut、所有 unwrap 都在测试代码或有 fallback
- **已知设计权衡**：`manager.prompt()` Oneshot 路径和 `ensure_session()` 在 `harnesses.read()` 锁内 await 长时间操作（进程启动/执行）。这不是 bug（tokio RwLock 设计上支持跨 await），但会阻塞并发的 harness 注册/更新操作。若未来需要更高并发度，可考虑将 trait 方法拆为"获取配置"+"执行"两步
- **测试覆盖充分**：protocol、transport、manager、mock_server 各有完善的单元测试
