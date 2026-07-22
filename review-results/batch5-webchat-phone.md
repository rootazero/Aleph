# 静态审查报告 — webchat-phone（移动端布局）

- 审查单元：`webchat-phone`
- 路径：`interfaces/webchat/src/platform/phone/` + `interfaces/webchat/src/platform/tablet/`
- 审查日期：2026-07-22（基于 /tmp/aleph-review-batch-5 worktree，与 main 一致）
- 审查方式：无 diff 全量静态阅读；rust-doctor 无该路径诊断，全部结论为人工读码确认。

## 统计

- 文件数：30（phone 29 + tablet 1）
- 总行数：3969（含测试与注释）
- 最大文件：`settings/model_route.rs` 367 行 —— 无超过 500 行的文件
- `tablet/mod.rs` 为 6 行文档桩（注释明确"screens added in a later phase"），无实际代码。

## 发现列表（按严重级排序）

### Critical
无。

### High
无。

### Medium

**1. `interfaces/webchat/src/platform/phone/chat/mod.rs:31-49` — stream.* 订阅 5 秒放弃且重连后不重新订阅**
`PhoneChat` 挂载时轮询 `is_connected` 最多 ~5s（50×100ms），超时后仍调用 `subscribe_topic("stream.*")`，失败仅打 console 错误，之后不再重试；且整个订阅逻辑只在挂载时跑一次，WS 断线重连后也不会重新向 Gateway 注册 topic 转发。移动网络下首连 >5s 或中途断连都很常见，后果是 chat 流式事件（含 `stream.ask_user` 一次性推送）静默丢失，聊天界面表现为"发了没反应"，直到用户切走再切回 tab 触发重挂载。该模式是从 wide `views/chat/view.rs:71-81` 原样复制的（两端同病），但在移动端被显著放大。
建议：改用跟踪 `is_connected` 的 `Effect`（false→true 时订阅），与本单元其它屏幕（history/providers/embeddings/memory/agents 的 connect-gated loader）保持一致，天然覆盖慢首连与重连两种场景。

**2. `interfaces/webchat/src/platform/phone/settings/model_route.rs:59-82` — 每次重连都重载表单，丢弃未保存修改**
加载逻辑包在 `Effect` 里跟踪 `is_connected`：socket 每次重连都会重新 `RouteConfigApi::get` 并 `mode.set / local_provider.set / rate_limits.set …` 全覆盖用户正在编辑的未保存值，同时 `loading.set(true)` 让整个表单（`Show when=!loading`）闪退为"加载中…"。对照 desktop `wide/views/settings/route.rs:50-70` 是挂载时一次性加载（`spawn_local` 裸跑，无 reconnect 重载）——phone 偏离了它声称"逐字镜像"的数据契约。移动端断连重连频繁，用户编辑到一半被静默清空是现实场景。
建议：只在"从未加载过"（如 `loading` 初值且无可编辑状态）时重载，或重连时跳过已有本地修改；至少不要重设 `loading` 导致表单闪烁。

### Low

**3. `interfaces/webchat/src/platform/phone/settings/mod.rs:48,66,81,94,112,127-131,143` — 设置落地页展示硬编码假数据**
`"remote · 10.10.10.4"`、`"Anthropic"`、`"text-embedding-3"`、`"Opus 4.8"`、`"System"`、5 个写死色值的 Accent swatch、`"Luxe"` 全部以真实配置的姿态展示。文件头注释声明这是 v1 刻意为之（spec §6 静态占位），但 `10.10.10.4` 显然是开发者内网 IP 被固化进 UI，且这些值与真实配置不符时具有误导性。
建议：接入真实状态（`DashboardState` / appearance 的 `read_*` 已有现成读取函数），或至少移除具体 IP。

**4. `interfaces/webchat/src/platform/phone/memory/mod.rs:85-103` — 笔记窗口加载无陈旧响应防护**
loader `Effect` 跟踪 `agent_id` / `reload_nonce`，每次变化 `spawn_local` 一个 `list_facts`，无序号/取消机制：快速切换 agent 时两个请求在途，后到者覆盖先到者，可能把旧 agent 的窗口写入新 agent 的视图（Leptos 单线程下完成顺序不确定）。概率低且下次操作自愈，但属真实竞态。
建议：加载前取当前 agent 快照，响应回来后比对再写入；或用递增 request id 丢弃过期响应。

**5. `interfaces/webchat/src/platform/phone/settings/providers.rs` vs `settings/embeddings.rs` — 两文件近乎逐行重复（各 ~320 行）**
展开行、chevron 旋转、API Key 密码输入行、启用开关、set-default/set-active 行的结构、样式、错误处理完全一致，仅 API 类型与文案不同。改动一处极易漏改另一处。
建议：抽出通用的"可展开 provider 行 + key 编辑"组件，差异用回调/泛型参数注入。

**6. `interfaces/webchat/src/platform/phone/memory/menu.rs:89,95,102` 及 `settings/mod.rs:36,56,102` — 硬编码中英混排文案，绕过 i18n**
`"视图"`、`"星系图"`、`"关系网络可视化"`、`"连接"`、`"外观"` 等直接写死，且与相邻英文 cell title 混排；同单元 `memory/graph.rs:36` 已正确使用 `t_string!(i18n, …)`。
建议：迁移到 i18n key。

**7. `interfaces/webchat/src/platform/phone/memory/detail.rs:3` — 文档注释与实现不符**
模块注释写"redirects to `/memory`"，实际 `detail.rs:37` 导航到 `/memory/list`（后者更合理）。
建议：改注释。

## 架构红线合规快照

| 红线 | 结论 | 说明 |
|------|------|------|
| R1 | 合规 | 无任何平台 API 调用；仅 `web_sys` 读 `location.host`（connection.rs）与 console 日志 |
| R2 | 合规 | 全部 Leptos/WASM；复用 desktop 视图组件（CanvasView/KanbanView 等）而非重写 |
| R3 | 合规 | 未引入新依赖（手写 wikilink 扫描器注释明确"no regex dep"） |
| R4 | 基本合规 | 屏幕为纯 I/O + 导航；排序/过滤/分页委托给 `views::memory::data`、`shared_ui_logic` 纯函数；残留的轻量展示逻辑（label 拼接、排序）可接受 |
| R7 | 合规 | 一切数据经 `DashboardState` RPC / 共享 ChatState，无本地业务真相 |
| R8 | 合规 | 无正则；意图路由全在服务端 |
| R9 | 不适用 | 本单元无可配置项新增 |
| R10 | 合规 | composer 注释明确"server remains the prompt-injection authority"，客户端无注入守卫中间件 |

## 安全核查记录（已验证为安全，不计入发现）

- `memory/detail.rs:91` `inner_html=render_excerpt(&md)`：已核实 `canvas_engine/markdown_excerpt.rs` 对所有 Text/Code/Html 事件做 `html_escape`，`sanitize_link_url` 仅放行 http/https/mailto，`javascript:` 等被降级为纯文本——无 XSS。
- providers/embeddings 的 API Key 处理：`type="password"`、展开时清空、从不回填；`api_key: None` 透传语义与 desktop `detail_panel.rs:156-198` 一致（None=服务端保留原值），不存在"切开关清空密钥"。
- 生产代码无 `unwrap()`/`expect()`（仅 `#[cfg(test)]` 内使用）；`expect_context` 为 Leptos 上下文原语，不在此列。
- 无 SSRF/证书/越权攻击面：本层不构造 URL、不处理 TLS，全部 RPC 走既有 `DashboardState` 通道。
