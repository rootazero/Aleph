编译错误在 `agent_init.rs`，与 markdown 模块无关，是工作树中的预存问题。

下面是审查结果：

# Module: markdown

## Summary
- Files reviewed: 2 (`mod.rs`, `fences.rs`)
- Issues found: 0
- Issues fixed: 0

## Analysis

逐项审查结果：

### 1. 安全与健壮性 — 全部通过
| 检查项 | 状态 | 说明 |
|--------|------|------|
| UTF-8 安全 | ✅ | 无 `&s[..n]` 字节切片。使用 `.lines()`, `.as_bytes().get()` — 全部安全 |
| Lock 安全 | ✅ | 无锁使用 |
| unwrap/expect | ✅ | 唯一的 `expect` 在 `LazyLock` 静态正则初始化 (`fences.rs:14`)，硬编码正则不会失败 |
| SQL 注入 | ✅ | 无数据库交互 |
| static mut | ✅ | 正确使用 `LazyLock` |

### 2. 代码质量 — 良好
| 检查项 | 状态 | 说明 |
|--------|------|------|
| 死代码 | ✅ | 所有函数都是 pub 且被 `mod.rs` re-export |
| 未使用 import | ✅ | 干净 |
| DRY | ✅ | 无重复 |
| 函数长度 | ✅ | `parse_fence_spans` ~80 行，逻辑线性清晰，拆分反而降低可读性 |

### 3. 逻辑正确性 — 无问题
- **offset 计算**: 正确处理 `\n`、`\r\n`、无尾换行三种情况
- **fence 匹配**: 正确检查字符类型（`` ` `` vs `~`）、长度（闭合 >= 开启）、info string（闭合行不允许）
- **未闭合 fence**: 正确扩展到文本末尾
- **测试覆盖**: 16 个测试，覆盖所有边界情况（空文本、无 fence、未闭合、混合类型、缩进、长闭合标记等）

### 4. 架构合规 — 符合
- 纯解析工具模块，无外部依赖（仅 `regex`），符合 R3 核心轻量化
- 无平台 API 调用，符合 R1

## Notes
这是一个写得很好的小模块。`current_fence` 使用 5-元组（`fences.rs:80`）可以考虑改为命名结构体提升可读性，但按 P6 (KISS) 原则，作为单函数内的局部状态，当前写法完全可接受。

编译错误 (`agent_init.rs:177`) 是工作树中的预存问题，与本模块无关。
