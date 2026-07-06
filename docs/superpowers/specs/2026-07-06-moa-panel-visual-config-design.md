# MoA Panel 可视化配置 — 设计 (Round 4)

- **日期**: 2026-07-06
- **主题**: 给 MoA（Mixture of Agents）持续咨询加一个 Panel 可视化配置页
- **前序**: MoA 后端已成熟（Round 1/2 优化 + Round 3 修复，见 FEATURE_LOCATOR §4.9）。本轮**不动推理链路**，只补「可视化配置入口」这一块缺失。
- **参考项目对比**: hermes-agent 是 Python CLI，MoA 配置只有 `moa_config.py`（纯文件），**无任何可视化配置**。Aleph 现状 = 文件 + `moa` 对话工具。本轮目标 = 文件 + 对话工具 + **可视化面板**，三入口共享同一后端写核心。

---

## 1. 问题与目标

**问题**：当前配置 MoA 只有两条路——手改 `config.toml` 的 `[moa]`，或对 LLM 说自然语言走 `moa` 工具。Panel 里没有可视化入口，用户无法「点选已配置的模型」来组建 preset。

**目标**：在 Panel 设置区新建一个顶级 **MoA** 页，提供**完整 preset 编辑器**：
- 可视化创建 / 编辑 / 删除 preset。
- advisor 与 aggregator 模型用**选择器**，选项来自 provider 里**已配置的模型**（`providers.catalog`，凭证感知）。**不提供自由输入**，不单独配置模型。
- 同一 preset 内所有槽位（全部 advisor + aggregator）的 `(provider, model)` **全局互斥**，不能重复选择。
- 设置默认 preset、全局 `save_traces` 开关。
- 暴露全部高级参数（fanout / 超时 / max_tokens / 温度），高级项默认折叠。

**非目标**：
- 不改 MoA 推理 / fan-out / 聚合逻辑。
- 不做 preset 的会话激活（on/once/off）——激活仍走既有 chat 模型选择器（`"moa:<preset>"`）与 `moa` 工具，Round 3 已把 arm 逻辑收拢到 `activation.rs`，本轮不碰。
- 不引入独立模型配置——模型来源唯一为 `providers.catalog`。

---

## 2. 架构与红线自检

```
Panel (Leptos, 纯 I/O)                     Core (Rust)
┌─────────────────────────┐   JSON-RPC   ┌──────────────────────────────┐
│ settings/moa/           │─────────────>│ gateway/handlers/moa.rs      │
│  ├ mod.rs  预设列表卡片  │  moa.list*   │  (写方法 operator 门控)        │
│  ├ preset_editor.rs 表单 │  moa.save*   │            │                  │
│  └ 复用 providers.       │  moa.delete* │            ▼                  │
│      catalog 做模型下拉  │  moa.setDefault│ providers/moa/preset_store.rs│  ← 新共享核心
└─────────────────────────┘  moa.setSaveTraces│ build→validate→patch→reload │
         ▲                                │            ▲                  │
   api/moa.rs (webchat)                   │   builtin_tools/moa_manage.rs │  ← 改：去内联，调共享核心
                                          └──────────────────────────────┘
```

**红线自检**：
- **R4（Panel 纯 I/O）** ✅ Panel 只发 JSON-RPC、渲染响应，零业务逻辑与持久化。
- **R8 / R2（工具即一切 / UI 单源）** ✅ `moa` 对话工具**保留**；可视化页是**互补**入口，与工具共享同一写核心（正如 providers 既有工具路径也有 `generation_providers/` 可视化页）。
- **R10（薄 harness）** ✅ 零代码进 `src/harness/`，纯 gateway + panel + config。
- **R7 / P8（LLM 主权）** ✅ 只搬运配置，不替代任何推理判断。
- **熵减** ✅ `moa_manage.rs` 的 set_preset/delete_preset 从内联 patch 改为调共享核心，净删重复。

---

## 3. 组件明细

### 3.1 后端

**`src/providers/moa/preset_store.rs`（新建，~120 行）** — 共享写核心，单一真源
- 持有 `Arc<RwLock<Config>>` + `Arc<ConfigPatcher>`。
- `async fn save_preset(&self, name: &str, preset: MoaPreset, make_default: bool) -> Result<()>`：
  - 构建 scratch `MoaToml`（把 preset 放入 presets、按需设 default_preset），跑 `validation_errors()`（含新去重规则），非空即 fail-fast。
  - 通过 `ConfigPatcher::apply(PatchRequest{ section: "moa", patch, ... })` 写 `config.toml`，随后经 `config_handle` 热更新进程级 `[moa]`。
  - 逻辑 = 现 `moa_manage.rs::set_preset` 主体（~373–471）上提。
- `async fn delete_preset(&self, name: &str) -> Result<()>`：= 现 `delete_preset`（~474–556）上提，保留"删除的正好是 default 时选下一个/清空"逻辑。
- `async fn set_default(&self, name: &str) -> Result<()>`：patch `default_preset`（要求 name 存在，否则 err）。
- `async fn set_save_traces(&self, on: bool) -> Result<()>`：patch `save_traces`。
- `fn list(&self) -> MoaToml`：从 `config` 读快照（presets + default_preset + save_traces）。

**`src/config/types/moa.rs`（增强）** — `validation_errors()` 追加全局去重
- 新规则：对每个 preset，收集所有槽位（`advisors` + `aggregator`）的 `(provider, model)`（provider/model 均 trim + 大小写规范化后比较），若有重复则 push 错误 `[moa.presets.{name}] duplicate slot (provider, model) — advisors and aggregator must all be distinct`。
- 新增单测：重复 advisor 拒；aggregator==advisor 拒；全异通过。

**`src/gateway/handlers/moa.rs`（新建，~150 行）** — RPC 处理
- `moa.listPresets`（读，chat-tier）→ `{ presets: {name: MoaPreset}, default_preset: Option<String>, save_traces: bool }`。
- `moa.savePreset`（写，operator）→ 参数 `{ name, enabled, advisors: [{provider, model}], aggregator: {provider, model}, fanout, advisor_timeout_secs, advisor_max_tokens?, advisor_temperature?, aggregator_temperature?, make_default? }` → 构建 `MoaPreset` → `MoaPresetStore::save_preset`。
- `moa.deletePreset`（写，operator）→ `{ name }`。
- `moa.setDefault`（写，operator）→ `{ name }`。
- `moa.setSaveTraces`（写，operator）→ `{ on }`。
- 验证错误以结构化 `{ code, message }` 透传（照 `providers` handler 现有错误形态）。

**`src/gateway/handlers/mod.rs` + `method_authz.rs`（接线）**
- 在方法分发注册 `moa.*` 五个方法。
- 写方法（save/delete/setDefault/setSaveTraces）按 `providers.update` 同款 operator/device-tier 门控；`moa.listPresets` 保持开放读。

**`src/builtin_tools/moa_manage.rs`（改，熵减）**
- `set_preset` / `delete_preset` 删除内联 patch 块，改调 `MoaPresetStore`。
- 现有测试（`on_with_resolvable_preset_writes_sticky_session_handle` 等）作回归护栏保持绿。

### 3.2 Panel

**`interfaces/webchat/src/api/moa.rs`（新建）** — `MoaApi`
- `list_presets` / `save_preset` / `delete_preset` / `set_default` / `set_save_traces`，各包一层 `state.rpc_call("moa.*", params)`，反序列化为强类型返回。

**`.../views/settings/moa/mod.rs`（新建）** — `MoaView`
- 进入时并行拉 `moa.listPresets` + `providers.catalog`。
- 渲染 preset 卡片列表：名称、default 徽章、advisor chips、aggregator chip、enabled 状态、编辑/删除按钮。
- 顶部："新建 preset" 按钮 + 全局 `save_traces` 开关。
- 空态：无 preset → 引导新建；已配置模型 < 2 → 提示"先去 Providers 配置更多模型"并禁用新建。

**`.../views/settings/moa/preset_editor.rs`（新建）** — 编辑器表单
- 字段：名称输入；advisor 行（增删按钮），每行一个 `(provider, model)` 下拉；aggregator 下拉；高级折叠区（fanout 单选 PerIteration/UserTurn、advisor_timeout_secs 数字、advisor_max_tokens 可选、advisor_temperature 可选、aggregator_temperature 可选）；enabled 开关；设为默认复选。
- **去重选择器**：维护一个"已用 `(provider,model)`"集合，任何下拉都把已用项置灰/过滤，天然选不出重复。
- 客户端内联校验镜像服务端：名称非空、enabled 时 ≥1 advisor、全局去重、可用模型 ≥2。保存前拦截，保存失败时服务端错误内联显示不关表单。

**settings `mod.rs` / `route.rs` / 侧边栏（接线）**
- 注册 `/settings/moa` 顶级页 + 侧边栏卡片，照 `generation_providers` 现有模式。

---

## 4. 数据流

**读（进入页面）**：`MoaView` → `moa.listPresets` + `providers.catalog` → 渲染卡片 + 备好下拉选项池。

**写（保存 preset）**：
```
表单提交 → MoaApi::save_preset → moa.savePreset RPC
  → handler 构建 MoaPreset
  → MoaPresetStore::save_preset
  → validation_errors(含全局去重)  ── 不过则 fail-fast，不落盘
  → ConfigPatcher::apply(section="moa")  ── 写 config.toml
  → config_handle 热更新进程级 [moa]
  → 返回 ok → Panel 重拉 listPresets
```

**删除 / 设默认 / save_traces**：同构，各走对应 RPC → `MoaPresetStore` → patch + reload → 重拉。

---

## 5. 错误处理

| 场景 | 处理 |
|------|------|
| 空名 / enabled 无 advisor / 重复槽 / aggregator==advisor | `validation_errors` 拦截 → 结构化 error → 编辑器内联报错，不关表单，不落盘 |
| catalog 拉取失败 | 编辑器提示"无法加载已配置模型"并禁用保存 |
| 已配置模型 < 2 | 空态引导"先去 Providers 配置更多模型" |
| 删除的正好是 default | 复用 `delete_preset` 既有"选下一个/清空 default"逻辑 |
| patcher 写盘失败 | 错误透传 Panel，配置不半写 |

---

## 6. 测试计划

- **单元**：`MoaToml::validation_errors` 去重新规则（重复 advisor 拒 / aggregator==advisor 拒 / 全异通过）。
- **单元**：`MoaPresetStore` save/delete/set_default/set_save_traces 走 temp `config.toml`（复用现有 moa_manage 测试的 `create_test_patcher` 兄弟模式）。
- **回归**：`moa_manage` 工具委托共享核心后现有测试保持绿。
- **Handler**：`moa.savePreset` 参数→`MoaPreset` 映射 + 验证错误透传（照 `gateway/handlers/providers/tests.rs`）。
- **Panel**：去重选择器的"已用集合过滤"若可抽纯函数则单测；WASM E2E 较重，标注**手动走查**（诚实标注，不假装覆盖）。

---

## 7. 熵减清理点（明确标注）

- `src/builtin_tools/moa_manage.rs`：`set_preset`（~373–471）与 `delete_preset`（~474–556）内联 patch 块 → **删除**，替换为 `MoaPresetStore` 调用。净删重复，写路径单一真源。

---

## 8. 实施与分支

- 按任务执行协议：所有代码改动在**新建 worktree 分支**中进行，不直接触碰 main。
- 提交遵循 `<scope>: <description>` 英文规范（如 `gateway: add moa preset config RPCs`）。
- FEATURE_LOCATOR §4.9 在实施后补一行 Round 4 记录（可视化配置入口 + `moa.*` RPC + `preset_store.rs` 共享核心）。
