# Multi-Bot Panel UI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Adapt the Panel UI so each platform's detail page shows a left sidebar with bot instance list and right panel with selected instance's config, and the overview page shows instance count badges.

**Architecture:** Create `ChannelPlatformPage` component with master-detail layout. Refactor `ChannelConfigTemplate` to accept an `instance_id` prop. Replace 13 static per-platform routes with a single dynamic route using `starts_with` matching. Delete unused legacy custom channel views.

**Tech Stack:** Leptos 0.7 (Rust WASM), Tailwind CSS, JSON-RPC over WebSocket

**Build check command:** `cargo check -p aleph-panel --target wasm32-unknown-unknown`

**Full build command:** `just wasm`

---

### Task 1: Delete unused legacy channel view files

**Files:**
- Delete: `apps/panel/src/views/settings/channels/telegram.rs`
- Delete: `apps/panel/src/views/settings/channels/whatsapp.rs`
- Delete: `apps/panel/src/views/settings/channels/imessage.rs`
- Modify: `apps/panel/src/views/settings/channels/mod.rs`

**Step 1: Remove module declarations and re-exports**

In `apps/panel/src/views/settings/channels/mod.rs`, change from:

```rust
pub mod telegram;
pub mod discord;
pub mod whatsapp;
pub mod imessage;
pub mod definitions;
pub mod config_template;
pub mod overview;

pub use telegram::TelegramChannelView;
pub use discord::DiscordChannelView;
pub use whatsapp::WhatsAppChannelView;
pub use imessage::IMessageChannelView;
pub use definitions::{ChannelDefinition, FieldDef, FieldKind, ALL_CHANNELS};
pub use config_template::ChannelConfigTemplate;
pub use overview::ChannelsOverview;
```

to:

```rust
pub mod discord;
pub mod definitions;
pub mod config_template;
pub mod overview;

pub use discord::DiscordChannelView;
pub use definitions::{ChannelDefinition, FieldDef, FieldKind, ALL_CHANNELS};
pub use config_template::ChannelConfigTemplate;
pub use overview::ChannelsOverview;
```

**Step 2: Delete the three files**

```bash
rm apps/panel/src/views/settings/channels/telegram.rs
rm apps/panel/src/views/settings/channels/whatsapp.rs
rm apps/panel/src/views/settings/channels/imessage.rs
```

**Step 3: Remove any imports in app.rs that reference deleted views**

In `apps/panel/src/app.rs`, check for `TelegramChannelView`, `WhatsAppChannelView`, `IMessageChannelView` imports. They are re-exported via `use crate::views::settings::*;` — since we removed re-exports from mod.rs, this is handled automatically. But verify no direct imports exist.

**Step 4: Verify compilation**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: Pass (routes in app.rs still reference wrapper components, not the deleted views directly).

**Step 5: Commit**

```bash
git add -A apps/panel/src/views/settings/channels/
git commit -m "panel: remove unused legacy channel views (telegram, whatsapp, imessage)"
```

---

### Task 2: Refactor `ChannelConfigTemplate` to accept `instance_id`

**Files:**
- Modify: `apps/panel/src/views/settings/channels/config_template.rs`

This is the core change. The template currently uses `definition.id` and `definition.config_section` as static channel identifiers. We need it to accept a dynamic `instance_id` so it loads config for a specific instance.

**Step 1: Change the component signature**

At line 34, change from:

```rust
#[component]
pub fn ChannelConfigTemplate(definition: &'static ChannelDefinition) -> impl IntoView {
```

to:

```rust
#[component]
pub fn ChannelConfigTemplate(
    definition: &'static ChannelDefinition,
    /// The channel instance id (e.g. "telegram-main"). Config is loaded from channels.{instance_id}.
    instance_id: String,
) -> impl IntoView {
```

**Step 2: Replace static channel_id and config_section with dynamic instance_id**

Replace lines 47-48:
```rust
    let channel_id: &'static str = definition.id;
    let config_section: &'static str = definition.config_section;
```

with:
```rust
    let channel_id = instance_id.clone();
    let config_section = format!("channels.{}", instance_id);
```

**Step 3: Fix all closures that capture channel_id**

Since `channel_id` is no longer `&'static str` but `String`, closures that move it need clones:

- Line 53: `config_section.split_once('.')` — this now operates on the `String`. The `(top_section, channel_sub_key)` destructure needs to produce owned strings:

```rust
    let (top_section, channel_sub_key) = config_section
        .split_once('.')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .unwrap_or((config_section.clone(), String::new()));
```

- The `spawn_local` blocks that capture `channel_id` — they already move `String` so no issue.
- The `on_connect` and `on_disconnect` closures at lines 161, 188 — they do `let id = channel_id.to_string()`. Since `channel_id` is already a `String`, change to `let id = channel_id.clone()` and make sure each closure captures its own clone:

```rust
    let channel_id_for_connect = channel_id.clone();
    let on_connect = move || {
        // ...
        let id = channel_id_for_connect.clone();
        // ...
    };

    let channel_id_for_disconnect = channel_id.clone();
    let on_disconnect = move || {
        // ...
        let id = channel_id_for_disconnect.clone();
        // ...
    };
```

- The `on_save` closure at line 120 — uses `config_section`:

```rust
    let config_section_for_save = config_section.clone();
    let on_save = move || {
        // ...
        let section = config_section_for_save.clone();
        // ...
    };
```

**Step 4: Remove the back link and outer header from ChannelConfigTemplate**

The back link, icon header, and page wrapper `<div class="flex-1 p-6 ...">` will move to `ChannelPlatformPage`. The template should only render the connection status card, fields, and action bar — it becomes a "panel content" component, not a full page.

Remove from the view:
- The outer `<div class="flex-1 p-6 overflow-y-auto bg-surface">` wrapper
- The `<A href="/settings/channels">` back link
- The header section (icon + name + description)

Keep:
- Connection status card
- Error/Success messages
- Loading state / field section
- Action bar (Save + docs link)

The resulting view should start with:
```rust
    view! {
        <div class="space-y-6">
            // Connection status card
            // ...
            // Error/Success
            // ...
            // Fields
            // ...
            // Action bar
            // ...
        </div>
    }
```

**Step 5: Add Delete Instance button**

Add a delete button next to the docs link in the action bar:

```rust
                // ---- Action bar: Save + Delete + docs link ----
                <div class="flex items-center justify-between">
                    <div class="flex items-center gap-3">
                        <SaveButton
                            on_click=move || on_save()
                            loading=saving.into()
                            text="Save Configuration"
                        />
                        <button
                            on:click=move |_| on_delete()
                            class="px-3 py-1.5 text-sm border border-danger/30 text-danger rounded-lg hover:bg-danger-subtle transition-colors"
                        >
                            "Delete Instance"
                        </button>
                    </div>
                    // docs link stays
                </div>
```

Add the delete handler:

```rust
    let channel_id_for_delete = channel_id.clone();
    let on_delete = move || {
        if !state.is_connected.get() {
            return;
        }
        let id = channel_id_for_delete.clone();
        spawn_local(async move {
            match state
                .rpc_call("channel.delete", json!({ "id": id }))
                .await
            {
                Ok(_) => {
                    // Navigate back or signal parent to refresh
                    // For now, use window location to go back
                    if let Some(window) = web_sys::window() {
                        let _ = window.location().set_href("/settings/channels");
                    }
                }
                Err(e) => {
                    error.set(Some(format!("Failed to delete: {}", e)));
                }
            }
        });
    };
```

**Step 6: Add an `on_delete` callback prop instead of hard-navigating**

Actually, since the template is embedded in ChannelPlatformPage, a better pattern is to accept an `on_deleted` callback:

```rust
#[component]
pub fn ChannelConfigTemplate(
    definition: &'static ChannelDefinition,
    instance_id: String,
    /// Called after instance is successfully deleted, so parent can refresh the list
    #[prop(optional)]
    on_deleted: Option<Callback<()>>,
) -> impl IntoView {
```

In the delete handler, call `on_deleted` instead of navigating:
```rust
                Ok(_) => {
                    if let Some(cb) = on_deleted {
                        cb.run(());
                    }
                }
```

**Step 7: Verify compilation**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: Errors expected — callers in app.rs don't pass `instance_id` yet. That's OK, we fix callers in Task 4.

**Step 8: Commit**

```bash
git add apps/panel/src/views/settings/channels/config_template.rs
git commit -m "panel: refactor ChannelConfigTemplate to accept instance_id prop"
```

---

### Task 3: Create `ChannelPlatformPage` with master-detail layout

**Files:**
- Create: `apps/panel/src/views/settings/channels/platform_page.rs`
- Modify: `apps/panel/src/views/settings/channels/mod.rs`

**Step 1: Create the platform_page.rs file**

```rust
//! Channel platform detail page with master-detail layout.
//!
//! Left sidebar: instance list with status indicators and "New" button.
//! Right panel: selected instance config (template-driven or Discord custom).

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use serde_json::json;

use crate::components::ui::channel_status::{ChannelStatus, ChannelStatusBadge};
use crate::context::DashboardState;

use super::config_template::ChannelConfigTemplate;
use super::definitions::{ChannelDefinition, ALL_CHANNELS};
use super::DiscordChannelView;

/// Info about a channel instance for the sidebar list.
#[derive(Clone, Debug)]
struct InstanceInfo {
    id: String,
    status: ChannelStatus,
}

/// Master-detail page for a single platform type.
///
/// Looks up the `ChannelDefinition` from the URL path, loads all instances
/// of that platform type, and renders sidebar + config panel.
#[component]
pub fn ChannelPlatformPage(
    /// Platform type id from URL (e.g. "telegram", "discord")
    platform_type: String,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();

    // Look up the static definition for this platform
    let definition: Option<&'static ChannelDefinition> = ALL_CHANNELS
        .iter()
        .find(|d| d.id == platform_type);

    let definition = match definition {
        Some(d) => d,
        None => {
            return view! {
                <div class="flex-1 p-6 bg-surface">
                    <p class="text-text-secondary">"Unknown channel type."</p>
                </div>
            }.into_any();
        }
    };

    let platform_type_owned = platform_type.clone();
    let instances = RwSignal::new(Vec::<InstanceInfo>::new());
    let selected_id = RwSignal::new(Option::<String>::None);

    // A version counter to trigger re-fetches when instances change
    let refresh_trigger = RwSignal::new(0u32);

    // Fetch instances on mount and when refresh_trigger changes
    Effect::new(move |_| {
        let _ = refresh_trigger.get(); // subscribe to trigger
        let pt = platform_type_owned.clone();
        spawn_local(async move {
            match state.rpc_call("channels.list", json!({})).await {
                Ok(val) => {
                    if let Some(channels) = val.get("channels").and_then(|c| c.as_array()) {
                        let filtered: Vec<InstanceInfo> = channels
                            .iter()
                            .filter_map(|ch| {
                                let ch_type = ch.get("channel_type").and_then(|v| v.as_str())?;
                                if ch_type != pt {
                                    return None;
                                }
                                let id = ch.get("id").and_then(|v| v.as_str())?.to_string();
                                let status = ch
                                    .get("status")
                                    .and_then(|v| v.as_str())
                                    .map(ChannelStatus::from_str)
                                    .unwrap_or(ChannelStatus::Disconnected);
                                Some(InstanceInfo { id, status })
                            })
                            .collect();

                        // Auto-select first if nothing selected or selected was deleted
                        let current = selected_id.get_untracked();
                        let should_reselect = current.is_none()
                            || !filtered.iter().any(|i| Some(&i.id) == current.as_ref());
                        if should_reselect {
                            selected_id.set(filtered.first().map(|i| i.id.clone()));
                        }

                        instances.set(filtered);
                    }
                }
                Err(_) => {}
            }
        });
    });

    // State for new instance dialog
    let show_new_dialog = RwSignal::new(false);
    let new_id_input = RwSignal::new(String::new());
    let new_error = RwSignal::new(Option::<String>::None);
    let creating = RwSignal::new(false);

    let platform_type_for_create = platform_type.clone();
    let on_create = move || {
        let id = new_id_input.get().trim().to_string();
        if id.is_empty() {
            new_error.set(Some("Instance ID cannot be empty".to_string()));
            return;
        }
        creating.set(true);
        new_error.set(None);

        let pt = platform_type_for_create.clone();
        spawn_local(async move {
            match state
                .rpc_call(
                    "channel.create",
                    json!({ "id": id, "type": pt, "config": {} }),
                )
                .await
            {
                Ok(_) => {
                    selected_id.set(Some(id));
                    show_new_dialog.set(false);
                    new_id_input.set(String::new());
                    refresh_trigger.update(|n| *n += 1);
                }
                Err(e) => {
                    new_error.set(Some(format!("Failed to create: {}", e)));
                }
            }
            creating.set(false);
        });
    };

    // Callback for when an instance is deleted from the config template
    let on_instance_deleted = Callback::new(move |_: ()| {
        selected_id.set(None);
        refresh_trigger.update(|n| *n += 1);
    });

    // Pre-compute static view data
    let icon_svg = definition.icon_svg;
    let brand_color = definition.brand_color;
    let name = definition.name;
    let description = definition.description;
    let icon_bg = format!("background-color: {}15", brand_color);
    let is_discord = definition.id == "discord";
    let def = definition;

    view! {
        <div class="flex-1 flex flex-col overflow-hidden bg-surface">
            // ---- Top bar: back link + platform header ----
            <div class="p-6 pb-4 border-b border-border">
                <A
                    href="/settings/channels"
                    attr:class="inline-flex items-center gap-1 text-sm text-text-tertiary hover:text-text-primary transition-colors mb-4"
                >
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <polyline points="15 18 9 12 15 6"/>
                    </svg>
                    "Back to Channels"
                </A>
                <div class="flex items-center gap-3">
                    <div
                        class="w-10 h-10 rounded-lg flex items-center justify-center"
                        style=icon_bg
                    >
                        <svg
                            width="22" height="22" viewBox="0 0 24 24"
                            fill="none" stroke=brand_color stroke-width="2"
                            stroke-linecap="round" stroke-linejoin="round"
                            inner_html=icon_svg
                        />
                    </div>
                    <div>
                        <h1 class="text-2xl font-semibold text-text-primary">{name}</h1>
                        <p class="text-sm text-text-secondary">{description}</p>
                    </div>
                </div>
            </div>

            // ---- Master-detail body ----
            <div class="flex flex-1 overflow-hidden">
                // ---- Left sidebar: instance list ----
                <div class="w-56 border-r border-border overflow-y-auto p-3 space-y-1">
                    <For
                        each=move || instances.get()
                        key=|inst| inst.id.clone()
                        children=move |inst| {
                            let id = inst.id.clone();
                            let id_for_click = inst.id.clone();
                            let status = inst.status;
                            view! {
                                <button
                                    on:click=move |_| selected_id.set(Some(id_for_click.clone()))
                                    class=move || {
                                        let base = "w-full text-left px-3 py-2 rounded-lg text-sm transition-colors flex items-center justify-between";
                                        if selected_id.get().as_deref() == Some(&id) {
                                            format!("{} bg-primary/10 text-primary font-medium", base)
                                        } else {
                                            format!("{} text-text-secondary hover:bg-surface-raised hover:text-text-primary", base)
                                        }
                                    }
                                >
                                    <span class="truncate">{inst.id.clone()}</span>
                                    <ChannelStatusBadge status=Signal::derive(move || status) />
                                </button>
                            }
                        }
                    />

                    // ---- New instance button / dialog ----
                    {move || {
                        if show_new_dialog.get() {
                            view! {
                                <div class="mt-2 p-3 bg-surface-raised border border-border rounded-lg space-y-2">
                                    <input
                                        type="text"
                                        placeholder="Instance ID (e.g. telegram-work)"
                                        prop:value=move || new_id_input.get()
                                        on:input=move |ev| {
                                            new_id_input.set(event_target_value(&ev));
                                        }
                                        on:keydown=move |ev| {
                                            if ev.key() == "Enter" {
                                                on_create();
                                            }
                                        }
                                        class="w-full px-2 py-1.5 text-sm bg-surface border border-border rounded-md focus:outline-none focus:ring-1 focus:ring-primary/30 text-text-primary"
                                    />
                                    {move || new_error.get().map(|e| view! {
                                        <p class="text-xs text-danger">{e}</p>
                                    })}
                                    <div class="flex gap-2">
                                        <button
                                            on:click=move |_| on_create()
                                            disabled=move || creating.get()
                                            class="flex-1 px-2 py-1 text-xs bg-primary text-text-inverse rounded-md hover:bg-primary-hover disabled:opacity-50"
                                        >
                                            {move || if creating.get() { "Creating..." } else { "Create" }}
                                        </button>
                                        <button
                                            on:click=move |_| {
                                                show_new_dialog.set(false);
                                                new_id_input.set(String::new());
                                                new_error.set(None);
                                            }
                                            class="px-2 py-1 text-xs border border-border text-text-secondary rounded-md hover:bg-surface-raised"
                                        >
                                            "Cancel"
                                        </button>
                                    </div>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <button
                                    on:click=move |_| show_new_dialog.set(true)
                                    class="w-full mt-2 px-3 py-2 text-sm text-primary border border-dashed border-primary/30 rounded-lg hover:bg-primary-subtle transition-colors flex items-center justify-center gap-1"
                                >
                                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                        <line x1="12" y1="5" x2="12" y2="19"/>
                                        <line x1="5" y1="12" x2="19" y2="12"/>
                                    </svg>
                                    "New Instance"
                                </button>
                            }.into_any()
                        }
                    }}
                </div>

                // ---- Right panel: selected instance config ----
                <div class="flex-1 overflow-y-auto p-6">
                    <div class="max-w-3xl">
                        {move || {
                            match selected_id.get() {
                                None => view! {
                                    <div class="flex flex-col items-center justify-center py-20 text-text-tertiary">
                                        <p class="text-sm">"No instances configured."</p>
                                        <button
                                            on:click=move |_| show_new_dialog.set(true)
                                            class="mt-3 px-4 py-2 text-sm bg-primary text-text-inverse rounded-lg hover:bg-primary-hover"
                                        >
                                            "Create your first bot"
                                        </button>
                                    </div>
                                }.into_any(),
                                Some(id) => {
                                    if is_discord {
                                        // Discord uses custom view
                                        view! { <DiscordChannelView /> }.into_any()
                                    } else {
                                        view! {
                                            <ChannelConfigTemplate
                                                definition=def
                                                instance_id=id.clone()
                                                on_deleted=on_instance_deleted
                                            />
                                        }.into_any()
                                    }
                                }
                            }
                        }}
                    </div>
                </div>
            </div>
        </div>
    }
    .into_any()
}
```

**Step 2: Register in mod.rs**

Add to `apps/panel/src/views/settings/channels/mod.rs`:

```rust
pub mod platform_page;
pub use platform_page::ChannelPlatformPage;
```

**Step 3: Verify compilation**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: May have errors if ChannelConfigTemplate signature not yet matching. That's OK — Task 2 and Task 3 are developed together.

**Step 4: Commit**

```bash
git add apps/panel/src/views/settings/channels/platform_page.rs apps/panel/src/views/settings/channels/mod.rs
git commit -m "panel: add ChannelPlatformPage with master-detail layout"
```

---

### Task 4: Update routing in app.rs

**Files:**
- Modify: `apps/panel/src/app.rs`

**Step 1: Delete the 13 wrapper components**

Remove lines 84-146 (all the `TelegramConfigPage`, `WhatsAppConfigPage`, etc. wrapper components).

**Step 2: Add import for ChannelPlatformPage**

At the top, add:
```rust
use crate::views::settings::channels::ChannelPlatformPage;
```

**Step 3: Replace 13 channel routes with dynamic matching**

In `SettingsRouter`, replace lines 224-237:

```rust
            // Channels
            "/settings/channels" => view! { <ChannelsOverview /> }.into_any(),
            "/settings/channels/discord" => view! { <DiscordChannelView /> }.into_any(),
            "/settings/channels/telegram" => view! { <TelegramConfigPage /> }.into_any(),
            // ... (13 lines)
```

with:

```rust
            // Channels
            "/settings/channels" => view! { <ChannelsOverview /> }.into_any(),
            _ if path.starts_with("/settings/channels/") => {
                let platform_type = path.strip_prefix("/settings/channels/")
                    .unwrap_or("")
                    .to_string();
                view! { <ChannelPlatformPage platform_type=platform_type /> }.into_any()
            },
```

**Important:** This `_ if` arm must come BEFORE the final `_ => ().into_any()` catch-all, and AFTER the exact `/settings/channels` match.

**Step 4: Remove unused imports**

Remove imports that are no longer needed:
- `DiscordChannelView` (now used inside `ChannelPlatformPage` directly)
- `ChannelConfigTemplate` (now used inside `ChannelPlatformPage` directly)
- `definitions` (now used inside `ChannelPlatformPage` directly)

Check if `use crate::views::settings::*` still covers everything needed. If `ChannelPlatformPage` is re-exported through that wildcard, no additional import needed.

**Step 5: Verify compilation**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: Pass.

**Step 6: Commit**

```bash
git add apps/panel/src/app.rs
git commit -m "panel: replace 13 channel routes with single dynamic ChannelPlatformPage route"
```

---

### Task 5: Add instance count badge to ChannelsOverview

**Files:**
- Modify: `apps/panel/src/views/settings/channels/overview.rs`
- Modify: `apps/panel/src/components/ui/channel_card.rs`

**Step 1: Modify ChannelCard to accept optional count**

In `apps/panel/src/components/ui/channel_card.rs`, add a `count` prop:

```rust
#[component]
pub fn ChannelCard(
    id: &'static str,
    name: &'static str,
    description: &'static str,
    icon_svg: &'static str,
    brand_color: &'static str,
    status: Signal<ChannelStatus>,
    /// Number of bot instances for this platform (shown as badge)
    #[prop(optional)]
    count: Option<Signal<usize>>,
) -> impl IntoView {
```

Add the badge next to the channel name (line 53):

```rust
            // Channel name + count badge
            <div class="flex items-center gap-2 mb-1">
                <h3 class="text-sm font-semibold text-text-primary group-hover:text-primary transition-colors">
                    {name}
                </h3>
                {move || {
                    count.and_then(|c| {
                        let n = c.get();
                        if n > 0 {
                            Some(view! {
                                <span class="px-1.5 py-0.5 text-xs font-medium bg-surface-sunken text-text-tertiary rounded-full">
                                    {n}
                                </span>
                            })
                        } else {
                            None
                        }
                    })
                }}
            </div>
```

Remove the old standalone `<h3>` and `mb-1`.

**Step 2: Modify ChannelsOverview to compute counts and pass to cards**

In `apps/panel/src/views/settings/channels/overview.rs`, change the statuses map to also track instance counts per platform type.

Replace the `statuses` signal (line 24) and the fetch logic (lines 27-47):

```rust
    let statuses = RwSignal::new(HashMap::<String, String>::new());
    let instance_counts = RwSignal::new(HashMap::<String, usize>::new());

    spawn_local(async move {
        match state.rpc_call("channels.list", json!({})).await {
            Ok(val) => {
                if let Some(channels) = val.get("channels").and_then(|c| c.as_array()) {
                    let mut status_map = HashMap::new();
                    let mut count_map = HashMap::<String, usize>::new();
                    for ch in channels {
                        if let Some(ch_type) = ch.get("channel_type").and_then(|v| v.as_str()) {
                            // Count instances per type
                            *count_map.entry(ch_type.to_string()).or_insert(0) += 1;
                            // Use first connected status, or last status for display
                            if let Some(status) = ch.get("status").and_then(|v| v.as_str()) {
                                let entry = status_map.entry(ch_type.to_string()).or_insert_with(|| status.to_string());
                                // Prefer "connected" over other statuses for the overview badge
                                if status == "connected" {
                                    *entry = status.to_string();
                                }
                            }
                        }
                    }
                    statuses.set(status_map);
                    instance_counts.set(count_map);
                }
            }
            Err(_) => {}
        }
    });
```

Then in the card rendering (lines 62-80), pass the count:

```rust
                    {ALL_CHANNELS.iter().map(|def| {
                        let channel_id = def.id.to_string();
                        let channel_id_for_count = def.id.to_string();
                        let status_signal = Signal::derive(move || {
                            statuses.get()
                                .get(&channel_id)
                                .map(|s| ChannelStatus::from_str(s))
                                .unwrap_or(ChannelStatus::Disconnected)
                        });
                        let count_signal = Signal::derive(move || {
                            instance_counts.get()
                                .get(&channel_id_for_count)
                                .copied()
                                .unwrap_or(0)
                        });
                        view! {
                            <ChannelCard
                                id=def.id
                                name=def.name
                                description=def.description
                                icon_svg=def.icon_svg
                                brand_color=def.brand_color
                                status=status_signal
                                count=Some(count_signal)
                            />
                        }
                    }).collect_view()}
```

**Step 3: Verify compilation**

Run: `cargo check -p aleph-panel --target wasm32-unknown-unknown`
Expected: Pass.

**Step 4: Commit**

```bash
git add apps/panel/src/views/settings/channels/overview.rs apps/panel/src/components/ui/channel_card.rs
git commit -m "panel: add instance count badge to channel overview cards"
```

---

### Task 6: Full build and verification

**Step 1: Full WASM build**

Run: `just wasm`
Expected: Build completes, `apps/panel/dist/` updated.

**Step 2: Manual smoke test**

Run: `cargo run --bin aleph`

Open Panel in browser, navigate to Settings > Channels:
1. Overview shows 13 platform cards with instance count badges
2. Click Telegram → master-detail page with sidebar + config panel
3. Sidebar shows existing `telegram` instance
4. Click "+ New Instance" → dialog appears, enter id, create
5. New instance appears in sidebar
6. Click between instances — right panel updates
7. Delete an instance — sidebar refreshes

**Step 3: Final commit (if any fixes needed)**

```bash
git add -A apps/panel/
git commit -m "panel: fix remaining issues from multi-bot UI integration"
```

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Delete unused legacy channel views | telegram.rs, whatsapp.rs, imessage.rs, mod.rs |
| 2 | Refactor ChannelConfigTemplate for instance_id | config_template.rs |
| 3 | Create ChannelPlatformPage with master-detail | platform_page.rs, mod.rs |
| 4 | Update routing in app.rs | app.rs |
| 5 | Add instance count badge to overview | overview.rs, channel_card.rs |
| 6 | Full build and verification | — |

Note: Tasks 1-4 should be developed together in one pass since they have compilation dependencies. Task 5 is independent. Task 6 is verification.
