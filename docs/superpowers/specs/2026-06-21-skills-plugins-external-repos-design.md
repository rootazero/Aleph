# Skills & Plugins → External Repos + Hub-Managed + Offline Fallback

**Date:** 2026-06-21
**Status:** Approved (design); pending implementation plan
**Scope boundary:** Spans three repos. The seam between them is (a) the two new content repos as git submodules + clone targets, and (b) the Aleph-Hub catalog artifact contract (unchanged wire schema; only the first-party `git_url`/`subdir` values change).

- **Aleph (main)** — build embeds via submodule snapshot; runtime bootstrap/sync; hub install routing.
- **Aleph-skills (new)** — `rootazero/Aleph-skills`, root = current `skills/` contents.
- **Aleph-plugins (new)** — `rootazero/Aleph-plugins`, root = current `plugins/` contents.
- **Aleph-Hub** — only `data/seeds/aleph-official.json` + a `regen-firstparty` run.

---

## 1. Problem & Goal

Today the 37 official skills (`skills/`) and 8 official plugins (`plugins/`) live as subdirectories **inside the main Aleph repo**. At build time `include_dir!` embeds them into the `aleph-server` binary (`src/bundled/mod.rs`); at startup `extract_bundled_content()` (`src/bundled/extractor.rs`, called from `src/bin/aleph-server/commands/start/helpers.rs:259`) extracts them to `~/.aleph/skills/` and `~/.aleph/plugins/cache/aleph-official/`, version-gated by `BUNDLED_VERSION` (= `ALEPH_VERSION`) and protected by a manifest + reconcile pass.

**Goal:** Move the official skills/plugins out of the main repo into two independent, GitHub-synced repos, so they:

1. Have an independent version lifecycle (no longer tied to a server release).
2. Are curated/listed by the Aleph Hub catalog **the same way third-party extensions are** (the first-party track already exists at `scripts/pipeline/firstParty.ts`, currently pointing the `skill` group at the main repo).
3. Are obtainable at install time via `git clone` / sync from the remote.

**Honest reconciliation of the original ask.** The original request said "build no longer bundles skills/plugins." The user then chose an **offline fallback** (§2 D1). An offline fallback requires an embedded source — so **the build still embeds a snapshot** (binary size is essentially unchanged). What is actually achieved: source-of-truth moves out of the main repo, independent versioning, hub management, and latest-on-online-install. "No bundling" is intentionally softened to "no source in main repo; build still embeds a fallback snapshot sourced from the external repos via submodule."

---

## 2. Locked Decisions

- **D1. Default availability = bulk clone + offline fallback.** A fresh machine tries to `git clone` the external repos; on failure (offline / no network / unreachable) it falls back to the embedded snapshot. The full desktop App therefore still works offline / zero-config.
- **D2. Reuse the extractor; only swap the source.** `~/.aleph/skills` and `~/.aleph/plugins` are **never** git working copies. The existing extractor/manifest/reconcile/atomic-swap/prune machinery is reused verbatim; only the *source* it reads from changes — from an embedded `include_dir::Dir` to **either** a freshly cloned on-disk checkout (online) **or** the embedded `Dir` (offline fallback).
- **D3. Sync cadence = explicit trigger.** First bootstrap clones; the startup path does **not** auto-pull thereafter. Updates are explicit: a CLI/RPC `skills update` / `plugins update` tool (LLM-callable, R8) + a per-entry "update" action in the Hub.
- **D4. Migration = fresh snapshot; we create GitHub repos and push.** Copy current contents into the two (already-present, empty) sibling dirs, `git init`, `gh repo create rootazero/Aleph-{skills,plugins} --public`, push. No git history is preserved (content files, low history value).
- **D5. Offline fallback stays in sync via git submodule.** The main repo's `skills/` and `plugins/` paths become submodules pointing at the two new repos. `include_dir!` paths are unchanged. The embedded snapshot is upgraded by `git submodule update --remote` + committing the pointer bump in the release workflow → each release embeds the latest-at-release content.
- **D6. App upgrade/reinstall upgrades skills via the embedded snapshot.** A CalVer version bump makes `manifest.bundled_version != BUNDLED_VERSION`, which already triggers re-extraction of the embedded snapshot — offline, deterministic, no network. `git pull` is reserved for first bootstrap (D1) and explicit update (D3).

### Default assumptions (stated; flippable)

- **A1. Owner = `rootazero`** (matches `data/seeds/aleph-official.json` and `plugins-index.json`).
- **A2. Runtime clone tracks `main`** (not a pinned tag).
- **A3. Repo root = current directory contents** (each skill/plugin leaf at repo root; no extra nested `skills/`/`plugins/` level). The Hub seed's `subdir_prefix` becomes `""`.
- **A4. Name-collision policy: user/third-party wins, official yields** (the existing reconcile behavior). The official sync skips any name owned by a `Local`/`Community` skill.

---

## 3. Architecture

```
SOURCE OF TRUTH (independent GitHub repos)
  rootazero/Aleph-skills      root = 37 skill leaves (current skills/ contents)
  rootazero/Aleph-plugins     root = 8 plugin leaves + .claude-plugin/marketplace.toml
        │
        ├── [git submodule, pinned commit] ── main repo skills/ + plugins/
        │        └── include_dir!("$CARGO_MANIFEST_DIR/skills"|"plugins")
        │                 └── compiled into aleph-server  → EMBEDDED FALLBACK SNAPSHOT
        │
        └── [git clone / fetch+reset --hard origin/main] ── runtime, into an
                 ISOLATED official-only checkout under ~/.aleph/cache/, then
                 extractor copies leaves into ~/.aleph (manifest-gated)

ALEPH-HUB (Next.js catalog site)
  data/seeds/aleph-official.json   git_url → the two new repos; subdir_prefix → ""
        └── scripts/pipeline/firstParty.ts → catalog.json
                 (official entries listed exactly like third-party: git_dir InstallSpec)
```

**Three runtime legs (mutually non-conflicting):**

| Trigger | Mechanism | Result | Network |
|---|---|---|---|
| **First install** (`~/.aleph/skills` absent) | try `git clone` latest `main` → on failure use embedded `Dir` → feed extractor | latest `X` (offline → embedded `Z`) | yes (with fallback) |
| **App upgrade / reinstall** (CalVer bump) | `bundled_version != BUNDLED_VERSION` → re-extract **embedded** snapshot `Z` | `Z` (= latest at that release) | **no** |
| **Same version, dir present** | nothing (D3) | unchanged | no |
| **Explicit update** (CLI/RPC/Hub button) | `fetch + reset --hard origin/main` in the isolated checkout → re-extract | latest `X` | yes |

Because the release workflow runs `git submodule update --remote` before building (D5), the embedded `Z` of any release equals the remote `main` at release time; content newer than that is delivered only by first-install clone or explicit update. **Edge:** if a user explicitly pulled to `X` and later installs an App whose embedded `Z` is older, the upgrade re-extract resets official skills to `Z` (consistent with today's version-gate overwrite of official content). User-added skills are unaffected (reconcile, §6).

---

## 4. Conflict Handling / Multi-Source Isolation

`~/.aleph/skills` and `~/.aleph/plugins` are **plain directories, not git working copies** (D2). This dissolves the git-pull-conflict concern structurally, in three layers:

1. **git layer — no merge ever.** The only `.git` is the isolated official-only checkout (e.g. `~/.aleph/cache/aleph-skills-checkout/`), which contains *only* the official repo and is never written by the user or by third-party installs. Sync uses `fetch + reset --hard origin/main` (or re-clone) — never `merge` — so a merge conflict is structurally impossible.
2. **landing layer — manifest-gated file copy.** Merging the checkout into `~/.aleph/skills/` is the existing `extract_skills` copy, gated by `manifest.source`: it only extract/overwrite/prune `source == Official` entries and **explicitly skips** `Local`/`Community`. Third-party and user skills are untouched by official sync.
3. **plugins — physical separation.** Official plugins land in `~/.aleph/plugins/cache/aleph-official/`; third-party plugins land in *other* marketplace caches / scopes. Official sync only touches `aleph-official/`. Zero overlap.

**Only real conflict surface — name collision** (a third-party and an official skill share a directory name): handled by the existing rule — official extraction skips a name already owned by a non-`Official` skill (**A4: user/third-party wins**).

**Third-party update** uses the same pattern: each third-party source has its *own* isolated checkout; updating re-fetches that source and re-copies (tagged `Community`). Per-source isolation ⇒ never cross-contaminates.

The `manifest.source` field must therefore reliably distinguish **Official / Community / Local**. Today the extractor treats it as Official-vs-non-Official (Community+Local both skipped by official sync), which already satisfies §4. The hub `git_dir → skill` install (§5 C) must stamp installed third-party skills as `Community`.

---

## 5. Components & Changes

### A. Migration (one-time, executed during plan)
1. Copy current `skills/` contents → `/Volumes/TBU4/Workspace/Aleph-skills/` → `git init` → `gh repo create rootazero/Aleph-skills --public --source=. --push`.
2. Copy current `plugins/` contents (incl. `.claude-plugin/marketplace.toml`, `.gitignore`) → `/Volumes/TBU4/Workspace/Aleph-plugins/` → same.
3. Main repo: `git rm -r --cached skills plugins` then re-add at the **same paths** as submodules (`git submodule add <url> skills`, `… plugins`). `include_dir!` paths are unchanged.

### B. Build / release
4. `.gitmodules` added; release workflow (`.github/workflows/*release*`) gains a pre-build step: `git submodule update --init --remote` + commit the pointer bump (this is the D5 sync guarantee). Optional CI freshness check: warn/fail if submodule pointer lags remote `main`.
5. `build.rs`: add `cargo:rerun-if-changed=skills` and `=plugins` so the embedded snapshot re-embeds when submodule content changes (fixes the existing "edit a skill, binary not re-embedded without clean build" wart).

### C. Runtime (`src/bundled/`)
6. `extractor.rs`: extract a filesystem-dir source path that mirrors the existing `include_dir::Dir` traversal (manifest / reconcile / atomic-swap / prune reused). The public entry generalizes to: *source = cloned checkout dir if available, else embedded `Dir`.* Behavior with the embedded source must remain byte-for-byte identical to today (regression-guarded).
7. New `src/bundled/sync.rs` (uses `git2`, already a dependency — no system `git` needed):
   - `bootstrap`: if target absent, `git clone` `main` into an isolated cache checkout with a bounded timeout → success feeds the extractor; failure logs `warn` and falls back to the embedded snapshot. Never panics, never blocks startup (P7).
   - `update`: `fetch + reset --hard origin/main` (or re-clone) in the isolated checkout → re-extract. No `merge`.
   - Plugins follow the same pattern, landing at `~/.aleph/plugins/cache/aleph-official/`.

### D. Hub integration
8. Aleph-Hub `data/seeds/aleph-official.json`: `skill` group `git_url` → `https://github.com/rootazero/Aleph-skills`, `subdir_prefix` → `""`, `tree_url` updated; `plugin` group `subdir_prefix` → `""` (root layout, A3). Run `regen-firstparty` to refresh `public/catalog.json` + `data/site-catalog.json`.
9. Client `src/hub/install.rs`: add the **`InstallSpec::GitDir` → skill** branch (today `GitDir` only routes to the marketplace *plugin* path). It must `git2`-clone the entry's `subdir` from `git_url`/`git_ref` into an isolated checkout and copy that leaf into `~/.aleph/skills/`, stamping `manifest.source = Community` (or `Official` for first-party). This is the gap that currently makes official/third-party **skills** un-installable via the Hub.
10. New RPC + CLI `skills update` / `plugins update` (R8 tool, LLM-callable) + a per-entry "update" action in the Hub UI, both invoking §C `update`.

### E. Cleanup to verify (not assume)
11. `plugins-index.json` (main repo root) already references `github:rootazero/Aleph-plugins` release assets — verify whether it is still consumed; update or remove accordingly. README/CHANGELOG references to in-repo `skills/`/`plugins/` updated to point at the new repos.

---

## 6. Error Handling (P7 defensive)
- Clone/fetch failure, no network, timeout → `warn` + fall back to embedded snapshot; startup never blocked, never panics.
- Partial clone → temp checkout + atomic swap (reuse `swap_dir_into_place`).
- User-added skills protected by the existing reconcile pass; official sync never overwrites/prunes non-`Official` entries.
- `git2`/libgit2 is vendored — no system `git` dependency. Public repos → anonymous HTTPS clone.

---

## 7. Testing
- **Unit:** extract-from-filesystem-dir (symmetric to existing `Dir` tests, asserting identical output for the embedded path); bootstrap clone-failure → embedded-fallback; `GitDir → skill` install lands a leaf + stamps `Community`; name-collision skip (A4); `update` is `reset --hard` (never merge).
- **Regression (must stay green):** the three existing `swap_dir_*` tests; embedded-source extraction byte-identical to pre-change.
- **Build:** `include_dir!` compiles against the submodule path.
- **Manual:** wipe `~/.aleph`, start offline (embedded) / online (latest clone); App version bump → re-extract embedded; explicit update → pull; install a third-party skill, then run official update → third-party untouched.

---

## 8. Risks & Tradeoffs
- Binary size **not** reduced (embedded fallback retained) — the "no bundling" goal is softened by the offline-fallback choice (D1).
- Submodule developer ergonomics (`git clone --recurse-submodules` / `submodule update --init`); contributors who skip it get an empty embed → `build.rs` should emit a clear warning.
- First-install online adds one bounded clone latency (timeout + fallback).
- Two more repos to maintain + a release-time submodule bump (workflow-automated; CI freshness check guards drift).
- App-upgrade-resets-to-`Z` edge (§3) accepted; eliminating it would need content-version-aware anti-downgrade logic (out of scope).

---

## 9. Open / Fast-follow (out of scope here)
- Upstream **removal** of an official skill (pruning a whole official leaf that vanished from the new checkout) — confirm/strengthen existing reconcile behavior; treat as fast-follow if not already covered.
- Pinned-tag runtime mode (A2 alternative) if reproducible per-release runtime content is later wanted.
- Catalog `content_hash`/ETag "unchanged → skip" optimization for the explicit-update path.
