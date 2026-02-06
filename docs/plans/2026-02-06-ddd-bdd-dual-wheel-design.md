# DDD 筑底，BDD 定向：双轮驱动架构演进方案

> 日期：2026-02-06
> 状态：设计完成，待实施

## 概述

本方案旨在通过 **DDD（领域驱动设计）筑底** 和 **BDD（行为驱动开发）定向** 的双轮驱动策略，在不破坏 Aleph 现有成熟架构的前提下，引入领域建模规约，并利用双轨测试体系确保演进过程的行为一致性。

### 核心理念

- **DDD 筑底**：通过 Rust trait 系统引入领域建模规约，隔离 AI 的不确定性
- **BDD 定向**：通过 Gherkin + YAML Spec 双轨测试体系，将业务价值转化为可衡量的评估指标

### 关键决策

| 决策点 | 选择 | 理由 |
|--------|------|------|
| 引入方式 | 渐进式提取 | 最小侵入性，尊重 Rust 哲学 |
| Trait 粒度 | 极简 Marker Trait | 避免过度设计，允许渐进采用 |
| BDD 模式 | 双轨并行 | Gherkin 用于回归，YAML Spec 用于 AI 评估 |
| 首个试点 | Dispatcher 上下文 | TaskGraph 是最典型的聚合根 |

---

## 架构设计

### 总体架构

```
┌─────────────────────────────────────────────────────────────┐
│                    BDD 定向层 (Direction)                    │
│   Gherkin (.feature)  ←→  YAML Spec (.spec.yaml)           │
│   行为回归测试              AI 能力评估                      │
│              ↓ TestRunner 统一协调 ↓                        │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│                    DDD 筑底层 (Foundation)                   │
│   core/src/domain/                                          │
│   ├── traits.rs    (Entity, AggregateRoot, ValueObject)    │
│   ├── errors.rs    (DomainError)                           │
│   └── mod.rs       (统一导出)                               │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│                    现有模块层 (Existing)                     │
│   dispatcher/  memory/  intent/  agent_loop/  ...          │
│   (逐步实现 domain traits，保持物理位置不变)                 │
└─────────────────────────────────────────────────────────────┘
```

### 关键约束

- 不进行物理目录重组
- 不引入新的运行时依赖
- 所有变更必须通过现有 Gherkin 测试

---

## DDD 筑底：Domain Traits 设计

### 文件结构

```
core/src/domain/
├── mod.rs          # 模块入口，统一导出
├── traits.rs       # 核心 Marker Traits
└── errors.rs       # 领域错误类型（可选）
```

### 核心 Traits 定义

```rust
// core/src/domain/traits.rs

//! Domain-Driven Design marker traits for Aleph.
//!
//! These traits establish semantic contracts without imposing
//! runtime overhead. They serve as documentation and enable
//! compile-time verification of domain modeling decisions.

/// An entity has a unique identity that persists across state changes.
pub trait Entity {
    type Id: Eq + Clone + std::fmt::Debug;
    fn id(&self) -> &Self::Id;
}

/// An aggregate root is the entry point for a consistency boundary.
/// All modifications to entities within the aggregate must go through the root.
pub trait AggregateRoot: Entity {
    /// Returns true if the aggregate is in a valid state.
    fn is_consistent(&self) -> bool { true }
}

/// A value object is defined by its attributes, not identity.
/// Two value objects with the same attributes are considered equal.
pub trait ValueObject: Eq + Clone {}
```

### 设计决策

- `is_consistent()` 提供默认实现，允许渐进式采用
- 不引入 `DomainEvent` 或 `Repository`，避免过度设计
- 使用 `std::fmt::Debug` 约束 Id，便于日志和调试

---

## 首个试点：TaskGraph 实现

### 现有结构分析

基于 `core/src/dispatcher/agent_types/graph.rs`：

```rust
// 现有结构（简化）
pub struct TaskGraph {
    pub tasks: Vec<Task>,
    pub dependencies: HashMap<String, Vec<String>>,
}

pub struct Task {
    pub id: String,
    pub name: String,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub parameters: serde_json::Value,
}
```

### 实现方案

```rust
// 在 graph.rs 中添加 trait 实现
use crate::domain::{Entity, AggregateRoot};

impl Entity for TaskGraph {
    type Id = String;  // 可考虑引入 TaskGraphId newtype

    fn id(&self) -> &Self::Id {
        &self.graph_id
    }
}

impl AggregateRoot for TaskGraph {
    fn is_consistent(&self) -> bool {
        // 验证：所有依赖引用的 task 都存在
        self.dependencies.values()
            .flatten()
            .all(|dep_id| self.tasks.iter().any(|t| &t.id == dep_id))
    }
}

impl Entity for Task {
    type Id = String;
    fn id(&self) -> &Self::Id { &self.id }
}
```

### 所需变更

- 为 `TaskGraph` 添加 `graph_id: String` 字段
- 在 `mod.rs` 中导出 `domain` 模块
- 更新相关构造函数

---

## BDD 定向：双轨测试体系

### 目录结构

```
core/tests/
├── features/              # 现有 Gherkin 测试（保持不变）
│   ├── scheduler.feature
│   ├── memory.feature
│   └── ...
└── specs/                 # 新增 YAML Spec 评估集
    ├── dispatcher/
    │   ├── dag_scheduling.spec.yaml
    │   └── tool_selection.spec.yaml
    └── README.md
```

### YAML Spec 格式设计

```yaml
# core/tests/specs/dispatcher/dag_scheduling.spec.yaml
name: "DAG 调度行为规范"
version: "1.0"
context:
  description: "验证 TaskGraph 的调度决策符合预期"

scenarios:
  - name: "依赖任务按序执行"
    given:
      - task_graph:
          tasks: [A, B, C]
          dependencies: { B: [A], C: [A, B] }
    when:
      action: "execute_graph"
    then:
      - execution_order: "A before B, B before C"
      - assertion_type: "deterministic"

  - name: "并行任务同时启动"
    given:
      - task_graph:
          tasks: [A, B, C]
          dependencies: { C: [A, B] }
    when:
      action: "execute_graph"
    then:
      - parallel_execution: [A, B]
      - llm_judge:
          prompt: "验证 A 和 B 是否在 C 之前并行执行"
          criteria: "时间戳差异 < 100ms"
```

### TestRunner 统一协调

```rust
// core/src/spec_driven/runner.rs (扩展)
pub enum TestSource {
    Gherkin(PathBuf),      // .feature 文件
    YamlSpec(PathBuf),     // .spec.yaml 文件
}

pub struct UnifiedTestRunner {
    gherkin_runner: CucumberRunner,
    spec_runner: SpecDrivenWorkflow,
}
```

---

## 实施路径

### Phase 1：基础设施

| 任务 | 产出 |
|------|------|
| 创建 `core/src/domain/` 模块 | `traits.rs`, `mod.rs` |
| 为 `TaskGraph` 添加 `graph_id` 字段 | 更新构造函数和序列化 |
| 实现 `Entity` 和 `AggregateRoot` | 首个 trait 实现 |
| 运行现有 Gherkin 测试 | 确保无回归 |

### Phase 2：BDD 扩展

| 任务 | 产出 |
|------|------|
| 创建 `core/tests/specs/` 目录结构 | 目录 + README |
| 编写 `dag_scheduling.spec.yaml` | 首个 YAML Spec |
| 扩展 `spec_driven` 支持 YAML 格式 | 解析器 + 适配器 |
| 实现 `UnifiedTestRunner` | 双轨协调器 |

### Phase 3：术语对齐（持续）

| 任务 | 产出 |
|------|------|
| 更新 `CLAUDE.md` 术语表 | 统一语言定义 |
| 审查 Gherkin 场景命名 | 与 domain traits 对齐 |
| 文档化 trait 使用指南 | `docs/DOMAIN_MODELING.md` |

---

## 成功标准

- [ ] `TaskGraph` 实现 `AggregateRoot`，`is_consistent()` 验证依赖完整性
- [ ] 至少 3 个 YAML Spec 场景通过 LlmJudge 验证
- [ ] 所有现有 Gherkin 测试保持绿色
- [ ] `CLAUDE.md` 包含 Entity/AggregateRoot/ValueObject 术语定义

---

## 后续扩展

完成 Dispatcher 上下文试点后，可按以下顺序扩展：

1. **Memory 上下文**：`MemoryFact` 实现 `Entity`，RAG 检索质量评估
2. **Intent 上下文**：`AggregatedIntent` 实现 `ValueObject`，意图分类准确率评估
3. **POE 上下文**：`SuccessManifest` 实现 `AggregateRoot`，契约验证评估

---

## 参考资料

- [Agent 设计哲学](../AGENT_DESIGN_PHILOSOPHY.md)
- [POE 架构设计](./2026-02-01-poe-architecture-design.md)
- [spec_driven 模块](../../core/src/spec_driven/)
