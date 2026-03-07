# Agent-Workspace 分层隔离设计

> 将 Agent 状态与 Workspace 内容解耦，实现灵活的多对多映射潜力。

## 背景

当前 Aleph 中 Agent 和 Workspace 是 1:1 绑定关系，所有文件（状态 + 内容）混在 `~/.aleph/workspaces/{id}/` 下。这限制了架构灵活性：

- Agent 运行时状态（sessions、auth）与用户知识内容（SOUL.md、MEMORY.md）耦合
- 无法让多个 Agent 共享同一知识库
- 删除 Agent 时必须决定是否保留知识内容
- 状态文件污染内容目录

参考 OpenClaw 的 agentDir/workspace 分离模式，结合 Aleph 现有架构，设计分层隔离方案。

## Section 1: 双目录树结构

### Agent 状态目录 (`~/.aleph/agents/{id}/`)

存放 Agent 运行时产物，生命周期与 Agent 实例绑定：

```
~/.aleph/agents/{id}/
├── sessions/              # 会话持久化
├── state.json             # 运行时状态 (last_active, loop_count, etc.)
└── auth-profiles.json     # Per-agent 认证凭据 (future)
```

### Workspace 内容目录 (`~/.aleph/workspaces/{id}/`)

存放用户创建/维护的知识内容，生命周期独立于 Agent：

```
~/.aleph/workspaces/{id}/
├── SOUL.md                # 核心人格定义
├── AGENTS.md              # Workspace 指令
├── USER.md                # 用户偏好 (future)
├── MEMORY.md              # 持久记忆笔记
├── memory/                # LanceDB 等记忆数据
└── skills/                # Per-workspace 技能 (future)
```

### 判定标准

| 问题 | 答案 → 位置 |
|------|------------|
| 删除 Agent 后还有意义吗？ | 有 → workspace，无 → agent_dir |
| 用户会手动编辑吗？ | 会 → workspace，不会 → agent_dir |
| 多个 Agent 可能共享吗？ | 可能 → workspace，不可能 → agent_dir |

## Section 2: AgentDefinition 字段扩展

### 新增字段

```rust
pub struct AgentDefinition {
    // ... existing fields ...

    /// Custom workspace path override (optional)
    /// Three-level resolution: per-agent → defaults.workspace_root/{id} → auto
    pub workspace: Option<PathBuf>,

    /// Custom agent state directory override (optional)
    /// Three-level resolution: per-agent → defaults.agents_root/{id} → auto
    pub agent_dir: Option<PathBuf>,

    /// Per-agent tool configuration (optional)
    pub tools: Option<AgentToolConfig>,
}
```

### AgentDefaults 扩展

```toml
[agents.defaults]
workspace_root = "~/.aleph/workspaces"   # 现有，不变
agents_root = "~/.aleph/agents"          # 新增
```

### 三级路径解析

1. **Per-agent override**: `agent.workspace` / `agent.agent_dir` 字段
2. **Defaults config**: `defaults.workspace_root/{id}` / `defaults.agents_root/{id}`
3. **Auto-generated**: `~/.aleph/workspaces/{id}` / `~/.aleph/agents/{id}`

## Section 3: Workspace 状态追踪

### workspace-state.json

位于 `~/.aleph/agents/{id}/workspace-state.json`（属于 Agent 状态层）：

```json
{
  "bootstrap_seeded_at": "2026-03-07T10:00:00Z",
  "onboarding_completed_at": null,
  "last_initialized_at": "2026-03-07T10:00:00Z"
}
```

### BOOTSTRAP.md 生命周期

1. **首次创建 Workspace**: `initialize_workspace()` 生成 `BOOTSTRAP.md`（引导用户完成首次配置）
2. **用户完成 Onboarding**: 用户阅读并删除 `BOOTSTRAP.md`
3. **系统记录**: 下次 resolve 检测到 `BOOTSTRAP.md` 消失，在 `workspace-state.json` 标记 `onboarding_completed_at`
4. **后续启动**: `onboarding_completed_at` 已设置，跳过 bootstrap 逻辑

## Section 4: 代码层变更映射

### AgentDefinitionResolver

```
resolve_one() 新流程:
  agent_dir = resolve_agent_dir()            →  初始化 agent 状态目录
  workspace_path = resolve_workspace_path()  →  初始化 workspace 内容目录
  加载 SOUL/AGENTS/MEMORY from workspace_path
  加载 state.json from agent_dir
```

- `ResolvedAgent` 新增 `agent_dir: PathBuf` 字段
- 新增 `resolve_agent_dir()` 方法，三级回退同 workspace
- 新增 `initialize_agent_dir()` 函数：创建 `sessions/`、写入 `state.json`
- `initialize_workspace()` 保持现有逻辑不变

### AgentManager

- 构造参数新增 `agents_root: PathBuf`（与 `workspace_root` 并列）
- `create()`: 同时创建 `agents/{id}/` 和 `workspaces/{id}/`
- `delete()`: 两个目录都移入 trash
- `rename()`: 两个目录同步重命名

### AgentInstance / AgentRegistry

- `AgentInstanceConfig` 新增 `agent_dir: PathBuf`
- Session 持久化路径从 `workspace/sessions/` → `agent_dir/sessions/`
- 运行时状态写入 `agent_dir/state.json`

### Gateway config.rs

- `AgentConfig.workspace` 语义不变（内容目录）
- 新增可选 `AgentConfig.agent_dir`（状态目录）

## Section 5: 迁移策略

### 懒迁移 (Lazy Migration)

不做批量迁移，每个 Agent 首次 resolve 时按需处理：

```
resolve_one() 内部:
  1. 计算 agent_dir = ~/.aleph/agents/{id}/
  2. 计算 workspace_path = ~/.aleph/workspaces/{id}/
  3. if agent_dir 不存在 && workspace_path/sessions/ 存在:
       → 移动 sessions/ 到 agent_dir/sessions/
       → 创建 state.json (标记 migrated_from: "unified")
  4. 正常初始化两个目录
```

### 迁移内容

| 文件/目录 | 从 (旧) | 到 (新) | 说明 |
|-----------|---------|---------|------|
| `sessions/` | `workspaces/{id}/sessions/` | `agents/{id}/sessions/` | 运行时状态 |
| `state.json` | 不存在 | `agents/{id}/state.json` | 新增 |
| SOUL.md 等 | `workspaces/{id}/` | `workspaces/{id}/` | 不动 |

### 回滚安全

- 迁移仅移动 `sessions/`，内容目录完全不动
- 旧版本会在 `workspaces/{id}/` 下重建空 `sessions/`，不丢数据
- `agents/{id}/` 对旧版本透明

## Section 6: 测试策略

### 单元测试

`agent_resolver.rs`:
- `test_resolve_creates_dual_directories` — 验证同时创建两个目录
- `test_lazy_migration_moves_sessions` — 预置 sessions/ 验证迁移
- `test_no_migration_when_agent_dir_exists` — 两目录都存在时不做迁移
- `test_workspace_content_untouched_after_migration` — 内容文件不动

`agent_manager.rs`:
- `test_create_creates_both_directories`
- `test_delete_trashes_both_directories`

### 集成测试

- 启动 server → 创建 agent → 验证双目录结构
- 模拟旧版目录布局 → 启动 → 验证懒迁移

### 不测什么

- 文件系统权限边界（靠 OS 保障）
- 跨平台路径差异（已有 `resolve_user_path` 覆盖）
