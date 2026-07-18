# 定时任务 Agent 选择器 + 删除回退 + Channel 只读显示

- **日期**: 2026-06-06
- **状态**: 设计已批准，待实现
- **范围**: Panel cron 表单 + cron executor + cron 读侧 API

## 背景与动机

定时任务（cron）表单里，绑定的 agent 现在是**手动填写的文本框**（`interfaces/webchat/src/views/cron.rs:1145`）——用户需要自己记住并敲对 agent id，易错。且当任务绑定的 agent 被删除后，executor 当前会**永久报错**（`executor.rs:120-130` 返回 `ErrorReason::Permanent("agent not found")`），任务静默死掉而非降级。

目标：
1. 文本框 → **下拉选择器**，从可用 agent 里选，默认 `main`。
2. 绑定 agent 被删除时，运行时**优雅回退到 main**（main 是内建默认 agent，不可删除，天然兜底），而非报错。
3. 顺带让用户在表单里**只读看到**该任务的真实投递通道，方便知晓结果去哪。

## 代码事实核查（决策依据）

实现前核查了相关代码，纠正了一处直觉偏差，作为设计依据：

- **agent ↔ channel 绑定**（`src/gateway/agent_env/manager_ops.rs:278-364`，表 `channel_active_agent`）：方向是 **channel → agent，多对一**——多个 channel 可绑同一 agent，但每个 channel 只能绑一个 agent。反过来 agent → channel **不是 1:1**：一个 agent 可绑 0 个或多个 channel，`get_channel_for_agent` 用 `LIMIT 1` 任取其一。
- **cron 投递不看该绑定**：executor 投递（`deliver_to_channel`）用的是 cron 任务自己存的 `source_channel_id`（NL 创建时所在聊天），全程不调用 `get_channel_for_agent`。panel 创建的 cron `source_channel_id` 为 `None` → 不投递，结果只进运行历史。
- **投递需 channel + conversation 两层**：`deliver_to_channel` 要求 `source_channel_id` 与 `source_conversation_id` 同时 `Some`，缺一即 `NotDelivered`（`executor.rs:260-261`）。

**结论**：「确定 agent 就确定 channel」在当前投递逻辑里**不成立**。因此本设计**不做 channel 选择器**，只做**只读显示真实投递目标 `source_channel_id`**，且**不改投递逻辑**。

## 已确认的范围裁剪（不做）

- ❌ channel 选择器（需要解决 conversation_id 这层，复杂度高，另行立项）。
- ❌ 删除 agent 时主动改写存量 cron 任务（执行时优雅降级已覆盖所有 agent 消失场景：删除/改名/配置损坏）。
- ❌ 改动 cron 写侧 API（`CreateCronJob`/`UpdateCronJob`）的 channel 字段——channel 只读。

## 设计

### 关键决策汇总

| 决策点 | 选择 |
|--------|------|
| 回退时机 | 执行时优雅降级（executor 运行时发现 agent 缺失才回退） |
| 回退留痕 | 记录到运行历史（`output_summary` 前置标记行，不加表字段） |
| 表单失效项显示 | 显示原 agent + 「（已删除）」标记，不静默改用户数据 |
| channel 处理 | 不做选择器；只读显示真实投递目标 `source_channel_id` |
| 显示哪个 channel | 任务真实投递目标 `source_channel_id`（非 agent 绑定推导值） |

### 1. Panel — Agent 下拉选择器

文件：`interfaces/webchat/src/views/cron.rs`

- 复用现有 `AgentsApi::list()`（`interfaces/webchat/src/api/agents.rs`），返回 `AgentsListResponse { agents: Vec<AgentSummary>, default_id }`。组件挂载/连接时拉取一次 agent 列表存入 `RwSignal<Vec<AgentSummary>>`（含 `default_id`）。
- 当前 agent 文本输入（约 `cron.rs:1144-1151`）替换为 `<select>`：
  - 选项 = 可用 agent，显示 `name`（回退 `id`），`value = id`。
  - 选中项跟随 `form_agent_id`。
- **默认值**：新建任务时 `form_agent_id` 预选 `default_id`（即 main）。当前 `cron.rs:456` 硬编码的 `agent_id: "main"` 改为读取 `default_id`（保留 `"main"` 作为列表未就绪时的兜底）。
- **失效项处理（显示 + 标记）**：编辑旧任务时若 `form_agent_id` 的值不在 `agents` 列表内（agent 已删），额外注入一个**选中**的 `<option value={id} disabled>`，文案 `"{id}{agent_deleted_suffix}"`（如 `my-old-agent（已删除）`），样式标灰，提示用户该绑定已失效、需改绑。用户不主动改则保持原值（执行时由 executor 兜底回退）。
- 移除 `cron.placeholder_agent` 的使用（文本框消失）。

### 2. Executor 优雅回退

文件：`src/tasks/cron/executor.rs:114-131`

当前逻辑（找不到即永久错误）：
```rust
let agent_id = snapshot.agent_id.as_deref().unwrap_or("main");
let agent = match registry.get(agent_id).await {
    Some(a) => a,
    None => return make_error_result(... Permanent("agent not found") ...),
};
```

改为：
```rust
let requested = snapshot.agent_id.as_deref().unwrap_or("main");
let (agent, used_agent_id, fell_back) = match registry.get(requested).await {
    Some(a) => (a, requested.to_string(), false),
    None => {
        warn!(job_id = %snapshot.id, requested, "cron agent missing, falling back to main");
        match registry.get("main").await {
            Some(a) => (a, "main".to_string(), true),
            None => return make_error_result(
                started_at,
                "main agent missing".into(),
                ErrorReason::Permanent("built-in 'main' agent is not registered".into()),
                RetryHint::permanent(),
                snapshot.trigger_source,
            ),
        }
    }
};
```

- 后续所有用到 `agent_id` 的位置（如 `SessionKey::task(...)`、metadata）统一改用 `used_agent_id`。
- 仅当 `main` 本身缺失才报错（兜底的兜底；main 内建不可删，理论不触发）。
- `fell_back` 透传到成功路径用于留痕（见 §3）。

### 3. 回退留痕

文件：`src/tasks/cron/executor.rs`（成功路径构造 output）

- `fell_back == true` 时，给最终写入 history 的 `output_summary` **前置一行**：渲染 i18n key `cron.fallback_note`（以 `requested` 为参数 `{0}`）再接 `\n`。注：留痕文案落在面向用户的运行历史里，按 panel 当前语言渲染；若 executor 侧无 i18n 上下文，则用固定中英双语模板字符串 `原 agent '{0}' 不存在，已回退到 main / Agent '{0}' not found, fell back to main`，与 §5 词条保持一致。
- **不新增** `cron_runs` 表字段（避免 schema 迁移，符合 R6）；run history UI 展示 output 即可见。
- `warn!` 日志保留。
- run 的 `status` 仍为 success（用 main 正常跑完）。

### 4. Channel 只读显示

文件：`interfaces/webchat/src/api/cron.rs`（读类型）+ `interfaces/webchat/src/views/cron.rs`（表单）

- 后端读视图 `CronJobView`（`src/tasks/cron/config.rs`）已有 `source_channel_id: Option<String>`；panel 读类型 `CronJobInfo` 当前**缺**该字段 → **新增** `source_channel_id: Option<String>`（`#[serde(default)]`），与 RPC 序列化对齐。
- 表单 agent 选择器下方加一行**只读**文本：
  - 标签 `cron.field_channel`（投递通道）。
  - 值为 `source_channel_id`；`None` 时显示 `cron.channel_none`（`无（仅记录到运行历史）`）。
- 写侧 `CreateCronJob` / `UpdateCronJob` **不动**（channel 只读，不可在 panel 设置/修改）。

### 5. i18n

`interfaces/webchat/src/i18n.rs`（或对应词条文件）新增 key（中英）：

| key | 中文 | 英文 |
|-----|------|------|
| `cron.field_channel` | 投递通道 | Delivery channel |
| `cron.channel_none` | 无（仅记录到运行历史） | None (recorded to run history only) |
| `cron.agent_deleted_suffix` | （已删除） | (deleted) |
| `cron.fallback_note` | 原 agent '{0}' 不存在，已回退到 main | Agent '{0}' not found, fell back to main |

移除 `cron.placeholder_agent` 的引用（key 本身可保留或清理）。

### 6. 测试

- **executor 单测**（`src/tasks/cron/executor.rs` tests）：
  - 新增：绑定 agent 不存在 → 用 main 跑通，且 `output_summary` 含回退标记。
  - 新增：`main` 也不存在 → 返回 permanent 错误。
  - 调整：现有断言「找不到 agent 即 permanent error」的用例，改为符合新回退语义。
- **panel**：失效项注入逻辑尽量抽成纯函数（输入 `form_agent_id` + `agents` 列表 → 是否需注入失效 option），便于 wasm 逻辑测试。

## 架构合规

- **R4（Interface 纯 I/O）**：panel 只新增一次 agent 列表读取 + 一个只读字段展示，不含业务逻辑。✅
- **R7 / R10**：回退是确定性的基础设施降级（agent 缺失 → 用兜底 agent），非 LLM 推理范畴，属脚手架。✅
- **R6（简洁）**：复用既有 `AgentsApi`，留痕走 `output_summary` 前置不加表字段，channel 只读不碰投递逻辑。✅
- **P7（防御性）**：执行时降级覆盖所有 agent 消失路径；main 缺失才报错。✅

## 实现顺序建议

1. executor 回退 + 留痕（含单测）→ verify: `cargo test -p alephcore --lib executor`
2. 读侧 API 加 `source_channel_id` → verify: `cargo check -p alephcore`
3. panel 下拉选择器 + 失效项 + channel 只读行 + i18n → verify: `just wasm` 编译通过
4. 全量 → verify: `just clippy` + 相关测试
