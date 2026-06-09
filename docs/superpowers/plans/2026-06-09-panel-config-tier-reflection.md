# Panel 配置页权限分层反映 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Panel 前端诚实反映后端已有的 2 层权限(Chat/Config tier):非 operator 连接时锁定配置页、显示当前身份、写失败给可操作提示。

**Architecture:** 纯前端 (`interfaces/webchat/`),后端零改动。新增一个 `permission` 组件模块(ConfigGate + PermissionBanner + 纯错误映射),在 `SettingsRouter` 路由层集中套 ConfigGate,在 Settings mode 区挂 PermissionBanner,在连接 chip 加 tier 徽章。全部复用已存在的 `DashboardState::is_operator()` / `role` signal。

**Tech Stack:** Rust + Leptos 0.7 (WASM)、leptos_i18n 0.6(编译期 codegen,en/zh parity)、Tailwind。测试:host-side `#[test]` 纯函数(WASM 组件不做 DOM 单测,沿用 `connection_status.rs` 既有范式)。

**Spec:** `docs/superpowers/specs/2026-06-09-panel-config-tier-reflection-design.md`

**构建/测试命令前缀**:所有命令在 `interfaces/webchat/` 下运行;panel 单测必须用 `-p aleph-panel`(不是 `-p alephcore`)。

---

## File Structure

| 文件 | 责任 |
|---|---|
| `interfaces/webchat/src/components/permission.rs`(新) | `ConfigGate` / `PermissionBanner` / `LockedNotice` 组件 + `is_permission_denied` / `friendly_error` 纯函数 + host 测试 |
| `interfaces/webchat/src/components/mod.rs` | 注册 `pub mod permission;` |
| `interfaces/webchat/src/app.rs` | `SettingsRouter` 各 config 路由套 `<ConfigGate>`;Settings mode 区挂 `<PermissionBanner>` |
| `interfaces/webchat/src/components/connection_status.rs` | 加 `tier_badge` 纯函数 + host 测试 + chip 渲染 tier 徽章 |
| `interfaces/webchat/src/context.rs` | WS 错误路径调用 `friendly_error` 兜底 |
| `interfaces/webchat/locales/en.json` / `zh.json` | 新增 `settings.permission.*` 键 |

---

## Task 1: 权限纯函数 `is_permission_denied` + `friendly_error`(TDD)

后端 RPC 错误消息已是英文(沿用 `connection_status.rs` 注释约定:raw RPC 错误英文,不 i18n)。operator-only 方法被拒时后端发 `"Operator privileges required for this method"`(`src/gateway/server/handler.rs:1035`);工具闸口发 `PermissionDenied`。本函数把这两类识别出来,替换成可操作的英文提示。纯函数,host 可测。

**Files:**
- Create: `interfaces/webchat/src/components/permission.rs`
- Modify: `interfaces/webchat/src/components/mod.rs`

- [ ] **Step 1: 写失败测试**

创建 `interfaces/webchat/src/components/permission.rs`,内容:

```rust
//! 权限分层 UI:配置页闸门 (ConfigGate)、全局身份横幅 (PermissionBanner)、
//! 以及 RPC 权限拒绝错误的友好映射。复用 DashboardState::is_operator()
//! —— 后端 2 层 tier 在前端的诚实投影。后端零改动。

/// 后端 RPC 错误消息是否为"权限不足"类(operator-only 方法 / 配置工具闸口)。
/// 纯字符串匹配,host 可测。后端消息为英文,沿用 raw-RPC-error-英文 约定。
pub(crate) fn is_permission_denied(raw: &str) -> bool {
    let l = raw.to_ascii_lowercase();
    l.contains("operator privileges required")
        || l.contains("permission denied")
        || l.contains("permissiondenied")
}

/// 把 RPC 错误消息映射为面向用户的展示串。权限拒绝替换成可操作提示
/// (指向「设置 → 安全」提权 / 重新配对选 Config);其余原样透传。
pub fn friendly_error(raw: &str) -> String {
    if is_permission_denied(raw) {
        "This action requires Config-tier permission. Ask an operator to grant it in \
         Settings → Security, or re-pair selecting Config."
            .to_string()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_operator_only_method_denial() {
        assert!(is_permission_denied(
            "Operator privileges required for this method"
        ));
    }

    #[test]
    fn detects_tool_permission_denied() {
        assert!(is_permission_denied("tool error: PermissionDenied"));
        assert!(is_permission_denied("Permission denied"));
    }

    #[test]
    fn passes_through_unrelated_errors() {
        assert!(!is_permission_denied("connection timeout"));
        assert_eq!(friendly_error("connection timeout"), "connection timeout");
    }

    #[test]
    fn friendly_error_rewrites_denial() {
        let out = friendly_error("Operator privileges required for this method");
        assert!(out.contains("Config-tier permission"));
        assert!(out.contains("Settings → Security"));
    }
}
```

在 `interfaces/webchat/src/components/mod.rs` 的 `pub mod notification_center;` 行之后插入(保持字母序,放在 `pre`→`pro` 之间即 `provider_badge` 之前):

```rust
pub mod permission;
```

- [ ] **Step 2: 运行测试确认失败/通过**

Run: `cd interfaces/webchat && cargo test -p aleph-panel permission:: -- --nocapture`
Expected: 4 个测试 PASS(纯函数,首次即过;若 mod 未注册会编译失败 → 注册后通过)

- [ ] **Step 3: 提交**

```bash
git add interfaces/webchat/src/components/permission.rs interfaces/webchat/src/components/mod.rs
git commit -m "panel: add permission helpers (is_permission_denied + friendly_error)"
```

---

## Task 2: i18n 键 `settings.permission.*`(en + zh parity)

leptos_i18n 0.6 编译期校验 en/zh key 严格对等。本任务只加键,不接组件。

**Files:**
- Modify: `interfaces/webchat/locales/en.json`
- Modify: `interfaces/webchat/locales/zh.json`

- [ ] **Step 1: en.json 加键**

在 `interfaces/webchat/locales/en.json` 的 `"settings"` 对象内,任选一个已存在子键(如 `"security"`)同级位置加入 `"permission"` 子对象:

```json
"permission": {
  "banner_chat": "You are connected as Chat (read-only). Configuration changes are locked. Ask an operator to grant Config permission in Settings → Security, or re-pair selecting Config.",
  "locked_title": "Configuration locked",
  "locked_notice": "This page requires Config-tier permission. Ask an operator to grant it in Settings → Security, or re-pair selecting Config."
}
```

- [ ] **Step 2: zh.json 加对等键**

在 `interfaces/webchat/locales/zh.json` 的 `"settings"` 对象内同样位置加入:

```json
"permission": {
  "banner_chat": "你当前以 Chat(只读)身份连接,配置修改已锁定。如需修改,请联系 operator 在「设置 → 安全」中授予 Config 权限,或重新配对时选择 Config。",
  "locked_title": "配置已锁定",
  "locked_notice": "此页面需要 Config 权限。请联系 operator 在「设置 → 安全」中授予,或重新配对时选择 Config。"
}
```

- [ ] **Step 3: 校验 JSON parse + key parity**

Run:
```bash
cd interfaces/webchat && python3 -c "
import json
en=json.load(open('locales/en.json'))['settings']['permission']
zh=json.load(open('locales/zh.json'))['settings']['permission']
assert set(en)==set(zh), f'key mismatch: {set(en)^set(zh)}'
assert set(en)=={'banner_chat','locked_title','locked_notice'}, en.keys()
print('parity OK', sorted(en))
"
```
Expected: `parity OK ['banner_chat', 'locked_notice', 'locked_title']`

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel: add settings.permission i18n keys (en + zh)"
```

---

## Task 3: `ConfigGate` / `PermissionBanner` / `LockedNotice` 组件

镜像 `cluster.rs:79-85` 的 `<Show when=move || state.is_operator()>` 范式。`DashboardState` 是 `Copy`,经 `expect_context` 取得。

**Files:**
- Modify: `interfaces/webchat/src/components/permission.rs`

- [ ] **Step 1: 在 permission.rs 顶部追加组件**

在 `interfaces/webchat/src/components/permission.rs` 文件**顶部**(模块文档注释之后、纯函数之前)插入 imports 与组件:

```rust
use crate::context::DashboardState;
use crate::i18n::*;
use leptos::prelude::*;

/// 配置页闸门:operator 渲染整页 children;非 operator 渲染锁定卡。
/// 在 `SettingsRouter` 路由层包住 config-write 页 —— 门控集中一处。
#[component]
pub fn ConfigGate(children: ChildrenFn) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    view! {
        <Show
            when=move || state.is_operator()
            fallback=move || view! { <LockedNotice /> }
        >
            {children()}
        </Show>
    }
}

/// 非 operator 打开配置页时的锁定卡。
#[component]
fn LockedNotice() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="p-6">
            <div class="bg-surface-raised rounded-lg border border-border p-6 max-w-2xl">
                <h2 class="text-lg font-semibold text-text-primary mb-2">
                    {t!(i18n, settings.permission.locked_title)}
                </h2>
                <p class="text-sm text-text-secondary">
                    {t!(i18n, settings.permission.locked_notice)}
                </p>
            </div>
        </div>
    }
}

/// Settings 区顶部常驻横幅:仅非 operator 显示,解释配置已锁定 + 如何提权。
#[component]
pub fn PermissionBanner() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    view! {
        <Show when=move || !state.is_operator()>
            <div class="mx-4 mt-3 px-4 py-2.5 rounded-lg border border-warning/40 bg-warning/10 text-sm text-text-secondary flex items-start gap-2">
                <svg class="w-4 h-4 mt-0.5 shrink-0 text-warning" viewBox="0 0 24 24" fill="none"
                    stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 9v4" /><path d="M12 17h.01" />
                    <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
                </svg>
                <span>{t!(i18n, settings.permission.banner_chat)}</span>
            </div>
        </Show>
    }
}
```

> 注意:`Show` 的 `fallback` 闭包内引用 `LockedNotice` 组件;`when` 闭包捕获 `Copy` 的 `state`。`ChildrenFn` 允许 `Show` 在切换时重复调用 children。

- [ ] **Step 2: 编译确认组件可用**

Run: `cd interfaces/webchat && cargo build -p aleph-panel --lib 2>&1 | tail -20`
Expected: 编译通过(无 error)。若 `ChildrenFn` 未在 prelude,改 `use leptos::prelude::*;` 已含;若报 `Show` 未导入同理在 prelude。

- [ ] **Step 3: 运行已有测试确认未回归**

Run: `cd interfaces/webchat && cargo test -p aleph-panel permission:: -- --nocapture`
Expected: Task 1 的 4 个测试仍 PASS。

- [ ] **Step 4: 提交**

```bash
git add interfaces/webchat/src/components/permission.rs
git commit -m "panel: add ConfigGate, PermissionBanner, LockedNotice components"
```

---

## Task 4: 路由层接线 —— SettingsRouter 套 ConfigGate + 挂 PermissionBanner

把 `SettingsRouter`(`app.rs:375-427`)的 config-write 路由整页套 `<ConfigGate>`;在 Settings mode 区(`app.rs:345`)挂 `<PermissionBanner>`。read-only / 本地 / 连接管理路由不套。

**Files:**
- Modify: `interfaces/webchat/src/app.rs:345`(挂横幅)
- Modify: `interfaces/webchat/src/app.rs:380-421`(套闸门)

- [ ] **Step 1: 引入组件**

在 `interfaces/webchat/src/app.rs` 顶部 imports 区(其它 `use crate::components::...` 附近)加入:

```rust
use crate::components::permission::{ConfigGate, PermissionBanner};
```

- [ ] **Step 2: Settings mode 区挂 PermissionBanner**

把 `app.rs:345-347` 这段:

```rust
        <div style:display=move || if mode.get() == PanelMode::Settings { "contents" } else { "none" }>
            <SettingsRouter />
        </div>
```

改为(横幅在路由内容之上;`contents` 改为正常块容器以容纳横幅 + 路由两个子节点):

```rust
        <div style:display=move || if mode.get() == PanelMode::Settings { "block" } else { "none" }>
            <PermissionBanner />
            <SettingsRouter />
        </div>
```

> `contents` → `block`:`contents` 不产生盒子,加横幅后需要一个真实容器包住「横幅 + 路由」两个兄弟节点。其余 mode 区不动。

- [ ] **Step 3: config-write 路由套 ConfigGate**

把 `SettingsRouter` match 中下列**每条** config-write 臂的 view 用 `<ConfigGate>` 包住。逐条替换(左→右):

```rust
// Basic
"/settings/general" => view! { <ConfigGate><GeneralView /></ConfigGate> }.into_any(),

// AI
"/settings/search" => view! { <ConfigGate><SearchView /></ConfigGate> }.into_any(),
"/settings/providers" => view! { <ConfigGate><ProvidersView /></ConfigGate> }.into_any(),
"/settings/embedding-providers" => view! { <ConfigGate><EmbeddingProvidersView /></ConfigGate> }.into_any(),
"/settings/reranking-providers" => view! { <ConfigGate><RerankingProvidersView /></ConfigGate> }.into_any(),
"/settings/generation-providers" => view! { <ConfigGate><GenerationProvidersView /></ConfigGate> }.into_any(),
"/settings/model-route" => view! { <ConfigGate><RouteView /></ConfigGate> }.into_any(),
"/settings/memory" => view! { <ConfigGate><MemoryView /></ConfigGate> }.into_any(),

// Extensions
"/settings/routing" => view! { <ConfigGate><RoutingRulesView /></ConfigGate> }.into_any(),
"/settings/mcp" => view! { <ConfigGate><McpView /></ConfigGate> }.into_any(),
"/settings/plugins" => view! { <ConfigGate><PluginsView /></ConfigGate> }.into_any(),
"/settings/skills" => view! { <ConfigGate><SkillsView /></ConfigGate> }.into_any(),
"/settings/clawhub" => view! { <ConfigGate><ClawHubView /></ConfigGate> }.into_any(),
"/settings/acp" => view! { <ConfigGate><AcpHarnessesView /></ConfigGate> }.into_any(),

// Advanced
"/settings/browser" => view! { <ConfigGate><BrowserView /></ConfigGate> }.into_any(),
"/settings/security" => view! { <ConfigGate><SecurityView /></ConfigGate> }.into_any(),
"/settings/auth" => view! { <ConfigGate><AuthView /></ConfigGate> }.into_any(),
"/settings/policies" => view! { <ConfigGate><PoliciesView /></ConfigGate> }.into_any(),
"/settings/execution" => view! { <ConfigGate><ExecutionView /></ConfigGate> }.into_any(),
```

并把 channels 平台页臂(`app.rs:415-421`)的 view 也套上:

```rust
            _ if path.starts_with("/settings/channels/") => {
                let platform_type = path
                    .strip_prefix("/settings/channels/")
                    .unwrap_or("")
                    .to_string();
                view! { <ConfigGate><ChannelPlatformPage platform_type=platform_type /></ConfigGate> }.into_any()
            }
```

**不要改**以下臂(保持原样):`"/settings"`(索引 `<Settings />`)、`"/settings/appearance"`、`"/settings/behavior"`、`"/settings/network"`(含连接目标切换 + cluster.rs 自带门控)、`"/settings/channels"`(概览只读)。

- [ ] **Step 4: 编译确认**

Run: `cd interfaces/webchat && cargo build -p aleph-panel --lib 2>&1 | tail -20`
Expected: 编译通过。若 `ConfigGate` children 类型报错,确认 Task 3 的 `children: ChildrenFn`。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/app.rs
git commit -m "panel: gate config settings routes with ConfigGate + Settings PermissionBanner"
```

---

## Task 5: 连接 chip 加 tier 身份徽章(TDD 纯函数 + 渲染)

在 `ConnectionStatus`(`components/connection_status.rs`)加一个 tier 徽章,复用现有 `settings.security.tier_config` / `tier_chat` i18n 标签与 DeviceCard 徽章配色。先 TDD 纯函数 `tier_badge`。

**Files:**
- Modify: `interfaces/webchat/src/components/connection_status.rs`

- [ ] **Step 1: 写失败测试 + 纯函数**

在 `connection_status.rs` 的 `#[cfg(test)] mod tests` **之前**(文件末尾 `mod tests` 上方)加入纯函数:

```rust
/// 连接身份徽章:从 connect 捕获的 role 派生。operator→Config,guest→Chat,
/// 其余(含未连接的 None / 未知 role)→无徽章。纯函数,host 可测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TierBadge {
    Config,
    Chat,
}

pub(crate) fn tier_badge(role: Option<&str>) -> Option<TierBadge> {
    match role {
        Some("operator") => Some(TierBadge::Config),
        Some("guest") => Some(TierBadge::Chat),
        _ => None,
    }
}
```

在 `mod tests` 内加入测试:

```rust
    #[test]
    fn tier_badge_maps_roles() {
        assert_eq!(tier_badge(Some("operator")), Some(TierBadge::Config));
        assert_eq!(tier_badge(Some("guest")), Some(TierBadge::Chat));
        assert_eq!(tier_badge(None), None);
        assert_eq!(tier_badge(Some("node")), None);
    }
```

- [ ] **Step 2: 运行测试**

Run: `cd interfaces/webchat && cargo test -p aleph-panel connection_status:: -- --nocapture`
Expected: 含新 `tier_badge_maps_roles` 在内全部 PASS(已有 `host_of` / `loopback_detection` 测试不受影响)。

- [ ] **Step 3: chip 渲染 tier 徽章**

在 `ConnectionStatus` 组件内,`status_text` 闭包定义之后、`view!` 之前加入徽章派生(`state` 是 `Copy`,`i18n` 已在作用域):

```rust
    // Tier 身份徽章 —— 从捕获的 role 派生,复用 DeviceCard 的配色与标签。
    let badge = move || {
        tier_badge(state.role.get().as_deref()).map(|b| {
            let (label, cls) = match b {
                TierBadge::Config => (
                    t_string!(i18n, settings.security.tier_config).to_string(),
                    "text-xs px-1.5 py-0.5 rounded bg-indigo-600 text-white shrink-0",
                ),
                TierBadge::Chat => (
                    t_string!(i18n, settings.security.tier_chat).to_string(),
                    "text-xs px-1.5 py-0.5 rounded bg-surface-raised text-text-secondary shrink-0",
                ),
            };
            view! { <span class=cls>{label}</span> }
        })
    };
```

把 `view!` 中状态行的 label span(`connection_status.rs:77-80` 那段 `<div class="flex items-center gap-2 min-w-0">...</div>`)改为在 label 后追加徽章:

```rust
                <div class="flex items-center gap-2 min-w-0">
                    <div class=move || format!("w-2 h-2 rounded-full shrink-0 {}", dot_class())></div>
                    <span class="text-sm font-medium truncate">{status_text}</span>
                    {badge}
                </div>
```

- [ ] **Step 4: 编译确认**

Run: `cd interfaces/webchat && cargo build -p aleph-panel --lib 2>&1 | tail -20`
Expected: 编译通过。

- [ ] **Step 5: 提交**

```bash
git add interfaces/webchat/src/components/connection_status.rs
git commit -m "panel: show connection tier badge (Config/Chat) in status chip"
```

---

## Task 6: WS 错误路径接入 `friendly_error` 兜底

在 `context.rs` 的 RPC 响应错误处理(`context.rs:510-514`)把原始错误消息经 `friendly_error` 映射,给任何漏网写操作的 `PERMISSION_DENIED` 兜底。

**Files:**
- Modify: `interfaces/webchat/src/context.rs:510-514`

- [ ] **Step 1: 替换错误转发**

把 `context.rs:510-514` 这段:

```rust
                                                if let Some(error) = value.get("error") {
                                                    let msg = error.get("message")
                                                        .and_then(|m| m.as_str())
                                                        .unwrap_or("Unknown error");
                                                    let _ = tx.send(Err(msg.to_string()));
                                                } else if let Some(result) = value.get("result") {
```

改为:

```rust
                                                if let Some(error) = value.get("error") {
                                                    let msg = error.get("message")
                                                        .and_then(|m| m.as_str())
                                                        .unwrap_or("Unknown error");
                                                    let msg = crate::components::permission::friendly_error(msg);
                                                    let _ = tx.send(Err(msg));
                                                } else if let Some(result) = value.get("result") {
```

> `friendly_error` 取 `&str` 返回 `String`,非权限错误原样透传 —— 对其余 RPC 错误零行为变化。

- [ ] **Step 2: 编译确认**

Run: `cd interfaces/webchat && cargo build -p aleph-panel --lib 2>&1 | tail -20`
Expected: 编译通过。

- [ ] **Step 3: 提交**

```bash
git add interfaces/webchat/src/context.rs
git commit -m "panel: map PERMISSION_DENIED RPC errors to actionable hint"
```

---

## Task 7: 整体验证(构建 + i18n parity + 冒烟清单)

**Files:** 无(验证任务)

- [ ] **Step 1: 全量 panel 单测**

Run: `cd interfaces/webchat && cargo test -p aleph-panel 2>&1 | tail -25`
Expected: 全部 PASS(含 permission:: 4 + connection_status:: tier_badge + 既有)。

- [ ] **Step 2: WASM release 构建(i18n 编译期 codegen 在此真正校验)**

Run: `just wasm 2>&1 | tail -30`(仓库根)
Expected: 构建成功,产出 `interfaces/webchat/dist/{aleph_panel.js, aleph_panel_bg.wasm, ...}`;leptos_i18n 不报 `settings.permission.*` key 缺失/不对等。

- [ ] **Step 3: clippy 触及零新增警告**

Run: `cd interfaces/webchat && cargo clippy -p aleph-panel --lib 2>&1 | tail -20`
Expected: 无新增 warning(预存警告若有,不在本次范围)。

- [ ] **Step 4: 人工冒烟清单(记录,不阻断)**

部署须重编 binary(rust_embed 编译期烧 dist,见 spec §6 + CLAUDE.md 资源嵌入链):`just wasm` → `cargo build --release -p alephcore --bin aleph-server` → 替换运行中 binary。完成后逐项核对:

- [ ] 本地 loopback Panel(operator):所有配置页正常渲染可用;无横幅;chip 显示 **Config** 徽章(indigo)。
- [ ] 远程 Chat-tier(或临时把设备降级 chat):Settings 区顶部显示锁定横幅;打开 providers/security/channels 等显示锁定卡;`/settings/network`、`/settings/appearance`、channels 概览仍可访问;chip 显示 **Chat** 徽章(灰)。
- [ ] 数据查看页(Dashboard / Memory / Trace / Logs / Usage):两种 tier 均正常,无门控。
- [ ] 在安全页对一台远程设备点「Grant config」→ 该设备刷新/重连后配置页解锁(spec §6 热提权延迟:重连后生效,符合预期)。

- [ ] **Step 5: 提交(若 Step 1-3 有微调)**

```bash
git add -A && git commit -m "panel: verify config tier reflection (tests + wasm build + clippy)"
```

---

## Self-Review 记录

- **Spec 覆盖**:组件 1(横幅)→Task 3+4;组件 2(ConfigGate)→Task 3+4;组件 3(tier 徽章)→Task 5;组件 4(PERMISSION_DENIED 映射)→Task 1+6;i18n→Task 2;验证标准→Task 7。无遗漏。
- **类型一致**:`ConfigGate(children: ChildrenFn)`、`PermissionBanner()`、`is_permission_denied(&str)->bool`、`friendly_error(&str)->String`、`tier_badge(Option<&str>)->Option<TierBadge>` 在定义(Task 1/3/5)与调用(Task 4/5/6)处签名一致。
- **无占位**:每步含完整代码 / 命令 / 预期。`SettingsRouter` 各臂逐条列出(未用"similar to")。
- **后端零改动**:全部改动在 `interfaces/webchat/`;无 `src/` 触碰。
