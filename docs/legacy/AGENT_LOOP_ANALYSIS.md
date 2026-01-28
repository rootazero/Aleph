# Agent Loop 问题分析报告

## 问题概述

使用 `classical-poetry` skill 时出现严重的执行效率和上下文传递问题：

1. **步数限制过早触发**：30步限制导致复杂任务无法完成
2. **上下文记忆丢失**：路径信息和参数选择在多轮对话中丢失
3. **重复工具调用**：同一文件被读取6次以上，未触发 doom loop 检测
4. **多次询问相同问题**：用户被重复询问路径和参数配置

## 日志分析

### 关键发现

#### 1. 步数限制触发 (MaxSteps)
```
行1930: Guard triggered, violation=MaxSteps { current: 30, limit: 30 }
行1936: Skill execution guard triggered, skill_id=classical-poetry
```
- **30步限制太低**，复杂 skill 需要更多步骤

#### 2. 重复读取同一文件 (至少6次)
```
行397:  file_ops - 读取文件: /tmp/poetry_refs.json (99 bytes)
行532:  file_ops - 读取文件: /tmp/poetry_refs.json (99 bytes)
行700:  file_ops - 读取文件: /tmp/poetry_refs.json (99 bytes)
行835:  file_ops - 读取文件: /tmp/poetry_refs.json (99 bytes)
行968:  file_ops - 读取文件: /tmp/poetry_refs.json (1592 bytes)
...
```
- **Doom loop 未触发**：虽然有机制，但阈值设置可能不合理
- **无缓存机制**：重复读取相同内容浪费 token 和步数

#### 3. 多次询问用户相同问题
```
行289:  User input required: 请告诉我 classical-poetry 技能的安装路径...
行1203: Auto-responding: script_path="$HOME/.claude/skills/classical-poetry"
行1908: User input required: 为继续意象采集与格律验证，请提供...路径
```
- **上下文丢失**：之前回答过的路径信息没有保留
- **记忆传递失败**：skill 内部的 LLM 调用之间缺乏上下文传递

#### 4. 多次自动回答相同参数问题 (至少4次)
```
行263:  Auto-responding: cipu="1. 钦定词谱", yunshu="1. 平水韵"
行606:  Auto-responding: cipu="1. 钦定词谱", yunshu="1. 平水韵"
行1203: Auto-responding: cipu="1. 钦定词谱", script_path="..."
行1885: Auto-responding: cipu="1. 钦定词谱", tone="哀婉含蓄"
```
- **重复决策**：相同参数被重复选择，说明没有记忆之前的选择

#### 5. API 超时和重试
```
行628:  OpenAI request timed out
行1374: OpenAI request timed out
行1541: Server error 502 Bad Gateway
```
- **网络问题**加剧了步数消耗

## 代码层面问题

### 1. Skill 执行步数限制过低

**位置**: `core/src/ffi/processing/skill.rs:132`

```rust
let loop_config = LoopConfig::default()
    .with_max_steps(30)  // ❌ 太低！
```

**默认值**: `core/src/agent_loop/config.rs:326`
```rust
fn default_max_steps() -> usize {
    50  // ✅ 默认是50，但 skill 执行时被硬编码为30
}
```

**问题**：
- 复杂的 skill（如 classical-poetry）需要多次工具调用（读取参考文件、格律检查、意象采集等）
- 30步对于需要多轮验证和修改的任务明显不足

### 2. Doom Loop 检测阈值

**位置**: `core/src/agent_loop/config.rs:361`

```rust
fn default_doom_loop_threshold() -> usize {
    3  // Match OpenCode: DOOM_LOOP_THRESHOLD = 3
}
```

**问题**：
- 阈值为3意味着需要**连续3次完全相同的工具调用**（相同工具名+相同参数）才触发
- 日志显示读取 `/tmp/poetry_refs.json` 至少6次，但**参数可能略有不同**（如路径展开方式）
- 需要检查是否正确记录了 tool call

### 3. 上下文传递机制缺失

**位置**: `core/src/ffi/processing/skill.rs:72-89`

```rust
// Build the skill execution prompt
let full_input = if let Some(att_text) = attachment_text {
    format!(
        "# 用户附件内容...\n\n{}\n\n---\n\n# Skill: {}\n\n{}\n\n---\n\n用户请求: {}",
        att_text, skill.display_name, skill.instructions, skill.args
    )
} else {
    format!(
        "# Skill: {}\n\n{}\n\n---\n\n用户请求: {}",
        skill.display_name, skill.instructions, skill.args
    )
};
```

**问题**：
- ❌ **没有注入历史对话上下文**
- ❌ **没有传递之前的用户回答**（如路径、参数选择）
- ❌ **每次工具调用后的结果没有持久化到下次 LLM 调用的上下文**

### 4. Agent Loop 的记忆传递

**位置**: `core/src/agent_loop/agent_loop.rs:227-230`

```rust
// Inject initial history if provided (for cross-session context)
if let Some(history) = initial_history {
    state.history_summary = history;
}
```

**问题**：
- ✅ 机制存在，但 **skill 执行时可能没有正确使用**
- 需要检查 `conversation_histories` 是否被正确读取并传递

## 改进建议

### 🔧 立即修复 (Critical)

#### 1. 提高 Skill 执行步数限制

**文件**: `core/src/ffi/processing/skill.rs`

```rust
// 从
.with_max_steps(30)

// 改为
.with_max_steps(100)  // 或者根据 skill 复杂度动态设置
```

**理由**：
- 复杂 skill 需要更多步骤
- 100步与 Claude Code 的限制相当

#### 2. 调整 Doom Loop 检测策略

**选项 A**: 降低阈值（更激进）
```rust
.with_doom_loop_threshold(2)  // 从3降到2
```

**选项 B**: 添加"近似 doom loop"检测（推荐）
- 检测"相似但不完全相同"的工具调用
- 例如：连续3次读取**相同路径的文件**，即使参数格式略有不同

**实现位置**: `core/src/agent_loop/guards.rs`

添加新方法：
```rust
fn check_similar_tool_calls(&self) -> Option<GuardViolation> {
    // 检测工具名相同 + 关键参数相同的重复调用
    // 例如：file_ops:read 同一文件路径
}
```

#### 3. 增强上下文传递

**文件**: `core/src/ffi/processing/skill.rs`

**3a. 注入会话历史**

```rust
// 在构建 full_input 前，添加历史上下文
let history_context = if let Some(topic) = topic_id {
    let histories = conversation_histories.read().unwrap();
    if let Some(msgs) = histories.get(topic) {
        let recent_msgs = msgs.iter().rev().take(10).collect::<Vec<_>>();
        let history_summary = build_history_summary(recent_msgs);
        format!("## 之前的对话上下文\n\n{}\n\n", history_summary)
    } else {
        String::new()
    }
} else {
    String::new()
};

let full_input = format!(
    "{}# Skill: {}\n\n{}\n\n---\n\n用户请求: {}",
    history_context, skill.display_name, skill.instructions, skill.args
);
```

**3b. 缓存工具调用结果**

在 `SingleStepExecutor` 中添加结果缓存：
```rust
struct ToolCallCache {
    cache: HashMap<String, (String, Instant)>,  // (tool_name + args hash) -> (result, timestamp)
}

impl ToolCallCache {
    fn get_cached(&self, tool_name: &str, args: &Value) -> Option<String> {
        let key = self.cache_key(tool_name, args);
        if let Some((result, timestamp)) = self.cache.get(&key) {
            if timestamp.elapsed() < Duration::from_secs(60) {  // 60秒内有效
                return Some(result.clone());
            }
        }
        None
    }
}
```

### 📊 中期优化 (Important)

#### 4. 添加工具调用分析器

**新模块**: `core/src/agent_loop/analytics.rs`

```rust
pub struct ToolCallAnalytics {
    calls: Vec<(String, Value, Instant)>,  // (tool_name, args, time)
}

impl ToolCallAnalytics {
    /// 检测重复模式
    pub fn detect_repetition_pattern(&self) -> Option<RepetitionPattern> {
        // 分析最近N次调用，找出重复模式
    }

    /// 生成优化建议
    pub fn suggest_optimization(&self) -> Vec<String> {
        // "检测到多次读取相同文件，建议添加缓存"
    }
}
```

#### 5. Skill 配置文件支持

允许 skill 声明自己的执行参数：

**新文件**: `~/.claude/skills/classical-poetry/skill-config.toml`

```toml
[execution]
max_steps = 80  # 该 skill 需要更多步骤
doom_loop_threshold = 2  # 更严格的检测
stuck_threshold = 6  # 允许更多相同类型的操作

[cache]
enable_tool_cache = true
cache_ttl_seconds = 300  # 5分钟
```

#### 6. 智能步数预算

根据任务复杂度动态分配步数：

```rust
fn estimate_steps_needed(skill: &SkillInvocation, args: &str) -> usize {
    // 简单启发式：
    // - 基础步数：30
    // - 每个附件：+10
    // - args 长度 > 200字符：+20
    // - skill 类型（从 metadata 读取）：creative=+30, analytical=+20

    let base = 30;
    let attachment_bonus = if has_attachments { 10 } else { 0 };
    let complexity_bonus = if args.len() > 200 { 20 } else { 0 };
    let skill_bonus = match skill.category {
        SkillCategory::Creative => 30,
        SkillCategory::Analytical => 20,
        _ => 10,
    };

    base + attachment_bonus + complexity_bonus + skill_bonus
}
```

### 🚀 长期架构改进 (Enhancement)

#### 7. Session State 持久化

将 agent loop 的关键状态持久化到数据库：

```rust
// 每次重要操作后保存
struct PersistedLoopState {
    session_id: String,
    step_count: usize,
    user_responses: HashMap<String, String>,  // 缓存用户回答
    tool_call_cache: HashMap<String, String>,  // 工具调用结果缓存
    working_context: String,  // 当前工作上下文
}
```

#### 8. 主动上下文压缩

在步数接近限制时，主动触发压缩：

```rust
// 在 agent_loop.rs 中
if guard.remaining_steps(&state) < 10 {
    // 主动压缩，释放步数预算
    let compressed = self.compressor.aggressive_compress(&state).await?;
    state.apply_compression(compressed.summary, 0);  // 压缩所有历史

    // 重置某些 guard 计数器
    guard.reset_doom_loop_detection();
}
```

## 测试验证计划

### Phase 1: 修复步数限制
1. ✅ 修改 `skill.rs` 中的 max_steps 为 100
2. ✅ 重新测试 classical-poetry skill
3. ✅ 验证是否能完成完整流程

### Phase 2: 优化上下文传递
1. ✅ 实现历史上下文注入
2. ✅ 实现工具调用缓存
3. ✅ 测试路径识别是否不再重复询问

### Phase 3: 增强 Doom Loop 检测
1. ✅ 添加"相似调用"检测
2. ✅ 添加日志，记录每次 doom loop 检查结果
3. ✅ 验证重复读取文件能被及时捕获

## 预期效果

修复后预期：
- ✅ **步数使用**：从30步限制提升到80-100步，足够完成复杂任务
- ✅ **重复调用**：通过缓存和检测，减少至1-2次
- ✅ **用户询问**：路径和参数问题只询问一次，后续自动复用
- ✅ **执行效率**：总体 token 消耗减少 40-60%

## 相关文件清单

### 需要修改的文件
1. `core/src/ffi/processing/skill.rs` - 提高 max_steps
2. `core/src/agent_loop/guards.rs` - 增强 doom loop 检测
3. `core/src/executor/single_step.rs` - 添加工具调用缓存

### 需要新增的文件
1. `core/src/agent_loop/analytics.rs` - 工具调用分析器
2. `core/src/agent_loop/cache.rs` - 工具调用缓存模块

### 需要阅读的文件
1. `core/src/ffi/processing/agent_loop.rs` - Agent loop FFI 入口
2. `core/src/agent_loop/agent_loop.rs` - 主循环逻辑
3. `core/src/agent_loop/config.rs` - 配置定义
4. `core/src/agent_loop/state.rs` - 状态管理

---

**报告生成时间**: 2026-01-27
**分析日志**: `dilog.md` (1.0MB)
**问题级别**: 🔴 Critical - 影响复杂 skill 的可用性
