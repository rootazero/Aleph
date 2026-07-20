# Content Workflow Discovery + Weekly Cron Partition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the content axis — autonomous prompt/workflow discovery (topic + awesome-list → repo → file-level explosion), LLM content curation, and a weekly kind-partitioned cron — so the website's workflow region fills from real upstreams and the daily crawl spreads across the week.

**Architecture:** The content pipeline (`content-index.ts → content-run.ts → catalog-content.json`) gains (1) an exploding GitHub source that turns each discovered repo into one candidate per prompt file / per workflow `.js`, (2) an Anthropic-backed content LLM curator that mirrors the install curator but authors only metadata + accept/reject (the body stays the upstream file verbatim), and (3) a `--kind` arg fed by a weekday→kind map. The cron picks install-vs-content by weekday; the **install pipeline (`run.ts`/`index.ts`/`emit.ts`/`catalog.json`) is not touched** — partition happens at pipeline-selection granularity.

**Tech Stack:** Next.js/TypeScript, Zod 4, Vitest, `@anthropic-ai/sdk` (already a dependency), GitHub REST (search + git trees + contents), Node ESM (`.mjs`) for the cron-callable kind map.

## Global Constraints

- **Two decoupled contracts.** `public/catalog.json` and the entire install pipeline (`scripts/pipeline/run.ts`, `index.ts`, `emit.ts`, `curate.ts`, `dedup.ts`, `enrich.ts`, `trust.ts`, `verify.ts`, `install_spec.ts`, `contract/schema.ts`, `contract/site.ts`, `contract/types.ts`) stay **byte-for-byte unchanged**. Verify with `git diff main -- <those paths>` = empty.
- **No schema change.** `content_schema_version` stays `1`. No field add/remove to a content entry. (If that changes, sync `../Aleph` first — out of scope here.)
- **Reuse enums.** `ContentKind = prompt|workflow`, `ContentFormat = markdown|javascript`; `category` is the existing `ExtensionCategory`; `trust_tier` is the existing `TrustTier`. No new categories/kinds.
- **Body is the file, verbatim.** `body` cap = 64 KB (`CONFIG.CONTENT_BODY_MAX = 65536`); over-cap → drop. The LLM curator **never rewrites `body`** — it authors only name/category/tags/descriptions/sec_note and the accept/reject decision. `body` = the upstream file's exact text (provenance + correctness).
- **workflow entry = one `.js` file.** Detection: text contains `export const meta` **and** one of `agent(` / `pipeline(` / `phase(`. `results-*.json` run outputs are never ingested (the `.js` filter already excludes `.json`); a `README.md` is read only as curator context, never as `body`.
- **P-Provenance (铁律).** Every entry carries a real upstream `repo_url` + `source_path`. Unresolvable upstream → drop, never obscure.
- **Curation policy (硬排除).** Reject 占卜/玄学, 成人/NSFW, 灰帽/spam 营销, 厂商锁定薄壳. Content-specific additions: jailbreak / safety-bypass / prompt-injection payloads, and "evade AI detector / strip AI fingerprints to pass as human-written" (灰帽). Security: defensive 收, attack/exploit 排除. AI-writing: readability/style 收, detection-evasion 排除. "不确定就排除."
- **Weekly partition map (single source of truth, `scripts/pipeline/target-kind.mjs`):** ISO `%u` 1→skill, 2→skill, 3→plugin, 4→mcp, 5→prompt, 6→prompt, 7→workflow.
- **Partition granularity = pipeline selection (decided).** Install pipeline runs whole on skill/plugin/mcp days; content pipeline runs with `--kind` on prompt/workflow days. `--kind` only partitions `prompt` vs `workflow` (their sources are kind-specific: `seeds.prompt` / `seeds.workflow`). No within-install kind partition, no carry-from-cache refactor, no kind-aware drop guards (the content pipeline has no drop guard by design).
- **Explosion granularity = file-level (decided).** repo → one candidate per prompt file / per workflow `.js`. Intra-file multi-prompt extraction (one CSV/README → N records) is an explicit **NON-GOAL** of this plan.
- **Slug-collision safety.** A content site-slug is `owner/repo/unit` (from id `owner/repo#unit`). `lib/site.ts buildBySlug()` throws at site build if a content slug shadows an install slug (43/125 install entries are 3-seg `owner/repo/skill`). `content-index.ts` passes the install site-slug set as `reservedSlugs`; colliding content entries are **dropped** (counted as `reservedDropped`) so the autonomous pipeline can never push an artifact that breaks the website build.
- **LLM gating.** `makeContentLlmCurator()` returns `null` without `ANTHROPIC_API_KEY` → auto-curation off; the run still emits existing human/LLM records.
- **No new dependencies.** No mutation (spread-copy). English code comments. Commit messages `<scope>: <description>` (English), **no attribution / Co-Authored-By trailer**.

---

### Task 1: Content detection & safety primitives

Pure, isolated helpers consumed by the source (Task 2), the curator (Task 3), and `content-curate.ts`. No type changes that ripple into other files.

**Files:**
- Create: `scripts/pipeline/workflow-detect.ts`
- Create: `scripts/pipeline/content-explode.ts`
- Modify: `scripts/pipeline/safety.ts`
- Test: `scripts/pipeline/__tests__/workflow-detect.test.ts`
- Test: `scripts/pipeline/__tests__/content-explode.test.ts`
- Test: `scripts/pipeline/__tests__/safety.test.ts` (extend)

**Interfaces:**
- Produces: `isWorkflowScript(text: string): boolean`; `isPromptFile(path: string): boolean`; `unitSlug(path: string): string`; extended `safeBodyOrNull` (drops AI-detection-evasion bodies).
- Consumes: nothing new.

- [ ] **Step 1: Write the failing test for workflow detection**

Create `scripts/pipeline/__tests__/workflow-detect.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { isWorkflowScript } from "@/scripts/pipeline/workflow-detect";

describe("isWorkflowScript", () => {
  it("accepts a script with meta + an orchestration hook", () => {
    expect(isWorkflowScript("export const meta = {};\nawait agent('do x')")).toBe(true);
    expect(isWorkflowScript("export const meta={}\nawait pipeline(items, s1)")).toBe(true);
    expect(isWorkflowScript("export const meta={}\nphase('Scan')")).toBe(true);
  });
  it("rejects a plain .js module with no meta", () => {
    expect(isWorkflowScript("export function add(a,b){return a+b}")).toBe(false);
  });
  it("rejects meta without any orchestration hook", () => {
    expect(isWorkflowScript("export const meta = { name: 'x' }; console.log('hi')")).toBe(false);
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `npx vitest run scripts/pipeline/__tests__/workflow-detect.test.ts`
Expected: FAIL — cannot find module `workflow-detect`.

- [ ] **Step 3: Implement workflow-detect.ts**

Create `scripts/pipeline/workflow-detect.ts`:

```ts
// Workflow detection signature (spec §4.1 / D4): a Claude Code Agent Workflow is a
// single .js file declaring `export const meta` AND calling at least one orchestration
// hook. This is a coarse classifier — the LLM curator applies the real policy gate.
const HOOKS = ["agent(", "pipeline(", "phase("];

export function isWorkflowScript(text: string): boolean {
  if (!text.includes("export const meta")) return false;
  return HOOKS.some((h) => text.includes(h));
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `npx vitest run scripts/pipeline/__tests__/workflow-detect.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Write the failing test for explosion helpers**

Create `scripts/pipeline/__tests__/content-explode.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { isPromptFile, unitSlug } from "@/scripts/pipeline/content-explode";

describe("isPromptFile", () => {
  it("accepts markdown/text prompt files", () => {
    expect(isPromptFile("prompts/summary.md")).toBe(true);
    expect(isPromptFile("a/b/Outline.mdx")).toBe(true);
    expect(isPromptFile("notes.txt")).toBe(true);
  });
  it("rejects doc files and non-text files", () => {
    expect(isPromptFile("README.md")).toBe(false);
    expect(isPromptFile("docs/CHANGELOG.md")).toBe(false);
    expect(isPromptFile("script.js")).toBe(false);
    expect(isPromptFile("Makefile")).toBe(false);
  });
});

describe("unitSlug", () => {
  it("derives a path-stable, repo-unique slug (no slashes)", () => {
    expect(unitSlug("prompts/Writing/Summary.md")).toBe("prompts-writing-summary");
    expect(unitSlug("workflows/find-flaky.js")).toBe("workflows-find-flaky");
  });
  it("collapses runs and trims separators", () => {
    expect(unitSlug("a//b__c.md")).toBe("a-b-c");
  });
});
```

- [ ] **Step 6: Run it to verify it fails**

Run: `npx vitest run scripts/pipeline/__tests__/content-explode.test.ts`
Expected: FAIL — cannot find module `content-explode`.

- [ ] **Step 7: Implement content-explode.ts**

Create `scripts/pipeline/content-explode.ts`:

```ts
// File-level collection explosion (plan decision): a repo's prompt files / workflow
// scripts each become one candidate unit. Intra-file multi-prompt extraction is a non-goal.

// Common repo docs that are not prompt units even though they are markdown.
const DOC_STEMS = new Set([
  "readme", "license", "licence", "contributing", "code_of_conduct",
  "changelog", "security", "authors", "notice", "support",
]);
const PROMPT_EXTS = [".md", ".markdown", ".mdx", ".txt", ".prompt"];

export function isPromptFile(path: string): boolean {
  const base = (path.split("/").pop() ?? path).toLowerCase();
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return false;                       // no extension (or dotfile) → skip
  if (!PROMPT_EXTS.includes(base.slice(dot))) return false;
  return !DOC_STEMS.has(base.slice(0, dot));
}

// Path-stable, repo-unique slug: strip extension, lowercase, non-alnum → '-'.
// '/' becomes '-' so the resulting content site-slug is exactly owner/repo/<unit>.
export function unitSlug(path: string): string {
  return path
    .replace(/\.[^./]+$/, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}
```

- [ ] **Step 8: Run it to verify it passes**

Run: `npx vitest run scripts/pipeline/__tests__/content-explode.test.ts`
Expected: PASS (4 tests).

- [ ] **Step 9: Extend the safety test for AI-detection-evasion**

Append to `scripts/pipeline/__tests__/safety.test.ts` inside the existing `describe("content body safety", ...)` block (add these `it` blocks before its closing `});`):

```ts
  it("drops an AI-detection-evasion body (灰帽)", () => {
    expect(safeBodyOrNull("Rewrite this to evade AI detectors and pass as human-written")).toBeNull();
    expect(safeBodyOrNull("Humanize AI text so it is undetectable by AI checkers")).toBeNull();
  });
  it("keeps a legitimate AI-writing-quality body", () => {
    expect(safeBodyOrNull("Rewrite this to remove clichés and AI-tone for better readability.")).not.toBeNull();
  });
```

- [ ] **Step 10: Run it to verify it fails**

Run: `npx vitest run scripts/pipeline/__tests__/safety.test.ts`
Expected: FAIL — the evasion body is not yet dropped (returns the string, not null).

- [ ] **Step 11: Add the EVASION net to safety.ts**

In `scripts/pipeline/safety.ts`, add the list after the `JAILBREAK` array:

```ts
// EVASION: "make AI output pass as human / defeat AI detectors" (灰帽, content kinds §4.3).
// Coarse net — the LLM curator applies the nuanced AI-writing boundary ruling.
const EVASION = [
  "evade ai detect", "bypass ai detect", "avoid ai detect", "beat ai detector",
  "pass as human-written", "humanize ai text", "remove ai fingerprint", "undetectable by ai",
];
```

Then in `safeBodyOrNull`, add the EVASION check alongside the existing JAILBREAK check:

```ts
export function safeBodyOrNull(text: string): string | null {
  const cleaned = sanitize(text);
  const lower = cleaned.toLowerCase();
  if (SUSPICIOUS.some((p) => lower.includes(p))) return null;
  if (JAILBREAK.some((p) => lower.includes(p))) return null;
  if (EVASION.some((p) => lower.includes(p))) return null;
  return cleaned;
}
```

- [ ] **Step 12: Run it to verify it passes**

Run: `npx vitest run scripts/pipeline/__tests__/safety.test.ts`
Expected: PASS (all, including the 2 new content-body cases).

- [ ] **Step 13: Commit**

```bash
git add scripts/pipeline/workflow-detect.ts scripts/pipeline/content-explode.ts scripts/pipeline/safety.ts scripts/pipeline/__tests__/workflow-detect.test.ts scripts/pipeline/__tests__/content-explode.test.ts scripts/pipeline/__tests__/safety.test.ts
git commit -m "feat(pipeline): content detection + AI-evasion safety primitives"
```

---

### Task 2: Exploding GitHub content source

Turn `GitHubContentSource` from pins-only into a kind-aware exploder: pins (file + repo) + topic queries + awesome-list scrape → per-repo file explosion. Introduces `kind`/`readme` on the candidate.

**Files:**
- Modify: `scripts/pipeline/content-model.ts` (add `kind`, `readme?` to `ContentCandidate`)
- Modify: `scripts/pipeline/sources/content-types.ts` (widen `ContentGitHub`)
- Modify: `scripts/pipeline/config.ts` (add `CONTENT_FILES_PER_REPO`)
- Modify: `scripts/pipeline/sources/github-content.ts` (rewrite)
- Modify: `scripts/pipeline/__tests__/content-source.test.ts` (rewrite)
- Modify: `scripts/pipeline/__tests__/content-run.test.ts` (add `kind` to the `cand()` factory only)

**Interfaces:**
- Consumes: `isPromptFile`, `unitSlug` (Task 1), `isWorkflowScript` (Task 1), `extractGitHubLinks` (existing `sources/types.ts`), `Http` (existing `ports.ts`), `ContentKindT` (existing `content-schema.ts`).
- Produces: `ContentCandidate` now has `kind: ContentKindT` and `readme?: string`. `GitHubContentSource` constructor `{ gh: ContentGitHub; http: Http; kind: ContentKindT; seeds: ContentKindSeeds }`. `ContentGitHub` gains `searchRepos(query): Promise<string[]>`, `getReadme(fullName): Promise<string|null>`, `listFiles(fullName): Promise<string[]>`.

- [ ] **Step 1: Add fields to the candidate model**

In `scripts/pipeline/content-model.ts`, update `ContentCandidate` (add `kind` and `readme?`; import `ContentKindT` is already imported at top):

```ts
// One discovered content unit (a single prompt/workflow file, post collection-explosion).
export interface ContentCandidate {
  repo_url: string;
  owner: string;
  repo: string;
  source_path: string;   // file path within the repo
  slug: string;          // stable per-unit slug
  kind: ContentKindT;    // the source is kind-specific; carried for the curator + queue
  via: string;
  readme?: string;       // repo README, fetched once per repo as curator context
  raw: { text: string };
}
```

- [ ] **Step 2: Widen the ContentGitHub interface**

In `scripts/pipeline/sources/content-types.ts`, replace the `ContentGitHub` interface and update the `ContentKindSeeds` comment:

```ts
// Per-kind discovery config. `pins` are "owner/repo:path" (single file, basename slug)
// or "owner/repo" (explode the whole repo). `queries` are topic searches; `seeds` are
// awesome-list URLs scraped for repo links.
export interface ContentKindSeeds {
  queries: string[];
  seeds: string[];
  pins?: string[];
}
export interface ContentSeeds {
  prompt: ContentKindSeeds;
  workflow: ContentKindSeeds;
}

// The GitHub surface the exploding content source needs (a subset of the real adapter).
export interface ContentGitHub {
  searchRepos(query: string): Promise<string[]>;          // topic discovery → full_names
  getContent(fullName: string, path: string): Promise<string | null>;
  getReadme(fullName: string): Promise<string | null>;
  listFiles(fullName: string): Promise<string[]>;          // recursive blob paths (HEAD tree)
}
```

- [ ] **Step 3: Add the per-repo file cap to config**

In `scripts/pipeline/config.ts`, add two entries inside the `CONFIG` object (after `CONTENT_BODY_MAX`):

```ts
  CONTENT_FILES_PER_REPO: 25,  // max files exploded per repo per run (bounds GitHub fan-out)
  LLM_BODY_CHARS: 16000,       // content body chars passed to the content curator (token bound)
```

- [ ] **Step 4: Write the failing source test**

Replace the entire contents of `scripts/pipeline/__tests__/content-source.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { GitHubContentSource } from "@/scripts/pipeline/sources/github-content";
import type { ContentGitHub } from "@/scripts/pipeline/sources/content-types";
import type { Http } from "@/scripts/pipeline/ports";

function fakeGh(opts: {
  search?: Record<string, string[]>;
  trees?: Record<string, string[]>;
  files?: Record<string, string>;       // "full:path" → text
  readmes?: Record<string, string>;
}): ContentGitHub {
  return {
    async searchRepos(q) { return opts.search?.[q] ?? []; },
    async listFiles(full) { return opts.trees?.[full] ?? []; },
    async getContent(full, path) { return opts.files?.[`${full}:${path}`] ?? null; },
    async getReadme(full) { return opts.readmes?.[full] ?? null; },
  };
}
const noHttp: Http = { async getText() { return null; } };

describe("GitHubContentSource (prompt explosion)", () => {
  it("explodes a discovered repo into one candidate per prompt file", async () => {
    const gh = fakeGh({
      search: { "topic:awesome-prompts": ["acme/prompts"] },
      trees: { "acme/prompts": ["README.md", "prompts/a.md", "prompts/b.md", "tool.js"] },
      files: { "acme/prompts:prompts/a.md": "Prompt A", "acme/prompts:prompts/b.md": "Prompt B" },
      readmes: { "acme/prompts": "A collection." },
    });
    const src = new GitHubContentSource({ gh, http: noHttp, kind: "prompt", seeds: { queries: ["topic:awesome-prompts"], seeds: [] } });
    const got = await src.fetch();
    expect(got.map((c) => c.slug).sort()).toEqual(["prompts-a", "prompts-b"]);
    expect(got.every((c) => c.kind === "prompt" && c.readme === "A collection.")).toBe(true);
    expect(got.find((c) => c.slug === "prompts-a")!.raw.text).toBe("Prompt A");
  });

  it("reads a file pin (owner/repo:path) with a basename slug", async () => {
    const gh = fakeGh({ files: { "acme/p:prompts/hello.md": "Hi." }, readmes: { "acme/p": "r" } });
    const src = new GitHubContentSource({ gh, http: noHttp, kind: "prompt", seeds: { queries: [], seeds: [], pins: ["acme/p:prompts/hello.md"] } });
    const got = await src.fetch();
    expect(got).toHaveLength(1);
    expect(got[0]).toMatchObject({ slug: "hello", source_path: "prompts/hello.md", kind: "prompt" });
  });
});

describe("GitHubContentSource (workflow explosion)", () => {
  it("keeps only .js files that look like workflow scripts", async () => {
    const gh = fakeGh({
      search: { "topic:agent-workflow": ["acme/wf"] },
      trees: { "acme/wf": ["flow.js", "util.js", "results-1.json"] },
      files: {
        "acme/wf:flow.js": "export const meta = {};\nawait pipeline(items, s1)",
        "acme/wf:util.js": "export function x(){}",
      },
    });
    const src = new GitHubContentSource({ gh, http: noHttp, kind: "workflow", seeds: { queries: ["topic:agent-workflow"], seeds: [] } });
    const got = await src.fetch();
    expect(got).toHaveLength(1);
    expect(got[0]).toMatchObject({ slug: "flow", kind: "workflow" });
  });

  it("caps the number of files exploded per repo", async () => {
    const paths = Array.from({ length: 40 }, (_, i) => `prompts/p${i}.md`);
    const files = Object.fromEntries(paths.map((p) => [`acme/big:${p}`, "body"]));
    const gh = fakeGh({ search: { q: ["acme/big"] }, trees: { "acme/big": paths }, files });
    const src = new GitHubContentSource({ gh, http: noHttp, kind: "prompt", seeds: { queries: ["q"], seeds: [] } });
    expect((await src.fetch()).length).toBe(25);
  });
});
```

- [ ] **Step 5: Run it to verify it fails**

Run: `npx vitest run scripts/pipeline/__tests__/content-source.test.ts`
Expected: FAIL — the source does not yet search/list/explode.

- [ ] **Step 6: Rewrite the source**

Replace the entire contents of `scripts/pipeline/sources/github-content.ts`:

```ts
import type { ContentCandidate } from "@/scripts/pipeline/content-model";
import type { ContentKindT } from "@/contract/content-schema";
import type { ContentSource, ContentKindSeeds, ContentGitHub } from "@/scripts/pipeline/sources/content-types";
import type { Http } from "@/scripts/pipeline/ports";
import { extractGitHubLinks } from "@/scripts/pipeline/sources/types";
import { isPromptFile, unitSlug } from "@/scripts/pipeline/content-explode";
import { isWorkflowScript } from "@/scripts/pipeline/workflow-detect";
import { CONFIG } from "@/scripts/pipeline/config";

// Discovers content repos (pins + topic queries + awesome-list seeds) and explodes each
// into per-file candidates (one prompt .md / one workflow .js = one unit). Kind-specific:
// constructed once per kind with that kind's seeds (seeds.prompt / seeds.workflow).
export class GitHubContentSource implements ContentSource {
  readonly id = "github-content" as const;
  constructor(private deps: { gh: ContentGitHub; http: Http; kind: ContentKindT; seeds: ContentKindSeeds }) {}

  async fetch(): Promise<ContentCandidate[]> {
    const repos = new Set<string>();
    const filePins: string[] = [];                       // "owner/repo:path" → single file
    for (const pin of this.deps.seeds.pins ?? []) {
      if (pin.includes(":")) filePins.push(pin);
      else repos.add(pin);                                // "owner/repo" → explode whole repo
    }
    for (const q of this.deps.seeds.queries) for (const fn of await this.deps.gh.searchRepos(q)) repos.add(fn);
    for (const seed of this.deps.seeds.seeds) {
      const html = await this.deps.http.getText(seed);
      if (html) for (const u of extractGitHubLinks(html)) repos.add(u.replace("https://github.com/", ""));
    }

    const out: ContentCandidate[] = [];
    for (const pin of filePins) {
      const c = await this.pinnedFile(pin);
      if (c) out.push(c);
    }
    for (const full of repos) out.push(...(await this.explode(full)));
    return out;
  }

  // A pinned single file keeps the legacy basename slug (curated, hand-pinned units).
  private async pinnedFile(pin: string): Promise<ContentCandidate | null> {
    const sep = pin.indexOf(":");
    const full = pin.slice(0, sep);
    const path = pin.slice(sep + 1);
    const [owner, repo] = full.split("/");
    if (!owner || !repo || !path) return null;
    const text = await this.deps.gh.getContent(full, path);
    if (!text) return null;
    const base = path.split("/").pop() ?? path;
    return {
      repo_url: `https://github.com/${owner}/${repo}`, owner, repo, source_path: path,
      slug: base.replace(/\.[^.]+$/, ""), kind: this.deps.kind, via: `github:${owner}`,
      readme: (await this.deps.gh.getReadme(full)) ?? "", raw: { text },
    };
  }

  // A discovered repo → one candidate per matching file (capped), path-derived slug.
  private async explode(full: string): Promise<ContentCandidate[]> {
    const [owner, repo] = full.split("/");
    if (!owner || !repo) return [];
    const paths = await this.deps.gh.listFiles(full);
    const wanted = this.deps.kind === "prompt"
      ? paths.filter(isPromptFile)
      : paths.filter((p) => p.toLowerCase().endsWith(".js"));
    const readme = (await this.deps.gh.getReadme(full)) ?? "";
    const out: ContentCandidate[] = [];
    for (const path of wanted.slice(0, CONFIG.CONTENT_FILES_PER_REPO)) {
      const text = await this.deps.gh.getContent(full, path);
      if (!text) continue;
      if (this.deps.kind === "workflow" && !isWorkflowScript(text)) continue;  // .js but not a workflow
      out.push({
        repo_url: `https://github.com/${owner}/${repo}`, owner, repo, source_path: path,
        slug: unitSlug(path), kind: this.deps.kind, via: `github:${owner}`, readme, raw: { text },
      });
    }
    return out;
  }
}
```

- [ ] **Step 7: Fix the content-run test factory**

In `scripts/pipeline/__tests__/content-run.test.ts`, add `kind: "prompt"` to the `cand` factory so `ContentCandidate` still type-checks (the only construction site outside the source):

```ts
const cand = (over: Partial<ContentCandidate> = {}): ContentCandidate => ({
  repo_url: "https://github.com/acme/prompts", owner: "acme", repo: "prompts",
  source_path: "p.md", slug: "p", kind: "prompt", via: "github:acme", raw: { text: "x" }, ...over,
});
```

- [ ] **Step 8: Run the source + run tests to verify they pass**

Run: `npx vitest run scripts/pipeline/__tests__/content-source.test.ts scripts/pipeline/__tests__/content-run.test.ts`
Expected: PASS (source 4 + run 4). Then `npm run typecheck` → clean.

- [ ] **Step 9: Commit**

```bash
git add scripts/pipeline/content-model.ts scripts/pipeline/sources/content-types.ts scripts/pipeline/config.ts scripts/pipeline/sources/github-content.ts scripts/pipeline/__tests__/content-source.test.ts scripts/pipeline/__tests__/content-run.test.ts
git commit -m "feat(pipeline): exploding github content source (topic + repo → per-file units)"
```

---

### Task 3: Content LLM curator

An Anthropic-backed `ContentLlmClient` mirroring `llm-curator.ts`: applies the content policy as a hard filter and authors bilingual metadata (never the body).

**Files:**
- Modify: `scripts/pipeline/ports.ts` (add content LLM types)
- Create: `scripts/pipeline/content-llm-curator.ts`
- Test: `scripts/pipeline/__tests__/content-llm-curator.test.ts`

**Interfaces:**
- Consumes: `ExtensionCategory` (`contract/schema.ts`), `CONFIG` (model, `LLM_README_CHARS`, `LLM_BODY_CHARS`), `Anthropic` + `zodOutputFormat` (already used by `llm-curator.ts`).
- Produces: `ContentLlmInput`, `ContentLlmProposal`, `ContentLlmResult`, `ContentLlmClient` (ports.ts); `makeContentLlmCurator(): ContentLlmClient | null` and exported pure `toContentResult(parsed): ContentLlmResult`.

- [ ] **Step 1: Add content LLM port types**

In `scripts/pipeline/ports.ts`, append after the install LLM section (after `LlmClient`):

```ts
// Content kinds autonomous curation. The LLM authors metadata + accept/reject ONLY;
// the body stays the upstream file verbatim (provenance), so there is no body field here.
export interface ContentLlmInput {
  repo_url: string;
  full_name: string;
  source_path: string;
  kind: "prompt" | "workflow";
  body: string;       // the file payload, for policy judgement (truncated by the curator)
  readme: string;     // repo README context (truncated by the curator)
}
export interface ContentLlmProposal {
  name: string;
  category: string;
  tags: string[];
  description_en: string; description_zh: string;
  long_en: string; long_zh: string;
  sec_note_en: string; sec_note_zh: string;
}
export type ContentLlmResult =
  | { decision: "accept"; proposal: ContentLlmProposal }
  | { decision: "reject"; reason: string };
export interface ContentLlmClient {
  // null = transport/parse failure (caller leaves the unit queued for a later run).
  curate(input: ContentLlmInput): Promise<ContentLlmResult | null>;
}
```

- [ ] **Step 2: Write the failing mapper test**

Create `scripts/pipeline/__tests__/content-llm-curator.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { toContentResult } from "@/scripts/pipeline/content-llm-curator";

const full = {
  decision: "accept" as const, reason: "ok", name: "Summarizer", category: "writing",
  tags: ["writing"], description_en: "e", description_zh: "z", long_en: "le", long_zh: "lz",
  sec_note_en: "se", sec_note_zh: "sz",
};

describe("toContentResult", () => {
  it("maps a complete accept into a proposal", () => {
    const r = toContentResult(full);
    expect(r.decision).toBe("accept");
    if (r.decision === "accept") expect(r.proposal.name).toBe("Summarizer");
  });
  it("treats an accept with a missing field as a reject", () => {
    const r = toContentResult({ ...full, sec_note_zh: null });
    expect(r.decision).toBe("reject");
  });
  it("passes a reject through with its reason", () => {
    const r = toContentResult({ ...full, decision: "reject", reason: "NSFW", name: null });
    expect(r).toEqual({ decision: "reject", reason: "NSFW" });
  });
});
```

- [ ] **Step 3: Run it to verify it fails**

Run: `npx vitest run scripts/pipeline/__tests__/content-llm-curator.test.ts`
Expected: FAIL — cannot find module `content-llm-curator`.

- [ ] **Step 4: Implement content-llm-curator.ts**

Create `scripts/pipeline/content-llm-curator.ts`:

```ts
// Autonomous content curator: an Anthropic-backed ContentLlmClient that applies the Aleph
// Hub content policy as a HARD filter and authors bilingual catalog copy. It NEVER rewrites
// the payload — the body stays the upstream file verbatim (provenance). Gated on
// ANTHROPIC_API_KEY: makeContentLlmCurator() returns null when the key is absent.
import Anthropic from "@anthropic-ai/sdk";
import { zodOutputFormat } from "@anthropic-ai/sdk/helpers/zod";
import { z } from "zod";
import { ExtensionCategory } from "@/contract/schema";
import { CONFIG } from "@/scripts/pipeline/config";
import type { ContentLlmClient, ContentLlmInput, ContentLlmResult } from "@/scripts/pipeline/ports";

// Flat nullable schema (record fields null on reject) — simplest strict structured output.
const ContentDecision = z.object({
  decision: z.enum(["accept", "reject"]),
  reason: z.string(),
  name: z.string().nullable(),
  category: ExtensionCategory.nullable(),
  tags: z.array(z.string()).max(5).nullable(),
  description_en: z.string().nullable(),
  description_zh: z.string().nullable(),
  long_en: z.string().nullable(),
  long_zh: z.string().nullable(),
  sec_note_en: z.string().nullable(),
  sec_note_zh: z.string().nullable(),
});

const SYSTEM_PROMPT = `You are the curator for **Aleph Hub**'s CONTENT catalog — public, copy-and-run prompts and workflow scripts (not installable extensions). You decide whether a discovered file belongs in the public catalog, and if so, write its bilingual (English + 简体中文) catalog copy. You do NOT rewrite the file; you only judge it and describe it.

Apply the policy below as a HARD filter. When in doubt, REJECT ("不确定就排除"). A public directory entry reads as an endorsement.

## 铁律 (provenance)
- The unit must trace to the real upstream file you are given (repo_url + source_path). Never invent or obscure it.

## 硬排除 (hard exclude — regardless of quality)
- 占卜/玄学 (八字, 紫微, 塔罗, 星座算命, 风水预测) presented as knowledge/tools.
- 成人/NSFW.
- 灰帽/spam 营销: bulk cold outreach, link/backlink farms, fake reviews / engagement farming, scaled SEO manipulation. (Legitimate copywriting/analytics/scheduling are KEPT.)
- 厂商锁定薄壳: a prompt that is worthless off one commercial platform (essentially an ad).
- Content-specific: jailbreak / safety-bypass / prompt-injection payloads. "Evade AI detectors / strip AI fingerprints to pass machine output off as human-written" → REJECT (灰帽).

## 边界裁定
- AI writing: readability / style quality (removing clichés, "AI tone") → ACCEPT. Detection evasion → REJECT.
- Security: defensive (threat hunting, CTI, config/code auditing, regression monitoring) → ACCEPT. Offensive/exploitation (pentest frameworks, vuln-hunting fuzzers, exploit dev, CAPTCHA/anti-detection bypass, credentialed single-site scaled scraping) → REJECT.

## 质量门
- prompt: a genuine, reusable prompt with substantive content — not a stub, not a thin vendor shell.
- workflow: a real Claude Code Agent Workflow .js script (it declares meta and orchestrates agents). Judge what the script actually does.
- Describe HONESTLY ("描述照实写"). Put dependencies and risks into sec_note_en/sec_note_zh (e.g. "runs shell via agents", "needs an API key", "controls a browser"). Never empty.

## Output
- decision: "accept" or "reject".
- reason: one concise sentence; on reject, name the rule that excluded it.
- On ACCEPT, fill every field: name (display name); category (one of [search, developer, data, productivity, writing, communication, knowledge, files, design, automation, finance, utilities, other]); tags (2–5 short lowercase); description_en/zh (one faithful sentence each); long_en/zh (1–3 sentences each, factual); sec_note_en/zh (honest dependency/risk note).
- On REJECT, set all record fields to null.`;

function buildUserPrompt(input: ContentLlmInput): string {
  return [
    `kind: ${input.kind}`,
    `repo_url: ${input.repo_url}`,
    `full_name: ${input.full_name}`,
    `source_path: ${input.source_path}`,
    "",
    "FILE (the payload, truncated):",
    "```",
    input.body.slice(0, CONFIG.LLM_BODY_CHARS) || "(empty)",
    "```",
    "",
    "REPO README (context, truncated):",
    "```",
    input.readme.slice(0, CONFIG.LLM_README_CHARS) || "(empty)",
    "```",
  ].join("\n");
}

// Exported for unit testing (pure mapping; an accept with any missing field → reject).
export function toContentResult(parsed: z.infer<typeof ContentDecision>): ContentLlmResult {
  if (parsed.decision !== "accept") return { decision: "reject", reason: parsed.reason };
  const { name, category, tags, description_en, description_zh, long_en, long_zh, sec_note_en, sec_note_zh } = parsed;
  if (!name || !category || !tags || !description_en || !description_zh || !long_en || !long_zh || !sec_note_en || !sec_note_zh) {
    return { decision: "reject", reason: "accept with missing fields → treated as reject" };
  }
  return { decision: "accept", proposal: { name, category, tags, description_en, description_zh, long_en, long_zh, sec_note_en, sec_note_zh } };
}

export function makeContentLlmCurator(): ContentLlmClient | null {
  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) return null;
  const client = new Anthropic({ apiKey });
  return {
    async curate(input: ContentLlmInput): Promise<ContentLlmResult | null> {
      try {
        const response = await client.messages.parse({
          model: CONFIG.LLM_CURATOR_MODEL,
          max_tokens: 4096,
          thinking: { type: "adaptive" },
          system: SYSTEM_PROMPT,
          messages: [{ role: "user", content: buildUserPrompt(input) }],
          output_config: { format: zodOutputFormat(ContentDecision) },
        });
        if (!response.parsed_output) return null;
        return toContentResult(response.parsed_output);
      } catch {
        return null;
      }
    },
  };
}
```

- [ ] **Step 5: Run it to verify it passes**

Run: `npx vitest run scripts/pipeline/__tests__/content-llm-curator.test.ts`
Expected: PASS (3 tests). Then `npm run typecheck` → clean.

- [ ] **Step 6: Commit**

```bash
git add scripts/pipeline/ports.ts scripts/pipeline/content-llm-curator.ts scripts/pipeline/__tests__/content-llm-curator.test.ts
git commit -m "feat(pipeline): autonomous content LLM curator (policy filter + bilingual copy)"
```

---

### Task 4: content-run LLM loop + reserved-slug filter

Wire autonomous curation and slug-collision safety into `runContent`. Accepted units are emitted this run and returned for persistence (`curated_by:"llm"`).

**Files:**
- Modify: `scripts/pipeline/content-model.ts` (report fields + `PersistedContentCuration`)
- Modify: `scripts/pipeline/content-run.ts`
- Modify: `scripts/pipeline/__tests__/content-run.test.ts`

**Interfaces:**
- Consumes: `ContentLlmClient`, `ContentLlmProposal`, `ContentCurationRecord` (ports.ts); `curateContent` (content-curate.ts); `CONFIG.LLM_CURATE_PER_RUN`.
- Produces: `ContentRunPorts` gains `llm: ContentLlmClient | null` and `reservedSlugs?: Set<string>`; `runContent` returns additionally `newCurations: PersistedContentCuration[]`; `ContentBuildReport` gains `autoCurated: number` and `reservedDropped: number`; new type `PersistedContentCuration`.

- [ ] **Step 1: Extend the report model**

In `scripts/pipeline/content-model.ts`, update `ContentBuildReport` and add the persisted type (add the `ContentCurationRecord` import at the top):

```ts
import type { ContentCurationRecord } from "@/scripts/pipeline/ports";
```

```ts
export interface ContentBuildReport {
  candidates: number;
  curated: number;        // entries from existing (human/LLM) records this run
  autoCurated: number;    // units the LLM accepted + emitted this run
  queued: number;
  emitted: number;        // total entries in the artifact
  reservedDropped: number; // entries dropped because their slug collides with an install slug
}

// An LLM-authored content record persisted as a human-auditable review buffer.
export type PersistedContentCuration = ContentCurationRecord & { curated_by: "llm" };
```

- [ ] **Step 2: Write the failing run tests**

In `scripts/pipeline/__tests__/content-run.test.ts`: (a) add `llm: null` to the four existing `runContent({...})` calls; (b) add a fake-LLM helper and three new tests. Add this helper after the `cand` factory:

```ts
import type { ContentLlmClient, ContentLlmResult } from "@/scripts/pipeline/ports";

const acceptProposal = {
  name: "Found", category: "writing", tags: ["t"], description_en: "e", description_zh: "z",
  long_en: "le", long_zh: "lz", sec_note_en: "se", sec_note_zh: "sz",
};
function llm(map: Record<string, ContentLlmResult | null>): ContentLlmClient {
  return { async curate(input) { return input.full_name in map ? map[input.full_name] : null; } };
}
```

Add these tests inside the `describe("runContent", ...)` block:

```ts
  it("auto-curates a queued candidate and returns it for persistence", async () => {
    const c = cand({ owner: "new", repo: "r", slug: "x", kind: "prompt", raw: { text: "Do a thing." } });
    const res = await runContent({
      sources: [source([c])], store: store([]), clock, officialOrgs: new Set(),
      llm: llm({ "new/r": { decision: "accept", proposal: acceptProposal } }),
    });
    expect(res.report.autoCurated).toBe(1);
    expect(res.report.emitted).toBe(1);
    expect(res.report.queued).toBe(0);                 // accepted unit leaves the backlog
    expect(res.newCurations).toHaveLength(1);
    expect(res.newCurations[0].curated_by).toBe("llm");
    expect(res.newCurations[0].id).toBe("aleph-hub:new/r#x");
  });

  it("leaves a rejected candidate queued and unpersisted", async () => {
    const c = cand({ owner: "new", repo: "r", slug: "x" });
    const res = await runContent({
      sources: [source([c])], store: store([]), clock, officialOrgs: new Set(),
      llm: llm({ "new/r": { decision: "reject", reason: "NSFW" } }),
    });
    expect(res.report.autoCurated).toBe(0);
    expect(res.report.queued).toBe(1);
    expect(res.newCurations).toHaveLength(0);
  });

  it("drops an entry whose slug collides with a reserved install slug", async () => {
    const res = await runContent({
      sources: [source([])], store: store([record()]), clock, officialOrgs: new Set(),
      llm: null, reservedSlugs: new Set(["acme/prompts/hello"]),  // matches record id acme/prompts#hello
    });
    expect(res.report.reservedDropped).toBe(1);
    expect(res.report.emitted).toBe(0);
  });
```

- [ ] **Step 3: Run it to verify it fails**

Run: `npx vitest run scripts/pipeline/__tests__/content-run.test.ts`
Expected: FAIL — `runContent` lacks the `llm` port / new report fields / `newCurations`.

- [ ] **Step 4: Rewrite content-run.ts**

Replace the entire contents of `scripts/pipeline/content-run.ts`:

```ts
import { curateContent } from "@/scripts/pipeline/content-curate";
import { buildContentArtifacts } from "@/scripts/pipeline/content-emit";
import { CONFIG } from "@/scripts/pipeline/config";
import type { Clock, ContentCurationStore, ContentLlmClient, ContentLlmProposal, ContentCurationRecord } from "@/scripts/pipeline/ports";
import type { ContentSource } from "@/scripts/pipeline/sources/content-types";
import type { ContentCandidate, ContentFinalEntry, ContentBuildReport, PersistedContentCuration } from "@/scripts/pipeline/content-model";
import type { TrustTierT } from "@/contract/types";

export interface ContentRunPorts {
  sources: ContentSource[];
  store: ContentCurationStore;
  clock: Clock;
  officialOrgs: Set<string>;   // lower-cased owners
  llm: ContentLlmClient | null; // null disables auto-curation (no ANTHROPIC_API_KEY)
  reservedSlugs?: Set<string>;  // install site-slugs; colliding content entries are dropped
}

// Plan-1 trust: official if the owner is an official org, else community.
function contentTrustTier(owner: string, officialOrgs: Set<string>): TrustTierT {
  return officialOrgs.has(owner.toLowerCase()) ? "official" : "community";
}

// "aleph-hub:owner/repo#unit" → site-slug "owner/repo/unit" (the website's routing key).
function siteSlug(id: string): string {
  return id.replace(/^aleph-hub:/, "").replace("#", "/");
}

const idOf = (c: ContentCandidate): string => `aleph-hub:${c.owner}/${c.repo}#${c.slug}`;

// Build a curation record from a candidate + the LLM's metadata. The body is the
// upstream file verbatim; format is fixed by kind. (curateContent re-validates + safety.)
function recordFromProposal(c: ContentCandidate, p: ContentLlmProposal): ContentCurationRecord {
  return {
    id: idOf(c), full_name: `${c.owner}/${c.repo}`, slug: c.slug, source_path: c.source_path,
    kind: c.kind, category: p.category, name: p.name, tags: p.tags,
    format: c.kind === "workflow" ? "javascript" : "markdown", body: c.raw.text,
    description_en: p.description_en, description_zh: p.description_zh,
    long_en: p.long_en, long_zh: p.long_zh,
    sec_note_en: p.sec_note_en, sec_note_zh: p.sec_note_zh,
  };
}

export async function runContent(ports: ContentRunPorts): Promise<{
  catalog: unknown; site: unknown; hash: string; report: ContentBuildReport;
  queue: ContentCandidate[]; newCurations: PersistedContentCuration[];
}> {
  // 1) Discovery → candidates.
  const candidates: ContentCandidate[] = [];
  for (const s of ports.sources) candidates.push(...(await s.fetch()));

  // 2) Emit from existing curation records (body is already curated).
  const records = ports.store.all();
  const finals: ContentFinalEntry[] = [];
  for (const rec of records) {
    const curated = curateContent(rec);
    if (!curated) continue;
    finals.push({ ...curated, trust_tier: contentTrustTier(curated.author, ports.officialOrgs) });
  }
  const recordEmitted = finals.length;

  // 3) Queue = discovered units with no record yet.
  const haveIds = new Set(records.map((r) => r.id));
  const queue = candidates.filter((c) => !haveIds.has(idOf(c)));

  // 4) Autonomous curation: LLM applies the policy as a hard filter over a capped batch.
  const newCurations: PersistedContentCuration[] = [];
  const autoAccepted = new Set<string>();
  if (ports.llm) {
    for (const c of queue.slice(0, CONFIG.LLM_CURATE_PER_RUN)) {
      const result = await ports.llm.curate({
        repo_url: c.repo_url, full_name: `${c.owner}/${c.repo}`, source_path: c.source_path,
        kind: c.kind, body: c.raw.text, readme: c.readme ?? "",
      });
      if (!result || result.decision !== "accept") continue;
      const record = recordFromProposal(c, result.proposal);
      const curated = curateContent(record);
      if (!curated) continue;                          // failed safety/zod → not persisted/emitted
      finals.push({ ...curated, trust_tier: contentTrustTier(curated.author, ports.officialOrgs) });
      newCurations.push({ ...record, curated_by: "llm" });
      autoAccepted.add(record.id);
    }
  }

  // 5) Drop any entry whose site-slug collides with an install slug (fail-safe vs the
  //    website build guard in lib/site.ts, which throws on collision).
  const reserved = ports.reservedSlugs ?? new Set<string>();
  const kept = finals.filter((e) => !reserved.has(siteSlug(e.id)));
  const reservedDropped = finals.length - kept.length;

  const { catalog, site, hash } = buildContentArtifacts({ entries: kept, generatedAt: ports.clock.nowIso() });
  const finalQueue = queue.filter((c) => !autoAccepted.has(idOf(c)));
  const report: ContentBuildReport = {
    candidates: candidates.length, curated: recordEmitted, autoCurated: autoAccepted.size,
    queued: finalQueue.length, emitted: kept.length, reservedDropped,
  };
  return { catalog, site, hash, report, queue: finalQueue, newCurations };
}
```

- [ ] **Step 5: Run it to verify it passes**

Run: `npx vitest run scripts/pipeline/__tests__/content-run.test.ts`
Expected: PASS (4 existing + 3 new). Then `npm run typecheck` → clean.

- [ ] **Step 6: Commit**

```bash
git add scripts/pipeline/content-model.ts scripts/pipeline/content-run.ts scripts/pipeline/__tests__/content-run.test.ts
git commit -m "feat(pipeline): content-run autonomous curation + reserved-slug guard"
```

---

### Task 5: Weekday kind map + content entrypoint wiring

The cron-callable `target-kind.mjs` and the `content-index.ts` wiring (real GitHub adapter, LLM curator, `--kind`, reserved slugs, LLM-record persistence).

**Files:**
- Create: `scripts/pipeline/target-kind.mjs`
- Modify: `scripts/pipeline/adapters.ts` (add `makeContentGitHub`)
- Modify: `scripts/pipeline/content-index.ts`
- Test: `scripts/pipeline/__tests__/target-kind.test.ts`

**Interfaces:**
- Consumes: `runContent` + its ports (Task 4); `makeContentLlmCurator` (Task 3); `GitHubContentSource` (Task 2); `ContentSeeds`/`ContentKindT`; `makeAdapters`/`makeContentCurationStore` (existing).
- Produces: `kindForDay(day): string`, `isContentKind(kind): boolean`, `resolveKind(argv, env, day): string` (all from `target-kind.mjs`); `makeContentGitHub(): ContentGitHub` (adapters.ts).

- [ ] **Step 1: Write the failing kind-map test**

Create `scripts/pipeline/__tests__/target-kind.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { kindForDay, isContentKind, resolveKind } from "@/scripts/pipeline/target-kind.mjs";

describe("kindForDay", () => {
  it("maps ISO weekday to the partitioned kind", () => {
    expect([1, 2, 3, 4, 5, 6, 7].map((d) => kindForDay(d))).toEqual(
      ["skill", "skill", "plugin", "mcp", "prompt", "prompt", "workflow"],
    );
  });
  it("defaults out-of-range days to skill", () => {
    expect(kindForDay(0)).toBe("skill");
  });
});

describe("isContentKind", () => {
  it("is true only for prompt/workflow", () => {
    expect(isContentKind("prompt")).toBe(true);
    expect(isContentKind("workflow")).toBe(true);
    expect(isContentKind("mcp")).toBe(false);
  });
});

describe("resolveKind", () => {
  it("prefers --kind, then TARGET_KIND, then the weekday", () => {
    expect(resolveKind(["--kind=workflow"], { TARGET_KIND: "prompt" }, 1)).toBe("workflow");
    expect(resolveKind([], { TARGET_KIND: "prompt" }, 1)).toBe("prompt");
    expect(resolveKind([], {}, 7)).toBe("workflow");
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `npx vitest run scripts/pipeline/__tests__/target-kind.test.ts`
Expected: FAIL — cannot find module `target-kind.mjs`.

- [ ] **Step 3: Implement target-kind.mjs**

Create `scripts/pipeline/target-kind.mjs`:

```js
// Weekly kind partition — the single source of truth for the cron and content entrypoint.
// ISO day-of-week: 1=Mon … 7=Sun. skill/prompt weighted ×2 (highest volume).
const BY_DAY = { 1: "skill", 2: "skill", 3: "plugin", 4: "mcp", 5: "prompt", 6: "prompt", 7: "workflow" };
const CONTENT_KINDS = new Set(["prompt", "workflow"]);

export function kindForDay(day) {
  return BY_DAY[Number(day)] ?? "skill";
}

export function isContentKind(kind) {
  return CONTENT_KINDS.has(kind);
}

// Precedence: explicit --kind=<k> arg > TARGET_KIND env > weekday default.
export function resolveKind(argv, env, day) {
  for (const a of argv ?? []) {
    const m = /^--kind=(.+)$/.exec(a);
    if (m) return m[1];
  }
  return (env && env.TARGET_KIND) || kindForDay(day);
}

// CLI: `node scripts/pipeline/target-kind.mjs 5` → prints "prompt" (no newline).
if (import.meta.url === `file://${process.argv[1]}`) {
  process.stdout.write(kindForDay(process.argv[2]));
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `npx vitest run scripts/pipeline/__tests__/target-kind.test.ts`
Expected: PASS (4 tests). Also verify the CLI: `node scripts/pipeline/target-kind.mjs 7` → prints `workflow`.

- [ ] **Step 5: Add the content GitHub adapter**

In `scripts/pipeline/adapters.ts`: add `ContentGitHub` to the type import from `sources/content-types` (it is not currently imported — add a new import line near the top), then add `makeContentGitHub` (after `makeGitHub`):

```ts
import type { ContentGitHub } from "@/scripts/pipeline/sources/content-types";
```

```ts
// Content source GitHub surface: reuse the raw adapter, add a recursive file lister
// (git trees at HEAD). Content uses the raw adapter directly (no etag caching layer).
export function makeContentGitHub(): ContentGitHub {
  const raw = makeGitHub();
  return {
    searchRepos: (q) => raw.searchRepos(q),
    getContent: (fn, p) => raw.getContent(fn, p),
    getReadme: (fn) => raw.getReadme(fn),
    async listFiles(fullName) {
      try {
        const res = await fetch(`${GH_API}/repos/${fullName}/git/trees/HEAD?recursive=1`, { headers: ghHeaders() });
        if (!res.ok) return [];
        const json = (await res.json()) as { tree?: { path: string; type: string }[] };
        return (json.tree ?? []).filter((t) => t.type === "blob").map((t) => t.path);
      } catch { return []; }
    },
  };
}
```

- [ ] **Step 6: Rewrite content-index.ts**

Replace the entire contents of `scripts/pipeline/content-index.ts`:

```ts
import { runContent } from "@/scripts/pipeline/content-run";
import { makeAdapters, makeContentCurationStore, makeContentGitHub } from "@/scripts/pipeline/adapters";
import { makeContentLlmCurator } from "@/scripts/pipeline/content-llm-curator";
import { GitHubContentSource } from "@/scripts/pipeline/sources/github-content";
import { resolveKind, isContentKind } from "@/scripts/pipeline/target-kind.mjs";
import type { ContentSeeds } from "@/scripts/pipeline/sources/content-types";
import type { ContentKindT } from "@/contract/content-schema";
import type { FileStore } from "@/scripts/pipeline/ports";

// Install site-slugs (owner/repo) the content axis must not shadow (lib/site.ts throws on collision).
function installSiteSlugs(fs: FileStore): Set<string> {
  const cat = fs.readJson<{ entries?: { id: string }[] }>("data/site-catalog.json");
  return new Set((cat?.entries ?? []).map((e) => e.id.replace(/^aleph-hub:/, "")));
}

async function main() {
  const { clock, fs, http } = makeAdapters();
  const day = new Date().getUTCDay() || 7;             // JS Sun=0 → ISO 7
  const resolved = resolveKind(process.argv.slice(2), process.env, day);
  const kind: ContentKindT = (isContentKind(resolved) ? resolved : "prompt") as ContentKindT;

  const seeds = fs.readJson<ContentSeeds>("data/seeds/content.json")
    ?? { prompt: { queries: [], seeds: [], pins: [] }, workflow: { queries: [], seeds: [], pins: [] } };
  const officialOrgs = new Set((fs.readJson<string[]>("data/seeds/official-orgs.json") ?? []).map((s) => s.toLowerCase()));
  const store = makeContentCurationStore();
  const gh = makeContentGitHub();
  const llm = makeContentLlmCurator();                  // null unless ANTHROPIC_API_KEY is set
  const sources = [new GitHubContentSource({ gh, http, kind, seeds: seeds[kind] })];
  const reservedSlugs = installSiteSlugs(fs);

  const prev = fs.readJson<{ manifest?: { content_hash?: string } }>("public/catalog-content.json");
  const res = await runContent({ sources, store, clock, officialOrgs, llm, reservedSlugs });

  // Persist LLM-authored records (review buffer): data/curation-content/<owner>__<repo>__<slug>.json
  for (const rec of res.newCurations) {
    fs.writeJson(`data/curation-content/${rec.full_name.replace(/\//g, "__")}__${rec.slug}.json`, rec);
  }
  fs.writeJson("data/queue/content-to-curate.json", res.queue);  // always — backlog visibility
  if (res.hash !== prev?.manifest?.content_hash) {               // skip-emit on unchanged content
    fs.writeJson("public/catalog-content.json", res.catalog);
    fs.writeJson("data/site-content.json", res.site);
  }
  console.log(JSON.stringify({ kind, ...res.report }, null, 2));
}
main().catch((e) => { console.error(e); process.exit(1); });
```

- [ ] **Step 7: Verify typecheck + full suite**

Run: `npm run typecheck` → clean.
Run: `npx vitest run scripts/pipeline/` → all pipeline tests pass.

- [ ] **Step 8: Commit**

```bash
git add scripts/pipeline/target-kind.mjs scripts/pipeline/adapters.ts scripts/pipeline/content-index.ts scripts/pipeline/__tests__/target-kind.test.ts
git commit -m "feat(pipeline): weekday kind map + content entrypoint discovery wiring"
```

---

### Task 6: Weekly cron partition + content seeds

The single daily cron computes the day's kind and runs install or content accordingly; content artifacts get staged. Refresh the content seeds so discovery has real inputs.

**Files:**
- Modify: `.github/workflows/pipeline.yml`
- Modify: `data/seeds/content.json`

**Interfaces:**
- Consumes: `scripts/pipeline/target-kind.mjs` (CLI), `npm run pipeline` (install), `npm run pipeline:content -- --kind=<k>` (content).
- Produces: a kind-partitioned daily run; both artifacts staged on commit.

- [ ] **Step 1: Refresh content seeds**

Replace the entire contents of `data/seeds/content.json`:

```json
{
  "prompt": {
    "queries": ["topic:awesome-prompts", "topic:prompt-engineering", "topic:claude-prompts", "topic:chatgpt-prompts"],
    "seeds": [],
    "pins": ["dair-ai/Prompt-Engineering-Guide:guides/prompts-basic-usage.md"]
  },
  "workflow": {
    "queries": ["topic:claude-code-workflow", "topic:agent-workflow", "topic:claude-agent-workflow"],
    "seeds": [],
    "pins": []
  }
}
```

- [ ] **Step 2: Verify the kind CLI for every weekday**

Run: `for d in 1 2 3 4 5 6 7; do echo "$d=$(node scripts/pipeline/target-kind.mjs $d)"; done`
Expected: `1=skill 2=skill 3=plugin 4=mcp 5=prompt 6=prompt 7=workflow`.

- [ ] **Step 3: Update the cron workflow**

Replace the entire contents of `.github/workflows/pipeline.yml`:

```yaml
name: pipeline
on:
  schedule:
    - cron: "17 3 * * *"   # daily 03:17 UTC
  workflow_dispatch:
    inputs:
      kind:
        description: "Force a kind (skill|plugin|mcp|prompt|workflow); empty = auto by weekday"
        required: false
        default: ""
concurrency:
  group: pipeline
  cancel-in-progress: false
jobs:
  refresh:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
        with:
          token: ${{ secrets.GH_PAT }}   # real identity so the push triggers downstream
      - uses: actions/setup-node@v5
        with:
          node-version: 24
          cache: npm
      - run: npm ci
      - name: Compute target kind
        id: kind
        run: echo "kind=$(node scripts/pipeline/target-kind.mjs $(date -u +%u))" >> "$GITHUB_OUTPUT"
      - name: Run deterministic pipeline (kind-partitioned)
        env:
          GH_TOKEN: ${{ secrets.GH_TOKEN }}
          # Enables autonomous LLM curation. Unset → auto-curation is skipped and the run
          # still publishes human + first-party / existing entries (curator returns null).
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
        run: |
          K="${{ github.event.inputs.kind || steps.kind.outputs.kind }}"
          echo "target kind: $K"
          case "$K" in
            prompt|workflow) npm run pipeline:content -- --kind="$K" ;;
            *)               npm run pipeline ;;
          esac
      # On push to main, Vercel's git integration auto-deploys the new catalog — no explicit hook step.
      - name: Commit artifacts if changed
        run: |
          git config user.name "aleph-hub-bot"
          git config user.email "bot@heyaleph.com"
          git add public/catalog.json public/catalog-content.json data/
          if git diff --cached --quiet; then
            echo "no changes to commit"
          else
            git commit -m "chore(catalog): refresh artifact $(date -u +%Y-%m-%d)"
            git push
          fi
```

- [ ] **Step 4: Verify the workflow parses and typecheck is clean**

Run: `node -e "const f=require('fs').readFileSync('.github/workflows/pipeline.yml','utf8'); if(!/target-kind\.mjs/.test(f)||!/pipeline:content -- --kind=/.test(f)) throw new Error('cron wiring missing'); console.log('cron OK')"`
Expected: `cron OK`.
Run: `npm run typecheck` → clean.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/pipeline.yml data/seeds/content.json
git commit -m "ci(pipeline): weekly kind-partitioned cron + content discovery seeds"
```

---

### Task 7: Full-suite + production build verification

No new feature files — prove the branch is green end-to-end and the install contract is byte-unchanged.

**Files:**
- None (verification only).

- [ ] **Step 1: Full test suite**

Run: `npm test`
Expected: PASS — all suites (existing + the new workflow-detect, content-explode, content-source, content-llm-curator, content-run, target-kind, safety tests).

- [ ] **Step 2: Typecheck**

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 3: Validate both artifacts**

Run: `npm run validate:content`
Expected: OK — content artifact validates (entry count ≥ 0; the committed `public/catalog-content.json` is unchanged by this branch since no live crawl ran).
Run: `npm run validate:catalog`
Expected: OK — 125 entries, `schema_version` 1.

- [ ] **Step 4: Install contract byte-unchanged vs main**

Run:
```bash
git diff --name-only main -- public/catalog.json contract/schema.ts contract/site.ts contract/types.ts scripts/pipeline/emit.ts scripts/pipeline/run.ts scripts/pipeline/index.ts scripts/pipeline/curate.ts
```
Expected: empty output (铁律 holds — the install pipeline and its contract are untouched).

- [ ] **Step 5: Production build**

Run: `npm run build`
Expected: build succeeds; the content/workflow site routes still render (workflow region shows real entries if any exist, else the Plan-2 "coming soon" region — no crash).

- [ ] **Step 6: Commit (if any verification fix was needed; otherwise skip)**

Only if a fix was required during verification:

```bash
git add -A
git commit -m "test(pipeline): verification fixes for content discovery + cron"
```

---

## Notes — deliberate deviations from the spec (for the reviewer)

These are documented choices, not internal contradictions. Both were confirmed with the human before planning:

1. **Partition at pipeline-selection granularity, not within a single pipeline (spec §4.5/§4.6).** The repo has two physically separate pipelines, and an install entry's *kind* is decided at curation time (the sources are not kind-separable). So `--kind` only meaningfully partitions `prompt` vs `workflow` inside the content pipeline; the cron picks install-vs-content by weekday. The install pipeline (`run.ts`/`index.ts`/`emit.ts`/`catalog.json`) is **untouched** — this keeps the install contract pipeline byte-stable and avoids a carry-from-cache refactor. The §4.6 "kind-aware drop guards" are therefore unnecessary: the content pipeline has no drop guard, and the install pipeline runs whole on its days.

2. **File-level explosion; intra-file extraction is a non-goal (spec §3 example).** A repo explodes into one candidate per prompt file / per workflow `.js` — the dominant structure of Claude prompt/workflow repos. Splitting a single collection file (one CSV/README) into N records would require a different, list-returning LLM call shape; it is explicitly deferred. The per-file backlog still drains over multiple prompt-days via `LLM_CURATE_PER_RUN`, matching §3's intent at file granularity.

3. **The content LLM curator never rewrites `body`.** Mirroring how the install curator never authors `install_spec`, the content curator authors only metadata + accept/reject; `body` is the upstream file verbatim (provenance + correctness). This is stricter than, and consistent with, the spec's inline-payload model.

4. **Reserved-slug fail-safe (Plan-2 carry-over).** `content-index.ts` passes the install site-slug set; colliding content entries are dropped (`reservedDropped`) rather than emitted, so an autonomous run can never publish an artifact that trips `lib/site.ts buildBySlug()` and breaks the website build.

---

## Self-Review

**1. Spec coverage**
- §4.1 content sources (topic + awesome-list + pins → file units): Task 2. ✅
- §4.2 curate-content skips install_spec, sets body/format: reuses existing `content-curate.ts` (unchanged); LLM authors metadata (Task 3). ✅
- §4.3 safety widens to body (already, via `safeBodyOrNull` over the whole body) + content hard-exclusions (jailbreak/injection existing; AI-evasion added Task 1). Workflow subagent-prompt text is inside `body`, so scanning `body` covers it. ✅
- §4.4 emit-content (own hash, skip-on-unchanged): unchanged `content-emit.ts` + `content-index.ts` skip logic. ✅
- §4.5 `--kind` selects that kind's sources/backlog: Tasks 5–6 (content pipeline). Install partition by pipeline selection — see Notes. ✅ (deviation documented)
- §4.6 kind-aware guards: N/A under the chosen partition — see Notes. ✅
- §5 weekly cron partition + `target-kind` + `workflow_dispatch` kind input + staging content artifacts: Tasks 5–6. ✅
- §6 website: already shipped in Plan 2 (5-kind browse; workflow region renders when data exists). No changes needed; verified in Task 7. ✅
- §9 testing (schema round-trip exists; curate-content exists; explosion → multiple records: Task 2; target-kind mapping + `--kind`: Task 5; emit skip-on-unchanged exists; safety drop on injection/jailbreak/evasion: Task 1; LLM curation: Tasks 3–4). ✅
- §10 contract-sync: no content-entry field add/remove, `content_schema_version` unchanged. ✅

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; every test step shows the assertions. Cron YAML is complete. ✅

**3. Type consistency:** `ContentCandidate` (`kind`/`readme`) introduced in Task 2 and consumed in Tasks 4–5; `ContentLlmProposal`/`ContentLlmResult`/`ContentLlmClient` defined in Task 3, consumed in Task 4; `ContentBuildReport` fields (`autoCurated`, `reservedDropped`) defined and asserted together (Task 4); `kindForDay`/`isContentKind`/`resolveKind` defined in Task 5 and used by Task 6's cron via the CLI; `makeContentGitHub` returns the `ContentGitHub` widened in Task 2. Function names match across producer/consumer tasks. ✅
