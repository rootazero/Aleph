---

# Module: tools

## Summary
- Files reviewed: 20
- Issues found: 7
- Issues fixed: 7

## Fixes

1. **`traits.rs:132`** Schema 序列化静默失败 → 添加 `inspect_err` 错误日志，失败时 tracing::error 记录工具名和错误详情
2. **`repair.rs:36`** `to_ascii_lowercase()` 无法处理非 ASCII 大写字符 → 改用 `c.to_lowercase()` 迭代器，正确处理 Unicode
3. **`repair.rs:26-28`** `to_snake_case` 不识别数字边界 → `prev_was_lower` 扩展为 `prev_was_lower_or_digit`，使 `"getMP3File"` → `"get_mp3_file"`
4. **`generator.rs:392`** YAML 注入：`escape_yaml_string` 未转义 `\n`/`\r` → 添加换行符转义，防止 LLM 生成的 description 注入 YAML 字段
5. **`executor.rs:461-510`** Docker flag 安全：注释写"allowlist"但实现是 denylist → 重写为真正的 allowlist，未知 flag 一律拒绝
6. **`auto_loader.rs:139`** `Ok(0)`（生成成功但加载 0 个工具）未计入 `failed` → 添加 guard `count > 0`，`Ok(0)` 归入 failed 并 warn
7. **`repair.rs:53-149`** `call_with_repair_impl` 对每个步骤分别获取 read lock → 合并为单次 read lock 查找所有候选，消除 TOCTOU 竞争窗口

## Notes
- `context.rs` 完全干净，`home_dir` 有正确 fallback
- `schema_strictify.rs` 的双重递归看似奇怪但逻辑正确
- `watcher.rs` 的 skill 删除热重载标注为 "not yet implemented"，是已知的待完成功能
- `server/mod.rs` 的 builder `expect()` 在构造期使用合理（self 独占所有权，无竞争）
- `parser.rs` 的字符串切片全部基于 ASCII 分隔符，UTF-8 安全
- 预存的 `agent_init.rs:177` 编译错误不在本次审查范围内
thout a trailing newline after the closing `---`.

### Warning (Defensive)

7. **`repair.rs:36`** `to_snake_case` multi-codepoint drop → Replaced `c.to_lowercase().next().unwrap_or(c)` with `c.to_ascii_lowercase()`. More explicit about the domain assumption (tool names are ASCII identifiers) and eliminates the iterator entirely.

8. **`tool_adapter.rs:241`** `obj.get(key).unwrap()` → Replaced with `if let Some(value) = obj.get(key)`. Technically safe today (keys collected from same map), but fragile under refactoring.

9. **`context.rs:42-44`** Silent error swallow → Added `tracing::warn!` with error details and path in the fallback. Previously, `from_workspace` failures were silently ignored, leading to opaque tool failures later.

## Verification
- `cargo check -p alephcore --lib` — **clean** (0 errors, 14 pre-existing warnings unchanged)
- `cargo test -p alephcore --lib tools::` — **485 passed, 0 failed**

## Notes
- The `agent_init.rs:177` compilation error in the binary target is pre-existing and unrelated to tools/.
- `executor.rs` VirtualFs sandbox provides only environment-level isolation (tools can still access real FS via absolute paths). This is documented but worth noting — untrusted skills should always use Docker mode.
- `list_tools_arc_impl` in `ops.rs` also iterates HashMap but returns `Vec<Arc<dyn AlephToolDyn>>` — ordering there is less critical since it's used internally for adapter wrapping, not LLM prompts.
