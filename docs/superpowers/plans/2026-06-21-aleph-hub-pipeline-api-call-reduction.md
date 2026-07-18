# Pipeline API-Call Reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the deterministic pipeline fetch each repo at most once per run and use HTTP conditional requests, cutting per-run GitHub core API calls from ~2800 to ~1500 (cold) and fewer (warm), so the nightly cron stops being rate-limit-bound.

**Architecture:** A new tested decorator `gh-cache.ts` wraps the thin GitHub adapter. The thin adapter (`makeGitHub`) is changed to honor `If-None-Match` and report `notModified` on 304, returning `meta: null` on 304 (new `RawGitHubApi` type). The decorator memoizes per run, holds a persistent etag cache (`data/cache/repos-meta.json`), and fills `meta` from cache on 304 — so the pipeline-facing `GitHubApi` is unchanged and `run.ts`/`dedup.ts` are untouched (one trivial arg cleanup).

**Tech Stack:** TypeScript, tsx, Vitest (jsdom, `globals: true`), Node 24, `@/*` path alias → repo root.

## Global Constraints

- Do NOT change `GitHubApi` (the pipeline-facing port) or `contract/schema.ts` (the external contract).
- Adapters stay thin: caching/memoization/304 logic lives in the tested `gh-cache.ts`, not in `adapters.ts`.
- All external-boundary errors degrade to `null` (existing adapter philosophy) — never throw out of an adapter.
- The etag cache is persisted on every run (always-write); the workflow's existing `git diff --cached --quiet` gate suppresses empty commits.
- No new dependencies. No `console.log` (the pipeline's single `console.log(report)` in `index.ts` is the existing, intended status line — leave it).
- After every task the tree must compile (`npm run typecheck`) and the full suite (`npx vitest run`) must be green.

---

### Task 1: Add `RawGitHubApi` type and the caching decorator

**Files:**
- Modify: `scripts/pipeline/ports.ts` (add `RawRepoResult` + `RawGitHubApi` after the `GitHubApi` interface, ~line 22)
- Create: `scripts/pipeline/gh-cache.ts`
- Test: `scripts/pipeline/__tests__/gh-cache.test.ts`

**Interfaces:**
- Consumes: `RepoMeta`, `GitHubApi` from `ports.ts` (existing).
- Produces:
  - `RawRepoResult = { meta: RepoMeta; etag: string; notModified: false } | { meta: null; etag: string; notModified: true }`
  - `interface RawGitHubApi` — same as `GitHubApi` except `getRepo(fullName, etag?): Promise<RawRepoResult | null>`.
  - `makeCachingGitHub(inner: RawGitHubApi, metaCache: Map<string, MetaCacheEntry>): GitHubApi`
  - `type MetaCacheEntry = { etag: string; meta: RepoMeta }`

This task only ADDS code (a type, a new module, a test). `makeGitHub` still returns `GitHubApi`, nothing else changes, so the tree stays green. The decorator is tested in isolation against a fake `RawGitHubApi`.

- [ ] **Step 1: Add the `RawGitHubApi` types to `ports.ts`**

Insert immediately after the `GitHubApi` interface (after the closing `}` near line 22):

```ts
// The thin HTTP adapter honors conditional requests: a 304 carries no body, so
// `meta` is null on notModified. The gh-cache decorator reconciles this back to a
// full RepoMeta (from cache) and presents the unchanged GitHubApi to the pipeline.
export type RawRepoResult =
  | { meta: RepoMeta; etag: string; notModified: false }
  | { meta: null; etag: string; notModified: true };
export interface RawGitHubApi {
  searchRepos(query: string, opts?: { perPage?: number; maxPages?: number }): Promise<string[]>;
  getRepo(fullName: string, etag?: string): Promise<RawRepoResult | null>;
  getReadme(fullName: string): Promise<string | null>;
  getContent(fullName: string, path: string): Promise<string | null>;
}
```

- [ ] **Step 2: Write the failing test** — `scripts/pipeline/__tests__/gh-cache.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { makeCachingGitHub, type MetaCacheEntry } from "@/scripts/pipeline/gh-cache";
import type { RawGitHubApi, RawRepoResult, RepoMeta } from "@/scripts/pipeline/ports";

function meta(fn: string, over: Partial<RepoMeta> = {}): RepoMeta {
  const [owner, repo] = fn.split("/");
  return { full_name: fn, owner, repo, stars: 1, license: "MIT", pushed_at: "2026-01-01T00:00:00Z", fork: false, source_full_name: null, default_branch: "main", ...over };
}
function fakeRaw(results: Record<string, RawRepoResult | null>) {
  const calls: { fullName: string; etag?: string }[] = [];
  const inner: RawGitHubApi = {
    searchRepos: async () => [],
    getReadme: async () => "",
    getContent: async () => null,
    getRepo: async (fullName, etag) => { calls.push({ fullName, etag }); return results[fullName.toLowerCase()] ?? null; },
  };
  return { inner, calls };
}

describe("makeCachingGitHub", () => {
  it("fetches each repo at most once per run (memo)", async () => {
    const { inner, calls } = fakeRaw({ "acme/foo": { meta: meta("acme/foo"), etag: "e1", notModified: false } });
    const gh = makeCachingGitHub(inner, new Map());
    await gh.getRepo("acme/foo");
    await gh.getRepo("acme/foo");
    expect(calls).toHaveLength(1);
  });

  it("sends the cached etag and reuses cached meta on a 304", async () => {
    const cache = new Map<string, MetaCacheEntry>([["acme/foo", { etag: "e1", meta: meta("acme/foo", { stars: 42 }) }]]);
    const { inner, calls } = fakeRaw({ "acme/foo": { meta: null, etag: "e1", notModified: true } });
    const gh = makeCachingGitHub(inner, cache);
    const got = await gh.getRepo("acme/foo");
    expect(calls[0].etag).toBe("e1");
    expect(got?.notModified).toBe(true);
    expect(got?.meta.stars).toBe(42);
  });

  it("updates the etag cache on a 200", async () => {
    const cache = new Map<string, MetaCacheEntry>();
    const { inner } = fakeRaw({ "acme/foo": { meta: meta("acme/foo"), etag: "e9", notModified: false } });
    const gh = makeCachingGitHub(inner, cache);
    await gh.getRepo("acme/foo");
    expect(cache.get("acme/foo")?.etag).toBe("e9");
  });

  it("memoizes under the canonical name so a renamed repo is reused", async () => {
    const { inner, calls } = fakeRaw({ "old/name": { meta: meta("new/name"), etag: "e1", notModified: false } });
    const gh = makeCachingGitHub(inner, new Map());
    await gh.getRepo("Old/Name");
    await gh.getRepo("new/name");
    expect(calls).toHaveLength(1);
  });

  it("passes through and memoizes a null (unresolved) repo", async () => {
    const { inner, calls } = fakeRaw({});
    const gh = makeCachingGitHub(inner, new Map());
    expect(await gh.getRepo("gone/x")).toBeNull();
    await gh.getRepo("gone/x");
    expect(calls).toHaveLength(1);
  });
});
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `npx vitest run scripts/pipeline/__tests__/gh-cache.test.ts`
Expected: FAIL — `Failed to resolve import "@/scripts/pipeline/gh-cache"` (module does not exist yet).

- [ ] **Step 4: Create `scripts/pipeline/gh-cache.ts`**

```ts
// Tested decorator over the thin GitHub adapter: per-run memoization + a persistent
// etag cache + 304 reconciliation. The thin adapter (RawGitHubApi) returns meta:null
// on a 304; this wrapper fills meta from the etag cache so the pipeline-facing
// GitHubApi always yields a RepoMeta. Each repo is fetched at most once per run.
import type { GitHubApi, RawGitHubApi, RepoMeta } from "@/scripts/pipeline/ports";

export type MetaCacheEntry = { etag: string; meta: RepoMeta };
type RepoResult = { meta: RepoMeta; etag: string; notModified: boolean };

export function makeCachingGitHub(inner: RawGitHubApi, metaCache: Map<string, MetaCacheEntry>): GitHubApi {
  const memo = new Map<string, RepoResult | null>(); // per-run, ephemeral
  return {
    searchRepos: (q, o) => inner.searchRepos(q, o),
    getReadme: (fn) => inner.getReadme(fn),
    getContent: (fn, p) => inner.getContent(fn, p),
    async getRepo(fullName) {
      const key = fullName.toLowerCase();
      if (memo.has(key)) return memo.get(key)!;
      const cached = metaCache.get(key);
      const r = await inner.getRepo(fullName, cached?.etag);
      if (r === null) { memo.set(key, null); return null; }
      if (r.notModified) {
        // A 304 only happens after we sent a cached etag, so `cached` exists; degrade safely otherwise.
        if (!cached) { memo.set(key, null); return null; }
        const result: RepoResult = { meta: cached.meta, etag: r.etag || cached.etag, notModified: true };
        memo.set(key, result);
        memo.set(cached.meta.full_name.toLowerCase(), result);
        return result;
      }
      metaCache.set(r.meta.full_name.toLowerCase(), { etag: r.etag, meta: r.meta });
      const result: RepoResult = { meta: r.meta, etag: r.etag, notModified: false };
      memo.set(key, result);
      memo.set(r.meta.full_name.toLowerCase(), result);
      return result;
    },
  };
}
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `npx vitest run scripts/pipeline/__tests__/gh-cache.test.ts`
Expected: PASS — 5 passing.

- [ ] **Step 6: Typecheck and full suite**

Run: `npm run typecheck && npx vitest run`
Expected: typecheck clean; all tests green (no regressions — nothing else changed).

- [ ] **Step 7: Commit**

```bash
git add scripts/pipeline/ports.ts scripts/pipeline/gh-cache.ts scripts/pipeline/__tests__/gh-cache.test.ts
git commit -m "feat(pipeline): caching GitHub decorator (per-run memo + etag cache)"
```

---

### Task 2: Honor conditional requests in the adapter and wire the decorator

**Files:**
- Modify: `scripts/pipeline/adapters.ts` (`makeGitHub` return type + 304 handling + export; drop `gh` from `makeAdapters`)
- Modify: `scripts/pipeline/index.ts` (compose the decorator, load/save `repos-meta.json`)
- Modify: `scripts/pipeline/run.ts:48` (drop the now-redundant `cached?.etag` argument)
- Test: `scripts/pipeline/__tests__/adapters.test.ts` (new)

**Interfaces:**
- Consumes: `RawGitHubApi`, `RawRepoResult` (Task 1, `ports.ts`); `makeCachingGitHub`, `MetaCacheEntry` (Task 1, `gh-cache.ts`).
- Produces: `export function makeGitHub(): RawGitHubApi`. `makeAdapters()` returns `{ store, registry, http, clock, fs }` (no `gh`).

The adapter's return type changes to `RawGitHubApi`, which is NOT assignable to `GitHubApi`. So the adapter change and the `index.ts` wiring MUST land in the same commit to keep the tree compiling. `run.ts`'s arg drop is trivial and included here.

- [ ] **Step 1: Write the failing adapter test** — `scripts/pipeline/__tests__/adapters.test.ts`

```ts
import { describe, it, expect, vi, afterEach } from "vitest";
import { makeGitHub } from "@/scripts/pipeline/adapters";

// Minimal fake Response; headers.get is case-insensitive for "etag".
function res(init: { status: number; etag?: string; body?: unknown }): Response {
  return {
    status: init.status,
    ok: init.status >= 200 && init.status < 300,
    headers: { get: (h: string) => (h.toLowerCase() === "etag" ? init.etag ?? null : null) },
    json: async () => init.body,
    text: async () => "",
  } as unknown as Response;
}

afterEach(() => vi.unstubAllGlobals());

describe("makeGitHub conditional requests", () => {
  it("sends If-None-Match when an etag is supplied and maps 304 to notModified", async () => {
    const fetchMock = vi.fn(async () => res({ status: 304, etag: "e1" }));
    vi.stubGlobal("fetch", fetchMock);
    const got = await makeGitHub().getRepo("acme/foo", "e1");
    const headers = (fetchMock.mock.calls[0][1] as RequestInit).headers as Record<string, string>;
    expect(headers["If-None-Match"]).toBe("e1");
    expect(got).toEqual({ meta: null, etag: "e1", notModified: true });
  });

  it("returns meta and the response etag on a 200, with no If-None-Match when no etag", async () => {
    const body = { full_name: "acme/foo", name: "foo", owner: { login: "acme" }, stargazers_count: 5, pushed_at: "2026-01-01T00:00:00Z", default_branch: "main", fork: false };
    const fetchMock = vi.fn(async () => res({ status: 200, etag: "e2", body }));
    vi.stubGlobal("fetch", fetchMock);
    const got = await makeGitHub().getRepo("acme/foo");
    expect(got?.notModified).toBe(false);
    expect(got?.etag).toBe("e2");
    expect(got?.meta?.full_name).toBe("acme/foo");
    expect(got?.meta?.stars).toBe(5);
    const headers = (fetchMock.mock.calls[0][1] as RequestInit).headers as Record<string, string>;
    expect(headers["If-None-Match"]).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run scripts/pipeline/__tests__/adapters.test.ts`
Expected: FAIL — the 304 case returns `null` (current `!res.ok` path) instead of `{ meta: null, etag: "e1", notModified: true }`; and `makeGitHub` is not exported (import error). Either failure is acceptable RED.

- [ ] **Step 3: Update `makeGitHub` in `scripts/pipeline/adapters.ts`**

Change the import line (line 5) to add `RawGitHubApi`:

```ts
import type { GitHubApi, RepoMeta, RegistryClient, Http, Clock, FileStore, CurationStore, CurationRecord, RawGitHubApi } from "@/scripts/pipeline/ports";
```

Replace the whole `makeGitHub` function (lines 15-57) with (only `getRepo` and the signature change; `searchRepos`/`getReadme`/`getContent` bodies are copied verbatim from the current file):

```ts
export function makeGitHub(): RawGitHubApi {
  return {
    async searchRepos(query, opts) {
      const perPage = opts?.perPage ?? 100;
      const maxPages = opts?.maxPages ?? 3;
      const out: string[] = [];
      for (let page = 1; page <= maxPages; page++) {
        const res = await fetch(`${GH_API}/search/repositories?q=${encodeURIComponent(query)}&per_page=${perPage}&page=${page}`, { headers: ghHeaders() });
        if (!res.ok) break;
        const json = (await res.json()) as { items?: { full_name: string }[] };
        const items = json.items ?? [];
        out.push(...items.map((i) => i.full_name));
        if (items.length < perPage) break;
      }
      return out;
    },
    async getRepo(fullName, etag) {
      try {
        const res = await fetch(`${GH_API}/repos/${fullName}`, { headers: ghHeaders(etag ? { "If-None-Match": etag } : {}) });
        if (res.status === 304) return { meta: null, etag: res.headers.get("etag") ?? etag ?? "", notModified: true };
        if (!res.ok) return null;
        const r = (await res.json()) as Record<string, any>;
        const meta: RepoMeta = {
          full_name: r.full_name, owner: r.owner?.login ?? fullName.split("/")[0], repo: r.name ?? fullName.split("/")[1],
          stars: r.stargazers_count ?? 0, license: r.license?.spdx_id ?? null, pushed_at: r.pushed_at ?? "",
          fork: !!r.fork, source_full_name: r.source?.full_name ?? null, default_branch: r.default_branch ?? "main",
        };
        return { meta, etag: res.headers.get("etag") ?? "", notModified: false };
      } catch { return null; }
    },
    async getReadme(fullName) {
      try {
        const res = await fetch(`${GH_API}/repos/${fullName}/readme`, { headers: ghHeaders({ Accept: "application/vnd.github.raw" }) });
        return res.ok ? await res.text() : null;
      } catch { return null; }
    },
    async getContent(fullName, path) {
      try {
        const res = await fetch(`${GH_API}/repos/${fullName}/contents/${path}`, { headers: ghHeaders({ Accept: "application/vnd.github.raw" }) });
        return res.ok ? await res.text() : null;
      } catch { return null; }
    },
  };
}
```

- [ ] **Step 4: Drop `gh` from `makeAdapters` in `scripts/pipeline/adapters.ts`**

Replace the body of `makeAdapters` (lines 112-114) with:

```ts
export function makeAdapters() {
  return { store: makeCurationStore(), registry: makeRegistry(), http: makeHttp(), clock: makeClock(), fs: makeFileStore() };
}
```

- [ ] **Step 5: Wire the decorator and persist the etag cache in `scripts/pipeline/index.ts`**

Update the imports (lines 1-6) — add `makeGitHub` to the adapters import and add the gh-cache import:

```ts
import { run } from "@/scripts/pipeline/run";
import { makeAdapters, makeGitHub } from "@/scripts/pipeline/adapters";
import { makeCachingGitHub, type MetaCacheEntry } from "@/scripts/pipeline/gh-cache";
import { GitHubSource } from "@/scripts/pipeline/sources/github";
import { ClawHubSource } from "@/scripts/pipeline/sources/clawhub";
import { HermesAtlasSource } from "@/scripts/pipeline/sources/hermes";
import type { CacheStore, RepoCache, FileStore } from "@/scripts/pipeline/ports";
```

Replace the first two lines of `main()` (current lines 21-22):

```ts
  const { store, registry, http, clock, fs } = makeAdapters();
  const metaCache = new Map<string, MetaCacheEntry>(
    Object.entries(fs.readJson<Record<string, MetaCacheEntry>>("data/cache/repos-meta.json") ?? {}),
  );
  const gh = makeCachingGitHub(makeGitHub(), metaCache);
  const cache = makeFileCache(fs);
```

Add the etag-cache write to the always-write block (immediately after the existing `fs.writeJson("data/queue/to-curate.json", res.queue);` line):

```ts
  fs.writeJson("data/cache/repos-meta.json", Object.fromEntries(metaCache)); // always — persist etags for next run's 304s
```

- [ ] **Step 6: Drop the redundant etag argument in `scripts/pipeline/run.ts`**

At line 48, change:

```ts
    const got = await ports.gh.getRepo(cand.full_name, cached?.etag);
```

to:

```ts
    const got = await ports.gh.getRepo(cand.full_name);
```

(`cached` is still used below for `readme_hash`/`entry`; only the unused argument is removed — the decorator self-manages etags.)

- [ ] **Step 7: Run the adapter test to verify it passes**

Run: `npx vitest run scripts/pipeline/__tests__/adapters.test.ts`
Expected: PASS — 2 passing.

- [ ] **Step 8: Typecheck and full suite**

Run: `npm run typecheck && npx vitest run`
Expected: typecheck clean (the `RawGitHubApi` → decorator → `GitHubApi` chain composes); all tests green, including the existing `dedup.test.ts`, `run.test.ts`, and `incremental.test.ts` (they inject `GitHubApi` fakes directly into the stage functions, which are unchanged).

- [ ] **Step 9: Commit**

```bash
git add scripts/pipeline/adapters.ts scripts/pipeline/index.ts scripts/pipeline/run.ts scripts/pipeline/__tests__/adapters.test.ts
git commit -m "feat(pipeline): honor If-None-Match/304 and wire the caching decorator"
```

---

## Verification (post-implementation)

These are checks for the controller after both tasks land — not pipeline steps to run blindly (a full real run costs ~1500 GitHub core calls; do not run it while another `gh` session shares the token's quota):

1. `npm run typecheck` clean, `npx vitest run` fully green.
2. The next `pipeline` workflow run (cron at 03:17 UTC, or a manual dispatch on a fresh quota) completes without a floor-gate failure, and `gh api rate_limit` afterward shows `core.used` well under 5000 (target ~1500 on the first warm run, lower thereafter).
3. `data/cache/repos-meta.json` appears in the bot's refresh commit and grows to hold the discovered repos' etags.

## Notes on coverage vs. the spec

- Spec component 1 (thin adapter honors 304) → Task 2, Steps 1-3.
- Spec component 2 (caching wrapper memo + 304 meta-fill + cache update) → Task 1.
- Spec component 3 (persistent `repos-meta.json`, always-write) → Task 2, Steps 5.
- Spec "dormant `notModified` README-reuse activates" → no code change needed; `run.ts:56` already consumes `notModified`, which becomes `true` once the decorator + warm cache are live. Exercised in production; the existing `incremental.test.ts` covers the README-hash reuse path with fakes.
- Spec D1-D4 (locked decisions) → honored by the task structure above.
