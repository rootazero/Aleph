# Skills-First Discovery Pivot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refocus the pipeline's discovery on skills by prioritizing the pre-curated HermesAtlas source, narrowing the github net to skill-oriented seeds, and parking ClawHub — via a one-line config reorder plus a seeds-data swap.

**Architecture:** Two surgical changes: (1) reorder `CONFIG.SOURCE_PRIORITY` so pre-curated sources outrank the raw github net (governs dedup `via`-badge tie-breaking); (2) replace the MCP-oriented queries/awesome-lists in `data/seeds/github.json` with skill topics + skill awesome-lists. No contract change, no scraper change, no new dependency.

**Tech Stack:** TypeScript, tsx, Vitest, Node 24, JSON seed data.

## Global Constraints

- Do NOT change `contract/schema.ts`, the install_spec inference/verification, or the Hermes/ClawHub/GitHub scraper code.
- ClawHub stays in the active source list (do NOT remove it) — removal would trip the `perSourceGuard` (it throws when a known source drops >50% vs the last run; ClawHub at ~19 → 0 would throw).
- `SOURCE_PRIORITY` must be exactly `["hermes-atlas", "clawhub", "github"]` (pre-curated sources first).
- New github seeds — queries: `topic:claude-skills`, `topic:claude-skill`, `topic:agent-skills`, `topic:claude-code-skill`; seed lists: `https://github.com/ComposioHQ/awesome-claude-skills`, `https://github.com/travisvn/awesome-claude-skills`.
- No new dependencies. End state: `npm run typecheck` clean and `npx vitest run` fully green.

---

### Task 1: Skills-first discovery (priority reorder + seed swap)

**Files:**
- Modify: `scripts/pipeline/config.ts:8` (`SOURCE_PRIORITY`)
- Modify: `data/seeds/github.json` (queries + seeds)
- Test: `scripts/pipeline/__tests__/config.test.ts:14` (priority[0] assertion)
- Test: `scripts/pipeline/__tests__/dedup.test.ts` (the "keeps the higher-priority source on a tie" test)

**Interfaces:**
- Consumes: `CONFIG.SOURCE_PRIORITY` (read by `dedup.ts` `sourceRank`, which maps a `via` to its source id — `via.startsWith("github:") ? "github" : via` — and ranks by index; lower index wins a tie). `via()` maps source id `hermesatlas` → the string `"hermes-atlas"`.
- Produces: nothing new (no new exports). Behavior change only: pre-curated sources now win dedup ties; discovery seeds are skill-focused.

TDD order: update the two assertions to the post-change expectations first (they go RED against the current `["github", "clawhub", "hermes-atlas"]` config), then flip the config to GREEN, then swap the seed data (which no test reads, so the suite stays green).

- [ ] **Step 1: Update the `config.test.ts` priority assertion (RED)**

In `scripts/pipeline/__tests__/config.test.ts`, line 14, change:

```ts
    expect(CONFIG.SOURCE_PRIORITY[0]).toBe("github");
```

to:

```ts
    expect(CONFIG.SOURCE_PRIORITY[0]).toBe("hermes-atlas");
```

- [ ] **Step 2: Update the `dedup.test.ts` tie-break test (RED)**

In `scripts/pipeline/__tests__/dedup.test.ts`, replace the entire `it("keeps the higher-priority source on a tie", ...)` test with this version (pairs the new primary source against github and expects the primary to win):

```ts
  it("keeps the higher-priority source on a tie", async () => {
    const gh = fakeGh({ "acme/foo": meta("acme/foo") });
    const out = await dedupe([
      { repo_url: "https://github.com/acme/foo", via: "github:acme", raw: {} },
      { repo_url: "https://github.com/acme/foo", via: "hermes-atlas", raw: {} },
    ], gh);
    expect(out).toHaveLength(1);
    expect(out[0].via).toBe("hermes-atlas");
  });
```

- [ ] **Step 3: Run the two tests to verify they fail (RED)**

Run: `npx vitest run scripts/pipeline/__tests__/config.test.ts scripts/pipeline/__tests__/dedup.test.ts`
Expected: FAIL — config test gets `"github"` not `"hermes-atlas"`; dedup tie-break gets `"github:acme"` not `"hermes-atlas"` (under the current config, github outranks hermes-atlas).

- [ ] **Step 4: Reorder `SOURCE_PRIORITY` in `config.ts` (GREEN for priority)**

In `scripts/pipeline/config.ts`, line 8, change:

```ts
  SOURCE_PRIORITY: ["github", "clawhub", "hermes-atlas"] as const,
```

to:

```ts
  SOURCE_PRIORITY: ["hermes-atlas", "clawhub", "github"] as const,
```

- [ ] **Step 5: Run the two tests to verify they pass (GREEN)**

Run: `npx vitest run scripts/pipeline/__tests__/config.test.ts scripts/pipeline/__tests__/dedup.test.ts`
Expected: PASS — both tests green.

- [ ] **Step 6: Swap the github seeds to skill-focused values**

Replace the entire contents of `data/seeds/github.json` with:

```json
{
  "queries": ["topic:claude-skills", "topic:claude-skill", "topic:agent-skills", "topic:claude-code-skill"],
  "seeds": [
    "https://github.com/ComposioHQ/awesome-claude-skills",
    "https://github.com/travisvn/awesome-claude-skills"
  ]
}
```

- [ ] **Step 7: Verify the seed file is valid JSON**

Run: `node -e 'const s=require("./data/seeds/github.json"); if(s.queries.length!==4||s.seeds.length!==2) throw new Error("unexpected shape"); console.log("seeds OK:", s.queries.length, "queries,", s.seeds.length, "lists")'`
Expected: `seeds OK: 4 queries, 2 lists` (no parse/throw).

- [ ] **Step 8: Run the full suite and typecheck**

Run: `npm run typecheck && npx vitest run`
Expected: typecheck clean; full suite green. The seed-data change affects no test (every source test uses inline fixture seeds, not `data/seeds/github.json`), so the only behavioral test deltas are the two updated in Steps 1-2.

- [ ] **Step 9: Commit**

```bash
git add scripts/pipeline/config.ts data/seeds/github.json scripts/pipeline/__tests__/config.test.ts scripts/pipeline/__tests__/dedup.test.ts
git commit -m "feat(pipeline): skills-first discovery (Hermes primary, github→skill seeds, ClawHub parked)"
```

---

## Verification (post-implementation)

Controller checks after the task lands — not pipeline steps to run blindly (a real run costs GitHub API calls; do not run it while another `gh` session shares the token's quota):

1. `npm run typecheck` clean, `npx vitest run` fully green.
2. The next `pipeline` run discovers a skill-dominated candidate set: `data/queue/to-curate.json` should show github candidates drawn from the new skill topics/lists, and `data/cache/per-source.json` should show `hermesatlas` still contributing ~173 with no source-guard failure. ClawHub stays ~19 (parked).
3. The catalog's existing 13 entries are unaffected (the content hash is unchanged unless curation changes).

## Notes on coverage vs. the spec

- Spec change 1 (`data/seeds/github.json`) → Task 1, Step 6.
- Spec change 2 (`config.ts` `SOURCE_PRIORITY`) → Task 1, Step 4.
- Spec change 3 (two priority assertions) → Task 1, Steps 1-2.
- Spec D1-D5 (locked decisions) → honored: D2/D4 by the priority order; D3 by the seed swap; D1/D5 require no code (skills-first is a curation behavior; no contract/scraper change is made).
- The agent's skills-first curation behavior (D1) is not a code change — it governs which queued candidates get curation records written, which happens outside this plan.
