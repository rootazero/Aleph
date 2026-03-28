`bundled` 模块无编译错误。

# Module: bundled

## Summary
- Files reviewed: 3 (`mod.rs`, `extractor.rs`, `manifest.rs`)
- Issues found: 0
- Issues fixed: 0

## Analysis

逐项检查结果：

| 检查项 | 结果 | 说明 |
|--------|------|------|
| UTF-8 安全 | **Pass** | 无 `&s[..n]` 字节切片。唯一的字符串转换 `to_string_lossy()` 用于编译时嵌入的路径，安全 |
| Lock 安全 | **Pass** | 模块无任何锁使用 |
| unwrap/expect | **Pass** | 所有错误路径均用 `if let Err` / `match` / `?` 处理，零 `unwrap()` |
| SQL 注入 | **N/A** | 无数据库交互 |
| static mut | **Pass** | 3 个 `static` 均为不可变（`Dir` 和 `&str`） |
| 死代码 | **Pass** | 所有函数和类型均被使用 |
| DRY | **Pass** | `extract_dir_contents` 被复用于 skills 和 plugins 提取 |
| 函数长度 | **Pass** | 最长函数 `extract_bundled_content` ~65 行，逻辑清晰无需拆分 |
| HashMap 顺序 | **Pass** | 使用 `BTreeMap` 保证确定性序列化 |
| 错误传播 | **Pass** | 所有 I/O 错误通过 `?` 或日志传播，无信息丢失 |
| 架构合规 | **Pass** | 纯文件系统操作，无平台 API 调用，符合 R1/R3 |

## Notes

这是一个干净、设计良好的模块：
- **防御性设计到位**: 每个 `create_dir_all`、`remove_dir_all`、`read_to_string` 都有错误处理
- **幂等性**: 提取逻辑通过版本比较避免重复工作，部分失败不更新版本号（下次重试）
- **用户技能保护**: `extract_skills` 跳过非 Official 来源的同名技能，不会覆盖用户自定义内容
- **BTreeMap** 确保 `manifest.json` 输出稳定，避免 git diff 噪音

无需修改。
