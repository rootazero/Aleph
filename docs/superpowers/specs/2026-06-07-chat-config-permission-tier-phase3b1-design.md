# Chat/Config 权限分层 Phase 3b-1 设计 — 设备档位 UI

> 续 Phase 3a（设备档位提升 RPC `devices.set_level` 已落地）。本期把档位能力接到 Leptos Panel：让 operator 在 Panel 里看到/切换每台设备的档位，并在配对审批时直接选档。纯前端为主 + 一处只读派生后端补字段。

**Goal:** Operator 在 Panel 的「已配对设备」列表看到每台设备当前档位（聊天/配置），一键升/降档；配对审批卡用「批准为聊天 / 批准为配置」双钮直接选档。

**Architecture:** 沿用现有 `security_config.list_devices`（Panel 设备列表唯一数据源）+ 复用 Phase 3a 的 `devices.set_level` RPC（operator-gated，带活连接即时刷新）+ 复用配对已有的 `pairing.approve {level}` 参数。后端只补一个**只读派生字段** `tier`（从 device 的 `permissions` 推导），不新增 RPC、不动 device_store、不动持久化。

**Tech Stack:** Rust（gateway handler + tier SSOT）、Leptos/WASM Panel、leptos-i18n（en.json + zh.json）。

---

## 背景与约束

- **R4**：Panel 是纯 I/O，不做业务逻辑——档位推导/门控全在 Core，Panel 只渲染 + 发 RPC。
- **R2**：复杂业务 UI 在 Leptos Panel，不在原生 bridge。
- **fail-closed**：档位推导沿用 SSOT；任何未知/缺失 permissions 视为聊天档（最小权限）。
- **DRY**：Panel 直接调 Phase 3a 的 `devices.set_level`（operator token 过门 + 活连接即时刷新），不在 `security_config.*` 复制一个 `set_device_level`。
- **现状缺口**：`security_config.list_devices` 返回的 `DeviceInfo`（后端 security_config.rs:181 + Panel api/security.rs:193）**不含档位/权限**。`handle_list_devices`（security_config.rs:370）能从 `ApprovedDevice.permissions`（device_store.rs:25）拿到权限但当前丢弃了。要在卡片显示当前档位（"降级"按钮需要知道当前是什么档），必须把档位透出。
- **配对卡现状**：notification_center.rs:164-177 单个 `Approve` 钮发 `pairing.approve {code}`（无 level）；`pairing.approve` 后端已支持 `level` 参（Phase 3a/B1）。

## 数据流

### 列出设备（带档位）
```
Panel DeviceList 组件
  → security_config.list_devices  (无参)
  → handle_list_devices: 对每台 ApprovedDevice 用 tier SSOT 从 permissions 推导 tier ("chat"|"config")
  → DeviceInfo { ..., tier }  → Panel DeviceCard 渲染档位徽章 + 情境感知切换钮
```

### 切换档位（升/降档）
```
DeviceCard 切换钮 on:click
  → SecurityConfigApi::set_level(device_id, level)   // level = 目标档位
  → devices.set_level {device_id, level}             // Phase 3a RPC, operator-gated
  → (3a 已实现) 更新 device_store permissions + 原地刷新活连接 role/permissions
  → Panel 重拉 list_devices → 徽章即时更新
```

### 配对审批选档
```
配对卡 「批准为聊天」/「批准为配置」 on:click
  → pairing.approve {code, level: "chat"|"config"}   // 已有 level 参
  → 「拒绝」 → pairing.reject {code}                   // 不变
```

## 组件设计

### 后端（一处只读派生字段）

**1. `src/gateway/handlers/auth/tier.rs` — 新增 SSOT helper**
- 新增 `pub fn tier_for_permissions(permissions: &[String]) -> &'static str`：含 `WILDCARD` ⇒ `"config"`，否则 `"chat"`。
- 与现有 `role_for_permissions`（operator/guest）并列，同一推导口径，UI 标签维度。
- 单测：config（含 `*`）、chat（["chat","read"]）、empty（→chat）三情形。

**2. `src/gateway/handlers/security_config.rs` — DeviceInfo 加 tier 字段**
- `DeviceInfo`（:181）加 `pub tier: String`。
- `handle_list_devices`（:378 map）填 `tier: tier_for_permissions(&d.permissions).to_string()`。
- 其余字段不变。`ApprovedDevice.permissions`（device_store.rs:25）已可用，无需改 device_store。

### Panel（主体，纯 Leptos）

**3. `interfaces/webchat/src/api/security.rs`**
- Panel 侧 `DeviceInfo`（:193）加 `pub tier: String`（与后端字段名/序列化对齐）。
- 新增 `SecurityConfigApi::set_level(state, device_id: String, level: String) -> Result<(), String>`：调 `devices.set_level {device_id, level}`（注意：调的是 `devices.set_level` 不是 `security_config.*`）。

**4. `interfaces/webchat/src/views/settings/security/gateway.rs` — DeviceCard**
- DeviceCard 接收 `device: DeviceInfo`（已带 tier）+ 新增 `on_set_level` 回调（参数为目标 level）。
- 渲染**档位徽章**：`tier == "config"` ⇒「配置档」（醒目色，如 indigo/amber），否则「聊天档」（中性色）。放在设备名旁或类型行。
- 渲染**情境感知切换钮**（放 Revoke 旁）：
  - 当前 `chat` ⇒ 钮文案「授权配置」，点击 `on_set_level("config")`。
  - 当前 `config` ⇒ 钮文案「降级为聊天」，点击 `on_set_level("chat")`。
- DeviceList（:165 map）把 `on_set_level` 接到一个 `set_level` 闭包：调 `SecurityConfigApi::set_level` → 成功后重拉 `list_devices` 刷新（镜像现有 `revoke_device` 闭包 :133-147 的刷新流），失败 `console::error`。

**5. `interfaces/webchat/src/components/notification_center.rs` — 配对卡**
- 把单个 `Approve` 钮（:161-177）换成**两个钮**：
  - 「批准为聊天」⇒ `pairing.approve {code, level: "chat"}`
  - 「批准为配置」⇒ `pairing.approve {code, level: "config"}`
- 「拒绝」钮（:178-194）不变。
- 布局：两个 approve 钮 + reject。三钮在窄通知卡里偏挤——两个 approve 钮放一行（各 `flex-1`），reject 放第二行（或三钮一列堆叠）。实现时取可读布局，文案用 i18n。

**6. i18n（`interfaces/webchat/locales/en.json` + `zh.json` 并行加键）**
- 档位徽章：`聊天档 / 配置档`（en: `Chat / Config`）。
- 切换钮：`授权配置 / 降级为聊天`（en: `Grant config / Downgrade to chat`）。
- 配对：`批准为聊天 / 批准为配置`（en: `Approve as chat / Approve as config`）。
- 键归类到现有 `settings.security` 段（设备相关）+ 配对相关键放配对卡所用段。

## 错误处理

- `set_level` / `pairing.approve` 失败：沿用现有 Panel 模式——`rpc_call` 返回 `Result<_, String>`，失败 `web_sys::console::error`（与 revoke_device 一致）。Panel 当前无错误 toast/modal 基建，**不为 3b-1 新建**，保持一致。
- 非 operator 连接调 `devices.set_level`：后端 method-authz 已硬拒（Phase 3a）。Panel 经 shared_token = operator，正常过门；UI 不为非 operator 情形特殊处理（这些 Panel 入口本就 operator-only）。

## 不做（明确排除）

- 不在 `security_config.*` 新增 `set_device_level`（直接用 `devices.set_level`，DRY）。
- 不动 device_store / 持久化 / DeviceRole 枚举。
- 不做 sudo 审批 UI、不订阅 `approval.**`、不做「等待 server 授权…」态——这些是 **Phase 3b-2**。
- 不为 Panel 新增错误 toast/modal 基建。

## 测试

- **后端**：`tier_for_permissions` 单测（config/chat/empty 三情形）；若 `handle_list_devices` 有测试夹具，加断言输出含正确 `tier`。
- **Panel（Leptos/WASM）**：组件无法 `cargo check`（需真 wasm build：`just wasm` 验证），按既往约束不盲 `cargo check`。逻辑薄（透传 + 文案切换），靠 wasm build + 部署后人工点验。
- 后端改动跑 `cargo check -p alephcore --all-targets`（Phase 3a 教训：`--all-targets` 才编译 `tests/`）。

## 部署说明

Panel 改动需 `just wasm` 重建 dist + 重编 `aleph-server` binary（rust_embed 烧 dist）+ 热替换运行中 daemon，才能见效（CLAUDE.md Panel↔Daemon 资源嵌入链）。本期是否部署由用户在 3b-1 完成后决定（此前已表态「3b 全 UI 一起做」，3b-2 后再统一部署亦可）。

## Addendum (2026-06-07, post-implementation) — 配对卡档位选择降级

最终集成审查发现：通知中心的「Pair browser」卡**只渲染 browser 类配对**（`IncomingPairing` 唯一来源 `handle_pairing_start_browser`），而 `pairing.approve` 的 `PairingRequest::Browser` 分支（`src/gateway/handlers/auth/pairing.rs:83-142`）提前 return——它用**共享令牌 HMAC 铸造 operator 会话**，从不读 `level`、不建 `ApprovedDevice`、不入 device_store。`Tier::from_level → permissions` 仅对 `Device`（CLI/native）配对生效，而那类配对不显示在此卡上。

因此 Task 5 的「批准为聊天/批准为配置」双钮在该卡上**无差别效果**（spec 阶段疏漏：只验证了 `level` 被解析，未验证 browser 审批路径是否使用它）。

**决策（用户）：回退 Task 5 为单 Approve**。browser 配对卡恢复单个 `pairing.approve {code}` 钮；连带移除变为死键的 `notifications.approve_chat/approve_config`（`settings.security.tier_*` 仍被 DeviceCard 使用，保留）。**Tasks 1-4（DeviceCard 档位徽章+升降档切换，对 device_store 设备完全有效）照常发布**。

**推迟到专门 phase**：让 browser/remote 配对档位化——即 chat 档 browser 审批铸造 guest-scoped 会话（而非共享 operator 令牌）。这是实质性的 auth/会话模型改动、有安全含义，应另立 spec。它也暴露了一个更广的事实：远程 browser/mobile 经 `/pair` 配对当前一律拿到共享令牌(operator)会话，与「远程 chat 档设备不能改配置」的目标相抵，值得在该 phase 一并处理。

## Git 约束（继承本会话纪律）

- 共享单分支 main + 并发提交者：只追加式提交，**显式文件路径**暂存（禁 `git add -A/-u/.`），禁 reset/amend/rebase/push。
- 仅用户要求时才 push；本期不 push。
- 提交信息英文，无 attribution footer。
