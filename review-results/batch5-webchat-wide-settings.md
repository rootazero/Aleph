# 静态审查报告：webchat-wide-settings

## 审查单元

- **单元名**：`webchat-wide-settings`
- **路径**：`interfaces/webchat/src/platform/wide/views/settings/`
- **关注点**：API key/secret 处理、provider 配置、不受信输入
- **代码基线**：`/tmp/aleph-review-batch-5`（git worktree，与 main 一致）
- **审查方式**：全量静态阅读，未运行 cargo check/build/clippy，未做 git 操作

## 统计

| 项目 | 数值 |
|------|------|
| 文件数 | 56 |
| 代码行数（LOC） | 22,271 |
| Critical | 0 |
| High | 8 |
| Medium | 6 |
| Low | 7 |

## 发现列表（按严重级排序）

### High

#### H1 `security/pii_rules.rs:188-196` — 自定义 PII 规则本地快照与异步加载竞态，可能导致规则被空列表覆盖

**描述**：
`CustomPiiRulesSection` 在组件构造时把 `config.get()` 的 `custom_pii_rules` 一次性快照到本地 `custom_rules` signal（第 188 行）。父组件 `SecurityView` 初始 `config` 为 `None`，加载完成前用户若点击 Apply，`save` 闭包会把空列表写回服务端（第 191-196 行）。即使加载完成，本地 signal 也不会随 `config` 更新，存在覆盖其他客户端/CLI 写入规则的风险。

**建议修法**：
- 用 `Effect` 将 `config` 的 `custom_pii_rules` 同步到本地 `custom_rules`；或
- 取消本地快照，直接对 `config` 内部的规则列表做原地编辑。

---

#### H2 `search.rs:651-657` — Test 成功后刷新 config 会清空 API key 输入框，导致刚测试通过的 key 在 Save 时被静默丢弃

**描述**：
`on_test` 在测试成功后调用 `SearchConfigApi::get` 并把结果 `config.set(new_cfg)`（第 655-656 行）。而 `Effect`（第 524-553 行）订阅了 `config`，每次 `config` 变化都会把 `form_api_key` 重置为空字符串（第 535 行）。用户常见的“Test 通过后点 Save”流程中，`build_backends` 收到空 `api_key`，按 `api_key: None`（保留旧 key）处理，新输入的 key 永远不会被持久化，且 UI 无任何提示。

**建议修法**：
- 测试成功后不要整体替换 `config`，仅更新 `verified` 等必要字段；或
- 在 `config.set(new_cfg)` 前保存 `form_api_key`，刷新后回填。

---

#### H3 `mcp.rs:376-390` — 编辑 MCP server 时会将 redacted/空的 env value 写回服务端，可能覆盖真实密钥

**描述**：
`EditMcpServerDialog::handle_save` 遍历 `env_rows`，把所有非空 key 的 `row.value.get()` 直接写入 `env_map`（第 376-384 行）。编辑时从服务端加载的 env value 注释写明是 *“loaded with its value redacted”*，且 UI placeholder 提示“saved — blank keeps it”，但代码没有“value 为空或仍是 redacted 占位符则跳过”的逻辑。若服务端 `mcp update` 是整表覆盖，编辑一次非 secret 字段就可能把 secret 改成占位符/空字符串。

**建议修法**：
- 加载时在 `EnvRow` 中标记 value 是否为 redacted；
- 保存时跳过 redacted 或空 value 的 secret key，或改用 patch 语义只发送修改过的键。

---

#### H4 `generation_providers/settings_panel.rs:30-56` — 配置加载失败吞错并展示默认值，Save 会用默认值覆盖服务端真实配置

**描述**：
`GenerationSettingsPanel` 初始 `config` 为硬编码默认值（`output_dir: String::new()` 等，第 15-24 行）。`GET` 失败时仅 `loading.set(false)`（第 36-38 行），随后 `Effect`（第 48-56 行）把默认值同步到 `output_dir`、`auto_paste`、`bg_threshold`、`smart_routing` 等表单 signal。用户此时点击 Save，`config.get()` 取到的是默认值，并把它们写回服务端。

**建议修法**：
- `GET` 失败时设置 `error` 并禁用保存按钮，不要渲染可编辑表单；或
- 加载成功前把表单控件设为 disabled。

---

#### H5 `browser.rs:60-141` — 配置加载失败后仍渲染可编辑 section 且改动即自动保存，会覆盖服务端真实浏览器配置

**描述**：
`BrowserView` 初始 `config` 为硬编码默认值（`headless: true`、`block_private: true`、空域名列表等，第 60-69 行）。`GET` 失败时只显示 info/danger banner（第 112-133 行），但继续渲染 `DefaultModeSection`、`EngineSection`、`DevToolsSection`、`SecuritySection`（第 135-138 行）。这些 section 的 radio/toggle/textarea 都绑定 `on:change` 并立即调用 `save_fn` 自动保存（如第 177-178、243-244、273-274、289-292、399-400、429-432、447-449 行）。用户一旦交互，就会用默认值覆盖服务端真实配置（包括 `blocked_domains`、`allowed_domains` 等安全相关字段）。

**建议修法**：
- 加载失败时隐藏所有可编辑 section，仅显示错误与重试按钮；或
- 加载成功前禁用所有控件。

---

#### H6 `routing_rules.rs:251-274` — 保存时硬编码丢弃 `strip_prefix/intent_type/preferred_model/icon`，会静默抹掉服务端字段

**描述**：
`on_save` 构造 `RoutingRuleConfig` 时，将 `strip_prefix`、`intent_type`、`preferred_model`、`icon` 全部设为 `None`（第 270-273 行）。如果服务端规则原本携带这些字段，用户仅编辑 regex/provider/system_prompt 后再保存，就会把这些字段静默删除。

**建议修法**：
- 编辑表单应展示并回写这些字段；或
- 保存前读取原规则，仅覆盖用户可编辑字段，保留其余字段。

---

#### H7 `routing_rules.rs:45-59` + `220-238` — 实时重载规则列表与按下标选择竞态，可能编辑/删除错规则

**描述**：
- 组件订阅 `config.changed` 事件，只要 `routing_rules` section 变化就重新 `RoutingRulesApi::list` 并 `rules.set(list)`（第 45-59 行）；
- 左侧列表和右侧编辑器都用 `Option<usize>` 作为选择键（第 23 行、第 220-238 行）。

当另一客户端/CLI 增删或重排规则后，本地 `rules` 列表刷新，原 `selected` 下标可能指向另一条规则，此时表单会刷成别人的数据，用户保存或删除会作用到错误的规则上。

**建议修法**：
- 用规则唯一 ID（如 uuid 或持久化 name）代替 `Vec` 下标作为选择键；
- 重载后若当前选择 ID 仍存在，则保持选择并重新定位下标。

---

#### H8 `skills.rs:814-827,920-928` — skill `homepage` 未经 scheme 校验直接渲染为 `href`，且 `target="_blank"` 缺少 `rel`

**描述**：
`SkillStatusEntry.homepage` 来自 `skills.status` RPC 返回的 skill 元数据（第 40 行）。skill 可通过 `skills.install` 从任意 URL 安装，因此该字段属于半受信输入。代码在 API key 区域（第 814-827 行）和 Info 区域（第 920-928 行）直接把 `homepage` 作为 `<a href=hp target="_blank">` 渲染，未限制 scheme。`javascript:` 等恶意 URL 可点击执行，造成 XSS；同时缺少 `rel="noopener noreferrer"`，存在 tabnabbing/钓鱼风险。

**建议修法**：
- 渲染前校验 scheme 白名单（仅 `http`/`https`），非白名单则不渲染链接或显示为纯文本；
- 所有 `target="_blank"` 链接补 `rel="noopener noreferrer"`。

---

### Medium

#### M1 `plugins.rs:304-324` — Install 插件 source 选择器是死状态，未发送给服务端

**描述**：
`InstallPluginDialog` 维护 `source` signal（`git`/`zip`/`local`，第 304 行），但 `handle_install` 只把 `url` 发给 `plugins.install` RPC（第 317-324 行），`source` 未被使用。用户选择 ZIP/Local 与 git 发出的请求完全相同，UI 行为与控制不符。

**建议修法**：
- 在 RPC 参数中加入 `source` 字段；或
- 若服务端只根据 URL 推断，移除该选择器以避免误导。

---

#### M2 `channels/discord.rs:100-102` — Discord 视图硬编码 `channel_id="discord-default"`，多实例选择失效

**描述**：
`DiscordChannelView` 固定 `let channel_id = "discord-default";`（第 101 行）。`platform_page.rs:308-309` 在渲染 `<DiscordChannelView />` 时不传递 `instance_id`，导致侧栏选中哪个 Discord 实例都操作同一个固定 ID，多实例功能对 Discord 实际失效。

**建议修法**：
- 将 `instance_id` 作为 prop 传入 `DiscordChannelView` 并使用它发起 RPC；或
- 若 Discord 明确只支持单实例，应在平台页禁止创建多个 Discord 实例。

---

#### M3 `search.rs:604` — 保存后本地 backend `has_api_key` 被硬编码为 `false`

**描述**：
`build_backends` 在 push 新/更新 backend 时固定 `has_api_key: false`（第 604 行）。保存成功后本地 `config.set(cfg)`（第 705 行），UI 会立即显示“未配置 key”，直到下次整页加载才恢复。同时 `api_key: Some(api_key)` 在更新成功前会短暂以明文形式驻留在前端 `config` signal 内存中。

**建议修法**：
- 保存成功后重新 `get` 配置以刷新 `has_api_key` 等服务端权威字段；
- 保存请求发送后立即清空 `api_key` 字段（Fetch 区域已采用 `api_key: None // never re-send`，应统一）。

---

#### M4 `behavior.rs:56-87` — 配置加载失败后仍渲染可编辑区域，手动 Save 可覆盖服务端配置

**描述**：
与 `browser.rs` 同模式但为手动保存：`BehaviorView` 初始 `config` 为硬编码默认值（`output_mode: "typewriter"`、`typing_speed: 100`，第 19-22 行）。`GET` 失败时 `loading=false` 后仍然渲染 `OutputModeSection` 和 `TypingSpeedSection`（第 57、83-84 行），用户点击 Save 会把默认值写回服务端。

**建议修法**：
- 加载失败时隐藏可编辑 section 或禁用保存按钮。

---

#### M5 `channels/config_template.rs:631-641` — TagList 保存时把字符串 tag 自动转 `i64`，导致前导零丢失、类型漂移

**描述**：
`TagList` 保存时遍历 tags，对可解析为 `i64` 的值调用 `Value::Number(n.into())`，否则保留字符串（第 631-641 行）。这会丢失 `"0123"` 的前导零，且同一字段在不同内容下类型不一致（字符串/数字），可能与各渠道服务端 schema 预期不符。

**建议修法**：
- 按渠道 schema 强类型处理，或在 UI 层统一保持字符串类型由服务端解析。

---

#### M6 `security/shell.rs:53,56` / `security/secrets.rs:46,78` / `security/pii_rules.rs:42` 等 — 多处使用 `Vec::remove(index)` 无越界保护

**描述**：
删除列表项时直接用 `cfg.xxx.remove(index)`，索引来自渲染时的 `enumerate` 快照。正常交互安全，但若用户快速连点或 config signal 被其他 section/客户端并发改写，闭包晚于列表变化触发时可能 panic。

**建议修法**：
- 统一改为 `retain` 或先 `get_mut` 再删除，并对删除失败给出提示。

---

### Low

#### L1 `providers/detail_panel.rs:315` — 生产代码使用 `sel.unwrap()`

**描述**：
虽然前置有 `if sel.is_none() { return ... }` 守卫，不会 panic，但违反项目“生产代码禁止 `unwrap()`”的风格红线。

**建议修法**：
- 改为 `if let Some(sel) = selected.get() { ... }`。

---

#### L2 `network/cluster.rs:252` — `Show` fallback 内使用 `enroll_result.get().unwrap()`

**描述**：
fallback 仅在 `enroll_result.get().is_none()` 为 false 时渲染，逻辑安全，但仍是生产代码中的 `unwrap()`。

**建议修法**：
- 用 `if let Some(r) = enroll_result.get()` 重构。

---

#### L3 `general.rs:91` / `behavior.rs:61` / `browser.rs:113` — 用错误消息子串判断错误类型

**描述**：
代码通过 `e.contains("Send failed") || e.contains("Failed to load")` 等字符串匹配来决定显示 info 还是 danger banner。后端错误文案一旦变化，分类即失效；同时包含这些子串的真实业务错误会被误降级为 info。

**建议修法**：
- 使用结构化错误类型或错误码；或
- 至少把分类逻辑收敛到一个地方并加注释说明依赖文案。

---

#### L4 `network/cluster.rs:17-19` — `join_command` 硬编码 `ws://`

**描述**：
```rust
fn join_command(host: &str, node_name: &str) -> String {
    format!("aleph-server node --center ws://{host} --name {node_name}")
}
```
若 Panel 通过 `https` 访问，给用户的命令仍是明文 `ws://`，在浏览器混合内容策略下可能无法连接，且存在明文传输风险。

**建议修法**：
- 根据 `location.protocol` 选择 `wss://`/`ws://`。

---

#### L5 `search.rs:1757-1766` / `execution.rs:121,146` — 数字输入越界/解析失败无前端校验

**描述**：
- `FetchProvidersSection` 的 timeout 输入框虽有 `min="5" max="300"`，但 `on:input` 仅做 `parse::<u64>()` 不夹取（第 1757-1766 行）；
- `execution.rs` 解析失败用 `unwrap_or(172_800)`/`unwrap_or(200)` 静默回退默认值（第 121、146 行）。

用户可能保存越界或错误值，且得不到反馈。

**建议修法**：
- 在 `on:input`/`on:change` 中校验范围并提示；解析失败时设置输入错误而非静默回退。

---

#### L6 `desktop_autostart.rs:91-101` — 切换失败时若重读也失败则状态不回滚且无错误提示

**描述**：
```rust
if set_autostart(want).await.is_err() {
    if let Ok(actual) = get_autostart().await { set_enabled.set(actual); }
}
```
若 `set_autostart` 和 `get_autostart` 都失败，复选框停留在错误状态，且用户看不到任何错误。

**建议修法**：
- 重读失败时保留/显示 `set_autostart` 的错误信息。

---

#### L7 `acp_harnesses/mod.rs:56-74,91-93` — RPC 错误被静默吞掉并置空列表，注释“Revert on failure”无实现

**描述**：
- 加载失败直接 `preset_metas.set(vec![])` / `harnesses.set(vec![])`，用户无法区分“无数据”与“加载失败”；
- 启用/禁用失败分支只有注释 `// Revert on failure`，没有实际回滚逻辑。

**建议修法**：
- 把错误写入 `error` signal；失败时不修改本地状态或立即回滚。

---

## 架构红线合规快照

| 红线 | 结论 | 说明 |
|------|------|------|
| R1 | ✅ | 未发现 settings 视图调用平台原生 API，全部通过 JSON-RPC 与 core 交互。 |
| R2 | ✅ | 复杂业务 UI 均在 Leptos/WASM 中实现，原生 shell 未参与。 |
| R3 | ✅ | 未在 settings 视图中引入新的重依赖。 |
| R4 | ⚠️ | 大部分视图是 signal ↔ RPC 的纯 I/O，但存在越界：**a)** `security/pii_rules.rs` 在 UI 层对 config 做本地快照并写回；**b)** `routing_rules.rs` 在前端构造 `RoutingRuleConfig` 并丢弃服务端字段；**c)** `skills.rs:936` 用展示字符串 `"Bundled"` 判定能否删除（业务规则应属 core）；**d)** 多处用错误文案子串做错误分类。 |
| R7 | ✅ | Rust Core 是唯一大脑，所有持久化操作都经 RPC 发给 core/gateway。 |
| R8 | ✅ | 正则仅用于 PII 规则、routing rules 等机器格式校验，未用于意图解析。 |
| R9 | ✅ | 可配置项均通过 settings 工具界面暴露。 |
| R10 | ✅ | 未观察到 settings 视图引入不必要的中间层逻辑。 |

## 其它说明

- 未发现 API key/secret 被明文回显、写入 `localStorage`/`URL` 或打印到日志；密钥处理整体采用“空值=保留现有密钥/只回传 `has_*` 标志”的策略，符合预期。
- 未发现 `inner_html`/`eval` 被用于注入不受信输入；`inner_html` 仅用于内置 SVG 图标和 qrcode 生成的 SVG。
- 生产代码中仍有少量 `unwrap()`（`providers/detail_panel.rs:315`、`network/cluster.rs:252`），虽逻辑上有守卫，但违反项目风格红线，建议清理。
