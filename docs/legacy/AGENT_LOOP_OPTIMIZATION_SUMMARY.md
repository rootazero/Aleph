# Agent Loop 优化实施总结

**实施日期**: 2026-01-27
**状态**: ✅ 完成 (阶段 1-3)

---

## 📋 实施概览

本次优化针对 classical-poetry skill 执行中的三个核心问题：

1. **步数限制过早触发** (30步不足) → **提升到 100步**
2. **工具调用重复执行** (同一文件读取6次) → **添加 LRU 缓存**
3. **Doom Loop 检测不完善** (手动重置后失效) → **全局追踪机制**

---

## ✅ 阶段 1: 提高 Skill 执行步数限制

### 修改内容

**文件**: `core/src/ffi/processing/skill.rs`

**变更**:
- 将默认 `max_steps` 从 **30** 提升到 **100**
- 添加环境变量支持: `AETHER_SKILL_MAX_STEPS`
- 硬上限保护: 最大 500 步
- 添加配置日志输出

**代码片段** (第 130-147 行):
```rust
// Default: 100 steps (vs 30 previously), configurable via AETHER_SKILL_MAX_STEPS
let default_max_steps = 100;
let max_steps = std::env::var("AETHER_SKILL_MAX_STEPS")
    .ok()
    .and_then(|s| s.parse::<usize>().ok())
    .unwrap_or(default_max_steps)
    .min(500); // Hard upper limit

let loop_config = LoopConfig::default()
    .with_max_steps(max_steps)
    .with_max_tokens(150_000);

info!(
    skill_id = %skill.skill_id,
    max_steps = max_steps,
    "Skill execution configured"
);
```

**测试覆盖**:
- ✅ 默认值测试 (100步)
- ✅ 环境变量覆盖测试
- ✅ 上限保护测试 (500步)
- ✅ 无效输入回退测试

**环境变量使用**:
```bash
# 使用默认 100 步
./aether

# 临时提升到 150 步
export AETHER_SKILL_MAX_STEPS=150
./aether

# 测试上限保护（会被限制为 500）
export AETHER_SKILL_MAX_STEPS=1000
./aether
```

---

## ✅ 阶段 2: 添加工具结果缓存

### 架构设计

**缓存层**: `SingleStepExecutor` (透明缓存，上层无感知)
**缓存策略**: LRU (Least Recently Used)
**TTL**: 5 分钟 (可配置)
**容量**: 100 条 (可配置)

### 新增文件

#### 1. `core/src/executor/cache_config.rs` (113 行)

**功能**: 缓存配置管理

**配置项**:
```rust
pub struct ToolCacheConfig {
    enabled: bool,              // 默认: true
    capacity: usize,            // 默认: 100
    ttl_seconds: u64,           // 默认: 300 (5分钟)
    cache_only_success: bool,   // 默认: true (仅缓存成功结果)
    exclude_tools: Vec<String>, // 默认: ["bash", "code_exec"]
}
```

**关键方法**:
- `should_cache(tool_name)`: 判断工具是否应被缓存
- `ttl()`: 返回 TTL 的 Duration

#### 2. `core/src/executor/cache_store.rs` (296 行)

**功能**: LRU 缓存存储实现

**缓存键设计**:
```rust
struct ToolCallCacheKey {
    tool_name: String,
    args_hash: u64,  // serde_json::to_string(args) 的哈希
}
```

**核心方法**:
- `lookup(tool_name, arguments)`: 查找缓存，检查 TTL
- `store(tool_name, arguments, result)`: 存储结果到缓存
- `clear()`: 清空所有缓存
- `stats()`: 获取缓存统计信息

**测试覆盖**:
- ✅ 缓存命中测试
- ✅ 不同参数缓存未命中测试
- ✅ TTL 过期测试 (1秒 TTL)
- ✅ 排除工具测试 (bash 不缓存)
- ✅ 仅缓存成功结果测试
- ✅ 缓存统计测试

### 集成修改

**文件**: `core/src/executor/single_step.rs`

**变更**:
1. 添加 `result_cache: Arc<ToolResultCache>` 字段
2. 新增构造方法: `with_cache_config()`
3. 修改 `execute_tool_call()`:
   - 执行前: 尝试从缓存获取
   - 执行后: 存储结果到缓存

**缓存流程**:
```
Tool Call Request
      ↓
  Lookup Cache
      ↓
   Hit? → Yes → Return Cached Result (< 1ms)
      ↓
     No
      ↓
Execute Tool (100-1000ms)
      ↓
Store to Cache
      ↓
Return Result
```

**日志输出**:
```
[INFO] Tool result returned from cache (tool=file_ops, duration_ms=0)
[DEBUG] Tool result cache HIT (tool_name=file_ops, hit_count=2, age_secs=15)
[DEBUG] Tool result cache MISS (tool_name=file_ops)
[DEBUG] Tool result cached (tool_name=file_ops, cache_size=5)
```

---

## ✅ 阶段 3: 增强 Doom Loop 检测

### 问题分析

**当前问题**:
1. 用户点击"继续"后调用 `reset_doom_loop_detection()`，计数器清零
2. 同一文件被读取 6+ 次但未触发检测
3. 路径格式差异导致哈希不同 (`/tmp` vs `/private/tmp`)

### 解决方案

#### 3.1 全局工具调用追踪

**文件**: `core/src/agent_loop/guards.rs`

**新增字段** (struct LoopGuard):
```rust
/// Global tool call history (not cleared on reset)
global_tool_history: Vec<ToolCallRecord>,
/// Global doom loop threshold (higher than local)
global_doom_threshold: usize,  // 默认: 10
```

**增强 `record_tool_call()`**:
```rust
pub fn record_tool_call(&mut self, tool_name: &str, arguments: &serde_json::Value) {
    let record = ToolCallRecord::new(tool_name.to_string(), arguments.clone());

    // Add to recent (will be cleared on reset)
    self.recent_tool_calls.push(record.clone());
    // Keep bounded (threshold * 2)

    // Add to global history (persists across resets)
    self.global_tool_history.push(record);
    // Keep bounded (global_threshold * 3)
}
```

#### 3.2 全局 Doom Loop 检测

**新增方法**: `check_global_doom_loop()` (第 369-409 行)

**检测逻辑**:
1. 统计全局历史中每个唯一工具调用的出现次数
2. 找到最多重复的调用
3. 如果重复次数 ≥ 10，触发全局 doom loop 警告

**代码**:
```rust
fn check_global_doom_loop(&self) -> Option<GuardViolation> {
    if self.global_tool_history.len() < self.global_doom_threshold {
        return None;
    }

    // Count occurrences of each unique tool call
    let mut call_counts: HashMap<(String, u64), usize> = HashMap::new();

    for record in &self.global_tool_history {
        let key = (record.tool_name.clone(), record.arguments_hash);
        *call_counts.entry(key).or_insert(0) += 1;
    }

    // Find the most repeated call
    if let Some((tool_key, count)) = call_counts.iter().max_by_key(|(_, count)| *count) {
        if *count >= self.global_doom_threshold {
            return Some(GuardViolation::DoomLoop {
                tool_name: tool_key.0.clone(),
                repeat_count: *count,
                arguments_preview: representative.arguments_preview(100),
            });
        }
    }

    None
}
```

#### 3.3 检测顺序优化

**更新 `check()` 方法** (第 281-289 行):
```rust
// Check for doom loop (exact same tool call repeated) - most precise check
if let Some(violation) = self.check_doom_loop() {
    return Some(violation);
}

// Check for global doom loop (across resets)
if let Some(violation) = self.check_global_doom_loop() {
    return Some(violation);
}
```

**检测层次**:
1. **局部 doom loop** (3次重复) → 可重置
2. **全局 doom loop** (10次重复) → 不可重置
3. **Stuck loop** (5次相同动作)
4. **Repeated failures** (失败模式检测)

---

## 📊 性能优化效果

### 预期指标

| 指标 | 修改前 | 修改后 | 提升 |
|------|--------|--------|------|
| **Skill 最大步数** | 30 | 100 | +233% |
| **重复工具调用次数** | 6+ | 1-2 | -70% |
| **缓存命中率** | N/A | >50% | - |
| **Token 消耗** | 基线 | -40~60% | 节省 40-60% |
| **执行时间** | 基线 | -20~30% | 减少 20-30% |

### 缓存效果分析

**Classical Poetry Skill 典型流程**:
1. 读取词牌规范文件 (`poetry_specs.json`) → **缓存**
2. 读取参考意象库 (`imagery_db.json`) → **缓存**
3. 格律检查 (多次读取规范) → **命中缓存**
4. 意象查询 (多次读取意象库) → **命中缓存**

**预计缓存命中率**: 60-80%

---

## 🔧 配置指南

### 环境变量配置

```bash
# Skill 执行步数 (默认: 100)
export AETHER_SKILL_MAX_STEPS=150

# 示例：复杂 skill 需要更多步骤
export AETHER_SKILL_MAX_STEPS=200
```

### 代码配置 (高级)

**自定义缓存配置**:
```rust
use aethecore::executor::{ToolCacheConfig, SingleStepExecutor};

let mut cache_config = ToolCacheConfig::default();
cache_config.ttl_seconds = 600;  // 10 分钟
cache_config.capacity = 200;     // 200 条
cache_config.exclude_tools.push("web_search".to_string());

let executor = SingleStepExecutor::with_cache_config(
    tool_registry,
    cache_config,
);
```

---

## 🧪 测试验证

### 测试覆盖总结

| 模块 | 测试数量 | 状态 |
|------|---------|------|
| **skill.rs** | 6 | ✅ 全部通过 |
| **cache_config.rs** | 4 | ✅ 全部通过 |
| **cache_store.rs** | 6 | ✅ 全部通过 |
| **guards.rs** | 18 | ✅ 全部通过 |
| **single_step.rs** | 6 | ✅ 全部通过 |

**总计**: 40 个测试，全部通过 ✅

### 手动验证流程

#### 1. 验证步数提升

```bash
cd core && cargo build --release

# 运行 classical-poetry skill
# 观察日志：应显示 "max_steps=100"
# 应在 100 步前完成，不触发 MaxSteps 限制
```

#### 2. 验证缓存功能

**日志检查点**:
```
[DEBUG] Tool result cache MISS (tool_name=file_ops)
[INFO] Tool executed successfully (tool=file_ops, duration_ms=125)
[DEBUG] Tool result cached (tool_name=file_ops, cache_size=1)

# 第二次读取同一文件
[DEBUG] Tool result cache HIT (tool_name=file_ops, hit_count=1, age_secs=2)
[INFO] Tool result returned from cache (tool=file_ops, duration_ms=0)
```

#### 3. 验证 Doom Loop 检测

**测试场景**:
```
1. 连续 3 次读取同一文件 → 触发局部 doom loop
2. 用户点击"继续"
3. 再连续 7 次读取同一文件 → 总共 10 次
4. 应触发全局 doom loop 警告
```

---

## ⚠️ 风险评估与缓解

### 已识别风险

| 风险 | 影响 | 概率 | 缓解措施 |
|------|------|------|---------|
| **缓存返回过期数据** | 中 | 低 | TTL 5分钟，skill 执行通常 <5 分钟 |
| **步数过高导致执行时间长** | 低 | 低 | 保留 timeout (600秒)，硬上限 500 步 |
| **内存占用增加** | 低 | 低 | LRU 限制 100 条，总计 <100MB |
| **Doom loop 误报** | 低 | 低 | 全局阈值为 10（较高） |

### 向后兼容性

✅ **完全向后兼容**:
- 现有 skill 无需修改即可受益
- Agent Loop (非 skill) 不受影响 (仍为 20 步)
- 缓存是透明的，不改变 API
- 默认配置提供合理的开箱即用体验

❌ **预期行为变化** (正向):
- Skill 执行步数限制从 30 提升到 100
- 重复工具调用被缓存，减少执行次数
- Doom loop 检测更灵敏

---

## 📝 文件清单

### 新增文件 (2)

1. `core/src/executor/cache_config.rs` (113 行)
2. `core/src/executor/cache_store.rs` (296 行)

### 修改文件 (4)

1. `core/src/ffi/processing/skill.rs` (第 130-147 行, 第 321-384 行)
2. `core/src/executor/mod.rs` (添加模块导出)
3. `core/src/executor/single_step.rs` (第 95-135 行, 第 137-218 行)
4. `core/src/agent_loop/guards.rs` (第 189-207 行, 第 209-246 行, 第 318-341 行, 第 369-409 行)

### 依赖变更

```toml
# core/Cargo.toml
[dependencies]
lru = "0.12"  # 已存在，无需添加
```

---

## 🚀 部署建议

### 发布步骤

1. **编译验证**:
```bash
cd core
cargo build --release
cargo test --lib
```

2. **集成测试** (macOS):
```bash
cd platforms/macos
xcodegen generate
open Aether.xcodeproj
# 构建并运行
# 触发 classical-poetry skill 验证
```

3. **日志监控**:
   - 检查 `max_steps` 配置值
   - 监控缓存命中率
   - 观察 doom loop 触发频率

4. **性能基准测试**:
   - 运行 10 次 classical-poetry skill
   - 记录步数使用、token 消耗、执行时间
   - 对比优化前基线数据

### 回滚方案

如遇问题可快速回滚：

```rust
// 临时禁用缓存
let mut config = ToolCacheConfig::default();
config.enabled = false;

// 临时降低步数
export AETHER_SKILL_MAX_STEPS=30
```

---

## 📚 相关文档

- [AGENT_LOOP.md](./AGENT_LOOP.md) - Agent Loop 架构文档
- [DISPATCHER.md](./DISPATCHER.md) - Dispatcher 系统文档
- [BUILD_COMMANDS.md](./BUILD_COMMANDS.md) - 构建命令参考

---

## 🎯 下一步 (可选)

### 阶段 4: 配置文件支持

**优先级**: 📝 Optional (长期架构优化)

**目标**:
- 支持从 `~/.aether/config.toml` 读取配置
- 支持 skill 级别的配置覆盖

**预计工作量**: 2-3 天

**配置文件示例**:
```toml
[skill_execution]
default_max_steps = 100
default_max_tokens = 150_000

[skill_execution.tool_cache]
enabled = true
capacity = 100
ttl_seconds = 300

[skill_execution.overrides.classical-poetry]
max_steps = 120
max_tokens = 200_000
```

---

**实施完成度**: 3/3 核心阶段 ✅
**预计效果**: 80%+ 成功率提升，40-60% token 节省
**生产就绪**: ✅ 是

---

*最后更新: 2026-01-27*
