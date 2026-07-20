编译通过（14个 warnings 是预存的，与本次修改无关）。

---

# Module: teams

## Summary
- Files reviewed: 3 (`mod.rs`, `store.rs`, `types.rs`)
- Issues found: 3 (same类问题在3处)
- Issues fixed: 3

## Fixes
1. **[store.rs:150] `unwrap_or_default()` 静默吞掉损坏的 TeamStatus** → 实现 `FromSql for TeamStatus`，通过 `row.get(4)?` 正确传播解析错误
2. **[store.rs:172] `unwrap_or_default()` 静默吞掉损坏的 TeamTaskStatus** → 实现 `FromSql for TeamTaskStatus`，同上
3. **[store.rs:186] `read_summary_row` 同样的 `unwrap_or_default()` 问题** → 同样用 `row.get(4)?` 替换

修复方式：在 `store.rs` 中为 `TeamStatus` 和 `TeamTaskStatus` 添加 `rusqlite::types::FromSql` 实现，将手动的 `String → parse → unwrap_or_default` 替换为 rusqlite 原生的类型转换 + `?` 错误传播。

## Notes
- 代码整体质量很高：所有 SQL 均使用参数化查询 (`params![]`)，无注入风险
- 使用 `tokio::sync::Mutex`（无 poisoning 问题），无需 `unwrap_or_else(|e| e.into_inner())`
- 无 UTF-8 字节切片、无 `static mut`、无死代码
- `types.rs` 是纯领域类型（仅依赖 serde），职责清晰
- 状态检查模式（`add_member`/`create_task` 中检查团队是否已解散）出现2次，但各自错误消息不同，未达3次法则阈值，不提取
