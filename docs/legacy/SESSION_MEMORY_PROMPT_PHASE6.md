# Aleph 项目会话记忆 - Phase 6 开始

**生成时间**: 2025-12-24  
**当前状态**: Phase 5 完成 ✅ → Phase 6 待开始

---

## 快速恢复上下文

### 项目简介
Aleph 是一个 macOS/Windows/Linux 的系统级 AI 中间件（Rust Core + Native UI）。用户按 Cmd+~ 选中文本，AI 处理后粘贴回去。

### Phase 5 完成成果 ✅ (2025-12-24)

**实现内容**:
- 智能路由系统（Router + RoutingRule）
- First-match-wins 路由算法
- System prompt override 支持
- 完整配置支持（RoutingRuleConfig）

**关键文件**:
- `Aether/core/src/router/mod.rs` (856 行)
- `Aether/core/src/config.rs` (添加 RoutingRuleConfig)
- `Aether/config.example.toml` (完整示例配置)

**测试**: 20 个测试全部通过 ✅

**详细文档**: `Aether/core/PHASE5_ROUTER_COMPLETION.md`

---

## Phase 6: Memory Integration（当前任务）

### 目标
将已实现的记忆模块集成到 AI 处理流程中，实现上下文感知的 AI 交互。

### 任务清单

#### Task 6.1: 集成记忆检索
- [ ] 在 `core.rs` 的处理流程中调用 `memory_store.retrieve()`
- [ ] 传递当前上下文（app_bundle_id + window_title）
- [ ] 获取相关的过去交互

#### Task 6.2: 实现提示词增强
- [ ] 检查是否已有 `memory/augmentation.rs`
- [ ] 实现 `augment_prompt(input: &str, memories: &[MemoryEntry]) -> String`
- [ ] 格式："Past Context:\n{memories}\n\nCurrent Request:\n{input}"

#### Task 6.3: 路由增强后的输入
- [ ] 将增强后的 prompt 传递给 Router
- [ ] Router 选择合适的 Provider
- [ ] Provider 处理包含上下文的完整输入

#### Task 6.4: 存储交互结果
- [ ] AI 响应后，异步存储交互（tokio::spawn）
- [ ] 存储：context + user_input + ai_response + timestamp
- [ ] 错误不阻塞主流程（仅记录日志）

#### Task 6.5: 记忆开关
- [ ] 检查 `config.memory.enabled`
- [ ] 如果禁用，跳过检索和存储
- [ ] 仍然正常路由和处理

### 关键代码位置

**记忆模块**（已实现）:
- `Aether/core/src/memory/retrieval.rs` - 检索逻辑
- `Aether/core/src/memory/ingestion.rs` - 存储逻辑
- `Aether/core/src/memory/context.rs` - 上下文捕获

**需要修改的文件**:
- `Aether/core/src/core.rs` - AlephCore 主逻辑
- `Aether/core/src/memory/augmentation.rs` - 提示词增强（可能需要创建）

### 预期流程

```
用户输入
  ↓
记忆检索 (if enabled) [<50ms]
  ↓
提示词增强 [<10ms]
  ↓
Router 路由 [<1ms]
  ↓
AI Provider 处理
  ↓
AI 响应
  ↓
记忆存储 (async, non-blocking)
  ↓
结果返回
```

### 性能目标
- 记忆检索：<50ms
- 提示词增强：<10ms
- 总额外开销：<60ms

---

## 技术栈提醒

### Rust Core
```rust
// 关键类型
pub struct AlephCore {
    // 已有字段
    pub memory_store: Option<Arc<MemoryStore>>,
    // 待添加 (Phase 7)
    // pub router: Option<Arc<Router>>,
}

// 记忆相关
pub struct MemoryEntry {
    pub user_input: String,
    pub ai_response: String,
    pub context: CapturedContext,
    pub timestamp: String,
}

pub struct CapturedContext {
    pub app_bundle_id: String,
    pub window_title: String,
    pub timestamp: String,
}
```

### 配置（已完成）
```toml
[memory]
enabled = true
max_context_items = 5
similarity_threshold = 0.7
```

---

## 开发命令

```bash
cd Aleph/core

# 运行记忆相关测试
cargo test memory --lib

# 运行所有测试
cargo test --lib

# 检查代码
cargo clippy
cargo fmt
```

---

## 下一步行动

1. 检查 `memory/augmentation.rs` 是否存在
2. 实现提示词增强逻辑
3. 在 `core.rs` 中集成记忆检索
4. 测试端到端流程
5. 验证性能目标（<60ms）

---

**参考文档**:
- `openspec/changes/integrate-ai-providers/tasks.md` - 详细任务
- `Aether/core/PHASE5_ROUTER_COMPLETION.md` - Phase 5 完成总结
- `SESSION_MEMORY_PROMPT.md` - 完整项目上下文
