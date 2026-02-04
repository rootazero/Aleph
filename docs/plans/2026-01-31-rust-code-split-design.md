# Rust 代码拆分设计

> 日期: 2026-01-31
> 状态: 实施中

## 背景

分析发现 47 个超过 800 行的 Rust 文件，主要问题包括：
- CLI/Server 单体文件
- 内联测试占比过高 (30-55%)
- 功能域混合
- 代码重复

## 优先级

| 优先级 | 任务 | 预计减少行数 | 复杂度 |
|-------|------|------------|-------|
| P0 | `aleph_gateway.rs` 拆分 | -2000+ | 高 |
| P1 | 测试文件外提 (5个文件) | -2500+ | 低 |
| P2 | `facts.rs` 功能拆分 | 重组 | 中 |
| P3 | `prompt_builder.rs` 去重 | -300 | 低 |

---

## P0: aleph_gateway.rs 拆分

**原文件:** `core/src/bin/aleph_gateway.rs` (2255 行)

**目标结构:**
```
core/src/bin/aleph_gateway/
├── main.rs          # main() 入口
├── cli.rs           # Clap CLI 定义
├── commands/
│   ├── mod.rs       # 命令分发
│   ├── start.rs     # 服务器启动
│   ├── plugins.rs   # 插件管理
│   ├── config.rs    # 配置命令
│   ├── pairing.rs   # 配对命令
│   ├── devices.rs   # 设备命令
│   ├── channels.rs  # 渠道命令
│   └── cron.rs      # 定时任务命令
├── daemon.rs        # 守护进程管理
└── server_init.rs   # 服务器初始化辅助
```

---

## P1: 测试文件外提

**受影响文件:**

| 文件 | 测试占比 | 目标 |
|------|---------|------|
| `plugin_registry.rs` | ~55% | `plugin_registry/tests.rs` |
| `agent_loop.rs` | ~55% | `agent_loop/tests.rs` |
| `inbound_router.rs` | ~45% | `inbound_router/tests.rs` |
| `facts.rs` | ~30% | `facts/tests.rs` |
| `prompt_builder.rs` | ~25% | `prompt_builder/tests.rs` |

**模式:**
```rust
// 原文件末尾改为:
#[cfg(test)]
mod tests;

// 新建同名目录下的 tests.rs
```

---

## P2: facts.rs 功能拆分

**目标结构:**
```
core/src/memory/database/facts/
├── mod.rs          # 重新导出
├── crud.rs         # insert_fact, invalidate_fact
├── search.rs       # search_facts, find_similar_facts
├── hybrid.rs       # hybrid_search_facts
├── stats.rs        # get_fact_stats, clear_facts
└── tests.rs        # 测试
```

---

## P3: prompt_builder.rs 去重

**重构方向:** 提取可复用的节构建器

```rust
impl PromptBuilder {
    fn build_dynamic_content(&self, tools: &[ToolInfo]) -> String {
        let mut prompt = String::new();
        prompt.push_str(&self.section_runtimes());
        prompt.push_str(&self.section_tools(tools));
        prompt.push_str(&self.section_special_actions());
        // ... 其他节
        prompt
    }

    fn section_runtimes(&self) -> String { /* 提取 */ }
    fn section_tools(&self, tools: &[ToolInfo]) -> String { /* 提取 */ }
    // ...
}
```

---

## 不拆分的文件

| 文件 | 理由 |
|------|------|
| `claude.rs` | 结构清晰，测试仅 15% |
| `unified.rs` | 类型定义应集中 |
| `error.rs` | 错误类型应集中 |
| `execution_engine.rs` | 核心引擎，内聚性强 |
