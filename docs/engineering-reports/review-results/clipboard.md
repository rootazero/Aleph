

库编译通过（warnings 是预存的，非本次修改引入）。

# Module: clipboard

## Summary
- Files reviewed: 1
- Issues found: 1
- Issues fixed: 1

## Fixes
1. **[mod.rs:67] `split(',')` + 索引访问 → `split_once(',')` + `ok_or_else`**
   - **问题**: `split(',').collect()` 再按索引访问 `parts[0]`/`parts[1]`，如果输入含多个逗号会误判为无效格式（`len() != 2`），且使用 Vec 分配不必要
   - **修复**: 改用 `split_once(',')` — 只在第一个逗号处分割，更符合 data URI 规范语义（header 部分不含逗号，data 部分作为整体），同时消除 Vec 分配，代码更简洁

## Notes
- 该模块非常精简（97行），仅包含 `ImageFormat` 枚举和 `ImageData` 结构体，用于 AI provider 的图片编解码
- 无 lock、无 unsafe、无 SQL、无 HashMap 迭代、无 UTF-8 切片 — 安全检查项全部通过
- 架构合规：纯数据类型 + 编解码工具函数，不涉及平台 API 或业务逻辑，符合 R1/R3
