# Hub Content Kinds (prompt / workflow) + Weekly Cron Partition — Design

**Date:** 2026-06-22
**Status:** Approved (design)
**Scope:** New **content** catalog (`prompt` / `workflow`) as a *second, parallel* wire
artifact — `public/catalog.json` (install contract) is untouched. Plus a weekly,
kind-partitioned pipeline schedule. Aleph-side consumption is a coordinated
follow-up spec in `../Aleph` (this spec defines the contract it implements against).

## Problem / Motivation

Aleph Hub today catalogs only **installable** extensions: `skill | plugin | mcp`,
each carrying an `install_spec`. The whole design — schema, pipeline, website — is
install-centric.

Two artifact families the community now produces do **not** fit that model:

- **prompt** — a reusable piece of *text*. The user action is **copy / one-click
  insert into the chat input box**, never "install + register."
- **workflow** — a Claude Code **Agent Workflow** engineering asset: a runnable
  `Workflow({scriptPath})` `.js` script (`export const meta = {…}` + `agent()` /
  `pipeline()` / `phase()` orchestration). Aleph has shipped an equivalent workflow
  runner. These are poised to become a peer protocol to MCP/skills. The user action
  is **fetch the script and run it**, not "install into skills/".

Both must become **first-class kinds** in the Hub, browsable alongside
skill/plugin/mcp, and usable by every Aleph instance. Separately, the daily
pipeline currently re-crawls *everything* every day; with five kinds that is
wasteful and rate-limit-prone. We want discovery **partitioned across the week by
kind**, weighted by volume.

## Boundary finding (why a second artifact, not new kinds inline)

Adding `prompt`/`workflow` to the existing `catalog.json` would be a **hard
breaking change** to the Hub↔Aleph wire contract:

- Aleph's `ExtensionKind` (`../Aleph/src/hub/types.rs:5`) is a **closed** enum
  (`skill|plugin|mcp`) with no unknown-variant fallback, and Aleph parses the whole
  artifact in one `serde_json::from_str` (`../Aleph/src/hub/catalog_client.rs:79`).
  A single `kind:"prompt"` entry makes the **entire** catalog fail to parse for every
  un-updated client.
- `schema_version > SUPPORTED_SCHEMA_VERSION` (currently `1`) is rejected wholesale
  (`catalog_client.rs:81`), so bumping the shared artifact to `2` also bricks old
  clients.
- Install routing is kind-blind to new kinds: `run_install` (`../Aleph/src/hub/install.rs:160`)
  routes `git_dir` → skill only when `kind == Skill`, else treats it as a plugin.

Therefore content kinds ship as a **separate artifact** with its own version line.
`catalog.json` stays `schema_version: 1`, byte-for-byte compatible. Existing Aleph
installs are at **zero risk**; the new content consumer is opt-in and independent.

## Decisions (resolved during brainstorming)

| # | Decision | Choice |
|---|----------|--------|
| D1 | Rollout / contract coupling | **Separate, decoupled artifact** `catalog-content.json`; `catalog.json` unchanged. |
| D2 | Entry granularity | **Single unit = one entry.** One prompt = one entry; one runnable `.js` workflow script = one entry. Collection repos are *exploded* into many entries. |
| D3 | Weekly cron partition | `2+1+1+2+1` over 7 days (skill/prompt weighted ×2). |
| D4 | workflow form | Claude Code Agent Workflow — **a single `.js` file**. One js file = one entry. README/`results-*.json` are context the curator reads, **not** entries. |
| D5 | Content payload model | **Unified inline content.** Both kinds carry inline `body` + `format` (`markdown` for prompt, `javascript` for workflow). prompt → copy/insert; workflow → save `.js` + run via the Workflow runner. No fetch/pointer in v1 (additive later). |

## §1 Architecture: two parallel contracts, one unified site

```
            ┌─ public/catalog.json          (install contract, schema_version=1, UNCHANGED)
Aleph Hub ──┤    skill | plugin | mcp   ──install_spec──▶ Aleph hub catalog client (install)
 (this repo)│
            └─ public/catalog-content.json  (content contract, content_schema_version=1, NEW)
                 prompt | workflow      ──inline body──▶ Aleph content library (copy / run)
```

- **Physical separation** at the contract/artifact layer (no `catalog.json` edits).
- **Logical unification** at the website layer: site data is ours, not a contract,
  so the browse UI merges both into one 5-kind experience.
- Each artifact has its own `manifest` (own `content_hash`, own version line) and its
  own skip-emit-on-unchanged path.

## §2 Content contract — `public/catalog-content.json`

```jsonc
{
  "manifest": {
    "content_schema_version": 1,        // independent of install schema_version
    "hub_id": "aleph-hub",
    "name": "Aleph Hub",
    "generated_at": "2026-06-22T00:00:00Z",
    "entry_count": 123,
    "content_hash": "sha256:…"          // client "unchanged → skip"
  },
  "entries": [{
    "id": "aleph-hub:<owner>/<repo>#<slug>",  // '#slug' makes collection units stable & unique
    "kind": "prompt",                   // "prompt" | "workflow"
    "category": "writing",              // REUSE existing ExtensionCategory enum (no new categories)
    "name": "…",
    "description": "…",
    "author": "<owner>",
    "tags": ["…"],
    "repo_url": "https://github.com/<owner>/<repo>",  // P-Provenance 铁律 — real upstream
    "source_path": "prompts/foo.md",    // file within repo (provenance / verifiability)
    "trust_tier": "community",          // official | verified | community | unverified
    "license": "MIT",                   // surfaced: copy-reuse needs license clarity
    "via": "github:<owner>",
    "body": "…full prompt markdown OR the single .js workflow source…",  // inline payload
    "format": "markdown"                // "markdown" (prompt) | "javascript" (workflow)
  }]
}
```

### Payload: unified inline `body` + `format` (D5)

Both kinds carry the content **inline** — the payload *is* the file's text. This is
mirror-safe / offline / zero-fetch and keeps prompt and workflow isomorphic:

- **prompt** → `body` = the prompt text, `format: "markdown"`. Action: copy / insert.
- **workflow** → `body` = the **single `.js` file's source**, `format: "javascript"`.
  Action: save as `<name>.js`, run via `Workflow({scriptPath})`.

Rules:
- `body` capped (default **64 KB** — comfortably holds a workflow script; prompts are
  far smaller); over-cap → drop. The cap bounds artifact size and the injection-scan
  surface.
- A workflow entry is exactly **one** `.js` file. `results-*.json` run outputs are
  **never** ingested. A companion `README.md` is read by the curator to write the
  description but is not part of `body`.
- No `content_spec`/pointer in v1. A future fetch/multi-file payload is an additive
  change (optional `content_spec`), not a rewrite.

### Zod (producer side) — `contract/content-schema.ts` (new, sibling of `schema.ts`)

New `ContentKind = z.enum(["prompt","workflow"])`; `ContentFormat = z.enum(["markdown","javascript"])`;
`ContentCatalogEntry` (with `body`, `format`), `ContentCatalogManifest`,
`ContentCatalogArtifact`, `validateContentArtifact()`. `ExtensionCategory` /
`TrustTier` are imported and reused.

## §3 Curation data model

- **Install kinds**: `data/curation/<owner>__<repo>.json` — **unchanged** (repo-grained).
- **Content kinds**: `data/curation-content/<owner>__<repo>__<slug>.json` — **one file
  per emitted entry** (per-slug). A 100-prompt awesome-list yields up to 100 records;
  each workflow script yields one record.
  Record fields: `id`/`slug`, `kind`, `category`, `name`, `description_en/zh`,
  `tags`, `sec_note_en/zh`, plus the payload `body` + `format`.
- **Explosion step**: a content source yields `(repo, file[, unit])` candidates; the
  curator extracts individual prompts from a collection file/dir into per-slug records.
  The per-run LLM cap (`LLM_CURATE_PER_RUN`) drains large collections over several
  prompt-days — which is exactly why prompt is weighted ×2 in D3.
- Like today, an LLM-authored record is persisted (`curated_by:"llm"`) as a
  human-auditable review buffer; a content record only re-emits when its source repo
  is rediscovered by that kind's day.

## §4 Pipeline changes (`scripts/pipeline/`)

1. **Content sources** (`sources/github-content.ts`): topic queries + awesome-list
   seeds → repo → file(s) → per-unit candidate. Seeds live in
   `data/seeds/content.json` (`{ prompt: {queries,seeds,pins}, workflow: {…} }`).
   - prompt (default, tunable): `topic:awesome-prompts`, `topic:prompt-engineering`,
     `topic:claude-prompts`, `topic:chatgpt-prompts` + curated awesome-list seeds.
   - workflow: `topic:claude-code-workflow`, `topic:agent-workflow` + repos with a
     `*-workflow` dir. Detection signature: file contains `export const meta` **and**
     workflow hooks (`agent(` / `pipeline(` / `phase(`); README mentions
     `Workflow` / `scriptPath`.
2. **`curate-content.ts`** (parallel to `curate.ts`): **skips** `install_spec`
   inference/verify entirely. Steps: extract unit → clean → bilingual
   description → categorize (reuse categories) → **safety-scan**, then set
   `body` + `format` (prompt = cleaned prompt text / `markdown`; workflow = the
   single `.js` source / `javascript`). Enforce the 64 KB `body` cap.
3. **Safety (`safety.ts` extension)**: scan target widens from `name+description`
   to **`name + description + body`** for prompts, and to the **subagent-prompt text
   inside the script** for workflows. Hard-exclude additions for content kinds:
   jailbreak / safety-bypass / prompt-injection attack payloads; "evade AI detector /
   strip AI fingerprints to pass as human-written" (already implied by the AI-writing
   ruling). These join the existing NSFW / grey-hat exclusions.
4. **`emit-content.ts`**: build `catalog-content.json` + `data/site-content.json`,
   own `content_hash`, skip-emit when unchanged (independent of the install artifact).
5. **Kind-targeted run** (`run.ts` + `index.ts`): a `--kind=<k>` arg (and
   `TARGET_KIND` env) selects **only that kind's** sources to fetch and **only that
   kind's** backlog to LLM-curate. Non-target kinds are carried over from cache;
   **both artifacts are still emitted in full every run** (only the target kind is
   deeply re-crawled that day). No `--kind` ⇒ legacy all-kinds behavior (kept for
   manual full runs / tests).
6. **Kind-aware guards**: `perSourceGuard` and the drop-guards (`config.ts`
   `PER_SOURCE_DROP_PCT`, `MAX_DROP_PCT`) only evaluate **sources active this run**.
   A kind not crawled today is "carried from cache", not "dropped" — otherwise a
   partitioned day would always false-trip the guard.

## §5 Weekly cron partition (`.github/workflows/pipeline.yml`)

- Keep a **single** daily cron. Add a step computing the day's kind:
  `TARGET_KIND=$(node scripts/pipeline/target-kind.mjs $(date -u +%u))` then
  `npm run pipeline -- --kind=$TARGET_KIND`.
- `workflow_dispatch` gains an optional `kind` input (default: auto-by-weekday) for
  manual targeting / backfill.
- Mapping table (single source of truth, `scripts/pipeline/target-kind.mjs`):

  | ISO day (`%u`) | 1 | 2 | 3 | 4 | 5 | 6 | 7 |
  |---|---|---|---|---|---|---|---|
  | kind | skill | skill | plugin | mcp | prompt | prompt | workflow |

- The commit/push step also stages `public/catalog-content.json`,
  `data/site-content.json`, `data/curation-content/`, and content cache files.

## §6 Website (unified 5-kind browse)

- **Home index**: 3 regions → **5** (add Prompts, Workflows). Extend
  `components/home/CategoryIndex.tsx` and the home composition.
- **Routes**: `app/c/[kind]/page.tsx` `KINDS` gains `prompt`/`workflow`; those read
  the content catalog.
- **Detail** (`app/e/[...slug]` / `components/detail/DetailView.tsx`): kind-aware
  action.
  - prompt → render `body` (markdown) + **Copy** button + an "insert in Aleph"
    deep-link placeholder.
  - workflow → render `body` (syntax-highlighted `.js`) + **Copy** button + a
    "save & run in Aleph" affordance; show `repo_url`/`source_path` as provenance.
  - install kinds → unchanged (install command).
- **Data** (`lib/catalog.ts`): load `data/site-catalog.json` **and**
  `data/site-content.json`, merge for browse; keep kind on each item so cards/detail
  branch correctly. `components/Card.tsx` reused, action slot is kind-aware.
- **i18n** (`lib/i18n.ts`): add `prompt` / `workflow` labels and copy/insert strings
  (zh + en).

## §7 Aleph-side consumer (out of this repo — contract handoff)

A new, lightweight **content library** feature in `../Aleph` (separate spec):
fetch `catalog-content.json`, validate `content_schema_version`, list entries, and
act on the inline `body` by `kind`:
- **prompt** → copy `body` to clipboard / insert into the input box.
- **workflow** → write `body` to `<name>.js` and run via `Workflow({scriptPath})`.

It does **not** touch the marketplace, the skills dir, or the existing hub catalog
client — so the current install flow is unaffected and the content feature can ship
independently. This spec freezes the contract above for it to implement against;
when `content_schema_version` changes, both sides sync (same rule as
`schema_version`).

## §8 Scope · phasing · non-goals

- **Phase 1**: content contract + Zod + **prompt** discovery/curation + content emit +
  website content browse/copy.
- **Phase 2**: **workflow** kind (`body`/`javascript`, script detection) +
  weekly cron partition + kind-aware guards.
- Content kinds may ship under the existing daily cron first; the partition refactor
  lands in Phase 2.
- **Aleph consumer**: parallel, separate spec in `../Aleph`.
- **Non-goals (YAGNI)**: no dual-granularity "whole collection" entries (D2 = single);
  no new `ExtensionCategory`; no executable-workflow runner in the Hub; no prompt
  version diffing beyond `content_hash`; the Aleph "insert into input" UI belongs to
  the Aleph spec; artifact signing remains the existing fast-follow.

## §9 Testing

Mirror the existing `contract/__tests__` + `scripts/pipeline` test style:
- `content-schema` Zod round-trip + reject cases (bad `format`, over-cap `body`,
  missing `repo_url`).
- `curate-content`: prompt extraction → `body`/`markdown`; workflow detection →
  `body`/`javascript`; `results-*.json` excluded; over-cap body dropped; safety drop
  on injection/jailbreak body.
- collection **explosion** → multiple per-slug records.
- `target-kind` mapping (1–7 → kind) + `--kind` source/backlog selection.
- kind-aware guard: a non-target kind carried from cache does **not** trip the drop
  guard.
- `emit-content` skip-on-unchanged (independent `content_hash`).
- e2e: 5-kind home index renders; content detail shows Copy and copies `body`.

## §10 Contract-sync checklist (must stay in lockstep with `../Aleph`)

- `content_schema_version` semantics === `schema_version` semantics (reject `>`
  supported).
- `ContentKind` (`prompt`/`workflow`) and `ContentFormat` (`markdown`/`javascript`)
  value spaces, and the inline `body`+`format` entry shape, must match the Aleph
  content-library deserializer.
- `category` / `trust_tier` value spaces remain the shared enums.
- Any field add/remove to a content entry → confirm Aleph parser sync first
  (same rule as the install contract).
