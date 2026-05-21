# Panel UI i18n Design

## Summary

Add multi-language support to the Aleph webchat panel UI using `leptos_i18n`, starting with Chinese as the first translation language. English remains the base/fallback. Architecture designed for full localization (plurals, date/number formatting, future RTL) but first phase implements UI text + formatting only.

## Context

- **Current state**: ~2000+ hardcoded English strings across 76 `.rs` files, zero i18n infrastructure
- **Existing**: `GeneralConfig.language` field exists in backend but is unused by frontend
- **Settings UI**: Language dropdown with "Follow System" option already present but non-functional
- **Tech stack**: Leptos 0.8 + WASM (CSR), Tailwind CSS, WebSocket JSON-RPC

## Decision

Use `leptos_i18n` v0.6.x (Leptos 0.8 compatible) with compile-time embedding, ICU4X formatting, and JSON translation files.

### Why `leptos_i18n`

- Native Leptos integration — signals, context provider, compile-time key validation
- ICU4X for plurals/dates/numbers — future languages require zero extra work
- Cookie-based persistence built in
- Actively maintained, version-locked to Leptos releases

### Alternatives Considered

- **`leptos_i18n` without ICU4X features**: Lighter but manual plural handling doesn't scale
- **Custom solution** (`include_str!` + custom `t!()` macro): Zero deps but reinvents the wheel for 2000+ strings

## Architecture

### File Structure

```
interfaces/webchat/
├── locales/
│   ├── en.json          # English (base/fallback)
│   └── zh.json          # Chinese translation
├── build.rs             # leptos_i18n_build code generation
├── src/
│   ├── i18n.rs          # include! generated i18n module + helpers
│   ├── app.rs           # Root component with I18nContextProvider
│   └── ...              # Components: hardcoded strings → t!(i18n, key)
```

### Dependencies

```toml
# Cargo.toml
[dependencies]
leptos_i18n = { version = "0.6", features = ["csr", "cookie", "plurals", "format_datetime", "format_nums"] }

[build-dependencies]
leptos_i18n_build = "0.6"
```

### Build Pipeline

The webchat crate currently has no `build.rs`. Adding one is compatible with the Trunk build toolchain — Trunk invokes `cargo build` which runs `build.rs` normally, and `OUT_DIR` is available in WASM cdylib builds.

1. `build.rs` reads `locales/*.json` → generates type-safe translation module
2. Compile-time validation: missing keys, mismatched interpolation params → build error
3. Generated module included via `include!(concat!(env!("OUT_DIR"), "/i18n/mod.rs"))`
4. Existing `just dev` / `just build` pipelines require no changes — Trunk handles `build.rs` transparently

### WASM Size Impact

ICU4X with `plurals` + `format_datetime` + `format_nums` adds ~200-400KB to the WASM binary (compressed). If this proves too large, the ICU4X features can be disabled and re-enabled later when more languages are added. The base `leptos_i18n` without ICU4X adds ~20-30KB.

## Translation File Structure

JSON with hierarchical keys, organized by UI module. Max 3 levels of nesting.

```json
{
  "nav": {
    "chat": "Chat",
    "dashboard": "Dashboard",
    "agents": "Agents",
    "settings": "Settings"
  },
  "home": {
    "title": "System Overview",
    "status": {
      "online": "Online",
      "offline": "Offline",
      "connecting": "Connecting..."
    },
    "stats": {
      "total_sessions": "Total Sessions",
      "active_agents": "Active Agents",
      "memory_entries": "Memory Entries"
    }
  },
  "settings": {
    "title": "Settings",
    "general": {
      "title": "General Settings",
      "language": {
        "label": "Language",
        "system": "Follow System"
      }
    },
    "behavior": { "title": "Behavior Settings" },
    "security": {
      "title": "Security",
      "require_auth": "Require Authentication",
      "device_pairing": "Enable Device Pairing"
    }
  },
  "common": {
    "save": "Save Changes",
    "cancel": "Cancel",
    "delete": "Delete",
    "retry": "Retry",
    "loading": "Loading...",
    "saving": "Saving...",
    "saved": "Saved successfully",
    "error": "An error occurred",
    "confirm": "Confirm"
  },
  "agents": {},
  "chat": {},
  "memory": {}
}
```

**Grouping rules:**
- Top-level key = UI module (`nav`, `home`, `settings`, `agents`, `chat`, `memory`)
- `common` = cross-module reusable text (buttons, status, prompts)
- `zh.json` mirrors structure exactly; missing keys cause build errors

## Component Transformation Patterns

### Pattern 1: Static text (~80% of changes)

```rust
// Before
view! { <h2>"System Overview"</h2> }

// After
let i18n = use_i18n();
view! { <h2>{t!(i18n, home.title)}</h2> }
```

### Pattern 2: Component props

Components using `&'static str` for labels must change to accept signals:

```rust
// Before
#[component]
fn StatCard(label: &'static str, ...) -> impl IntoView { ... }
<StatCard label="Total Sessions" />

// After
#[component]
fn StatCard(label: Signal<String>, ...) -> impl IntoView { ... }
<StatCard label=t!(i18n, home.stats.total_sessions) />
```

**Migration approach:** Audit all `&'static str` props in `components/` and `views/` that carry user-facing text. Known affected components include `StatCard`, `BottomBarItem`, `SettingsSidebar`, `Tooltip`, `ChannelCard`, `TagListInput`, `SecretInput`, and similar. Each component's signature change updates all call sites in the same commit to avoid partial breakage.

### Pattern 3: Dynamic formatted text

```rust
// Before
format!("Connected {} sessions", count)

// After
t!(i18n, home.connected_sessions, count = move || count.get())
```

With plural support in JSON:
```json
{
  "connected_sessions_one": "Connected {{ count }} session",
  "connected_sessions_other": "Connected {{ count }} sessions"
}
```

### Transformation Priority

1. **Base component signatures** (`components/ui/`) — unblocks all pages
2. **Navigation/Shell** (`bottom_bar`, `top_bar`, `settings_sidebar`) — most visible
3. **Settings pages** (including language switch wiring)
4. **Home dashboard**
5. **Chat, Agents, Memory** and remaining pages

## Language Switching & Persistence

### Settings Page Integration

Existing Language dropdown in `general.rs` is wired to `leptos_i18n`:

```rust
enum LanguageOption {
    System,  // Follow system — navigator.languages auto-detection
    En,      // English
    Zh,      // Chinese
}

fn on_language_change(option: LanguageOption) {
    match option {
        LanguageOption::System => {
            i18n.set_locale(detect_browser_locale());
        }
        LanguageOption::En => i18n.set_locale(Locale::en),
        LanguageOption::Zh => i18n.set_locale(Locale::zh),
    }
    // Also save to backend GeneralConfig.language
}
```

### Dual Persistence

- **Cookie** (frontend): `leptos_i18n` auto-writes, instant on page load, no network wait
- **GeneralConfig** (backend): saved to Gateway config, survives cross-device / cookie clearing

### Initialization Priority

1. Cookie exists → use immediately (fastest, no network)
2. No cookie, WebSocket connected → read GeneralConfig → apply and write cookie
3. Neither → `navigator.languages` browser detection
4. Browser language not supported → fallback to English

## Error Handling & Edge Cases

### Compile-Time Guarantees (built into leptos_i18n)

- Translation key doesn't exist → build error
- `zh.json` missing a key from `en.json` → build error
- Interpolation parameter name mismatch → build error

### Runtime Edge Cases

| Scenario | Handling |
|----------|----------|
| Browser language unsupported (e.g. `ja`) | Fallback to English |
| Cookie contains removed locale | Ignore, follow normal detection chain |
| Backend GeneralConfig conflicts with cookie | Cookie wins (frontend UX priority) |
| New key added without Chinese translation | Build blocked, forces completion |

## Locale Matching Rules

When resolving `navigator.languages` or the "Follow System" option:

- `zh`, `zh-CN`, `zh-Hans`, `zh-SG` → `Locale::zh`
- `zh-TW`, `zh-HK`, `zh-Hant` → `Locale::zh` (single Chinese locale for now; Traditional Chinese can be added as a separate locale later)
- `en`, `en-US`, `en-GB`, `en-*` → `Locale::en`
- Any unrecognized locale → `Locale::en` (fallback)

## Phasing

**Phase 1 (this spec):** Steps 1-3 of transformation priority
- Infrastructure setup (`build.rs`, `locales/`, `I18nContextProvider`)
- Base component signature migration
- Navigation/Shell i18n
- Settings pages + language switch wiring
- Deliverable: language switching works, navigation and settings fully translated

**Phase 2 (follow-up):** Steps 4-5
- Home dashboard
- Chat, Agents, Memory, and remaining pages
- Full `en.json` and `zh.json` with all keys

## Testing Strategy

- **Compile-time**: `leptos_i18n` validates key completeness and interpolation correctness
- **Manual verification**: After each transformation step, verify in browser that (a) English renders correctly, (b) switching to Chinese shows translated text, (c) no layout breakage from different string lengths
- **Cookie persistence**: Verify language choice survives page refresh
- **"Follow System"**: Test by changing browser language preference

## R9 (Tool) Alignment

Language preference is already saved to `GeneralConfig.language` via the existing `config.update` RPC tool — no new tool needed. The LLM can already change language via natural language → `config.update` tool call.

## Future Extensions (not implemented now, architecture supports)

- **New language**: add `locales/ja.json` + register in `build.rs` → dropdown auto-updates
- **RTL support**: Tailwind `rtl:` prefix + `dir` attribute, add when needed
- **Plural rules**: ICU4X already enabled, each language's rules auto-applied
