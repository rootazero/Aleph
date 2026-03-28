全部 100 个测试通过。审查完成。

---

# Module: routing

## Summary
- Files reviewed: 9
- Issues found: 2
- Issues fixed: 2

## Fixes

1. **`session_key.rs:390,400`** `PeerKind::Thread` roundtrip 断裂 → 添加 `[channel, "thread", peer_id]` 和 `[channel, "thread", peer_id, "thread", thread_id]` 两个 parse 模式。`to_key_string()` 对 `PeerKind::Thread` 生成 `agent:id:ch:thread:peer`，但 `parse()` 缺少对应模式导致返回 `None`。

2. **`identity_links.rs:31`** 排序时不必要的堆分配 → `sort_by_key(|(k, _)| (*k).clone())` 每次比较都 clone String，改为 `sort_by(|(a, _), (b, _)| a.cmp(b))` 零分配比较。

## Notes

- 该模块整体质量很高：无 `lock().unwrap()`、无 unsafe 字节切片、无 SQL 注入风险、HashMap 迭代已排序
- `llm_classifier.rs` 的 `extract_json` 中所有 `&str` 切片的 `find()` 目标均为 ASCII 字符（`` ``` ``、`{`、`}`、`\n`），字节偏移始终落在合法 char boundary，UTF-8 安全
- `EscalationSnapshot` 定义在 `task_router.rs` 中但模块内未使用——可能被外部模块消费，未删除
- 架构合规：无红线违反，符合 P1-P8 设计原则
