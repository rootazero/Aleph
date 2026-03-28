

# Module: pii

## Summary
- Files reviewed: 11
- Issues found: 4
- Issues fixed: 4

## Fixes

1. **`engine.rs:106`** Lock 安全 — `reload()` 中 `if let Ok(mut guard) = engine.write()` 静默忽略 poisoned RwLock → 改用 `.unwrap_or_else(|e| e.into_inner())` 恢复锁
2. **`id_card.rs:41`** UTF-8 安全 — `id[..2].parse()` 字节切片 → 改用 `id.get(..2).and_then(|s| s.parse().ok())`
3. **`id_card.rs:50,54,58`** UTF-8 安全 — `id[6..10]`, `id[10..12]`, `id[12..14]` 字节切片 → 同样改用 `.get()` 安全模式
4. **`id_card.rs:77`** 边界安全 — `bytes[i]` 直接索引 → 改用 `bytes.iter().zip(WEIGHTS.iter())` 迭代器模式 + 前置长度检查
5. **`allowlist.rs:31-35`** unwrap 规范 — 5 处 `Regex::new().unwrap()` → 改用 `.expect("valid regex literal")`

## Notes

**代码质量良好的方面：**
- `OnceLock` 用于所有静态正则（无 `static mut`）
- `engine.rs:179-183` 替换前已做 `is_char_boundary()` 检查，UTF-8 替换安全
- `phone.rs:56-63` 的 timestamp context 检查已正确处理多字节字符边界
- 所有规则文件都正确使用 `OnceLock` 而非 `lazy_static`
- Luhn 校验、ID 卡校验码等算法实现正确

**架构合规：** 无违反红线或设计原则。模块职责清晰（engine/rules/allowlist 分离），符合 P2 高内聚。

**DRY 观察：** `api_key.rs`、`ssh_key.rs`、`email.rs`、`ip_address.rs` 四个文件的 `detect()` 方法体完全相同（简单的 regex find_iter → PiiMatch 收集）。已超过三次法则阈值，未来可考虑提取为 `PiiRule` trait 的默认方法或辅助函数，但当前不影响正确性。
