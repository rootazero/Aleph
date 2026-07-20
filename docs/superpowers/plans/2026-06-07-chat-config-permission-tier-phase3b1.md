# Chat/Config 权限分层 Phase 3b-1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Operator 在 Panel「已配对设备」列表看到每台设备档位（聊天/配置）并一键升降档；配对审批用「批准为聊天/批准为配置」双钮直接选档。

**Architecture:** 后端补一个只读派生字段 `tier`（从 device `permissions` 经 SSOT 推导），透出到既有 `security_config.list_devices`；Panel DeviceCard 渲染档位徽章+情境感知切换钮，调 Phase 3a 的 `devices.set_level`（operator-gated + 活连接即时刷新）；配对卡用已有 `pairing.approve {level}` 参选档。不新增 RPC、不动 device_store/持久化。

**Tech Stack:** Rust（gateway handler + tier SSOT，TDD 单测）、Leptos/WASM Panel（验证 = `just wasm` build，不 cargo-check）、leptos-i18n（en.json + zh.json 并行）。

**Spec:** `docs/superpowers/specs/2026-06-07-chat-config-permission-tier-phase3b1-design.md`

**Git 约束（全程）:** 共享单分支 main + 并发提交者——只追加式提交、**显式文件路径**暂存（禁 `git add -A/-u/.`）、禁 reset/amend/rebase/push；提交信息英文、无 attribution footer；不 push。

---

## File Structure

- `src/gateway/handlers/auth/tier.rs` — 新增 `tier_for_permissions` SSOT helper（与 `role_for_permissions` 并列）。
- `src/gateway/handlers/security_config.rs` — `DeviceInfo` 加 `tier` 字段；`handle_list_devices` 填充。
- `interfaces/webchat/src/api/security.rs` — Panel `DeviceInfo` 加 `tier`；`SecurityConfigApi::set_level` 调 `devices.set_level`。
- `interfaces/webchat/locales/en.json` + `zh.json` — 新增档位徽章/切换钮/配对选档 i18n 键（并行）。
- `interfaces/webchat/src/views/settings/security/gateway.rs` — DeviceCard 档位徽章+切换钮；PairedDevices 接 `set_level` 闭包。
- `interfaces/webchat/src/components/notification_center.rs` — 配对卡单 Approve → 双钮。

任务顺序：Task 1 后端先行（透出 tier）→ Task 2 Panel API → Task 3 i18n 键（leptos-i18n 编译期校验，组件引用前键必须存在）→ Task 4 DeviceCard → Task 5 配对卡。

---

### Task 1: 后端 — tier SSOT helper + DeviceInfo.tier 透出

**Files:**
- Modify: `src/gateway/handlers/auth/tier.rs`（加 `tier_for_permissions` + 单测）
- Modify: `src/gateway/handlers/security_config.rs:181`（`DeviceInfo` 加 `tier`）、`:378`（`handle_list_devices` 填充）

- [ ] **Step 1: 在 tier.rs 写 `tier_for_permissions` 的失败测试**

在 `tier.rs` 的 `#[cfg(test)] mod tests` 内追加：

```rust
    #[test]
    fn tier_label_config_for_wildcard() {
        assert_eq!(tier_for_permissions(&["*".to_string()]), "config");
    }

    #[test]
    fn tier_label_chat_for_non_wildcard() {
        assert_eq!(
            tier_for_permissions(&["chat".to_string(), "read".to_string()]),
            "chat"
        );
    }

    #[test]
    fn tier_label_chat_for_empty() {
        assert_eq!(tier_for_permissions(&[]), "chat");
    }
```

- [ ] **Step 2: 运行测试确认失败（函数未定义）**

Run: `cargo test -p alephcore --lib tier_label 2>&1 | tail -20`
Expected: 编译失败 `cannot find function tier_for_permissions`

- [ ] **Step 3: 在 tier.rs 实现 `tier_for_permissions`**

在 `role_for_permissions`（:53 之后、`#[cfg(test)]` 之前）追加：

```rust
/// UI-facing tier label for a device holding `permissions`.
/// `"config"` iff the wildcard is present, else `"chat"` (the default tier).
/// Mirrors `role_for_permissions` but yields the label the Panel shows on a
/// device card, decoupled from the connect-response role string.
pub fn tier_for_permissions(permissions: &[String]) -> &'static str {
    if permissions.iter().any(|p| p == WILDCARD) {
        "config"
    } else {
        "chat"
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cargo test -p alephcore --lib tier_label 2>&1 | tail -20`
Expected: 3 个测试 PASS

- [ ] **Step 5: security_config.rs 的 `DeviceInfo` 加 `tier` 字段**

`src/gateway/handlers/security_config.rs:181` 的 struct 改为：

```rust
/// Device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub paired_at: String,
    pub last_seen: Option<String>,
    /// UI tier label derived from the device permissions: `"chat"` | `"config"`.
    pub tier: String,
}
```

- [ ] **Step 6: `handle_list_devices` 填充 `tier`**

`src/gateway/handlers/security_config.rs:378` 的 map 闭包改为（`ApprovedDevice.permissions` 已可用，device_store.rs:25）：

```rust
        .map(|d| DeviceInfo {
            tier: crate::gateway::handlers::auth::tier::tier_for_permissions(&d.permissions)
                .to_string(),
            device_id: d.device_id,
            device_name: d.device_name,
            device_type: d.device_type.unwrap_or_else(|| "unknown".to_string()),
            paired_at: d.approved_at,
            last_seen: d.last_seen_at,
        })
```

> 注意：`tier` 取在 `device_id` move 之前（`tier_for_permissions` 借用 `&d.permissions`，与移动其它字段不冲突，但把 tier 行放最前更清晰）。确认 `tier` 模块在此文件可达——若 `crate::gateway::handlers::auth::tier` 路径不通，改用文件顶部已有的 `auth` 引入或加 `use crate::gateway::handlers::auth::tier;` 后写 `tier::tier_for_permissions`。

- [ ] **Step 7: 全目标编译 + 后端测试**

Run: `cargo check -p alephcore --all-targets 2>&1 | tail -20`
Expected: 编译通过（Phase 3a 教训：`--all-targets` 才编译 `tests/`）
Run: `cargo test -p alephcore --lib tier 2>&1 | tail -20`
Expected: tier 模块全部 PASS

- [ ] **Step 8: 提交（显式路径）**

```bash
git add src/gateway/handlers/auth/tier.rs src/gateway/handlers/security_config.rs
git commit -m "gateway: surface device tier in security_config.list_devices"
```

---

### Task 2: Panel API — DeviceInfo.tier + SecurityConfigApi::set_level

**Files:**
- Modify: `interfaces/webchat/src/api/security.rs:193`（`DeviceInfo` 加 `tier`）、`:243`（加 `set_level`）

- [ ] **Step 1: Panel `DeviceInfo` 加 `tier` 字段**

`interfaces/webchat/src/api/security.rs:193` 改为（与后端序列化字段名对齐）：

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub device_type: String,
    pub paired_at: String,
    pub last_seen: Option<String>,
    /// UI tier label from the backend: `"chat"` | `"config"`.
    pub tier: String,
}
```

- [ ] **Step 2: 加 `SecurityConfigApi::set_level`**

在 `impl SecurityConfigApi` 内 `revoke_device`（:233-242）之后追加。**注意：调的是 `devices.set_level`（Phase 3a RPC），不是 `security_config.*`**：

```rust
    /// Change a device's tier (chat | config) via the Phase 3a operator RPC.
    /// `level` is the target tier: "chat" downgrades, "config" elevates.
    pub async fn set_level(
        state: &DashboardState,
        device_id: String,
        level: String,
    ) -> Result<(), String> {
        let params = serde_json::json!({
            "device_id": device_id,
            "level": level,
        });

        state.rpc_call("devices.set_level", params).await?;
        Ok(())
    }
```

- [ ] **Step 3: WASM 构建验证**

Run: `just wasm 2>&1 | tail -25`
Expected: 构建成功（dist 重建）。Panel 无 cargo-check，靠 wasm build 兜底类型/宏错误。

- [ ] **Step 4: 提交（显式路径）**

```bash
git add interfaces/webchat/src/api/security.rs
git commit -m "panel: add device tier field + set_level api binding"
```

---

### Task 3: i18n 键（en.json + zh.json 并行）

**Files:**
- Modify: `interfaces/webchat/locales/en.json`、`interfaces/webchat/locales/zh.json`

> leptos-i18n 在 wasm 编译期校验 `t!` 键，且要求 en/zh 键集一致——组件引用前两边都要有键，故本任务先于 Task 4/5。

- [ ] **Step 1: en.json 在 `settings.security` 段加档位键**

在 `settings.security` 对象内（如紧跟 `"never"` 之后）加：

```json
    "tier_chat": "Chat",
    "tier_config": "Config",
    "grant_config": "Grant config",
    "downgrade_chat": "Downgrade to chat",
```

- [ ] **Step 2: en.json 在 `notifications` 段加配对选档键**

在 `notifications` 对象内（如紧跟 `"empty"` 之后）加：

```json
    "approve_chat": "Approve as chat",
    "approve_config": "Approve as config",
```

- [ ] **Step 3: zh.json 在 `settings.security` 段加同名键**

```json
    "tier_chat": "聊天档",
    "tier_config": "配置档",
    "grant_config": "授权配置",
    "downgrade_chat": "降级为聊天",
```

- [ ] **Step 4: zh.json 在 `notifications` 段加同名键**

```json
    "approve_chat": "批准为聊天",
    "approve_config": "批准为配置",
```

- [ ] **Step 5: JSON 合法性检查**

Run: `cd interfaces/webchat/locales && python3 -c "import json; json.load(open('en.json')); json.load(open('zh.json')); print('OK')"`
Expected: `OK`

- [ ] **Step 6: 提交（显式路径）**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel: i18n keys for device tier badge + pairing tier approval"
```

---

### Task 4: DeviceCard 档位徽章 + 切换钮 + PairedDevices 接线

**Files:**
- Modify: `interfaces/webchat/src/views/settings/security/gateway.rs:132`（PairedDevices 加 `set_level` 闭包）、`:165`（call site 传 `on_set_level`）、`:181`（DeviceCard 加泛型 + 徽章 + 钮）

> `DashboardState` 与 `RwSignal` 均 `Copy`（context.rs:84），故 `set_level` 闭包可照搬 `revoke_device` 的捕获方式，无需 clone。

- [ ] **Step 1: PairedDevices 内加 `set_level` 闭包**

在 `revoke_device` 闭包（:133-147）之后追加（`state`/`devices` 是 Copy，直接捕获）：

```rust
    let set_level = move |device_id: String, level: String| {
        spawn_local(async move {
            match SecurityConfigApi::set_level(&state, device_id, level).await {
                Ok(_) => {
                    if let Ok(devs) = SecurityConfigApi::list_devices(&state).await {
                        devices.set(devs);
                    }
                }
                Err(e) => {
                    web_sys::console::error_1(
                        &format!("Failed to set device level: {}", e).into(),
                    );
                }
            }
        });
    };
```

- [ ] **Step 2: call site（:165-170 map）传 `on_set_level`**

把 map 闭包改为同时克隆出 set_level 用的 device_id：

```rust
                                {device_list.into_iter().map(|device| {
                                    let device_id = device.device_id.clone();
                                    let device_id_sl = device.device_id.clone();
                                    view! {
                                        <DeviceCard
                                            device=device
                                            on_revoke=move || revoke_device(device_id.clone())
                                            on_set_level=move |level: String| set_level(device_id_sl.clone(), level)
                                        />
                                    }
                                }).collect::<Vec<_>>()}
```

- [ ] **Step 3: DeviceCard 加 `on_set_level` 泛型 + 徽章 + 切换钮**

把 `DeviceCard`（:180-211）整体改为：

```rust
#[component]
pub(super) fn DeviceCard<F, G>(device: DeviceInfo, on_revoke: F, on_set_level: G) -> impl IntoView
where
    F: Fn() + 'static,
    G: Fn(String) + 'static,
{
    let i18n = use_i18n();
    let paired_date = device.paired_at.clone();
    let last_seen_text = device
        .last_seen
        .clone()
        .unwrap_or_else(|| t_string!(i18n, settings.security.never).to_string());
    let is_config = device.tier == "config";
    let target_level = if is_config { "chat" } else { "config" };

    view! {
        <div class="flex items-center justify-between p-4 bg-surface-sunken rounded border border-border">
            <div class="flex-1">
                <div class="font-medium flex items-center gap-2">
                    {device.device_name}
                    <span class=move || {
                        if is_config {
                            "text-xs px-1.5 py-0.5 rounded bg-indigo-600 text-white"
                        } else {
                            "text-xs px-1.5 py-0.5 rounded bg-surface-raised text-text-secondary"
                        }
                    }>
                        {move || if is_config {
                            t_string!(i18n, settings.security.tier_config).to_string()
                        } else {
                            t_string!(i18n, settings.security.tier_chat).to_string()
                        }}
                    </span>
                </div>
                <div class="text-sm text-text-tertiary">
                    {device.device_type} " • " {device.device_id}
                </div>
                <div class="text-xs text-text-secondary mt-1">
                    {t!(i18n, settings.security.paired)} ": " {paired_date} " • " {t!(i18n, settings.security.last_seen)} ": " {last_seen_text}
                </div>
            </div>
            <div class="flex items-center gap-2">
                <button
                    on:click=move |_| on_set_level(target_level.to_string())
                    class="px-3 py-1 bg-surface-raised text-text-primary text-sm rounded hover:bg-surface-sunken border border-border"
                >
                    {move || if is_config {
                        t_string!(i18n, settings.security.downgrade_chat).to_string()
                    } else {
                        t_string!(i18n, settings.security.grant_config).to_string()
                    }}
                </button>
                <button
                    on:click=move |_| on_revoke()
                    class="px-3 py-1 bg-danger text-white text-sm rounded hover:bg-danger"
                >
                    {t!(i18n, settings.security.revoke)}
                </button>
            </div>
        </div>
    }
}
```

> 若 `t_string!`/`t!` 在条件分支里的具体返回类型在 wasm 构建时报错，按 leptos-i18n 既有用法微调（例如统一用 `t_string!(...).to_string()` 或包到 `move ||`）；逻辑（is_config 决定徽章文案/钮文案/目标档位）不变。

- [ ] **Step 4: WASM 构建验证**

Run: `just wasm 2>&1 | tail -30`
Expected: 构建成功。若报 `t!` 类型不一致或键缺失，按提示修（键已在 Task 3 加）。

- [ ] **Step 5: 提交（显式路径）**

```bash
git add interfaces/webchat/src/views/settings/security/gateway.rs
git commit -m "panel: device card tier badge + elevate/downgrade toggle"
```

---

### Task 5: 配对卡 — 单 Approve → 「批准为聊天/批准为配置」双钮

**Files:**
- Modify: `interfaces/webchat/src/components/notification_center.rs:144-195`（配对卡按钮区）

- [ ] **Step 1: 配对卡按钮区改为双 approve + reject**

把 map 闭包（:144-197）内的按钮区改为：先克隆出两个 approve 用的 code，approve 行放两钮（各 `flex-1`），reject 单独一行。整体替换 `:144` 起的 map 闭包：

```rust
                                    {pairings.into_iter().map(|p: IncomingPairing| {
                                        let code_for_chat = p.code.clone();
                                        let code_for_config = p.code.clone();
                                        let code_for_reject = p.code.clone();
                                        let code_display = p.code.clone();
                                        let label = p.origin_label.clone();
                                        let i18n = use_i18n();
                                        view! {
                                            <li class="px-4 py-3">
                                                <div class="text-sm font-medium text-text-primary">
                                                    "Pair browser"
                                                </div>
                                                <div class="text-xs text-text-secondary mt-0.5">
                                                    {label}
                                                </div>
                                                <div class="font-mono text-2xl my-2 text-center tracking-widest text-indigo-300">
                                                    {code_display}
                                                </div>
                                                <div class="flex gap-2">
                                                    <button
                                                        type="button"
                                                        class="flex-1 py-1.5 rounded bg-surface-sunken hover:bg-surface-raised text-text-secondary text-xs transition-colors"
                                                        on:click=move |_| {
                                                            let c = code_for_chat.clone();
                                                            spawn_local(async move {
                                                                let _ = dashboard
                                                                    .rpc_call(
                                                                        "pairing.approve",
                                                                        json!({"code": c, "level": "chat"}),
                                                                    )
                                                                    .await;
                                                            });
                                                        }
                                                    >
                                                        {t!(i18n, notifications.approve_chat)}
                                                    </button>
                                                    <button
                                                        type="button"
                                                        class="flex-1 py-1.5 rounded bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-semibold transition-colors"
                                                        on:click=move |_| {
                                                            let c = code_for_config.clone();
                                                            spawn_local(async move {
                                                                let _ = dashboard
                                                                    .rpc_call(
                                                                        "pairing.approve",
                                                                        json!({"code": c, "level": "config"}),
                                                                    )
                                                                    .await;
                                                            });
                                                        }
                                                    >
                                                        {t!(i18n, notifications.approve_config)}
                                                    </button>
                                                </div>
                                                <div class="mt-2">
                                                    <button
                                                        type="button"
                                                        class="w-full py-1.5 rounded bg-surface-sunken hover:bg-surface-raised text-text-tertiary text-xs transition-colors"
                                                        on:click=move |_| {
                                                            let c = code_for_reject.clone();
                                                            spawn_local(async move {
                                                                let _ = dashboard
                                                                    .rpc_call(
                                                                        "pairing.reject",
                                                                        json!({"code": c}),
                                                                    )
                                                                    .await;
                                                            });
                                                        }
                                                    >
                                                        "Reject"
                                                    </button>
                                                </div>
                                            </li>
                                        }
                                    }).collect::<Vec<_>>()}
```

> `dashboard` 在三个 on:click 闭包里使用：原代码 `dashboard` 在两钮中已可被多个闭包用（`DashboardState: Copy`），三钮同理 Copy 复制即可，无需手动 clone。Reject 文案沿用字面量 `"Reject"`（spec：拒绝不变）。`use_i18n()` 在 map 闭包内取（与文件其它处一致，:208 已有 `use_i18n()` 内联用法）。

- [ ] **Step 2: WASM 构建验证**

Run: `just wasm 2>&1 | tail -30`
Expected: 构建成功。

- [ ] **Step 3: 提交（显式路径）**

```bash
git add interfaces/webchat/src/components/notification_center.rs
git commit -m "panel: pairing card approve-as-chat / approve-as-config buttons"
```

---

## 最终验证（全任务完成后）

- [ ] `cargo check -p alephcore --all-targets` 绿（后端）
- [ ] `cargo test -p alephcore --lib tier` 绿（tier SSOT 单测）
- [ ] `just wasm` 绿（Panel dist 重建，无 `t!`/类型错误）
- [ ] 派 final code reviewer 审整体（spec 合规 + 代码质量）

## 部署（用户决定时机）

Panel 见效需：`just wasm` → 重编 `aleph-server` binary（rust_embed 烧 dist）→ 热替换运行中 daemon。用户此前表态「3b 全 UI 一起做」，可在 3b-2 完成后统一部署。
