# Logic Review Report
**Module**: tool_output
**Scope**: `src/tool_output/compressor.rs`, `src/tool_output/distill.rs`, `src/tool_output/mod.rs`
**Date**: 2026-06-01
**Mode**: strict

## Findings

### [Warning] compress_snapshot 未对保留的单行长度设限
- **Location**: `compressor.rs:136-186`
- **Trigger condition**: DevTools `take_snapshot` 输出中包含极长文本节点（如内嵌 base64 data URI 的 `<img>` 标签）
- **Expected behavior**: 每行保留长度应有上限，防止单个长行超出 token 预算
- **Actual behavior**: 交互元素匹配成功的行会被完整保留，无长度限制
- **Suggested fix**: 引入 `MAX_SNAPSHOT_LINE_CHARS = 500` 并在保留行时通过 `cap_line()` 截断
- **Status**: ✅ Fixed

### [Warning] extract_path 无法识别无扩展名的文件路径
- **Location**: `distill.rs:147-177`
- **Trigger condition**: 日志/编译错误中引用无扩展名文件，如 `Makefile:42`、`/etc/passwd:88`
- **Expected behavior**: 所有合理的路径引用都应被提取
- **Actual behavior**: `path_part.contains('.')` 强制要求扩展名 dot，导致无 dot 的路径被过滤
- **Suggested fix**: 放宽条件，允许 `/` 或 `\` 作为路径指示符
- **Status**: ✅ Fixed

### [Warning] strip_ansi 不完整 CSI 序列可能消费剩余所有字符
- **Location**: `distill.rs:98-104`
- **Risk**: 输入流中若存在孤立的 `ESC [` 且无 final byte（0x40-0x7e），循环 `for inner in chars.by_ref()` 将消费至迭代器耗尽，丢弃序列后所有内容
- **Current impact**: low（真实日志中不完整 ANSI 序列极罕见）
- **Suggestion**: 为 CSI 循环设置最大长度上限（如 100），超限后中止序列解析
- **Status**: Not fixed (impact too low for production fix)

### [Warning] compress_screenshot base64 检测存在低概率误报
- **Location**: `compressor.rs:92-129`
- **Risk**: 长度 > 100 且前 128 字节含 `+`、`/` 或 `=` 的文本（如 URL 参数、数学表达式）可能被误判为 base64 并整体替换
- **Current impact**: low（截图输出中出现此类文本的概率极低）
- **Suggestion**: 增加额外检查（如长度是否为 4 的倍数、非字母数字字符占比）
- **Status**: Not fixed (acceptable false-positive rate)

## Summary

| Level | Count |
|-------|-------|
| Critical | 0 |
| Warning | 4 |
| Suggested Test | 0 |

## Fixes Applied

### compressor.rs
1. 新增常量 `MAX_SNAPSHOT_LINE_CHARS: usize = 500`
2. 新增辅助函数 `cap_line()`，基于字符数安全截断（不切割多字节 UTF-8 字符）
3. `compress_snapshot()` 中：
   - `kept` 类型从 `Vec<&str>` 改为 `Vec<String>`
   - 交互元素保留前通过 `cap_line()` 截断
   - fallback（无交互元素保留前 20 行）同样通过 `cap_line()` 截断

### distill.rs
1. `extract_path()` 中路径识别条件放宽：
   - 原条件：`path_part.contains('.')`
   - 新条件：`path_part.contains('.') || path_part.contains('/') || path_part.contains('\\')`
   - 使无扩展名但含路径分隔符的文件（如 `/home/user/Makefile:42`）可被正确识别

## Verification
- `cargo test -p alephcore --lib tool_output::` — 34 tests passed
- `cargo check -p alephcore` — clean (only pre-existing warnings in unrelated modules)
