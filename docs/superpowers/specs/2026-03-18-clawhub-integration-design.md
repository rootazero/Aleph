# ClawHub Integration Design

> Aleph integrates with ClawHub (clawhub.ai), the OpenClaw project's official skill registry, enabling users to search, browse, install, and update skills directly from ClawHub — both through natural language conversation (builtin tools) and the Panel UI.

## Goals

1. **Read-only consumption** — search, browse, install, update skills from ClawHub
2. **Format compatibility** — native support for `openclaw` metadata namespace alongside `aleph`, so one SKILL.md works on both platforms
3. **Dual interface** — LLM tools for conversational access + Panel UI for visual browsing

## Non-Goals

- Publishing skills to ClawHub from Aleph
- ClawHub authentication (login/token management)
- Auto-update or background version checking
- MCP Server bridging (direct HTTP chosen over MCP indirection)

---

## Architecture Overview

```
┌──────────────┐     ┌──────────────────┐
│  LLM (tools) │     │  Panel UI (RPC)  │
└──────┬───────┘     └────────┬─────────┘
       │                      │
       ▼                      ▼
┌──────────────┐     ┌──────────────────┐
│ Builtin Tools│     │ Gateway Handlers │
│ (clawhub_*)  │     │ (clawhub.*)      │
└──────┬───────┘     └────────┬─────────┘
       │                      │
       └──────────┬───────────┘
                  ▼
         ┌────────────────┐
         │ ClawHubClient  │  (shared, Arc)
         └───────┬────────┘
                 ▼
         ┌────────────────┐
         │ clawhub.ai API │
         └────────────────┘
                 │
                 ▼
         ┌────────────────┐
         │ ~/.aleph/skills│  (local install)
         └────────────────┘
```

---

## Component 1: ClawHub HTTP Client

**Location**: `core/src/clawhub/`

```
core/src/clawhub/
├── mod.rs        // module entry, re-exports
├── client.rs     // HTTP client
└── types.rs      // request/response types
```

### `ClawHubClient`

```rust
pub struct ClawHubClient {
    base_url: String,        // default "https://clawhub.ai"
    http: reqwest::Client,   // 15s timeout
}
```

- `Clone + Send + Sync`, shared via `Arc`
- Uses existing `reqwest` dependency — zero new crates
- No authentication (all endpoints are public read-only)

### API Methods

| Method | ClawHub Endpoint | Purpose |
|--------|-----------------|---------|
| `search(query, limit)` | `GET /api/v1/search?q={query}&limit={limit}` | Keyword search |
| `browse(sort, limit, cursor)` | `GET /api/v1/skills?sort={sort}&limit={limit}&cursor={cursor}` | Paginated browsing |
| `get_skill(slug)` | `GET /api/v1/skills/{slug}` | Skill detail |
| `get_versions(slug)` | `GET /api/v1/skills/{slug}/versions` | Version history |
| `download(slug, version)` | `GET /api/v1/download?slug={slug}&version={version}` | Download ZIP |

### Response Types

```rust
pub enum SortOrder { Downloads, Stars, Updated, Trending }

pub struct SkillSearchResult {
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub downloads: u64,
    pub stars: u64,
    pub owner_handle: String,
}

pub struct BrowseResponse {
    pub skills: Vec<SkillSearchResult>,
    pub cursor: Option<String>,
    pub has_more: bool,
}

pub struct SkillDetail {
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub tags: Vec<String>,
    pub owner: OwnerInfo,
    pub latest_version: VersionInfo,
    pub moderation: ModerationInfo,
}

pub struct ModerationInfo {
    pub is_malware_blocked: bool,
    pub is_suspicious: bool,
    pub verdict: String,
}

pub struct VersionInfo {
    pub number: String,
    pub changelog: String,
    pub published_at: String,
    pub files: Vec<String>,
}

pub struct OwnerInfo {
    pub handle: String,
    pub display_name: String,
}
```

### Safety

- **Malware blocked** (`is_malware_blocked: true`): refuse download, return error
- **Suspicious** (`is_suspicious: true`): include warning in output, proceed with download
- **Rate limiting**: respect HTTP 429 + `Retry-After`, surface error to caller

---

## Component 2: Builtin Tools (LLM Interface)

**Location**: `core/src/builtin_tools/clawhub.rs`

Three tools sharing one `ClawHubClient` instance via `BuiltinToolConfig`.

### Tool: `clawhub_search`

```rust
const NAME: &str = "clawhub_search";
const DESCRIPTION: &str = "Search ClawHub skill registry for skills by keyword. Returns skill names, descriptions, and download counts.";

pub struct ClawHubSearchArgs {
    pub query: String,
    #[serde(default = "default_10")]
    pub limit: usize,
}
```

Output: list of `{ slug, name, summary, downloads, stars, tags }`.

### Tool: `clawhub_install`

```rust
const NAME: &str = "clawhub_install";
const DESCRIPTION: &str = "Install a skill from ClawHub registry. Downloads and extracts to local skills directory.";

pub struct ClawHubInstallArgs {
    pub slug: String,
    pub version: Option<String>,
}
```

Flow:
1. `get_skill(slug)` — check moderation status
2. Refuse if malware-blocked, warn if suspicious
3. `download(slug, version)` — get ZIP to temp dir
4. Extract to `~/.aleph/skills/{slug}/`
5. Validate `SKILL.md` exists and parses
6. Write `.clawhub.json` metadata
7. Return `{ name, version, path, warning? }`

### Tool: `clawhub_update`

```rust
const NAME: &str = "clawhub_update";
const DESCRIPTION: &str = "Update an installed ClawHub skill to its latest version.";

pub struct ClawHubUpdateArgs {
    pub slug: String,
}
```

Flow:
1. Read `~/.aleph/skills/{slug}/.clawhub.json` — get current version
2. `get_versions(slug)` — get latest remote version
3. Semver compare — if newer, run install flow (overwrite)
4. If already latest — return "already up to date"

### Registration

- `BUILTIN_TOOL_DEFINITIONS`: 3 new entries, `requires_config: true`
- `BuiltinToolConfig`: add `pub clawhub_client: Option<Arc<ClawHubClient>>`
- `BuiltinToolRegistry`: 3 tool fields + `execute_tool` match arms
- `TOOL_GROUPS`: new group `"clawhub"` (name: "ClawHub 技能市场")

---

## Component 3: Gateway RPC Handlers (Panel Interface)

**Location**: `core/src/gateway/handlers/clawhub.rs`

### RPC Methods

| Method | Params | Purpose |
|--------|--------|---------|
| `clawhub.search` | `{ query, limit }` | Search box |
| `clawhub.browse` | `{ sort, limit, cursor }` | Hot/new skill listing |
| `clawhub.install` | `{ slug, version? }` | One-click install |
| `clawhub.detail` | `{ slug }` | View skill detail |

- Returns JSON directly (no LLM-friendly text formatting)
- `install` emits EventBus event after completion to refresh local skills list
- Shares same `ClawHubClient` instance as builtin tools

### Registration

Added alongside existing `skills.list`, `skills.install` etc. in gateway handler registration.

---

## Component 4: Skill Format Compatibility

**Modified files**: `core/src/tools/markdown_skill/spec.rs`, `parser.rs`

### Extended Metadata

```rust
pub struct SkillMetadata {
    pub requires: RequiresSpec,
    pub aleph: Option<AlephExtensions>,
    pub openclaw: Option<OpenClawMetadata>,  // NEW
}

pub struct OpenClawMetadata {
    pub emoji: Option<String>,
    pub primary_env: Option<String>,
    pub homepage: Option<String>,
    pub os: Option<Vec<String>>,
    pub always: Option<bool>,
    pub install: Option<Vec<OpenClawInstallSpec>>,
}

pub struct OpenClawInstallSpec {
    pub id: String,
    pub kind: String,            // "brew", "node", "download", etc.
    pub formula: Option<String>,
    pub package: Option<String>,
    pub url: Option<String>,
    pub bins: Option<Vec<String>>,
}
```

### Compatibility

- `#[serde(default)]` on `openclaw` field ensures backward compatibility
- Both namespaces coexist — a SKILL.md can have `aleph` + `openclaw` simultaneously
- No deep conversion: each namespace retains its semantics
- Runtime reads: `openclaw.os` for platform eligibility, `openclaw.install` for install hints

### Example dual-namespace SKILL.md

```yaml
---
name: some-skill
description: A cross-platform skill
metadata:
  requires:
    bins: ["gh"]
  aleph:
    security:
      sandbox: host
  openclaw:
    emoji: "🔧"
    primaryEnv: "GITHUB_TOKEN"
    os: ["darwin", "linux"]
    install:
      - id: brew
        kind: brew
        formula: gh
        bins: ["gh"]
---
# Some Skill

Instructions here...
```

---

## Component 5: Panel UI (ClawHub Skill Marketplace)

**Location**: `apps/panel/` (Leptos WASM)

### Layout

New **"ClawHub" tab** alongside existing local skills tab:

```
┌─────────────────────────────────────┐
│  Skills │ ClawHub                   │
├─────────────────────────────────────┤
│  🔍 [Search...]                     │
│                                     │
│  ── Hot Skills ──                   │
│  ┌─────────┐ ┌─────────┐ ┌──────┐  │
│  │ skill-a │ │ skill-b │ │ ...  │  │
│  │ ⬇ 1.2k  │ │ ⬇ 890  │ │      │  │
│  │[Install] │ │[Install]│ │      │  │
│  └─────────┘ └─────────┘ └──────┘  │
│                                     │
│  [Load more...]                     │
└─────────────────────────────────────┘
```

### Components

1. **ClawHubTab** — top-level container, manages search/browse state
2. **SearchBar** — input with 300ms debounce, calls `clawhub.search` RPC
3. **SkillGrid** — card grid display
4. **SkillCard** — name, summary, tags, downloads, stars + install button
5. **Install button states** — Not installed / Installing (spinner) / Installed (greyed out)

### Interactions

1. Enter ClawHub tab → auto-load `clawhub.browse(sort: Downloads, limit: 20)`
2. Type in search → debounce → `clawhub.search`
3. Click install → `clawhub.install` → spinner → refresh local skills
4. Scroll to bottom → cursor-based pagination, load more
5. Cross-reference local `.clawhub.json` to show "Installed" status

---

## Component 6: Installation Metadata

### `.clawhub.json` format

Written to `~/.aleph/skills/{slug}/.clawhub.json` on install:

```json
{
  "slug": "sonoscli",
  "version": "1.2.0",
  "registry": "https://clawhub.ai",
  "installed_at": "2026-03-18T10:30:00Z",
  "owner": "someuser"
}
```

### Install flow

```
download ZIP → extract to temp dir → validate SKILL.md parses
→ check if ~/.aleph/skills/{slug}/ exists
  → exists: backup as {slug}.bak, install new, delete bak on success
  → not exists: write directly
→ write .clawhub.json
→ notify skill discovery system to refresh
```

### Update detection

`clawhub_update` reads local `.clawhub.json` version, compares with remote latest via semver.

### Uninstall

No dedicated tool. Delete `~/.aleph/skills/{slug}/` directory (existing `skills.delete` RPC or manual deletion). `.clawhub.json` removed with directory.

---

## Files Changed Summary

| Action | Path | Description |
|--------|------|-------------|
| **NEW** | `core/src/clawhub/mod.rs` | Module entry |
| **NEW** | `core/src/clawhub/client.rs` | HTTP client |
| **NEW** | `core/src/clawhub/types.rs` | Request/response types |
| **NEW** | `core/src/builtin_tools/clawhub.rs` | 3 builtin tools |
| **NEW** | `core/src/gateway/handlers/clawhub.rs` | 4 RPC handlers |
| **MOD** | `core/src/tools/markdown_skill/spec.rs` | Add `OpenClawMetadata` |
| **MOD** | `core/src/tools/markdown_skill/parser.rs` | Deserialize `openclaw` namespace |
| **MOD** | `core/src/executor/builtin_registry/definitions.rs` | Register 3 tools |
| **MOD** | `core/src/executor/builtin_registry/groups.rs` | Add "clawhub" group |
| **MOD** | `core/src/executor/builtin_registry/config.rs` | Add `clawhub_client` field |
| **MOD** | `core/src/executor/builtin_registry/registry.rs` | Tool fields + execute match |
| **MOD** | `core/src/gateway/handlers/mod.rs` | Register clawhub handlers |
| **MOD** | `core/src/lib.rs` | Add `pub mod clawhub` |
| **NEW** | `apps/panel/src/clawhub/` | Leptos UI components |

## Design Principles Alignment

- **R3 Core Minimalism**: thin HTTP client, no heavy dependencies
- **R9 Everything is a Tool**: search/install/update exposed as LLM tools
- **R10 Intelligence in Prompt**: LLM decides when to search/install, no auto-detection
- **R2 UI in Leptos**: ClawHub marketplace UI in Panel WASM, not Tauri
- **P1 Low Coupling**: ClawHubClient is standalone, shared via Arc
- **P6 Simplicity**: 3 tools cover all use cases, no over-engineering
- **P7 Defensive Design**: malware check, backup-on-update, SKILL.md validation
