

All 19 tests pass. Here's the review report:

# Module: logging

## Summary
- Files reviewed: 5
- Issues found: 3
- Issues fixed: 3

## Fixes
1. **[pii_filter.rs:188,191]** `write!().unwrap()` in production `StringVisitor::record_debug` → Changed to `let _ = write!()` (defensive — `String`'s `fmt::Write` is infallible but `unwrap` violates P7 defensive design)
2. **[pii_filter.rs:266]** `lock().unwrap()` in test `CaptureWriter::write` → Changed to `.unwrap_or_else(|e| e.into_inner())` (lock poisoning safety)
3. **[pii_filter.rs:293]** `lock().unwrap()` in test assertion → Changed to `.unwrap_or_else(|e| e.into_inner())`

## Notes
- **整体质量很高** — logging 模块代码简洁，大部分逻辑已委托给 `aleph-logging` crate，core 层仅保留薄包装和 PII scrubbing layer
- **`PiiScrubbingLayer` 是空实现** — `on_event` 什么都不做，实际 scrubbing 通过 `PiiScrubbingFormat` 完成。struct 保留作为公共 API 和文档用途，但注释已充分说明原因
- **无安全/架构违规** — 没有 UTF-8 切片、SQL 注入、`static mut`、HashMap 顺序依赖等问题
- **架构合规** — 符合 R3（核心轻量化，委托给独立 crate）和 P2（高内聚，5 个文件各司其职）
