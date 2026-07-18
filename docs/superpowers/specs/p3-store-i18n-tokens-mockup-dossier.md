# P3 Extensions Store — i18n / Design Tokens / Mockup Dossier

Area: i18n wiring + design tokens + approved visual mockup, so store-UI copy is
translatable and styling matches the "warm paper gallery" look.

Panel root: `D:/Workspace/Aleph/interfaces/webchat` (crate `aleph-panel`,
Leptos 0.8 CSR / WASM, Tailwind v4 CSS-config, `leptos_i18n` 0.6).

All paths absolute. Line numbers are as of this scan; re-grep before editing.

---

## 1. i18n key-adding workflow (exact)

### 1a. Files & how keys are namespaced
- Locale JSON lives in `D:/Workspace/Aleph/interfaces/webchat/locales/en.json`
  and `.../locales/zh.json`. Both are a single nested JSON object, 1795 lines each;
  the EN and ZH trees are **structurally identical** (same keys, translated values).
- Namespacing = nested objects. Top-level groups: `nav`, `session`, `common`,
  `boot_gate`, `service_gate`, `notifications`, `dashboard`, `chat`, `memory`,
  `logs`, `trace`, `settings` (huge — `settings.tabs`, `settings.groups`,
  `settings.general`, `settings.plugins`, `settings.skills`, `settings.clawhub`,
  `settings.mcp`, `settings.acp`, `settings.security`, `settings.channels`, …),
  plus `settings.generation_config` near the tail.
- A key path like `settings.general.title` maps to nested object
  `{"settings":{"general":{"title":"…"}}}`.

### 1b. The existing `nav` group (en.json lines 2-9, zh.json lines 2-9)
```json
"nav": {
  "chat": "Chat",
  "dashboard": "Dashboard",
  "agents": "Agents",
  "memory": "Memory",
  "teams": "Teams",
  "settings": "Settings"
}
```
ZH equivalent: 聊天 / 仪表盘 / 智能体 / 记忆 / 团队 / 设置.

**To add the store nav entry, append `"extensions": "Extensions"` (ZH: "扩展")
to BOTH `nav` blocks.** Note `settings.groups.extensions` ("Extensions" / "扩展")
ALREADY exists (en.json:379) for the settings-sidebar group — do not confuse the
two; the store's top-level nav item is a new `nav.extensions` key.

### 1c. Build pipeline (compile-time codegen — keys are typed)
- `D:/Workspace/Aleph/interfaces/webchat/Cargo.toml`:
  - dep (line 21): `leptos_i18n = { version = "0.6", features = ["csr","cookie","plurals","format_datetime","format_nums"] }`
  - build-dep (line 86): `leptos_i18n_build = "0.6"`
  - metadata (lines 88-90): `[package.metadata.leptos-i18n]  default = "en"  locales = ["en","zh"]`
- `D:/Workspace/Aleph/interfaces/webchat/build.rs` (lines 10-23): builds the
  i18n module from `en` + `zh`, calls `infos.rerun_if_locales_changed()` and
  `generate_i18n_module(out_dir.join("i18n"))`. **Editing a locale JSON triggers
  regeneration automatically.** Keys are validated at compile time — a key present
  in `en.json` but missing in `zh.json` (or vice-versa) is a BUILD ERROR. Add the
  key to BOTH files.
- `D:/Workspace/Aleph/interfaces/webchat/src/i18n.rs` (whole file, 7 lines):
  ```rust
  include!(concat!(env!("OUT_DIR"), "/i18n/mod.rs"));
  pub use i18n::*;   // re-exports t!, t_string!, use_i18n, Locale, I18nContextProvider
  ```
  So views do `use crate::i18n::{t, t_string, use_i18n, Locale};`.

### 1d. The exact macro forms used in views
Get the context once per component: `let i18n = use_i18n();`

- **`t!` — returns a reactive view fragment** (use directly inside `view!{}` text
  position). From `src/views/settings/general.rs`:
  ```rust
  <h1 class="…">{t!(i18n, settings.general.title)}</h1>
  <div>{t!(i18n, common.loading)}</div>
  ```
  Key path is dot-separated **bare tokens** (NOT a string): `t!(i18n, common.loading)`.

- **`t_string!` — returns a `Cow<'static,str>`** for when you need an actual
  String (attributes, `.to_string()`, format args, non-view positions). From
  `general.rs:258-261` and `nav_menu.rs:41`:
  ```rust
  t_string!(i18n, settings.general.config_reload.reloading).to_string()
  t_string!(i18n, nav.chat).to_string()
  ```

- **`t!` with interpolation** — named args after the key. Real example,
  `src/views/settings/network/connection.rs:144-149`:
  ```rust
  {t!(i18n, settings.network.confirm_switch,
      target = move || if use_remote.get() { remote_input.get() }
               else { t_string!(i18n, settings.network.local_target).to_string() })}
  ```
  The matching JSON value uses `{{ target }}` (en.json:353:
  `"confirm_switch": "Switch to {{ target }} and reload the Panel — confirm?"`).
  So for store copy needing a value (e.g. "{{ count }} available"), write
  `{{ count }}` in JSON and pass `count = move || …` in `t!`.

- **Label-lookup pattern (enum → key)** — used by both nav and settings sidebar.
  `src/components/nav_menu.rs:39-48` maps a `PanelMode` enum to `t_string!`:
  ```rust
  fn label_of(mode: PanelMode, i18n: I18nContext<Locale>) -> String {
      match mode {
          PanelMode::Chat => t_string!(i18n, nav.chat).to_string(),
          PanelMode::Settings => t_string!(i18n, nav.settings).to_string(),
          …
      }
  }
  ```
  The store plan should add `PanelMode::Extensions => t_string!(i18n, nav.extensions)`
  if a new PanelMode is introduced (see §3 / nav wiring below).

### 1e. How locale is selected & persisted
- Provider at app root: `src/app.rs:1` imports `I18nContextProvider`; `app.rs:46-50`
  wraps the tree:
  ```rust
  <I18nContextProvider>
      <DashboardContext><AppContent /></DashboardContext>
  </I18nContextProvider>
  ```
- Runtime switch via `i18n.set_locale(Locale::en | Locale::zh)`. Enum variants are
  lowercase: `Locale::en`, `Locale::zh` (generated from the `locales=["en","zh"]`).
- Persistence is two-layer:
  1. **cookie** — the `"cookie"` feature on `leptos_i18n` auto-persists the chosen
     locale in a cookie (library-managed; no app code needed).
  2. **backend GeneralConfig.language** — `src/views/settings/general.rs` is the
     source of truth UI. On load (`general.rs:50-56`) it restores locale from the
     backend `cfg.language` ("en"/"zh"/None). The `<LanguageSection>` `<select>`
     (`general.rs:167-198`) writes locale immediately AND persists to backend; the
     "system" option (`general.rs:178-187`) reads `web_sys::window().navigator().language()`
     and picks zh if it `starts_with("zh")`, else en.
- **Implication for the store:** the store inherits the active locale automatically
  through context — it only needs `use_i18n()` + `t!`/`t_string!`. No store-specific
  locale plumbing.

### 1f. Net checklist to make the store translatable
1. Add a new namespace, e.g. `"extensions": { … }` (or `"store": { … }`) at top
   level in BOTH `en.json` and `zh.json` — keys for: search placeholder, chips
   (Featured / Search & Web / Developer / Data & DB / Productivity / Writing /
   Communication / Knowledge / Files / Design & Media / Automation / Finance),
   type seg (All/Skill/Plugin/MCP), trust seg (All/Official/Verified/Community),
   featured eyebrows (Editor's pick / Trending / New), card labels (Install /
   Installed / Remove / Update), drawer headings (What it does / What it can reach /
   Stars / Version / Category / Runs via / Docs), trust-modal copy (verdict text,
   Publisher/Version/Integrity/Secrets, "Command that will run", ack checkbox,
   Cancel/Continue/Back), wizard (Step x of 2, field labels, keychain note,
   Install & verify), installed view header, manual/"not in catalog" tag, trust
   labels (Official/Verified/Community/Unverified).
2. Add `"extensions"` to BOTH `nav` blocks if a top-level nav item is wanted.
3. In every store component: `let i18n = use_i18n();` then `{t!(i18n, extensions.<key>)}`
   in view text, `t_string!(…).to_string()` in attributes/format args.

---

## 2. Design-token inventory (Tailwind v4, OKLCH, CSS-config)

### 2a. There is NO `tailwind.config.js`
Tailwind v4. Single source: `D:/Workspace/Aleph/interfaces/webchat/styles/tailwind.css`
(2177 lines). Top: `@import "tailwindcss";` then `@source "../src/**/*.rs";` (scans
Rust for class names) and `@source "../dist/**/*.html";`. Tokens defined in an
`@theme { … }` block (lines 12-135). Every `--color-*` token in `@theme`
auto-generates Tailwind utilities (`bg-surface`, `text-text-primary`,
`border-border`, etc.). **Use the utility classes, not raw CSS vars, in components.**

### 2b. Color tokens (utility ⇐ token), light values @ `@theme`
| Token (CSS var) | Tailwind utility stem | Light value (OKLCH) | Use |
|---|---|---|---|
| `--color-surface` | `surface` (`bg-surface`) | `oklch(0.96 0.005 220)` | app canvas |
| `--color-surface-raised` | `surface-raised` | `oklch(1.00 0 0)` | cards |
| `--color-surface-sunken` | `surface-sunken` | `oklch(0.905 0.010 220)` | input wells |
| `--color-surface-overlay` | `surface-overlay` | `oklch(0.985 0.004 220)` | popovers/modals |
| `--color-sidebar` | `sidebar` | `oklch(0.99 0.003 220)` | left rail |
| `--color-sidebar-accent` | `sidebar-accent` | `oklch(0.55 0.120 310)` | active nav tint |
| `--color-text-primary` | `text-primary` (`text-text-primary`) | `oklch(0.20 0.015 310)` | body text |
| `--color-text-secondary` | `text-secondary` | `oklch(0.40 0.010 220)` | secondary text |
| `--color-text-tertiary` | `text-tertiary` | `oklch(0.48 0.008 220)` | muted/labels |
| `--color-text-inverse` | `text-inverse` | `oklch(0.97 0.005 220)` | text on accent |
| `--color-border` | `border` (`border-border`) | `oklch(0.86 0.009 220)` | default border |
| `--color-border-subtle` | `border-subtle` | `oklch(0.91 0.006 220)` | faint divider |
| `--color-border-strong` | `border-strong` | `oklch(0.75 0.012 220)` | strong border |
| `--color-ring` | `ring` | `oklch(0.55 0.120 310 / .35)` | focus ring |
| `--color-primary` | `primary` (`bg-primary`,`text-primary`*) | `oklch(0.55 0.120 310)` mauve | CTA / accent |
| `--color-primary-hover` | `primary-hover` | `oklch(0.50 0.110 310)` | CTA hover |
| `--color-primary-subtle` | `primary-subtle` | `oklch(0.95 0.020 310)` | accent wash |
| `--color-success` | `success` | `oklch(0.55 0.120 130)` olive | verified/ok |
| `--color-success-subtle` | `success-subtle` | `oklch(0.95 0.025 130)` | ok wash |
| `--color-warning` | `warning` | `oklch(0.60 0.080 70)` taupe | warn / unverified |
| `--color-warning-subtle` | `warning-subtle` | `oklch(0.95 0.015 70)` | warn wash |
| `--color-danger` | `danger` | `oklch(0.55 0.150 25)` muted red | risk banner |
| `--color-danger-subtle` | `danger-subtle` | `oklch(0.95 0.020 25)` | risk wash |
| `--color-info` | `info` | `oklch(0.50 0.030 220)` | info banner |
| `--color-info-subtle` | `info-subtle` | `oklch(0.95 0.010 220)` | info wash |
| `--color-chart-1..4` | `chart-1`… | mauve/olive/taupe/mist | data viz |

> NOTE: `text-primary` utility = the mauve accent color used as text; the BODY
> text utility is `text-text-primary` (token `--color-text-primary`). Don't mix
> them up — the store's CTA text on accent buttons is `text-white`/`text-inverse`,
> body copy is `text-text-primary`, muted is `text-text-tertiary`.

### 2c. Radius / shadow / focus / motion tokens (`@theme`, lines 87-134)
- Radii (utilities `rounded-md/lg/xl/2xl/3xl`): `--radius-md` 8px, `--radius-lg`
  12px, `--radius-xl` 16px, `--radius-2xl` 20px, `--radius-3xl` 28px, `--radius-full`
  9999px. All scale by `--control-ui-radius-scale` (runtime roundness knob). Mockup
  uses `--radius:14px` for cards → closest utility is `rounded-lg` (12) / `rounded-xl`
  (16); pick `rounded-xl`.
- Shadows (utilities `shadow-xs/sm/md/lg/xl`): mauve-tinted 3-layer elevation,
  lines 106-120. Mockup's `--shadow-sm/md/lg` map to `shadow-sm`/`shadow-md`/`shadow-xl`.
- `--shadow-glow` (accent-tinted), `--focus-ring`/`--focus-glow` (lines 121-130) —
  focus is already global: `:focus-visible { box-shadow: var(--focus-ring) }`
  (lines 239-242). Store inputs get the accent ring for free.
- Motion: `--default-transition-duration: 180ms`, `--ease-out`, `--duration-fast/normal/slow`
  (lines 79-85). Use existing `transition`/`duration-*` utilities.

### 2d. Typography tokens (lines 68-74) — and the FONT GAP
- `--font-sans` = **Inter** + system fallback (utility `font-sans`).
- `--font-mono` = **JetBrains Mono** + mono fallback (utility `font-mono`).
- **There is NO serif token.** The body uses Inter globally (`body{font-family:var(--font-sans)}`,
  line 195). Headings just tighten tracking (`h1..h6`, lines 224-227), still Inter.
- Fonts actually loaded: `D:/Workspace/Aleph/interfaces/webchat/index.html:13-16`
  loads ONLY `Inter:400;500;600;700` + `JetBrains Mono:400;500;600` from Google Fonts.

> **FONT GAP — the mockup's display serif + sans are NOT wired into the panel.**
> The mockup (`...-extensions-store-mockup.html:9`) loads **Fraunces** (serif
> display, used for `section-title`, card `h3`, drawer `d-name`, modal titles,
> wordmark), **Hanken Grotesk** (body sans), **JetBrains Mono** (already present),
> and **Noto Sans SC** (CJK fallback). Of these only JetBrains Mono exists in the
> panel today. Plan MUST choose one of:
>   (A) Map mockup fonts to existing tokens — body/sans → `font-sans` (Inter),
>       display serif → also Inter (drop the serif look). Lowest effort, loses
>       the "gallery" serif character.
>   (B) Add Fraunces (+ optionally Hanken Grotesk, Noto Sans SC) to the Google
>       Fonts `<link>` in `index.html:13-16` AND add a `--font-serif` token in the
>       `@theme` block (which auto-creates a `font-serif` utility) — then use
>       `font-serif` on section titles / card names / drawer & modal headings.
>       This is the faithful path to "warm paper gallery"; it is a small, additive
>       change (one `<link>` edit + one token).
> Recommend (B): add `--font-serif: "Fraunces","Noto Sans SC",Georgia,serif;`
> to `@theme`, extend the font `<link>`, keep `font-sans`/`font-mono` as-is.

### 2e. Warm-paper palette vs current cool-mauve palette
The mockup's palette is WARM (paper `#F6F3EC`, ink `#211E1A`, teal-green brand
`#10665C`). The panel's tokens are COOL ("Quiet Luxury" mist/mauve/olive/taupe,
hue 220/310). The mockup's own CSS comment (line 12) says *"maps to Aleph design
tokens at impl time"* and trust colors are bespoke. Decision for the plan:
- The store should consume the panel tokens so it respects theme/dark/accent.
  i.e. use `bg-surface`, `bg-surface-raised`, `text-text-primary`, `border-border`,
  `bg-primary`/`text-white` for CTAs, `shadow-md`, `rounded-xl`.
- The mockup's **brand teal `#10665C` is NOT a panel token.** Either (a) accept the
  panel's mauve `--color-primary` as the store CTA/brand color (cleanest, theme-
  aware, supports the 4 accent palettes), or (b) if the warm/teal gallery identity
  is a hard requirement, the store needs its own scoped tokens (e.g. a `data-store`
  or wrapper-class block in `tailwind.css` overriding `--color-primary` and the
  surface hues to warm values) — heavier, and would NOT auto-follow dark/accent.
  **Default recommendation: (a) reuse panel tokens; the "gallery" feel then comes
  from the Fraunces serif + generous spacing + paper-toned surfaces, not from a
  separate teal palette.**
- **Trust semantic colors** in the mockup (official teal, verified green, community
  slate, unverified amber, risk red) — map to panel tokens: official →
  `--color-primary`/mauve or keep a dedicated token; verified → `--color-success`
  (olive); unverified/warn → `--color-warning` (taupe); risk/danger →
  `--color-danger`. Community/slate has no panel token → use `--color-text-tertiary`
  or add one new token. Recommend adding a small `--color-trust-*` token set in
  `@theme` (4 tokens) so dark mode resolves them, rather than hardcoding hex.

---

## 3. Screen-by-screen structural summary of the mockup

Source: `D:/Workspace/Aleph/docs/superpowers/specs/2026-06-19-extensions-store-mockup.html`
(626 lines: CSS lines 10-286, markup 288-557, JS data + interactivity 559-623).
App frame is `display:grid; grid-template-columns:64px 1fr` — a 64px dark left rail
(mimics Aleph's `ModeSidebar`) + the store column.

### 3.0 Global frame / nav (markup 289-302)
- `.railnav` dark vertical rail (64px), `.brand` square "A" badge, stack of
  `.nav-i` icon buttons (Chat, Dashboard, Memory, Agents | divider | Teams,
  **Extensions (active)**), Settings pinned to bottom (`.nav-group`). Each item
  has a hover `.nav-tip` tooltip. The Extensions item sits grouped with Teams,
  below a divider. **In the real panel this maps to `ModeSidebar` + the
  `NavMenu`/`PanelMode` switcher (`src/components/nav_menu.rs`); adding the store
  means a new `PanelMode::Extensions` + route, not a hand-rolled rail.**

### 3.1 Store chrome (topbar + chips + filters, markup 305-342)
- `.topbar` (sticky, frosted `backdrop-filter:blur(12px)`, paper bg @ 82%):
  left "← Back to chat" (`.back`); `.wordmark` = serif title "Extensions" +
  subtitle "Curated by your **Store Agent** · 312 available"; `.search` flex
  input (max 440px) with leading magnifier icon, placeholder is intent-based
  ("Search by what you want to do — "query my database"…"); spacer; right
  `Installed` button with a count pill (`.btn` + `.count` badge).
- `.chips` horizontal scroll row (functional taxonomy, primary): Featured (active),
  Search & Web, Developer, Data & DB, Productivity, Writing, Communication,
  Knowledge, Files, Design & Media, Automation, Finance. Each `.chip` = pill with
  leading emoji `.ic`; active chip = inverted (ink bg, paper text).
- `.filters` secondary row: two segmented controls (`.seg` of `<button>`):
  **Type** = All / Skill / Plugin / MCP; **Trust** = All / Official / Verified /
  Community. Active segment = `.on` (ink bg). Seg button font is mono.

### 3.2 Browse home / scroll body (markup 344-400)
- `.scroll` is the scroll region. Sections:
  - **Featured strip** (`.feat-head` + `.feat-grid`): grid `1.4fr 1fr 1fr`. Three
    `.feat-card` (min-height 172px, `justify-content:space-between`): one `.lead`
    (teal gradient bg, light text, "Editor's pick" eyebrow) + two `.alt` (white
    surface, "Trending"/"New" eyebrows). Card anatomy: `.ed` mono eyebrow at top,
    then `.feat-ic` 52px rounded icon tile, serif `h3` title, `p` blurb,
    `.meta` row (kind badge + trust dot). Whole card opens the drawer on click.
  - **Shelves** (`.shelf` repeated): `.shelf-head` = mono index ("01") + serif
    `.shelf-title` + right "See all N →" link. Body `.grid` =
    `repeat(auto-fill,minmax(244px,1fr))` of `.card`. Shelves shown: 01 Search & Web,
    02 Developer, 03 Data & Databases. Cards are injected by JS (`EXT` object,
    lines 560-606) with a staggered rise-in animation.

### 3.3 Extension card anatomy (`.card`, JS `card()` lines 583-603; CSS 156-190)
- White surface, `--radius` corners, `shadow-sm`, hover lifts -3px + `shadow-md`.
- `.card-top`: `.card-ic` 44px rounded icon tile (emoji) + name block:
  `.card-name` (semibold + inline `.kind` badge) + `.card-author` (muted).
- `.card-blurb`: 2-line clamped description.
- `.card-foot`: `.trust` dot+label, `.stars` (star glyph + count, tabular-nums),
  spacer, then install control `.ibtn` ("Install" accent button) or
  `.ibtn.installed` (ghost, check icon, "Installed"). `.kind` badge variants:
  `.skill` (green), `.plugin` (purple), `.mcp` (teal), each mono uppercase.
- `.trust` variants: `.official` (teal), `.verified` (green), `.community` (slate),
  `.unverified` (amber) — colored dot + label.

### 3.4 Detail drawer (markup 446-493; CSS 196-223)
- Right-side `aside.drawer`: 480px wide (max 94vw), full height, slides in from
  right (`transform:translateX(100%)` → `none`), `shadow-lg`; backed by a `.scrim`
  (fixed, dim + blur). Close "×" button top-right.
- `.drawer-scroll` content:
  - `.d-hero`: 64px `.d-ic` icon tile + serif `.d-name` + `.d-author` +
    `.d-tags` (kind badge + trust).
  - `.d-stat` row (top+bottom bordered): 4 stat cells (Stars / Version / Category /
    Runs via) with mono "Runs via" value (`npx`).
  - `.d-section` "What it does": `.d-desc` paragraph.
  - `.d-section` "What it can reach" (permissions disclosure): stacked `.perm`
    rows, each = icon + `.pt` title + `.pd` detail. Severity variants:
    `.perm.danger` (red wash — "Runs a command on your computer"),
    `.perm.warn` (amber wash — "Network access"), `.perm.ok` (accent icon —
    "Requires a secret … stored in OS keychain").
- `.drawer-foot` (sticky bottom): primary "Install" `.ibtn` (opens trust modal) +
  secondary "Docs ↗" `.btn`.

### 3.5 Trust disclosure modal (markup 496-526; CSS 225-258)
- Centered `.modal` (560px, scale-in). `.modal-head`: mono eyebrow
  ("Review before installing · GitHub (MCP)") + serif title ("This will run a
  program on your computer").
- `.modal-body`:
  - `.verdict.red` banner: triangle-warning icon + bold verdict
    ("Community extension — installs and runs unsandboxed code") + sub-line.
  - `.kv` rows (key/value, mono-ish): Publisher (+ "namespace-verified"),
    Version ("1.4.0 · pinned"), Integrity (mono `sha256:… ✓ verified`),
    Secrets ("1 required — GITHUB_TOKEN · kept in OS keychain").
  - `.disclose` `<details>` ("Command that will run") revealing a dark `.cmd`
    block (mono, copy button) with the exact `npx -y @modelcontextprotocol/server-github`.
  - `.ack` checkbox row (amber wash): "I understand this runs third-party code…".
- `.modal-foot`: Cancel `.btn` (close) + spacer + "Continue" `.ibtn` (→ wizard).

### 3.6 Config wizard modal (markup 529-555; CSS 260-270)
- Same `.modal` shell. Head: mono eyebrow "Step 2 of 2 · Configure GitHub" + serif
  "Add your credentials".
- `.modal-body`: `.field` blocks. Each field = `label` (+ `.req` red asterisk or
  "(optional)") + `.help` hint + input. Secret field uses `.secret-row` with a
  trailing eye toggle and `input.mono type=password`; below it a `.keychain` note
  ("Stored securely in your OS keychain — never written to config"). Second field
  "Default org (optional)" plain input.
- `.modal-foot`: Back `.btn` (→ trust) + spacer + "Install & verify" `.ibtn`
  (`finishInstall()` — alert mentions atomic copy + SHA256 verify + start server +
  list tools, "Store Agent drives install + post-install verification").

### 3.7 Installed view (markup 403-439; CSS 272-281)
- `.installed` panel absolutely positioned over the store, slides in from right
  (`transform:translateX(100%)` → `none`). Own `.topbar` ("← Store" back, wordmark
  "Installed / 7 extensions · including items added before the store").
- Body = stacked `.inst-row` rows: 40px icon + name(+kind badge) + author/version
  line + a trailing status (trust badge, or `.upd` "Update → v2.2" amber pill, or
  `.manual-tag` "manual · not in catalog" dashed mono tag for items added outside
  the store) + a `.toggle` enable switch (`.toggle.off` = disabled) + ghost
  "Remove" `.ibtn`. Demonstrates: official MCP, skill with update available,
  manual/not-in-catalog MCP (disabled toggle), community MCP.

### 3.8 Interactivity model (JS, lines 559-623) — for component state shape
- Cards rendered from a JS `EXT` map keyed by shelf id; each entry:
  `{i(icon), n(name), a(author), b(blurb), k(kind: skill|mcp|plugin), t(trust:
  official|verified|community|unverified), s(stars), inst(bool)}`. Good shape hint
  for the store's catalog item struct.
- Overlay flow: `openDrawer` → drawer; drawer Install → `openTrust` (closes drawer);
  trust Continue → `openWizard`; wizard finish → `finishInstall`. Single shared
  `.scrim`. Esc closes all. Chips/segs are single-select toggles.

---

## 4. Light/dark theming — is it token-driven? (YES)

- **Fully token-driven and three-axis**, all in `styles/tailwind.css`:
  1. **Light** = the `@theme` defaults (lines 12-135).
  2. **Dark** = `.dark { … }` block (lines 262-297) re-defines the same
     `--color-*` tokens for dark; PLUS a `@media (prefers-color-scheme: dark)
     :root:not(.light) { … }` mirror (lines 300-337) for "System" mode. The two
     blocks are kept verbatim-identical (enforced by a Rust test
     `mirror_blocks_are_verbatim_copies` in `src/appearance.rs`, referenced at
     tailwind.css:640).
  3. **Accent palettes** (orthogonal to light/dark): `html[data-accent="ocean|
     forest|sunset|rose"]` blocks (lines 348-461) re-tint only `--color-primary*`,
     `--color-ring`, `--color-sidebar-accent`, `--color-chart-1` — both light and
     `.dark` variants exist. Default (absent) = mauve hue 310.
  4. **Material** (`data-material="liquid|aurora"`, lines 681-887) drives the glass
     `--mat-*` primitives — relevant only if the store uses `.glass` surfaces.
- **Switching mechanism:** a `.dark` / `.light` class and `data-accent` /
  `data-material` attributes are set on `<html>` at runtime (the Appearance settings
  page). Components never read brightness directly — they use the semantic utilities
  (`bg-surface`, `text-text-primary`, `border-border`, `bg-primary`…) and the right
  values resolve via cascade.
- **Implication for the store:** if the store is built with the panel's semantic
  utility classes (and panel tokens for trust/brand colors), it **respects
  light/dark/accent automatically with zero extra work**. The risk is ONLY if the
  store hardcodes the mockup's warm hex values (`#F6F3EC`, `#10665C`, etc.) — those
  would be frozen in light/warm and would NOT flip in dark mode. So: do not inline
  the mockup hex; bind to tokens. Any NEW store-specific token (serif font, trust
  colors, optional warm surface override) must be declared in BOTH the light
  `@theme`/`:root` and the `.dark` (+ system-dark mirror) blocks to stay theme-aware.

---

## 5. Concrete edit points (for the plan)
- Add nav copy: `locales/en.json` + `locales/zh.json` → `nav.extensions`,
  plus a new `extensions.*` (or `store.*`) namespace in BOTH files.
- Fonts: `index.html:13-16` (extend Google Fonts `<link>` with Fraunces[/Hanken/
  Noto Sans SC]) + `styles/tailwind.css` `@theme` (add `--font-serif`).
- Optional trust tokens: add `--color-trust-*` (4) to `@theme` + `.dark` + system
  mirror in `styles/tailwind.css`.
- Routing/nav: `src/components/mode_sidebar.rs` + `src/components/nav_menu.rs`
  (`PanelMode` enum, `route_of`, `label_of`, `icon_of`) + `src/app.rs` router
  (`MainContent` / a new `ExtensionsRouter`, mirroring `SettingsRouter` at
  `app.rs:405-461`). [These nav/route files are owned by another dossier area;
  noted here only for the font/i18n cross-references.]
- Store components: each uses `use crate::i18n::{t, t_string, use_i18n, Locale};`,
  `let i18n = use_i18n();`, semantic Tailwind utilities, `font-serif` for display
  headings.
