# Aleph Architecture Activation Diagnostic Report

> Date: 2026-03-05
> Server: alephcore v0.1.0, RUST_LOG=debug
> Channel: Telegram Bot
> Model: claude-sonnet-4-20250514 (via chatgpt provider)

## Activation Matrix

| Subsystem | Task A (Q&A) | Task B (Tool) | Task C (Complex) | Status |
|-----------|:---:|:---:|:---:|--------|
| Agent Loop | **Y** | **Y** | **Y** | OK |
| Thinker | **Y** | **Y** | **Y** | OK |
| POE Lazy | **Y** | **Y** | **Y** | OK |
| Memory Read | - | **Y** | - | Partial |
| Memory Write | - | - | - | INACTIVE |
| Dispatcher DAG | - | - | - | INACTIVE |
| POE Full | - | - | - | INACTIVE |
| Swarm | - | - | - | INACTIVE |
| Resilience | - | - | - | INACTIVE |
| Group Chat | - | - | - | DISABLED |

**Y** = activated, **-** = not activated

---

## Session Summary

| Task | Session ID | Duration | Steps | Tokens | LLM Calls | Decision Pattern |
|------|-----------|----------|-------|--------|-----------|------------------|
| A (Q&A) | `67cf4618...` | 19s | 2 | 3,223 | 3 | ask_user -> tool -> complete |
| B (Tool) | `7f8b40ff...` | 24s | 4 | 14,930 | 5 | tool x4 -> complete |
| C (Complex) | `0c6efbe7...` | 36s | 4 | 17,048 | 5 | tool x4 -> complete |

---

## Evidence

### 1. Agent Loop (ACTIVATED - all 3 tasks)

```
12:24:46 agent loop initialized, session_id=67cf4618...
12:24:46 agent loop entered first execution cycle
12:25:05 agent loop session ended, result=completed, steps=2
12:25:08 agent loop initialized, session_id=7f8b40ff...
12:25:32 agent loop session ended, result=completed, steps=4
12:25:34 agent loop initialized, session_id=0c6efbe7...
12:26:10 agent loop session ended, result=completed, steps=4
```

Agent Loop 的 OTAF 循环确实在运行，每个 session 执行了 2-4 个 step。但注意：即使 Task C（复杂多步任务）也只有 4 个 step，说明任务没有被分解为子任务。

### 2. Thinker (ACTIVATED - all 3 tasks)

```
13 provider_selected events, 13 response_completed events
All using: model=claude-sonnet-4-20250514, provider=chatgpt
Decision types: 9 tool, 3 complete, 1 ask_user
```

Thinker 正常工作。所有任务都通过同一个 model/provider，没有 model routing 差异化。

### 3. POE Lazy Evaluator (ACTIVATED - all 3 tasks)

```
12:25:03 validation_triggered, tools_invoked=1, retries_remaining=2  -> passed=true
12:25:30 validation_triggered, tools_invoked=4, retries_remaining=2  -> passed=true
12:26:08 validation_triggered, tools_invoked=4, retries_remaining=2  -> passed=true
```

**这是最积极的发现。** POE Lazy Evaluator 在每个 session 的 completion 阶段都被触发，验证了工具调用是否真实发生、输出是否存在幻觉。所有 3 次都通过验证。说明 POE 架构的"轻量级守护"层确实在运行时保护质量。

### 4. Memory (PARTIAL - only read, only Task B)

```
12:24:44 store_initialized, backend=lancedb, db_path=~/.aleph/data/memory.lance
12:25:14 first_read, table=facts, dim=1536, limit=10  (during Task B)
```

Memory Store 初始化成功，Task B 触发了向量搜索（1536 维 embedding, limit=10）。但：
- **没有 first_write 事件** — 没有任何 session 向记忆系统写入新事实
- Task A 和 Task C 甚至没有触发 memory read

### 5. Dispatcher DAG (NOT ACTIVATED)

零 dispatcher 探针日志。Telegram teloxide 的 "dispatcher" 是日志中唯一的相关词，与我们的 DAG Dispatcher 无关。

**所有任务都在 Agent Loop 内以单线程 think-act 循环完成，没有任何任务被分解为 DAG 图。**

### 6. POE Full Manager (NOT ACTIVATED)

零 `subsystem=poe` 探针日志。Full POE 的 P->O->E 循环从未启动。

### 7. Swarm Coordinator (NOT ACTIVATED)

零 `subsystem=swarm` 探针日志。没有任何 agent 事件被发布到 message bus，没有 context injection。

### 8. Resilience / StateDatabase (NOT ACTIVATED)

零 `subsystem=resilience` 探针日志。StateDatabase 没有被初始化，没有事件被持久化。

### 9. Group Chat (DISABLED)

启动日志明确显示：`Group Chat: Disabled (requires ANTHROPIC_API_KEY or OPENAI_API_KEY)`

---

## Diagnostic Conclusion

### Verdict: PARTIALLY UTILIZED

Aleph 当前的运行时行为是一个 **增强型流处理器**，而非设计中的多层智能体系统：

```
User Message (Telegram)
    |
    v
Agent Loop (OTAF: 2-4 steps)
    |
    v
Thinker (LLM call, always same model)
    |
    v
Tool Execution (sequential, within Agent Loop)
    |
    v
POE Lazy Validation (completion quality check)
    |
    v
Response
```

**被利用的架构 (3/9)**:
- Agent Loop: 基础 OTAF 循环正常
- Thinker: LLM 交互正常
- POE Lazy: 轻量级质量验证正常

**未被利用的架构 (6/9)**:
- Dispatcher DAG: 任务从未被分解为多步 DAG
- POE Full: 从未创建 SuccessManifest 或进入 P->O->E 循环
- Swarm: 从未发布/消费 agent 事件
- Resilience: StateDatabase 从未初始化
- Memory Write: 从未向记忆系统写入新事实
- Group Chat: 配置缺失导致禁用

---

## Improvement Suggestions

### P0: Critical (架构核心路径未被触发)

#### 1. Dispatcher DAG 从未被调用

**原因**: Agent Loop 直接通过 Thinker 做 tool call，没有经过 Dispatcher 的任务规划和 DAG 调度。当前所有任务都在 Agent Loop 的 think-act 循环内完成，相当于 Dispatcher 被完全旁路了。

**修复方向**:
- 检查 Agent Loop 中是否有条件判断来决定"什么时候走 Dispatcher"
- 需要一个**任务复杂度评估器**：当 LLM 判断任务需要多步骤时，应该走 Dispatcher 分解为 DAG，而非在 Agent Loop 内顺序执行
- 关键文件: `core/src/agent_loop/agent_loop.rs` — 在 Decision::UseTool 处理逻辑中，检查是否应该 delegate 给 Dispatcher

#### 2. POE Full Manager 从未被触发

**原因**: Full POE 需要外部调用 `PoeManager::execute(PoeTask)` 并提供 SuccessManifest。当前没有任何入口代码为任务创建 manifest 并调用 POE Manager。

**修复方向**:
- 在 Agent Loop 或 Dispatcher 中加入"POE 升级"逻辑：当任务匹配特定模式（如"生成报告"、"分析数据"）时，自动创建 SuccessManifest 并通过 POE Manager 执行
- 或者通过 Gateway RPC 暴露 POE 任务接口，让外部可以显式提交 POE 任务
- 关键文件: `core/src/poe/manager.rs`, 需要新增调用入口

#### 3. Swarm 从未被使用

**原因**: SwarmCoordinator 需要在 Agent Loop 中被注册（`with_swarm_coordinator()`），并且需要多个 agent 同时运行才能体现价值。当前所有任务都是单 agent 处理。

**修复方向**:
- 确认 server 启动时是否创建了 SwarmCoordinator 实例并注入到 Agent Loop
- 即使单 agent，event publishing 也应该工作（为未来多 agent 积累数据）
- 关键文件: `core/src/bin/aleph/commands/start/mod.rs` — 检查 SwarmCoordinator 的初始化和注入

### P1: Important (功能可用但未充分利用)

#### 4. Memory 只读不写

**原因**: Memory 的 vector_search 被触发（Task B），但没有任何 session 后的事实提取和存储。对话内容没有被学习。

**修复方向**:
- 在 session 结束后（Agent Loop 的 session_completed 阶段），自动提取关键事实并写入 MemoryStore
- 关键文件: `core/src/agent_loop/agent_loop.rs` session 结束逻辑, `core/src/memory/` 事实提取

#### 5. Resilience StateDatabase 未初始化

**原因**: StateDatabase 需要显式创建和初始化。可能 server 启动流程中没有调用 `StateDatabase::new()`。

**修复方向**:
- 确认 server 启动时是否创建了 StateDatabase
- 如果有创建但探针没触发，检查是否使用了不同的初始化路径
- 关键文件: `core/src/resilience/database/state_database.rs`, server 启动代码

### P2: Expected (符合预期的未激活)

#### 6. Group Chat 因配置缺失而禁用

**修复**: 在 `config.toml` 中配置 `ANTHROPIC_API_KEY` 或 `OPENAI_API_KEY`。这是配置问题，非架构问题。

---

## Overall Assessment

Aleph 的代码库拥有约 60,000+ 行实现精良的架构代码（POE、DAG Scheduler、Swarm、Resilience），但运行时只有最基础的管道在工作。核心问题是 **缺少"调度决策层"**：

> **Agent Loop 不知道什么时候应该把任务交给 Dispatcher，什么时候应该升级为 Full POE，什么时候应该启动 Swarm 协作。**

当前的 Agent Loop 是一个"万能打工者"——收到任务就自己 think-act 循环直到完成，从不请求帮助。这导致了大量精心设计的高级架构成为"死代码"。

**最高优先级的改进是引入任务路由决策**：根据任务复杂度、类型和约束，决定走基础 Agent Loop 还是走 Dispatcher DAG + POE Full 路径。
