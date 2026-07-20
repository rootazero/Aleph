---

# Module: memory

## Summary
- **Files reviewed**: 143
- **Issues found**: 16
- **Issues fixed**: 16
- **Compilation**: `cargo check --lib` ✅ (0 errors)
- **Tests**: 729 passed, 0 failed ✅

## Fixes

### 安全与健壮性

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| 1 | `cli/commands.rs:112` | UTF-8 不安全 `&fact.content[..47]` 字节切片 | → `fact.content.chars().take(47).collect()` |
| 2 | `transcript_indexer/mod.rs:138` | 测试中 UTF-8 不安全字节切片 (chunk overlap) | → `chars().rev().take(40)` / `chars().take(40)` |
| 3 | `transcript_indexer/mod.rs:189` | 测试 helper 中 UTF-8 不安全 `current_chunk[len - overlap..]` | → `char_indices().nth(skip)` 安全切片 |
| 4 | `cortex/meta_cognition/injection.rs:213` | `anchor_store.read().map_err()` lock 中毒未恢复 | → `.unwrap_or_else(\|e\| e.into_inner())` |
| 5 | `cortex/meta_cognition/reactive.rs:180` | `anchor_store.write().map_err()` lock 中毒未恢复 | → `.unwrap_or_else(\|e\| e.into_inner())` |
| 6 | `cortex/meta_cognition/reactive.rs:475` | 同上 (async 变体) | → `.unwrap_or_else(\|e\| e.into_inner())` |
| 7 | `cortex/meta_cognition/injection.rs:176` | `NonZeroUsize::new(cache_size).unwrap()` 当 cache_size=0 时 panic | → fallback 到 1 |

### 逻辑正确性

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| 8 | `decay.rs:37` | `calculate_strength()` 除零 — `half_life_days <= 0.0` 未防护 | → 添加 guard `return 1.0` |
| 9 | `decay.rs:79` | `calculate_strength_tiered()` 除零 — tier config 零半衰期 | → 添加 guard `return 1.0` |
| 10 | `decay.rs:113` | `calculate_strength_for_type()` 仅检查 `is_infinite()` | → 扩展为 `is_infinite() \|\| <= 0.0` |
| 11 | `decay.rs:397` | `effective_half_life()` 除零 — `access_decay_days` 为零 | → fallback 到 30.0 |
| 12 | `consolidation/analyzer.rs:285` | `cosine_similarity` 缺少 NaN 防护 | → 添加 `is_finite()` + `clamp(-1.0, 1.0)` |
| 13 | `consolidation/analyzer.rs:255` | `.unwrap()` on `max_by` 无上下文 | → `expect()` + 文档化不变量 |

### 代码质量

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| 14 | `graph.rs` | `extract_entities_from_text()` 每次调用编译 4 个 Regex | → `static Lazy<Regex>` 一次编译 |
| 15 | `graph.rs` | `extract_query_hints()` 每次调用编译 2 个 Regex | → `static Lazy<Regex>` 一次编译 |
| 16 | `compression_daemon/daemon.rs:140` | 不必要的 `unsafe impl Send + Sync` | → 移除（所有字段已自动满足） |

### 代码质量 (续)

| # | 文件 | 问题 | 修复 |
|---|------|------|------|
| — | `value_estimator/llm_scorer.rs:225` | `__msgs` 双下划线命名误导 (实际有使用) | → 重命名为 `msgs` |

## Notes

- **store/ 子目录质量优秀** — SQL 注入防护已全面覆盖 (`escape_sql_string`)，无任何问题
- **retrieval 子系统质量优秀** — 25 个文件全部使用 `chars().take(n)` 安全截断，浮点比较均有 `partial_cmp` fallback
- `decay.rs` 是本次审查的最大发现 — 4 处除零漏洞，在极端配置下可产生 `NaN`/`Infinity` 污染整个衰减计算链
- `graph.rs` Regex 性能优化影响每次实体提取调用，在高频记忆写入场景下有明显改善
