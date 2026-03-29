# E2E Verify Skill Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite the Aleph-specific `aleph-e2e-verify` skill into a generic `e2e-verify` skill with intent analysis, negative/boundary scenario design, environment manipulation protocol, and multi-probe verification.

**Architecture:** Single SKILL.md contains the complete generic methodology (8-step flow). Project-specific materials (Aleph WS protocol doc, Python client) remain in `references/` and `scripts/` as optional accelerators. The skill is a prompt artifact — no compiled code, no runtime dependencies.

**Tech Stack:** Markdown (SKILL.md), Python (existing client retained), no new dependencies.

**Spec:** `docs/superpowers/specs/2026-03-29-e2e-verify-skill-redesign.md`

---

### Task 1: Create new skill directory and move existing assets

**Files:**
- Create: `.claude/skills/e2e-verify/` (new directory)
- Move: `.claude/skills/aleph-e2e-verify/references/websocket-protocol.md` → `.claude/skills/e2e-verify/references/websocket-protocol.md`
- Move: `.claude/skills/aleph-e2e-verify/scripts/aleph_e2e_client.py` → `.claude/skills/e2e-verify/scripts/aleph_e2e_client.py`
- Delete: `.claude/skills/aleph-e2e-verify/` (after moves complete)

- [ ] **Step 1: Create new directory structure**

```bash
mkdir -p .claude/skills/e2e-verify/references
mkdir -p .claude/skills/e2e-verify/scripts
```

- [ ] **Step 2: Copy existing assets to new location**

```bash
cp .claude/skills/aleph-e2e-verify/references/websocket-protocol.md .claude/skills/e2e-verify/references/
cp .claude/skills/aleph-e2e-verify/scripts/aleph_e2e_client.py .claude/skills/e2e-verify/scripts/
```

- [ ] **Step 3: Verify copies are identical**

```bash
diff .claude/skills/aleph-e2e-verify/references/websocket-protocol.md .claude/skills/e2e-verify/references/websocket-protocol.md
diff .claude/skills/aleph-e2e-verify/scripts/aleph_e2e_client.py .claude/skills/e2e-verify/scripts/aleph_e2e_client.py
```
Expected: no output (files identical)

- [ ] **Step 4: Commit (git rm handles old directory removal)**

Note: `__pycache__/` in the old directory is not git-tracked and can be ignored.

```bash
git add .claude/skills/e2e-verify/
git rm -rf .claude/skills/aleph-e2e-verify/
rm -rf .claude/skills/aleph-e2e-verify/  # clean up any untracked files like __pycache__
git commit -m "skill: rename aleph-e2e-verify → e2e-verify"
```

---

### Task 2: Write the new SKILL.md

This is the core deliverable — the complete generic methodology in a single file.

**Files:**
- Create: `.claude/skills/e2e-verify/SKILL.md`

**Source material:** Spec at `docs/superpowers/specs/2026-03-29-e2e-verify-skill-redesign.md` — all content comes from there.

- [ ] **Step 1: Write SKILL.md frontmatter**

The `description` field is critical — it determines when Claude Code triggers this skill. Must cover all trigger phrases from the old skill plus new generic triggers.

```yaml
---
name: e2e-verify
description: >
  E2E verification against a live server with intent-aware scenario design.
  Use when: user says "verify", "e2e test", "e2e verify", "production verify",
  "validate module", "生产验证", "验证模块", or after completing a major feature.
  Analyzes git diff + user instructions to design positive, negative, and boundary
  test scenarios. Manipulates environment (with backup/restore) to test error handling,
  fallback, and degradation paths. Multi-probe verification: log + API + state + process.
  Flags: --debug (verbose output), --skip-build (reuse last binary).
---
```

- [ ] **Step 2: Write the 8-step overview table**

Content from spec "New Flow: 8 Steps" section. Include the "Step 0 decides everything" principle.

- [ ] **Step 3: Write Step 0 — Intent Analysis**

Content from spec "Step 0: Intent Analysis" section. Three sources (user instructions, git diff, related docs). Include the N determination strategy and test target list output format.

- [ ] **Step 4: Write Steps 1-4 — Infrastructure (Kill/Build/Start/Verify)**

Adapt from old SKILL.md Steps 1-4 but make generic. **Strip all Aleph-specific elements:**
- `pkill -f aleph-server` → generic "find and kill project server process"
- `just build` / `just build-debug` → "discover build command from Makefile/justfile/package.json"
- `target/release/aleph-server start &` → "discover start command from project files"
- Port 18790 → "discover port from config or startup logs"
- `sqlite3 ~/.aleph/data/security.db` shared token auth → "discover auth method from project code"
- `lsof -i -P | grep aleph` → generic port check

**Replace with generic patterns:**
- Step 1 (Kill): "Read CLAUDE.md for process management rules. Find running server processes and kill them."
- Step 2 (Build): "Read Makefile/justfile/package.json to find build command. Run it."
- Step 3 (Start): "Start service with elevated log level. Redirect stdout/stderr to temp file for log capture." Include example: `RUST_LOG=debug ./start-command > /tmp/e2e-server.log 2>&1 &`
- Step 4 (Verify): "Confirm process alive + port listening. If auth required, obtain credentials."

Include project discovery guidance: "Read README / Makefile / justfile / CLAUDE.md to discover build/start commands. Check references/ and scripts/ for existing test tools."

- [ ] **Step 5: Write Step 5 — Scenario Design**

Content from spec "Step 5: Scenario Design — Test Cards" section. Include:
- Test card format template
- Mandatory scenario rules table
- Language-specific error-handling pattern keywords
- "When no negative scenarios exist" clause

- [ ] **Step 6: Write Step 6 — Execute (Environment Manipulation Protocol)**

Content from spec "Step 6: Environment Manipulation Protocol" section. Include:
- Phase A (Backup), Phase B (Manipulate + Test + Collect), Phase C (Restore)
- Manipulation methods table
- Safety redlines
- Phase C step ordering (verify checksums BEFORE deleting backups)

- [ ] **Step 7: Write Step 7 — Report**

Content from spec "Report Format" section. Include evidence-based report template and FAIL output enhancement.

- [ ] **Step 8: Write Probe System section (place BETWEEN Step 5 and Step 6 in the document)**

This section is referenced by both Scenario Design (Step 5) and Execute (Step 6), so it must appear before Step 6 in the SKILL.md document flow. Content from spec "Probe System" section. Include:
- Four probe types table
- Probe selection rules per scenario type
- Critical constraint about negative scenarios requiring log probes
- Log capture method

- [ ] **Step 9: Write Red Flags table**

Content from spec "Red Flags" section — the thinking traps table.

- [ ] **Step 10: Review the complete SKILL.md**

Read the entire file end-to-end. Verify:
- All 8 steps are covered
- Frontmatter description matches all trigger phrases
- No Aleph-specific hardcoded references in the generic sections
- references/ and scripts/ integration points are mentioned in Steps 4, 5, 6

- [ ] **Step 11: Commit**

```bash
git add .claude/skills/e2e-verify/SKILL.md
git commit -m "skill: rewrite e2e-verify with intent analysis and negative scenarios"
```

---

### Task 3: Update CLAUDE.md skill reference

**Files:**
- Modify: `CLAUDE.md` (if it references `aleph-e2e-verify`)

- [ ] **Step 1: Check if CLAUDE.md references the old skill name**

```bash
grep -n "aleph-e2e-verify" CLAUDE.md
```

- [ ] **Step 2: Update any references to use new name `e2e-verify`**

If found, replace `aleph-e2e-verify` → `e2e-verify` and update the description to reflect the generic nature.

- [ ] **Step 3: Commit (if changes made)**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md skill reference to e2e-verify"
```

---

### Task 4: Verify skill loads correctly

- [ ] **Step 1: Check skill file structure is valid**

```bash
ls -la .claude/skills/e2e-verify/
ls -la .claude/skills/e2e-verify/references/
ls -la .claude/skills/e2e-verify/scripts/
```

Expected: SKILL.md exists, references/websocket-protocol.md exists, scripts/aleph_e2e_client.py exists.

- [ ] **Step 2: Verify SKILL.md frontmatter is valid YAML**

Read the first few lines and confirm `name`, `description` fields are present and properly formatted.

- [ ] **Step 3: Verify old skill directory is fully removed**

```bash
ls .claude/skills/aleph-e2e-verify/ 2>&1
```

Expected: "No such file or directory"
