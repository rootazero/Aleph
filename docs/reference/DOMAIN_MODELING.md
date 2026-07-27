# Domain Modeling Guide

> DDD 筑底：通过领域建模规约确保核心业务逻辑的稳定性

本文档描述 Aleph 的领域建模方法，基于 Domain-Driven Design (DDD) 的核心概念，通过 Rust trait 系统实现轻量级的领域规约。

---

## 核心概念

### Entity

**定义**：具有唯一身份标识的对象，身份在状态变化中保持不变。

```rust
pub trait Entity {
    type Id: Eq + Clone + std::fmt::Display;
    fn id(&self) -> &Self::Id;
}
```

**特征**：
- 通过 `id()` 方法返回唯一标识
- 两个 Entity 相等当且仅当它们的 ID 相等
- 状态可变，但身份不变

**示例**：
```rust
impl Entity for Task {
    type Id = String;
    fn id(&self) -> &Self::Id { &self.id }
}
```

### AggregateRoot

**定义**：聚合的入口点，管理一组相关对象的一致性边界。所有对聚合内部对象的修改必须通过聚合根进行。

```rust
pub trait AggregateRoot: Entity {}
```

**特征**：
- 继承 `Entity` trait
- 作为事务边界的入口
- 负责维护聚合内部的一致性

**示例**：
```rust
impl AggregateRoot for TaskGraph {}

// TaskGraph 管理其内部的 Tasks
// 外部代码不应直接修改 Task，而应通过 TaskGraph 的方法
```

### ValueObject

**定义**：由属性定义的不可变对象，无身份标识。两个 ValueObject 相等当且仅当它们的所有属性相等。

```rust
pub trait ValueObject: Eq + Clone {}
```

**特征**：
- 不可变（immutable）
- 通过属性比较相等性
- 可自由复制和替换

**示例**：
```rust
#[derive(Eq, PartialEq, Clone)]
pub struct TaskStatus {
    pub state: TaskState,
    pub progress: f32,
}

impl ValueObject for TaskStatus {}
```

---

## 限界上下文

Aleph 的核心领域划分为以下限界上下文：

### Dispatcher Context

**职责**：DAG 调度、工具编排、任务状态管理

| 类型 | 角色 | 位置 |
|------|------|------|
| `TaskGraph` | AggregateRoot | `dispatcher/agent_types/graph.rs` |
| `Task` | Entity | `dispatcher/agent_types/task.rs` |
| `TaskStatus` | ValueObject | `dispatcher/agent_types/status.rs` |

### Memory Context

**职责**：事实存储、RAG 检索、知识压缩

| 类型 | 角色 | 位置 |
|------|------|------|
| `MemoryFact` | AggregateRoot | `memory/context.rs` |
| `ContextAnchor` | ValueObject | `memory/context.rs` |
| `FactType` | ValueObject | `memory/context.rs` |

### Intent Context

**职责**：意图检测、L1-L3 分层过滤

| 类型 | 角色 | 位置 |
|------|------|------|
| `AggregatedIntent` | ValueObject | `intent/types.rs` |
| `IntentSignal` | ValueObject | `intent/types.rs` |

---

## 使用指南

### 何时使用 Entity

当对象需要：
- 跨越多个操作保持身份
- 被其他对象引用
- 有生命周期（创建、更新、删除）

### 何时使用 AggregateRoot

当对象需要：
- 管理一组相关对象的一致性
- 作为事务边界
- 封装复杂的业务规则

### 何时使用 ValueObject

当对象：
- 只关心属性值，不关心身份
- 可以自由复制和替换
- 是不可变的

---

## 实现新的领域类型

### 步骤 1：确定角色

问自己：
1. 这个对象需要唯一身份吗？→ Entity
2. 它管理其他对象的一致性吗？→ AggregateRoot
3. 它只是一组属性的容器吗？→ ValueObject

### 步骤 2：实现 Trait

```rust
use crate::domain::{Entity, AggregateRoot, ValueObject};

// Entity 示例
impl Entity for MyEntity {
    type Id = String;
    fn id(&self) -> &Self::Id { &self.id }
}

// AggregateRoot 示例
impl AggregateRoot for MyAggregate {}

// ValueObject 示例
impl ValueObject for MyValue {}
```

### 步骤 3：添加测试

不变量测试与聚合根同文件，放在 `#[cfg(test)] mod tests`：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn my_aggregate_maintains_identity() {
        let agg = MyAggregate::new("agg-001");
        assert_eq!(agg.id(), "agg-001");
    }
}
```

跨模块的行为验证放到顶层 `tests/<name>.rs`（**必须是顶层** —— cargo 只自动发现
`tests/*.rs`，子目录需要某个 target 显式 `mod` 才会被编译）。

---

## 测试形态说明 (Historical note)

本文档此前指向 `tests/features/domain/`（Gherkin）与 `tests/specs/`（YAML +
LlmJudge）两套 BDD 设施。**两者均已于 2026-07-27 删除**：承载它们的 runner
target 早在 `6e7ab1303`（2026-05-05）就被删掉，此后 66 个 `.feature` / step /
world 文件与 `cucumber` 依赖再未参与任何编译 —— 零执行断言，且其中多数针对的是
已被红线禁止或早已移除的子系统（`dispatcher` 意图分析、lancedb 后端等）。

领域不变量现在一律用上面的普通 Rust 测试表达。

---

## 参考资料

- [Domain-Driven Design](https://martinfowler.com/bliki/DomainDrivenDesign.html) - Martin Fowler
- [Implementing Domain-Driven Design](https://www.amazon.com/Implementing-Domain-Driven-Design-Vaughn-Vernon/dp/0321834577) - Vaughn Vernon
