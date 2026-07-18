# Pipeline API-Call Reduction — Design

**Date:** 2026-06-21
**Status:** Approved (design)
**Scope:** `scripts/pipeline/` — the deterministic cron pipeline's GitHub fetch layer.

## Problem

A single `npm run pipeline` run issues ~2800 GitHub **core** API calls against the
5000/hr ceiling, with almost no headroom. Two manual `pipeline` workflow runs on
2026-06-20 failed: the first because a concurrently-running local `gh` session
shared the same token's quota and starved the run (→ 0 curated → floor gate
`0 < MIN_ENTRIES 8`); the clean re-run reached the deploy step but consumed 3784
calls and ran 15+ minutes. The fragility is structural, not transient.

### Root cause (two compounding defects)

1. **Double fetch.** `dedup.ts:20` calls `gh.getRepo(fn)` for every **raw**
   candidate (~1500) to resolve fork→canonical, then discards the meta.
   `run.ts:48` calls `gh.getRepo(cand.full_name)` again for every **deduped**
   candidate (~1295). The same repos are fetched twice.
2. **Conditional requests never implemented.** The `GitHubApi.getRepo(fullName, etag?)`
   port (`ports.ts:19`) specifies `{ meta, etag, notModified }` and the README-reuse
   path in `run.ts:56` already consumes `notModified`. But the adapter
   (`adapters.ts:31`) ignores the `etag` argument and hard-codes `notModified: false`,
   so every call is a full 200 GET counted against the rate limit, and the
   cache layer is dead. Only the ~13 curated repos are cached at all (the
   `RepoCache` shape requires a `CuratedEntry`).

## Goal

Each repo is fetched **at most once per run**, and unchanged repos use HTTP
conditional requests so they cost no rate-limit budget. Cold-cache run:
~2800 → **~1500** core calls (guaranteed, from de-dup). Warm-cache run:
best-effort fewer (304s for low-activity repos). Comfortable headroom under
5000/hr; the nightly cron stops being rate-limit-bound.

## Locked decisions

- **D1 — Caching wrapper, not fat adapter.** Caching/memoization/304-reconciliation
  logic lives in a NEW tested module `gh-cache.ts`, not in the untested adapter
  boundary. This matches the codebase rule "business logic in tested stage
  modules; adapters stay thin."
- **D2 — Pipeline-facing port unchanged.** `GitHubApi` (consumed by `run.ts`/
  `dedup.ts`) keeps returning non-null `meta`. A new internal `RawGitHubApi`
  type lets the thin adapter return `meta: null` on 304; the wrapper fills meta
  from cache and presents the unchanged `GitHubApi` to the pipeline.
- **D3 — Separate persistent etag store.** `data/cache/repos-meta.json` =
  `Record<lowercaseFullName, { etag, meta }>`, distinct from the curation cache
  `data/cache/repos.json`. Single responsibility per file.
- **D4 — Always write the etag store; let git gate the commit.** `index.ts`
  writes `repos-meta.json` every run (alongside heartbeat/queue). The workflow's
  existing `git diff --cached --quiet` check suppresses empty commits when no
  etags changed.

## Components

### 1. Thin adapter — `adapters.ts` (untested boundary)

`makeGitHub(): RawGitHubApi`. `getRepo(fullName, etag?)`:
- send `If-None-Match: <etag>` when `etag` is provided
- **304** → `{ meta: null, etag, notModified: true }` (no body; not rate-limited)
- **200** → `{ meta, etag: <response etag>, notModified: false }`
- network error / 404 → `null`

`searchRepos`, `getReadme`, `getContent` are unchanged.

```ts
type RawRepoResult =
  | { meta: RepoMeta; etag: string; notModified: false }
  | { meta: null;     etag: string; notModified: true };
interface RawGitHubApi {
  searchRepos(query: string, opts?): Promise<string[]>;
  getRepo(fullName: string, etag?: string): Promise<RawRepoResult | null>;
  getReadme(fullName: string): Promise<string | null>;
  getContent(fullName: string, path: string): Promise<string | null>;
}
```

### 2. Caching wrapper — `gh-cache.ts` (NEW, tested)

`makeCachingGitHub(inner: RawGitHubApi, metaCache: Map<string, { etag: string; meta: RepoMeta }>): GitHubApi`.

`getRepo(fullName)`:
1. per-run **memo** (ephemeral `Map<key, result>`) hit → return it (**the A win**:
   each repo fetched ≤1×/run, even on a cold cache).
2. `etag = metaCache.get(key)?.etag`; `r = await inner.getRepo(fullName, etag)`.
3. `r === null` → memoize `null`, return `null`.
4. `r.notModified` → `meta = metaCache.get(key)!.meta`; result
   `{ meta, etag: r.etag, notModified: true }`.
5. `200` → `metaCache.set(key, { etag: r.etag, meta: r.meta })`; result
   `{ meta: r.meta, etag: r.etag, notModified: false }`.
6. memoize the result under **both** the request key and `meta.full_name`
   (both lower-cased) so a renamed/aliased repo fetched in dedup is reused by
   the main loop.

`searchRepos` / `getReadme` / `getContent` pass through to `inner`.

`key = fullName.toLowerCase()` throughout.

### 3. Persistent etag store — `data/cache/repos-meta.json`

`Record<lowercaseFullName, { etag: string; meta: RepoMeta }>`. Wired in
`index.ts`:
- load file → `Map`
- `const gh = makeCachingGitHub(makeGitHub(), map)`
- after `run()`, write `Object.fromEntries(map)` to `data/cache/repos-meta.json`
  in the **always-write** section.

## Data flow

`dedup` calls `getRepo(rawName)` → 200/304 → folds fork→canonical via cached
`meta.fork`/`meta.source_full_name`. The main loop calls `getRepo(canonical)`:
- non-fork (the common case): dedup already fetched this exact repo →
  **memo hit**, zero HTTP.
- folded fork (rare): a real fetch of the source repo (a 304 once its etag is warm).

With the etag store warm, the dormant `run.ts:56` `notModified` branch activates:
a curated repo that 304s reuses its cached `CuratedEntry` and skips the README
fetch + re-curation. `got.meta` on a 304 is the cached meta, so `trustTier`/
`enrich` still receive a valid `RepoMeta`.

**Soundness of README-reuse on 304:** a README change requires a commit (push),
which changes `pushed_at`, which changes the `/repos` response body, which
changes the etag → not a 304. So a 304 on `/repos` implies no push implies the
README is unchanged. Reusing the cached entry is safe.

## What does NOT change

- `GitHubApi` (the pipeline-facing port), `contract/schema.ts`, the site catalog,
  the curation cache `data/cache/repos.json`.
- `dedup.ts` logic. `run.ts` logic — one trivial cleanup only: drop the now-redundant
  `cached?.etag` argument at `run.ts:48` (the wrapper self-manages etags).
- Existing `run`/`dedup`/component tests inject fakes directly into the stage
  functions, which still consume the unchanged `GitHubApi`, so they are unaffected.

## Testing (TDD)

- `gh-cache.test.ts` (NEW) against a **fake `RawGitHubApi`** (no global-fetch
  mocking): (a) memo — two `getRepo` of the same name → inner called once;
  (b) 304 → returns cached meta with `notModified: true`; (c) 200 → updates the
  metaCache; (d) canonical-alias — request `Old/Name`, inner returns
  `meta.full_name = new/name`; subsequent `getRepo("new/name")` is a memo hit;
  (e) `null` passthrough memoized.
- Adapter conditional-request test (NEW) stubbing `globalThis.fetch`: etag present
  → request carries `If-None-Match`; a 304 response → `{ meta: null, notModified: true }`;
  a 200 response → meta populated + etag captured. Guards against re-introducing
  the exact silent bug being fixed.
- Full existing suite stays green.

## Expectation honesty

Cold run ~2800 → ~1500 is guaranteed by de-dup. Warm-run 304 yield depends on
GitHub's etag semantics: push-changed repos always re-fetch (200); low-activity
repos return 304. Realistic steady state is a meaningful reduction (hundreds to
~1000), not a guaranteed near-zero. Either way the run clears the 5000/hr
ceiling with comfortable headroom, which is the objective.

## Out of scope

Concurrency/parallel fetch (wall-clock, not rate-limit), discovery-breadth caps,
floor-gate-before-queue-write ordering, and the missing-`full_name` curation test
remain separate follow-ups.
