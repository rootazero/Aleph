

编译通过，无新增 warning。以下是审查报告：

# Module: init_unified

## Summary
- Files reviewed: 3
- Issues found: 3
- Issues fixed: 3

## Fixes

1. **`coordinator.rs:171`** Database rollback path mismatch → LanceDB 创建的是 `memory.lance` 目录，而非 `memory.db` 文件。修正路径并改用 `remove_dir_all` 替代 `remove_file`。

2. **`coordinator.rs:149`** Rollback 使用字符串匹配 `phase.as_str()` + `_ => {}` 兜底 → 改为直接匹配 `InitPhase` 枚举，消除 name 变更时静默跳过的风险。内部跟踪改为 `Vec<InitPhase>`，仅在构建 `InitializationResult` 时转为字符串。

3. **`coordinator.rs:270-286`** `install_runtimes` 在 async fn 中调用同步 `std::fs::create_dir_all` 和 `migrate_from_legacy` → 改用 `tokio::fs::create_dir_all` 和 `tokio::task::spawn_blocking` 包装，避免阻塞 tokio 运行时。

## Notes
- `error.rs` 和 `mod.rs` 代码简洁干净，无问题。
- 无 UTF-8 切片、lock poisoning、unwrap/expect、SQL 注入、static mut 等常见问题。
- 整体模块职责清晰，符合 P2（高内聚）和 P6（简洁性）原则。
