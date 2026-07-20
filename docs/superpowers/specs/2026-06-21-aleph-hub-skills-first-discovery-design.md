# Skills-First Discovery Pivot — Design

**Date:** 2026-06-21
**Status:** Approved (design)
**Scope:** `scripts/pipeline/` discovery sources + seeds. No contract change.

## Problem

The deterministic pipeline's discovery is MCP-heavy and undifferentiated by
source quality. The current `data/seeds/github.json` runs 3 MCP-oriented queries
(`topic:mcp`, `topic:model-context-protocol`, `mcp-server in:name,description`)
plus one skill query, and scrapes two MCP awesome-lists
(`modelcontextprotocol/servers`, `punkpeye/awesome-mcp-servers`). The last real
run discovered github=1129, hermesatlas=173, clawhub=19 — a github firehose that
demands heavy filtering. `CONFIG.SOURCE_PRIORITY = ["github", "clawhub", "hermes-atlas"]`
ranks the raw github source above the pre-curated directories.

The goal is to refocus discovery on **skills**, and to **lean on pre-curated
sources** (which have already done quality filtering) so the agent's curation
burden drops.

## Source assessment (investigated 2026-06-21)

- **HermesAtlas (hermesatlas.com)** — community directory for Hermes Agent
  (Nous Research). 100+ open-source tools; every listing links a GitHub repo with
  star counts. **Fits our GitHub-provenance + `git_dir` install model perfectly.**
  Homepage scrape already yields 173 pre-vetted candidates. → **primary source.**
- **ClawHub (clawhub.ai)** — a package registry/marketplace for OpenClaw agents.
  Its `/api/v1/skills` exposes `slug`/`displayName`/`summary`/`topics`/`stats`/
  `latestVersion` but **no GitHub repository URL**; skills install via the ClawHub
  registry, not git-clone. This **conflicts with the P-Provenance mandate**
  (every entry needs a real upstream `repo_url`) and the `git_dir` install model.
  Most ClawHub candidates cannot be ingested. → **parked: kept wired at lowest
  priority, no contract change, near-zero usable yield expected.**
- **GitHub topic search** — precise skill topics have strong populations:
  `agent-skills` 7316, `claude-skills` 4405, `claude-skill` 2836,
  `claude-code-skill` 1618. High-star pre-curated skill awesome-lists exist:
  `ComposioHQ/awesome-claude-skills` (⭐65k), `travisvn/awesome-claude-skills`
  (⭐13.6k). → **narrowed to a skill-focused net.**

## Locked decisions

- **D1 — Kind scope: skills-first, keep others.** Skills are the emphasis and the
  bulk of new entries. The existing 3 MCP entries stay; MCP/plugins remain valid
  when notable. Non-skill kinds are no longer actively fished from github, but
  HermesAtlas still surfaces them and nothing is deleted.
- **D2 — HermesAtlas is the primary pre-curated source.** Highest dedup priority.
  Its scraper is unchanged (homepage yields 173, already good coverage).
- **D3 — GitHub narrowed to skills.** Replace the MCP queries/lists with skill
  topics + skill awesome-lists. It is the broad net for skills not in Hermes.
- **D4 — ClawHub parked, not removed.** Kept in the active source list at lowest
  priority so the per-source collapse guard (`run.ts` `perSourceGuard`, which
  throws when a known source drops >50% vs last run) is not tripped. Its
  provenance-failing candidates are excluded by existing behavior.
- **D5 — No contract change, no scraper rewrite.** `contract/schema.ts`, the
  install_spec inference/verification, and the Hermes/ClawHub scraper code are
  untouched. The change is data (seeds) + one config line + test updates.

## Changes

### 1. `data/seeds/github.json` (data)

```json
{
  "queries": ["topic:claude-skills", "topic:claude-skill", "topic:agent-skills", "topic:claude-code-skill"],
  "seeds": [
    "https://github.com/ComposioHQ/awesome-claude-skills",
    "https://github.com/travisvn/awesome-claude-skills"
  ]
}
```

All four topics are validated as populated (>1000 repos each). Both seed
awesome-lists are real, high-star, skill-curated repos. The awesome-lists are
scraped the same way the MCP lists were (`http.getText` on the repo page →
`extractGitHubLinks`), so no scraper change is needed.

### 2. `scripts/pipeline/config.ts` (one line)

```ts
SOURCE_PRIORITY: ["hermes-atlas", "clawhub", "github"] as const,
```

Pre-curated sources (Hermes, then ClawHub) rank above the raw github net. This
governs dedup tie-breaking: when the same repo is discovered by multiple sources,
the higher-priority source's `via` badge wins.

### 3. Tests

Exactly two assertions depend on the old `SOURCE_PRIORITY` order and must change
(the seed-data change affects no test — every source test uses inline fixture
seeds, not the real `data/seeds/github.json`):

- `scripts/pipeline/__tests__/config.test.ts:14` — `expect(CONFIG.SOURCE_PRIORITY[0]).toBe("github")`
  → `.toBe("hermes-atlas")`.
- `scripts/pipeline/__tests__/dedup.test.ts` "keeps the higher-priority source on
  a tie" — currently pairs `via: "clawhub"` vs `via: "github:acme"` and expects
  `github:acme` to win. Under the new order ClawHub (rank 1) outranks github
  (rank 2), so the expected winner flips. Update the test to validate the new
  design: pair `via: "hermes-atlas"` vs `via: "github:acme"` and expect
  `hermes-atlas` to win (this asserts D2, the primary source).

Run the full suite to confirm nothing else regresses.

## Trade-offs & expectations

- **Volume vs. burden.** Four skill topics (each paginating to ~300) plus the
  awesome-lists can still produce a large candidate pool, but it is now
  skill-homogeneous and carries star counts. A large queue is acceptable: the
  agent curates the top entries by stars/activity. The awesome-list portion is
  pre-curated (highest signal).
- **MCP coverage shrinks deliberately.** New MCP discovery now comes only from
  HermesAtlas, not github. The existing 3 MCP entries persist (cached, curated).
  This is the intended effect of skills-first.
- **ClawHub yield stays near-zero.** Expected and accepted; ClawHub's data model
  is incompatible with the GitHub-provenance contract, and forcing it in would
  require an Aleph-side contract change (out of scope).

## Out of scope

A new install_spec type for ClawHub-registry skills, a Hermes skills-list-specific
scraper, search-result sorting by stars, and any change to the catalog contract
schema. These are possible follow-ups, not part of this pivot.
