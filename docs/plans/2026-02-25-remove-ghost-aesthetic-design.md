# 去名存实：移除 Ghost 美学概念

> Date: 2026-02-25
> Status: Approved

## Problem

Ghost 美学（Ghost in the Shell 美学）作为 Aleph 的产品设计哲学，其四个原则（Invisible First、Frictionless、Native-Powered、Polymorphic）本质上是好的 UX 原则。但"Ghost"这个名字和攻壳机动队的哲学包装，导致 AI 助手在设计讨论中：

1. **Halo-only 极端化**：因为 Invisible First，被引导设计成完全无常驻窗口，所有交互只通过 Halo 浮层，导致复杂场景（设置、长对话）体验差
2. **名字/概念误导**："Ghost"让 AI 往哲学和攻壳机动队方向跑偏，玄学意义大于实际项目指导意义
3. **过度追求隐形**：在性能、架构等非 UI 领域，"轻量""无痕"理念被过度延伸，导致该做的功能不敢做

## Decision

**方案 A：去名存实** — 删除"Ghost 美学"这个品牌概念，把四个原则拆解为具体的产品约束规则。

选择理由：
- 问题根源是"命名引发过度解读"，最有效解法是不要有可被解读的名字
- 四个原则本属不同层面（UI 形态、交互哲学、技术栈、架构），没必要强行绑在一个品牌下
- 变成具体规则后，AI 只能照着执行，没有"解读"空间

## Scope

改动 CLAUDE.md + openspec/project.md + 宪法文档。历史 plans 文档保留不动。

## Changes

### 1. CLAUDE.md

**删除**：
- "Ghost 美学 (The Skin — 皮肤)" 整个表格和标题
- 核心哲学开头的 "遵循 Ghost 隐形美学 (Skin)"

**新增**（架构红线部分，R4 之后）：

```markdown
### R5. 菜单栏优先，按需展窗 (Menu Bar First)

- **默认形态**: macOS 无 Dock 图标，菜单栏常驻，Halo 浮窗为主要快捷交互入口
- **允许窗口**: 复杂场景（设置、长对话、调试面板）应使用正常窗口，不要为"隐形"牺牲可用性
- **原则**: 轻量入口 + 按需展开，而非"绝对无窗口"

### R6. AI 主动到达 (AI Comes to You)

- **原则**: 减少用户切换上下文的成本，AI 尽量在用户当前工作环境中提供帮助
- **实现**: Halo 浮窗、通知、内联建议等
- **边界**: 不打扰用户 (不抢焦点、不弹模态对话框)，但不要因此拒绝提供必要的 UI

### R7. 一核多端 (One Core, Many Shells)

- **原则**: Rust Core 是唯一大脑，UI 通过 Leptos/WASM 统一，原生壳只负责窗口容器和系统集成
- **备注**: 这已在 R1 和 R2 中体现，此条作为产品层面的重申
```

**修改核心哲学段落**：

> **Aleph 是一个完整的智能生命体。** 它拥有五层涌现的进化灵魂 (Soul)，由 1-2-3-4 工程骨架 (Skeleton) 支撑，以 POE+DDD 思维 (Mind) 驱动决策，以具体产品约束 (R1-R7) 保障实用性。

### 2. openspec/project.md

**Purpose 段落**（第 5 行）：
- 删除 "Ghost" aesthetic 引用
- 改为具体描述：menu bar daemon + Halo overlay + normal windows when needed

**Key Architectural Principles**（第 85-86 行）：
- `Native-First` → `Native-Powered`
- `Invisible-First` → `Menu Bar First`

**Anti-Patterns**（第 246 行）：
- 删除 "violates 'Ghost' philosophy"
- 改为具体指导：prefer menu bar + Halo for quick interactions, use proper windows for complex tasks

### 3. 宪法文档（1234-architecture-constitution-design.md）

**Living Being 表格 Skin 行**：
- 删除 "Ghost 美学" 引用
- 改为 "产品设计约束 (R5-R7)"

**一句话定位**：
- 删除 "遵循 Ghost 的隐形美学"

**Ghost 美学更新章节**：
- 替换为 "产品设计约束 (R5-R7)" 章节，解释原 Ghost 美学已被拆解为具体约束

## What Stays

- README.md 中的攻壳机动队引用（项目精神象征，不影响 AI 决策）
- 已实现的产品决策（无 Dock 图标、菜单栏常驻、Halo 浮窗）
- 历史 plans 文档中的 Ghost 引用（历史记录，不再影响新设计）
- `button.rs` 中的 Ghost button variant（UI 组件命名，与美学概念无关）
