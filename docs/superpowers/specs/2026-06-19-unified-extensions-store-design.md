# Unified Extensions Store — Design Spec

- **Date:** 2026-06-19
- **Status:** Approved for planning (brainstorming → writing-plans)
- **Owner:** rootazero

---

## 概述 (Summary)

为 Aleph 增加一个**统一扩展商店**：对普通用户，MCP / plugin / skill 不再分家，统统叫**扩展 (Extension)**，以 app-store 形式呈现——浏览、一键装、需配置就弹向导、启停/卸载；对高级用户保留 kind 标签与原有高级面板。

核心机件：
- **薄聚合**——对外一个 `Extension` 抽象，对内按 `kind` 路由到三套既有后端（plugin marketplace / MCP / skill）。
- **多源实时聚合**——`SourceProvider` 抽象之上，由确定性数据管道拉取少数 mirror-safe 精选源，缓存进本地 SQLite。
- **内置 Store Agent**（受保护、不可删，和 `main` 同级别）独占 *发现 → 策展 → 安装 → 验证* 全生命周期；安装的安全护栏（信任闸门 / SHA256 / keychain / 沙箱）由系统强制，agent 无权跳过。
- **长尾经 LLM 按需安装**——用户指向任意 GitHub/URL，Store Agent 读其文档推断安装规格，覆盖整个开源社区，绕开全网爬虫。

A "thin federation" store: one user-facing `Extension` concept, internally routed by `kind` to the existing plugin/MCP/skill subsystems; a built-in, non-deletable **Store Agent** owns discover→curate→install→verify, with system-enforced trust rails it cannot bypass.

---

## 1. Goals / Non-goals

### Goals (v1)
- One top-level **Extensions** surface in the chat window; full-screen takeover; browse + search/filter by kind+tag; one-click install; config wizard; enable/disable; uninstall; kind tag badges.
- Unify MCP / plugin / skill under a single `Extension` umbrella **at the store layer**, internally routed by `kind`.
- Multi-source, real-time-ish catalog aggregation from a few **mirror-safe curated sources**, cached locally; long-tail coverage via **LLM-assisted install-from-URL**.
- A built-in, protected **Store Agent** that curates the catalog and drives installs.
- Reconcile **already-installed** extensions (configured before the store existed) into the store.
- Trust/security rails: pre-install disclosure, trust tiers, keychain secrets, pin + re-gate-on-change, injection hardening.

### Non-goals (v1)
- GitHub-wide crawler populating the browse grid (deferred; needs backend token proxy). Long-tail handled by on-demand LLM install instead.
- Ratings / reviews / download counts as first-class operational/curation data.
- Third-party self-publish / submission flow.
- Auto-update (manual "Update" button only).
- Process/container **sandboxing** of untrusted stdio MCP servers — flagged **fast-follow**, not v1 (see §11).
- Renaming the existing `src/extension/` module (kept as the plugin backend; see §4).

---

## 2. Decision Log (locked during brainstorming)

| # | Decision | Choice |
|---|---|---|
| 1 | Catalog substrate | **Thin federation**: one façade, internal kind-routing over existing backends |
| 2 | Placement | New **top-level `Extensions` mode**, grouped with Teams in the upper nav; **full-screen takeover** of chat; explicit "← Back to chat" button |
| 3 | Old per-type panels | **Demote** Settings→Extensions (MCP/Plugins/Skills) to "Advanced management"; **remove ClawHub menu** |
| 4 | v1 scope | MVP closed loop (browse + filter/search + install + config wizard + toggle + uninstall + kind tags) |
| 5 | Install-in-progress guard | **Allow leave but confirm dialog** (backend install is atomic → interrupt only cancels, never corrupts) |
| 6 | LLM in install | **Hybrid**: deterministic fast-path for clean specs + LLM-assisted for long-tail; trust gate + verify mandatory on both |
| 7 | Long-tail / GitHub | **Curated browse + LLM on-demand URL/search install**; full crawler deferred |
| 8 | v1 safety boundary | **Informed-consent disclosure + trust tiers**; sandbox = fast-follow; explicit "I understand the risk" gate for community/LLM-derived MCP |
| 9 | Store Agent | User's idea adopted: built-in agent owns **discover/curate/install/verify**; **protected, non-deletable** like `main`; SubAgent mode |
| 10 | Store Agent boundary | Agent curates **presentation & discovery + drives install**; **install spec, trust verdicts, SHA256, secrets, sandbox are system rails** the agent cannot fabricate/bypass |
| 11 | Installed reconciliation | Store shows installed **and** not-installed; pulls in already-installed extensions, matched to catalog where possible |
| 12 | Umbrella naming | **New umbrella module + `Extension*` types**; do NOT rename existing `src/extension/` |
| 13 | Browse taxonomy | **Primary axis = functional categories** (Developer, Data, Search, Productivity…), curated by the Store Agent; **kind (Skill/Plugin/MCP) is a secondary badge/filter only** |
| 14 | UI design | Designed via `frontend-design` — "warm paper gallery" editorial direction; mockup at `docs/superpowers/specs/2026-06-19-extensions-store-mockup.html` |

---

## 3. Architecture

```
确定性数据管道  SourceProviders ──background sync──► local SQLite catalog cache
  · marketplace(existing) · official MCP registry · Docker MCP catalog
        │                                  + deterministic InstallSpec + TrustTier (verifiable)
        ▼
【Store Agent】 built-in · protected · non-deletable  (private STORE_TOOLS)
  DISCOVER : sync curated sources  +  long-tail "LLM reads any repo/URL" on demand
  CURATE   : fill metadata gaps (blurbs/tags/icon fallback) · featured/high-star/collections
             (lazy + incremental + cached)
  INSTALL  : clean spec → deterministic install tool (no LLM) ; wild → derive InstallSpec
        │            ┌──── system-enforced rails (agent cannot bypass) ────┐
        │            │ trust gate (= required user approval) · SHA256 ·     │
        │            │ keychain secrets · post-install verify · sandbox(ff) │
        ▼            └──────────────────────────────────────────────────────┘
   curated catalog (cached) ──► Store UI (top-level Extensions mode, reads cache, offline-capable)
```

**Layering:**
1. **SourceProvider pipes** — deterministic fetch + normalize to `ExtensionEntry` + `InstallSpec`. Sit *above* the existing `MarketplaceManager` (not replacing it).
2. **Store Agent (curation/editorial/install brain)** — orchestrates sync, enriches metadata, curates editorial surfaces, drives installs. Never fabricates install specs or trust verdicts.
3. **System rails** — trust gate, integrity, secrets, verify; enforced by the store/permission layer, not at the agent's discretion.
4. **`extensions.*` JSON-RPC façade** — what the UI talks to; delegates to existing per-kind handlers.
5. **Store UI** — Leptos, reads the cached curated catalog.

---

## 4. Naming & module layout

The umbrella concept **Extension** (扩展) ⊇ {Skill, Plugin, Mcp} is realized by **new types in a new module**; existing modules are untouched.

- **New:** `src/store/` (umbrella + aggregation layer)
  - `Extension`, `ExtensionEntry`, `ExtensionKind { Skill, Plugin, Mcp }`, `InstallSpec`, `TrustTier`
  - `SourceProvider` trait + `ProviderRegistry`
  - catalog cache (SQLite), background sync scheduler
  - installed-state reconciliation
  - Store Agent wiring + private store tools
- **Unchanged:** `src/extension/` (= the **plugin** backend; `ExtensionManager`, `marketplace`, `manifest`, `loader`, `runtime`), `src/skill/`, `src/mcp/`, `src/agents/`.
- **Mapping rule:** the store-facing `ExtensionKind` is distinct from the runtime `PluginKind { Wasm, Mcp, Static }`. Map `ExtensionKind` → backend at install time, not in the catalog:
  - `ExtensionKind::Plugin` → existing `src/extension/` plugin install (`PluginKind::Wasm`/`Mcp`)
  - `ExtensionKind::Skill` → skill install (`PluginKind::Static` / `~/.aleph/skills/`)
  - `ExtensionKind::Mcp` → `mcp.add` config

> Note (debt): the word "extension" is overloaded — umbrella **type** vs the existing plugin-host **module**. Disambiguated by type names; honest rename of `src/extension/`→`src/plugin/` is explicitly out of scope (repo is mid multi-branch refactor).

---

## 5. Data model

```rust
// src/store/types.rs (new)

pub enum ExtensionKind { Skill, Plugin, Mcp }      // store-facing; != runtime PluginKind; SECONDARY (a badge)

// PRIMARY browse taxonomy — functional, curated. Open set; start with a fixed seed list.
pub enum ExtensionCategory {
    Search, Developer, Data, Productivity, Writing, Communication,
    Knowledge, Files, Design, Automation, Finance, Utilities, Other,
}

pub enum TrustTier { Official, Verified, Community, Unverified } // T0..T3

pub enum InstallSpec {
    McpStdio { command: String, args: Vec<String>, env_decls: Vec<EnvDecl> },
    McpRemote { url: String, transport: McpTransport, headers: Vec<HeaderDecl> },
    OciImage { image: String /* image@sha256 */ },
    GitDir { git_url: String, subdir: Option<String>, git_ref: Option<String>, sha: Option<String>, tarball_url: Option<String> },
}

pub struct EnvDecl { name: String, description: Option<String>, required: bool, secret: bool, default: Option<String>, placeholder: Option<String> }

pub struct ExtensionEntry {
    id: String,                       // provider-prefixed, e.g. "mcp-official:io.github.user/foo"
    kind: ExtensionKind,
    name: String,
    description: String,
    author: Option<AuthorInfo>,       // reuse existing AuthorInfo
    icon: Option<String>,             // kind-derived placeholder if missing
    category: ExtensionCategory,      // PRIMARY browse axis (functional, curated) — NOT kind
    tags: Vec<String>,                // free-form; always includes kind for the secondary kind-filter
    version: Option<String>,
    source_id: String,                // provider id; also the de-dup key alongside repo_url
    repo_url: Option<String>,         // de-dup key
    trust_tier: TrustTier,
    requires_config: bool,            // true iff InstallSpec has any required env/userConfig/arg
    config_schema: Option<JsonValue>, // JSON Schema (synthesized for MCP from env_decls)
    config_ui_hints: HashMap<String, ConfigUiHint>, // reuse existing ConfigUiHint (sensitive/help/placeholder)
    installed: bool,
    enabled: bool,
    update_available: bool,
    meta: HashMap<String, JsonValue>, // namespaced enrichment (com.aleph.store/*): stars, scan verdict, curator blurb
}
```

**Metadata mapping (source → ExtensionEntry):** per-source normalization; gaps default to kind-derived placeholders. For MCP-official, `config_schema` is **synthesized** from `environmentVariables[]` + required `packageArguments[]` (`name`→property, `isRequired`→`required[]`, `isSecret`→`config_ui_hints.sensitive`, `default`, `valueHint`→placeholder). For plugins, reuse `plugin.json` `userConfig{}` → existing `config_schema`/`config_ui_hints`.

---

## 6. SourceProvider layer

```rust
// src/store/provider.rs (new)
#[async_trait]
pub trait SourceProvider: Send + Sync {
    fn id(&self) -> &str;                              // "cc-marketplace" | "mcp-official" | "docker-mcp"
    fn kinds(&self) -> &[ExtensionKind];
    fn trust_tier(&self) -> TrustTier;                 // source-level default
    async fn sync(&self, ctx: &SyncCtx) -> Result<Vec<ExtensionEntry>, SourceError>; // background only
    async fn search(&self, _q: &Query) -> Option<Result<Vec<ExtensionEntry>, SourceError>> { None } // optional live
    async fn resolve_install_spec(&self, e: &ExtensionEntry) -> Result<InstallSpec, SourceError>;    // cached, action-time
}
```

`ProviderRegistry` fans `sync()` out concurrently (one failing provider never blocks the catalog), writes the normalized rows into **one local SQLite catalog** (`~/.aleph/`, reuse `aleph_home_dir`), de-dups across providers by `(repo_url, package_identifier)`. **UI + JSON-RPC depend only on `ExtensionEntry` + `InstallSpec`.** Adding a community = implement the trait + register; zero UI change.

### v1 sources (mirror-safe only)

| Provider | kind | Effort | Access | Trust |
|---|---|---|---|---|
| Aleph built-in + `anthropics/claude-plugins-community` + `claude-plugins-official` (`marketplace.json`) | plugin (+bundled skills) | low | reuse existing `MarketplaceManager`; one ETag raw GET seeds 2000+ SHA-pinned plugins | T0/T1 |
| Official MCP Registry (`registry.modelcontextprotocol.io`) | mcp | med | unauth REST `GET /v0/servers?limit&cursor&updated_since&search`; validate vs pinned `server.schema.json (2025-12-11)`; drop `status="deleted"` | T2 (namespace-verified only) |
| Docker MCP Catalog (`catalog.yaml`) | mcp | low | one unauth YAML GET; ~220 signed `image@sha256` servers | T0/T1 |

- **ClawHub:** NOT a first-class deterministic provider in v1 (no stable third-party API, low trust, documented attack chain). Reachable via the Store Agent **long-tail/URL path** (LLM reads clawhub). Promote to a dedicated provider if/when a stable API appears.
- **Deferred (post-v1):** GitHub topic/code-search crawler (needs backend token proxy), Glama/Smithery/npm enrichment overlays, awesome-list seeds.

### Caching & rate-limit strategy
- Browse/filter **always served from SQLite** (offline-capable; per-source "last synced" + "may be stale" chip).
- Background sync scheduler, per-provider TTL: marketplaces ~6–12h (ETag, 304=no-op); MCP registry hourly incremental (`updated_since`); Docker daily. Syncs isolated + concurrent.
- Search hits local SQLite index with ~200–300ms debounce; live `search()` only when cache cold/stale, throttled.
- **Never ship a GitHub PAT in the client**; `raw.githubusercontent` fetches OK direct (CDN). Crawler (deferred) must run behind a backend proxy.
- MCP registry is **preview** → on empty/changed dataset keep last-good cache rather than wiping.

---

## 7. Installed-state reconciliation

On catalog build, enumerate **locally installed** items via existing `plugins.list`, `skills.status`, `mcp.list` and represent each as `ExtensionEntry { installed: true }`:
- **Matched** to a catalog entry (by `repo_url` / `package_id` / `name`) → merge → enriched + "installed".
- **Unmatched** (hand-configured old MCP, local skill) → surfaced as **"Installed · manual / not in catalog"** (TrustTier::Unverified/manual), still toggle/uninstall-able from the store.
- Browse grid shows an **Installed** badge; the **Installed** section lists *all* locally-installed extensions regardless of how they were installed.

---

## 8. `extensions.*` RPC façade

Thin dispatch over existing handlers (UI talks only to this):

| `extensions.*` | skill | plugin | mcp |
|---|---|---|---|
| `catalog` (browse, reads cache) | — | — | — (served from SQLite; provider-agnostic) |
| `detail` / `resolve` | clawhub/long-tail | `MarketplaceManager` | registry record |
| `install` | skill install path | `plugin.marketplace.install` | `mcp.add` (+ wizard secrets) |
| `configure` | `skills.update` | write plugin `config_schema` | write mcp `env`/`args` |
| `toggle` | `skills.update(enabled)` | `plugins.enable`/`disable` | `mcp.start`/`stop` |
| `uninstall` | `skills.remove` | `plugins.uninstall` | `mcp.delete` |

Plus: `extensions.sources.list/add/remove/refresh` (wrap `plugin.marketplace.*`), `extensions.installed` (reconciled list).

---

## 9. Store Agent

A built-in, **protected (non-deletable)** agent that owns discover→curate→install→verify.

- **Registration:** add `store` to `builtin_agents()` (`src/agents/registry.rs`), `AgentMode::SubAgent`, `source: AgentSource::Builtin`.
- **Protection:** extend the deletion guard in `src/builtin_tools/agent_manage/delete.rs` — **generalize to reject `source == AgentSource::Builtin`** (covers `main` + `store` + the other builtins, fixing the current "only main is guarded" inconsistency). Hide delete/disable for it in the Agents panel (`interfaces/webchat/src/views/agents/`).
- **Private tools:** new `STORE_TOOLS` set in `src/agents/tool_sets.rs`; new `AlephTool` impls under `src/builtin_tools/store/`:
  - `store_catalog_sync` — run provider syncs into cache
  - `store_fetch_docs` — fetch a repo/URL's README/manifest (long-tail)
  - `store_resolve_spec` — derive/validate an `InstallSpec`
  - `store_install_run` — execute install via the **deterministic** install path (atomic, SHA256)
  - `store_install_verify` — post-install verification
  - Scoped via `allowed_tool_sets = ["STORE_TOOLS"]`; **not exposed to chat agents** (AllowlistToolService).
- **Internal driving:** spawned via `subagent_spawner::spawn(SpawnerBase, SpawnRequest)` for (a) background curation jobs and (b) install flows.
- **Curation (background, lazy + cached):** fill metadata gaps (blurbs/tags/icon fallback), **assign the functional `category`** (deterministic keyword map first; LLM only for ambiguous entries), editorial surfaces (featured / high-star by *real* source stars / collections), ranking. Only enriches entries lacking good metadata and slices actually browsed; results persisted (one-time cost per entry). `category` is the **primary** browse axis; `kind` is never a curation decision (it's intrinsic).

---

## 10. Install model

Two tiers, both gated by the same system rails:

- **Deterministic fast-path (most installs, no LLM):** clean machine-readable specs (MCP-official synthesized spec, Docker image ref, marketplace plugins/skills) → `store_install_run` directly.
- **LLM-assisted (long-tail / URL / ClawHub):** Store Agent `store_fetch_docs` → `store_resolve_spec` (infer kind + spec + required secrets) → trust gate → `store_install_run` → `store_install_verify`.

**Verification (post-install):** MCP starts + lists tools; skill parses; plugin loads. On failure → self-repair attempt or honest report (no silent "success").

**Install-in-progress guard:** if the user navigates away mid-install → confirm dialog ("Install in progress; leaving cancels it. Continue?"). Backend install is atomic, so worst case is a cancelled install.

---

## 11. Trust & security model

**Two risk classes, surfaced per item:**
- MCP-stdio → red banner "runs commands on your computer".
- skill / plugin → yellow banner "can instruct the agent" (prompt-injection / tool-poisoning).
- MCP-remote URL → softer note.

**Trust tiers (source-level default, one human phrase):** `Official` (T0) · `Verified publisher` (T1) · `Community` (T2, listed+scanned, unverified author) · `Unverified` (T3, manual URL / long-tail). **"Listed in a registry" ≠ vetted — Aleph is the trust boundary.**

**Mandatory pre-install disclosure screen (no silent bypass; enforced at install/run layer):**
1. exact command + args (verbatim, copyable);
2. each required env/secret and what it's for;
3. declared network + filesystem reach;
4. pinned version + **SHA256** Aleph will install (reuse `verify_plugin_integrity`);
5. trust tier + provenance/scan result.

Consumer-friendly (Chrome model): one-line risk verdict + tier badge up top, plain-language capabilities, only the single scariest applicable warning, technical details behind a "Show details" expander, secrets asked at runtime-with-context. **Community/LLM-derived MCP also require an explicit "I understand the risk" ack** (the interim substitute for sandboxing).

**Secrets:** every `secret`/`sensitive` field renders masked and is stored in the **OS keychain**, never plaintext config / never raw inheritable child env. Reuse Aleph's **WASM credential-injection boundary** (`src/extension/runtime/wasm/credential_injector.rs`) so a compromised server can't read the whole environment.

**Pin + re-gate on change (kills rug-pulls):** bind approved SHA256 to approved version **and** the approved set of tool/skill descriptions. On version bump / hash change / tool-description change → re-scan + re-prompt ("this extension changed what it can do"). Reuse existing `should_update` + `verify_plugin_integrity` + atomic `update_plugin_from_cache`; add the re-consent gate.

**Injection hardening (skill/plugin/tool text):** before display/approval, scan for hidden-instruction patterns (zero-width chars, RTL overrides, hidden/oversized blocks, "ignore previous / read .env / exfiltrate"). Render the **full untruncated** text in the approval UI. Also applies to text reaching the **curator agent** (a repo README must not be able to inject the Store Agent into featuring malware).

**Lifecycle:** installed-extensions audit view (what's installed, tier, granted permissions, last update); per-extension disable/revoke (reuse `PluginStatus::Disabled`); source allowlist gate for which sources may be added.

**Sandbox (fast-follow, NOT v1):** isolated runtime (subprocess jail / container / WASM) + per-host network allowlist + scoped FS for untrusted stdio MCP. Big, OS-specific work — explicitly deferred; informed consent is the v1 boundary (matches current VS Code / Claude Desktop / Cursor norms).

---

## 12. UI / Navigation (Leptos)

> Designed via `frontend-design`. Visual reference (clickable, all screens/states): **`docs/superpowers/specs/2026-06-19-extensions-store-mockup.html`**.

**Aesthetic direction — "warm paper gallery" (editorial / curated).** A calm, trustworthy, magazine-quality showcase that reads as *curated* (matching the Store Agent's editorial role), deliberately NOT another dark dev-tool grid. Three-tier type system: `Fraunces` (serif display — store/category titles), `Hanken Grotesk` (UI/body), `JetBrains Mono` (kind badges, command disclosure, SHA — anything technical). Warm paper bg + ink text + single deep teal-green brand accent; trust tiers carry semantic colors. All colors map to Aleph's existing Tailwind design tokens at implementation time; supports light/dark + `leptos_i18n` (en/zh).

- **Top-level mode:** add `PanelMode::Extensions` (`components/mode_sidebar.rs`), grouped with Teams in the **upper** nav (`nav_menu.rs` `ALL_MODES` + `route_of`/`label_of`/`icon_of`); route `/extensions`; `MainContent` swaps chat → store (**full-screen takeover**); explicit "← Back to chat" button + clicking "Chat" returns.

- **Browse taxonomy — PRIMARY = functional category, SECONDARY = kind:**
  - Top **functional category chip bar** (Featured · Search & Web · Developer · Data & DB · Productivity · Writing · Communication · Knowledge · Files · Design · Automation · Finance · Utilities) drives the browse — this is the main axis.
  - `kind` (Skill/Plugin/MCP) and `trust tier` are **secondary segmented filters** + a small **mono badge** on each card (for the curious / advanced users). Users browse by *what an extension does*, not its technical type.
  - **Home layout:** a **Featured** strip (Store Agent's editorial picks) + per-category **shelves** ("Developer →", "Data & Databases →", each with "See all"). Clicking a category chip filters to that category's full grid.
  - **Card:** icon · name · author · 2-line blurb · footer = kind badge + trust dot + real star count + Install/Installed button.

- **Store screens:**
  1. **Browse** (above): featured + category shelves + responsive card grid; debounced local search phrased around *intent* ("query my database", "search the web").
  2. **Detail drawer** (right slide-over): description, author, version, **"What it can reach"** permissions (the two risk classes rendered as banners), kind/category/tags, trust tier; install / docs.
  3. **Pre-install trust disclosure modal:** one-line risk verdict + tier badge; exact command (mono, copyable); secrets + purpose; network/fs reach; pinned version + **SHA256 ✓**; "Show technical details" expander; **"I understand the risk" ack** for community/LLM-derived items. (§11)
  4. **Config wizard modal:** new `components/json_schema_form.rs` renders a form from `config_schema` + `config_ui_hints` (`sensitive` → masked + "stored in OS keychain" affordance). (No generic JSON-Schema form builder exists today — net-new.)
  5. **Installed view** (slide-over panel): all locally-installed (reconciled), including pre-store/manual items flagged "manual · not in catalog"; enable/disable toggle, remove, update badge + button.

- **Install UX:** Install → trust disclosure → (wizard if `requires_config`) → progress state → success (post-install verify). Leave-mid-install → confirm dialog.
- Reuse existing card/modal/tab/toggle/badge/empty/error patterns from `views/settings/{mcp,plugins,skills}.rs`.

---

## 13. Migration & i18n

- **Demote** `views/settings/{mcp,plugins,skills}.rs` to an **"Advanced management"** group (manual MCP command/env editing, raw manifest view) — kept for power users, no longer the primary path.
- **Remove** the ClawHub settings menu (`SettingsTab::ClawHub`, `views/settings/clawhub.rs` route) — subsumed into the store; ClawHub becomes a long-tail source. (`acp` is unrelated — leave it.)
- **i18n:** add `nav.extensions` + store-UI keys to `interfaces/webchat/locales/{en,zh}.json`; reuse `leptos_i18n`.

---

## 14. Phasing (build order)

| Phase | Content |
|---|---|
| **P0 Foundations** | `src/store/` umbrella types (`ExtensionKind/Entry/InstallSpec/TrustTier`); `extensions.*` façade RPC (delegating); local SQLite catalog cache; **installed-state reconciliation** |
| **P1 Source layer** | `SourceProvider` + `ProviderRegistry` + background sync; 3 providers (marketplace[low], MCP registry[med], Docker MCP[low]); normalize + synthesize MCP `config_schema`; de-dup |
| **P2 Trust rails** | trust tiers · pre-install disclosure screen · keychain secrets (WASM injection) · pin+re-gate (`verify_plugin_integrity`+atomic update) · injection scan · installed audit / disable·revoke. **Gates any install.** |
| **P3 Store UI** | top-level `Extensions` mode (with Teams, full-screen, back button) · browse grid + filter/search · detail drawer · installed section · `json_schema_form.rs` · install progress + leave-confirm |
| **P4 Store Agent** | register protected `store` builtin + generalize delete guard · `STORE_TOOLS` + 5 private tools · background curation (enrichment/editorial/high-star) · install ownership (fast-path + LLM long-tail) · verify + self-repair |
| **P5 Migration & i18n** | demote MCP/Plugins/Skills panels → Advanced · remove ClawHub menu · en/zh i18n |

> Dependency note: P2 rails must wrap any install before P3/P4 ship installs. The deterministic fast-path install can exist before P4 (the agent orchestrates it; the primitive is independent). writing-plans will sequence precisely.

---

## 15. Open risks (flagged)

1. **MCP registry is preview** — schema/data may reset before GA; mitigate with pinned schema + last-good cache, but a breaking change still forces client work.
2. **No sandbox in v1** — single biggest security gap; informed consent is the only boundary for stdio MCP. Acceptable per industry norms but must be honestly surfaced; sandbox is the top fast-follow.
3. **Aleph automated scan not built** — T1 "Verified" relies on a scan that doesn't exist yet; if it slips, tiers degrade to provenance-only.
4. **Curator-agent injection** — a repo's README could try to inject the Store Agent; mitigated by injection hardening applied to text reaching the curator, and by keeping trust verdicts deterministic (agent can't promote tiers).
5. **ClawHub brittleness** — long-tail path depends on LLM reading an unstable site; acceptable as best-effort, not a guaranteed install.
6. **De-dup imperfect** — forks/monorepos/renames may dup or wrongly-merge; degrades catalog quality.
7. **Curation cost** — local Store Agent enrichment must stay lazy/incremental/cached or it burns user tokens.

---

## 16. Reuse map (existing code to leverage)

- Plugin marketplace: `plugin.marketplace.{list,add,remove,update,install}`, `MarketplaceManager` (`src/extension/marketplace/`), `installer.rs` (atomic + `verify_plugin_integrity` SHA256).
- Plugins: `plugins.{list,install,uninstall,enable,disable}`; `PluginRecord`, `PluginStatus::Disabled`.
- MCP: `mcp.{list,add,update,delete,status,start,stop,restart}`; `McpManagerConfig`.
- Skills: `skills.{status,update,install_dep,remove,install}`; `SkillRegistry`.
- Config wizard inputs: `config_schema` + `config_ui_hints` (`ConfigUiHint { label, help, sensitive, placeholder, advanced }`).
- Secrets: WASM credential injection (`src/extension/runtime/wasm/credential_injector.rs`).
- Agents: `AgentDef`/`AgentSource` (`src/agents/types.rs`), `builtin_agents()` (`src/agents/registry.rs`), delete guard (`src/builtin_tools/agent_manage/delete.rs:92`), `spawn`/`SpawnerBase` (`src/agents/subagent_spawner/`), `AllowlistToolService`, `tool_sets.rs`, `AlephTool` (`src/builtin_tools/mod.rs`).
- Frontend: Leptos/WASM `aleph-panel` (`interfaces/webchat/`), JSON-RPC/WS (`context.rs`), `PanelMode` (`components/mode_sidebar.rs`), `nav_menu.rs`, `settings_sidebar.rs`, `views/settings/{mcp,plugins,skills,clawhub}.rs`, `views/agents/`. (No `json_schema_form` component yet — net-new.)
- ClawHub (to subsume): `clawhub.{search,browse,detail,install}` (`src/gateway/handlers/clawhub.rs`).
