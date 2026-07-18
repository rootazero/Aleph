# Panel Provider 行为一致性 + 强壮性加固 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 WebChat Panel 三套 provider UI(Chat/生成/Embedding)的 填写·保存·测试·验证·设为默认·徽章 行为逻辑完全一致,消灭结果漂移,稳定密钥回显。

**Architecture:** 纯 Panel 一致性重构 + 后端只读 I/O 整形。后端的 verified/test/setDefault 业务契约已正确(test 成功持久 verified、update 清 verified、setDefault 强制 verified),本计划只让 Panel 忠实反映它,并把"明文回显密钥"改为"只回 has_api_key"。服务器是唯一真相:所有写操作后重拉权威 list,不做乐观本地态。

**Tech Stack:** Rust, Leptos 0.7(CSR/WASM, crate `aleph-panel`), JSON-RPC gateway(crate `alephcore`)。

**关键约定**
- 实施在 **worktree 隔离**中进行(用 `superpowers:using-git-worktrees`)。
- 后端属 `alephcore`,可 `cargo check -p alephcore` / `cargo test -p alephcore --lib` 验证。
- Panel 属 `aleph-panel`,wasm 目标;纯逻辑 helper 加 `#[cfg(test)]` 单测,验证用 `cargo test -p aleph-panel --lib <name>`(若该 crate 无法在 host 编译则降级为构建验证并在 PR 注明)。视图改动主要靠 `just wasm` 构建通过 + 末尾手动 e2e 清单验证。
- 提交规范:`<scope>: <desc>`,English。单分支 main 衍生 worktree,显式路径暂存(勿 `git add -A`,避免卷入并发会话 WIP)。
- i18n:每个新 key 必须 en + zh 同时加,保持 parity。

---

## 文件结构总览

**后端(`src/`,只读 I/O 整形)**
- `src/gateway/handlers/providers/handlers.rs` — `handle_list`/`handle_get` 停止回显 `api_key` 明文(保留 `has_api_key`)
- `src/gateway/handlers/generation_providers/handlers.rs` — `handle_list`/`handle_get` 注入 `has_api_key`,停止注入 `api_key`
- `src/gateway/handlers/embedding_providers.rs` — `handle_list`/`handle_get` 注入 `has_api_key`,停止注入 `api_key`

**Panel 共享原语(`interfaces/webchat/src/components/`)**
- `provider_badge.rs`(新建)— 统一徽章决策 + 渲染(已验证/默认 可并存)
- `provider_key_field.rs`(新建)— 统一密钥输入(空=保持不变,has_api_key 驱动占位/指示)
- `provider_row_card.rs`(改)— 增加可选尾部 slot + 图标尺寸变体,承载 OAuth 行

**Panel API wire(`interfaces/webchat/src/api/`)**
- `providers.rs` — `ProviderInfo` 增 `has_api_key`
- `generation_providers.rs` — `GenerationProviderEntry` 增 `has_api_key`
- `embedding.rs` — `EmbeddingProviderEntry` 增 `has_api_key`

**Panel 视图(`interfaces/webchat/src/views/settings/`)**
- `providers/detail_panel.rs` + `providers/list.rs`
- `generation_providers/detail_view.rs` + `preset_setup.rs` + `add_custom.rs` + `mod.rs`
- `embedding_providers/detail_panel.rs` + `add_panel.rs` + `mod.rs`

**i18n** — `interfaces/webchat/src/i18n/`(en + zh)

---

## Phase A — 后端只读 I/O 整形(alephcore)

### Task A1: Chat 停止明文回显密钥

**Files:**
- Modify: `src/gateway/handlers/providers/handlers.rs:35`(handle_list),`:78`(handle_get)

后端 `ProviderInfo`(`types.rs:7`)已同时有 `has_api_key: bool` 与 `api_key: Option<String>`(skip_serializing_if none)。当前 `handle_list`/`handle_get` 用 `resolve_api_key` 把真 key 填进 `api_key` 明文下发。改为:`has_api_key = resolve_api_key(...).is_some()`,`api_key = None`(不再下发明文)。

- [ ] **Step 1: 改 handle_list**

`handlers.rs` handle_list 中,把:

```rust
            let api_key = resolve_api_key(name, &vault);
            ProviderInfo {
                ...
                has_api_key: api_key.is_some(),
                api_key,
                ...
```

改为:

```rust
            let has_api_key = resolve_api_key(name, &vault).is_some();
            ProviderInfo {
                ...
                has_api_key,
                api_key: None,
                ...
```

- [ ] **Step 2: 改 handle_get**

同样在 handle_get 中,把 `let api_key = resolve_api_key(&params.name, &vault);` 后的 `has_api_key: api_key.is_some(), api_key,` 改为 `has_api_key: resolve_api_key(&params.name, &vault).is_some(), api_key: None,`。

- [ ] **Step 3: 编译验证**

Run: `cargo check -p alephcore`
Expected: 通过(`api_key` 字段仍存在,只是恒为 None;`resolve_api_key` 仍被调用,无 unused 警告)。

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/providers/handlers.rs
git commit -m "gateway: stop echoing chat provider api_key plaintext, keep has_api_key"
```

---

### Task A2: 生成类 list/get 注入 has_api_key,停止注入 api_key

**Files:**
- Modify: `src/gateway/handlers/generation_providers/handlers.rs:50-86`(handle_list),handle_get(`:92+`)

当前 handle_list 在 `cfg_clone.api_key = resolve_api_key(...)` 后,再在 JSON 层手动 `obj.insert("api_key", ...)`(line 72-75)。改为:计算 `has_api_key`,在 entry 级注入 `has_api_key`,删除 api_key 注入。

- [ ] **Step 1: 改 handle_list 的 map**

把:

```rust
            let mut cfg_clone = provider_config;
            cfg_clone.api_key = resolve_api_key(&name, &vault);
            (
                GenerationProviderEntry { name, config: cfg_clone, is_default_for },
                gen_type,
            )
```

改为(不再把 key 塞进 config):

```rust
            let has_api_key = resolve_api_key(&name, &vault).is_some();
            let mut cfg_clone = provider_config;
            cfg_clone.api_key = None;
            (
                GenerationProviderEntry { name, config: cfg_clone, is_default_for },
                gen_type,
                has_api_key,
            )
```

并把元组类型 `Vec<(GenerationProviderEntry, GenerationType)>` 改为 `Vec<(GenerationProviderEntry, GenerationType, bool)>`,排序闭包 `a.0.name.cmp(&b.0.name)` 不变。

- [ ] **Step 2: 改 JSON 序列化段(line 68-86)**

把手动注入 api_key 的块替换为注入 has_api_key:

```rust
    let json_arr: Vec<serde_json::Value> = providers
        .iter()
        .map(|(entry, gen_type, has_api_key)| {
            let mut val = serde_json::to_value(entry).unwrap_or_default();
            if let Some(obj) = val.as_object_mut() {
                obj.insert("has_api_key".into(), serde_json::json!(has_api_key));
                obj.insert(
                    "generation_type".into(),
                    serde_json::Value::String(format!("{:?}", gen_type).to_lowercase()),
                );
            }
            val
        })
        .collect();
```

- [ ] **Step 3: 改 handle_get**

阅读 handle_get(`:92` 起),找到其同样 `config.api_key = resolve_api_key(...)` + 注入 api_key 的逻辑,改为注入 `has_api_key` bool、不注入 api_key 明文(与 list 一致)。若 handle_get 直接返回 entry,则在其 JSON 上 `obj.insert("has_api_key", json!(resolve_api_key(name,&vault).is_some()))` 并把 config.api_key 置 None。

- [ ] **Step 4: 编译验证**

Run: `cargo check -p alephcore`
Expected: 通过。

- [ ] **Step 5: Commit**

```bash
git add src/gateway/handlers/generation_providers/handlers.rs
git commit -m "gateway: generation list/get expose has_api_key, drop api_key echo"
```

---

### Task A3: Embedding list/get 注入 has_api_key,停止注入 api_key

**Files:**
- Modify: `src/gateway/handlers/embedding_providers.rs:99-105`(handle_list),`:134-139`(handle_get)

Embedding 凭证可能来自 vault 或 `api_key_env` 环境变量。`has_api_key` 定义为:vault 有 key **或** 配置了 `api_key_env`(环境变量名非空)。停止注入明文 api_key。

- [ ] **Step 1: 改 handle_list 的 map(line 98-107)**

```rust
        .map(|p| {
            let mut val = inject_is_active(p, &settings.active_provider_id);
            if let Some(obj) = val.as_object_mut() {
                let has_api_key =
                    resolve_api_key(&p.id, &vault).is_some() || p.api_key_env.is_some();
                obj.insert("has_api_key".into(), serde_json::json!(has_api_key));
            }
            val
        })
```

(删除原 `obj.insert("api_key", ...)` 块。)

- [ ] **Step 2: 改 handle_get(line 134-139)**

同样:不注入 api_key,改注入 `has_api_key`:

```rust
            let mut val = inject_is_active(provider, &settings.active_provider_id);
            if let Some(obj) = val.as_object_mut() {
                let has_api_key =
                    resolve_api_key(&params.id, &vault).is_some() || provider.api_key_env.is_some();
                obj.insert("has_api_key".into(), serde_json::json!(has_api_key));
            }
            JsonRpcResponse::success(request.id, val)
```

- [ ] **Step 3: 编译验证**

Run: `cargo check -p alephcore`
Expected: 通过(`resolve_api_key` 仍被 handle_test 等使用,无 unused)。

- [ ] **Step 4: Commit**

```bash
git add src/gateway/handlers/embedding_providers.rs
git commit -m "gateway: embedding list/get expose has_api_key, drop api_key echo"
```

---

## Phase B — Panel 共享原语(aleph-panel)

### Task B1: 统一徽章组件 provider_badge.rs

**Files:**
- Create: `interfaces/webchat/src/components/provider_badge.rs`
- Modify: `interfaces/webchat/src/components/mod.rs`(导出 module)

徽章决策是纯逻辑,可单测;渲染返回 `AnyView` 供 `ProviderRowCard` 的 `badge` slot 复用。「已验证」与「默认」可同时显示。

- [ ] **Step 1: 写纯逻辑 + 失败测试**

新建文件,先写纯函数与测试:

```rust
//! Unified provider status badges shared across Chat / Generation / Embedding
//! provider lists. "Default" and "Verified" can show simultaneously.

use leptos::prelude::*;

/// Pure badge-state decision. `verified` already folds OAuth `connected`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BadgeState {
    pub is_default: bool,
    pub verified: bool,
}

impl BadgeState {
    pub fn any(self) -> bool {
        self.is_default || self.verified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_verified_coexist() {
        let s = BadgeState { is_default: true, verified: true };
        assert!(s.is_default && s.verified && s.any());
    }

    #[test]
    fn empty_state_renders_nothing() {
        assert!(!BadgeState::default().any());
    }
}
```

- [ ] **Step 2: 运行测试确认通过**

Run: `cargo test -p aleph-panel --lib provider_badge`
Expected: 2 passed。(若 crate host 不可编译,跳过并记录,改用 Step 5 构建验证。)

- [ ] **Step 3: 加渲染组件**

在同文件追加(i18n key 在 Task G1 添加,先用 `t_string!` 引用):

```rust
use crate::i18n::*;

/// Render the badge row (`AnyView`) for use in `ProviderRowCard`'s `badge` slot.
/// Pass an i18n-aware closure-free view; caller provides reactive `state`.
#[component]
pub fn ProviderBadges(state: BadgeState) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <span class="flex items-center gap-1">
            {state.is_default.then(|| view! {
                <span class="px-2 py-0.5 rounded-full text-[10px] font-medium bg-primary-subtle text-primary">
                    {t!(i18n, settings.providers.badge_default)}
                </span>
            })}
            {state.verified.then(|| view! {
                <span class="px-2 py-0.5 rounded-full text-[10px] font-medium bg-success-subtle text-success">
                    {t!(i18n, settings.providers.badge_verified)}
                </span>
            })}
        </span>
    }
}
```

- [ ] **Step 4: 导出**

`components/mod.rs` 加 `pub mod provider_badge;`(参照现有 `pub mod provider_row_card;` 风格)。

- [ ] **Step 5: 构建验证**

Run: `just wasm`
Expected: 构建成功,无 `aleph-panel` 报错。

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/components/provider_badge.rs interfaces/webchat/src/components/mod.rs
git commit -m "panel: add unified ProviderBadges (default+verified coexist)"
```

---

### Task B2: 统一密钥输入组件 provider_key_field.rs

**Files:**
- Create: `interfaces/webchat/src/components/provider_key_field.rs`
- Modify: `interfaces/webchat/src/components/mod.rs`

封装稳定回显契约:编辑框**永不预填真 key**;`has_api_key` 驱动占位与状态指示;空=保持不变。配合调用方"脏追踪"——空串提交时映射为 `None`。

- [ ] **Step 1: 纯逻辑 + 测试**

```rust
//! Unified API-key field with stable echo semantics:
//! the input is NEVER pre-filled with the stored secret. `has_api_key`
//! drives the placeholder + a "configured/unset" indicator. An empty
//! value means "keep existing key" (callers map empty -> None on save).

use crate::components::ui::SecretInput;
use crate::i18n::*;
use leptos::prelude::*;

/// Placeholder text key selection is pure logic (testable).
pub fn key_placeholder_is_configured(has_api_key: bool) -> bool {
    has_api_key
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configured_drives_placeholder() {
        assert!(key_placeholder_is_configured(true));
        assert!(!key_placeholder_is_configured(false));
    }
}
```

- [ ] **Step 2: 运行测试**

Run: `cargo test -p aleph-panel --lib provider_key_field`
Expected: passed(或降级构建验证)。

- [ ] **Step 3: 加组件**

```rust
#[component]
pub fn ProviderKeyField(
    /// Editable value; ALWAYS starts empty regardless of stored secret.
    value: RwSignal<String>,
    /// Whether a key is already configured server-side.
    has_api_key: Signal<bool>,
    /// Optional vendor-specific hint placeholder (e.g. "sk-...").
    #[prop(optional)]
    hint: Option<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    let placeholder = move || {
        if has_api_key.get() {
            t_string!(i18n, settings.providers.key_configured_hint).to_string()
        } else {
            hint.clone()
                .unwrap_or_else(|| t_string!(i18n, settings.providers.key_unset_hint).to_string())
        }
    };
    view! {
        <div class="space-y-1">
            <SecretInput
                value=Signal::derive(move || value.get())
                on_change=move |v| value.set(v)
                placeholder=Signal::derive(placeholder)
                monospace=true
            />
            <p class="text-xs text-text-tertiary">
                {move || if has_api_key.get() {
                    t!(i18n, settings.providers.key_status_configured)
                } else {
                    t!(i18n, settings.providers.key_status_unset)
                }}
            </p>
        </div>
    }
}
```

> 注意:`SecretInput` 当前 `placeholder` 是 `String`(见 `components/ui/secret_input.rs`)。若它不接受 `Signal`,Step 3 改为在外层用 `{move || ...}` 包裹两种静态占位分支,或先小改 `SecretInput` 接受 `Signal<String>`(只读整形,记录在 commit)。实现时先读 `secret_input.rs` 确认签名再定形。

- [ ] **Step 4: 导出 + 构建**

`components/mod.rs` 加 `pub mod provider_key_field;`。
Run: `just wasm` → Expected: 成功。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/provider_key_field.rs interfaces/webchat/src/components/mod.rs
git commit -m "panel: add ProviderKeyField with stable echo (blank=keep existing)"
```

---

### Task B3: ProviderRowCard 支持 OAuth 变体

**Files:**
- Modify: `interfaces/webchat/src/components/provider_row_card.rs`

增加可选 props:尾部 slot(chevron)与大图标尺寸,使 OAuth 订阅行能复用本组件。

- [ ] **Step 1: 加可选 props**

在 `ProviderRowCard` 参数表末尾追加:

```rust
    /// Optional trailing element (e.g. a chevron for navigable rows).
    #[prop(optional, into)]
    trailing: Option<ViewFn>,
    /// Use the larger 10x10 icon tile (OAuth subscription rows).
    #[prop(optional)]
    large_icon: bool,
```

- [ ] **Step 2: 应用尺寸 + 尾部**

图标 div 的 `w-8 h-8` 改为 `move || if large_icon { "w-10 h-10" } else { "w-8 h-8" }` 拼进 class;在最外层 `<div class="flex items-center gap-3">` 末尾、name/subtitle 块之后插入:

```rust
                {trailing.map(|t| view! { <div class="ml-auto shrink-0">{t.run()}</div> })}
```

- [ ] **Step 3: 构建验证**

Run: `just wasm`
Expected: 成功;现有 5 处调用未传新 props 仍编译(均为 optional)。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/provider_row_card.rs
git commit -m "panel: ProviderRowCard optional trailing slot + large icon for OAuth rows"
```

---

## Phase C — Panel API wire 结构

### Task C1: ProviderInfo 增 has_api_key

**Files:**
- Modify: `interfaces/webchat/src/api/providers.rs:5-27`

- [ ] **Step 1: 加字段**

在 `ProviderInfo` 的 `verified` 字段旁加:

```rust
    #[serde(default)]
    pub has_api_key: bool,
```

(`api_key` 字段保留——后端现在恒不发,反序列化得 None,Panel 不再用它预填。)

- [ ] **Step 2: 构建**

Run: `just wasm`
Expected: 成功(新增 default 字段不破坏现有构造点;若有 panel 内构造 `ProviderInfo` 的字面量需补 `has_api_key: false`)。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/api/providers.rs
git commit -m "panel(api): ProviderInfo expose has_api_key"
```

---

### Task C2: GenerationProviderEntry 增 has_api_key

**Files:**
- Modify: `interfaces/webchat/src/api/generation_providers.rs:75-84`

- [ ] **Step 1: 加字段**

`GenerationProviderEntry` 加:

```rust
    #[serde(default)]
    pub has_api_key: bool,
```

- [ ] **Step 2: 构建**

Run: `just wasm`
Expected: 成功(修复所有构造 `GenerationProviderEntry` 的字面量补字段)。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/api/generation_providers.rs
git commit -m "panel(api): GenerationProviderEntry expose has_api_key"
```

---

### Task C3: EmbeddingProviderEntry 增 has_api_key

**Files:**
- Modify: `interfaces/webchat/src/api/embedding.rs:5-27`

- [ ] **Step 1: 加字段**

`EmbeddingProviderEntry` 在 `verified` 旁加:

```rust
    #[serde(default)]
    pub has_api_key: bool,
```

- [ ] **Step 2: 构建**

Run: `just wasm`
Expected: 成功。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/api/embedding.rs
git commit -m "panel(api): EmbeddingProviderEntry expose has_api_key"
```

---

## Phase D — Chat 视图

### Task D1: Chat detail_panel 反漂移 + 稳定密钥 + verified 镜像 + 默认门控

**Files:**
- Modify: `interfaces/webchat/src/views/settings/providers/detail_panel.rs`

- [ ] **Step 1: 修复 hydrate Effect 依赖污染**

当前 hydrate Effect(`:53-110`)在 existing 分支读 `providers.get()`(line 82),使 list 刷新会重灌表单。改为:让该 Effect **只依赖选中标识**,通过 `providers.get_untracked()` 读取数据(不订阅)。把 line 82 的 `providers.get()` 改为 `providers.get_untracked()`。这样选中项变化才重灌,后台 list 刷新不再清空编辑内容。

- [ ] **Step 2: 密钥框永不预填真 key**

删除 line 91 `form_api_key.set(provider.api_key.clone().unwrap_or_default());`,改为 `form_api_key.set(String::new());`(始终空)。`__new__`/`__preset__` 分支已是空,无需改。

- [ ] **Step 3: API Key 区改用 ProviderKeyField**

把标准 provider 视图里(`:589-602`)的 `<ApiKeyInput .../>` 替换为:

```rust
<ProviderKeyField
    value=form_api_key
    has_api_key=Signal::derive(move || {
        selected.get()
            .and_then(|s| providers.get().into_iter().find(|p| p.name == s))
            .map(|p| p.has_api_key)
            .unwrap_or(false)
    })
    hint=preset_info.map(|p| p.api_key_placeholder.to_string())
/>
```

并在顶部 `use` 引入 `crate::components::provider_key_field::ProviderKeyField;`,移除不再用的 `ApiKeyInput` import。

- [ ] **Step 4: set_default 门控 verified**

`on_set_default` 按钮(`:707-713` 与 OAuth 分支 `:530-536`)`prop:disabled` 改为同时判断 verified:

```rust
prop:disabled=move || saving.get() || !selected.get()
    .and_then(|s| providers.get().into_iter().find(|p| p.name == s))
    .map(|p| p.verified).unwrap_or(false)
```

并在按钮下加一行未验证提示(i18n `settings.providers.verify_before_default`),仅当未验证时显示。

- [ ] **Step 5: 动作后重拉(已有,确认一致)**

确认 `on_save`/`on_set_default`/`on_delete`/oauth login·logout 末尾都 `ProvidersApi::list(&state)` 重拉并 `providers.set(list)`(现状已如此,保持)。`on_test` 成功后**新增**重拉:

```rust
            match ProvidersApi::test_connection(&state, provider_name.as_deref(), config).await {
                Ok(r) => {
                    test_result.set(Some(r));
                    if let Ok(list) = ProvidersApi::list(&state).await {
                        providers.set(list);
                    }
                }
                Err(e) => error.set(Some(format!("Test failed: {}", e))),
            }
```

(后端测试成功已持久 verified=true,重拉后徽章自动亮。)

- [ ] **Step 6: 构建验证**

Run: `just wasm`
Expected: 成功。

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/settings/providers/detail_panel.rs
git commit -m "panel(chat): anti-drift hydrate, stable key echo, verified-gated default, refetch on test"
```

---

### Task D2: Chat list 统一徽章 + OAuth 行入 ProviderRowCard

**Files:**
- Modify: `interfaces/webchat/src/views/settings/providers/list.rs`

- [ ] **Step 1: preset/custom 行徽章改用 ProviderBadges**

把 `:198-228` 与 `:280-307` 里 `badge=move || { ... if is_default {Default} else if verified {Verified} ... }` 的互斥闭包替换为:

```rust
badge=move || {
    let p = providers.get();
    let s = p.iter().find(|p| p.name == name);
    let state = crate::components::provider_badge::BadgeState {
        is_default: s.map(|p| p.is_default).unwrap_or(false),
        verified: s.map(|p| p.verified).unwrap_or(false),
    };
    view! { <ProviderBadges state=state /> }.into_any()
}
```

(import `use crate::components::provider_badge::{BadgeState, ProviderBadges};`。)

- [ ] **Step 2: OAuth 订阅行改用 ProviderRowCard**

把 `SubscriptionLoginSection`(`:86-154`)里手写的内联行替换为 `ProviderRowCard`,传:`large_icon=true`、`trailing` 给 chevron SVG、`badge` 用 `ProviderBadges`(connected 映射为 verified):

```rust
<ProviderRowCard
    name=display_name.clone()
    icon_color=icon_color.clone()
    subtitle=subtitle.clone()
    is_selected=move || selected.get().as_deref() == Some(provider_id.as_str())
    is_configured=move || connected || is_verified
    dot=move || if connected || is_verified { RowDot::Verified } else { RowDot::None }
    badge=move || view! { <ProviderBadges state=BadgeState { is_default, verified: connected || is_verified } /> }.into_any()
    large_icon=true
    trailing=move || view! { <svg class="w-4 h-4 text-text-tertiary" .../*chevron*/></svg> }.into_any()
    on_click=move || selected.set(Some(provider_id.clone()))
/>
```

(具体变量名以现有 `SubscriptionLoginSection` 内既有绑定为准;保持其 connected/is_verified/is_default 计算逻辑。)

- [ ] **Step 3: 构建验证**

Run: `just wasm`
Expected: 成功。

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/providers/list.rs
git commit -m "panel(chat): unify list badges + fold OAuth row into ProviderRowCard"
```

---

## Phase E — 生成类视图

### Task E1: 生成 detail_view 反漂移 + 稳定密钥 + verified 镜像 + 默认门控

**Files:**
- Modify: `interfaces/webchat/src/views/settings/generation_providers/detail_view.rs`

- [ ] **Step 1: 密钥框永不预填**

line 31 `let form_api_key = RwSignal::new(provider.config.api_key.clone().unwrap_or_default());` 改为 `RwSignal::new(String::new())`。

- [ ] **Step 2: API Key 区改用 ProviderKeyField**

找到 detail_view 渲染 API key 的 `<SecretInput .../>`,替换为:

```rust
<ProviderKeyField
    value=form_api_key
    has_api_key=Signal::derive(move || provider_has_api_key)
    hint=None
/>
```

其中 `let provider_has_api_key = provider.has_api_key;`(Task C2 已加字段)在组件顶部克隆捕获。import `ProviderKeyField`。

- [ ] **Step 3: test 成功后 + setDefault 改为重拉**

当前 save/test/setDefault 用 `on_reload()` callback(由 `mod.rs` 重拉 list)。确认 `on_test` 成功分支也调用 `on_reload()`(让 verified 徽章刷新)。在 test 成功 `set` test_result 后追加 `on_reload();`。

- [ ] **Step 4: setDefault 门控 verified**

set-default 按钮 `disabled` 追加 `|| !config_verified`(line 93 已捕获 `config_verified`),并加未验证提示文案(i18n `settings.providers.verify_before_default`)。

- [ ] **Step 5: 构建 + Commit**

Run: `just wasm` → Expected: 成功。

```bash
git add interfaces/webchat/src/views/settings/generation_providers/detail_view.rs
git commit -m "panel(generation): stable key echo, verified-gated default, refetch on test"
```

---

### Task E2: 生成 preset_setup + add_custom 密钥一致

**Files:**
- Modify: `interfaces/webchat/src/views/settings/generation_providers/preset_setup.rs`
- Modify: `interfaces/webchat/src/views/settings/generation_providers/add_custom.rs`

这两个是"新增"面板(无既有 key),密钥框本就空。统一为 `ProviderKeyField`(`has_api_key=Signal::derive(|| false)`,即恒显"请输入密钥"占位),保证视觉与编辑面板一致。

- [ ] **Step 1: preset_setup 替换 key 输入**

把 preset_setup.rs 里 key 的 `<SecretInput>`/`<ApiKeyInput>` 替换为 `<ProviderKeyField value=form_api_key has_api_key=Signal::derive(|| false) hint=None />`。

- [ ] **Step 2: add_custom 同样替换**

add_custom.rs 同样处理。

- [ ] **Step 3: 构建 + Commit**

Run: `just wasm` → Expected: 成功。

```bash
git add interfaces/webchat/src/views/settings/generation_providers/preset_setup.rs interfaces/webchat/src/views/settings/generation_providers/add_custom.rs
git commit -m "panel(generation): unify key field in preset/custom add panels"
```

---

### Task E3: 生成 mod.rs 行入 ProviderRowCard + 统一徽章

**Files:**
- Modify: `interfaces/webchat/src/views/settings/generation_providers/mod.rs`(`fn ProviderCard` `:389-` 及 preset/custom 行 `:196-296`)

- [ ] **Step 1: ProviderCard 重写为 ProviderRowCard**

把自写内联 `<button>`(`:413+`)的 `ProviderCard` 改为内部调用 `ProviderRowCard`,徽章用 `ProviderBadges`(is_default 来自 `!entry.is_default_for.is_empty()`,verified 来自 `entry.config.verified`),dot 由 verified 驱动。参照 `providers/list.rs:179-228` 的 `ProviderRowCard` 调用法。

- [ ] **Step 2: preset/custom 行徽章统一**

`:196-296` 内行的徽章("Active"/"Default")替换为 `ProviderBadges`,消灭 "Active" 文案。

- [ ] **Step 3: 构建 + Commit**

Run: `just wasm` → Expected: 成功。

```bash
git add interfaces/webchat/src/views/settings/generation_providers/mod.rs
git commit -m "panel(generation): rows via ProviderRowCard + unified badges"
```

---

## Phase F — Embedding 视图

### Task F1: Embedding detail_panel 稳定密钥 + verified 镜像 + 默认门控

**Files:**
- Modify: `interfaces/webchat/src/views/settings/embedding_providers/detail_panel.rs`

- [ ] **Step 1: 密钥框永不预填**

line 35 `let api_key = RwSignal::new(provider.api_key.clone().unwrap_or_default());` 改为 `RwSignal::new(String::new())`。

- [ ] **Step 2: API Key 区改用 ProviderKeyField**

`:223-228` 的 `<SecretInput>` 替换为:

```rust
<ProviderKeyField
    value=api_key
    has_api_key=Signal::derive(move || provider_has_api_key)
    hint=None
/>
```

其中 `let provider_has_api_key = provider.has_api_key;` 顶部捕获。保留下方 `api_key_env` 提示行。

- [ ] **Step 3: 徽章统一 + set-active 门控**

header(`:188-207`)的 "Default"/"Verified" 内联徽章替换为 `<ProviderBadges state=BadgeState { is_default: is_active, verified: provider.verified } />`。set-active 按钮(`:359-367`)`disabled` 追加 `|| !provider.verified`(顶部捕获 `let provider_verified = provider.verified;`),加未验证提示。

- [ ] **Step 4: test 成功后重拉**

test handler 成功分支追加 `on_reload();`(让 verified 徽章刷新)。

- [ ] **Step 5: 构建 + Commit**

Run: `just wasm` → Expected: 成功。

```bash
git add interfaces/webchat/src/views/settings/embedding_providers/detail_panel.rs
git commit -m "panel(embedding): stable key echo, unified badges, verified-gated activate, refetch on test"
```

---

### Task F2: Embedding add_panel 密钥一致

**Files:**
- Modify: `interfaces/webchat/src/views/settings/embedding_providers/add_panel.rs`

- [ ] **Step 1: 替换 key 输入**

把 add_panel.rs 的 key `<SecretInput>` 替换为 `<ProviderKeyField value=api_key has_api_key=Signal::derive(|| false) hint=None />`。

- [ ] **Step 2: 构建 + Commit**

Run: `just wasm` → Expected: 成功。

```bash
git add interfaces/webchat/src/views/settings/embedding_providers/add_panel.rs
git commit -m "panel(embedding): unify key field in add panel"
```

---

### Task F3: Embedding mod.rs 徽章统一

**Files:**
- Modify: `interfaces/webchat/src/views/settings/embedding_providers/mod.rs`(`:163` 与 `:244` 的 ProviderRowCard 调用)

Embedding 已用 ProviderRowCard,只需把两处 `badge` 闭包(`:173-179`、`:254-260`)的互斥逻辑换成 `ProviderBadges`:

- [ ] **Step 1: 替换两处 badge slot**

```rust
badge=move || view! {
    <ProviderBadges state=BadgeState { is_default: is_active, verified: is_verified } />
}.into_any()
```

(`is_active`/`is_verified` 用现有 `:129-130`、`:238-239` 的绑定。)import `ProviderBadges`/`BadgeState`。

- [ ] **Step 2: 构建 + Commit**

Run: `just wasm` → Expected: 成功。

```bash
git add interfaces/webchat/src/views/settings/embedding_providers/mod.rs
git commit -m "panel(embedding): unify list badges via ProviderBadges"
```

---

## Phase G — i18n + 终验

### Task G1: i18n 文案(en + zh)

**Files:**
- Modify: `interfaces/webchat/src/i18n/`(找到 `settings.providers.*` 所在的 en 与 zh 文件)

- [ ] **Step 1: 加 key(en + zh 同步)**

新增以下 key(en 值 / zh 值):
- `settings.providers.badge_default` = "Default" / "默认"
- `settings.providers.badge_verified` = "Verified" / "已验证"
- `settings.providers.verify_before_default` = "Test the connection before setting as default." / "请先测试通过后再设为默认。"
- `settings.providers.key_configured_hint` = "Configured · leave blank to keep" / "已配置 · 留空保持不变"
- `settings.providers.key_unset_hint` = "Not set · enter API key" / "未配置 · 请输入密钥"
- `settings.providers.key_status_configured` = "A key is on file." / "已保存密钥。"
- `settings.providers.key_status_unset` = "No key configured." / "尚未配置密钥。"

(若 generation/embedding 用独立 i18n 命名空间,在其各自命名空间也加同义 key,或统一引用 `settings.providers.*`——实现时确认 i18n 结构,优先复用同一组 key。)

- [ ] **Step 2: 构建验证 parity**

Run: `just wasm`
Expected: 成功(i18n 宏在缺 key 时会编译失败,构建通过即证 en/zh parity)。

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/i18n
git commit -m "panel(i18n): unified provider badge + key-field strings (en/zh)"
```

---

### Task G2: 全量构建 + clippy + 手动 e2e

**Files:** 无(验证任务)

- [ ] **Step 1: 后端检查**

Run: `cargo check -p alephcore && cargo clippy -p alephcore -- -D warnings`
Expected: 通过,无 warning。

- [ ] **Step 2: Panel 单测(best-effort)**

Run: `cargo test -p aleph-panel --lib provider_badge provider_key_field`
Expected: 通过(或记录 host 不可编译,降级)。

- [ ] **Step 3: 重建 wasm + 重嵌 binary 部署**

```bash
just wasm
cargo build --release -p alephcore --bin aleph-server
```

按 CLAUDE.md「Panel↔Daemon 资源嵌入链」替换正在跑的 binary(dev:`./target/release/aleph-server stop` 后重启;.app:替换 `Aleph.app/Contents/MacOS/aleph-server` 后 kill pid 让 supervisor relaunch)。

- [ ] **Step 4: 手动 e2e 清单(对三套各跑一遍)**

逐项确认(对 Chat / 生成 / Embedding 各一遍):
1. 打开已配置 provider → 密钥框为空 + 显示"已配置 · 留空保持不变" + 状态行"已保存密钥"。
2. 不动密钥点保存 → 不报错;重开仍"已配置"(密钥未被冲掉)。
3. 点测试成功 → 无需手动刷新,「已验证」徽章 + 绿点自动亮。
4. 编辑 base_url 保存 → 「已验证」自动消失(后端清 verified)。
5. 未验证时「设为默认」按钮禁用 + 显示"请先测试通过"。
6. 测试通过后「设为默认」可用 → 点击 → 「默认」徽章亮,可与「已验证」并存。
7. 在列表停留时另一端触发 list 刷新(如切走再回)→ 正在输入的密钥/字段不被覆盖。
8. 列表行徽章文案统一为「默认」「已验证」(无 Active/Connected 残留);OAuth 行外观与其它行一致。

- [ ] **Step 5: 收尾**

按 `superpowers:finishing-a-development-branch` 决定合并/PR;worktree 清理用新会话(本会话内 `git worktree remove` 会损 shell,见 CLAUDE.md)。

---

## 自检对照(spec → task)

- 反漂移(spec 3.2)→ D1.Step1(Effect 去依赖)、各 detail 动作后重拉、F/E test 重拉 ✅
- 测试/验证镜像(3.3)→ D1.Step5 / E1.Step3 / F1.Step4 + setDefault 门控 D1.Step4 / E1.Step4 / F1.Step3 ✅
- 徽章统一(3.4)→ B1 + D2 / E3 / F3 + i18n G1 ✅
- 密钥稳定(3.5)→ A1/A2/A3(后端不回显 + has_api_key) + B2(KeyField) + 各 detail 永不预填 + C1/C2/C3 wire ✅
- 行收敛(3.6)→ B3 + D2(OAuth) + E3(生成) + F3(embedding 已用,统一徽章) ✅
- 动作反馈统一(3.7)→ 各 detail 重拉 + 现有 toast 保留 ✅
- 验收(spec 5)→ G2 e2e 清单逐条 ✅
