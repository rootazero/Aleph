# Panel UI i18n (Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add i18n infrastructure to the webchat panel and translate navigation, shell, and settings pages into Chinese using `leptos_i18n`.

**Architecture:** `leptos_i18n` v0.6 generates a type-safe i18n module at compile time from JSON locale files. An `I18nContextProvider` at the app root makes translations available to all components via `use_i18n()` + `t!()` macro. Language switching is wired to the existing Settings → General → Language dropdown.

**Tech Stack:** Leptos 0.8, leptos_i18n 0.6, ICU4X (plurals, datetime, nums), Trunk (WASM build)

**Spec:** `docs/superpowers/specs/2026-03-23-panel-i18n-design.md`

---

## File Map

| Action | File | Responsibility |
|--------|------|----------------|
| Create | `interfaces/webchat/locales/en.json` | English translations (base) |
| Create | `interfaces/webchat/locales/zh.json` | Chinese translations |
| Create | `interfaces/webchat/build.rs` | leptos_i18n code generation |
| Create | `interfaces/webchat/src/i18n.rs` | Re-export generated i18n module |
| Modify | `interfaces/webchat/Cargo.toml` | Add leptos_i18n deps |
| Modify | `interfaces/webchat/src/lib.rs` | Import i18n module |
| Modify | `interfaces/webchat/src/app.rs` | Add I18nContextProvider |
| Modify | `interfaces/webchat/src/components/bottom_bar.rs` | i18n nav labels |
| Modify | `interfaces/webchat/src/components/settings_sidebar.rs` | i18n tab labels + group labels |
| Modify | `interfaces/webchat/src/components/mode_sidebar.rs` | Consume i18n sidebar labels |
| Modify | `interfaces/webchat/src/views/settings/general.rs` | Wire language switch to leptos_i18n |
| Modify | `interfaces/webchat/src/views/settings/mod.rs` | i18n Settings welcome page |
| Modify | `interfaces/webchat/src/views/settings/*.rs` | i18n all settings page titles/descriptions |

**Not modified (Phase 1):**
- `components/top_bar.rs` — "Aleph" is a brand name, not translatable
- `components/forms.rs` — Form component prop signatures (`&'static str`) are Phase 2; Phase 1 only translates page-level text using `t!()` directly in view bodies
- `views/settings/channels/platform_page.rs`, `config_template.rs` — detailed channel config text is Phase 2

---

### Task 1: Add leptos_i18n dependencies

**Files:**
- Modify: `interfaces/webchat/Cargo.toml`

- [ ] **Step 1: Add leptos_i18n to dependencies**

```toml
# Add after the leptos_router line:
leptos_i18n = { version = "0.6", features = ["csr", "cookie", "plurals", "format_datetime", "format_nums"] }

# Add new section:
[build-dependencies]
leptos_i18n_build = "0.6"
```

- [ ] **Step 2: Verify dependency resolution**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p aleph-panel 2>&1 | head -20`

Expected: Dependencies download and resolve (will fail on missing build.rs/locales — that's OK at this step, just verify no version conflicts).

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/Cargo.toml
git commit -m "panel: add leptos_i18n dependencies for i18n support"
```

---

### Task 2: Create locale files and build infrastructure

**Files:**
- Create: `interfaces/webchat/locales/en.json`
- Create: `interfaces/webchat/locales/zh.json`
- Create: `interfaces/webchat/build.rs`
- Create: `interfaces/webchat/src/i18n.rs`
- Modify: `interfaces/webchat/src/lib.rs`

- [ ] **Step 1: Create English locale file with navigation and common keys**

Create `interfaces/webchat/locales/en.json`:

```json
{
  "nav": {
    "chat": "Chat",
    "dashboard": "Dashboard",
    "agents": "Agents",
    "settings": "Settings"
  },
  "common": {
    "loading": "Loading...",
    "saving": "Saving...",
    "saved": "Saved successfully",
    "save": "Save Changes",
    "cancel": "Cancel",
    "delete": "Delete",
    "retry": "Retry",
    "confirm": "Confirm",
    "error": "An error occurred",
    "no_data": "No data available",
    "connect": "Connect to Gateway",
    "disconnect": "Disconnect",
    "connected": "Connected",
    "disconnected": "Disconnected",
    "gateway_unavailable": "Gateway not available — showing default settings"
  },
  "settings": {
    "title": "Settings",
    "welcome": "Welcome to Settings",
    "select_category": "Select a category from the sidebar to configure Aleph Gateway",
    "quick_start": {
      "title": "Quick Start",
      "description": "Configure the essential settings to get started with Aleph",
      "providers": "Set up AI providers and API keys",
      "behavior": "Customize agent behavior",
      "memory": "Enable memory and knowledge base"
    },
    "help": {
      "title": "Need Help?",
      "description": "Learn more about Aleph's features and configuration options",
      "docs": "Check the documentation",
      "community": "Join the community",
      "issues": "Report issues on GitHub",
      "support": "Contact support"
    },
    "groups": {
      "basic": "Basic",
      "ai": "AI",
      "channels": "Channels",
      "extensions": "Extensions",
      "advanced": "Advanced"
    },
    "tabs": {
      "general": "General",
      "behavior": "Behavior",
      "providers": "AI Providers",
      "embedding": "Embedding",
      "reranking": "Reranking",
      "generation": "Generation",
      "memory": "Memory & Knowledge",
      "mcp": "MCP",
      "plugins": "Plugins",
      "skills": "Skills",
      "clawhub": "ClawHub",
      "acp": "ACP",
      "channels": "Channels",
      "telegram": "Telegram",
      "discord": "Discord",
      "whatsapp": "WhatsApp",
      "imessage": "iMessage",
      "search": "Search",
      "policies": "Policies",
      "routing_rules": "Routing Rules",
      "security": "Security",
      "auth": "Token Auth"
    },
    "general": {
      "title": "General Settings",
      "description": "Configure general application settings",
      "language": {
        "title": "Language",
        "label": "Interface Language",
        "system": "System Default",
        "en": "English",
        "zh": "简体中文"
      },
      "config_reload": {
        "title": "Configuration Reload",
        "description": "Reload configuration files from disk and refresh subsystems (profiles, providers).",
        "button": "Reload Config",
        "reloading": "Reloading...",
        "no_config": "No configuration loaded"
      }
    },
    "behavior": {
      "title": "Behavior Settings",
      "description": "Configure agent behavior and output settings"
    },
    "providers": {
      "title": "AI Providers",
      "description": "Configure AI model providers"
    },
    "embedding": {
      "title": "Embedding Providers",
      "description": "Configure embedding model providers"
    },
    "reranking": {
      "title": "Reranking Providers",
      "description": "Configure reranking providers"
    },
    "generation": {
      "title": "Generation Providers",
      "description": "Configure generation model providers"
    },
    "memory": {
      "title": "Memory & Knowledge",
      "description": "Configure memory and knowledge settings"
    },
    "mcp": {
      "title": "MCP Servers",
      "description": "Configure Model Context Protocol servers"
    },
    "plugins": {
      "title": "Plugins",
      "description": "Manage installed plugins"
    },
    "skills": {
      "title": "Skills",
      "description": "Manage agent skills"
    },
    "clawhub": {
      "title": "ClawHub",
      "description": "Browse and install from ClawHub marketplace"
    },
    "acp": {
      "title": "ACP Harnesses",
      "description": "Configure Agent Communication Protocol harnesses"
    },
    "search": {
      "title": "Search Settings",
      "description": "Configure search and retrieval settings"
    },
    "policies": {
      "title": "Policies",
      "description": "Configure security and access policies"
    },
    "routing_rules": {
      "title": "Routing Rules",
      "description": "Configure message routing rules"
    },
    "security": {
      "title": "Security",
      "description": "Configure security settings"
    },
    "auth": {
      "title": "Token Auth",
      "description": "Configure token-based authentication"
    },
    "channels": {
      "title": "Channels",
      "description": "Configure messaging channels"
    }
  }
}
```

- [ ] **Step 2: Create Chinese locale file**

Create `interfaces/webchat/locales/zh.json` — same structure, Chinese values:

```json
{
  "nav": {
    "chat": "聊天",
    "dashboard": "仪表盘",
    "agents": "智能体",
    "settings": "设置"
  },
  "common": {
    "loading": "加载中...",
    "saving": "保存中...",
    "saved": "保存成功",
    "save": "保存更改",
    "cancel": "取消",
    "delete": "删除",
    "retry": "重试",
    "confirm": "确认",
    "error": "发生错误",
    "no_data": "暂无数据",
    "connect": "连接网关",
    "disconnect": "断开连接",
    "connected": "已连接",
    "disconnected": "未连接",
    "gateway_unavailable": "网关不可用 — 显示默认设置"
  },
  "settings": {
    "title": "设置",
    "welcome": "欢迎使用设置",
    "select_category": "从侧边栏选择一个类别来配置 Aleph 网关",
    "quick_start": {
      "title": "快速开始",
      "description": "配置基本设置以开始使用 Aleph",
      "providers": "设置 AI 提供商和 API 密钥",
      "behavior": "自定义智能体行为",
      "memory": "启用记忆与知识库"
    },
    "help": {
      "title": "需要帮助？",
      "description": "了解更多关于 Aleph 的功能和配置选项",
      "docs": "查看文档",
      "community": "加入社区",
      "issues": "在 GitHub 上报告问题",
      "support": "联系支持"
    },
    "groups": {
      "basic": "基础",
      "ai": "AI",
      "channels": "频道",
      "extensions": "扩展",
      "advanced": "高级"
    },
    "tabs": {
      "general": "通用",
      "behavior": "行为",
      "providers": "AI 提供商",
      "embedding": "嵌入",
      "reranking": "重排序",
      "generation": "生成",
      "memory": "记忆与知识",
      "mcp": "MCP",
      "plugins": "插件",
      "skills": "技能",
      "clawhub": "ClawHub",
      "acp": "ACP",
      "channels": "频道",
      "telegram": "Telegram",
      "discord": "Discord",
      "whatsapp": "WhatsApp",
      "imessage": "iMessage",
      "search": "搜索",
      "policies": "策略",
      "routing_rules": "路由规则",
      "security": "安全",
      "auth": "令牌认证"
    },
    "general": {
      "title": "通用设置",
      "description": "配置通用应用设置",
      "language": {
        "title": "语言",
        "label": "界面语言",
        "system": "跟随系统",
        "en": "English",
        "zh": "简体中文"
      },
      "config_reload": {
        "title": "配置重载",
        "description": "从磁盘重新加载配置文件并刷新子系统（配置、提供商）。",
        "button": "重载配置",
        "reloading": "重载中...",
        "no_config": "未加载配置"
      }
    },
    "behavior": {
      "title": "行为设置",
      "description": "配置智能体行为和输出设置"
    },
    "providers": {
      "title": "AI 提供商",
      "description": "配置 AI 模型提供商"
    },
    "embedding": {
      "title": "嵌入提供商",
      "description": "配置嵌入模型提供商"
    },
    "reranking": {
      "title": "重排序提供商",
      "description": "配置重排序提供商"
    },
    "generation": {
      "title": "生成提供商",
      "description": "配置生成模型提供商"
    },
    "memory": {
      "title": "记忆与知识",
      "description": "配置记忆和知识设置"
    },
    "mcp": {
      "title": "MCP 服务器",
      "description": "配置模型上下文协议服务器"
    },
    "plugins": {
      "title": "插件",
      "description": "管理已安装的插件"
    },
    "skills": {
      "title": "技能",
      "description": "管理智能体技能"
    },
    "clawhub": {
      "title": "ClawHub",
      "description": "浏览和安装 ClawHub 市场内容"
    },
    "acp": {
      "title": "ACP 适配器",
      "description": "配置智能体通信协议适配器"
    },
    "search": {
      "title": "搜索设置",
      "description": "配置搜索和检索设置"
    },
    "policies": {
      "title": "策略",
      "description": "配置安全和访问策略"
    },
    "routing_rules": {
      "title": "路由规则",
      "description": "配置消息路由规则"
    },
    "security": {
      "title": "安全",
      "description": "配置安全设置"
    },
    "auth": {
      "title": "令牌认证",
      "description": "配置基于令牌的认证"
    },
    "channels": {
      "title": "频道",
      "description": "配置消息频道"
    }
  }
}
```

- [ ] **Step 3: Create build.rs**

Create `interfaces/webchat/build.rs`:

```rust
fn main() {
    println!("cargo::rerun-if-changed=locales");
    leptos_i18n_build::TranslationsInfos::parse()
        .expect("Failed to parse i18n translations")
        .rerun_if_locales_changed();
}
```

Note: `leptos_i18n_build` reads configuration from `Cargo.toml` metadata. Add the following to `Cargo.toml`:

```toml
[package.metadata.leptos-i18n]
default = "en"
locales = ["en", "zh"]
```

- [ ] **Step 4: Create i18n.rs module**

Create `interfaces/webchat/src/i18n.rs`:

```rust
leptos_i18n::load_locales!();
```

This macro reads the build-time generated translations and creates the `Locale` enum, `I18nContext`, and all typed translation keys.

- [ ] **Step 5: Register i18n module in lib.rs**

In `interfaces/webchat/src/lib.rs`, add the module declaration:

```rust
pub mod i18n;
```

Add it near the top of the file, before other module declarations.

- [ ] **Step 6: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p aleph-panel 2>&1 | tail -20`

Expected: Compilation succeeds. The `leptos_i18n_build` step in `build.rs` will generate the i18n module, and `load_locales!()` will include it. If there are locale file format errors, they'll show as build errors here.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/locales/ interfaces/webchat/build.rs interfaces/webchat/src/i18n.rs interfaces/webchat/Cargo.toml interfaces/webchat/src/lib.rs
git commit -m "panel: add i18n infrastructure with en/zh locale files"
```

---

### Task 3: Add I18nContextProvider to app root

**Files:**
- Modify: `interfaces/webchat/src/app.rs`

- [ ] **Step 1: Add I18nContextProvider wrapper**

In `app.rs`, import the i18n provider and wrap the app content:

```rust
// Add import at top:
use crate::i18n::*;

// In App component, wrap DashboardContext with I18nContextProvider:
#[component]
pub fn App() -> impl IntoView {
    view! {
        <I18nContextProvider>
            <DashboardContext>
                <AppContent />
            </DashboardContext>
        </I18nContextProvider>
    }
}
```

`I18nContextProvider` is generated by `load_locales!()` and provides i18n context to all children. It automatically detects locale from cookie → `navigator.languages` → default (en).

- [ ] **Step 2: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p aleph-panel 2>&1 | tail -10`

Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/app.rs
git commit -m "panel: add I18nContextProvider to app root"
```

---

### Task 4: i18n bottom bar navigation labels

**Files:**
- Modify: `interfaces/webchat/src/components/bottom_bar.rs`

- [ ] **Step 1: Replace hardcoded labels with t! macro**

The `BottomBar` component currently passes `label="Chat"` etc. to `BottomBarItem`. Change `BottomBarItem` to accept a reactive label and use `t!()` at call sites.

In `bottom_bar.rs`:

1. Add import: `use crate::i18n::*;`

2. In `BottomBar` component, get i18n context and replace label props:

```rust
#[component]
pub fn BottomBar() -> impl IntoView {
    let i18n = use_i18n();
    let location = use_location();
    let navigate = use_navigate();

    // ... active_mode and go closures stay the same ...

    view! {
        <nav class="h-12 bg-sidebar border-t border-border flex justify-around items-center flex-shrink-0">
            <BottomBarItem
                label=t_string!(i18n, nav.chat)
                mode=PanelMode::Chat
                active_mode=Signal::derive(active_mode)
                on_click=go("/chat")
            >
                // ... SVG path unchanged ...
            </BottomBarItem>
            // Repeat for Dashboard, Agents, Settings with:
            // t_string!(i18n, nav.dashboard)
            // t_string!(i18n, nav.agents)
            // t_string!(i18n, nav.settings)
        </nav>
    }
}
```

3. Change `BottomBarItem` signature from `label: &'static str` to `label: Signal<String>`:

```rust
#[component]
fn BottomBarItem(
    label: Signal<String>,
    mode: PanelMode,
    active_mode: Signal<PanelMode>,
    on_click: impl Fn(web_sys::MouseEvent) + 'static,
    children: Children,
) -> impl IntoView {
    // ... body unchanged, label is already used as {label} in view ...
}
```

Note on `leptos_i18n` macros:
- `t!(i18n, key)` — returns `impl IntoView`, use directly in `view!{}` markup
- `t_string!(i18n, key)` — returns `String`, use for component props and programmatic access

For `Signal<String>` props, wrap with `Signal::derive`:
```rust
label=Signal::derive(move || t_string!(i18n, nav.chat))
```

Verify exact macro names against `leptos_i18n` 0.6 docs during implementation — the API may differ slightly.

- [ ] **Step 2: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p aleph-panel 2>&1 | tail -10`

Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/bottom_bar.rs
git commit -m "panel: i18n bottom bar navigation labels"
```

---

### Task 5: i18n settings sidebar labels

**Files:**
- Modify: `interfaces/webchat/src/components/settings_sidebar.rs`
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs`

- [ ] **Step 1: Replace SettingsTab::label() with i18n keys**

The current `SettingsTab::label()` returns `&'static str`. We need to replace this with i18n lookups. Two approaches:

**Approach A (recommended):** Remove `label()` method entirely. In `mode_sidebar.rs` where labels are rendered, use `t!()` directly with a match on the tab variant:

In `settings_sidebar.rs`, add a method that returns the translation key path:

```rust
use crate::i18n::*;

impl SettingsTab {
    /// Get the i18n-aware label for this tab
    pub fn i18n_label(&self, i18n: I18nContext) -> String {
        match self {
            Self::General => t_string!(i18n, settings.tabs.general),
            Self::Behavior => t_string!(i18n, settings.tabs.behavior),
            Self::Providers => t_string!(i18n, settings.tabs.providers),
            Self::EmbeddingProviders => t_string!(i18n, settings.tabs.embedding),
            Self::RerankingProviders => t_string!(i18n, settings.tabs.reranking),
            Self::GenerationProviders => t_string!(i18n, settings.tabs.generation),
            Self::Memory => t_string!(i18n, settings.tabs.memory),
            Self::Mcp => t_string!(i18n, settings.tabs.mcp),
            Self::Plugins => t_string!(i18n, settings.tabs.plugins),
            Self::Skills => t_string!(i18n, settings.tabs.skills),
            Self::ClawHub => t_string!(i18n, settings.tabs.clawhub),
            Self::Acp => t_string!(i18n, settings.tabs.acp),
            Self::Channels => t_string!(i18n, settings.tabs.channels),
            Self::Telegram => t_string!(i18n, settings.tabs.telegram),
            Self::Discord => t_string!(i18n, settings.tabs.discord),
            Self::WhatsApp => t_string!(i18n, settings.tabs.whatsapp),
            Self::IMessage => t_string!(i18n, settings.tabs.imessage),
            Self::Search => t_string!(i18n, settings.tabs.search),
            Self::Policies => t_string!(i18n, settings.tabs.policies),
            Self::RoutingRules => t_string!(i18n, settings.tabs.routing_rules),
            Self::Security => t_string!(i18n, settings.tabs.security),
            Self::Auth => t_string!(i18n, settings.tabs.auth),
        }
    }
}
```

Note: The exact return type of `t_string!()` depends on `leptos_i18n` version. It may return `String`, `&str`, or a custom type. Adjust accordingly. If `t_string!()` is not available, use `t!()` which returns an `impl IntoView` and use it directly in the view macro.

Also change `SettingsGroup` to use i18n for group labels. Since `SETTINGS_GROUPS` is a `const`, we can't put i18n calls in it. Instead, add an `i18n_label` helper:

```rust
impl SettingsGroup {
    pub fn i18n_label(&self, i18n: I18nContext) -> String {
        match self.label {
            "Basic" => t_string!(i18n, settings.groups.basic),
            "AI" => t_string!(i18n, settings.groups.ai),
            "Channels" => t_string!(i18n, settings.groups.channels),
            "Extensions" => t_string!(i18n, settings.groups.extensions),
            "Advanced" => t_string!(i18n, settings.groups.advanced),
            other => other.to_string(),
        }
    }
}
```

- [ ] **Step 2: Update mode_sidebar.rs to use i18n labels**

In `mode_sidebar.rs`, the `SettingsSidebar` component iterates over `SETTINGS_GROUPS` and renders `tab.label()` and `group.label`. Update to use i18n:

```rust
use crate::i18n::*;

#[component]
fn SettingsSidebar() -> impl IntoView {
    let i18n = use_i18n();
    let location = use_location();

    view! {
        <div class="flex flex-col h-full overflow-y-auto">
            {SETTINGS_GROUPS.iter().map(|group| {
                let group_label = group.i18n_label(i18n);
                view! {
                    <div class="px-3 py-2 space-y-0.5">
                        <h3 class="px-3 py-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
                            {group_label}
                        </h3>
                        {group.tabs.iter().map(|tab| {
                            let path = tab.path();
                            let tab_label = tab.i18n_label(i18n);
                            // ... rest unchanged, use tab_label instead of tab.label() ...
                        }).collect_view()}
                    </div>
                }
            }).collect_view()}
        </div>
    }
}
```

Note: If `t_string!()` returns a reactive value, the labels will auto-update on locale change. If it returns a plain `String`, you may need to wrap the sidebar rendering in a `move ||` closure to re-evaluate on locale change.

- [ ] **Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p aleph-panel 2>&1 | tail -10`

Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/settings_sidebar.rs interfaces/webchat/src/components/mode_sidebar.rs
git commit -m "panel: i18n settings sidebar tab and group labels"
```

---

### Task 6: Wire language switching in Settings → General

**Files:**
- Modify: `interfaces/webchat/src/views/settings/general.rs`

- [ ] **Step 1: Replace hardcoded strings and wire locale switching**

In `general.rs`, replace all hardcoded strings with `t!()` calls and wire the language dropdown to `leptos_i18n`'s `set_locale()`:

```rust
use crate::i18n::*;

#[component]
pub fn GeneralView() -> impl IntoView {
    let i18n = use_i18n();
    let state = expect_context::<DashboardState>();
    // ... existing signals ...

    view! {
        <div class="p-8 max-w-4xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 text-text-primary">
                    {t!(i18n, settings.general.title)}
                </h1>
                <p class="text-text-secondary">
                    {t!(i18n, settings.general.description)}
                </p>
            </div>
            // ... loading/error states use t!(i18n, common.loading) etc. ...
            // ... LanguageSection unchanged in structure ...
        </div>
    }
}
```

- [ ] **Step 2: Update LanguageSection to set locale**

The key change is in `LanguageSection` — when user selects a language, also call `i18n.set_locale()`:

```rust
#[component]
fn LanguageSection(
    language: Option<String>,
    on_change: impl Fn(Option<String>) + 'static + Copy,
) -> impl IntoView {
    let i18n = use_i18n();
    let (selected, set_selected) = signal(language.unwrap_or_else(|| "system".to_string()));

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-4">
                {t!(i18n, settings.general.language.title)}
            </h2>

            <div>
                <label class="block text-sm font-medium text-text-secondary mb-2">
                    {t!(i18n, settings.general.language.label)}
                </label>
                <select
                    prop:value=move || selected.get()
                    on:change=move |ev| {
                        let value = event_target_value(&ev);
                        set_selected.set(value.clone());

                        // Set leptos_i18n locale
                        match value.as_str() {
                            "en" => i18n.set_locale(Locale::en),
                            "zh" => i18n.set_locale(Locale::zh),
                            _ => {
                                // "system" — detect from browser
                                let browser_lang = web_sys::window()
                                    .and_then(|w| w.navigator().language())
                                    .unwrap_or_default();
                                if browser_lang.starts_with("zh") {
                                    i18n.set_locale(Locale::zh);
                                } else {
                                    i18n.set_locale(Locale::en);
                                }
                            }
                        }

                        // Also save to backend
                        let lang = if value == "system" { None } else { Some(value) };
                        on_change(lang);
                    }
                    class="w-full px-3 py-2 bg-surface-sunken border border-border rounded-lg text-text-primary focus:outline-none focus:ring-2 focus:ring-primary/30"
                >
                    <option value="system">{t!(i18n, settings.general.language.system)}</option>
                    <option value="en">{t!(i18n, settings.general.language.en)}</option>
                    <option value="zh">{t!(i18n, settings.general.language.zh)}</option>
                </select>
            </div>
        </div>
    }
}
```

Note: The language dropdown is now simplified to only show supported locales (system, en, zh) instead of the previous unsupported options (ja, ko, zh-Hant). Those can be added when translations are ready.

- [ ] **Step 3: Restore locale from GeneralConfig on connect**

When the WebSocket connects and `GeneralConfig` is loaded, if no cookie exists, apply the backend language preference:

```rust
// In GeneralView's config load callback, after set_config:
if let Some(ref lang) = cfg.language {
    // Only override if cookie hasn't already set a locale
    // (leptos_i18n reads cookie on init, so we only apply backend
    //  preference if the current locale is still the default)
    match lang.as_str() {
        "en" => i18n.set_locale(Locale::en),
        "zh" => i18n.set_locale(Locale::zh),
        _ => {} // unknown language, keep current
    }
}
```

This implements the spec's initialization priority: cookie (handled by leptos_i18n automatically) → GeneralConfig (this code) → browser detection (leptos_i18n fallback).

- [ ] **Step 4: i18n the ConfigReloadSection (renumbered from Step 3)**

Replace hardcoded strings in `ConfigReloadSection`:

```rust
#[component]
fn ConfigReloadSection() -> impl IntoView {
    let i18n = use_i18n();
    // ... existing logic ...

    view! {
        <div class="bg-surface-raised border border-border rounded-xl p-6">
            <h2 class="text-xl font-semibold text-text-primary mb-2">
                {t!(i18n, settings.general.config_reload.title)}
            </h2>
            <p class="text-sm text-text-secondary mb-4">
                {t!(i18n, settings.general.config_reload.description)}
            </p>
            <button ...>
                {move || if reloading.get() {
                    t!(i18n, settings.general.config_reload.reloading).into_any()
                } else {
                    t!(i18n, settings.general.config_reload.button).into_any()
                }}
            </button>
            // ... result messages stay dynamic (from API) ...
        </div>
    }
}
```

Note: The `t!()` macro returns something that `impl IntoView`. For conditional rendering (`if/else`), you may need `.into_any()` on each branch, or use `t_string!()` for String values. Adjust based on what compiles.

- [ ] **Step 4: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p aleph-panel 2>&1 | tail -10`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/settings/general.rs
git commit -m "panel: i18n general settings page and wire language switching"
```

---

### Task 7: i18n settings welcome page and remaining settings pages

**Files:**
- Modify: `interfaces/webchat/src/views/settings/mod.rs` (Settings welcome page)
- Modify: `interfaces/webchat/src/views/settings/behavior.rs`
- Modify: `interfaces/webchat/src/views/settings/providers.rs`
- Modify: `interfaces/webchat/src/views/settings/embedding_providers.rs`
- Modify: `interfaces/webchat/src/views/settings/reranking_providers.rs`
- Modify: `interfaces/webchat/src/views/settings/generation_providers.rs`
- Modify: `interfaces/webchat/src/views/settings/generation.rs`
- Modify: `interfaces/webchat/src/views/settings/memory.rs`
- Modify: `interfaces/webchat/src/views/settings/mcp.rs`
- Modify: `interfaces/webchat/src/views/settings/plugins.rs`
- Modify: `interfaces/webchat/src/views/settings/skills.rs`
- Modify: `interfaces/webchat/src/views/settings/clawhub.rs`
- Modify: `interfaces/webchat/src/views/settings/acp_harnesses.rs`
- Modify: `interfaces/webchat/src/views/settings/search.rs`
- Modify: `interfaces/webchat/src/views/settings/policies.rs`
- Modify: `interfaces/webchat/src/views/settings/routing_rules.rs`
- Modify: `interfaces/webchat/src/views/settings/security.rs`
- Modify: `interfaces/webchat/src/views/settings/auth.rs`
- Modify: `interfaces/webchat/src/views/settings/channels/overview.rs`

**Scope:** Replace page titles (`<h1>`), descriptions (`<p>`), and common strings (`"Loading..."`, `"Saving..."`). Detailed form field labels within each page are Phase 2 scope.

- [ ] **Step 0: i18n the Settings welcome page (mod.rs)**

The `Settings` component in `mod.rs` has "Welcome to Settings", "Quick Start", "Need Help?" and related text. Replace all with `t!()`:

```rust
use crate::i18n::*;

#[component]
pub fn Settings() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="p-8 max-w-5xl mx-auto">
            <div class="mb-8">
                <h1 class="text-3xl font-bold mb-2 text-text-primary">
                    {t!(i18n, settings.welcome)}
                </h1>
                <p class="text-text-secondary">
                    {t!(i18n, settings.select_category)}
                </p>
            </div>
            <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                <div class="p-6 bg-surface-raised border border-border rounded-xl">
                    <h3 class="text-lg font-semibold text-text-primary mb-2">
                        {t!(i18n, settings.quick_start.title)}
                    </h3>
                    <p class="text-sm text-text-secondary mb-4">
                        {t!(i18n, settings.quick_start.description)}
                    </p>
                    <ul class="space-y-2 text-sm text-text-secondary">
                        <li>"• " {t!(i18n, settings.quick_start.providers)}</li>
                        <li>"• " {t!(i18n, settings.quick_start.behavior)}</li>
                        <li>"• " {t!(i18n, settings.quick_start.memory)}</li>
                    </ul>
                </div>
                <div class="p-6 bg-surface-raised border border-border rounded-xl">
                    <h3 class="text-lg font-semibold text-text-primary mb-2">
                        {t!(i18n, settings.help.title)}
                    </h3>
                    <p class="text-sm text-text-secondary mb-4">
                        {t!(i18n, settings.help.description)}
                    </p>
                    <ul class="space-y-2 text-sm text-text-secondary">
                        <li>"• " {t!(i18n, settings.help.docs)}</li>
                        <li>"• " {t!(i18n, settings.help.community)}</li>
                        <li>"• " {t!(i18n, settings.help.issues)}</li>
                        <li>"• " {t!(i18n, settings.help.support)}</li>
                    </ul>
                </div>
            </div>
        </div>
    }
}
```

- [ ] **Step 1: Add i18n imports and replace page headers**

For each settings view file, apply this pattern:

```rust
// Add at top:
use crate::i18n::*;

// In the component, get i18n context:
let i18n = use_i18n();

// Replace page title and description:
// Before: <h1>"Behavior Settings"</h1>
// After:  <h1>{t!(i18n, settings.behavior.title)}</h1>
// Before: <p>"Configure agent behavior..."</p>
// After:  <p>{t!(i18n, settings.behavior.description)}</p>
```

Apply to all 17 settings view files listed above. Each page has a consistent pattern of `<h1>` title + `<p>` description at the top.

Also replace common strings like `"Loading..."` with `t!(i18n, common.loading)` and `"Saving..."` with `t!(i18n, common.saving)` where they appear.

- [ ] **Step 2: Add any missing keys to locale files**

If any settings pages have strings not yet in `en.json`/`zh.json`, add them. The `leptos_i18n` build step will fail if a key used in `t!()` doesn't exist in the locale files — use this as a guide.

- [ ] **Step 3: Verify compilation**

Run: `cd /Users/zouguojun/Workspace/Aleph && cargo check -p aleph-panel 2>&1 | tail -20`

Expected: PASS. If any keys are missing, the build error will point to the exact missing key.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/settings/ interfaces/webchat/locales/
git commit -m "panel: i18n all settings page titles and descriptions"
```

---

### Task 8: Full WASM build verification

**Files:** None (verification only)

- [ ] **Step 1: Run full Trunk build**

Run: `cd /Users/zouguojun/Workspace/Aleph/interfaces/webchat && trunk build 2>&1 | tail -20`

Expected: WASM build succeeds. This validates that all i18n code works in the WASM target, not just native compilation.

If Trunk is not installed or fails, fall back to:
```bash
cd /Users/zouguojun/Workspace/Aleph && cargo check -p aleph-panel --target wasm32-unknown-unknown 2>&1 | tail -20
```

- [ ] **Step 2: Manual browser verification**

Start the dev server and verify in browser:

```bash
cd /Users/zouguojun/Workspace/Aleph && just dev
```

Open the panel URL and verify:
1. Bottom bar shows "Chat", "Dashboard", "Agents", "Settings" in English (default)
2. Navigate to Settings → General → Language dropdown
3. Select "简体中文" → all navigation labels, sidebar labels, and settings page titles switch to Chinese
4. Refresh page → Chinese persists (cookie)
5. Switch to "System Default" → follows browser language
6. Switch back to "English" → everything reverts

- [ ] **Step 3: Commit any fixes**

If browser testing reveals issues, fix and commit:

```bash
git add -u
git commit -m "panel: fix i18n issues found during browser testing"
```
