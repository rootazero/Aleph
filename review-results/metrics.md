所有 17 个测试通过。

# Module: metrics

## Summary
- Files reviewed: 1 (`core/src/metrics/mod.rs`, 371 行)
- Issues found: 1
- Issues fixed: 1

## Fixes
1. **[mod.rs:205-219] 除零保护** — `target_ms` 为 0 时，`ratio` 计算产生 `f64::INFINITY`，且 `threshold_ms` 为 0 导致任何操作都误触发 slow warning。添加 `target_ms > 0` 守卫跳过无意义的零值目标。

## Notes
这个模块非常干净，没有发现其他审查清单中的问题：
- 无 `&s[..n]` 字节切片、无 `lock().unwrap()`、无 `static mut`、无 SQL 拼接
- 无死代码或未使用 import
- 函数长度合理，无需拆分
- `HashMap` 仅用于 logging metadata（`?self.metadata` 用 Debug 格式），不涉及安全规则排序
- 架构合规，纯工具模块，无红线违反
