# Content Catalog Website (5-kind browse + content copy/detail) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface the new `prompt`/`workflow` content catalog on the public website — a unified 5-kind browse (home index, kind pages, detail) with copy-the-body actions — while hardening three Plan-1 pipeline carry-overs.

**Architecture:** Plan 1 published a second, decoupled artifact pair (`public/catalog-content.json` + `data/site-content.json`). This plan consumes `data/site-content.json` on the website. The install catalog (`lib/catalog.ts`, install-typed) stays untouched; a new `lib/content.ts` loads the content site data and a thin `lib/site.ts` unifies both for routing/detail. Content entries carry no `stars`/`install_cmd`; the UI branches on `kind`. Content detail renders the inline `body` in a `<pre>` block with a Copy button (no new markdown/highlighter dependency).

**Tech Stack:** Next.js 15 App Router, React 19, TypeScript, Zod 4, Vitest + Testing Library. Path alias `@/` → repo root. No new runtime dependencies.

## Global Constraints

- **P-Provenance 铁律**: every content entry carries a real upstream `repo_url` + `source_path`; the website must surface them (never hide provenance).
- **Contract decoupling 铁律**: do **not** edit `public/catalog.json`, `contract/schema.ts`, `contract/site.ts`, `contract/types.ts`, or any install-pipeline emit. The install wire contract stays `schema_version: 1`, byte-for-byte. Content lives in its own files.
- **`content_schema_version` is a wire contract**: its value space syncs with Aleph. This plan only reads it (and centralizes the producer constant); it does not bump it.
- **Immutability (CRITICAL)**: never mutate inputs; build new objects/arrays (spread). No in-place `.sort()` on shared arrays without copying first.
- **No per-user state**: artifacts never carry `installed`/`enabled`; the site reads catalog data only.
- **Reuse existing enums**: content uses the shared `ExtensionCategory` / `TrustTier`; do not add categories.
- **Style/process**: reply in Chinese, code comments in English. Commit format `<scope>: <description>` (English), **no attribution trailer**. Match the existing inline-style + component conventions (no CSS modules, no new design system). Files focused (<800 lines).
- **No new dependencies**: render `body` as preformatted text; do not add `react-markdown`, a syntax highlighter, or similar.

---

## File Structure

**Create:**
- `lib/content.ts` — content site-catalog loader + getters (mirrors `lib/catalog.ts` shape, content-typed).
- `lib/site.ts` — unified browse layer: `AnySiteEntry` union, `isContent`, slug scheme, `anyBySlug`/`allSlugs` over both catalogs.
- `components/home/ContentIndex.tsx` — the two new home index regions (Prompts, Workflows).
- `components/detail/ContentDetailView.tsx` — kind-aware content detail (body + Copy + provenance + Aleph action placeholder).
- `lib/__tests__/content.test.ts`, `lib/__tests__/site.test.ts` — data-layer tests.
- `components/home/__tests__/content-index.test.tsx`, `components/detail/__tests__/content-detail.test.tsx` — UI tests.

**Modify:**
- `scripts/pipeline/content-curate.ts` — reconstruct/validate `id` against `full_name`+`slug` (carry-over a).
- `contract/content-schema.ts` — export `CONTENT_SCHEMA_VERSION` constant (carry-over b).
- `scripts/pipeline/content-emit.ts` — use `CONTENT_SCHEMA_VERSION` (carry-over b).
- `scripts/pipeline/safety.ts` — comment noting the coarse DROP-net intent (carry-over c).
- `lib/i18n.ts` — content strings + `CONTENT_KIND_LABELS`.
- `components/Card.tsx` — accept `AnySiteEntry`, kind-aware footer + link.
- `components/category/CategoryView.tsx` — accept content kinds, branch data source + titles.
- `app/c/[kind]/page.tsx` — add `prompt`/`workflow` to `KINDS`.
- `app/e/[...slug]/page.tsx` — resolve via `anyBySlug`, branch to `ContentDetailView`.
- `app/page.tsx` — compose `<ContentIndex />` after `<CategoryIndex />`.
- `scripts/pipeline/__tests__/content-curate.test.ts`, `scripts/pipeline/__tests__/content-emit.test.ts` — carry-over tests.
- `components/__tests__/card.test.tsx`, `components/category/__tests__/category.test.tsx` — extend for content.

---

### Task 1: Pipeline carry-overs (id integrity, shared version constant, safety comment)

Three independent Plan-1 follow-ups, landed before content volume grows. All pure-function / contract changes.

**Files:**
- Modify: `scripts/pipeline/content-curate.ts`
- Modify: `contract/content-schema.ts`
- Modify: `scripts/pipeline/content-emit.ts`
- Modify: `scripts/pipeline/safety.ts`
- Test: `scripts/pipeline/__tests__/content-curate.test.ts`
- Test: `scripts/pipeline/__tests__/content-emit.test.ts`

**Interfaces:**
- Consumes: existing `curateContent(record)`, `buildContentArtifacts(input)`.
- Produces: `CONTENT_SCHEMA_VERSION` exported from `contract/content-schema.ts` (number `1`); `curateContent` now drops records whose `id` ≠ `aleph-hub:{full_name}#{slug}`.

- [ ] **Step 1: Write the failing test for id integrity**

Add a new `it` inside the existing `describe`, reusing the file's existing `rec()` factory (its default record is canonical: id `aleph-hub:acme/prompts#hello`, full_name `acme/prompts`, slug `hello`):

```ts
it("drops a record whose id does not match full_name + slug", () => {
  // canonical id (the factory default) passes
  expect(curateContent(rec())).not.toBeNull();
  // a drifted id (slug mismatch) is dropped
  expect(curateContent(rec({ id: "aleph-hub:acme/prompts#WRONG" }))).toBeNull();
  // the emitted id is reconstructed from full_name + slug, not trusted verbatim
  expect(curateContent(rec())!.id).toBe("aleph-hub:acme/prompts#hello");
});
```

- [ ] **Step 2: Run it to confirm it fails**

Run: `npx vitest run scripts/pipeline/__tests__/content-curate.test.ts`
Expected: FAIL on the tampered-id assertion (current code accepts any `id`).

- [ ] **Step 3: Add the id refine + reconstruction in `content-curate.ts`**

Add a `.refine` to the `Curated` schema so a drifted id is dropped, and emit a reconstructed canonical id. Change the schema object close and the return:

```ts
const Curated = z.object({
  id: z.string().min(1),
  full_name: z.string().min(1),
  slug: z.string().min(1),
  source_path: z.string().min(1),
  kind: ContentKind,
  category: ExtensionCategory,
  name: z.string().min(1),
  tags: z.array(z.string()).max(5),
  format: ContentFormat,
  body: z.string().min(1).max(CONFIG.CONTENT_BODY_MAX),
  description_en: z.string().min(1), description_zh: z.string().min(1),
  long_en: z.string().min(1), long_zh: z.string().min(1),
  sec_note_en: z.string().min(1), sec_note_zh: z.string().min(1),
}).refine(
  // id is the queue dedup + provenance key; it must equal the canonical form
  // derived from full_name + slug, or discovery/curation have drifted.
  (r) => r.id === `aleph-hub:${r.full_name}#${r.slug}`,
  { message: "id does not match full_name#slug" },
);
```

Then in the returned object use the reconstructed id (authoritative; the refine already guarantees equality, so reconstructing keeps the formula in one place):

```ts
const [owner, repo] = c.full_name.split("/");
if (!owner || !repo) return null;            // unresolvable upstream → drop (provenance)
const id = `aleph-hub:${owner}/${repo}#${c.slug}`;
```

and change `id: c.id,` → `id,` in the return literal.

- [ ] **Step 4: Run it to confirm it passes**

Run: `npx vitest run scripts/pipeline/__tests__/content-curate.test.ts`
Expected: PASS (all cases).

- [ ] **Step 5: Write the failing test for the shared version constant**

Add to `scripts/pipeline/__tests__/content-emit.test.ts`:

```ts
import { CONTENT_SCHEMA_VERSION } from "@/contract/content-schema";

it("stamps the manifest with the shared CONTENT_SCHEMA_VERSION", () => {
  const { catalog } = buildContentArtifacts({ entries: [], generatedAt: "2026-06-22T00:00:00Z" });
  expect((catalog as { manifest: { content_schema_version: number } }).manifest.content_schema_version).toBe(CONTENT_SCHEMA_VERSION);
});
```

- [ ] **Step 6: Run it to confirm it fails**

Run: `npx vitest run scripts/pipeline/__tests__/content-emit.test.ts`
Expected: FAIL on missing export `CONTENT_SCHEMA_VERSION`.

- [ ] **Step 7: Add the constant and consume it**

In `contract/content-schema.ts`, add after the `CONTENT_BODY_MAX` line:

```ts
export const CONTENT_SCHEMA_VERSION = 1; // producer constant; wire value synced with Aleph
```

In `scripts/pipeline/content-emit.ts`, import it and use it in `manifestBase`:

```ts
import { CONTENT_SCHEMA_VERSION } from "@/contract/content-schema";
```
```ts
const manifestBase = { content_schema_version: CONTENT_SCHEMA_VERSION, hub_id: "aleph-hub", name: "Aleph Hub", entry_count: input.entries.length };
```

- [ ] **Step 8: Add the safety comment (carry-over c)**

In `scripts/pipeline/safety.ts`, add a comment directly above the `SUSPICIOUS` array declaration (do not change any phrase lists or logic):

```ts
// NOTE: SUSPICIOUS + JAILBREAK are a coarse DROP-net, not a complete filter. They
// catch obvious override/jailbreak phrasing; residual whitespace/separator evasion
// is accepted and backstopped by human curation review before an entry ships.
```

- [ ] **Step 9: Run the carry-over tests + typecheck**

Run: `npx vitest run scripts/pipeline/__tests__/content-curate.test.ts scripts/pipeline/__tests__/content-emit.test.ts`
Expected: PASS.
Run: `npm run typecheck`
Expected: no output (clean).

- [ ] **Step 10: Commit**

```bash
git add scripts/pipeline/content-curate.ts contract/content-schema.ts scripts/pipeline/content-emit.ts scripts/pipeline/safety.ts scripts/pipeline/__tests__/content-curate.test.ts scripts/pipeline/__tests__/content-emit.test.ts
git commit -m "fix(content): canonical id integrity, shared schema-version constant, safety net comment"
```

---

### Task 2: Shared data + i18n layer (`lib/content.ts`, `lib/site.ts`, content strings)

The foundation every UI task consumes: content getters, the unified slug/resolver, and content i18n.

**Files:**
- Create: `lib/content.ts`
- Create: `lib/site.ts`
- Modify: `lib/i18n.ts`
- Test: `lib/__tests__/content.test.ts`
- Test: `lib/__tests__/site.test.ts`

**Interfaces:**
- Consumes: `data/site-content.json`, `validateContentSiteCatalog`/`ContentSiteEntryT` (`@/contract/content-site`), `ContentKindT` (`@/contract/content-schema`), `getAll`/`slugForEntry` (`@/lib/catalog`), `SiteEntryT` (`@/contract/site`).
- Produces:
  - `lib/content.ts`: `getAllContent()`, `getContentByKind(kind)`, `slugForContent(e)`, `contentKindCounts()`, `flagshipContent(kind)`, `featuredContent(kind, n)`, `relatedContent(entry, n)`.
  - `lib/site.ts`: `type AnySiteEntry = SiteEntryT | ContentSiteEntryT`, `isContent(e): e is ContentSiteEntryT`, `slugForAny(e)`, `allSlugs(): string[][]`, `anyBySlug(slug): AnySiteEntry | undefined`.
  - `lib/i18n.ts`: new `Strings` keys `copyPrompt`, `copyScript`, `insertAleph`, `runAleph`, `mSource`, `mFormat`, `contentSoon`; exported `CONTENT_KIND_LABELS: Record<ContentKindT, { zh: string; en: string }>`.

- [ ] **Step 1: Write `lib/__tests__/content.test.ts` (failing)**

```ts
import { describe, it, expect } from "vitest";
import { getAllContent, getContentByKind, slugForContent, contentKindCounts, relatedContent } from "@/lib/content";

describe("lib/content", () => {
  it("loads content entries and filters by kind", () => {
    expect(getAllContent().length).toBeGreaterThan(0);
    const prompts = getContentByKind("prompt");
    expect(prompts.length).toBeGreaterThan(0);
    expect(prompts.every((e) => e.kind === "prompt")).toBe(true);
  });
  it("maps a content id to a path-safe slug (# -> /)", () => {
    const e = getContentByKind("prompt")[0];
    const slug = slugForContent(e);
    expect(slug.includes("#")).toBe(false);
    expect(slug).toBe(e.id.replace(/^aleph-hub:/, "").replace("#", "/"));
    // owner/repo/unit => 3 segments
    expect(slug.split("/").length).toBe(3);
  });
  it("counts kinds and never mutates inputs", () => {
    const counts = contentKindCounts();
    expect(counts.prompt).toBe(getContentByKind("prompt").length);
    expect(typeof counts.workflow).toBe("number");
  });
  it("related excludes self", () => {
    const e = getContentByKind("prompt")[0];
    expect(relatedContent(e, 3).every((r) => r.id !== e.id)).toBe(true);
  });
});
```

- [ ] **Step 2: Run to confirm it fails**

Run: `npx vitest run lib/__tests__/content.test.ts`
Expected: FAIL with "Cannot find module '@/lib/content'".

- [ ] **Step 3: Create `lib/content.ts`**

```ts
import contentData from "@/data/site-content.json";
import { validateContentSiteCatalog, type ContentSiteEntryT } from "@/contract/content-site";
import type { ContentKindT } from "@/contract/content-schema";

const CONTENT = validateContentSiteCatalog(contentData);

export function getAllContent(): ContentSiteEntryT[] { return CONTENT.entries; }
export function getContentByKind(kind: ContentKindT): ContentSiteEntryT[] {
  return CONTENT.entries.filter((e) => e.kind === kind);
}

// content id "aleph-hub:owner/repo#unit" <-> slug "owner/repo/unit": '#' -> '/' makes
// the unit addressable as a path segment (a literal '#' would be a URL fragment).
export function slugForContent(e: ContentSiteEntryT): string {
  return e.id.replace(/^aleph-hub:/, "").replace("#", "/");
}

export function contentKindCounts(): Record<ContentKindT, number> {
  return CONTENT.entries.reduce(
    (acc, e) => ({ ...acc, [e.kind]: (acc[e.kind] ?? 0) + 1 }),
    { prompt: 0, workflow: 0 } as Record<ContentKindT, number>,
  );
}

// Headline pick for a kind's home region: first entry (content has no star ranking
// yet); undefined when the axis is empty.
export function flagshipContent(kind: ContentKindT): ContentSiteEntryT | undefined {
  return getContentByKind(kind)[0];
}
// The remaining entries of a kind for the region's rows (excludes the flagship).
export function featuredContent(kind: ContentKindT, n: number): ContentSiteEntryT[] {
  return getContentByKind(kind).slice(1, 1 + n);
}
// Related = same category, excluding self (mirrors lib/catalog related()).
export function relatedContent(entry: ContentSiteEntryT, n: number): ContentSiteEntryT[] {
  return CONTENT.entries.filter((e) => e.category === entry.category && e.id !== entry.id).slice(0, n);
}
```

- [ ] **Step 4: Run to confirm it passes**

Run: `npx vitest run lib/__tests__/content.test.ts`
Expected: PASS.

- [ ] **Step 5: Write `lib/__tests__/site.test.ts` (failing)**

```ts
import { describe, it, expect } from "vitest";
import { isContent, slugForAny, allSlugs, anyBySlug } from "@/lib/site";
import { getAll, slugForEntry } from "@/lib/catalog";
import { getContentByKind, slugForContent } from "@/lib/content";

describe("lib/site", () => {
  it("resolves an install slug to its install entry", () => {
    const e = getAll()[0];
    const found = anyBySlug(slugForEntry(e));
    expect(found?.id).toBe(e.id);
    expect(isContent(found!)).toBe(false);
  });
  it("resolves a content slug to its content entry", () => {
    const e = getContentByKind("prompt")[0];
    const found = anyBySlug(slugForContent(e));
    expect(found?.id).toBe(e.id);
    expect(isContent(found!)).toBe(true);
  });
  it("slugForAny round-trips through anyBySlug for both families", () => {
    const install = getAll()[0];
    const content = getContentByKind("prompt")[0];
    expect(anyBySlug(slugForAny(install))?.id).toBe(install.id);
    expect(anyBySlug(slugForAny(content))?.id).toBe(content.id);
  });
  it("allSlugs covers install (2-seg) and content (3-seg) entries", () => {
    const slugs = allSlugs();
    expect(slugs.some((s) => s.length === 2)).toBe(true);
    expect(slugs.some((s) => s.length === 3)).toBe(true);
  });
});
```

- [ ] **Step 6: Run to confirm it fails**

Run: `npx vitest run lib/__tests__/site.test.ts`
Expected: FAIL with "Cannot find module '@/lib/site'".

- [ ] **Step 7: Create `lib/site.ts`**

```ts
import type { SiteEntryT } from "@/contract/site";
import type { ContentSiteEntryT } from "@/contract/content-site";
import { getAll, slugForEntry } from "@/lib/catalog";
import { getAllContent, slugForContent } from "@/lib/content";

// Unified browse model over the install catalog (skill|plugin|mcp) and the content
// catalog (prompt|workflow). The two kind value spaces are disjoint, so `isContent`
// narrows the union safely.
export type AnySiteEntry = SiteEntryT | ContentSiteEntryT;

export function isContent(e: AnySiteEntry): e is ContentSiteEntryT {
  return e.kind === "prompt" || e.kind === "workflow";
}

export function slugForAny(e: AnySiteEntry): string {
  return isContent(e) ? slugForContent(e) : slugForEntry(e);
}

// Canonical forward-slug map across both catalogs. install full_name is always
// 2 segments and content unit slugs never contain '/', so:
//   install "owner/repo" (2 seg) and content "owner/repo/unit" (3 seg) never collide.
// Keying by the precomputed forward slug avoids any reverse '/'->'#' guesswork.
const entries: Array<[string, AnySiteEntry]> = [
  ...getAll().map((e): [string, AnySiteEntry] => [slugForEntry(e), e]),
  ...getAllContent().map((e): [string, AnySiteEntry] => [slugForContent(e), e]),
];
const BY_SLUG = new Map<string, AnySiteEntry>(entries);

export function allSlugs(): string[][] {
  return [...BY_SLUG.keys()].map((s) => s.split("/"));
}
export function anyBySlug(slug: string): AnySiteEntry | undefined {
  return BY_SLUG.get(slug);
}
```

- [ ] **Step 8: Run to confirm it passes**

Run: `npx vitest run lib/__tests__/site.test.ts`
Expected: PASS.

- [ ] **Step 9: Add content i18n strings + labels**

In `lib/i18n.ts`: import `ContentKindT`, extend the `Strings` interface, add the new keys to both `zh` and `en`, and export `CONTENT_KIND_LABELS`.

Add to the top imports:
```ts
import type { ContentKindT } from "@/contract/content-schema";
```

Add these fields to the `Strings` interface (append to the existing list, e.g. after `viewGithub: string; related: string; copy: string; copied: string;`):
```ts
  copyPrompt: string; copyScript: string; insertAleph: string; runAleph: string;
  mSource: string; mFormat: string; contentSoon: string;
```

Add to `STRINGS.zh` (place alongside the related copy fields):
```ts
    copyPrompt: "复制提示词", copyScript: "复制脚本", insertAleph: "在 Aleph 中插入", runAleph: "在 Aleph 中保存并运行",
    mSource: "来源文件", mFormat: "格式", contentSoon: "更多内容正在整理中，敬请期待。",
```

Add to `STRINGS.en`:
```ts
    copyPrompt: "Copy prompt", copyScript: "Copy script", insertAleph: "Insert in Aleph", runAleph: "Save & run in Aleph",
    mSource: "Source", mFormat: "Format", contentSoon: "More are being curated — stay tuned.",
```

Add at the end of the file (after `CATEGORY_LABELS`):
```ts
// Display names for the two content kinds (used by the home index region + kind pages).
export const CONTENT_KIND_LABELS: Record<ContentKindT, { zh: string; en: string }> = {
  prompt: { zh: "提示词", en: "Prompts" },
  workflow: { zh: "工作流", en: "Workflows" },
};
```

- [ ] **Step 10: Run the data-layer tests + typecheck**

Run: `npx vitest run lib/__tests__/content.test.ts lib/__tests__/site.test.ts`
Expected: PASS.
Run: `npm run typecheck`
Expected: clean.

- [ ] **Step 11: Commit**

```bash
git add lib/content.ts lib/site.ts lib/i18n.ts lib/__tests__/content.test.ts lib/__tests__/site.test.ts
git commit -m "feat(site): content data layer, unified slug resolver, content i18n"
```

---

### Task 3: Card union support (kind-aware footer + link)

`Card` must render both install and content entries. Install footer keeps sparkline/stars/trend; content footer shows the payload `format` (no stars).

**Files:**
- Modify: `components/Card.tsx`
- Test: `components/__tests__/card.test.tsx`

**Interfaces:**
- Consumes: `AnySiteEntry`/`isContent`/`slugForAny` (`@/lib/site`), `formatStars` (`@/lib/catalog`), `getContentByKind` (`@/lib/content`, test only).
- Produces: `Card` accepts `{ entry: AnySiteEntry; rank?: number }`.

- [ ] **Step 1: Write the failing content-card test**

Replace `components/__tests__/card.test.tsx` content with (keeps an install assertion, adds a content one):

```ts
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ThemeProvider } from "@/components/providers/ThemeProvider";
import { LangProvider } from "@/components/providers/LangProvider";
import { Card } from "@/components/Card";
import { getByKind } from "@/lib/catalog";
import { getContentByKind } from "@/lib/content";

const wrap = (ui: React.ReactNode) => render(<ThemeProvider><LangProvider>{ui}</LangProvider></ThemeProvider>);

describe("Card", () => {
  it("renders an install entry with its star count and detail link", () => {
    const e = getByKind("mcp")[0];
    wrap(<Card entry={e} />);
    expect(screen.getByText(e.name)).toBeInTheDocument();
    const link = screen.getByRole("link");
    expect(link.getAttribute("href")).toBe(`/e/${e.id.replace(/^aleph-hub:/, "")}`);
  });
  it("renders a content entry with its format and a path-safe detail link", () => {
    const e = getContentByKind("prompt")[0];
    wrap(<Card entry={e} />);
    expect(screen.getByText(e.name)).toBeInTheDocument();
    expect(screen.getByText(e.format)).toBeInTheDocument();
    const link = screen.getByRole("link");
    expect(link.getAttribute("href")).toBe(`/e/${e.id.replace(/^aleph-hub:/, "").replace("#", "/")}`);
  });
});
```

- [ ] **Step 2: Run to confirm it fails**

Run: `npx vitest run components/__tests__/card.test.tsx`
Expected: FAIL (content entry not assignable / format text absent).

- [ ] **Step 3: Rewrite `components/Card.tsx`**

```tsx
"use client";
import Link from "next/link";
import { useLang } from "@/components/providers/LangProvider";
import { formatStars } from "@/lib/catalog";
import { type AnySiteEntry, isContent, slugForAny } from "@/lib/site";
import { TrustBadge } from "@/components/TrustBadge";
import { Sparkline } from "@/components/Sparkline";

export function Card({ entry, rank }: { entry: AnySiteEntry; rank?: number }) {
  const { lang } = useLang();
  const desc = lang === "zh" ? entry.description_zh : entry.description_en;
  return (
    <Link href={`/e/${slugForAny(entry)}`} className="cat-card" style={{ display: "block", textDecoration: "none", color: "inherit", background: "var(--panel)", border: "1px solid var(--hair)", borderRadius: 3, padding: 20 }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: 13 }}>
        <div style={{ display: "flex", gap: 12, alignItems: "baseline", minWidth: 0 }}>
          {rank != null && <span style={{ fontFamily: "var(--font-mono), monospace", fontSize: 12, color: "var(--orange)", flex: "none" }}>{String(rank).padStart(2, "0")}</span>}
          <div style={{ minWidth: 0 }}>
            <div style={{ fontFamily: "var(--font-mono), monospace", fontSize: 15, fontWeight: 500 }}>{entry.name}</div>
            <div style={{ fontSize: 11, color: "var(--taupe)", marginTop: 3 }}>{entry.author}</div>
          </div>
        </div>
        <TrustBadge tier={entry.trust_tier} />
      </div>
      <p style={{ fontSize: 13, lineHeight: 1.5, color: "var(--ink-soft)", margin: "0 0 16px", minHeight: 39 }}>{desc}</p>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", paddingTop: 14, borderTop: "1px solid var(--hair)" }}>
        <span style={{ fontSize: 10, letterSpacing: ".08em", textTransform: "uppercase", color: "var(--ink-soft)", background: "var(--chip)", padding: "3px 8px", borderRadius: 2 }}>{entry.kind}</span>
        {isContent(entry) ? (
          // content has no stars/trend; surface the payload format instead.
          <span style={{ fontFamily: "var(--font-mono), monospace", fontSize: 11, color: "var(--ink-soft)" }}>{entry.format}</span>
        ) : (
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <Sparkline points={entry.spark} color={(entry.trend ?? 0) >= 15 ? "var(--green)" : "var(--taupe)"} />
            <span style={{ fontFamily: "var(--font-mono), monospace", fontSize: 12 }}>★{formatStars(entry.stars)}</span>
            {entry.trend != null && <span style={{ fontSize: 11, fontWeight: 600, color: (entry.trend ?? 0) >= 15 ? "var(--green)" : "var(--taupe)" }}>▲{entry.trend}%</span>}
          </div>
        )}
      </div>
    </Link>
  );
}
```

- [ ] **Step 4: Run to confirm it passes + typecheck**

Run: `npx vitest run components/__tests__/card.test.tsx`
Expected: PASS.
Run: `npm run typecheck`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add components/Card.tsx components/__tests__/card.test.tsx
git commit -m "feat(site): kind-aware Card for content entries"
```

---

### Task 4: Browse — kind pages for prompt/workflow

`/c/[kind]` gains `prompt`/`workflow`; `CategoryView` branches its data source and titles.

**Files:**
- Modify: `app/c/[kind]/page.tsx`
- Modify: `components/category/CategoryView.tsx`
- Test: `components/category/__tests__/category.test.tsx`

**Interfaces:**
- Consumes: `getByKind` (`@/lib/catalog`), `getContentByKind` (`@/lib/content`), `AnySiteEntry` (`@/lib/site`), `ContentKindT` (`@/contract/content-schema`), `ExtensionKindT` (`@/contract/types`).
- Produces: `CategoryView` accepts `{ kind: ExtensionKindT | ContentKindT }`.

- [ ] **Step 1: Write the failing content-kind browse test**

Append to `components/category/__tests__/category.test.tsx` a new `it` inside the existing `describe`:

```ts
it("lists prompt entries on the content kind page", () => {
  const prompts = getContentByKind("prompt");
  expect(prompts.length).toBeGreaterThan(0);
  wrap(<CategoryView kind="prompt" />);
  expect(screen.getAllByText(prompts[0].name).length).toBeGreaterThan(0);
});
```

Add the import at the top of the test file:
```ts
import { getContentByKind } from "@/lib/content";
```

- [ ] **Step 2: Run to confirm it fails**

Run: `npx vitest run components/category/__tests__/category.test.tsx`
Expected: FAIL (`kind="prompt"` not assignable / entry absent).

- [ ] **Step 3: Update `components/category/CategoryView.tsx`**

Change the kind type, title map, and data source. Replace the imports + the `KIND_TITLE`/signature/`all` lines:

```tsx
"use client";
import { useState } from "react";
import type { ExtensionKindT } from "@/contract/types";
import type { ContentKindT } from "@/contract/content-schema";
import { useLang } from "@/components/providers/LangProvider";
import { STRINGS } from "@/lib/i18n";
import { getByKind } from "@/lib/catalog";
import { getContentByKind } from "@/lib/content";
import type { AnySiteEntry } from "@/lib/site";
import { Card } from "@/components/Card";
import { Header } from "@/components/Header";
import { Footer } from "@/components/Footer";

type AnyKind = ExtensionKindT | ContentKindT;
const CATS = ["search", "developer", "data", "productivity", "writing", "communication", "knowledge", "files", "design", "automation", "finance", "utilities", "other"];
const KIND_TITLE: Record<AnyKind, string> = { skill: "Agent Skills", plugin: "Plugins", mcp: "MCP Servers", prompt: "Prompts", workflow: "Workflows" };
function isContentKind(k: AnyKind): k is ContentKindT { return k === "prompt" || k === "workflow"; }

export function CategoryView({ kind }: { kind: AnyKind }) {
  const { lang } = useLang();
  const t = STRINGS[lang];
  const [q, setQ] = useState("");
  const [cat, setCat] = useState<string>("all");
  const all: AnySiteEntry[] = isContentKind(kind) ? getContentByKind(kind) : getByKind(kind);
  const query = q.trim().toLowerCase();
  const visible = all
    .filter((e) => cat === "all" || e.category === cat)
    .filter((e) => !query || `${e.name} ${e.description_en} ${e.description_zh} ${e.tags.join(" ")}`.toLowerCase().includes(query));
```

Leave the entire JSX return below unchanged — `Card` now accepts `AnySiteEntry`, and every referenced field (`name`, `description_*`, `tags`, `category`) exists on both families.

- [ ] **Step 4: Update `app/c/[kind]/page.tsx`**

```tsx
import { notFound } from "next/navigation";
import type { ExtensionKindT } from "@/contract/types";
import type { ContentKindT } from "@/contract/content-schema";
import { CategoryView } from "@/components/category/CategoryView";

type AnyKind = ExtensionKindT | ContentKindT;
const KINDS: AnyKind[] = ["skill", "plugin", "mcp", "prompt", "workflow"];
export function generateStaticParams() { return KINDS.map((kind) => ({ kind })); }

export default async function Page({ params }: { params: Promise<{ kind: string }> }) {
  const { kind } = await params;
  if (!KINDS.includes(kind as AnyKind)) notFound();
  return <CategoryView kind={kind as AnyKind} />;
}
```

- [ ] **Step 5: Run to confirm it passes + typecheck**

Run: `npx vitest run components/category/__tests__/category.test.tsx`
Expected: PASS (both the existing mcp cases and the new prompt case).
Run: `npm run typecheck`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add app/c/[kind]/page.tsx components/category/CategoryView.tsx components/category/__tests__/category.test.tsx
git commit -m "feat(site): prompt/workflow kind pages"
```

---

### Task 5: Home — content index regions (Prompts, Workflows)

Add two index regions below the existing three, driven by content data. A separate `ContentIndex` component keeps the install-typed `CategoryIndex` untouched (lower risk; "many small files"); composed into the home it delivers the unified 5-region index.

**Files:**
- Create: `components/home/ContentIndex.tsx`
- Modify: `app/page.tsx`
- Test: `components/home/__tests__/content-index.test.tsx`

**Interfaces:**
- Consumes: `contentKindCounts`/`flagshipContent`/`featuredContent`/`slugForContent` (`@/lib/content`), `STRINGS`/`CONTENT_KIND_LABELS` (`@/lib/i18n`), `ContentKindT` (`@/contract/content-schema`), `ContentSiteEntryT` (`@/contract/content-site`).
- Produces: `<ContentIndex />` (default exportless named component).

- [ ] **Step 1: Write the failing ContentIndex test**

```ts
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ThemeProvider } from "@/components/providers/ThemeProvider";
import { LangProvider } from "@/components/providers/LangProvider";
import { ContentIndex } from "@/components/home/ContentIndex";
import { flagshipContent } from "@/lib/content";

const wrap = (ui: React.ReactNode) => render(<ThemeProvider><LangProvider>{ui}</LangProvider></ThemeProvider>);

describe("ContentIndex", () => {
  it("renders a Prompts region with the flagship prompt", () => {
    wrap(<ContentIndex />);
    // region label (zh default) for the prompt axis
    expect(screen.getByText("提示词")).toBeInTheDocument();
    const flagship = flagshipContent("prompt");
    expect(flagship).toBeDefined();
    expect(screen.getAllByText(flagship!.name).length).toBeGreaterThan(0);
  });
});
```

- [ ] **Step 2: Run to confirm it fails**

Run: `npx vitest run components/home/__tests__/content-index.test.tsx`
Expected: FAIL with "Cannot find module '@/components/home/ContentIndex'".

- [ ] **Step 3: Create `components/home/ContentIndex.tsx`**

```tsx
"use client";
import Link from "next/link";
import type { ContentKindT } from "@/contract/content-schema";
import type { ContentSiteEntryT } from "@/contract/content-site";
import { useLang } from "@/components/providers/LangProvider";
import { STRINGS, CONTENT_KIND_LABELS } from "@/lib/i18n";
import { contentKindCounts, flagshipContent, featuredContent, slugForContent } from "@/lib/content";

// The two content axes continue the home Index below the three install axes.
// Numbers 04/05 follow skill(01)/mcp(02)/plugin(03).
const META: Record<ContentKindT, { num: string; zhTag: string; enTag: string }> = {
  prompt: { num: "04", zhTag: "即用型提示词", enTag: "Copy-ready prompts" },
  workflow: { num: "05", zhTag: "可运行的 Agent 工作流", enTag: "Runnable agent workflows" },
};
const ORDER: ContentKindT[] = ["prompt", "workflow"];

export function ContentIndex() {
  const { lang } = useLang();
  const t = STRINGS[lang];
  const counts = contentKindCounts();
  const descOf = (e: ContentSiteEntryT) => (lang === "zh" ? e.description_zh : e.description_en);
  const nameOf = (k: ContentKindT) => (lang === "zh" ? CONTENT_KIND_LABELS[k].zh : CONTENT_KIND_LABELS[k].en);

  const head = (k: ContentKindT) => (
    <div style={{ display: "flex", justifyContent: "space-between", alignItems: "baseline", gap: 16, marginBottom: 18 }}>
      <span style={{ display: "flex", gap: 18, alignItems: "baseline", minWidth: 0 }}>
        <span style={{ fontFamily: "var(--font-mono), monospace", fontSize: 12, color: "var(--orange)" }}>{META[k].num}</span>
        <span style={{ fontFamily: "var(--font-cormorant), var(--font-noto-serif-sc), serif", fontSize: 30 }}>{nameOf(k)}</span>
        <span style={{ fontSize: 13, color: "var(--taupe)" }}>{lang === "zh" ? META[k].zhTag : META[k].enTag}</span>
      </span>
      <Link href={`/c/${k}`} className="sec-more" style={{ display: "flex", alignItems: "center", gap: 14, textDecoration: "none", color: "var(--taupe)", flex: "none" }}>
        <span style={{ fontFamily: "var(--font-mono), monospace", fontSize: 13, color: "var(--ink-soft)" }}>{counts[k]}</span>
        <span style={{ fontSize: 11, letterSpacing: ".10em", textTransform: "uppercase" }}>{t.viewAll} →</span>
      </Link>
    </div>
  );

  const featRow = (e: ContentSiteEntryT) => (
    <Link key={e.id} href={`/e/${slugForContent(e)}`} className="idx-feat-row" style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: 16, padding: "13px 0", borderTop: "1px solid var(--hair)", textDecoration: "none", color: "inherit" }}>
      <span style={{ display: "flex", alignItems: "baseline", gap: 11, minWidth: 0 }}>
        <span className="idx-feat-name" style={{ fontFamily: "var(--font-mono), monospace", fontSize: 14, color: "var(--ink)", whiteSpace: "nowrap" }}>{e.name}</span>
        <span style={{ fontSize: 12, color: "var(--taupe)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{e.author}</span>
      </span>
      <span style={{ fontFamily: "var(--font-mono), monospace", fontSize: 11.5, color: "var(--ink-soft)", flex: "none" }}>{e.format}</span>
    </Link>
  );

  const mainCard = (e: ContentSiteEntryT) => (
    <Link href={`/e/${slugForContent(e)}`} className="idx-main idx-spotlight" style={{ display: "flex", gap: 16, alignItems: "center", background: "var(--panel)", border: "1px solid var(--hair)", borderRadius: 3, padding: 14, textDecoration: "none", color: "inherit" }}>
      <div style={{ width: 74, height: 74, flex: "none", borderRadius: 2, background: e.cover_color, display: "flex", alignItems: "center", justifyContent: "center" }}>
        <span style={{ fontFamily: "var(--font-cormorant), serif", fontSize: 44, color: "rgba(255,255,255,.92)", lineHeight: 1 }}>{e.name[0]}</span>
      </div>
      <div style={{ minWidth: 0 }}>
        <div style={{ fontFamily: "var(--font-cormorant), var(--font-noto-serif-sc), serif", fontWeight: 600, fontSize: 21, marginBottom: 4 }}>{e.name}</div>
        <p style={{ fontSize: 12.5, lineHeight: 1.4, color: "var(--ink-soft)", margin: "0 0 6px", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{descOf(e)}</p>
        <span style={{ fontFamily: "var(--font-mono), monospace", fontSize: 11.5, color: "var(--ink-soft)" }}>{e.author} · {e.format}</span>
      </div>
    </Link>
  );

  // Empty axis (e.g. workflow before any are curated): a quiet coming-soon note.
  const emptyRegion = (
    <div style={{ minWidth: 0 }}>
      <div style={{ fontFamily: "var(--font-mono), monospace", fontSize: 11, letterSpacing: ".06em", textTransform: "uppercase", color: "var(--taupe)", marginBottom: 7 }}>{t.comingSoon}</div>
      <p style={{ fontSize: 14, lineHeight: 1.55, color: "var(--ink-soft)", margin: 0, maxWidth: "42ch" }}>{t.contentSoon}</p>
    </div>
  );

  const region = (k: ContentKindT) => {
    const main = flagshipContent(k);
    if (!main) return emptyRegion;
    const rows = featuredContent(k, 6);
    return (
      <div className="idx-region-grid" style={{ display: "grid", gridTemplateColumns: "minmax(0, 1fr) minmax(0, 1fr)", gap: "0 44px", alignItems: "start", gridAutoFlow: "row dense" }}>
        {mainCard(main)}
        {rows.map((e) => featRow(e))}
      </div>
    );
  };

  return (
    <section style={{ maxWidth: 1480, margin: "0 auto", padding: "0 48px 8px" }}>
      {ORDER.map((k) => (
        <div key={k} style={{ padding: "30px 0", borderTop: "1px solid var(--hair)" }}>
          {head(k)}
          {region(k)}
        </div>
      ))}
    </section>
  );
}
```

- [ ] **Step 4: Compose into `app/page.tsx`**

Add the import and render `<ContentIndex />` immediately after `<CategoryIndex />`:

```tsx
import { CategoryIndex } from "@/components/home/CategoryIndex";
import { ContentIndex } from "@/components/home/ContentIndex";
```
```tsx
        <CategoryIndex />
        <ContentIndex />
        <Trending />
```

- [ ] **Step 5: Run to confirm it passes + typecheck**

Run: `npx vitest run components/home/__tests__/content-index.test.tsx components/home/__tests__/home.test.tsx`
Expected: PASS (new region test + the existing home render still green).
Run: `npm run typecheck`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add components/home/ContentIndex.tsx app/page.tsx components/home/__tests__/content-index.test.tsx
git commit -m "feat(site): home index Prompts/Workflows regions"
```

---

### Task 6: Detail — content detail view + unified `/e` routing

`/e/[...slug]` resolves either family; content entries render `ContentDetailView` (body + Copy + provenance + an Aleph action placeholder).

**Files:**
- Create: `components/detail/ContentDetailView.tsx`
- Modify: `app/e/[...slug]/page.tsx`
- Test: `components/detail/__tests__/content-detail.test.tsx`

**Interfaces:**
- Consumes: `ContentSiteEntryT` (`@/contract/content-site`), `relatedContent` (`@/lib/content`), `allSlugs`/`anyBySlug`/`isContent` (`@/lib/site`), `STRINGS`/`CATEGORY_LABELS` (`@/lib/i18n`), `TrustBadge`, `Card`.
- Produces: `<ContentDetailView entry={ContentSiteEntryT} />`.

- [ ] **Step 1: Write the failing content-detail test**

```ts
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ThemeProvider } from "@/components/providers/ThemeProvider";
import { LangProvider } from "@/components/providers/LangProvider";
import { ContentDetailView } from "@/components/detail/ContentDetailView";
import { getContentByKind } from "@/lib/content";

const wrap = (ui: React.ReactNode) => render(<ThemeProvider><LangProvider>{ui}</LangProvider></ThemeProvider>);

describe("ContentDetailView", () => {
  it("renders the body, provenance, and copies the body to the clipboard", () => {
    const e = getContentByKind("prompt")[0];
    const writeText = vi.fn();
    Object.assign(navigator, { clipboard: { writeText } });
    wrap(<ContentDetailView entry={e} />);
    // name + body text present
    expect(screen.getByRole("heading", { name: e.name })).toBeInTheDocument();
    expect(screen.getByText(e.body)).toBeInTheDocument();
    // provenance link points at the source file in the repo
    const src = screen.getByRole("link", { name: /来源文件|Source/ });
    expect(src.getAttribute("href")).toBe(`${e.repo_url}/blob/HEAD/${e.source_path}`);
    // copy button copies the body verbatim
    fireEvent.click(screen.getByText(/复制提示词|Copy prompt/));
    expect(writeText).toHaveBeenCalledWith(e.body);
  });
});
```

- [ ] **Step 2: Run to confirm it fails**

Run: `npx vitest run components/detail/__tests__/content-detail.test.tsx`
Expected: FAIL with "Cannot find module '@/components/detail/ContentDetailView'".

- [ ] **Step 3: Create `components/detail/ContentDetailView.tsx`**

```tsx
"use client";
import { useState } from "react";
import Link from "next/link";
import type { ContentSiteEntryT } from "@/contract/content-site";
import { useLang } from "@/components/providers/LangProvider";
import { STRINGS, CATEGORY_LABELS } from "@/lib/i18n";
import { relatedContent } from "@/lib/content";
import { TrustBadge } from "@/components/TrustBadge";
import { Card } from "@/components/Card";

// Content sibling of DetailView. Same shell (cover, tabs, sidebar, related), but the
// action is copy-the-body (prompt → copy/insert, workflow → save & run), and the
// sidebar shows provenance (repo + source file) instead of an install command.
export function ContentDetailView({ entry }: { entry: ContentSiteEntryT }) {
  const { lang } = useLang();
  const t = STRINGS[lang];
  const [tab, setTab] = useState<"overview" | "security">("overview");
  const [copied, setCopied] = useState(false);
  const desc = lang === "zh" ? entry.description_zh : entry.description_en;
  const long = lang === "zh" ? entry.long_zh : entry.long_en;
  const secNote = lang === "zh" ? entry.sec_note_zh : entry.sec_note_en;
  const isPrompt = entry.kind === "prompt";
  const copyLabel = isPrompt ? t.copyPrompt : t.copyScript;
  const actionLabel = isPrompt ? t.insertAleph : t.runAleph;
  const sourceUrl = `${entry.repo_url}/blob/HEAD/${entry.source_path}`;

  const copy = () => {
    try { navigator.clipboard?.writeText(entry.body); setCopied(true); setTimeout(() => setCopied(false), 1500); } catch {}
  };
  const tabStyle = (active: boolean) => ({ fontSize: 13, padding: "6px 2px", cursor: "pointer", fontWeight: 600, color: active ? "var(--ink)" : "var(--taupe)", borderBottom: active ? "2px solid var(--orange)" : "2px solid transparent", marginBottom: -9 });
  const metaRow = { display: "flex", justifyContent: "space-between", alignItems: "baseline" } as const;
  const metaKey = { fontSize: 11, letterSpacing: ".10em", textTransform: "uppercase" as const, color: "var(--taupe)" };
  const metaVal = { fontSize: 13, fontWeight: 500 } as const;

  return (
    <main style={{ maxWidth: 1240, margin: "0 auto", padding: "34px 48px 80px" }}>
      <Link href="/" style={{ fontSize: 12, color: "var(--taupe)", letterSpacing: ".04em", textDecoration: "none" }}>{t.back}</Link>
      <section style={{ display: "grid", gridTemplateColumns: "1fr 320px", gap: 48, marginTop: 26, alignItems: "start" }}>
        <div>
          <div style={{ height: 220, background: entry.cover_color, borderRadius: 3, display: "flex", alignItems: "center", justifyContent: "center", marginBottom: 28 }}>
            <span style={{ fontFamily: "var(--font-cormorant), serif", fontSize: 150, color: "rgba(255,255,255,.92)", lineHeight: 1 }}>{entry.name[0]}</span>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 14, marginBottom: 13 }}>
            <TrustBadge tier={entry.trust_tier} />
            <span style={{ fontSize: 11, letterSpacing: ".10em", textTransform: "uppercase", color: "var(--orange)", fontWeight: 600 }}>{entry.kind}</span>
          </div>
          <h1 style={{ fontFamily: "var(--font-cormorant), serif", fontWeight: 600, fontSize: 52, lineHeight: 1, margin: "0 0 10px" }}>{entry.name}</h1>
          <p style={{ fontSize: 18, color: "var(--ink-soft)", margin: "0 0 26px" }}>{desc}</p>
          <div style={{ display: "flex", gap: 18, paddingBottom: 8, borderBottom: "1px solid var(--hair)", marginBottom: 22 }}>
            <span style={tabStyle(tab === "overview")} onClick={() => setTab("overview")}>{t.tabOverview}</span>
            <span style={tabStyle(tab === "security")} onClick={() => setTab("security")}>{t.tabSecurity}</span>
          </div>
          {tab === "overview" ? (
            <>
              <p style={{ fontSize: 15.5, lineHeight: 1.75, color: "var(--ink)", margin: "0 0 22px", whiteSpace: "pre-line" }}>{long}</p>
              {/* inline payload: prompt text or the .js source, rendered verbatim (no highlighter). */}
              <pre style={{ whiteSpace: "pre-wrap", wordBreak: "break-word", fontFamily: "var(--font-mono), monospace", fontSize: 13, lineHeight: 1.6, color: "var(--ink)", background: "var(--chip)", border: "1px solid var(--hair)", borderRadius: 3, padding: 16, margin: "0 0 24px", overflowX: "auto" }}><code>{entry.body}</code></pre>
              <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                {entry.tags.map((tg) => (
                  <span key={tg} style={{ fontSize: 12, fontFamily: "var(--font-mono), monospace", color: "var(--ink-soft)", background: "var(--chip)", padding: "5px 11px", borderRadius: 2 }}>#{tg}</span>
                ))}
              </div>
            </>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 14 }}>
              <div style={{ display: "flex", gap: 14, alignItems: "flex-start", padding: "16px 18px", background: "var(--panel)", border: "1px solid var(--hair)", borderRadius: 3 }}>
                <span style={{ color: "var(--green)", fontSize: 18 }}>✓</span>
                <div><div style={{ fontWeight: 600, fontSize: 14, marginBottom: 3 }}>{t.secScan}</div><div style={{ fontSize: 13, color: "var(--ink-soft)", lineHeight: 1.5 }}>{secNote}</div></div>
              </div>
              <div style={{ display: "flex", gap: 14, alignItems: "flex-start", padding: "16px 18px", background: "var(--panel)", border: "1px solid var(--hair)", borderRadius: 3 }}>
                <span style={{ color: "var(--orange)", fontSize: 18 }}>◷</span>
                <div><div style={{ fontWeight: 600, fontSize: 14, marginBottom: 3 }}>{t.secReview}</div><div style={{ fontSize: 13, color: "var(--ink-soft)", lineHeight: 1.5 }}>{t.secReviewNote}</div></div>
              </div>
            </div>
          )}
        </div>
        <aside style={{ position: "sticky", top: 90, display: "flex", flexDirection: "column" }}>
          <span onClick={copy} style={{ textAlign: "center", fontSize: 12, letterSpacing: ".14em", textTransform: "uppercase", fontWeight: 600, color: "var(--bg)", background: "var(--ink)", padding: 13, borderRadius: 2, cursor: "pointer" }}>{copied ? t.copied : copyLabel}</span>
          {/* Aleph-side action is a placeholder until the content-library consumer ships (separate ../Aleph spec). */}
          <span title={t.comingSoon} style={{ textAlign: "center", fontSize: 12, letterSpacing: ".10em", textTransform: "uppercase", fontWeight: 600, color: "var(--taupe)", border: "1px solid var(--hair-strong)", padding: 12, borderRadius: 2, marginTop: 10, cursor: "default" }}>{actionLabel}</span>
          <div style={{ borderTop: "1px solid var(--hair)", marginTop: 26, paddingTop: 20, display: "flex", flexDirection: "column", gap: 16 }}>
            <div style={metaRow}><span style={metaKey}>{t.mBy}</span><span style={metaVal}>{entry.author}</span></div>
            <div style={metaRow}><span style={metaKey}>{t.mCategory}</span><span style={metaVal}>{CATEGORY_LABELS[entry.category][lang]}</span></div>
            <div style={metaRow}><span style={metaKey}>{t.mLicense}</span><span style={metaVal}>{entry.license ?? "—"}</span></div>
            <div style={metaRow}><span style={metaKey}>{t.mFormat}</span><span style={{ ...metaVal, fontFamily: "var(--font-mono), monospace" }}>{entry.format}</span></div>
            <div style={metaRow}>
              <span style={metaKey}>{t.mSource}</span>
              {/* aria-label gives the link a stable accessible name (the source file path is truncated visually). */}
              <Link href={sourceUrl} aria-label={t.mSource} target="_blank" rel="noreferrer" style={{ fontSize: 12, fontFamily: "var(--font-mono), monospace", color: "var(--orange)", textDecoration: "none", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", maxWidth: 180 }}>{entry.source_path}</Link>
            </div>
          </div>
          <a href={entry.repo_url} target="_blank" rel="noreferrer" style={{ marginTop: 22, fontSize: 12, letterSpacing: ".10em", textTransform: "uppercase", color: "var(--orange)", border: "1px solid var(--orange)", padding: 11, borderRadius: 2, textAlign: "center", textDecoration: "none" }}>{t.viewGithub} ↗</a>
        </aside>
      </section>
      <section style={{ marginTop: 60 }}>
        <div style={{ fontSize: 11, letterSpacing: ".20em", textTransform: "uppercase", color: "var(--ink-soft)", fontWeight: 600, borderBottom: "1px solid var(--hair-strong)", paddingBottom: 11, marginBottom: 24 }}>{t.related}</div>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(3,1fr)", gap: 20 }}>
          {relatedContent(entry, 3).map((e) => <Card key={e.id} entry={e} />)}
        </div>
      </section>
    </main>
  );
}
```

- [ ] **Step 4: Update `app/e/[...slug]/page.tsx`**

```tsx
import { notFound } from "next/navigation";
import { allSlugs, anyBySlug, isContent } from "@/lib/site";
import { Header } from "@/components/Header";
import { Footer } from "@/components/Footer";
import { DetailView } from "@/components/detail/DetailView";
import { ContentDetailView } from "@/components/detail/ContentDetailView";

export function generateStaticParams() {
  return allSlugs().map((slug) => ({ slug }));
}

export default async function Page({ params }: { params: Promise<{ slug: string[] }> }) {
  const { slug } = await params;
  const entry = anyBySlug(slug.join("/"));
  if (!entry) notFound();
  return (
    <>
      <Header />
      {isContent(entry) ? <ContentDetailView entry={entry} /> : <DetailView entry={entry} />}
      <Footer />
    </>
  );
}
```

- [ ] **Step 5: Run to confirm it passes + typecheck**

Run: `npx vitest run components/detail/__tests__/content-detail.test.tsx components/detail/__tests__/detail.test.tsx`
Expected: PASS (new content detail + existing install detail).
Run: `npm run typecheck`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add components/detail/ContentDetailView.tsx app/e/[...slug]/page.tsx components/detail/__tests__/content-detail.test.tsx
git commit -m "feat(site): content detail view with copy + provenance"
```

---

### Task 7: Full-suite + production build verification

A whole-website integration gate: the route changes (`generateStaticParams` over 5 kinds + content slugs) only fully exercise under a Next build.

**Files:** none (verification only).

- [ ] **Step 1: Run the full unit suite**

Run: `npm test`
Expected: all tests pass (the 136 Plan-1 baseline plus the new content tests).

- [ ] **Step 2: Typecheck**

Run: `npm run typecheck`
Expected: clean.

- [ ] **Step 3: Production build (static params over both catalogs)**

Run: `npm run build`
Expected: build succeeds; the route tree shows `/c/[kind]` and `/e/[...slug]` prerendered including `prompt`/`workflow` and the content slug `dair-ai/Prompt-Engineering-Guide/text-summarize`.

- [ ] **Step 4: Confirm the install contract is untouched**

Run: `npm run validate:catalog`
Expected: `catalog OK` with the unchanged install entry count + `schema_version 1`.
Run: `git diff --stat main -- public/catalog.json contract/schema.ts contract/site.ts contract/types.ts`
Expected: no output (these are byte-for-byte unchanged on this branch).

- [ ] **Step 5: Commit (only if the build emitted tracked changes; otherwise skip)**

```bash
git status --short
# If only untracked build output (.next/) — do not commit it; it is git-ignored.
```

---

## Notes on scope (explicit non-goals for this plan)

- **Home StatsBar / Hero counts stay install-scoped.** `getAll().length` (install, ~125) still drives the projects stat and the "Browse all {n}" CTA. Folding content into those counts is a product decision outside §6's enumerated website changes; left unchanged to keep the diff surgical.
- **No syntax highlighting / markdown rendering.** The `body` renders verbatim in a `<pre>` (YAGNI / no new dependency). A highlighter is an additive future change.
- **The Aleph action button is a labeled placeholder** ("Insert in Aleph" / "Save & run in Aleph") — the actual deep-link/runner is the separate `../Aleph` content-library spec. It carries a `coming soon` title and no handler.
- **`workflow` axis renders an empty "coming soon" region** until workflow entries are curated (Plan 3 adds workflow discovery). The home region and `/c/workflow` both degrade gracefully to zero entries.
- **Weekly cron partition + workflow discovery are Plan 3**, not here.
```
