# 远程 Panel 认证硬化设计 (Remote Panel Auth Hardening)

- **日期**: 2026-07-03
- **对应文档段落**: FEATURE_LOCATOR.md §6.2 Panel 远程连接与 Gateway Token 授权
- **参考项目**: openclaw（`extensions/device-pair/`：pair-command-auth / approve / device-auth）
- **红线对齐**: R4（Interface 纯 I/O）、R10（笨循环，新逻辑不进 harness）、P7（防御性）、gateway/CLAUDE.md（认证边界，改动必同步测试）

## 1. 诊断：任务前提 vs 现状

用户任务前提是「Aleph 认证靠 URL 里的 `?token=xxx`，易经浏览器历史 / Referer / 访问日志泄露」。**这反映的是 FEATURE_LOCATOR §6.2 的文档状态，不是实际代码。**

实际代码里，规避 URL-token 泄露的现代流程 **已端到端落地**：

| 层 | 现状 |
|----|------|
| 后端判定 | `connect::resolve_connect_auth` 4 级优先：loopback → `device_token`（长效绑设备）→ `bootstrap_ticket`（短效一次性，换取 device token）→ legacy shared token |
| 设备令牌 | `DeviceTokenManager`（`aleph-bt-*` 一次性票 5min → 换 `aleph-dt-*` 10 年设备 token），boot 构造并 `set_device_token_manager` 注入 |
| RPC | `gateway.ticket.create` 已注册（start/mod.rs:470），签发一次性配对票 |
| 前端 | `context.rs` 解析 `?bt=`、connect 换取、持久化 `aleph_device_token`、按优先级发送 |
| UI | settings `gateway_token.rs` 生成 `?bt=` 一次性配对 QR；legacy 共享 token 明确标注「勿放进 URL/QR」 |
| 清痕 | `scrub_credentials_from_url` 授权后同时清 `?token=` 和 `?bt=` |

**结论**：架构重构已基本完成。文档陈旧。真正待做的是这个「已实现但不完善」流程里的**错误修复 + 功能连线 + 打磨**。

## 2. 缺口清单

| # | 缺口 | 性质 | 证据 |
|---|------|------|------|
| 1 | **轮换共享 token 并不撤销 device token，但 UI + handler 文档声称会** | 🐛 安全相关虚假声明 | `shared_token.rs::reset_token` 只重生共享 token + 重加密 vault，不碰 `security_store` 的 device token；UI 注释写「A rotation invalidates previously issued device tokens」 |
| 2 | **无 Panel 设备的 list/revoke RPC + UI**——配对设备 10 年 operator token 无法单独查看或吊销 | 🕳️ 功能缺失 | `gateway.devices.*` 不存在；`list_devices`/`revoke_device` 只被 `cluster.rs`（集群节点）使用 |
| 3 | `DeviceTokenManager::prune_expired` 从未被调度 | 🧹 卫生 | 全仓无生产调用点 |
| 4 | FEATURE_LOCATOR §6.2 + gateway/CLAUDE.md 描述陈旧单层模型 | 📄 文档漂移 | 与实际 4 级 auth 不符 |

#1 + #2 合起来是真正的安全洞：**一旦远程设备配对，其 10 年 operator token 通过任何界面都撤销不掉**——而「远程访问安全」正是这个功能的全部卖点。

## 3. 关键约束：panel 设备 vs cluster 节点

`devices` 表**同时**存 panel 设备（`device_type='panel'`，bootstrap 交换写入）与 cluster 节点（`role='node'`，有独立 enroll/dereg 路径 `cluster.rs`）。

**所有新增撤销 / 列表操作必须只作用于 `device_type='panel'` 的行**，绝不误杀 cluster 节点。这是 #1、#2 的核心防御约束。

## 4. 设计决策（用户离开期间按推荐落地，待审阅）

### 决策 A — #1 修法：轮换 = 核弹级全撤销（推荐）
`gateway.token.rotate` 在重生共享 token 后，**额外撤销所有 panel 设备 + 其 device token**。
- 理由：匹配 UI 现有措辞与 §6.2「撤销：轮换 token，所有远程端须重新输入」的心智模型；改动最小；立即消除虚假安全声明。
- 叠加 #2 的 per-device 精细撤销补足粒度。
- （备选：解耦——轮换只管共享 token，改前端文案，撤销全交给 #2。未采纳，因偏离既有心智且需改更多前端文案。）

### 决策 B — 范围：#1 + #2 + #3 + #4 全做
四项连贯、都服务目标、改动可控。

## 5. 实现方案（surgical，复用既有 store API）

### 5.1 `DeviceTokenManager` 新增方法（`device_token_manager.rs`）
```
list_panel_devices() -> Result<Vec<DeviceRow>>
    // list_devices().filter(device_type == Some("panel"))
revoke_all_panel_devices() -> Result<usize>
    // for d in list_panel_devices { revoke_device(d.device_id) }  复用既有 revoke_device（tokens+device）
revoke_panel_device(device_id) -> Result<bool>
    // 校验目标 device_type=='panel' 后再 revoke_device；防经 panel RPC 误杀 cluster 节点
prune_now() -> Result<(usize, usize)>
    // 薄封装 prune_expired(current_timestamp_ms())，供机会式调用
```
不写新 SQL（rotate 是罕见操作，遍历撤销可接受），最大化复用。

### 5.2 #1：`gateway.token.rotate` 注入 DeviceTokenManager
- `handle_token_rotate` 改签名接收 `Arc<DeviceTokenManager>`（或含它的小 ctx，镜像 `TicketHandlerContext`）。
- 成功 rotate 后调 `revoke_all_panel_devices()`；失败不撤销。
- start/mod.rs 注册处捕获 `device_token_mgr.clone()`。
- 回归测试：rotate 后旧 device token `validate_device_token` 返回 None。

### 5.3 #2：`gateway.devices.list` / `gateway.devices.revoke`
- 新 handler 文件 `src/gateway/handlers/gateway_devices.rs`（纯 I/O，R4）：
  - `handle_devices_list`：返回 `[{device_id, device_name, created_at, last_seen_at}]`（仅 panel）。
  - `handle_devices_revoke {device_id}`：`revoke_panel_device`，返回 `{revoked: bool}`；成功广播 `TokenRotated` 同类事件使该设备下线（或复用现有踢下线机制）。
- start/mod.rs 注册两方法，捕获 `device_token_mgr.clone()`（镜像 ticket 注册）。
- 授权：登录墙保证只有 operator/loopback 可达（与 `gateway.token.rotate` 同性质），无需额外 method_authz。
- Panel UI：`gateway_token.rs` 新增「Paired devices」区——列出设备 + 每行 Revoke 按钮，调 `gateway.devices.list/revoke`。

### 5.4 #3：机会式 prune（无新 daemon 任务，R10/R3 极简）
在两个 chokepoint 调 `prune_now()`：`gateway.ticket.create`、`gateway.devices.list`。避免新增周期任务税。

### 5.5 #4：文档
- FEATURE_LOCATOR §6.2：改写为真实的 bootstrap-ticket + device-token 4 级模型 + 新增 `gateway.devices.*` 锚点。
- gateway/CLAUDE.md：更新信任模型段，补 device token / 配对票 / 设备撤销。

## 6. 非目标 (YAGNI，本轮不做)
- **显式设备审批**（openclaw device-pair approve 步骤）：票的生成已受 operator 门禁，自动授权对 MVP 足够。记为未来增强。
- **device token 绑客户端公钥**：`exchange_bootstrap_ticket` 现忽略 pubkey。真硬化但范围更大，另立。
- **把配对票移出 URL**：QR 本质需 URL 承载；票已一次性 + 5min + 授权后 scrub，剩余暴露窗口窄，可接受。

## 7. 验证
- `cargo check -p alephcore --lib`（一次，遵循节制 cargo 调用）。
- 新增 host 单测：rotate 撤销 device token、revoke_panel_device 不碰 node、list 只返回 panel。
- 前端改动需重编 binary（rust_embed 嵌入链）——本轮只改代码 + 单测，不跑 WASM 构建。
