# Remove Ghost Aesthetic Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the philosophical "Ghost美学" branding with concrete product constraints (R5-R7) across core documentation.

**Architecture:** Pure documentation change. Three files: CLAUDE.md (primary AI guidance), openspec/project.md (project context for OpenSpec), constitution design doc (architectural record). No code changes.

**Tech Stack:** Markdown editing, git.

---

### Task 1: Update CLAUDE.md — Remove Ghost美学 section and update core philosophy

**Files:**
- Modify: `CLAUDE.md:31` (core philosophy paragraph)
- Modify: `CLAUDE.md:50-57` (Ghost美学 section to delete)

**Step 1: Replace core philosophy paragraph (line 31)**

Replace:
```
**Aleph 是一个完整的智能生命体。** 它拥有五层涌现的进化灵魂 (Soul)，遵循 Ghost 隐形美学 (Skin)，由 1-2-3-4 工程骨架 (Skeleton) 支撑，以 POE+DDD 思维 (Mind) 驱动决策。
```

With:
```
**Aleph 是一个完整的智能生命体。** 它拥有五层涌现的进化灵魂 (Soul)，由 1-2-3-4 工程骨架 (Skeleton) 支撑，以 POE+DDD 思维 (Mind) 驱动决策，以具体产品约束 (R1-R7) 保障实用性。
```

**Step 2: Delete Ghost美学 section (lines 50-57)**

Delete the entire block:
```markdown
### Ghost 美学 (The Skin — 皮肤)

| 原则 | 实现 |
|------|------|
| **Invisible First** | 无 Dock 图标、无常驻窗口，只有后台进程 + 菜单栏 |
| **Frictionless** | AI 来到你身边，而不是你去找 AI |
| **Native-Powered** | UI 归一化 (Leptos/WASM) 保证多端一致性，系统能力原生化 (Rust/Swift) 保证对硬件 100% 掌控 |
| **Polymorphic** | 一个灵魂，无限形态 |
```

**Step 3: Verify the edit**

Read `CLAUDE.md:29-60` to confirm Ghost section is gone and the philosophy paragraph is updated.

---

### Task 2: Update CLAUDE.md — Add R5-R7 product constraints

**Files:**
- Modify: `CLAUDE.md:163-187` (insert after R4, before the `---` separator)

**Step 1: Insert R5-R7 after R4 block (after line 186)**

After the R4 block ending with `- **原则**: Interface 层是"纯 I/O"— 输入转为 JSON-RPC 发给 Server，响应渲染给用户`, insert:

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

**Step 2: Verify the edit**

Read `CLAUDE.md:163-205` to confirm R5-R7 are properly inserted after R4.

**Step 3: Commit CLAUDE.md changes**

```bash
git add CLAUDE.md
git commit -m "docs: replace Ghost aesthetic with product constraints R5-R7 in CLAUDE.md"
```

---

### Task 3: Update openspec/project.md

**Files:**
- Modify: `openspec/project.md:5` (Purpose paragraph)
- Modify: `openspec/project.md:85-86` (Key Architectural Principles)
- Modify: `openspec/project.md:246` (Anti-Patterns)

**Step 1: Replace Purpose paragraph (line 5)**

Replace:
```
**Aleph** is a system-level AI middleware for macOS (with future Windows/Linux support) that acts as an invisible "ether" connecting user intent with AI models through a frictionless, native interface. The project embodies a "Ghost" aesthetic - no permanent windows, no dock icon, only ephemeral UI that appears at the cursor when summoned.
```

With:
```
**Aleph** is a system-level AI middleware for macOS (with future Windows/Linux support) that connects user intent with AI models through a lightweight, native interface. The application runs as a menu bar daemon (no Dock icon), with a lightweight Halo overlay as the primary quick-interaction surface. Normal windows are used when complexity demands it (settings, long conversations, debugging).
```

**Step 2: Replace Key Architectural Principles (lines 85-86)**

Replace:
```
1. **Native-First**: No webviews, no Electron, no Tauri for UI
2. **Invisible-First**: No permanent windows, Halo overlay is ephemeral
```

With:
```
1. **Native-Powered**: System capabilities in native code (Rust/Swift), UI unified via Leptos/WASM
2. **Menu Bar First**: Default presence is menu bar + Halo overlay; use normal windows when the task requires it
```

**Step 3: Replace Anti-Pattern (line 246)**

Replace:
```
- **DO NOT** create permanent GUI windows (violates "Ghost" philosophy)
```

With:
```
- **DO NOT** create unnecessary permanent GUI windows — prefer menu bar + Halo for quick interactions, but use proper windows for complex tasks (settings, conversations, debug panels)
```

**Step 4: Verify and commit**

Read `openspec/project.md:1-10,84-88,244-248` to confirm all three edits.

```bash
git add openspec/project.md
git commit -m "docs: remove Ghost aesthetic references from openspec/project.md"
```

---

### Task 4: Update constitution design doc

**Files:**
- Modify: `docs/plans/2026-02-24-1234-architecture-constitution-design.md:19` (Skin row in table)
- Modify: `docs/plans/2026-02-24-1234-architecture-constitution-design.md:25` (one-liner positioning)
- Modify: `docs/plans/2026-02-24-1234-architecture-constitution-design.md:96-102` (Ghost美学更新 section)

**Step 1: Replace Skin row in Living Being table (line 19)**

Replace:
```
| **Skin (皮肤)** | 存在状态 | Ghost 美学：AI 如何出现在用户世界里 | `### Ghost 美学 (The Skin — 皮肤)` |
```

With:
```
| **Skin (皮肤)** | 存在状态 | 产品设计约束 (R5-R7)：菜单栏优先、AI 主动到达、一核多端 | `### R5-R7 产品设计约束` |
```

**Step 2: Replace one-liner positioning (line 25)**

Replace:
```
> "Aleph 拥有五层涌现的进化灵魂，遵循 Ghost 的隐形美学。在工程上，它由 1 个核心驱动，拥有 2 种交互面，通过 3 类执行系统干涉现实，并由 4 层通讯协议编织成一个完整的智能生命体。"
```

With:
```
> "Aleph 拥有五层涌现的进化灵魂。在工程上，它由 1 个核心驱动，拥有 2 种交互面，通过 3 类执行系统干涉现实，并由 4 层通讯协议编织成一个完整的智能生命体。"
```

**Step 3: Replace Ghost美学更新 section (lines 96-102)**

Replace the entire section:
```markdown
## Ghost 美学更新

| 原则 | 旧描述 | 新描述 |
|------|--------|--------|
| **Native-First → Native-Powered** | 100% 原生代码 (Rust + Swift) | UI 归一化 (Leptos/WASM) 保证多端一致性，系统能力原生化 (Rust/Swift) 保证对硬件 100% 掌控 |

原因：Leptos/WASM 统一 UI 后，"100% 原生代码"已不准确。新描述体现了双轨策略——UI 一致性与系统能力原生化并行。
```

With:
```markdown
## 产品设计约束 (R5-R7)

原 "Ghost 美学" 概念已被拆解为具体的产品设计约束，避免哲学化表述误导设计决策：

- **R5. 菜单栏优先，按需展窗**: 默认无 Dock 图标 + 菜单栏常驻 + Halo 浮窗快捷交互。复杂场景使用正常窗口
- **R6. AI 主动到达**: 减少用户切换上下文成本，不打扰用户但不拒绝必要 UI
- **R7. 一核多端**: Rust Core 唯一大脑，Leptos/WASM 统一 UI，原生壳负责窗口和系统集成
```

**Step 4: Verify and commit**

Read the modified lines to confirm all three edits.

```bash
git add docs/plans/2026-02-24-1234-architecture-constitution-design.md
git commit -m "docs: replace Ghost aesthetic with product constraints in constitution doc"
```

---

### Task 5: Final verification

**Step 1: Search for remaining Ghost美学 references in active docs**

```bash
grep -rn "Ghost 美学\|Ghost美学\|Ghost aesthetic\|Ghost.*philosophy\|Invisible.First\|Invisible-First" CLAUDE.md openspec/project.md docs/plans/2026-02-24-1234-architecture-constitution-design.md
```

Expected: Zero matches.

**Step 2: Confirm CLAUDE.md redlines section has R1-R7**

Read `CLAUDE.md` redlines section and verify R1 through R7 are all present and properly formatted.

**Step 3: Done**

All changes complete. Historical plans and README.md are intentionally left unchanged.
