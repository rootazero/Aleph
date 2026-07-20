# Aleph Hub — Contract + Website (Milestone 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a deployable, pixel-faithful Next.js boutique catalog site that renders a hand-authored fixture catalog and validates that fixture against the exact Aleph wire contract.

**Architecture:** Phase A builds a `contract/` module — a zod schema that is the single source of truth for the `catalog.json` wire format (mirrors Aleph's `src/hub/` serde types) plus a richer site-data schema. Phase B ports the `Aleph Hub.dc.html` mockup (DCLogic) into React Server/Client components reading a fixture `data/site-catalog.json`, producing a static (SSG) site Vercel deploys. No pipeline, no LLM, no network yet — those are later plans.

**Tech Stack:** Next.js 15 (App Router) · React 19 · TypeScript (strict) · zod (validation) · vitest + @testing-library/react (unit/component) · Playwright (e2e) · next/font (Google fonts).

## Global Constraints

- **Code + comments in English** (project convention; prose/docs in Chinese). Commit messages: `<scope>: <description>` English, conventional types (feat/fix/docs/test/chore/refactor).
- **No attribution footer** in commits (disabled globally).
- **Contract is law:** the wire format MUST match Aleph `src/hub/{hub_catalog.rs,types.rs}`. Enums are snake_case. `kind` ∈ `skill|plugin|mcp`. `category` ∈ the 13 values. `trust_tier` ∈ `official|verified|community|unverified`. `McpTransport` ∈ `stdio|streamable_http|sse` (NO `http`). `install_spec` is a discriminated union on `"type"` ∈ `mcp_stdio|mcp_remote|oci_image|git_dir`. Root is `{ "manifest": {...}, "entries": [...] }`. `schema_version` = `1`.
- **repo_url is mandatory** in our contract (stricter than Aleph's `Option`). Producer never emits `oci_image`.
- **No per-user state** in `catalog.json` (`installed`/`enabled`/`update_available`/`source_id` never appear).
- **`description` in `catalog.json` is English** (canonical). Bilingual content lives only in `site-catalog.json`.
- **`trusted` is a display label only** → it maps to the contract value `verified`. Never write `trusted` into data/contract.
- **Theme must be applied before first paint** (no FOUC); never branch server-rendered text on client-only state (no hydration mismatch).
- **Files focused & small** (200–400 lines typical, 800 max). Immutable updates (spread, no mutation).
- Path alias `@/*` → repo root (already in `tsconfig.json`). `resolveJsonModule` already enabled.
- **Pinned toolchain:** `zod@^4` (contract uses zod-4 API: `z.url()`, 2-arg `z.record`), `@testing-library/react@^16` + its required peer `@testing-library/dom` (React-19 line). `npx tsc --noEmit` (script `typecheck`) is part of the gate. Unit tests mock `next/link` → plain `<a>` and stub `matchMedia` in `vitest.setup.ts` (no App Router context under jsdom).

---

## File Structure

**Phase A — Contract (`contract/`)**
- `contract/schema.ts` — zod schemas for the wire contract (enums, nested decls, `InstallSpec` union, manifest, entry, artifact) + `validateArtifact()`.
- `contract/types.ts` — `z.infer` TS types re-exported for app + future pipeline.
- `contract/site.ts` — zod `SiteEntry` (contract entry + display fields) + `SiteCatalog` + `validateSiteCatalog()` + inferred types.
- `public/catalog.json` — hand-authored fixture (contract-valid, 12 entries).
- `data/site-catalog.json` — hand-authored fixture (rich/bilingual, same 12 entries).
- `contract/__tests__/*.test.ts` — schema unit tests + fixture golden tests.

**Phase B — Website (`app/`, `components/`, `lib/`)**
- `lib/theme.ts` — `PALETTES` (light/dark), `THEME_VARS`, `Theme` type.
- `lib/i18n.ts` — `STRINGS` dict (zh/en), `Lang` type.
- `lib/catalog.ts` — load + validate `site-catalog.json`; selectors (`getAll/getByKind/getById/bySlug/trending/related/editorsPick/collections`), `formatStars`, `slugForEntry/idFromSlug`.
- `components/providers/{ThemeProvider,LangProvider,ThemeScript}.tsx` — client context + pre-paint script.
- `app/layout.tsx` (replace) · `app/globals.css` (replace) — fonts, CSS vars, providers.
- `components/{Header,Footer,Card,TrustBadge,Sparkline}.tsx`.
- `components/home/{Hero,EditorsPick,StatsBar,CategoryIndex,Trending,Collection}.tsx` · `app/page.tsx` (replace).
- `components/category/CategoryView.tsx` · `app/c/[kind]/page.tsx`.
- `components/detail/{Cover,Tabs,InstallSidebar,Related}.tsx` · `app/e/[...slug]/page.tsx`.
- `components/SubmitForm.tsx` · `app/submit/page.tsx` · `lib/submit.ts` (issue-URL builder).
- `vercel.json` — `/catalog.json` cache-control override.
- `tests/e2e/*.spec.ts` — Playwright.

**Mockup reference (port source, in repo):** `Aleph Hub网站设计/Aleph Hub.dc.html` (DCLogic). Key line ranges cited per task. Palettes: lines 270–271. Fonts: line 13. i18n strings: lines 334–335. Data shape: `DATA()` lines 274–337. Decorate/render logic: `renderVals()` lines 340–428. Header: 24–41. Home: 44–125. Category: 129–153. Detail: 156–202. Submit: 205–225. Footer: 228–232.

---

# Phase A — Contract Layer

### Task A1: Test tooling + zod

**Files:**
- Modify: `package.json` (deps + scripts)
- Create: `vitest.config.ts`
- Create: `vitest.setup.ts`
- Create: `playwright.config.ts`

**Interfaces:**
- Produces: `npm test` (vitest run, jsdom), `npm run test:watch`, `npm run e2e` (Playwright). zod available.

- [ ] **Step 1: Install deps**

Run:
```bash
npm install zod@^4
npm install -D vitest @vitejs/plugin-react jsdom @testing-library/react@^16 @testing-library/dom @testing-library/jest-dom @testing-library/user-event @playwright/test
npx playwright install chromium
```

**Why pinned:** `@testing-library/react@16` is the React-19-compatible line and declares `@testing-library/dom` as a REQUIRED peer that is NOT auto-installed — omitting it breaks every component test. `zod@^4` is pinned so the contract code targets one API across machines (this plan uses zod-4 semantics: `z.url()`, 2-arg `z.record`, `.default()` short-circuit).

- [ ] **Step 2: Add scripts to `package.json`**

Merge into `"scripts"`:
```json
"test": "vitest run",
"test:watch": "vitest",
"typecheck": "tsc --noEmit",
"e2e": "playwright test"
```

- [ ] **Step 3: Create `vitest.config.ts`**

```ts
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./vitest.setup.ts"],
    include: ["**/*.test.ts", "**/*.test.tsx"],
    exclude: ["node_modules", ".next", "tests/e2e/**"],
  },
  resolve: { alias: { "@": path.resolve(__dirname, ".") } },
});
```

- [ ] **Step 4: Create `vitest.setup.ts`**

```ts
import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

// next/link needs the App Router context, which jsdom unit tests don't mount.
// Render it as a plain anchor so component tests (Header/Card/CategoryView/DetailView)
// can assert hrefs without "invariant: app router not mounted".
vi.mock("next/link", async () => {
  const React = await import("react");
  return {
    __esModule: true,
    default: ({ href, children, ...props }: { href: unknown; children: React.ReactNode }) =>
      React.createElement("a", { href: typeof href === "string" ? href : "#", ...props }, children),
  };
});

// jsdom lacks matchMedia; the pre-paint theme script reads prefers-color-scheme.
if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false, media: query, onchange: null,
    addEventListener: () => {}, removeEventListener: () => {},
    addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
}
```

- [ ] **Step 5: Create `playwright.config.ts`**

```ts
import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/e2e",
  use: { baseURL: "http://localhost:3000" },
  webServer: {
    command: "npm run build && npm run start",
    url: "http://localhost:3000",
    reuseExistingServer: !process.env.CI,
    // next/font/google fetches 5 families at build time; allow a slow cold build.
    timeout: 240_000,
  },
});
```

- [ ] **Step 6: Smoke test the runner**

Create `contract/__tests__/smoke.test.ts`:
```ts
import { describe, it, expect } from "vitest";
describe("tooling", () => { it("runs", () => { expect(1 + 1).toBe(2); }); });
```
Run: `npm test` → Expected: PASS (1 test). Then delete `contract/__tests__/smoke.test.ts`.

- [ ] **Step 7: Commit**

```bash
git add package.json package-lock.json vitest.config.ts vitest.setup.ts playwright.config.ts
git commit -m "chore: add vitest, testing-library, playwright, zod"
```

---

### Task A2: Contract enums + nested decls

**Files:**
- Create: `contract/schema.ts`
- Test: `contract/__tests__/enums.test.ts`

**Interfaces:**
- Produces: `ExtensionKind`, `ExtensionCategory`, `TrustTier`, `McpTransport`, `EnvDecl`, `HeaderDecl` (all exported zod schemas in `contract/schema.ts`).

- [ ] **Step 1: Write the failing test** — `contract/__tests__/enums.test.ts`

```ts
import { describe, it, expect } from "vitest";
import {
  ExtensionKind, ExtensionCategory, TrustTier, McpTransport, EnvDecl, HeaderDecl,
} from "@/contract/schema";

describe("contract enums", () => {
  it("kind accepts the three wire values", () => {
    for (const k of ["skill", "plugin", "mcp"]) expect(ExtensionKind.parse(k)).toBe(k);
    expect(() => ExtensionKind.parse("workflow")).toThrow();
  });
  it("category accepts all 13 values and rejects others", () => {
    const cats = ["search","developer","data","productivity","writing","communication","knowledge","files","design","automation","finance","utilities","other"];
    for (const c of cats) expect(ExtensionCategory.parse(c)).toBe(c);
    expect(() => ExtensionCategory.parse("misc")).toThrow();
  });
  it("trust_tier accepts the four tiers, not 'trusted'", () => {
    for (const t of ["official","verified","community","unverified"]) expect(TrustTier.parse(t)).toBe(t);
    expect(() => TrustTier.parse("trusted")).toThrow();
  });
  it("McpTransport is stdio|streamable_http|sse and rejects 'http'", () => {
    for (const t of ["stdio","streamable_http","sse"]) expect(McpTransport.parse(t)).toBe(t);
    expect(() => McpTransport.parse("http")).toThrow();
  });
  it("EnvDecl defaults required/secret to false", () => {
    expect(EnvDecl.parse({ name: "X" })).toEqual({ name: "X", required: false, secret: false });
  });
  it("HeaderDecl defaults secret to false", () => {
    expect(HeaderDecl.parse({ name: "Authorization" })).toEqual({ name: "Authorization", secret: false });
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- enums` → Expected: FAIL (cannot import from `@/contract/schema`).

- [ ] **Step 3: Write minimal implementation** — `contract/schema.ts`

```ts
import { z } from "zod";

// Mirrors Aleph src/hub/types.rs — all snake_case wire values.
export const ExtensionKind = z.enum(["skill", "plugin", "mcp"]);

export const ExtensionCategory = z.enum([
  "search", "developer", "data", "productivity", "writing", "communication",
  "knowledge", "files", "design", "automation", "finance", "utilities", "other",
]);

export const TrustTier = z.enum(["official", "verified", "community", "unverified"]);

// Aleph McpTransport { Stdio, StreamableHttp, Sse } (snake_case). No bare "http".
export const McpTransport = z.enum(["stdio", "streamable_http", "sse"]);

export const EnvDecl = z.object({
  name: z.string(),
  description: z.string().optional(),
  required: z.boolean().default(false),
  secret: z.boolean().default(false),
  default: z.string().nullable().optional(),
  placeholder: z.string().optional(),
});

export const HeaderDecl = z.object({
  name: z.string(),
  secret: z.boolean().default(false),
});
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- enums` → Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add contract/schema.ts contract/__tests__/enums.test.ts
git commit -m "feat: contract enums and env/header decls"
```

---

### Task A3: InstallSpec discriminated union

**Files:**
- Modify: `contract/schema.ts`
- Test: `contract/__tests__/install-spec.test.ts`

**Interfaces:**
- Consumes: `McpTransport`, `EnvDecl`, `HeaderDecl` (Task A2).
- Produces: `InstallSpec` (zod discriminated union on `"type"`).

- [ ] **Step 1: Write the failing test** — `contract/__tests__/install-spec.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { InstallSpec } from "@/contract/schema";

describe("InstallSpec", () => {
  it("parses mcp_stdio with env defaults", () => {
    const s = InstallSpec.parse({ type: "mcp_stdio", command: "npx", args: ["-y", "@acme/foo"] });
    expect(s).toMatchObject({ type: "mcp_stdio", command: "npx", args: ["-y", "@acme/foo"], env: [] });
  });
  it("parses mcp_remote with a valid transport", () => {
    const s = InstallSpec.parse({ type: "mcp_remote", url: "https://x", transport: "streamable_http" });
    expect(s).toMatchObject({ type: "mcp_remote", transport: "streamable_http", headers: [] });
  });
  it("rejects mcp_remote with transport 'http'", () => {
    expect(() => InstallSpec.parse({ type: "mcp_remote", url: "https://x", transport: "http" })).toThrow();
  });
  it("parses git_dir with nullable optionals", () => {
    const s = InstallSpec.parse({ type: "git_dir", git_url: "https://github.com/a/b" });
    expect(s).toMatchObject({ type: "git_dir", git_url: "https://github.com/a/b" });
  });
  it("parses oci_image (schema completeness) even though producer never emits it", () => {
    expect(InstallSpec.parse({ type: "oci_image", image: "ghcr.io/a/b:1" })).toMatchObject({ type: "oci_image" });
  });
  it("rejects an unknown type", () => {
    expect(() => InstallSpec.parse({ type: "brew", formula: "x" })).toThrow();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- install-spec` → Expected: FAIL (`InstallSpec` undefined).

- [ ] **Step 3: Write minimal implementation** — append to `contract/schema.ts`

```ts
// Aleph InstallSpec: #[serde(tag = "type", rename_all = "snake_case")]
export const InstallSpec = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("mcp_stdio"),
    command: z.string(),
    args: z.array(z.string()).default([]),
    env: z.array(EnvDecl).default([]),
  }),
  z.object({
    type: z.literal("mcp_remote"),
    url: z.string(),
    transport: McpTransport,
    headers: z.array(HeaderDecl).default([]),
  }),
  z.object({
    type: z.literal("oci_image"),
    image: z.string(),
  }),
  z.object({
    type: z.literal("git_dir"),
    git_url: z.string(),
    subdir: z.string().nullable().optional(),
    git_ref: z.string().nullable().optional(),
    sha256: z.string().nullable().optional(),
  }),
]);
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- install-spec` → Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add contract/schema.ts contract/__tests__/install-spec.test.ts
git commit -m "feat: InstallSpec discriminated union"
```

---

### Task A4: Manifest, Entry, Artifact + validateArtifact

**Files:**
- Modify: `contract/schema.ts`
- Test: `contract/__tests__/artifact.test.ts`

**Interfaces:**
- Consumes: all of A2/A3.
- Produces: `HubCatalogManifest`, `HubCatalogEntry`, `HubCatalogArtifact`, `validateArtifact(json: unknown): HubCatalogArtifactT`.

- [ ] **Step 1: Write the failing test** — `contract/__tests__/artifact.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { HubCatalogEntry, validateArtifact } from "@/contract/schema";

const goodEntry = {
  id: "aleph-hub:acme/foo", kind: "mcp", category: "developer",
  name: "Acme Foo", description: "A tool.",
  repo_url: "https://github.com/acme/foo", trust_tier: "verified",
  install_spec: { type: "mcp_stdio", command: "npx", args: ["@acme/foo"] },
};

describe("entry + artifact", () => {
  it("parses a good entry with defaults", () => {
    const e = HubCatalogEntry.parse(goodEntry);
    expect(e).toMatchObject({ requires_config: false, tags: [] });
  });
  it("rejects an entry missing repo_url (mandatory in our contract)", () => {
    const { repo_url, ...noRepo } = goodEntry;
    expect(() => HubCatalogEntry.parse(noRepo)).toThrow();
  });
  it("rejects an entry with a bad category", () => {
    expect(() => HubCatalogEntry.parse({ ...goodEntry, category: "misc" })).toThrow();
  });
  it("validateArtifact accepts a valid artifact", () => {
    const art = validateArtifact({
      manifest: { schema_version: 1, hub_id: "aleph-hub", name: "Aleph Hub" },
      entries: [goodEntry],
    });
    expect(art.entries).toHaveLength(1);
  });
  it("validateArtifact rejects schema_version != number / missing entries", () => {
    expect(() => validateArtifact({ manifest: { schema_version: 1, hub_id: "x", name: "y" } })).toThrow();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- artifact` → Expected: FAIL.

- [ ] **Step 3: Write minimal implementation** — append to `contract/schema.ts`

```ts
export const HubCatalogManifest = z.object({
  schema_version: z.number().int().nonnegative(),
  hub_id: z.string(),
  name: z.string(),
  generated_at: z.string().optional(),
  entry_count: z.number().int().nonnegative().optional(),
  content_hash: z.string().optional(),
});

export const HubCatalogEntry = z.object({
  id: z.string(),
  kind: ExtensionKind,
  category: ExtensionCategory,
  name: z.string(),
  description: z.string(),
  repo_url: z.url(), // mandatory in our contract (D7); zod-4 top-level URL format
  trust_tier: TrustTier,
  install_spec: InstallSpec,
  requires_config: z.boolean().default(false),
  author: z.string().optional(),
  icon: z.url().optional(), // intentional producer-side narrowing (Aleph is plain Option<String>)
  tags: z.array(z.string()).default([]),
  version: z.string().optional(),
  config_schema: z.record(z.string(), z.unknown()).optional(),
  via: z.string().optional(), // producer convention, not constrained
});

export const HubCatalogArtifact = z.object({
  manifest: HubCatalogManifest,
  entries: z.array(HubCatalogEntry),
});

export function validateArtifact(json: unknown) {
  return HubCatalogArtifact.parse(json);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- artifact` → Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add contract/schema.ts contract/__tests__/artifact.test.ts
git commit -m "feat: catalog manifest, entry, artifact schema + validateArtifact"
```

---

### Task A5: Inferred contract types

**Files:**
- Create: `contract/types.ts`
- Test: `contract/__tests__/types.test-d.ts` (type-level, compiled by `tsc`)

**Interfaces:**
- Produces: `ExtensionKindT`, `ExtensionCategoryT`, `TrustTierT`, `McpTransportT`, `InstallSpecT`, `HubCatalogEntryT`, `HubCatalogManifestT`, `HubCatalogArtifactT`.

- [ ] **Step 1: Write the implementation** — `contract/types.ts`

```ts
import { z } from "zod";
import {
  ExtensionKind, ExtensionCategory, TrustTier, McpTransport,
  InstallSpec, HubCatalogEntry, HubCatalogManifest, HubCatalogArtifact,
} from "@/contract/schema";

export type ExtensionKindT = z.infer<typeof ExtensionKind>;
export type ExtensionCategoryT = z.infer<typeof ExtensionCategory>;
export type TrustTierT = z.infer<typeof TrustTier>;
export type McpTransportT = z.infer<typeof McpTransport>;
export type InstallSpecT = z.infer<typeof InstallSpec>;
export type HubCatalogEntryT = z.infer<typeof HubCatalogEntry>;
export type HubCatalogManifestT = z.infer<typeof HubCatalogManifest>;
export type HubCatalogArtifactT = z.infer<typeof HubCatalogArtifact>;
```

- [ ] **Step 2: Write a compile-time assertion** — `contract/__tests__/types.test-d.ts`

```ts
import type { HubCatalogEntryT, ExtensionKindT } from "@/contract/types";

const k: ExtensionKindT = "mcp";
// @ts-expect-error 'workflow' is not a kind
const bad: ExtensionKindT = "workflow";
const e: HubCatalogEntryT = {
  id: "aleph-hub:a/b", kind: k, category: "developer", name: "n", description: "d",
  repo_url: "https://github.com/a/b", trust_tier: "verified",
  install_spec: { type: "git_dir", git_url: "https://github.com/a/b" },
  requires_config: false, tags: [],
};
void e; void bad;
```

- [ ] **Step 3: Verify types compile (with the expected error present)**

Run: `npx tsc --noEmit` → Expected: PASS (the `@ts-expect-error` consumes the one intentional error; no other errors).

- [ ] **Step 4: Commit**

```bash
git add contract/types.ts contract/__tests__/types.test-d.ts
git commit -m "feat: inferred contract types"
```

---

### Task A6: Site-data schema (display superset)

**Files:**
- Create: `contract/site.ts`
- Test: `contract/__tests__/site.test.ts`

**Interfaces:**
- Consumes: `HubCatalogEntry`, `HubCatalogManifest` (A4).
- Produces: `SiteEntry`, `SiteCatalog`, `validateSiteCatalog(json)`, types `SiteEntryT`, `SiteCatalogT`.

**Note:** `SiteEntry` extends the contract entry (so `description` stays the English canonical) and adds bilingual + presentation fields. `trend`/`spark` are nullable/empty on first pipeline run.

- [ ] **Step 1: Write the failing test** — `contract/__tests__/site.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { SiteEntry, validateSiteCatalog } from "@/contract/site";

const base = {
  id: "aleph-hub:acme/foo", kind: "mcp", category: "developer",
  name: "Acme Foo", description: "A tool.", repo_url: "https://github.com/acme/foo",
  trust_tier: "verified", install_spec: { type: "mcp_stdio", command: "npx", args: ["@acme/foo"] },
};
const display = {
  description_zh: "一个工具。", description_en: "A tool.",
  long_zh: "长描述。", long_en: "Long description.",
  cover_color: "#C9542A", stars: 1234, trend: null, spark: [],
  install_cmd: "npx aleph add acme-foo", sec_note_zh: "已审核。", sec_note_en: "Reviewed.",
};

describe("site schema", () => {
  it("parses a site entry (contract + display)", () => {
    const e = SiteEntry.parse({ ...base, ...display });
    expect(e).toMatchObject({ name: "Acme Foo", trend: null, spark: [] });
  });
  it("defaults spark to [] and trend nullable", () => {
    const { spark, trend, ...rest } = display;
    const e = SiteEntry.parse({ ...base, ...rest });
    expect(e.spark).toEqual([]);
    expect(e.trend ?? null).toBeNull();
  });
  it("validateSiteCatalog accepts an artifact of site entries", () => {
    const c = validateSiteCatalog({
      manifest: { schema_version: 1, hub_id: "aleph-hub", name: "Aleph Hub" },
      entries: [{ ...base, ...display }],
    });
    expect(c.entries).toHaveLength(1);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- site` → Expected: FAIL.

- [ ] **Step 3: Write minimal implementation** — `contract/site.ts`

```ts
import { z } from "zod";
import { HubCatalogEntry, HubCatalogManifest } from "@/contract/schema";

export const SiteEntry = HubCatalogEntry.extend({
  description_zh: z.string(),
  description_en: z.string(),
  long_zh: z.string(),
  long_en: z.string(),
  cover_color: z.string(),       // palette color key, not an image
  stars: z.number().nonnegative(),
  trend: z.number().nullable().default(null),   // week-over-week %, null on first run
  spark: z.array(z.number()).default([]),       // sparkline points, [] on first run
  license: z.string().optional(),
  updated: z.string().optional(),
  install_cmd: z.string(),       // display CLI string (not the wire install_spec)
  sec_note_zh: z.string(),
  sec_note_en: z.string(),
});

export const SiteCatalog = z.object({
  manifest: HubCatalogManifest,
  entries: z.array(SiteEntry),
});

export function validateSiteCatalog(json: unknown) {
  return SiteCatalog.parse(json);
}

export type SiteEntryT = z.infer<typeof SiteEntry>;
export type SiteCatalogT = z.infer<typeof SiteCatalog>;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- site` → Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add contract/site.ts contract/__tests__/site.test.ts
git commit -m "feat: site-data schema (display superset)"
```

---

### Task A7: Fixture catalog + site-catalog + golden test

**Files:**
- Create: `public/catalog.json`
- Create: `data/site-catalog.json`
- Test: `contract/__tests__/fixtures.test.ts`

**Interfaces:**
- Consumes: `validateArtifact` (A4), `validateSiteCatalog` (A6).
- Produces: the two fixture files every Phase-B task reads.

**Mapping (derive all 12 entries from mockup `DATA()` lines 283–332).** `id = "aleph-hub:<owner>/<repo>"`. Contract `description` = the mockup `desc_en`. Site `description_zh/en` = mockup `desc_zh/desc_en`; `long_zh/en` = `long_zh/long_en`; `cover_color` = mockup `cover`; `stars` = mockup `stars`; `install_cmd` = mockup `install`; `sec_note_zh/en` = mockup `secNote_*`; `spark` = mockup `spark`. `author` = mockup `author`. `version` omitted. Editorial-collection membership is expressed via `tags` (`integration`/`template`/`workflow`) so Home's Collection can group by tag (per spec §7.4).

**`trend` is MANDATORY (not optional)** — `trending()` and the Home test depend on it being deterministic. Use these exact mockup values: goose 29, langgraph 26, servers 24, cline 22, Figma-Context-MCP 21, OpenHands 19, playwright-mcp 18, github-mcp-server 16, swarm 15, supabase-mcp 14, manim 12, ai (vercel) 11. So `trending(6)` deterministically resolves to `[goose, langgraph, servers, cline, Figma-Context-MCP, OpenHands]`.

**Trust-tier note:** `playwright-mcp` is rated `verified`, not the mockup badge `official` — `official` is reserved for first-party protocol/Anthropic orgs; every other entry's `trust_tier` derives directly from the mockup badge (`official`→official, `community`→community, `trusted`→verified).

| mockup id | kind | category | trust_tier | install_spec | tags add |
|---|---|---|---|---|---|
| microsoft/playwright-mcp | mcp | developer | verified | `mcp_stdio` `npx ["-y","@playwright/mcp@latest"]` | — |
| modelcontextprotocol/servers | mcp | developer | official | `git_dir` `https://github.com/modelcontextprotocol/servers` | — |
| github/github-mcp-server | mcp | developer | official | `mcp_remote` url `https://api.githubcopilot.com/mcp/` transport `streamable_http` headers `[{name:"Authorization",secret:true}]` | — |
| GLips/Figma-Context-MCP | mcp | design | community | `mcp_stdio` `npx ["-y","figma-developer-mcp","--stdio"]` env `[{name:"FIGMA_API_KEY",required:true,secret:true}]` | `integration` |
| supabase-community/supabase-mcp | mcp | data | verified | `mcp_stdio` `npx ["-y","@supabase/mcp-server-supabase@latest"]` env `[{name:"SUPABASE_ACCESS_TOKEN",required:true,secret:true}]` | `integration` |
| ManimCommunity/manim | skill | design | verified | `git_dir` `https://github.com/ManimCommunity/manim` | — |
| block/goose | skill | developer | verified | `git_dir` `https://github.com/block/goose` | — |
| cline/cline | skill | developer | verified | `git_dir` `https://github.com/cline/cline` | — |
| All-Hands-AI/OpenHands | skill | developer | verified | `git_dir` `https://github.com/All-Hands-AI/OpenHands` | — |
| openai/swarm | plugin | developer | official | `git_dir` `https://github.com/openai/swarm` | `template` |
| vercel/ai | plugin | developer | official | `git_dir` `https://github.com/vercel/ai` | `template` |
| langchain-ai/langgraph | plugin | automation | verified | `git_dir` `https://github.com/langchain-ai/langgraph` | `workflow` |

`requires_config` = true iff the chosen `install_spec` has any env with `required:true` (Figma, Supabase) or any header with `secret:true` (github-mcp-server); else false. `repo_url` = `https://github.com/<owner>/<repo>`. `via` = `github:<owner>`.

- [ ] **Step 1: Write the failing golden test** — `contract/__tests__/fixtures.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { validateArtifact } from "@/contract/schema";
import { validateSiteCatalog } from "@/contract/site";

const read = (p: string) => JSON.parse(readFileSync(p, "utf8"));

describe("fixtures", () => {
  it("public/catalog.json is a valid contract artifact", () => {
    const art = validateArtifact(read("public/catalog.json"));
    expect(art.entries.length).toBe(12);
    expect(art.manifest.hub_id).toBe("aleph-hub");
    for (const e of art.entries) {
      expect(e.id.startsWith("aleph-hub:")).toBe(true);
      expect(e.repo_url).toMatch(/^https:\/\/github\.com\//);
      expect(e.install_spec.type).not.toBe("oci_image"); // producer never emits OCI
    }
  });
  it("data/site-catalog.json is a valid site catalog with the same 12 ids", () => {
    const site = validateSiteCatalog(read("data/site-catalog.json"));
    const contract = validateArtifact(read("public/catalog.json"));
    expect(site.entries.map((e) => e.id).sort()).toEqual(contract.entries.map((e) => e.id).sort());
  });
  it("catalog.json description matches site description_en (English canonical)", () => {
    const site = validateSiteCatalog(read("data/site-catalog.json"));
    const contract = validateArtifact(read("public/catalog.json"));
    const byId = new Map(site.entries.map((e) => [e.id, e]));
    for (const e of contract.entries) expect(e.description).toBe(byId.get(e.id)!.description_en);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- fixtures` → Expected: FAIL (files missing).

- [ ] **Step 3: Author `public/catalog.json`**

Root `{ "manifest": {...}, "entries": [...] }`. Manifest: `{ "schema_version": 1, "hub_id": "aleph-hub", "name": "Aleph Hub", "generated_at": "2026-06-20T00:00:00Z", "entry_count": 12 }`. Author all 12 entries per the mapping table. Worked examples (author the remaining 9 the same way):

```jsonc
// entry — mcp_stdio
{ "id": "aleph-hub:microsoft/playwright-mcp", "kind": "mcp", "category": "developer",
  "name": "playwright-mcp", "description": "Browser automation via accessibility snapshots — no vision model needed.",
  "repo_url": "https://github.com/microsoft/playwright-mcp", "trust_tier": "verified",
  "author": "microsoft", "tags": ["browser","testing","automation"], "via": "github:microsoft",
  "requires_config": false,
  "install_spec": { "type": "mcp_stdio", "command": "npx", "args": ["-y","@playwright/mcp@latest"] } }

// entry — mcp_remote (requires_config true: secret header)
{ "id": "aleph-hub:github/github-mcp-server", "kind": "mcp", "category": "developer",
  "name": "github-mcp-server", "description": "Let agents work with repos, pull requests and workflows directly.",
  "repo_url": "https://github.com/github/github-mcp-server", "trust_tier": "official",
  "author": "github", "tags": ["github","ci","repos"], "via": "github:github", "requires_config": true,
  "install_spec": { "type": "mcp_remote", "url": "https://api.githubcopilot.com/mcp/",
    "transport": "streamable_http", "headers": [{ "name": "Authorization", "secret": true }] } }

// entry — git_dir (skill)
{ "id": "aleph-hub:block/goose", "kind": "skill", "category": "developer",
  "name": "goose", "description": "A local-first, open-source AI agent that loads any extension.",
  "repo_url": "https://github.com/block/goose", "trust_tier": "verified",
  "author": "block", "tags": ["agent","local","extensions"], "via": "github:block", "requires_config": false,
  "install_spec": { "type": "git_dir", "git_url": "https://github.com/block/goose" } }
```

- [ ] **Step 4: Author `data/site-catalog.json`**

Same root + same manifest. Each entry = the contract entry above **plus** display fields. Worked example for playwright-mcp (author all 12, copying bilingual text from mockup lines 284–331):

```jsonc
{ "id": "aleph-hub:microsoft/playwright-mcp", "kind": "mcp", "category": "developer",
  "name": "playwright-mcp", "description": "Browser automation via accessibility snapshots — no vision model needed.",
  "repo_url": "https://github.com/microsoft/playwright-mcp", "trust_tier": "verified",
  "author": "microsoft", "tags": ["browser","testing","automation"], "via": "github:microsoft",
  "requires_config": false,
  "install_spec": { "type": "mcp_stdio", "command": "npx", "args": ["-y","@playwright/mcp@latest"] },
  "description_zh": "用可访问性快照驱动浏览器自动化，无需视觉模型。",
  "description_en": "Browser automation via accessibility snapshots — no vision model needed.",
  "long_zh": "playwright-mcp 是微软出品的 Model Context Protocol 服务…",
  "long_en": "playwright-mcp is Microsoft's Model Context Protocol server…",
  "cover_color": "#C9542A", "stars": 34000, "trend": 18, "spark": [15,13,14,9,10,5,4,1],
  "license": "Apache-2.0", "updated": "2026-06-09", "install_cmd": "npx aleph add playwright-mcp",
  "sec_note_zh": "微软官方维护，纳入前已完成安全审计。", "sec_note_en": "Officially maintained by Microsoft; security-audited before inclusion." }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `npm test -- fixtures` → Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add public/catalog.json data/site-catalog.json contract/__tests__/fixtures.test.ts
git commit -m "feat: fixture catalog.json + site-catalog.json (12 entries) with golden validation"
```

---

# Phase B — Website

### Task B1: Theme palettes + i18n strings

**Files:**
- Create: `lib/theme.ts`
- Create: `lib/i18n.ts`
- Test: `lib/__tests__/theme-i18n.test.ts`

**Interfaces:**
- Produces: `PALETTES: Record<Theme, Record<string,string>>`, `THEME_VARS` (ordered var keys), `Theme = "light"|"dark"`, `paletteToCssVars(theme)`. `STRINGS: Record<Lang, Strings>`, `Lang = "zh"|"en"`, `Strings` type, `CATEGORY_LABELS: Record<ExtensionCategoryT, { zh: string; en: string }>` (human labels for the 13 contract categories — the mockup's `catName` does not exist for these, so the detail meta row uses this map).

- [ ] **Step 1: Write the failing test** — `lib/__tests__/theme-i18n.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { PALETTES, paletteToCssVars } from "@/lib/theme";
import { STRINGS } from "@/lib/i18n";

describe("theme + i18n", () => {
  it("has light and dark palettes with the orange accent", () => {
    expect(PALETTES.light.orange).toBe("#C9501A");
    expect(PALETTES.dark.orange).toBe("#EE863F");
  });
  it("paletteToCssVars emits --orange etc.", () => {
    const vars = paletteToCssVars("light");
    expect(vars["--orange"]).toBe("#C9501A");
    expect(vars["--bg"]).toBe("#F4EBDD");
  });
  it("has zh and en strings for the submit label", () => {
    expect(STRINGS.zh.submit).toBe("提交");
    expect(STRINGS.en.submit).toBe("Submit");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- theme-i18n` → Expected: FAIL.

- [ ] **Step 3: Implement `lib/theme.ts`** (palette values from mockup lines 270–271)

```ts
export type Theme = "light" | "dark";

export const PALETTES: Record<Theme, Record<string, string>> = {
  light: { bg: "#F4EBDD", paper: "#FBF6EE", panel: "#FFFFFF", ink: "#241C16", "ink-soft": "#5C4F42", taupe: "#8A7B6B", hair: "#E4D9C8", "hair-strong": "#241C16", orange: "#C9501A", chip: "#EDE3D3", green: "#4E6B4A" },
  dark:  { bg: "#17120D", paper: "#1F1811", panel: "#271F17", ink: "#F2E8DA", "ink-soft": "#C3B4A1", taupe: "#8F8273", hair: "#372B20", "hair-strong": "#C3B4A1", orange: "#EE863F", chip: "#2C2218", green: "#83A971" },
};

export const THEME_VARS = Object.keys(PALETTES.light);

export function paletteToCssVars(theme: Theme): Record<string, string> {
  const p = PALETTES[theme];
  return Object.fromEntries(Object.keys(p).map((k) => [`--${k}`, p[k]]));
}
```

- [ ] **Step 4: Implement `lib/i18n.ts`** (port the `zh`/`en` objects verbatim from mockup lines 334–335)

```ts
export type Lang = "zh" | "en";

export interface Strings {
  submit: string; kicker: string; heroA: string; heroEm: string; heroB: string; heroSub: string;
  ctaExplore: string; ctaAll: string; editorPick: string; searchPh: string;
  stProjects: string; stCats: string; stDailyN: string; stSync: string;
  indexTitle: string; browseByCat: string; trendingTitle: string; collectionTitle: string;
  viewAll: string; catalogKicker: string; results: string; sortBy: string; sortTrend: string;
  noResults: string; allCats: string; back: string; tabOverview: string; tabSecurity: string;
  secScan: string; secReview: string; secReviewNote: string;
  mBy: string; mCategory: string; mStars: string; mLicense: string; mUpdated: string;
  viewGithub: string; related: string; copy: string; copied: string;
  submitKicker: string; submitTitle: string; submitSub: string;
  fRepo: string; fName: string; fCategory: string; fDesc: string; fDescPh: string; fTags: string;
  submitNote: string; submitBtn: string; cancel: string; footer: string; footerTag: string;
}

export const STRINGS: Record<Lang, Strings> = {
  zh: { /* port from mockup line 334 verbatim */ } as Strings,
  en: { /* port from mockup line 335 verbatim */ } as Strings,
};

// Human labels for the 13 contract categories (mockup has no catName for these).
import type { ExtensionCategoryT } from "@/contract/types";
export const CATEGORY_LABELS: Record<ExtensionCategoryT, { zh: string; en: string }> = {
  search: { zh: "搜索", en: "Search" },
  developer: { zh: "开发者", en: "Developer" },
  data: { zh: "数据", en: "Data" },
  productivity: { zh: "效率", en: "Productivity" },
  writing: { zh: "写作", en: "Writing" },
  communication: { zh: "沟通", en: "Communication" },
  knowledge: { zh: "知识", en: "Knowledge" },
  files: { zh: "文件", en: "Files" },
  design: { zh: "设计", en: "Design" },
  automation: { zh: "自动化", en: "Automation" },
  finance: { zh: "金融", en: "Finance" },
  utilities: { zh: "工具", en: "Utilities" },
  other: { zh: "其他", en: "Other" },
};
```

(Replace the `/* port ... */` with the exact key/value pairs from mockup lines 334–335. Remove the `as Strings` cast once filled — it's only to let this skeleton typecheck before you paste.)

- [ ] **Step 5: Run test to verify it passes**

Run: `npm test -- theme-i18n` → Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add lib/theme.ts lib/i18n.ts lib/__tests__/theme-i18n.test.ts
git commit -m "feat: theme palettes and i18n strings"
```

---

### Task B2: Catalog data layer

**Files:**
- Create: `lib/catalog.ts`
- Test: `lib/__tests__/catalog.test.ts`

**Interfaces:**
- Consumes: `validateSiteCatalog`, `SiteEntryT` (A6); `data/site-catalog.json` (A7).
- Produces: `getAll(): SiteEntryT[]`, `getByKind(kind)`, `getByCategory(cat)`, `getById(id)`, `bySlug(slug)`, `trending(n)`, `related(entry, n)`, `editorsPick()`, `collections()`, `formatStars(n)`, `slugForEntry(e)`, `idFromSlug(slug)`, `kindCounts()`.

- [ ] **Step 1: Write the failing test** — `lib/__tests__/catalog.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { getAll, getByKind, getById, bySlug, slugForEntry, idFromSlug, trending, formatStars, kindCounts } from "@/lib/catalog";

describe("catalog data layer", () => {
  it("loads 12 validated entries", () => { expect(getAll()).toHaveLength(12); });
  it("filters by kind", () => { expect(getByKind("mcp").every((e) => e.kind === "mcp")).toBe(true); });
  it("slug round-trips through id and strips the hub prefix", () => {
    const e = getAll()[0];
    expect(idFromSlug(slugForEntry(e))).toBe(e.id);
    expect(bySlug(slugForEntry(e))?.id).toBe(e.id);
    // pin the exact slug shape ("owner/repo", no prefix)
    expect(slugForEntry(getById("aleph-hub:block/goose")!)).toBe("block/goose");
  });
  it("getById returns the entry", () => {
    expect(getById("aleph-hub:block/goose")?.name).toBe("goose");
  });
  it("trending sorts by trend desc and respects n", () => {
    const t = trending(3);
    expect(t).toHaveLength(3);
    // ordering, not just length (fixture trend: goose 29 > langgraph 26 > servers 24)
    expect(t.map((e) => e.name)).toEqual(["goose", "langgraph", "servers"]);
    expect(t[0].trend!).toBeGreaterThanOrEqual(t[1].trend!);
  });
  it("formatStars compacts thousands", () => {
    expect(formatStars(34000)).toBe("34k");
    expect(formatStars(950)).toBe("950");
  });
  it("kindCounts counts per kind", () => {
    const c = kindCounts();
    expect(c.mcp + c.skill + c.plugin).toBe(12);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- catalog` → Expected: FAIL.

- [ ] **Step 3: Implement `lib/catalog.ts`**

```ts
import siteData from "@/data/site-catalog.json";
import { validateSiteCatalog, type SiteEntryT } from "@/contract/site";
import type { ExtensionKindT } from "@/contract/types";

const CATALOG = validateSiteCatalog(siteData);

export function getAll(): SiteEntryT[] { return CATALOG.entries; }
export function getByKind(kind: ExtensionKindT): SiteEntryT[] { return CATALOG.entries.filter((e) => e.kind === kind); }
export function getByCategory(cat: string): SiteEntryT[] { return CATALOG.entries.filter((e) => e.category === cat); }
export function getById(id: string): SiteEntryT | undefined { return CATALOG.entries.find((e) => e.id === id); }

// id "aleph-hub:owner/repo" <-> slug "owner/repo"
export function slugForEntry(e: SiteEntryT): string { return e.id.replace(/^aleph-hub:/, ""); }
export function idFromSlug(slug: string): string { return `aleph-hub:${slug}`; }
export function bySlug(slug: string): SiteEntryT | undefined { return getById(idFromSlug(slug)); }

export function trending(n: number): SiteEntryT[] {
  return [...CATALOG.entries].sort((a, b) => (b.trend ?? 0) - (a.trend ?? 0)).slice(0, n);
}
// Related = same category (matches the mockup's detail "Related" logic), excluding self.
export function related(entry: SiteEntryT, n: number): SiteEntryT[] {
  return CATALOG.entries.filter((e) => e.category === entry.category && e.id !== entry.id).slice(0, n);
}
export function editorsPick(): SiteEntryT {
  return [...CATALOG.entries].sort((a, b) => b.stars - a.stars)[0];
}
// Editorial collections by tag (spec §7.4): Integrations / Templates / Workflows
export function collections(): { tag: string; entries: SiteEntryT[] }[] {
  return ["integration", "template", "workflow"].map((tag) => ({
    tag, entries: CATALOG.entries.filter((e) => e.tags.includes(tag)),
  }));
}
export function kindCounts(): Record<ExtensionKindT, number> {
  return CATALOG.entries.reduce(
    (acc, e) => ({ ...acc, [e.kind]: (acc[e.kind] ?? 0) + 1 }),
    { skill: 0, plugin: 0, mcp: 0 } as Record<ExtensionKindT, number>,
  );
}
export function formatStars(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(1).replace(/\.0$/, "")}k` : String(n);
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- catalog` → Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add lib/catalog.ts lib/__tests__/catalog.test.ts
git commit -m "feat: catalog data layer (load + selectors)"
```

---

### Task B3: Providers, fonts, layout, globals

**Files:**
- Create: `components/providers/ThemeScript.tsx`
- Create: `components/providers/ThemeProvider.tsx`
- Create: `components/providers/LangProvider.tsx`
- Modify: `app/layout.tsx` (replace)
- Modify: `app/globals.css` (replace)
- Test: `components/providers/__tests__/providers.test.tsx`

**Interfaces:**
- Produces: `useTheme(): { theme, toggle }`, `useLang(): { lang, set }`, `<ThemeProvider>`, `<LangProvider>`, `<ThemeScript>`. CSS vars `--bg`/`--ink`/`--orange`/… set on `:root[data-theme=...]`.

**Approach:** `ThemeScript` is a blocking inline `<script>` in `<head>` that reads `localStorage.theme` ‖ `prefers-color-scheme` and sets `documentElement.dataset.theme` BEFORE paint (no FOUC). `globals.css` declares both palettes as `:root[data-theme="light"]{…}` / `:root[data-theme="dark"]{…}`. `<html>` gets `suppressHydrationWarning`. Lang default = `"zh"` server-side (documented default); client may switch. Text content that differs by lang lives in Client Components so SSR uses the default and the client re-renders on toggle without a hydration mismatch on server-fixed nodes.

- [ ] **Step 1: Write the failing test** — `components/providers/__tests__/providers.test.tsx`

```tsx
import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ThemeProvider, useTheme } from "@/components/providers/ThemeProvider";
import { LangProvider, useLang } from "@/components/providers/LangProvider";

function Probe() {
  const { theme, toggle } = useTheme();
  const { lang, set } = useLang();
  return (
    <div>
      <span data-testid="theme">{theme}</span>
      <span data-testid="lang">{lang}</span>
      <button onClick={toggle}>t</button>
      <button onClick={() => set("en")}>en</button>
    </div>
  );
}

describe("providers", () => {
  it("toggles theme and sets lang", () => {
    render(<ThemeProvider><LangProvider><Probe /></LangProvider></ThemeProvider>);
    expect(screen.getByTestId("theme").textContent).toBe("light");
    fireEvent.click(screen.getByText("t"));
    expect(screen.getByTestId("theme").textContent).toBe("dark");
    fireEvent.click(screen.getByText("en"));
    expect(screen.getByTestId("lang").textContent).toBe("en");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- providers` → Expected: FAIL.

- [ ] **Step 3: Implement `components/providers/ThemeProvider.tsx`**

```tsx
"use client";
import { createContext, useContext, useEffect, useState } from "react";
import type { Theme } from "@/lib/theme";

const Ctx = createContext<{ theme: Theme; toggle: () => void }>({ theme: "light", toggle: () => {} });

export function ThemeProvider({ children }: { children: React.ReactNode }) {
  const [theme, setTheme] = useState<Theme>("light");
  useEffect(() => {
    const initial = (document.documentElement.dataset.theme as Theme) || "light";
    setTheme(initial);
  }, []);
  const toggle = () => setTheme((t) => {
    const next = t === "dark" ? "light" : "dark";
    document.documentElement.dataset.theme = next;
    try { localStorage.theme = next; } catch {}
    return next;
  });
  return <Ctx.Provider value={{ theme, toggle }}>{children}</Ctx.Provider>;
}
export const useTheme = () => useContext(Ctx);
```

- [ ] **Step 4: Implement `components/providers/LangProvider.tsx`**

```tsx
"use client";
import { createContext, useContext, useState } from "react";
import type { Lang } from "@/lib/i18n";

const Ctx = createContext<{ lang: Lang; set: (l: Lang) => void }>({ lang: "zh", set: () => {} });

export function LangProvider({ children }: { children: React.ReactNode }) {
  const [lang, setLang] = useState<Lang>("zh");
  const set = (l: Lang) => { setLang(l); try { localStorage.lang = l; } catch {} };
  return <Ctx.Provider value={{ lang, set }}>{children}</Ctx.Provider>;
}
export const useLang = () => useContext(Ctx);
```

- [ ] **Step 5: Implement `components/providers/ThemeScript.tsx`**

```tsx
// Server component: blocking inline script that sets data-theme before paint (no FOUC).
export function ThemeScript() {
  const js = `(function(){try{var t=localStorage.theme;if(t!=="light"&&t!=="dark"){t=matchMedia("(prefers-color-scheme: dark)").matches?"dark":"light";}document.documentElement.dataset.theme=t;}catch(e){document.documentElement.dataset.theme="light";}})();`;
  return <script dangerouslySetInnerHTML={{ __html: js }} />;
}
```

- [ ] **Step 6: Replace `app/globals.css`** (both palettes as data-theme scopes; base styles from mockup lines 14–20)

```css
:root[data-theme="light"] { --bg:#F4EBDD; --paper:#FBF6EE; --panel:#FFFFFF; --ink:#241C16; --ink-soft:#5C4F42; --taupe:#8A7B6B; --hair:#E4D9C8; --hair-strong:#241C16; --orange:#C9501A; --chip:#EDE3D3; --green:#4E6B4A; }
:root[data-theme="dark"]  { --bg:#17120D; --paper:#1F1811; --panel:#271F17; --ink:#F2E8DA; --ink-soft:#C3B4A1; --taupe:#8F8273; --hair:#372B20; --hair-strong:#C3B4A1; --orange:#EE863F; --chip:#2C2218; --green:#83A971; }

* { box-sizing: border-box; }
body { margin: 0; -webkit-font-smoothing: antialiased; text-rendering: optimizeLegibility;
  background: var(--bg); color: var(--ink); min-height: 100vh;
  font-family: var(--font-hanken), var(--font-noto-sans-sc), system-ui, sans-serif;
  transition: background .25s, color .25s; }
::selection { background: var(--orange); color: #fff; }
input, textarea, select, button { font-family: inherit; }
input::placeholder, textarea::placeholder { color: var(--taupe); }
```

- [ ] **Step 7: Replace `app/layout.tsx`** (next/font wiring + providers + ThemeScript)

```tsx
import type { Metadata } from "next";
import { Cormorant_Garamond, Hanken_Grotesk, JetBrains_Mono, Noto_Serif_SC, Noto_Sans_SC } from "next/font/google";
import "./globals.css";
import { ThemeScript } from "@/components/providers/ThemeScript";
import { ThemeProvider } from "@/components/providers/ThemeProvider";
import { LangProvider } from "@/components/providers/LangProvider";

const cormorant = Cormorant_Garamond({ subsets: ["latin"], weight: ["400","500","600"], style: ["normal","italic"], variable: "--font-cormorant" });
const hanken = Hanken_Grotesk({ subsets: ["latin"], weight: ["400","500","600","700"], variable: "--font-hanken" });
const mono = JetBrains_Mono({ subsets: ["latin"], weight: ["400","500"], variable: "--font-mono" });
const notoSerif = Noto_Serif_SC({ subsets: ["latin"], weight: ["500","600"], variable: "--font-noto-serif-sc" });
const notoSans = Noto_Sans_SC({ subsets: ["latin"], weight: ["400","500"], variable: "--font-noto-sans-sc" });

export const metadata: Metadata = {
  title: "Aleph Hub — The Agent Capability Atlas",
  description: "Discover Agent Skills, MCP servers and plugins. Open source, security-reviewed, always fresh.",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  const cls = [cormorant, hanken, mono, notoSerif, notoSans].map((f) => f.variable).join(" ");
  return (
    <html lang="en" suppressHydrationWarning className={cls}>
      <head><ThemeScript /></head>
      <body><ThemeProvider><LangProvider>{children}</LangProvider></ThemeProvider></body>
    </html>
  );
}
```

- [ ] **Step 8: Run providers test + typecheck**

Run: `npm test -- providers` → Expected: PASS. Run: `npx tsc --noEmit` → Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add components/providers app/layout.tsx app/globals.css
git commit -m "feat: theme/lang providers, fonts, pre-paint theme script, layout"
```

---

### Task B4: Header + Footer

**Files:**
- Create: `components/Header.tsx`
- Create: `components/Footer.tsx`
- Test: `components/__tests__/header.test.tsx`

**Interfaces:**
- Consumes: `useLang`, `useTheme`, `STRINGS`, `kindCounts` (for nav), Next `Link`.
- Produces: `<Header />` (sticky; logo→`/`, kind nav→`/c/[kind]`, 中/EN toggle, theme toggle, Submit→`/submit`), `<Footer />`.

**Port:** Header markup/styles from mockup lines 24–41 (translate inline styles to React `style={{…}}` preserving values). Nav items = the 3 kinds with labels from `STRINGS` (`Agent Skills`/`Plugins`/`MCP Servers` — add these three keys to `Strings` if not present, or derive: zh `["Agent 技能","插件","MCP 服务"]`, en `["Agent Skills","Plugins","MCP Servers"]`). Footer from lines 228–232.

- [ ] **Step 1: Write the failing test** — `components/__tests__/header.test.tsx`

```tsx
import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ThemeProvider } from "@/components/providers/ThemeProvider";
import { LangProvider } from "@/components/providers/LangProvider";
import { Header } from "@/components/Header";

const wrap = (ui: React.ReactNode) => render(<ThemeProvider><LangProvider>{ui}</LangProvider></ThemeProvider>);

describe("Header", () => {
  it("renders brand and a submit control", () => {
    wrap(<Header />);
    expect(screen.getByText("ALEPH HUB")).toBeInTheDocument();
    expect(screen.getByText("提交")).toBeInTheDocument(); // zh default
  });
  it("switches language label to English", () => {
    wrap(<Header />);
    fireEvent.click(screen.getByText("EN"));
    expect(screen.getByText("Submit")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- header` → Expected: FAIL.

- [ ] **Step 3: Implement `components/Header.tsx`** (Client Component)

```tsx
"use client";
import Link from "next/link";
import { useLang } from "@/components/providers/LangProvider";
import { useTheme } from "@/components/providers/ThemeProvider";
import { STRINGS } from "@/lib/i18n";

const NAV: { kind: string; zh: string; en: string }[] = [
  { kind: "skill", zh: "Agent 技能", en: "Agent Skills" },
  { kind: "plugin", zh: "插件", en: "Plugins" },
  { kind: "mcp", zh: "MCP 服务", en: "MCP Servers" },
];

export function Header() {
  const { lang, set } = useLang();
  const { theme, toggle } = useTheme();
  const t = STRINGS[lang];
  // styles: port from mockup lines 24-41 into these objects, preserving every value.
  return (
    <header style={{ position: "sticky", top: 0, zIndex: 50, display: "flex", alignItems: "center", justifyContent: "space-between", padding: "18px 48px", background: "var(--bg)", borderBottom: "1px solid var(--hair)" }}>
      <Link href="/" style={{ display: "flex", alignItems: "center", gap: 12, textDecoration: "none", color: "inherit" }}>
        <span style={{ fontFamily: "var(--font-cormorant), serif", fontSize: 30, lineHeight: 1, color: "var(--orange)" }}>ℵ</span>
        <span style={{ fontSize: 13, letterSpacing: ".30em", fontWeight: 600, whiteSpace: "nowrap" }}>ALEPH HUB</span>
      </Link>
      <nav style={{ display: "flex", gap: 26 }}>
        {NAV.map((n) => (
          <Link key={n.kind} href={`/c/${n.kind}`} style={{ fontSize: 11, letterSpacing: ".16em", textTransform: "uppercase", color: "var(--ink-soft)", textDecoration: "none", whiteSpace: "nowrap" }}>
            {lang === "zh" ? n.zh : n.en}
          </Link>
        ))}
      </nav>
      <div style={{ display: "flex", alignItems: "center", gap: 16 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 5, fontSize: 11, letterSpacing: ".06em", fontWeight: 600 }}>
          <span onClick={() => set("zh")} style={{ cursor: "pointer", color: lang === "zh" ? "var(--ink)" : "var(--taupe)" }}>中</span>
          <span style={{ color: "var(--taupe)" }}>/</span>
          <span onClick={() => set("en")} style={{ cursor: "pointer", color: lang === "en" ? "var(--ink)" : "var(--taupe)" }}>EN</span>
        </div>
        <span onClick={toggle} style={{ fontSize: 16, color: "var(--ink-soft)", cursor: "pointer", lineHeight: 1 }}>{theme === "dark" ? "☼" : "☾"}</span>
        <Link href="/submit" style={{ fontSize: 11, letterSpacing: ".14em", textTransform: "uppercase", fontWeight: 600, color: "var(--bg)", background: "var(--ink)", padding: "9px 18px", borderRadius: 2, textDecoration: "none", whiteSpace: "nowrap" }}>{t.submit}</Link>
      </div>
    </header>
  );
}
```

- [ ] **Step 4: Implement `components/Footer.tsx`** (Client Component; port lines 228–232)

```tsx
"use client";
import { useLang } from "@/components/providers/LangProvider";
import { STRINGS } from "@/lib/i18n";

export function Footer() {
  const { lang } = useLang();
  const t = STRINGS[lang];
  return (
    <footer style={{ borderTop: "1px solid var(--hair)", marginTop: 20, padding: "40px 48px", maxWidth: 1480, marginLeft: "auto", marginRight: "auto", display: "flex", alignItems: "center", justifyContent: "space-between", flexWrap: "wrap", gap: 20 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 12 }}>
        <span style={{ fontFamily: "var(--font-cormorant), serif", fontSize: 26, color: "var(--orange)" }}>ℵ</span>
        <span style={{ fontSize: 12, letterSpacing: ".28em", fontWeight: 600 }}>ALEPH HUB</span>
      </div>
      <span style={{ fontSize: 12, color: "var(--taupe)", letterSpacing: ".04em" }}>{t.footer}</span>
      <span style={{ fontSize: 12, color: "var(--taupe)" }}>© 2026 · {t.footerTag}</span>
    </footer>
  );
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `npm test -- header` → Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add components/Header.tsx components/Footer.tsx components/__tests__/header.test.tsx
git commit -m "feat: header (kind nav, lang/theme toggle) and footer"
```

---

### Task B5: TrustBadge, Sparkline, Card

**Files:**
- Create: `components/TrustBadge.tsx`
- Create: `components/Sparkline.tsx`
- Create: `components/Card.tsx`
- Test: `components/__tests__/card.test.tsx`

**Interfaces:**
- Consumes: `SiteEntryT`, `useLang`, `formatStars`, `slugForEntry`, `Link`.
- Produces: `<TrustBadge tier={TrustTierT} />` (display label `verified`→"Trusted"), `<Sparkline points={number[]} color={string} />` (neutral when empty), `<Card entry={SiteEntryT} />` (links to `/e/[slug]`).

**Port:** card markup/styles from mockup lines 98–102 (trending card) / decorate badge styles lines 348–352. Trust label map: official→"Official", verified→"Trusted", community→"Community", unverified→"Unverified". Badge style by tier from mockup lines 349–351.

- [ ] **Step 1: Write the failing test** — `components/__tests__/card.test.tsx`

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { LangProvider } from "@/components/providers/LangProvider";
import { TrustBadge } from "@/components/TrustBadge";
import { Sparkline } from "@/components/Sparkline";
import { Card } from "@/components/Card";
import { getById } from "@/lib/catalog";

describe("card primitives", () => {
  it("TrustBadge shows 'Trusted' for verified (display alias)", () => {
    render(<TrustBadge tier="verified" />);
    expect(screen.getByText("Trusted")).toBeInTheDocument();
  });
  it("Sparkline renders nothing meaningful when empty", () => {
    const { container } = render(<Sparkline points={[]} color="var(--green)" />);
    expect(container.querySelector("polyline")).toBeNull();
  });
  it("Card renders entry name and links to its detail page", () => {
    const e = getById("aleph-hub:block/goose")!;
    render(<LangProvider><Card entry={e} /></LangProvider>);
    expect(screen.getByText("goose")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /goose/i })).toHaveAttribute("href", "/e/block/goose");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- card` → Expected: FAIL.

- [ ] **Step 3: Implement `components/TrustBadge.tsx`**

```tsx
import type { TrustTierT } from "@/contract/types";

const LABEL: Record<TrustTierT, string> = { official: "Official", verified: "Trusted", community: "Community", unverified: "Unverified" };

export function TrustBadge({ tier }: { tier: TrustTierT }) {
  // styles per mockup lines 349-351
  const base = { fontSize: 10, letterSpacing: ".10em", textTransform: "uppercase" as const, fontWeight: 600, padding: "3px 8px", borderRadius: 2, whiteSpace: "nowrap" as const, flex: "none" as const };
  const style =
    tier === "official" ? { ...base, color: "#FBF6EE", background: "var(--orange)", padding: "4px 9px" }
    : tier === "verified" ? { ...base, color: "var(--green)", border: "1px solid var(--green)" }
    : { ...base, color: "var(--taupe)", border: "1px solid var(--taupe)" };
  return <span style={style}>{LABEL[tier]}</span>;
}
```

- [ ] **Step 4: Implement `components/Sparkline.tsx`**

```tsx
export function Sparkline({ points, color }: { points: number[]; color: string }) {
  if (!points.length) return <svg width={50} height={18} viewBox="0 0 56 18" aria-hidden />;
  const pts = points.map((y, i) => `${(i / (points.length - 1)) * 56},${y}`).join(" ");
  return (
    <svg width={50} height={18} viewBox="0 0 56 18">
      <polyline points={pts} fill="none" stroke={color} strokeWidth={1.5} />
    </svg>
  );
}
```

- [ ] **Step 5: Implement `components/Card.tsx`** (Client; bilingual desc via `useLang`; port lines 98–102)

```tsx
"use client";
import Link from "next/link";
import type { SiteEntryT } from "@/contract/site";
import { useLang } from "@/components/providers/LangProvider";
import { formatStars, slugForEntry } from "@/lib/catalog";
import { TrustBadge } from "@/components/TrustBadge";
import { Sparkline } from "@/components/Sparkline";

export function Card({ entry }: { entry: SiteEntryT }) {
  const { lang } = useLang();
  const desc = lang === "zh" ? entry.description_zh : entry.description_en;
  const trendColor = (entry.trend ?? 0) >= 15 ? "var(--green)" : "var(--taupe)";
  return (
    <Link href={`/e/${slugForEntry(entry)}`} style={{ display: "block", textDecoration: "none", color: "inherit", background: "var(--panel)", border: "1px solid var(--hair)", borderRadius: 3, padding: 20 }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "flex-start", marginBottom: 13 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontFamily: "var(--font-mono), monospace", fontSize: 15, fontWeight: 500 }}>{entry.name}</div>
          <div style={{ fontSize: 11, color: "var(--taupe)", marginTop: 3 }}>{entry.author}</div>
        </div>
        <TrustBadge tier={entry.trust_tier} />
      </div>
      <p style={{ fontSize: 13, lineHeight: 1.5, color: "var(--ink-soft)", margin: "0 0 16px", minHeight: 39 }}>{desc}</p>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", paddingTop: 14, borderTop: "1px solid var(--hair)" }}>
        <span style={{ fontSize: 10, letterSpacing: ".08em", textTransform: "uppercase", color: "var(--ink-soft)", background: "var(--chip)", padding: "3px 8px", borderRadius: 2 }}>{entry.kind}</span>
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <Sparkline points={entry.spark} color={trendColor} />
          <span style={{ fontFamily: "var(--font-mono), monospace", fontSize: 12 }}>★{formatStars(entry.stars)}</span>
          {entry.trend != null && <span style={{ fontSize: 11, fontWeight: 600, color: trendColor }}>▲{entry.trend}%</span>}
        </div>
      </div>
    </Link>
  );
}
```

- [ ] **Step 6: Run test to verify it passes**

Run: `npm test -- card` → Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add components/TrustBadge.tsx components/Sparkline.tsx components/Card.tsx components/__tests__/card.test.tsx
git commit -m "feat: trust badge, sparkline, catalog card"
```

---

### Task B6: Home page

**Files:**
- Create: `components/home/{Hero,EditorsPick,StatsBar,CategoryIndex,Trending,Collection}.tsx`
- Modify: `app/page.tsx` (replace)
- Test: `components/home/__tests__/home.test.tsx`

**Interfaces:**
- Consumes: `getAll/trending/editorsPick/collections/kindCounts`, `useLang`, `STRINGS`, `Card`, `Header`, `Footer`.
- Produces: composed home view.

**Port:** Home sections from mockup lines 44–125. Hero+EditorsPick lines 46–68; StatsBar 70–76 (projects=`getAll().length`, cats=`13`, daily sync); CategoryIndex 78–89 (rows = 3 kinds with `kindCounts()`, links `/c/[kind]`); Trending 91–105 (`trending(6)` → `Card`); Collection 107–124 (`collections()` → grouped grids labeled Integrations/Templates/Workflows). All section text from `STRINGS`.

- [ ] **Step 1: Write the failing test** — `components/home/__tests__/home.test.tsx`

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ThemeProvider } from "@/components/providers/ThemeProvider";
import { LangProvider } from "@/components/providers/LangProvider";
import Home from "@/app/page";

describe("Home", () => {
  it("renders hero kicker, stats, and a trending card", () => {
    render(<ThemeProvider><LangProvider><Home /></LangProvider></ThemeProvider>);
    // kicker is intentionally identical in zh and en (per mockup line 334), so the
    // zh-default render still shows the English atlas line.
    expect(screen.getByText("The Agent Capability Atlas")).toBeInTheDocument();
    // projects count = getAll().length = 12, asserted via a stable testid (not a
    // bare getByText("12"), which would collide with card "▲12%" / cats "13").
    expect(screen.getByTestId("stat-projects")).toHaveTextContent("12");
    // an entry name appears in trending/collection (langgraph is in trending(6))
    expect(screen.getAllByText("langgraph").length).toBeGreaterThan(0);
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- home` → Expected: FAIL.

- [ ] **Step 3: Implement the six section components**

Each is a `"use client"` component using `useLang()` + selectors. Port the exact markup/styles from the cited mockup lines into `style={{…}}`. Bind data from `lib/catalog`. Example — `components/home/Trending.tsx`:

```tsx
"use client";
import { useLang } from "@/components/providers/LangProvider";
import { STRINGS } from "@/lib/i18n";
import { trending } from "@/lib/catalog";
import { Card } from "@/components/Card";

export function Trending() {
  const { lang } = useLang();
  const t = STRINGS[lang];
  return (
    <section style={{ maxWidth: 1480, margin: "0 auto", padding: "44px 48px 76px" }}>
      <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", borderBottom: "1px solid var(--hair-strong)", paddingBottom: 11, marginBottom: 26 }}>
        <span style={{ fontSize: 11, letterSpacing: ".20em", textTransform: "uppercase", color: "var(--ink-soft)", fontWeight: 600 }}>{t.trendingTitle}</span>
      </div>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(3,1fr)", gap: 20 }}>
        {trending(6).map((e) => <Card key={e.id} entry={e} />)}
      </div>
    </section>
  );
}
```

Implement `Hero`, `EditorsPick` (uses `editorsPick()`; cover block uses `cover_color` + first letter of name; install cmd from `install_cmd`), `StatsBar` (`getAll().length`, `13`, `t.stDailyN`), `CategoryIndex` (3 kind rows with `kindCounts()` + `Link href="/c/${kind}"`), `Collection` (`collections()` → labeled grids of `Card`) the same way, each citing its mockup lines (Hero/EditorsPick 46–68, StatsBar 70–76, CategoryIndex 78–89, Collection 107–124).

**StatsBar:** do NOT keep the mockup's hardcoded `622`/`12`. Bind projects = `getAll().length` (=12) and render that value wrapped with `data-testid="stat-projects"`; categories = `13` (the 13-value `ExtensionCategory` taxonomy). **Collection:** render only non-empty groups — `collections().filter((g) => g.entries.length)` — so an empty tag never produces a labeled grid with no cards (all three are non-empty in the fixture).

- [ ] **Step 4: Replace `app/page.tsx`**

```tsx
import { Header } from "@/components/Header";
import { Footer } from "@/components/Footer";
import { Hero } from "@/components/home/Hero";
import { EditorsPick } from "@/components/home/EditorsPick";
import { StatsBar } from "@/components/home/StatsBar";
import { CategoryIndex } from "@/components/home/CategoryIndex";
import { Trending } from "@/components/home/Trending";
import { Collection } from "@/components/home/Collection";

export default function Home() {
  return (
    <>
      <Header />
      <main>
        <section style={{ maxWidth: 1480, margin: "0 auto", padding: "52px 48px 40px", display: "grid", gridTemplateColumns: "1.05fr .95fr", gap: 56, alignItems: "center" }}>
          <Hero />
          <EditorsPick />
        </section>
        <StatsBar />
        <CategoryIndex />
        <Trending />
        <Collection />
      </main>
      <Footer />
    </>
  );
}
```

- [ ] **Step 5: Run test + typecheck**

Run: `npm test -- home` → Expected: PASS. Run: `npx tsc --noEmit` → Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add components/home app/page.tsx
git commit -m "feat: home page (hero, editor's pick, stats, index, trending, collection)"
```

---

### Task B7: Category route + view

**Files:**
- Create: `components/category/CategoryView.tsx`
- Create: `app/c/[kind]/page.tsx`
- Test: `components/category/__tests__/category.test.tsx`

**Interfaces:**
- Consumes: `getByKind`, `ExtensionCategory` values, `Card`, `useLang`.
- Produces: `/c/skill|plugin|mcp` (SSG via `generateStaticParams`), search box + 13 category filter chips + grid + no-results, title from kind.

**Port:** Category view from mockup lines 129–153 (search 136, chips 139–141 → here chips are the 13 categories, tab style lines 394–399, no-results 151).

- [ ] **Step 1: Write the failing test** — `components/category/__tests__/category.test.tsx`

```tsx
import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ThemeProvider } from "@/components/providers/ThemeProvider";
import { LangProvider } from "@/components/providers/LangProvider";
import { CategoryView } from "@/components/category/CategoryView";

const wrap = (ui: React.ReactNode) => render(<ThemeProvider><LangProvider>{ui}</LangProvider></ThemeProvider>);

describe("CategoryView", () => {
  it("lists mcp entries and filters by search query", () => {
    wrap(<CategoryView kind="mcp" />);
    expect(screen.getByText("playwright-mcp")).toBeInTheDocument();
    fireEvent.change(screen.getByPlaceholderText(/搜索|Search/), { target: { value: "supabase" } });
    expect(screen.queryByText("playwright-mcp")).toBeNull();
    expect(screen.getByText("supabase-mcp")).toBeInTheDocument();
  });
  it("shows no-results for an impossible query", () => {
    wrap(<CategoryView kind="mcp" />);
    fireEvent.change(screen.getByPlaceholderText(/搜索|Search/), { target: { value: "zzzzz" } });
    expect(screen.getByText(/没有找到|No matching/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- category` → Expected: FAIL.

- [ ] **Step 3: Implement `components/category/CategoryView.tsx`** (Client; search + category chip state)

```tsx
"use client";
import { useState } from "react";
import type { ExtensionKindT } from "@/contract/types";
import { useLang } from "@/components/providers/LangProvider";
import { STRINGS } from "@/lib/i18n";
import { getByKind } from "@/lib/catalog";
import { Card } from "@/components/Card";
import { Header } from "@/components/Header";
import { Footer } from "@/components/Footer";

const CATS = ["search","developer","data","productivity","writing","communication","knowledge","files","design","automation","finance","utilities","other"];
const KIND_TITLE: Record<ExtensionKindT, string> = { skill: "Agent Skills", plugin: "Plugins", mcp: "MCP Servers" };

export function CategoryView({ kind }: { kind: ExtensionKindT }) {
  const { lang } = useLang();
  const t = STRINGS[lang];
  const [q, setQ] = useState("");
  const [cat, setCat] = useState<string>("all");
  const all = getByKind(kind);
  const query = q.trim().toLowerCase();
  const visible = all
    .filter((e) => cat === "all" || e.category === cat)
    .filter((e) => !query || `${e.name} ${e.description_en} ${e.description_zh} ${e.tags.join(" ")}`.toLowerCase().includes(query));
  return (
    <>
      <Header />
      <main style={{ maxWidth: 1480, margin: "0 auto", padding: "0 48px 76px" }}>
        <section style={{ padding: "56px 0 28px", borderBottom: "1px solid var(--hair-strong)" }}>
          <h1 style={{ fontFamily: "var(--font-cormorant), serif", fontWeight: 500, fontSize: 60, margin: 0 }}>{KIND_TITLE[kind]}</h1>
        </section>
        <section style={{ display: "flex", gap: 16, padding: "22px 0", borderBottom: "1px solid var(--hair)" }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10, flex: 1, background: "var(--panel)", border: "1px solid var(--hair)", borderRadius: 2, padding: "11px 15px" }}>
            <span style={{ color: "var(--taupe)" }}>⌕</span>
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={t.searchPh} style={{ flex: 1, border: "none", outline: "none", background: "transparent", fontSize: 14, color: "var(--ink)" }} />
          </div>
        </section>
        <section style={{ display: "flex", gap: 10, padding: "18px 0 28px", flexWrap: "wrap" }}>
          {["all", ...CATS].map((c) => (
            <span key={c} onClick={() => setCat(c)} style={{ fontSize: 12, padding: "8px 18px", borderRadius: 20, cursor: "pointer", color: cat === c ? "#FBF6EE" : "var(--ink-soft)", background: cat === c ? "var(--orange)" : "var(--panel)", border: cat === c ? "none" : "1px solid var(--hair)" }}>{c === "all" ? t.allCats : c}</span>
          ))}
        </section>
        {visible.length ? (
          <section style={{ display: "grid", gridTemplateColumns: "repeat(3,1fr)", gap: 20 }}>
            {visible.map((e) => <Card key={e.id} entry={e} />)}
          </section>
        ) : (
          <div style={{ textAlign: "center", padding: "60px 0", color: "var(--taupe)", fontSize: 15 }}>{t.noResults}</div>
        )}
      </main>
      <Footer />
    </>
  );
}
```

- [ ] **Step 4: Implement `app/c/[kind]/page.tsx`** (SSG)

```tsx
import { notFound } from "next/navigation";
import type { ExtensionKindT } from "@/contract/types";
import { CategoryView } from "@/components/category/CategoryView";

const KINDS: ExtensionKindT[] = ["skill", "plugin", "mcp"];
export function generateStaticParams() { return KINDS.map((kind) => ({ kind })); }

export default async function Page({ params }: { params: Promise<{ kind: string }> }) {
  const { kind } = await params;
  if (!KINDS.includes(kind as ExtensionKindT)) notFound();
  return <CategoryView kind={kind as ExtensionKindT} />;
}
```

- [ ] **Step 5: Run test + typecheck**

Run: `npm test -- category` → Expected: PASS. Run: `npx tsc --noEmit` → Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add components/category app/c
git commit -m "feat: category route with search and category filters"
```

---

### Task B8: Detail route + view

**Files:**
- Create: `components/detail/{Cover,Tabs,InstallSidebar,Related}.tsx`
- Create: `components/detail/DetailView.tsx`
- Create: `app/e/[...slug]/page.tsx`
- Test: `components/detail/__tests__/detail.test.tsx`

**Interfaces:**
- Consumes: `bySlug`, `related`, `getAll` (for params), `useLang`, `Card`.
- Produces: `/e/<owner>/<repo>` (SSG via `generateStaticParams` over all entries), cover, Overview/Security tabs, install sidebar (copy button), related grid.

**Port:** Detail from mockup lines 156–202 (cover 161, tabs 165–175, sidebar 177–188, related 190–200, sub-tab style 404–406). Security tab text from `sec_note_*` + `STRINGS.secReview/secReviewNote`.

- [ ] **Step 1: Write the failing test** — `components/detail/__tests__/detail.test.tsx`

```tsx
import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ThemeProvider } from "@/components/providers/ThemeProvider";
import { LangProvider } from "@/components/providers/LangProvider";
import { DetailView } from "@/components/detail/DetailView";
import { bySlug } from "@/lib/catalog";

const wrap = (ui: React.ReactNode) => render(<ThemeProvider><LangProvider>{ui}</LangProvider></ThemeProvider>);

describe("DetailView", () => {
  it("renders name, install command, and switches to the security tab", () => {
    const e = bySlug("microsoft/playwright-mcp")!;
    wrap(<DetailView entry={e} />);
    expect(screen.getByRole("heading", { name: "playwright-mcp" })).toBeInTheDocument();
    expect(screen.getByText("npx aleph add playwright-mcp")).toBeInTheDocument();
    fireEvent.click(screen.getByText(/安全|Security/));
    expect(screen.getByText(/审核|review/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- detail` → Expected: FAIL.

- [ ] **Step 3: Implement the detail components + `DetailView.tsx`**

`DetailView` is `"use client"`, holds the `tab` state (`overview|security`) and renders Cover, the title block, Tabs, the active panel, InstallSidebar (copies `entry.install_cmd` via `navigator.clipboard`), and Related (`related(entry,3)` → `Card`). Port styles from the cited mockup lines; bind bilingual text via `useLang` (`long_zh/en`, `sec_note_zh/en`). Provide the full component (model the structure on the mockup’s detail `<main>` 156–202). `View source ↗` links to `entry.repo_url`. The "Category" meta row (mockup `cur.catName`, which does not exist for the 13-value contract taxonomy) renders `CATEGORY_LABELS[entry.category][lang]` from `@/lib/i18n`.

- [ ] **Step 4: Implement `app/e/[...slug]/page.tsx`** (SSG over all entries)

```tsx
import { notFound } from "next/navigation";
import { getAll, slugForEntry, bySlug } from "@/lib/catalog";
import { Header } from "@/components/Header";
import { Footer } from "@/components/Footer";
import { DetailView } from "@/components/detail/DetailView";

export function generateStaticParams() {
  return getAll().map((e) => ({ slug: slugForEntry(e).split("/") }));
}

export default async function Page({ params }: { params: Promise<{ slug: string[] }> }) {
  const { slug } = await params;
  const entry = bySlug(slug.join("/"));
  if (!entry) notFound();
  return <><Header /><DetailView entry={entry} /><Footer /></>;
}
```

- [ ] **Step 5: Run test + typecheck**

Run: `npm test -- detail` → Expected: PASS. Run: `npx tsc --noEmit` → Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add components/detail app/e
git commit -m "feat: detail route (cover, overview/security tabs, install sidebar, related)"
```

---

### Task B9: Submit form (prefilled GitHub issue)

**Files:**
- Create: `lib/submit.ts`
- Create: `components/SubmitForm.tsx`
- Create: `app/submit/page.tsx`
- Test: `lib/__tests__/submit.test.ts`

**Interfaces:**
- Produces: `buildIssueUrl(input): string` (prefilled GH issue against this repo), `<SubmitForm />`, `/submit` page.

**Repo for issues:** `https://github.com/rootazero/Aleph-Hub` (the project repo; adjust if the canonical remote differs). Issue template `suggest-extension` (created in the Automation plan); the URL uses `?template=suggest-extension.yml&title=...&body=...`.

- [ ] **Step 1: Write the failing test** — `lib/__tests__/submit.test.ts`

```ts
import { describe, it, expect } from "vitest";
import { buildIssueUrl } from "@/lib/submit";

describe("buildIssueUrl", () => {
  it("builds a prefilled GitHub issue URL", () => {
    const url = buildIssueUrl({ repo: "https://github.com/a/b", name: "b", category: "developer", description: "x", tags: "ci, git" });
    expect(url).toContain("https://github.com/rootazero/Aleph-Hub/issues/new");
    expect(url).toContain("template=suggest-extension.yml");
    // URLSearchParams encodes spaces as "+" (form-encoding; GitHub reads it as space).
    const decoded = decodeURIComponent(url).replace(/\+/g, " ");
    expect(decoded).toContain("Repo: https://github.com/a/b");
    expect(decoded).toContain("Name: b");
    expect(decoded).toContain("Suggest extension: b"); // prefilled title
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- submit` → Expected: FAIL.

- [ ] **Step 3: Implement `lib/submit.ts`**

```ts
const REPO = "https://github.com/rootazero/Aleph-Hub";

export interface SubmitInput { repo: string; name: string; category: string; description: string; tags: string; }

export function buildIssueUrl(input: SubmitInput): string {
  const body = [
    `Repo: ${input.repo}`, `Name: ${input.name}`, `Category: ${input.category}`,
    `Description: ${input.description}`, `Tags: ${input.tags}`,
  ].join("\n");
  const params = new URLSearchParams({
    template: "suggest-extension.yml",
    title: `Suggest extension: ${input.name}`,
    body,
  });
  return `${REPO}/issues/new?${params.toString()}`;
}
```

- [ ] **Step 4: Implement `components/SubmitForm.tsx` + `app/submit/page.tsx`**

`SubmitForm` is `"use client"`: controlled inputs for repo/name/category(select of 13)/description/tags, a submit button that `window.open(buildIssueUrl(...))`, a cancel link to `/`. Port markup/styles from mockup lines 205–225. `app/submit/page.tsx` renders `<Header /><SubmitForm /><Footer />`.

- [ ] **Step 5: Run test + typecheck**

Run: `npm test -- submit` → Expected: PASS. Run: `npx tsc --noEmit` → Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add lib/submit.ts components/SubmitForm.tsx app/submit
git commit -m "feat: submit form building a prefilled github issue"
```

---

### Task B10: Vercel cache header + production build

**Files:**
- Create: `vercel.json`
- Test: build verification (manual command)

**Interfaces:**
- Produces: `/catalog.json` served `application/json` with revalidating cache (not immutable); a green `next build`.

- [ ] **Step 1: Create `vercel.json`**

```json
{
  "headers": [
    {
      "source": "/catalog.json",
      "headers": [
        { "key": "Cache-Control", "value": "public, max-age=0, must-revalidate" },
        { "key": "Content-Type", "value": "application/json; charset=utf-8" }
      ]
    }
  ]
}
```

- [ ] **Step 2: Run the production build**

Run: `npm run build` → Expected: PASS; output shows `/`, `/c/[kind]` (3), `/e/[...slug]` (12), `/submit` as static/prerendered.

- [ ] **Step 3: Run the full unit suite + typecheck**

Run: `npm test` → Expected: all PASS. Run: `npx tsc --noEmit` → Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add vercel.json
git commit -m "chore: vercel cache-control override for /catalog.json"
```

---

### Task B11: Playwright e2e

**Files:**
- Create: `tests/e2e/site.spec.ts`

**Interfaces:**
- Consumes: the built site (Playwright `webServer` builds + serves).
- Produces: e2e coverage for the four views + theme/lang toggles + the served contract artifact.

- [ ] **Step 1: Write the e2e spec** — `tests/e2e/site.spec.ts`

```ts
import { test, expect } from "@playwright/test";

test("home renders and navigates to a detail page", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByText("ALEPH HUB").first()).toBeVisible();
  await page.getByText("goose").first().click();
  await expect(page).toHaveURL(/\/e\/block\/goose/);
  await expect(page.getByRole("heading", { name: "goose" })).toBeVisible();
});

test("language toggle switches submit label", async ({ page }) => {
  await page.goto("/");
  await page.getByText("EN", { exact: true }).click();
  await expect(page.getByText("Submit").first()).toBeVisible();
});

test("theme toggle flips data-theme with no FOUC flag", async ({ page }) => {
  await page.goto("/");
  const before = await page.evaluate(() => document.documentElement.dataset.theme);
  await page.locator("header span", { hasText: /☾|☼/ }).click();
  const after = await page.evaluate(() => document.documentElement.dataset.theme);
  expect(before).not.toBe(after);
});

test("category page filters by search", async ({ page }) => {
  await page.goto("/c/mcp");
  await page.getByPlaceholder(/搜索|Search/).fill("supabase");
  await expect(page.getByText("supabase-mcp")).toBeVisible();
  await expect(page.getByText("playwright-mcp")).toHaveCount(0);
});

test("contract artifact is served as valid JSON", async ({ request }) => {
  const res = await request.get("/catalog.json");
  expect(res.status()).toBe(200);
  expect(res.headers()["content-type"]).toContain("application/json");
  const json = await res.json();
  expect(json.manifest.hub_id).toBe("aleph-hub");
  expect(json.entries).toHaveLength(12);
});
```

- [ ] **Step 2: Run the e2e suite**

Run: `npm run e2e` → Expected: all 5 tests PASS (Playwright builds + serves automatically).

- [ ] **Step 3: Commit**

```bash
git add tests/e2e/site.spec.ts
git commit -m "test: playwright e2e for the four views and served artifact"
```

---

## Self-Review (completed during authoring)

- **Spec coverage:** Phase A covers spec §4 (contract), §4.5 McpTransport (pinned), §4.7 validation layering (zod gate), §5 two-artifact split (fixtures for both), §7.4 taxonomy (kind nav + tag collections). Phase B covers spec §7 (all four views), §7.1 pre-paint theme + no server-text branching, §7.4 trust label remap, §8.2 vercel.json cache override. Pipeline (§6), automation/cron (§6.7/§6.8/§8.1), and live stars/SEO (§7.5/P4) are **explicitly deferred to follow-on plans** (see below) — not gaps.
- **Type consistency:** `SiteEntryT` (A6) is consumed everywhere; `slugForEntry`/`idFromSlug`/`bySlug` (B2) used identically in B5/B7/B8; `Theme`/`Lang` types shared from `lib/`. `kind` values `skill|plugin|mcp` consistent across nav (B4), category params (B7), and counts (B2).
- **Placeholder scan:** The only "port from mockup lines X–Y" instructions point at concrete, in-repo source with exact line numbers + the load-bearing values inlined (palettes, fonts, trust map, data bindings) — not logic placeholders. All schemas, selectors, the issue-URL builder, and every test are complete code.

## Follow-on plans (to write after this milestone lands)

1. **Pipeline plan** (spec §6): source adapters (github/clawhub/hermes), normalize, GitHub-API-canonicalized dedup, LLM curation, install_spec inference + **semantic verification**, trust heuristic, enrich, injection safety, emit (+ floor gate), incremental/budgeted crawl — replaces the fixtures with a real generator.
2. **Automation plan** (spec §8): `pipeline.yml` (PAT commit + Vercel Deploy Hook + keepalive), `ci.yml`, `.github/ISSUE_TEMPLATE/suggest-extension.yml`, secrets, external freshness monitor, domain `hub.heyaleph.com`.
3. **Polish plan** (spec §7.5/P4): client-side live stars, SEO/OG images, a11y, Lighthouse.
