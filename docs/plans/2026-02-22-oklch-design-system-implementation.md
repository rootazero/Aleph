# OKLCH Design System Migration — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Migrate Aleph Dashboard from Tailwind 3.4 Slate+Indigo to Tailwind 4.2 OKLCH (Mauve/Mist/Olive/Taupe) with CSS variable token system, supporting both light and dark modes.

**Architecture:** CSS variable abstraction layer via Tailwind 4.2 `@theme`. All components reference semantic tokens (`bg-surface`, `text-text-primary`) instead of hardcoded colors. Dark mode via `.dark` class on `<html>`, persisted to `localStorage`.

**Tech Stack:** Tailwind CSS 4.2, @tailwindcss/cli, OKLCH color space, Leptos 0.8 (WASM), tailwind_fuse

**Design Doc:** `docs/plans/2026-02-22-oklch-design-system-migration.md`

---

### Task 1: Upgrade Tailwind to v4.2

**Files:**
- Modify: `core/ui/control_plane/package.json`
- Delete: `core/ui/control_plane/tailwind.config.js`

**Step 1: Update package.json**

Replace the full content of `package.json` with:

```json
{
  "name": "control_plane",
  "version": "1.0.0",
  "description": "Aleph Control Plane UI",
  "scripts": {
    "build:css": "npx @tailwindcss/cli -i styles/tailwind.css -o dist/tailwind.css --minify"
  },
  "keywords": [],
  "author": "",
  "license": "ISC",
  "devDependencies": {
    "tailwindcss": "^4.2",
    "@tailwindcss/cli": "^4.2"
  }
}
```

**Step 2: Delete tailwind.config.js**

```bash
rm core/ui/control_plane/tailwind.config.js
```

**Step 3: Install new dependencies**

```bash
cd core/ui/control_plane && npm install
```

Expected: `tailwindcss@4.x` and `@tailwindcss/cli@4.x` installed.

**Step 4: Commit**

```bash
git add -A core/ui/control_plane/package.json core/ui/control_plane/package-lock.json
git rm core/ui/control_plane/tailwind.config.js
git commit -m "dashboard: upgrade Tailwind CSS from v3 to v4.2"
```

---

### Task 2: Define OKLCH Token System

**Files:**
- Rewrite: `core/ui/control_plane/styles/tailwind.css`

**Step 1: Write the complete CSS with tokens**

Replace the full content of `styles/tailwind.css`:

```css
@import "tailwindcss";

/* Scan Leptos Rust source files for class names */
@source "../src/**/*.rs";
@source "../dist/**/*.html";

/* ============================================================
   OKLCH Design Token System — "Quiet Luxury"
   Mist (breath) + Mauve (depth) + Olive (balance) + Taupe (warmth)
   ============================================================ */

@theme {
  /* === Surface Hierarchy === */
  --color-surface:          oklch(0.97 0.005 220);
  --color-surface-raised:   oklch(1.00 0.000 0);
  --color-surface-sunken:   oklch(0.94 0.008 220);
  --color-surface-overlay:  oklch(0.98 0.004 220);

  /* === Text Hierarchy === */
  --color-text-primary:     oklch(0.20 0.015 310);
  --color-text-secondary:   oklch(0.45 0.010 220);
  --color-text-tertiary:    oklch(0.60 0.008 220);
  --color-text-inverse:     oklch(0.97 0.005 220);

  /* === Borders === */
  --color-border:           oklch(0.88 0.008 220);
  --color-border-subtle:    oklch(0.92 0.006 220);
  --color-border-strong:    oklch(0.78 0.010 220);

  /* === Primary — Mauve === */
  --color-primary:          oklch(0.55 0.120 310);
  --color-primary-hover:    oklch(0.50 0.110 310);
  --color-primary-subtle:   oklch(0.95 0.020 310);

  /* === Success — Olive === */
  --color-success:          oklch(0.55 0.120 130);
  --color-success-subtle:   oklch(0.95 0.025 130);

  /* === Warning — Taupe === */
  --color-warning:          oklch(0.60 0.080 70);
  --color-warning-subtle:   oklch(0.95 0.015 70);

  /* === Danger — Muted Red === */
  --color-danger:           oklch(0.55 0.150 25);
  --color-danger-subtle:    oklch(0.95 0.020 25);

  /* === Info — Mist Deep === */
  --color-info:             oklch(0.50 0.030 220);
  --color-info-subtle:      oklch(0.95 0.010 220);

  /* === Chart Palette === */
  --color-chart-1:          oklch(0.55 0.120 310);
  --color-chart-2:          oklch(0.58 0.120 130);
  --color-chart-3:          oklch(0.60 0.080 70);
  --color-chart-4:          oklch(0.50 0.030 220);
}

/* ============================================================
   Dark Mode Overrides
   ============================================================ */

.dark {
  --color-surface:          oklch(0.15 0.020 310);
  --color-surface-raised:   oklch(0.20 0.018 310);
  --color-surface-sunken:   oklch(0.12 0.015 310);
  --color-surface-overlay:  oklch(0.18 0.020 310);

  --color-text-primary:     oklch(0.97 0.005 220);
  --color-text-secondary:   oklch(0.65 0.008 220);
  --color-text-tertiary:    oklch(0.50 0.006 220);

  --color-border:           oklch(0.28 0.020 310);
  --color-border-subtle:    oklch(0.22 0.018 310);
  --color-border-strong:    oklch(0.35 0.022 310);

  --color-primary:          oklch(0.65 0.120 310);
  --color-primary-hover:    oklch(0.70 0.110 310);
  --color-primary-subtle:   oklch(0.20 0.040 310);

  --color-success:          oklch(0.65 0.120 130);
  --color-success-subtle:   oklch(0.20 0.030 130);

  --color-warning:          oklch(0.70 0.080 70);
  --color-warning-subtle:   oklch(0.20 0.020 70);

  --color-danger:           oklch(0.65 0.150 25);
  --color-danger-subtle:    oklch(0.20 0.030 25);

  --color-info:             oklch(0.65 0.030 220);
  --color-info-subtle:      oklch(0.20 0.015 220);
}

/* System preference fallback */
@media (prefers-color-scheme: dark) {
  :root:not(.light) {
    --color-surface:          oklch(0.15 0.020 310);
    --color-surface-raised:   oklch(0.20 0.018 310);
    --color-surface-sunken:   oklch(0.12 0.015 310);
    --color-surface-overlay:  oklch(0.18 0.020 310);

    --color-text-primary:     oklch(0.97 0.005 220);
    --color-text-secondary:   oklch(0.65 0.008 220);
    --color-text-tertiary:    oklch(0.50 0.006 220);

    --color-border:           oklch(0.28 0.020 310);
    --color-border-subtle:    oklch(0.22 0.018 310);
    --color-border-strong:    oklch(0.35 0.022 310);

    --color-primary:          oklch(0.65 0.120 310);
    --color-primary-hover:    oklch(0.70 0.110 310);
    --color-primary-subtle:   oklch(0.20 0.040 310);

    --color-success:          oklch(0.65 0.120 130);
    --color-success-subtle:   oklch(0.20 0.030 130);

    --color-warning:          oklch(0.70 0.080 70);
    --color-warning-subtle:   oklch(0.20 0.020 70);

    --color-danger:           oklch(0.65 0.150 25);
    --color-danger-subtle:    oklch(0.20 0.030 25);

    --color-info:             oklch(0.65 0.030 220);
    --color-info-subtle:      oklch(0.20 0.015 220);
  }
}

/* ============================================================
   Global Transitions (smooth theme switching)
   ============================================================ */

:root {
  transition: background-color 0.2s ease, color 0.2s ease, border-color 0.2s ease;
}
```

**Step 2: Build CSS to verify tokens compile**

```bash
cd core/ui/control_plane && npm run build:css
```

Expected: `dist/tailwind.css` regenerated without errors.

**Step 3: Commit**

```bash
git add core/ui/control_plane/styles/tailwind.css
git commit -m "dashboard: define OKLCH token system with light/dark modes"
```

---

### Task 3: Theme Initialization + HTML Entry Point

**Files:**
- Modify: `core/ui/control_plane/index.html`
- Modify: `core/ui/control_plane/src/lib.rs`

**Step 1: Update index.html**

Change the body class from hardcoded dark colors to semantic tokens:

```html
<!-- Before -->
<body class="bg-gray-900 text-gray-100">

<!-- After -->
<body class="bg-surface text-text-primary">
```

Also update the CSS link for Tailwind 4 (remove Trunk-specific link if using wasm-bindgen build):

The `dist/index.html` served by the server already references the compiled CSS directly. The `index.html` in root is the Trunk dev template. Update it to use the token classes.

**Step 2: Add theme initialization to lib.rs**

Add theme detection logic that reads `localStorage` and sets the `dark`/`light` class on `<html>`:

```rust
pub mod app;
pub mod api;
pub mod components;
pub mod context;
pub mod generation;
pub mod models;
pub mod preset_providers;
pub mod views;

use wasm_bindgen::prelude::*;

/// Initialize the Leptos application
/// This function is automatically called when the WASM module is loaded
#[wasm_bindgen(start)]
pub fn main() {
    use leptos::prelude::*;

    // Set up panic hook for better error messages
    console_error_panic_hook::set_once();

    // Initialize theme from localStorage or system preference
    init_theme();

    // Mount the app to the body
    mount_to_body(app::App);
}

/// Read theme preference from localStorage and apply dark/light class to <html>
fn init_theme() {
    let window = web_sys::window().expect("no window");
    let document = window.document().expect("no document");
    let html = document.document_element().expect("no html element");

    // Check localStorage for saved preference
    let storage = window.local_storage().ok().flatten();
    let saved_theme = storage
        .as_ref()
        .and_then(|s| s.get_item("aleph-theme").ok())
        .flatten();

    match saved_theme.as_deref() {
        Some("dark") => {
            let _ = html.class_list().add_1("dark");
        }
        Some("light") => {
            let _ = html.class_list().add_1("light");
        }
        _ => {
            // Follow system preference (CSS @media handles this)
            // No class needed — the @media rule in CSS covers it
        }
    }
}
```

**Step 3: Commit**

```bash
git add core/ui/control_plane/index.html core/ui/control_plane/src/lib.rs
git commit -m "dashboard: add theme initialization and semantic body classes"
```

---

### Task 4: Migrate Core UI Components

**Files:**
- Modify: `core/ui/control_plane/src/components/ui/button.rs`
- Modify: `core/ui/control_plane/src/components/ui/card.rs`
- Modify: `core/ui/control_plane/src/components/ui/badge.rs`
- Modify: `core/ui/control_plane/src/components/ui/tooltip.rs`

**Step 1: Migrate button.rs**

Replace `ButtonVariant` enum classes:

```rust
#[derive(TwVariant, PartialEq)]
pub enum ButtonVariant {
    #[tw(default, class = "bg-primary text-text-inverse hover:bg-primary-hover")]
    Primary,
    #[tw(class = "bg-surface-sunken text-text-primary border border-border hover:bg-surface-raised")]
    Secondary,
    #[tw(class = "bg-transparent text-text-secondary hover:text-text-primary hover:bg-surface-sunken")]
    Ghost,
    #[tw(class = "bg-danger text-text-inverse hover:brightness-95")]
    Destructive,
}
```

No changes to `ButtonSize` or the component structure.

**Step 2: Migrate card.rs**

Apply these replacements across the file:

| Old | New |
|-----|-----|
| `bg-slate-900/40 border border-slate-800 rounded-3xl backdrop-blur-sm shadow-glass` | `bg-surface-raised border border-border rounded-2xl` |
| `border-b border-slate-800/50` | `border-b border-border-subtle` |
| `text-slate-100` | `text-text-primary` |
| `text-slate-400` | `text-text-secondary` |

Full replacement for Card component:
```rust
#[component]
pub fn Card(
    #[prop(into, optional)] class: String,
    children: Children,
) -> impl IntoView {
    view! {
        <div class=format!("bg-surface-raised border border-border rounded-2xl {}", class)>
            {children()}
        </div>
    }
}
```

CardHeader:
```rust
<div class=format!("p-6 border-b border-border-subtle {}", class)>
```

CardTitle:
```rust
<h3 class=format!("text-xl font-semibold tracking-tight text-text-primary {}", class)>
```

CardDescription:
```rust
<p class=format!("text-sm text-text-secondary mt-1 {}", class)>
```

**Step 3: Migrate badge.rs**

Replace `BadgeVariant` enum:

```rust
#[derive(TwVariant, PartialEq)]
pub enum BadgeVariant {
    #[tw(default, class = "bg-primary-subtle text-primary border-primary/20")]
    Indigo,
    #[tw(class = "bg-success-subtle text-success border-success/20")]
    Emerald,
    #[tw(class = "bg-warning-subtle text-warning border-warning/20")]
    Amber,
    #[tw(class = "bg-danger-subtle text-danger border-danger/20")]
    Red,
    #[tw(class = "bg-surface-sunken text-text-secondary border-border")]
    Slate,
}
```

Replace `StatusBadge` colors:

```rust
let (bg_class, animation_class) = match level {
    AlertLevel::None => return view! {}.into_any(),
    AlertLevel::Info => ("bg-info", ""),
    AlertLevel::Warning => ("bg-warning", ""),
    AlertLevel::Critical => ("bg-danger", "animate-pulse"),
};
```

**Step 4: Migrate tooltip.rs**

Replace the outer div class:

```rust
<div class="absolute left-full ml-2 px-3 py-2 bg-surface-raised border border-border rounded-lg opacity-0 group-hover:opacity-100 transition-opacity duration-200 pointer-events-none whitespace-nowrap z-50">
    <div class="text-sm font-medium text-text-primary">{text}</div>
    {move || alert.get().and_then(|a| a.message).map(|msg| view! {
        <div class="text-xs text-text-tertiary mt-1">{msg}</div>
    })}
</div>
```

Remove: `shadow-xl` (was on tooltip).

**Step 5: Commit**

```bash
git add core/ui/control_plane/src/components/ui/
git commit -m "dashboard: migrate core UI components to OKLCH tokens"
```

---

### Task 5: Migrate Sidebar Components

**Files:**
- Modify: `core/ui/control_plane/src/components/sidebar/sidebar.rs`
- Modify: `core/ui/control_plane/src/components/sidebar/sidebar_item.rs`
- Modify: `core/ui/control_plane/src/components/settings_sidebar.rs`

**Step 1: Migrate sidebar.rs**

Sidebar container (line 45):
```rust
let base = "border-r border-border bg-surface-raised flex flex-col transition-all duration-300";
```

Bottom actions section (line 83):
```rust
<div class="p-4 border-t border-border">
    <A href="/settings" attr:class="flex items-center gap-3 px-3 py-2 rounded-lg text-text-secondary hover:text-text-primary hover:bg-surface-sunken transition-all duration-200">
```

LogoSection — replace the gradient logo (line 105):
```rust
<div class="w-8 h-8 bg-primary rounded-lg flex items-center justify-center">
    <span class="text-text-inverse font-bold text-xl">"A"</span>
</div>
```

Remove: `shadow-lg shadow-indigo-500/20`, `bg-gradient-to-br from-indigo-500 to-purple-600`.

**Step 2: Migrate sidebar_item.rs**

The `<A>` tag class (line 33):
```rust
<A href=href attr:class="relative group flex items-center gap-3 px-3 py-2 rounded-lg text-text-secondary hover:text-text-primary hover:bg-surface-sunken transition-all duration-200">
```

**Step 3: Migrate settings_sidebar.rs**

Nav container (line 146):
```rust
<nav class="w-64 bg-surface-raised border-r border-border p-4 space-y-6 overflow-y-auto">
```

Settings title gradient → solid (line 148):
```rust
<h2 class="text-xl font-bold text-text-primary">
    "Settings"
</h2>
<p class="text-xs text-text-tertiary mt-1">
```

Group label (line 159):
```rust
<h3 class="px-3 py-1 text-xs font-medium text-text-tertiary uppercase tracking-wider">
```

SettingsSidebarItem `<A>` class (line 186):
```rust
<A
    href=path
    attr:class="flex items-center gap-3 px-3 py-2 rounded-lg text-sm transition-colors hover:bg-surface-sunken group text-text-secondary hover:text-text-primary"
    exact=true
>
```

Icon svg class (line 198):
```rust
class="text-text-tertiary group-hover:text-text-secondary"
```

**Step 4: Commit**

```bash
git add core/ui/control_plane/src/components/sidebar/ core/ui/control_plane/src/components/settings_sidebar.rs
git commit -m "dashboard: migrate sidebar components to OKLCH tokens"
```

---

### Task 6: Migrate Forms + Connection Status

**Files:**
- Modify: `core/ui/control_plane/src/components/forms.rs`
- Modify: `core/ui/control_plane/src/components/connection_status.rs`

**Step 1: Migrate forms.rs**

Apply these replacements throughout the file:

| Old | New |
|-----|-----|
| `bg-slate-900/50 backdrop-blur-sm border border-slate-800` | `bg-surface-raised border border-border` |
| `text-slate-200` (headings) | `text-text-primary` |
| `text-slate-300` (labels) | `text-text-secondary` |
| `text-slate-400` (descriptions) | `text-text-secondary` |
| `text-slate-500` (help text) | `text-text-tertiary` |
| `bg-slate-800 border border-slate-700` (inputs) | `bg-surface-raised border border-border` |
| `text-slate-200` (input text) | `text-text-primary` |
| `focus:ring-2 focus:ring-indigo-500` | `focus:ring-2 focus:ring-primary/30 focus:border-primary` |
| `bg-slate-700` (slider track) | `bg-border` |
| `accent-indigo-500` | `accent-primary` |
| `bg-slate-700 peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-indigo-500` (switch) | `bg-border peer-focus:outline-none peer-focus:ring-2 peer-focus:ring-primary/30` |
| `peer-checked:bg-indigo-600` | `peer-checked:bg-primary` |
| `bg-red-900/20 border border-red-500/50 ... text-red-400` | `bg-danger-subtle border border-danger/30 ... text-danger` |
| `bg-green-900/20 border border-green-500/50 ... text-green-400` | `bg-success-subtle border border-success/30 ... text-success` |
| `bg-indigo-600 text-white ... hover:bg-indigo-700` (SaveButton) | `bg-primary text-text-inverse ... hover:bg-primary-hover` |

**Step 2: Migrate connection_status.rs**

Replace `status_class`:
```rust
let status_class = move || {
    if is_connected.get() {
        "bg-success"
    } else {
        "bg-warning"
    }
};
```

Replace outer container (line 29):
```rust
<div class="bg-surface-raised border border-border rounded-2xl p-4">
```

Replace reconnect text (line 38):
```rust
<div class="text-xs text-text-tertiary">
```

Remove: all `shadow-[0_0_8px_...]` glow effects, `backdrop-blur-sm`.

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/components/forms.rs core/ui/control_plane/src/components/connection_status.rs
git commit -m "dashboard: migrate forms and connection status to OKLCH tokens"
```

---

### Task 7: Migrate App Root + Settings Layout

**Files:**
- Modify: `core/ui/control_plane/src/app.rs`
- Modify: `core/ui/control_plane/src/components/layouts/settings_layout.rs`

**Step 1: Migrate app.rs**

Replace the root div class (line 54):
```rust
<div class="flex h-screen bg-surface text-text-primary font-sans selection:bg-primary/30">
```

Remove the two background glow divs entirely (lines 62-63). Delete these lines:
```rust
// DELETE: <div class="fixed top-0 right-0 -z-10 w-[500px] h-[500px] bg-indigo-500/10 blur-[120px] rounded-full translate-x-1/2 -translate-y-1/2"></div>
// DELETE: <div class="fixed bottom-0 left-0 -z-10 w-[400px] h-[400px] bg-emerald-500/5 blur-[100px] rounded-full -translate-x-1/2 translate-y-1/2"></div>
```

**Step 2: settings_layout.rs needs no color changes**

This file only defines layout structure (flex, overflow). No color classes to change.

**Step 3: Commit**

```bash
git add core/ui/control_plane/src/app.rs
git commit -m "dashboard: migrate app root to OKLCH tokens, remove background glows"
```

---

### Task 8: Migrate Home View

**Files:**
- Modify: `core/ui/control_plane/src/views/home.rs`

**Step 1: Apply color replacements**

Header description (line 46):
```rust
<p class="text-text-secondary">"Command center for your personal AI instance."</p>
```

Connection warning box (line 53):
```rust
<div class="bg-warning-subtle border border-warning/20 rounded-xl p-6 flex items-start gap-4">
```
Warning icon (line 54): `text-amber-500` → `text-warning`
Warning title (line 60): `text-amber-400` → `text-warning`
Warning text (line 61): `text-amber-300/80` → `text-text-secondary`

StatCard color props (lines 72-91):
```rust
<StatCard label="Active Tasks" value=... color="text-primary">
<StatCard label="CPU Usage" value=... color="text-success">
<StatCard label="Knowledge Base" value=... color="text-primary">
<StatCard label="Gateway Latency" value=... color="text-warning">
```

StatCard component (line 149):
```rust
<div class="bg-surface-raised border border-border p-6 rounded-2xl hover:bg-surface-sunken transition-colors group">
    <div class="flex items-start justify-between mb-4">
        <div class=format!("p-2 rounded-lg bg-surface-sunken {}", color)>
```
Line 157: `text-slate-400` → `text-text-secondary`, `text-slate-300` → `text-text-primary`

Remove from StatCard: `backdrop-blur-sm`, `shadow-sm`.

Recent Activity section (line 98):
```rust
<div class="bg-surface-raised border border-border rounded-2xl overflow-hidden">
    <div class="p-4 border-b border-border bg-surface-sunken">
        <div class="flex items-center justify-between">
            <span class="text-sm font-medium text-text-secondary">"Event Log"</span>
            <button class="text-xs text-primary hover:text-primary-hover">"View All"</button>
```
Line 105: `text-slate-500` → `text-text-tertiary`

Remove from Recent Activity: `backdrop-blur-sm`, `shadow-glass`, `bg-slate-800/20`.

QuickAction component (line 169):
```rust
<button class="flex items-center justify-between p-4 rounded-xl bg-surface-raised border border-border hover:bg-surface-sunken hover:border-primary/30 transition-all group text-left w-full">
    <div class="flex items-center gap-3">
        <svg ... attr:class="w-5 h-5 text-text-tertiary group-hover:text-primary transition-colors" ...>
        <span class="text-sm font-medium text-text-secondary group-hover:text-text-primary transition-colors">{label}</span>
    </div>
    <div class="text-text-tertiary group-hover:translate-x-1 transition-transform">"→"</div>
```

Remove: `hover:shadow-neon-indigo`.

**Step 2: Commit**

```bash
git add core/ui/control_plane/src/views/home.rs
git commit -m "dashboard: migrate Home view to OKLCH tokens"
```

---

### Task 9: Migrate System Status View

**Files:**
- Modify: `core/ui/control_plane/src/views/system_status.rs`

**Step 1: Apply color replacements**

Header (line 100-101): `text-slate-100` → `text-text-primary`, `text-emerald-500` → `text-success`
Subheader (line 115): `text-slate-400` → `text-text-secondary`

Connection error box (line 173):
```rust
<div class="bg-danger-subtle border border-danger/20 rounded-xl p-4 text-sm text-danger">
```

Section headings (lines 185, 216): `text-slate-300` → `text-text-secondary`

ServiceCard (line 255):
```rust
<div class="bg-surface-raised border border-border p-5 rounded-2xl flex items-center justify-between group hover:border-border-strong transition-all">
```

ServiceCard status dots (lines 257-260):
```rust
let dot_class = if status.get() == "Healthy" { "bg-success" }
    else if status.get() == "Degraded" { "bg-warning" }
    else { "bg-danger" };
```
Remove: `shadow-[0_0_12px]`, `shadow-emerald-500/60`, `shadow-amber-500/60`, `shadow-red-500/60`.

ServiceCard text colors:
- `text-slate-200` → `text-text-primary`
- `text-slate-500` → `text-text-tertiary`
- `text-slate-300` → `text-text-secondary`

Remove: `hover:shadow-indigo-500/5`, `hover:bg-slate-800/20`, `shadow-sm`.

Divider line (line 272): `bg-slate-800` → `bg-border`

ResourceMetric (line 294):
```rust
<div class=format!("p-2.5 rounded-xl bg-surface-sunken text-text-inverse transition-transform group-hover:scale-110 {}", color)>
```

ResourceMetric progress bar colors — replace inline `color` prop values at call sites (lines 218-232):
```rust
<ResourceMetric label="CPU Clusters" ... color="bg-success" progress=24>
<ResourceMetric label="Neural Memory" ... color="bg-primary" progress=26>
<ResourceMetric label="Encrypted Storage" ... color="bg-primary" progress=15>
<ResourceMetric label="Security Layer" ... color="bg-info" progress=100>
```

Progress bar track (line 304): `bg-slate-800` → `bg-border`

ResourceMetric text: `text-slate-400` → `text-text-secondary`, `text-slate-200` → `text-text-primary`, `text-slate-500` → `text-text-tertiary`.

**Step 2: Commit**

```bash
git add core/ui/control_plane/src/views/system_status.rs
git commit -m "dashboard: migrate System Status view to OKLCH tokens"
```

---

### Task 10: Migrate Agent Trace View

**Files:**
- Modify: `core/ui/control_plane/src/views/agent_trace.rs`

**Step 1: Apply color replacements**

Header (line 70):
```rust
<header class="p-8 border-b border-border bg-surface-raised sticky top-0 z-10">
```
Remove: `backdrop-blur-md`, `bg-slate-900/20`.

Title icon (line 74): `text-indigo-500` → `text-primary`
Title text (line 73): `text-slate-100` → `text-text-primary`
Subtitle (line 79): `text-slate-400` → `text-text-secondary`

Pause button (line 85):
```rust
class="flex items-center gap-2 px-4 py-2 rounded-lg bg-surface-sunken hover:bg-surface-raised transition-colors border border-border hover:border-border-strong"
```
Remove: `shadow-sm`.

Clear button (line 111):
```rust
class="p-2 rounded-lg text-text-tertiary hover:text-danger hover:bg-danger-subtle transition-all border border-transparent hover:border-danger/20"
```

Connection warning — same pattern as Home (line 128):
```rust
<div class="max-w-4xl mx-auto bg-warning-subtle border border-warning/20 rounded-xl p-6 flex items-start gap-4">
    ... text-warning ... text-warning ... text-text-secondary ...
```

Empty state (lines 154-161):
- `text-slate-500` → `text-text-tertiary`
- `text-slate-400` → `text-text-secondary`

Timeline line (line 165): `border-slate-800` → `border-border`

TraceNodeItem accent colors (lines 202-207):
```rust
let accent_color = match node.node_type {
    TraceNodeType::Thinking => "text-info bg-info-subtle border-info/20",
    TraceNodeType::ToolCall => "text-warning bg-warning-subtle border-warning/20",
    TraceNodeType::ToolResult => "text-success bg-success-subtle border-success/20",
    _ => "text-text-tertiary bg-surface-sunken border-border",
};
```

Timeline dot (line 212): replace `bg-slate-950` → `bg-surface`
Remove: `shadow-glass`.

Trace card (line 219):
```rust
<div class="bg-surface-raised border border-border rounded-2xl p-6 group-hover:border-border-strong transition-all">
```
Remove: `backdrop-blur-sm`, `shadow-xl shadow-black/20`.

Timestamp text: `text-slate-500` → `text-text-tertiary`
Content text (line 230): `text-slate-200` → `text-text-primary`

Child items (lines 237-245):
- `border-slate-800/50` → `border-border-subtle`
- `border-slate-800` → `border-border`
- `bg-slate-700` → `bg-border`
- `text-slate-400` → `text-text-secondary`

**Step 2: Commit**

```bash
git add core/ui/control_plane/src/views/agent_trace.rs
git commit -m "dashboard: migrate Agent Trace view to OKLCH tokens"
```

---

### Task 11: Migrate Memory View

**Files:**
- Modify: `core/ui/control_plane/src/views/memory.rs`

**Step 1: Apply color replacements**

Header icon (line 70): `text-purple-500` → `text-primary`
Title (line 69): `text-slate-100` → `text-text-primary`
Subtitle (line 77): `text-slate-400` → `text-text-secondary`

Search icon (line 82): `text-slate-500 group-focus-within:text-indigo-400` → `text-text-tertiary group-focus-within:text-primary`

Search input (line 89):
```rust
class="pl-10 pr-4 py-2 bg-surface-raised border border-border rounded-xl focus:outline-none focus:border-primary/50 focus:ring-4 focus:ring-primary/10 w-64 transition-all text-sm text-text-primary placeholder:text-text-tertiary"
```
Remove: `shadow-sm`, `bg-slate-900/40`.

Connection warning — same token pattern as other views.

Memory Stats cards (lines 132-155):
```rust
<Card class="bg-primary-subtle border-primary/10 p-6 flex flex-col items-start".to_string()>
    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">"Total Facts"</span>
...
<Card class="bg-success-subtle border-success/10 p-6 flex flex-col items-start".to_string()>
    <span class="text-[10px] font-bold text-success uppercase tracking-widest mb-1.5">"Vector Size"</span>
...
<Card class="bg-primary-subtle border-primary/10 p-6 flex flex-col items-start".to_string()>
    <span class="text-[10px] font-bold text-primary uppercase tracking-widest mb-1.5">"Active Sources"</span>
```

Table header (line 162):
```rust
<tr class="bg-surface-sunken text-[10px] font-bold text-text-tertiary uppercase tracking-widest">
```

Table body (line 169): `divide-slate-800/50` → `divide-border-subtle`

Empty state text: `text-slate-500` → `text-text-tertiary`

MemoryRow hover (line 228): `hover:bg-slate-800/20` → `hover:bg-surface-sunken`
Row text (line 230): `text-slate-200` → `text-text-primary`
Date text (line 238): `text-slate-500` → `text-text-tertiary`

**Step 2: Commit**

```bash
git add core/ui/control_plane/src/views/memory.rs
git commit -m "dashboard: migrate Memory view to OKLCH tokens"
```

---

### Task 12: Migrate Settings Views (Bulk)

**Files:**
- Modify: `core/ui/control_plane/src/views/settings/mod.rs`
- Modify: All 15 settings view files in `core/ui/control_plane/src/views/settings/`

**Important Context:** Settings views have inconsistent styling — some use light theme (`bg-white`, `text-slate-900`), some use dark (`bg-slate-900/50`, `text-slate-200`), some use `gray-*`. ALL must be unified to use the semantic token system.

**Step 1: Migrate mod.rs (Settings welcome page)**

Replace `bg-slate-950` → `bg-surface`
Replace gradient title:
```rust
<!-- Before -->
<h1 class="text-3xl font-bold mb-2 bg-gradient-to-r from-indigo-400 to-emerald-400 bg-clip-text text-transparent">

<!-- After -->
<h1 class="text-3xl font-bold mb-2 text-text-primary">
```

Replace card containers:
```rust
<div class="p-6 bg-surface-raised border border-border rounded-xl">
    <h3 class="text-lg font-semibold text-text-primary mb-2">
    <p class="text-sm text-text-secondary mb-4">
    <ul class="space-y-2 text-sm text-text-secondary">
```
Remove: `backdrop-blur-sm`, `bg-slate-900/50`.

**Step 2: Apply universal replacement rules across ALL 15 settings files**

For every settings file, apply the following global replacements. The implementer should process each file systematically:

**Dark theme files** (general.rs, agent.rs, providers.rs, routing_rules.rs, plugins.rs, skills.rs, policies.rs):

| Pattern | Replacement |
|---------|-------------|
| `bg-slate-900/50` | `bg-surface-raised` |
| `bg-slate-900/40` | `bg-surface-raised` |
| `bg-slate-900/30` | `bg-surface-raised` |
| `bg-slate-800` (as bg) | `bg-surface-sunken` |
| `bg-slate-800/50` | `bg-surface-sunken` |
| `bg-slate-800/20` | `bg-surface-sunken` |
| `border-slate-800` | `border-border` |
| `border-slate-700` | `border-border` |
| `text-slate-50` | `text-text-primary` |
| `text-slate-100` | `text-text-primary` |
| `text-slate-200` | `text-text-primary` |
| `text-slate-300` | `text-text-secondary` |
| `text-slate-400` | `text-text-secondary` |
| `text-slate-500` | `text-text-tertiary` |
| `text-slate-600` | `text-text-tertiary` |
| `text-indigo-400` | `text-primary` |
| `text-indigo-500` | `text-primary` |
| `bg-indigo-600` | `bg-primary` |
| `bg-indigo-500` | `bg-primary` |
| `hover:bg-indigo-700` | `hover:bg-primary-hover` |
| `hover:bg-indigo-500` | `hover:bg-primary-hover` |
| `text-emerald-400` | `text-success` |
| `bg-emerald-500` | `bg-success` |
| `text-red-400` | `text-danger` |
| `text-red-500` | `text-danger` |
| `bg-red-500` | `bg-danger` |
| `bg-red-900/20` | `bg-danger-subtle` |
| `border-red-500/50` | `border-danger/30` |
| `text-amber-400` | `text-warning` |
| `text-amber-500` | `text-warning` |
| `bg-amber-500/10` | `bg-warning-subtle` |
| `border-amber-500/20` | `border-warning/20` |
| `text-yellow-500` | `text-warning` |
| `text-yellow-400` | `text-warning` |
| `bg-yellow-900/20` | `bg-warning-subtle` |
| `border-yellow-500/20` | `border-warning/20` |
| `text-blue-400` | `text-info` |
| `text-blue-500` | `text-info` |
| `bg-blue-500` | `bg-info` |
| `bg-blue-600` | `bg-primary` |
| `hover:bg-blue-700` | `hover:bg-primary-hover` |
| `text-purple-400` | `text-primary` |
| `bg-purple-500` | `bg-primary` |
| `focus:ring-indigo-500` | `focus:ring-primary/30` |
| `focus:ring-blue-500` | `focus:ring-primary/30` |
| `backdrop-blur-sm` | (remove) |
| `backdrop-blur-xl` | (remove) |
| `backdrop-blur-md` | (remove) |
| `shadow-glass` | (remove) |
| `shadow-lg` | (remove) |
| `shadow-xl` | (remove) |
| `shadow-sm` | (remove) |

**Light theme files** (shortcuts.rs, behavior.rs, generation.rs, search.rs):

These files use `bg-white`, `text-slate-900`, `border-slate-200` etc. Apply:

| Pattern | Replacement |
|---------|-------------|
| `bg-white` | `bg-surface-raised` |
| `bg-slate-50` | `bg-surface-sunken` |
| `text-slate-900` | `text-text-primary` |
| `text-slate-700` | `text-text-secondary` |
| `text-slate-600` | `text-text-secondary` |
| `text-slate-500` | `text-text-tertiary` |
| `text-slate-400` | `text-text-tertiary` |
| `border-slate-200` | `border-border` |
| `border-slate-300` | `border-border` |
| Plus all the accent color replacements from above |

**Gray-based files** (generation_providers.rs, mcp.rs, memory.rs [settings], security.rs):

| Pattern | Replacement |
|---------|-------------|
| `bg-gray-900` | `bg-surface` |
| `bg-gray-800` | `bg-surface-raised` |
| `bg-gray-700` | `bg-surface-sunken` |
| `text-gray-100` | `text-text-primary` |
| `text-gray-200` | `text-text-primary` |
| `text-gray-300` | `text-text-secondary` |
| `text-gray-400` | `text-text-secondary` |
| `text-gray-500` | `text-text-tertiary` |
| `border-gray-700` | `border-border` |
| `border-gray-600` | `border-border` |
| Plus all the accent color replacements from above |

**Step 3: Commit per batch**

```bash
git add core/ui/control_plane/src/views/settings/
git commit -m "dashboard: migrate all settings views to OKLCH tokens"
```

---

### Task 13: Build CSS + Verify + Final Commit

**Step 1: Rebuild CSS**

```bash
cd core/ui/control_plane && npm run build:css
```

Expected: Successful compilation. New CSS should include all the OKLCH custom properties and token utility classes.

**Step 2: Check for any remaining old color references**

```bash
cd core/ui/control_plane && grep -rn "bg-slate\|bg-gray\|bg-indigo\|text-slate\|text-gray\|text-indigo\|border-slate\|border-gray\|shadow-glass\|backdrop-blur" src/ --include="*.rs"
```

Expected: No matches. If any remain, fix them using the replacement tables above.

**Step 3: Build WASM to verify Rust compiles**

```bash
cd core/ui/control_plane && cargo build --lib --target wasm32-unknown-unknown
```

Expected: Successful compilation.

**Step 4: Generate WASM bindings**

```bash
cd core/ui/control_plane && wasm-bindgen --target web --out-dir dist --out-name aleph-dashboard \
  ../../target/wasm32-unknown-unknown/debug/aleph_dashboard.wasm
```

**Step 5: Build the server with control-plane**

```bash
cargo build --bin aleph-server --features control-plane
```

Expected: Successful compilation.

**Step 6: Final commit**

```bash
git add -A core/ui/control_plane/
git commit -m "dashboard: complete OKLCH design system migration"
```

---

## Summary

| Task | Files | Description |
|------|-------|-------------|
| 1 | 2 | Upgrade Tailwind v3→v4.2 |
| 2 | 1 | Define OKLCH token system in CSS |
| 3 | 2 | Theme init + HTML entry point |
| 4 | 4 | Core UI components (button, card, badge, tooltip) |
| 5 | 3 | Sidebar components |
| 6 | 2 | Forms + connection status |
| 7 | 1 | App root (remove glows) |
| 8 | 1 | Home view |
| 9 | 1 | System Status view |
| 10 | 1 | Agent Trace view |
| 11 | 1 | Memory view |
| 12 | 16 | All settings views |
| 13 | — | Build verification |

**Total: ~35 files, 13 tasks**
