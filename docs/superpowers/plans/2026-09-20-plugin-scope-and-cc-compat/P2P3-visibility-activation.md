# Plan — P2 spatial composability (visibility) + P3 terminal status & activation gate

Base: `3ddc1f2e7` (+ spec commit `35e5f8bca`). Worktree `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/plugin-scope-round`.
Every `file:line` below was read at that commit. `src/harness/` diff is 0 lines in every task.

**Orientation facts the tasks rely on (verified in code, not in the spec):**

- `src/extension/scope.rs` (135 lines) does **not** hold `project_scope_allows`. It holds `scope_install_dir` (`:7`) and `parse_scope` (`:29`), consumed by `src/extension/marketplace/mod.rs:352,388,445`, `src/bin/aleph-server/commands/plugins.rs:279,288,325,330,336`, `src/gateway/handlers/plugins/handlers/marketplace.rs:209,262`. `project_scope_allows` is a private method on `HookExecutor` at `src/extension/hooks/executor.rs:362-378`, called at `:918` (interceptors) and `:1113` (observers). Both live on after the rename; only the module path changes.
- The "current project" derivation hooks use today (`executor.rs:372-373`): `crate::projects::current_project_root().or_else(|| std::env::current_dir().ok())` — the task-local `CURRENT_PROJECT_ROOT` (`src/projects/run_context.rs:23-43`) published by `run_loop/mod.rs:734` from `request.workspace_override.clone()`, with the daemon CWD as fallback. Its producer is `RunRequest.workspace_override`, which `src/gateway/handlers/agent.rs:907-958` builds from `AgentRunParams.project_root` (Panel "Enter Project"), and which `src/gateway/inbound_router/executor.rs:425-435` builds from `ctx.workspace.or(channel_workspace)`. Path comparison is canonicalise-best-effort (`executor.rs:183-187 paths_equal`).
- `PluginStatus` at `src/extension/types/plugins.rs:174-208` is `{Loaded, Disabled, Overridden, Error(String), Blocked(String)}`. Per G-2 the names stay (`Loaded` / `Disabled` / `Blocked(String)` / `Error(String)`); P3 adds `Pending { waiting_on }` and removes `Overridden`; P1's mount failure writes `Error("<step>: <reason>")` at `lifecycle.rs::fail_mount` (R1.4 names it `write_failed_row`). The wire twin is `aleph_protocol::plugins::PluginRuntimeStatus` (`shared/protocol/src/plugins.rs:137-183`), mapped from `PluginStatus::label()` by `src/gateway/handlers/plugins/types.rs:61-69 parse_status`.
- MCP-plugin server ids are `format!("plugin:{plugin_id}/{server_name}")` (`src/extension/mcp_config.rs:181`); plugin ids are `[a-z0-9-]` (`manifest/mod.rs:108 validate_plugin_id`), so the id decodes unambiguously.
- `McpManagerHandle::add_transient_server` (`src/mcp/manager/handle.rs:106`) awaits `start_server_internal` (`actor.rs:603-618`, `:860-893`), which awaits the MCP handshake and `list_tools()` before answering. P1.4's `add_transient_server_detached` returns that answer as a `oneshot::Receiver` instead of awaiting it, and P1's `lifecycle.rs::watch_server_starts` (R1.1) awaits the receivers per plugin; `Ok(())` on a receiver means "initialised". So the only *pending* states are "no manager handle attached" (the `mcp_server` step was skipped) and "enqueued, receiver not yet answered" — P3.2's `ServerStart` is exactly that vocabulary.
- Process-global container statics are pinned by `src/capability/census.rs` (the "seventeen" count); adding a new `OnceLock<RwLock<…>>` global would move that number. P2 therefore puts the plugin-id→`ScopeKey` map on `ExtensionManager` (already a `CapabilitySlot`, `manager_global.rs:28`) as a sync `StdRwLock` snapshot next to `active_plugin_tools` (`mod.rs:176`), not in a new static.
- Phase order: P1 lands first. Anchors below that P1 moves are cited against plan-P1's code (`lifecycle.rs::{build_record, unparsed_record, write_failed_row, discover_and_mount, mount_inner, mount_parsed, watch_server_starts, activation_settled}`, `slash_effect.rs::plugin_command_skill_info`, the P1.8 boot order) with the 3ddc1f2e7 line in parentheses as history; `collect_plugin_dirs` (`mod.rs:1016-1073`), `sync_hooks_from_registry` (`:1128`), `refresh_active_plugin_tools` (`:1237`) and `active_plugin_tools_snapshot` (`:1261`) stay in `mod.rs` under P1 and are cited as-is.
- `PluginRegistry::unregister_plugin(id)` already exists (`registry/plugin_registry/mod.rs:432`); the contract lists it as "(new)". Noted for P1 in Contract deltas.

---

## P2 — spatial composability

### Task P2.1: `visibility.rs` — `ScopeKey`, `VisibilityCtx`, `visible_to` (module rename)

**Files:**
- Rename: `src/extension/scope.rs` → `src/extension/visibility.rs` (`git mv`; keep `scope_install_dir` / `parse_scope` and their 6 tests unchanged inside it)
- Modify: `src/extension/mod.rs:30` (`pub mod scope;` → `pub mod visibility;`)
- Modify: `src/extension/marketplace/mod.rs:352,388,445`; `src/bin/aleph-server/commands/plugins.rs:279,325`; `src/gateway/handlers/plugins/handlers/marketplace.rs:209,262` (path `extension::scope::` → `extension::visibility::`)
- Test: `src/extension/visibility.rs` (`#[cfg(test)] mod tests`, existing module)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  ```rust
  pub enum ScopeKey { Global, Project(PathBuf) }           // Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash
  impl ScopeKey { pub fn project(root: &Path) -> Self; }   // the ONLY constructor of Project(...): canonicalises
  pub struct VisibilityCtx { pub project_root: Option<PathBuf> }   // Clone, Debug, Default
  pub fn visible_to(key: &ScopeKey, ctx: &VisibilityCtx) -> bool;
  pub fn canonical_root(p: &Path) -> PathBuf;               // best-effort canonicalise (was executor.rs paths_equal's body)
  ```

- [ ] **Step 1: Write the failing test** (append to the existing `mod tests` in the renamed file)

```rust
    // ── visible_to truth table ────────────────────────────────────────────
    //
    // Global × {project, no project} and Project(p) × {same p, other p, none}.
    // "none" is the fail-closed row: a session bound to no project sees Global
    // only. Today every face shows everything; this is the behaviour change
    // the spec (§3.4) records.
    #[test]
    fn visible_to_truth_table() {
        let p = tempdir().unwrap();
        let q = tempdir().unwrap();
        let in_p = VisibilityCtx {
            project_root: Some(canonical_root(p.path())),
        };
        let in_q = VisibilityCtx {
            project_root: Some(canonical_root(q.path())),
        };
        let nowhere = VisibilityCtx { project_root: None };

        assert!(visible_to(&ScopeKey::Global, &in_p), "Global × project");
        assert!(visible_to(&ScopeKey::Global, &nowhere), "Global × no project");

        let key_p = ScopeKey::project(p.path());
        assert!(visible_to(&key_p, &in_p), "Project(p) × same p");
        assert!(!visible_to(&key_p, &in_q), "Project(p) × other project");
        assert!(!visible_to(&key_p, &nowhere), "Project(p) × no project (fail-closed)");
    }

    /// `ScopeKey::project` canonicalises, so a key built from `/var/…` and a
    /// ctx built from `/private/var/…` (macOS) still compare equal. Without
    /// this every macOS tempdir-based project would be invisible to itself.
    #[test]
    fn project_key_survives_symlinked_spellings() {
        let p = tempdir().unwrap();
        let canonical = p.path().canonicalize().unwrap();
        let key_raw = ScopeKey::project(p.path());
        let key_canon = ScopeKey::project(&canonical);
        assert_eq!(key_raw, key_canon);
        let ctx = VisibilityCtx {
            project_root: Some(canonical),
        };
        assert!(visible_to(&key_raw, &ctx));
    }

    /// A root that does not exist keeps its literal spelling (no panic, no
    /// silent `Global`): the comparison degrades to string equality, the same
    /// rule `paths_equal` in the hook executor applied.
    #[test]
    fn project_key_of_a_missing_dir_is_still_a_project_key() {
        let ghost = std::path::Path::new("/definitely/not/here/aleph-p2");
        match ScopeKey::project(ghost) {
            ScopeKey::Project(root) => assert_eq!(root, ghost),
            ScopeKey::Global => panic!("a missing dir must not decay to Global"),
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib extension::visibility -- --nocapture`
Expected: FAIL to compile — `error[E0433]: failed to resolve: could not find `visibility` in `extension`` (module not yet renamed) / `cannot find function `visible_to``.

- [ ] **Step 3: Write minimal implementation**

`git mv src/extension/scope.rs src/extension/visibility.rs`, then replace the file header (lines 1–4 today: `//! Plugin scope path resolution` + the two `use` lines) with:

```rust
//! Where a plugin lives, and who may see it.
//!
//! Two halves of one question, deliberately in one file:
//!
//! * **install side** — [`scope_install_dir`] / [`parse_scope`]: given an
//!   install scope, which directory receives the plugin;
//! * **visibility side** — [`ScopeKey`] / [`VisibilityCtx`] / [`visible_to`]:
//!   given where a plugin was *found*, which sessions may see it.
//!
//! `visible_to` is the single predicate every capability face uses (tool
//! index, skills index, sub-agents, slash list, MCP bridge, hooks). It has
//! exactly two shapes: `Global` is visible everywhere; `Project(root)` is
//! visible only to a session whose project root is that directory. A session
//! bound to no project sees `Global` only — fail-closed, and a behaviour
//! change from the union-of-all-projects discovery that preceded this round.

use crate::extension::types::PluginScope;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Best-effort canonicalisation for scope comparison. Symlinks (`/var` →
/// `/private/var`), `.`/`..` segments and trailing slashes must not make two
/// spellings of one directory look like two directories; a path that does not
/// resolve keeps its literal spelling so the comparison degrades to string
/// equality instead of panicking or decaying to `Global`.
#[must_use]
pub fn canonical_root(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Where a registry row was discovered, as the visibility predicate sees it.
///
/// `Project(root)` is produced only for `<project>/.claude/…` and
/// `<project>/.aleph/plugins{,.local}/…` (see `ScopeKey::from_discovery` in
/// Task P2.3); every other origin — `~/.aleph`, `~/.claude`, bundled,
/// marketplace cache, the future `ClaudeCache` — is `Global`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeKey {
    Global,
    /// Canonicalised project root. Construct through [`ScopeKey::project`] so
    /// the canonicalisation happens exactly once, on the way in.
    Project(PathBuf),
}

impl ScopeKey {
    /// The only way to build a `Project` key: canonicalises the root.
    #[must_use]
    pub fn project(root: &Path) -> Self {
        Self::Project(canonical_root(root))
    }
}

/// What the requesting session is bound to. `None` = no project (a Panel
/// session that never entered a project, or a run whose `workspace_override`
/// is unset AND whose daemon has no readable CWD).
#[derive(Clone, Debug, Default)]
pub struct VisibilityCtx {
    pub project_root: Option<PathBuf>,
}

/// `Global` → always visible. `Project(p)` → visible iff the session's project
/// root is `p`. A session with no project sees `Global` only.
#[must_use]
pub fn visible_to(key: &ScopeKey, ctx: &VisibilityCtx) -> bool {
    match key {
        ScopeKey::Global => true,
        ScopeKey::Project(root) => ctx.project_root.as_deref() == Some(root.as_path()),
    }
}
```

(`scope_install_dir`, `parse_scope` and their tests follow unchanged.)

Then the module declaration — `src/extension/mod.rs:30`:

```rust
// before
pub mod scope;
// after
pub mod visibility;
```

and the seven path references, each `crate::extension::scope::` → `crate::extension::visibility::` / `alephcore::extension::scope::` → `alephcore::extension::visibility::`:

- `src/extension/marketplace/mod.rs:352` (doc link), `:388`, `:445`
- `src/bin/aleph-server/commands/plugins.rs:279` (`use alephcore::extension::scope::parse_scope;`), `:325` (`use alephcore::extension::scope::{parse_scope, scope_install_dir};`)
- `src/gateway/handlers/plugins/handlers/marketplace.rs:209`, `:262`

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension::visibility -- --nocapture`
Expected: PASS (9 tests: the 6 pre-existing `test_scope_install_dir_*` / `test_parse_scope` + 3 new). Then `rg -n 'extension::scope::' src interfaces shared qa crates` → 0 hits, and `cargo test -p alephcore --bins --no-run` (the `aleph-server` binary imports the moved fns).

- [ ] **Step 5: Commit**

```bash
git add src/extension/visibility.rs src/extension/mod.rs src/extension/marketplace/mod.rs src/bin/aleph-server/commands/plugins.rs src/gateway/handlers/plugins/handlers/marketplace.rs
git commit -m "extension: rename scope.rs to visibility.rs and add ScopeKey/VisibilityCtx/visible_to

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.2: `VisibilityCtx::for_session` — extract the hooks' project-root derivation; hooks become the first face

**Files:**
- Modify: `src/extension/visibility.rs` (add `from_project_root` / `for_session`)
- Modify: `src/extension/types/hooks.rs:369-411` (`HookConfig` gains `scope_key: ScopeKey`)
- Modify: `src/extension/hooks/executor.rs:179-187` (delete `paths_equal`), `:346-378` (`project_scope_allows` → `visible_to`), `:1162-1175` + `:1335-1368` (tests)
- Modify: `src/extension/hooks/user_settings.rs:126,136,141,151-172,282-292` (stamp the key at the producer)
- Modify: `src/extension/mod.rs:1190-1206` (`sync_hooks_from_registry` stamps `ScopeKey::Global` — replaced by the record's key in P2.3)
- Modify (test-only struct literals, add one line `scope_key: ScopeKey::Global,` each): `src/extension/hooks/executor.rs:1163,1392`; `src/extension/hooks/mod.rs:710,741,769,812,944,973,1060,1112,1194`; `src/tools/scoped/tests.rs:519`; `src/verification/extension_stop_gate.rs:328`; `src/memory/session_compactor/prepare_history.rs:295,351`; `src/extension/hooks/user_settings.rs:390,415,444,483,514,533,543,558` (the 8 `load_into` test calls gain a `&ScopeKey` argument)
- Modify: `docs/reference/FEATURE_LOCATOR.md:3012` (§5.10 Hook 系统, the 代码锚点 line — one sentence naming the new hook-scope gate; R3.4 / plan-P5P8 doc-code 同笔 matrix)
- Test: `src/extension/visibility.rs`, `src/extension/hooks/executor.rs` (tests module)

**Interfaces:**
- Consumes: `ScopeKey`, `VisibilityCtx`, `visible_to`, `canonical_root` (P2.1); `crate::projects::current_project_root()` (`src/projects/mod.rs:32`).
- Produces:
  ```rust
  impl VisibilityCtx {
      /// THE derivation. `root` is `RunRequest.workspace_override` however it reached you
      /// (task-local, request field, RPC param). `None` → daemon CWD, the rule hooks always applied.
      pub fn from_project_root(root: Option<PathBuf>) -> Self;
      /// `from_project_root(current_project_root())` — for code running inside `run_loop`'s scope.
      pub fn for_session() -> Self;
  }
  pub struct HookConfig { /* … existing … */ pub scope_key: ScopeKey }
  ```

**Why one function with two entry points, not two derivations:** the fact is `RunRequest.workspace_override`. Inside a run it is readable only through the task-local; before the run is constructed (slash resolution, `commands.list`) it is readable only from the request/params. Both entry points hand the same value to `from_project_root`, which is the only place that knows about the CWD fallback and canonicalisation. `AgentRegistry::resolve(id, project_root)` (P2.7) feeds its existing `project_root` parameter through the same function rather than re-reading the task-local, so a caller that passed an explicit root and the ctx can never disagree.

- [ ] **Step 1: Write the failing tests**

In `src/extension/visibility.rs` `mod tests`:

```rust
    /// The derivation hooks have always used: task-local first, daemon CWD
    /// second. Inside a `with_project_root(Some(p))` scope the ctx is `p`.
    #[tokio::test]
    async fn for_session_reads_the_run_task_local() {
        let p = tempdir().unwrap();
        let want = canonical_root(p.path());
        let got = crate::projects::with_project_root(Some(p.path().to_path_buf()), async {
            VisibilityCtx::for_session()
        })
        .await;
        assert_eq!(got.project_root, Some(want));
    }

    /// Outside any scope (or with an explicit `None` override) the daemon CWD
    /// stands in — plain-server mode, where "the project" is the directory the
    /// operator launched from. This is NOT a new rule: `project_scope_allows`
    /// applied it to project hooks before this round.
    #[tokio::test]
    async fn for_session_falls_back_to_the_daemon_cwd() {
        let got = crate::projects::with_project_root(None, async { VisibilityCtx::for_session() })
            .await;
        let cwd = std::env::current_dir().ok().map(|c| canonical_root(&c));
        assert_eq!(got.project_root, cwd);
        assert!(got.project_root.is_some(), "the test process has a cwd");
    }

    /// The request-side entry point is the same function fed from the field
    /// instead of the task-local: same canonicalisation, same fallback.
    #[test]
    fn from_project_root_canonicalises_and_matches_the_key() {
        let p = tempdir().unwrap();
        let ctx = VisibilityCtx::from_project_root(Some(p.path().to_path_buf()));
        assert!(visible_to(&ScopeKey::project(p.path()), &ctx));
    }
```

In `src/extension/hooks/executor.rs` `mod tests`, replace lines `1335-1368` (`project_hook`, `global_and_plugin_hooks_are_never_project_gated`, `project_hook_fires_only_in_its_own_workspace`) with:

```rust
    /// A project hook carries its project's key; the hook loader stamps it
    /// from the file's directory (`<root>/.aleph/hooks*.json` → `Project(root)`).
    fn project_hook(plugin_name: &str, project_root: &Path) -> HookConfig {
        let mut h = dummy_hook(plugin_name);
        h.plugin_root = project_root.join(".aleph");
        h.scope_key = ScopeKey::project(project_root);
        h
    }

    #[test]
    fn global_hooks_are_never_project_gated() {
        let exec = HookExecutor::empty();
        // No `with_project_root` scope active here, yet these must still fire.
        assert!(exec.project_scope_allows(&dummy_hook("user:global"), &VisibilityCtx::for_session()));
        assert!(exec.project_scope_allows(&dummy_hook("plugin:foo"), &VisibilityCtx::for_session()));
    }

    /// The gate no longer sniffs the `user:project` label: a plugin-shipped
    /// hook whose plugin was found under a project is gated exactly like a
    /// project hook file. Same predicate, sixth face.
    #[tokio::test]
    async fn a_plugin_hook_with_a_project_key_is_gated_like_a_project_hook() {
        let proj = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let exec = HookExecutor::empty();
        let mut hook = dummy_hook("some-plugin");
        hook.scope_key = ScopeKey::project(proj.path());

        let inside = crate::projects::with_project_root(Some(proj.path().to_path_buf()), async {
            exec.project_scope_allows(&hook, &VisibilityCtx::for_session())
        })
        .await;
        let elsewhere = crate::projects::with_project_root(Some(other.path().to_path_buf()), async {
            exec.project_scope_allows(&hook, &VisibilityCtx::for_session())
        })
        .await;
        assert!(inside, "plugin hook must fire inside its own project");
        assert!(!elsewhere, "plugin hook must NOT fire inside another project");
    }

    #[tokio::test]
    async fn project_hook_fires_only_in_its_own_workspace() {
        let proj_a = tempfile::tempdir().unwrap();
        let proj_b = tempfile::tempdir().unwrap();
        let exec = HookExecutor::empty();
        let hook_a = project_hook("user:project", proj_a.path());

        // Active project == the hook's project → fires.
        let in_a = crate::projects::with_project_root(Some(proj_a.path().to_path_buf()), async {
            exec.project_scope_allows(&hook_a, &VisibilityCtx::for_session())
        })
        .await;
        assert!(in_a, "project hook must fire inside its own project");

        // A different project is active → suppressed (no cross-project leak).
        let in_b = crate::projects::with_project_root(Some(proj_b.path().to_path_buf()), async {
            exec.project_scope_allows(&hook_a, &VisibilityCtx::for_session())
        })
        .await;
        assert!(!in_b, "project hook must NOT fire inside another project");
    }

    /// Behaviour change recorded in the spec: with NO resolvable project
    /// (task-local `None` and CWD unreadable is not reproducible here, so the
    /// ctx is built directly) a project hook is suppressed, where it used to
    /// fail open.
    #[test]
    fn a_project_hook_with_no_project_context_is_suppressed_not_fired() {
        let proj = tempfile::tempdir().unwrap();
        let exec = HookExecutor::empty();
        let hook = project_hook("user:project", proj.path());
        let nowhere = VisibilityCtx { project_root: None };
        assert!(!exec.project_scope_allows(&hook, &nowhere));
    }
```

In `src/extension/hooks/user_settings.rs` `mod tests`, add:

```rust
    /// The producer stamps the key: a project layer file yields `Project(root)`
    /// rows, the global file yields `Global` rows. Without the stamp the
    /// executor's gate has nothing to compare and every project hook would
    /// silently become global (fail-open).
    #[test]
    fn project_layer_rows_carry_their_project_key_and_global_rows_carry_global() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".aleph")).unwrap();
        std::fs::write(
            root.path().join(".aleph/hooks.json"),
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        load_project_layer(root.path(), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].scope_key, ScopeKey::project(root.path()));

        let global = root.path().join("hooks.json");
        std::fs::write(
            &global,
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        load_into(&global, "user:global", &ScopeKey::Global, &mut out);
        assert_eq!(out[0].scope_key, ScopeKey::Global);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::visibility::tests::for_session -- --nocapture`
Expected: FAIL to compile — `no function or associated item named `for_session` found for struct `VisibilityCtx``.
Run: `cargo test -p alephcore --lib extension::hooks::executor::tests::a_plugin_hook_with_a_project_key -- --nocapture`
Expected: FAIL to compile — `struct `HookConfig` has no field named `scope_key``.

- [ ] **Step 3: Write minimal implementation**

`src/extension/visibility.rs`, after `impl ScopeKey`:

```rust
impl VisibilityCtx {
    /// The one derivation of "which project is this session in".
    ///
    /// `root` is `RunRequest.workspace_override`, however it reached the
    /// caller: the run-loop task-local ([`Self::for_session`]), the request
    /// field before the run exists (slash resolution), or an RPC parameter
    /// (`commands.list`). `None` falls back to the daemon CWD — plain-server
    /// mode, where the operator launched Aleph *inside* the project. That is
    /// the rule the hook executor's `project_scope_allows` has applied since
    /// project mode shipped; it is not widened here. In App mode the CWD is
    /// meaningless and holds no `.aleph/plugins`, so the fallback resolves to
    /// "Global only" there, which is the fail-closed answer.
    #[must_use]
    pub fn from_project_root(root: Option<PathBuf>) -> Self {
        let effective = root.or_else(|| std::env::current_dir().ok());
        Self {
            project_root: effective.map(|p| canonical_root(&p)),
        }
    }

    /// [`Self::from_project_root`] fed from the run-loop task-local
    /// (`crate::projects::current_project_root`, published by
    /// `run_loop/mod.rs` from the request's `workspace_override`). Only
    /// meaningful inside a run; callers outside one must use
    /// `from_project_root` with the request's own field.
    #[must_use]
    pub fn for_session() -> Self {
        Self::from_project_root(crate::projects::current_project_root())
    }
}
```

`src/extension/types/hooks.rs` — inside `pub struct HookConfig` (after `timeout_secs`, line 410):

```rust
    /// Who may see this hook fire: stamped by the producer that knows where
    /// the hook came from (`hooks::load_user_hooks` for `~/.aleph/hooks.json`
    /// and project files, `ExtensionManager::sync_hooks_from_registry` for
    /// plugin-shipped hooks, from the owning plugin's registry row). The
    /// executor compares it with `extension::visibility::visible_to`. There is
    /// deliberately no `Default`: a hook constructed without saying where it
    /// belongs would fire everywhere.
    pub scope_key: crate::extension::visibility::ScopeKey,
```

`src/extension/hooks/executor.rs` — delete lines 179-187 (`paths_equal` and its doc comment; quote):

```rust
/// Compare two directory paths for identity, canonicalising best-effort so
/// symlinks (`/var` → `/private/var`), `.`/`..` segments and trailing slashes
/// don't cause a spurious mismatch. Falls back to the raw path when a side
/// doesn't resolve so the comparison degrades gracefully rather than failing.
fn paths_equal(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let cb = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}
```

and replace lines 346-378 (the doc comment + `project_scope_allows`; today's body is quoted in the orientation section: `if !hook.plugin_name.starts_with("user:project") { return true; } … paths_equal(&root, hook_project) … None => true`) with:

```rust
    /// Gate a hook to the sessions that may see it.
    ///
    /// Every hook carries a [`ScopeKey`](crate::extension::visibility::ScopeKey)
    /// stamped by its producer; this is the hook face of the one visibility
    /// predicate ([`visible_to`](crate::extension::visibility::visible_to)) the
    /// tool index, skills, sub-agents, slash list and MCP bridge share. The
    /// daemon serves every registered project from one process, so all
    /// project hooks live in one executor — without this gate a hook checked
    /// into project A would fire while the agent works inside project B (an
    /// isolation / arbitrary-command-execution leak).
    ///
    /// `ctx` is computed once per fire-site call, not per hook, so a batch of
    /// interceptors is judged against one answer.
    fn project_scope_allows(
        &self,
        hook: &HookConfig,
        ctx: &crate::extension::visibility::VisibilityCtx,
    ) -> bool {
        crate::extension::visibility::visible_to(&hook.scope_key, ctx)
    }
```

Call sites: `executor.rs:906-920` (`execute_interceptors`) — before the `for hook in interceptors` loop add `let visibility = crate::extension::visibility::VisibilityCtx::for_session();` and change `:918` to `if !self.project_scope_allows(hook, &visibility) {`. `executor.rs:1105-1114` (`execute_observers`) — add the same `let visibility = …;` before the `let observers: Vec<_> = …` and change `:1113` to `.filter(|h| self.project_scope_allows(h, &visibility))`. Remove the now-unused `use std::path::Path;` at `:12` only if the compiler reports it unused (other code in the file may still use `Path`).

In the tests module `:1162-1175` `dummy_hook` gains `scope_key: ScopeKey::Global,` and the module imports `use crate::extension::visibility::{ScopeKey, VisibilityCtx};`. `interceptor_command_hook` at `:1392` gains the same line.

`src/extension/hooks/user_settings.rs`:

```rust
// :50 — add the import
use crate::extension::visibility::{canonical_root, ScopeKey};

// :126
        load_into(&p, "user:global", &ScopeKey::Global, &mut out);

// :151-153 — `canonical` becomes a thin alias of the shared derivation so the
// dedup bookkeeping and the visibility key cannot canonicalise differently:
fn canonical(p: &Path) -> PathBuf {
    canonical_root(p)
}

// :158-165
fn load_project_layer(root: &Path, out: &mut Vec<HookConfig>) {
    let key = ScopeKey::project(root);
    load_into(&root.join(".aleph/hooks.json"), "user:project", &key, out);
    load_into(
        &root.join(".aleph/hooks.local.json"),
        "user:project-local",
        &key,
        out,
    );
}

// :167
fn load_into(path: &Path, source_label: &str, scope: &ScopeKey, out: &mut Vec<HookConfig>) {

// :282-292 — the struct literal gains
                    scope_key: scope.clone(),
```

The 8 test calls `load_into(&cfg, "user:project", &mut out)` at `:390,415,444,483,514,533,543,558` become `load_into(&cfg, "user:project", &ScopeKey::Global, &mut out)` (those tests assert parsing, not scoping).

`src/extension/mod.rs:1190-1206` (`sync_hooks_from_registry`'s `HookConfig { … }` literal) gains `scope_key: crate::extension::visibility::ScopeKey::Global,` **with the comment** `// P2.3 replaces this with the owning record's key.` — the record has no key until P2.3.

The remaining test-only literals each gain `scope_key: crate::extension::visibility::ScopeKey::Global,` (or `ScopeKey::Global` with a local `use`): `hooks/mod.rs:710,741,769,812,944,973,1060,1112,1194`; `tools/scoped/tests.rs:519`; `verification/extension_stop_gate.rs:328`; `memory/session_compactor/prepare_history.rs:295,351`.

`docs/reference/FEATURE_LOCATOR.md:3012` (§5.10 的 **代码锚点** 行, which today ends with the `hooks/` module list and never names the project gate) gains, appended to that line: `；**项目 hook 的可见性闸** = `src/extension/visibility.rs::visible_to`（`HookConfig.scope_key` 由两个生产者盖章：`hooks/user_settings.rs::load_into` 与 `mod.rs::sync_hooks_from_registry`；`executor.rs::project_scope_allows` 只比较，不再嗅 `user:project` 前缀——与工具索引 / 技能 / 子代理 / slash / MCP 五面共用同一个谓词，见 §3.10 本轮条目）`. Same commit as the code (判据 §1).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::visibility -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib extension::hooks -- --nocapture` → PASS (all pre-existing hook tests plus the four new executor tests and the user_settings test).
Run: `cargo test -p alephcore --lib --no-run && cargo test -p alephcore --features test-helpers --test '*' --no-run` → compiles (catches any `HookConfig {` literal the list above missed; the compiler is the census here).
Run: `rg -n 'paths_equal' src` → 0 hits.

- [ ] **Step 5: Commit**

```bash
git add src/extension/visibility.rs src/extension/types/hooks.rs src/extension/hooks/executor.rs src/extension/hooks/user_settings.rs src/extension/hooks/mod.rs src/extension/mod.rs src/tools/scoped/tests.rs src/verification/extension_stop_gate.rs src/memory/session_compactor/prepare_history.rs docs/reference/FEATURE_LOCATOR.md
git commit -m "extension/hooks: derive VisibilityCtx once; hooks carry a ScopeKey and gate through visible_to

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.3: Every registry row carries a `scope_key`, derived at discovery

> Anchors are against the tree P1 leaves behind: `load_all` / `mount` live in `src/extension/lifecycle.rs` (plan-P1 P1.9), the narrow `reload_plugin` is gone (P1.11), and `collect_plugin_dirs` / `sync_hooks_from_registry` stay in `src/extension/mod.rs` (P1.9 consumes them from `mod.rs:1016-1073` / `:1128`). 3ddc1f2e7 cites are kept in parentheses as history.

**Files:**
- Modify: `src/discovery/types.rs:8-58` (`DiscoveryScope` replaces the bare `source` on `ScanDirectory` / `DiscoveredPath`; new `ProjectPluginParent`)
- Modify: `src/discovery/scanner.rs:98,107` (global scan dirs), `:139,161` (project scan dirs), `:207-231` (`discover_plugins_with_extra` takes `&[ProjectPluginParent]`), `:236-243` + `:289,304` (`scan_plugin_parent` threads the scope), `:384-388` (`classify_entry` copies the scope), tests `:541`, `:792`, `:804-807`
- Modify: `src/discovery/mod.rs:100-108` (wrapper signature)
- Modify: `src/extension/types/plugins.rs:308-343,347-368,379-435` (`PluginRecord.scope_key`)
- Modify: `src/extension/mod.rs:125,216,356` (test-only extras become `ProjectPluginParent`s), `:140-144` (`DiscoveredExtensionDir.scope_key`), `:1030-1044` (project parents carry their root), `:1052-1077` (derive the key), `:1128-1206` (`sync_hooks_from_registry` stamps the record's key), test helper (3ddc1f2e7 `:1531`)
- Modify: `src/extension/lifecycle.rs` (plan-P1 P1.9, final names): `build_record` (the ONE `PluginRecord` construction for parsed plugins), `unparsed_record` (the fallback `Error` row) and its single caller in `discover_and_mount`, `mount_inner` (re-reads the key from the row); `write_failed_row` needs no edit (see Step 3)
- Modify: `src/extension/visibility.rs` (`ScopeKey::from_discovery`)
- Test: `src/extension/visibility.rs`, `src/extension/lifecycle.rs` (tests module, next to P1.9's), `src/discovery/scanner.rs` (tests module)

**Interfaces:**
- Consumes: `ScopeKey::project` (P2.1); P1.9's `build_record(output, root_dir, origin)`, `unparsed_record(dir_path, error)`, `discover_and_mount`, `mount_inner`, `write_failed_row`.
- Produces:
  ```rust
  // src/discovery/types.rs
  pub enum GlobalRoot { Aleph, Claude }                                   // Copy; P4.10 adds `ClaudeCache`
  pub enum DiscoveryScope { Global(GlobalRoot), Project { root: PathBuf } } // `(Project, no root)` cannot be written
  impl DiscoveryScope { pub const fn source(&self) -> DiscoverySource; }   // the old `source` field, derived
  pub struct ProjectPluginParent { pub project_root: PathBuf, pub dir: PathBuf }
  pub struct DiscoveredPath { pub path: PathBuf, pub priority: u32, pub scope: DiscoveryScope }
  impl DiscoveredPath { pub fn global(path, root: GlobalRoot, priority) -> Self;
                        pub fn in_project(path, project_root: PathBuf, priority) -> Self;
                        pub const fn source(&self) -> DiscoverySource }
  // src/extension/visibility.rs
  impl ScopeKey { pub fn from_discovery(d: &DiscoveredPath) -> Self }   // wildcard-free over GlobalRoot
  // src/extension/types/plugins.rs
  pub struct PluginRecord { /* … */ pub scope_key: ScopeKey }
  // src/extension/lifecycle.rs (P1.9's, each one parameter wider)
  fn build_record(output: &AdapterOutput, root_dir: PathBuf, origin: PluginOrigin, scope_key: ScopeKey) -> PluginRecord;
  fn unparsed_record(dir_path: &Path, error: &str, scope_key: ScopeKey) -> PluginRecord;
  ```

**The derivation is one function, `ScopeKey::from_discovery`, fed by the scanner** — the only code that knows which directory a scan started from. It never parses a path string to guess the root (判据 §12): `<root>/.claude` and `<root>/.aleph` scan dirs record `root = dir.parent()` at the moment the upward walk produced them (`scanner.rs:139,161`), and `<root>/.aleph/plugins{,.local}` parents are handed to the scanner *as* `(root, dir)` pairs by `collect_plugin_dirs`, which already has the root (`mod.rs:1035-1043`). **A `Project` discovery without a root is unrepresentable** (R2.4): `DiscoveryScope::Project { root }` carries the root by value and there is no constructor that yields `source() == Project` without one. `DiscoverySource` (the `Copy` enum `SkillRegistration.source` / `PluginOrigin::classify` consume) stays; it is now *derived* from the scope, never stored beside it.

**Wildcard-free on purpose (R2.5):** `from_discovery` matches every `GlobalRoot` variant by name and `PluginOrigin::classify` (`types/plugins.rs:108-118`) already matches every `DiscoverySource`. P4.10 adds `GlobalRoot::ClaudeCache` (and the `DiscoverySource` / `PluginOrigin` twins it needs); the compiler then forces the `=> ScopeKey::Global` arm here — P4.10 owns adding it, this task owns leaving no wildcard for it to hide behind.

- [ ] **Step 1: Write the failing tests**

`src/extension/visibility.rs` `mod tests`:

```rust
    #[test]
    fn from_discovery_yields_project_only_for_project_scopes() {
        use crate::discovery::{DiscoveredPath, DiscoverySource, GlobalRoot};
        let root = tempdir().unwrap();
        let plugin_dir = root.path().join(".aleph/plugins/x");

        let in_project = DiscoveredPath::in_project(plugin_dir.clone(), root.path().to_path_buf(), 20);
        assert_eq!(in_project.source(), DiscoverySource::Project);
        assert_eq!(ScopeKey::from_discovery(&in_project), ScopeKey::project(root.path()));

        for global in [GlobalRoot::Aleph, GlobalRoot::Claude] {
            let d = DiscoveredPath::global(plugin_dir.clone(), global, 10);
            assert_ne!(d.source(), DiscoverySource::Project);
            assert_eq!(ScopeKey::from_discovery(&d), ScopeKey::Global, "{global:?}");
        }
    }
```

`src/discovery/scanner.rs` `mod tests` (the module exists at `:454`; add):

```rust
    /// Project plugin parents are scanned with the root they were handed, and
    /// the root survives onto every discovered plugin — a plugin found under
    /// `<root>/.aleph/plugins.local/` reports `<root>`, not its own directory
    /// and not the parent's.
    #[test]
    fn project_plugin_parents_carry_their_root_onto_discovered_plugins() {
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let local = root.path().join(".aleph/plugins.local/proj-local");
        std::fs::create_dir_all(local.join(".claude-plugin")).unwrap();
        std::fs::write(
            local.join(".claude-plugin/plugin.toml"),
            "name = \"proj-local\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        let scanner = DirectoryScanner::new(&DiscoveryConfig {
            working_dir: home.path().to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
        })
        .unwrap();
        let found = scanner
            .discover_plugins_with_extra(&[ProjectPluginParent {
                project_root: root.path().to_path_buf(),
                dir: root.path().join(".aleph/plugins.local"),
            }])
            .unwrap();
        let hit = found
            .iter()
            .find(|d| d.path.ends_with("proj-local"))
            .expect("plugin discovered");
        assert_eq!(
            hit.scope,
            DiscoveryScope::Project { root: root.path().to_path_buf() }
        );
        assert_eq!(hit.source(), DiscoverySource::Project);
    }
```

(`AlephHomeEnvGuard::acquire_and_set` is `pub(crate)` in `src/utils/paths.rs:96-103`; if the scanner tests module already holds a home guard convention, reuse it.)

`src/extension/lifecycle.rs` `mod tests` (P1.9's module; it has `isolated_manager` / `write_project_plugin`-style helpers — reuse them by their P1 names):

```rust
    /// Discovery stamps the key on the row: a plugin found under a project's
    /// plugin parent is `Project(root)`, a plugin found under `~/.aleph` is
    /// `Global`. Everything downstream (tool index, skills, agents, slash,
    /// MCP, hooks) reads this field; a row without it is a row every session
    /// can see.
    #[tokio::test]
    async fn load_all_stamps_project_scope_key_from_the_discovery_root() {
        let dir = tempfile::tempdir().unwrap();
        write_project_plugin(dir.path(), "p2-scoped");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let registry = manager.get_plugin_registry().await;
        let record = registry.get_plugin("p2-scoped").expect("registered");
        assert_eq!(
            record.scope_key,
            crate::extension::visibility::ScopeKey::project(dir.path()),
            "test extras are handed to discovery as (root, parent) pairs, so the row must carry the root"
        );
    }

    /// The parse-error row (a directory whose manifest will not parse) also
    /// carries the key of where it was found, so `plugins.list` can say which
    /// project owns the broken plugin.
    #[tokio::test]
    async fn load_all_stamps_the_key_on_parse_error_rows_too() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("plugins/p2-broken");
        std::fs::create_dir_all(broken.join(".claude-plugin")).unwrap();
        std::fs::write(broken.join(".claude-plugin/plugin.toml"), "name = [not toml").unwrap();
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let registry = manager.get_plugin_registry().await;
        let record = registry.get_plugin("p2-broken").expect("error row registered");
        assert!(matches!(record.status, PluginStatus::Error(_)));
        assert_eq!(
            record.scope_key,
            crate::extension::visibility::ScopeKey::project(dir.path())
        );
    }

    /// `unmount` keeps the row (Disabled) and `mount` re-reads it: the key
    /// survives a disable/enable cycle without discovery re-running. (The
    /// old narrow `reload_plugin` used to rebuild the record from the adapter
    /// alone; P1 deleted it, and the new one is unmount+mount.)
    #[tokio::test]
    async fn mount_after_unmount_keeps_the_rows_scope_key() {
        let dir = tempfile::tempdir().unwrap();
        write_project_plugin(dir.path(), "p2-cycle");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let want = crate::extension::visibility::ScopeKey::project(dir.path());
        manager.unmount("p2-cycle").await.unwrap();
        assert_eq!(manager.get_plugin_record("p2-cycle").await.unwrap().scope_key, want);
        manager.mount("p2-cycle").await.unwrap();
        assert_eq!(manager.get_plugin_record("p2-cycle").await.unwrap().scope_key, want);
    }

    /// `sync_hooks_from_registry` stamps plugin hooks with the OWNING ROW's key
    /// (P2.2 wrote a placeholder `Global` there).
    #[tokio::test]
    async fn plugin_hooks_inherit_the_owning_rows_scope_key() {
        let dir = tempfile::tempdir().unwrap();
        write_project_plugin(dir.path(), "p2-hooky");
        // `hooks/hooks.json` is what the TOML adapter reads when the manifest
        // names no hooks field (`manifest/component_source.rs:114`).
        let hooks_dir = dir.path().join("plugins/p2-hooky/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        std::fs::write(
            hooks_dir.join("hooks.json"),
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
        )
        .unwrap();
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let executor = manager.hook_executor_snapshot().await;
        let mine: Vec<_> = executor
            .hooks_for_plugin("p2-hooky")
            .into_iter()
            .map(|h| h.scope_key.clone())
            .collect();
        assert!(!mine.is_empty(), "the plugin's hooks.json must produce at least one hook");
        assert!(
            mine.iter().all(|k| *k == crate::extension::visibility::ScopeKey::project(dir.path())),
            "got {mine:?}"
        );
    }
```

(`hook_executor_snapshot()` is `skill_ops.rs:41`. `hooks_for_plugin(&str) -> Vec<&HookConfig>` is a 3-line `pub(crate)` accessor to add to `HookExecutor`: `self.hooks.iter().filter(|h| h.plugin_name == plugin_id).collect()`.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib from_discovery_yields_project -- --nocapture`
Expected: FAIL to compile — `cannot find type DiscoveryScope`, `no function in_project`.

- [ ] **Step 3: Write minimal implementation**

`src/discovery/types.rs` — replace `ScanDirectory` (`:18-34`) and `DiscoveredPath` (`:36-58`) with:

```rust
/// Which global root a global discovery came from. Every variant maps to
/// `ScopeKey::Global`; the enum exists so that mapping is written per name
/// (no wildcard) — a new root (P4.10's `ClaudeCache`) must be placed by a
/// human, and the compiler refuses to build until it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlobalRoot {
    /// `~/.aleph` (`DiscoverySource::AlephGlobal`).
    Aleph,
    /// `~/.claude` (`DiscoverySource::ClaudeGlobal`).
    Claude,
}

/// Where a scan started, as the visibility key needs it. A project scope
/// carries its root by value: there is no way to say "project, root unknown".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryScope {
    Global(GlobalRoot),
    Project { root: PathBuf },
}

impl DiscoveryScope {
    /// The legacy `DiscoverySource` label, derived. Every consumer that used
    /// to read a stored `source` field reads this instead.
    #[must_use]
    pub const fn source(&self) -> DiscoverySource {
        match self {
            Self::Global(GlobalRoot::Aleph) => DiscoverySource::AlephGlobal,
            Self::Global(GlobalRoot::Claude) => DiscoverySource::ClaudeGlobal,
            Self::Project { .. } => DiscoverySource::Project,
        }
    }
}

/// A directory to scan for components (scanner-internal).
#[derive(Debug, Clone)]
pub(crate) struct ScanDirectory {
    pub path: PathBuf,
    pub scope: DiscoveryScope,
    pub priority: u32,
}

impl ScanDirectory {
    #[must_use]
    pub(crate) const fn new(path: PathBuf, scope: DiscoveryScope, priority: u32) -> Self {
        Self {
            path,
            scope,
            priority,
        }
    }

    /// A project-level `.claude` / `.aleph` dir found by the upward walk. The
    /// root is the dir's parent — recorded here, at the one place that knows
    /// which walk produced it, never re-derived downstream. `None` only for
    /// a filesystem root, which the upward walk cannot produce; the caller
    /// skips such a dir rather than scanning it as global.
    #[must_use]
    pub(crate) fn project(dir: PathBuf, priority: u32) -> Option<Self> {
        let root = dir.parent()?.to_path_buf();
        Some(Self {
            path: dir,
            scope: DiscoveryScope::Project { root },
            priority,
        })
    }
}

/// A registered project's plugin parent directory, with the project it belongs to.
#[derive(Debug, Clone)]
pub struct ProjectPluginParent {
    pub project_root: PathBuf,
    /// `<project_root>/.aleph/plugins` or `<project_root>/.aleph/plugins.local`.
    pub dir: PathBuf,
}

/// A discovered path with metadata
#[derive(Debug, Clone)]
pub struct DiscoveredPath {
    /// Full path to the discovered item
    pub path: PathBuf,
    /// Priority for conflict resolution
    pub priority: u32,
    /// Where discovery found it. The visibility key
    /// (`extension::visibility::ScopeKey::from_discovery`) is derived from
    /// this and nothing else.
    pub scope: DiscoveryScope,
}

impl DiscoveredPath {
    /// A global discovery (`~/.aleph` or `~/.claude`).
    #[must_use]
    pub fn global(path: PathBuf, root: GlobalRoot, priority: u32) -> Self {
        Self {
            path,
            priority,
            scope: DiscoveryScope::Global(root),
        }
    }

    /// A project discovery: the root is required.
    #[must_use]
    pub fn in_project(path: PathBuf, project_root: PathBuf, priority: u32) -> Self {
        Self {
            path,
            priority,
            scope: DiscoveryScope::Project { root: project_root },
        }
    }

    /// The legacy label; see [`DiscoveryScope::source`].
    #[must_use]
    pub const fn source(&self) -> DiscoverySource {
        self.scope.source()
    }
}
```

`src/discovery/scanner.rs`:

```rust
// :10 — import the new names
use super::types::{DiscoveredPath, DiscoveryScope, DiscoverySource, GlobalRoot, ProjectPluginParent, ScanDirectory};

// :98 (was `ScanDirectory::new(claude_home.clone(), DiscoverySource::ClaudeGlobal, 0)`)
            dirs.push(ScanDirectory::new(claude_home.clone(), DiscoveryScope::Global(GlobalRoot::Claude), 0));
// :107 (was `… DiscoverySource::AlephGlobal, 10`)
            dirs.push(ScanDirectory::new(self.aleph_home.clone(), DiscoveryScope::Global(GlobalRoot::Aleph), 10));
// :139 (was `dirs.push(ScanDirectory::new(dir, DiscoverySource::Project, priority));`)
                match ScanDirectory::project(dir, priority) {
                    Some(sd) => dirs.push(sd),
                    None => debug!("project dir with no parent skipped"),
                }
// :161 — same replacement.

// :207-231 — signature + loop
    pub fn discover_plugins_with_extra(
        &self,
        extra_parents: &[ProjectPluginParent],
    ) -> DiscoveryResult<Vec<DiscoveredPath>> {
        let mut discovered = Vec::new();
        self.scan_plugin_parent(
            &self.aleph_home.join(PLUGINS_DIR),
            &mut discovered,
            &DiscoveryScope::Global(GlobalRoot::Aleph),
            10,
        );
        for parent in extra_parents {
            self.scan_plugin_parent(
                &parent.dir,
                &mut discovered,
                &DiscoveryScope::Project { root: parent.project_root.clone() },
                20,
            );
        }
        // (sort + trace unchanged)

// :236-243 — signature: `source: DiscoverySource` → `scope: &DiscoveryScope`
    fn scan_plugin_parent(
        &self,
        plugins_dir: &Path,
        discovered: &mut Vec<DiscoveredPath>,
        scope: &DiscoveryScope,
        priority: u32,
    ) {
        // …
        // :289 (was `discovered.push(DiscoveredPath::new(path, source, priority));`)
                discovered.push(DiscoveredPath { path, priority, scope: scope.clone() });
        // :304 — same with `sub_path`.
```

`scanner.rs:384-388` (`classify_entry`'s push) becomes:

```rust
    if is_component {
        out.push(DiscoveredPath {
            path: path.to_path_buf(),
            priority: scan_dir.priority,
            scope: scan_dir.scope.clone(),
        });
    }
```

Existing scanner tests: `:541` `d.source == DiscoverySource::AlephGlobal` → `d.source() == …`; `:792` `&[project.join(".aleph/plugins")]` → `&[ProjectPluginParent { project_root: project.clone(), dir: project.join(".aleph/plugins") }]`; `:804-807` the two-element slice likewise (`root.join("workspace/proj-missing")` as the second root). `src/discovery/mod.rs:103-108` changes `extra_parents: &[PathBuf]` → `&[ProjectPluginParent]` (`pub use types::*;` at `:11` exports it).

`src/extension/visibility.rs`, in `impl ScopeKey`:

```rust
    /// Derive the key from where discovery found the item. `Project(root)`
    /// for a project scope (the scanner recorded the root when it produced
    /// the scan dir — `<root>/.claude`, `<root>/.aleph`, or a registered
    /// project's `.aleph/plugins{,.local}`); `Global` for every global root,
    /// each named — no wildcard, so a new root must be placed here by hand.
    #[must_use]
    pub fn from_discovery(d: &crate::discovery::DiscoveredPath) -> Self {
        use crate::discovery::{DiscoveryScope, GlobalRoot};
        match &d.scope {
            DiscoveryScope::Project { root } => Self::project(root),
            DiscoveryScope::Global(GlobalRoot::Aleph | GlobalRoot::Claude) => Self::Global,
        }
    }
```

`src/extension/types/plugins.rs`:

```rust
// :330 — after `pub root_dir: PathBuf,`
    /// Who may see this plugin (`extension::visibility`). Stamped by
    /// `lifecycle.rs::build_record` from the discovery that found the
    /// directory — the adapters cannot know it, exactly as they cannot know
    /// `origin` (the `record.origin = origin` line beside it). `PluginRecord::new`
    /// / `from_adapter_output` start it at `Global` for the same reason they
    /// start `origin` at the adapter's placeholder; `load_all` overwrites it
    /// on every row it registers, including error rows.
    pub scope_key: crate::extension::visibility::ScopeKey,

// :365 (`new`) — after `root_dir: PathBuf::new(),`
            scope_key: crate::extension::visibility::ScopeKey::Global,
// :426 (`from_adapter_output`) — after `root_dir,`
            scope_key: crate::extension::visibility::ScopeKey::Global,
```

`src/extension/mod.rs`:

```rust
// :125 (ExtensionConfig, cfg(test))
    pub extra_plugin_parents: Vec<crate::discovery::ProjectPluginParent>,
// :216 (manager field, cfg(test))
    extra_plugin_parents: Vec<crate::discovery::ProjectPluginParent>,
// :356 unchanged (`.clone()` still works)

// :140-144
struct DiscoveredExtensionDir {
    path: PathBuf,
    /// Where it came from, per [`PluginOrigin::classify`].
    origin: PluginOrigin,
    /// Who may see it, per [`visibility::ScopeKey::from_discovery`].
    scope_key: visibility::ScopeKey,
}

// :1030-1044 — project parents keep their root
        let project_plugin_parents: Vec<crate::discovery::ProjectPluginParent> =
            crate::projects::ProjectStore::shared()
                .list()
                .map(|projects| {
                    projects
                        .into_iter()
                        .filter_map(|p| p.workspace_path)
                        .flat_map(|root| {
                            [
                                crate::discovery::ProjectPluginParent {
                                    project_root: root.clone(),
                                    dir: root.join(".aleph/plugins"),
                                },
                                crate::discovery::ProjectPluginParent {
                                    project_root: root.clone(),
                                    dir: root.join(".aleph/plugins.local"),
                                },
                            ]
                        })
                        .collect()
                })
                .unwrap_or_default();
// :1046-1050 unchanged in shape (`parents.extend(self.extra_plugin_parents.iter().cloned())`).

// :1070-1074 — inside `if seen.insert(canonical) {`
                    result.push(DiscoveredExtensionDir {
                        origin: PluginOrigin::classify(d.source()),
                        scope_key: visibility::ScopeKey::from_discovery(&d),
                        path: d.path,
                    });
```

(P1.9's `discover_and_mount` reads `found.origin` from this struct; it now also reads `found.scope_key`.)

The test helper (3ddc1f2e7 `mod.rs:1531`, wherever P1.9 leaves `isolated_manager`):

```rust
            extra_plugin_parents: vec![crate::discovery::ProjectPluginParent {
                project_root: dir.to_path_buf(),
                dir: dir.join("plugins"),
            }],
```

`src/extension/lifecycle.rs` (plan-P1 P1.9 code, three edits):

1. `build_record` gains the key (quote of P1's fn):

```rust
    fn build_record(output: &AdapterOutput, root_dir: PathBuf, origin: PluginOrigin) -> PluginRecord {
        let mut record = PluginRecord::from_adapter_output(output, root_dir.clone());
        record.origin = origin;
        if let Ok(m) = manifest::parse_manifest_from_dir_cached_global(&root_dir) {
            record.kind = m.kind;
        }
        record
    }
```

becomes:

```rust
    /// The record for a parsed plugin. Adapters hardcode `Global` and `Static`;
    /// where it was found (origin AND visibility key) and what runtime it
    /// needs are facts of discovery and of the manifest, applied here in one
    /// place.
    fn build_record(
        output: &AdapterOutput,
        root_dir: PathBuf,
        origin: PluginOrigin,
        scope_key: visibility::ScopeKey,
    ) -> PluginRecord {
        let mut record = PluginRecord::from_adapter_output(output, root_dir.clone());
        record.origin = origin;
        record.scope_key = scope_key;
        if let Ok(m) = manifest::parse_manifest_from_dir_cached_global(&root_dir) {
            record.kind = m.kind;
        }
        record
    }
```

   with its two callers: `discover_and_mount` — `let record = Self::build_record(&output, dir_path.clone(), found.origin, found.scope_key.clone());` and `mount_inner` — the row read `.map(|r| (r.root_dir.clone(), r.origin))` becomes `.map(|r| (r.root_dir.clone(), r.origin, r.scope_key.clone()))`, destructured as `(root_dir, origin, scope_key)`, and `Self::build_record(&output, root_dir, origin, scope_key)`. (`unmount_inner` re-registers the shell row, so the key survives a disable; `mount` never re-runs discovery.)

2. `unparsed_record` (P1.9; quote):

```rust
    fn unparsed_record(dir_path: &std::path::Path, error: &str) -> PluginRecord {
        let leaf = dir_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir_path.display().to_string());
        PluginRecord::new(leaf.clone(), leaf, PluginKind::Static, PluginOrigin::Global)
            .with_root_dir(dir_path.to_path_buf())
            .with_error(error.to_string())
    }
```

   becomes:

```rust
    /// The `Error` row for a directory whose manifest does not parse. It used
    /// to vanish at `debug!` level — on every surface identical to "never
    /// installed" — so it gets a row, an id derived from the directory, the
    /// parse error, and the key of where it was found, so `plugins.list` can
    /// say which project owns the broken plugin. Origin stays `Global` (as it
    /// always was for this row).
    fn unparsed_record(
        dir_path: &std::path::Path,
        error: &str,
        scope_key: visibility::ScopeKey,
    ) -> PluginRecord {
        let leaf = dir_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| dir_path.display().to_string());
        let mut record = PluginRecord::new(leaf.clone(), leaf, PluginKind::Static, PluginOrigin::Global)
            .with_root_dir(dir_path.to_path_buf())
            .with_error(error.to_string());
        record.scope_key = scope_key;
        record
    }
```

   and its one caller in `discover_and_mount` (P1.9 quote: `.register_plugin(Self::unparsed_record(dir_path, &e.to_string()));`) becomes `.register_plugin(Self::unparsed_record(dir_path, &e.to_string(), found.scope_key.clone()));`.

3. `write_failed_row(shell, step, reason)` (P1.9, the ONE site a mount-step failure becomes `Error("<step>: <reason>")`) needs no edit: `shell` is the clone `mount_parsed` took of the record `build_record` built, key included — the key survives a failed mount for the same reason `origin` does.

`src/extension/mod.rs:1128-1145` (`sync_hooks_from_registry`, which P1.9 keeps and calls from `after_transition`): the registry snapshot block collects `(HookRegistration, ScopeKey)` pairs instead of bare registrations:

```rust
        let hook_regs: Vec<(HookRegistration, visibility::ScopeKey)> = {
            let registry = self.plugin_registry.read().await;
            registry
                .list_hooks()
                .into_iter()
                .filter_map(|hook| {
                    registry
                        .get_plugin(&hook.plugin_id)
                        .filter(|plugin| plugin.status.is_active())
                        .map(|plugin| (hook.clone(), plugin.scope_key.clone()))
                })
                .collect()
        };
        // …
        for (hr, scope_key) in hook_regs {
            // (destructure unchanged)
            let hook_config = HookConfig {
                // … (unchanged fields)
                scope_key,   // replaces P2.2's `ScopeKey::Global` placeholder
            };
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib from_discovery_yields_project -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib discovery::scanner::tests -- --nocapture` → PASS (new test + the three pre-existing `discover_plugins_with_extra` tests, re-typed).
Run: `cargo test -p alephcore --lib extension::lifecycle::tests::load_all_stamps -- --nocapture`, `… mount_after_unmount_keeps_the_rows_scope_key`, `… plugin_hooks_inherit_the_owning_rows_scope_key` → PASS.
Run: `cargo test -p alephcore --lib --no-run && cargo test -p alephcore --features test-helpers --test '*' --no-run` → compiles (`tests/mcp_scope_isolation.rs:41` uses `PluginRecord::new` with 4 args, unchanged; every `DiscoveredPath::new` / `.source` field reader the list above missed is a compile error — `rg -n 'DiscoveredPath::new\(|\.source\b' src/discovery src/extension/mod.rs` before building).

**Mutation step (guard proof):** in `lifecycle.rs::build_record` delete `record.scope_key = scope_key;` → `load_all_stamps_project_scope_key_from_the_discovery_root` red with `left: Global, right: Project(...)`; in `unparsed_record` delete `record.scope_key = scope_key;` → `load_all_stamps_the_key_on_parse_error_rows_too` red. Restore both. Second mutation (R2.5's reason to exist): add `ClaudeCache` to `GlobalRoot` → `cargo check -p alephcore` fails at `DiscoveryScope::source` and at `ScopeKey::from_discovery` with `non-exhaustive patterns` — the two places a new root must be classified. Revert.

- [ ] **Step 5: Commit**

```bash
git add src/discovery/types.rs src/discovery/scanner.rs src/discovery/mod.rs src/extension/types/plugins.rs src/extension/mod.rs src/extension/lifecycle.rs src/extension/visibility.rs src/extension/hooks/executor.rs
git commit -m "discovery/extension: registry rows carry a ScopeKey derived from the discovery scope

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.4: `ExtensionManager::plugin_visible` — the one id→key lookup every face uses

**Files:**
- Modify: `src/extension/mod.rs:176-178` (field), `:344` (constructor), `:1210-1249` (`build_active_plugin_tool_index` also yields the key map), `:1237-1249` (`refresh_active_plugin_tools` writes both snapshots), new methods after `:1261`
- Test: `src/extension/mod.rs` (tests module)

**Interfaces:**
- Consumes: `PluginRecord.scope_key` (P2.3), `visible_to` (P2.1).
- Produces:
  ```rust
  impl ExtensionManager {
      /// Sync snapshot of `plugin_id → ScopeKey` for every registered row (any status), refreshed
      /// with the tool index. `None` = unknown plugin id.
      pub fn plugin_scope_key(&self, plugin_id: &str) -> Option<ScopeKey>;
      /// `plugin_scope_key(id)` ⋄ `visible_to`. An UNKNOWN id is NOT visible (fail-closed): a face that
      /// asks about a plugin the registry never saw gets "no", not "sure".
      pub fn plugin_visible(&self, plugin_id: &str, ctx: &VisibilityCtx) -> bool;
  }
  ```

Why a snapshot on the manager and not a lookup through the async registry lock: `active_plugin_tools_for_agent` (`tool_refresh.rs:27`) and `resolve_plugin_handler` (`free_fns.rs:29`) are sync and already read a sync snapshot (`active_plugin_tools`, `mod.rs:176`) for that reason; the key map is derived in the same registry read, so the two cannot drift. It is *not* a new process-global static (see orientation: `capability/census.rs` pins that count).

- [ ] **Step 1: Write the failing test**

`src/extension/mod.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn plugin_visible_answers_from_the_rows_key_and_fails_closed_on_unknown_ids() {
        use crate::extension::visibility::{ScopeKey, VisibilityCtx};
        let dir = tempfile::tempdir().unwrap();
        write_project_plugin(dir.path(), "p2-vis");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();

        let here = VisibilityCtx { project_root: Some(crate::extension::visibility::canonical_root(dir.path())) };
        let elsewhere = VisibilityCtx { project_root: None };
        assert_eq!(manager.plugin_scope_key("p2-vis"), Some(ScopeKey::project(dir.path())));
        assert!(manager.plugin_visible("p2-vis", &here));
        assert!(!manager.plugin_visible("p2-vis", &elsewhere));
        // Never registered → not visible anywhere. `Some(true)` here would let
        // a face show a tool whose owner the registry cannot name.
        assert!(!manager.plugin_visible("never-registered", &here));
        assert_eq!(manager.plugin_scope_key("never-registered"), None);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib plugin_visible_answers_from_the_rows_key -- --nocapture`
Expected: FAIL to compile — `no method named `plugin_scope_key` found`.

- [ ] **Step 3: Write minimal implementation**

`src/extension/mod.rs`:

```rust
// :176-178 — next to `active_plugin_tools`
    /// `plugin_id → ScopeKey` for every registered row, refreshed in the same
    /// registry read as [`Self::active_plugin_tools`]. Read by every visibility
    /// face through [`Self::plugin_visible`]; `Option::None` for an id the
    /// registry never saw, which the predicate reads as "not visible".
    plugin_scope_keys: Arc<StdRwLock<HashMap<String, visibility::ScopeKey>>>,

// :344 (constructor)
            plugin_scope_keys: Arc::new(StdRwLock::new(HashMap::new())),

// :1210 — `build_active_plugin_tool_index` returns both maps
    fn build_active_plugin_tool_index(
        registry: &PluginRegistry,
    ) -> (HashMap<String, ToolRegistration>, HashMap<String, visibility::ScopeKey>) {
        // (existing body builds `active_tools`; then:)
        let scope_keys = registry
            .list_plugins()
            .into_iter()
            .map(|p| (p.id.clone(), p.scope_key.clone()))
            .collect();
        (active_tools, scope_keys)
    }

// :1237-1249
    async fn refresh_active_plugin_tools(&self) {
        let (active_tools, scope_keys) = {
            let registry = self.plugin_registry.read().await;
            Self::build_active_plugin_tool_index(&registry)
        };
        *self.active_plugin_tools.write().unwrap_or_else(|e| e.into_inner()) = active_tools;
        *self.plugin_scope_keys.write().unwrap_or_else(|e| e.into_inner()) = scope_keys;
        self.plugin_tool_revision.fetch_add(1, Ordering::SeqCst);
    }

// after :1261 (`active_plugin_tools_snapshot`)
    /// The visibility key of a registered plugin, any status. `None` = unknown id.
    pub fn plugin_scope_key(&self, plugin_id: &str) -> Option<visibility::ScopeKey> {
        self.plugin_scope_keys
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(plugin_id)
            .cloned()
    }

    /// May a session in `ctx` see anything this plugin contributes?
    ///
    /// Fail-closed on an unknown id: the registry is the only authority on
    /// where a plugin came from, and a plugin it cannot name has no key to
    /// compare. Every capability face (tool index, skills, sub-agents, slash
    /// list, MCP bridge) asks this; hooks compare their own stamped key with
    /// the same `visible_to`.
    pub fn plugin_visible(&self, plugin_id: &str, ctx: &visibility::VisibilityCtx) -> bool {
        self.plugin_scope_key(plugin_id)
            .is_some_and(|key| visibility::visible_to(&key, ctx))
    }
```

Note the existing callers of the renamed-signature index builder: only `refresh_active_plugin_tools` calls `build_active_plugin_tool_index` (`rg -n 'build_active_plugin_tool_index' src` → 2 hits, definition + that call).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib plugin_visible_answers_from_the_rows_key -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib extension:: --no-run` → compiles.

- [ ] **Step 5: Commit**

```bash
git add src/extension/mod.rs
git commit -m "extension: plugin_visible — sync plugin_id→ScopeKey snapshot, fail-closed on unknown ids

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.5: Face ① — the per-request plugin tool index

**Files:**
- Modify: `src/extension/mod.rs` (new `active_plugin_tools_visible_to`, next to `active_plugin_tools_snapshot` at `:1261`)
- Modify: `src/gateway/execution_engine/tool_refresh.rs:26-37` (`active_plugin_tools_for_agent` takes `&VisibilityCtx`)
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:213-226` (compute the ctx once; pass it)
- Test: `src/extension/mod.rs` (tests module)

**Interfaces:**
- Consumes: `ExtensionManager::plugin_visible` (P2.4).
- Produces:
  ```rust
  impl ExtensionManager {
      /// `active_plugin_tools_snapshot()` retained by the owning plugin's visibility. The snapshot itself
      /// stays global (one index, all projects); the filter is applied at request time.
      pub fn active_plugin_tools_visible_to(&self, ctx: &VisibilityCtx) -> Vec<ToolRegistration>;
  }
  pub(super) fn active_plugin_tools_for_agent(ext: &ExtensionManager, agent: &AgentInstance, ctx: &VisibilityCtx) -> Vec<UnifiedTool>;
  ```

Decision (lead's question): **request time, snapshot stays global.** The snapshot is rebuilt on every activation change by `refresh_active_plugin_tools`; a per-project snapshot would need a map keyed by every project root that ever ran, refreshed on every change, i.e. N derivations of one fact. The request-time filter is one `HashMap` lookup per tool.

Dispatch twin: `resolve_active_plugin_tool` (`mod.rs:1272`) → `resolve_plugin_handler` (`inherent.rs:334`) is reached from `tool_registry_impl.rs:1763` only for a tool the model called; the model can only call what is in `allowed_names` (`inner.rs:778`, built from the filtered `allowed_tools`) because `ScopedToolService::is_allowed` (`scoped/builder.rs:443`) retains on dispatch (`scoped/dispatch.rs:189`). So filtering the listing filters dispatch for the model path. The RPC face `plugins.callTool` is an operator verb with no project and is deliberately not gated (Contract deltas).

- [ ] **Step 1: Write the failing test**

`src/extension/mod.rs` `mod tests` (the `builtin_registry/mod.rs:95-140` tests show the registry-seeding shape):

```rust
    /// Face ①: a Project(p) plugin's tools are in the index for a session in
    /// p and absent for a session with no project. The snapshot is global;
    /// the request-time filter is what changes.
    #[tokio::test]
    async fn plugin_tool_index_is_filtered_by_the_owning_plugins_visibility() {
        use crate::extension::visibility::{canonical_root, ScopeKey, VisibilityCtx};
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let proj = tempfile::tempdir().unwrap();
        let manager = ExtensionManager::with_defaults().await.unwrap();
        {
            let mut registry = manager.get_plugin_registry_mut().await;
            let mut record = PluginRecord::new(
                "p2-tools".into(),
                "P2 Tools".into(),
                PluginKind::Wasm,
                PluginOrigin::Workspace,
            );
            record.scope_key = ScopeKey::project(proj.path());
            registry.register_plugin(record);
            registry.register_tool(ToolRegistration {
                name: "p2_scoped_tool".into(),
                description: "only in its project".into(),
                parameters: serde_json::json!({"type": "object"}),
                handler: "tool_p2_scoped_tool".into(),
                plugin_id: "p2-tools".into(),
            });
            let mut global = PluginRecord::new(
                "p2-global-tools".into(),
                "P2 Global".into(),
                PluginKind::Wasm,
                PluginOrigin::Global,
            );
            global.scope_key = ScopeKey::Global;
            registry.register_plugin(global);
            registry.register_tool(ToolRegistration {
                name: "p2_global_tool".into(),
                description: "everywhere".into(),
                parameters: serde_json::json!({"type": "object"}),
                handler: "tool_p2_global_tool".into(),
                plugin_id: "p2-global-tools".into(),
            });
        }
        manager.sync_runtime_snapshots().await;

        let names = |ctx: &VisibilityCtx| -> Vec<String> {
            manager
                .active_plugin_tools_visible_to(ctx)
                .into_iter()
                .map(|t| t.name)
                .collect()
        };
        let in_p = VisibilityCtx { project_root: Some(canonical_root(proj.path())) };
        let nowhere = VisibilityCtx { project_root: None };
        assert_eq!(names(&in_p), vec!["p2_global_tool", "p2_scoped_tool"]);
        assert_eq!(names(&nowhere), vec!["p2_global_tool"]);
        // The unfiltered snapshot still holds both: the filter is per request.
        assert_eq!(manager.active_plugin_tools_snapshot().len(), 2);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib plugin_tool_index_is_filtered -- --nocapture`
Expected: FAIL to compile — `no method named `active_plugin_tools_visible_to``.

- [ ] **Step 3: Write minimal implementation**

`src/extension/mod.rs`, after `active_plugin_tools_snapshot` (`:1261-1269`):

```rust
    /// [`Self::active_plugin_tools_snapshot`] narrowed to the plugins a
    /// session in `ctx` may see. This is face ① of the visibility predicate;
    /// the request-build site (`tool_refresh::active_plugin_tools_for_agent`)
    /// is its only production caller.
    pub fn active_plugin_tools_visible_to(
        &self,
        ctx: &visibility::VisibilityCtx,
    ) -> Vec<ToolRegistration> {
        self.active_plugin_tools_snapshot()
            .into_iter()
            .filter(|tool| self.plugin_visible(&tool.plugin_id, ctx))
            .collect()
    }
```

`src/gateway/execution_engine/tool_refresh.rs:26-37` — quote of today's body:

```rust
/// Get active plugin tools filtered by agent allowlist.
pub(super) fn active_plugin_tools_for_agent(
    extension_manager: &crate::extension::ExtensionManager,
    agent: &AgentInstance,
) -> Vec<crate::tool_metadata::UnifiedTool> {
    extension_manager
        .active_plugin_tools_snapshot()
        .into_iter()
        .filter(|tool| agent.is_tool_allowed(&tool.name))
        .map(plugin_tool_to_unified_tool)
        .collect()
}
```

becomes:

```rust
/// Get active plugin tools filtered by agent allowlist AND by the owning
/// plugin's visibility to this session (`extension::visibility`).
pub(super) fn active_plugin_tools_for_agent(
    extension_manager: &crate::extension::ExtensionManager,
    agent: &AgentInstance,
    visibility: &crate::extension::visibility::VisibilityCtx,
) -> Vec<crate::tool_metadata::UnifiedTool> {
    extension_manager
        .active_plugin_tools_visible_to(visibility)
        .into_iter()
        .filter(|tool| agent.is_tool_allowed(&tool.name))
        .map(plugin_tool_to_unified_tool)
        .collect()
}
```

`src/gateway/execution_engine/run_loop/inner.rs:213-226` — before `let base_allowed_tools` add:

```rust
        // Who may see what, for this run: derived once here from the
        // run-loop task-local (`run_agent_loop` published `workspace_override`
        // just above this frame) and handed to every face built below —
        // the plugin tool index here, the MCP join further down. One value
        // per run, not one read per face.
        let visibility = crate::extension::visibility::VisibilityCtx::for_session();
```

and `:224` becomes `allowed_tools.extend(active_plugin_tools_for_agent(ext_manager, &agent, &visibility));`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib plugin_tool_index_is_filtered -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib gateway::execution_engine --no-run` → compiles.

**Mutation step:** in `active_plugin_tools_visible_to` replace the filter body with `true` → the test's `names(&nowhere)` assertion goes red (`["p2_global_tool", "p2_scoped_tool"]`). Restore.

- [ ] **Step 5: Commit**

```bash
git add src/extension/mod.rs src/gateway/execution_engine/tool_refresh.rs src/gateway/execution_engine/run_loop/inner.rs
git commit -m "execution_engine: plugin tool index is filtered by the owning plugin's visibility at request time

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.6: Face ② — skills: the `<available_skills>` index and the `skill_read` search set

**Files:**
- Modify: `src/extension/visibility.rs` (new `retain_visible_plugin_skills`)
- Modify: `src/extension/projection.rs:53-61,77-107,118-143` (`plugin_skill_dirs` carries `(PathBuf, ScopeKey)`)
- Modify: `src/utils/paths.rs:642-660` (`PLUGIN_SKILL_DIRS: RwLock<Vec<(PathBuf, ScopeKey)>>`), `:918-928` (`get_all_skills_dirs` filters)
- Modify: `src/orchestrator/harness_bridge/prompt_build.rs:294-298` (filter the snapshot's manifests right after the fetch)
- Modify (doc row): `src/capability/census.rs:136` (`RwLock<Vec<PathBuf>>` → `RwLock<Vec<(PathBuf, ScopeKey)>>`)
- Test: `src/extension/visibility.rs`, `src/utils/paths.rs` (tests module), `src/extension/mod.rs` (tests module)

**Interfaces:**
- Consumes: `ExtensionManager::plugin_scope_key` (P2.4), `crate::domain::skill::{SkillManifest, SkillSource}`.
- Produces:
  ```rust
  /// The one list-face filter (P2.8 reuses it): unowned items pass; owned items pass iff the owner is visible.
  pub fn retain_visible_owned<T>(items: Vec<T>, ctx: &VisibilityCtx, owner_of: impl Fn(&T) -> Option<&str>, visible: impl Fn(&str, &VisibilityCtx) -> bool) -> Vec<T>;
  /// Keep every non-plugin skill; keep a `SkillSource::Plugin(id)` skill iff `lookup(id)` is a key visible to `ctx`.
  pub fn retain_visible_plugin_skills(
      manifests: Vec<SkillManifest>, ctx: &VisibilityCtx,
      lookup: impl Fn(&str) -> Option<ScopeKey>,
  ) -> Vec<SkillManifest>;
  pub fn publish_plugin_skill_dirs(dirs: Vec<(PathBuf, ScopeKey)>);   // utils::paths
  pub fn plugin_skill_dirs() -> Vec<(PathBuf, ScopeKey)>;
  ```

Two sub-faces of one verb (judgment §9): the index the model reads (`prompt_build.rs:587` → `SkillInstructionsLayer`) and the search set `skill_read` / `skill_list` resolve against (`get_all_skills_dirs`, `utils/paths.rs:918-928`, reached from `skill_reader/read.rs:118` with the run's `current_project_root()` — `tool_registry_impl.rs:137-147`). Filtering only the index would leave a model in project B able to `skill_read` project A's plugin skill by name.

- [ ] **Step 1: Write the failing tests**

`src/extension/visibility.rs` `mod tests`:

```rust
    #[test]
    fn retain_visible_plugin_skills_keeps_non_plugin_and_visible_plugin_skills_only() {
        use crate::domain::skill::{PluginId, SkillContent, SkillManifest, SkillSource};
        let p = tempdir().unwrap();
        let mk = |id: &str, src: SkillSource| {
            SkillManifest::new(id, id, format!("{id} desc"), SkillContent::new("body"), src)
        };
        let manifests = vec![
            mk("bundled-one", SkillSource::Bundled),
            mk("proj-plugin-skill", SkillSource::Plugin(PluginId::new("proj-plugin"))),
            mk("global-plugin-skill", SkillSource::Plugin(PluginId::new("global-plugin"))),
            mk("orphan-plugin-skill", SkillSource::Plugin(PluginId::new("unknown-plugin"))),
        ];
        let lookup = |id: &str| match id {
            "proj-plugin" => Some(ScopeKey::project(p.path())),
            "global-plugin" => Some(ScopeKey::Global),
            _ => None,
        };
        let names = |ctx: &VisibilityCtx| -> Vec<String> {
            retain_visible_plugin_skills(manifests.clone(), ctx, lookup)
                .iter()
                .map(|m| m.name().to_string())
                .collect()
        };
        let in_p = VisibilityCtx { project_root: Some(canonical_root(p.path())) };
        let nowhere = VisibilityCtx { project_root: None };
        assert_eq!(names(&in_p), vec!["bundled-one", "proj-plugin-skill", "global-plugin-skill"]);
        // No project: the project plugin's skill is gone; the orphan (a plugin
        // id the registry cannot name) is gone in BOTH contexts — fail-closed.
        assert_eq!(names(&nowhere), vec!["bundled-one", "global-plugin-skill"]);
    }
```

`src/utils/paths.rs` `mod tests` (next to `test_get_all_skills_dirs` at `:1890`):

```rust
    /// The `skill_read` search set honours the published plugin dirs' keys:
    /// a Project(p) plugin's `skills/` dir is searched from p and not from
    /// another project. Mirrors the prompt-index filter one layer up; the two
    /// are the read and the list of one verb.
    #[test]
    fn get_all_skills_dirs_drops_plugin_dirs_invisible_to_the_project() {
        use crate::extension::visibility::ScopeKey;
        let _home = IsolatedAlephHome::new();
        let proj_a = tempfile::tempdir().unwrap();
        let proj_b = tempfile::tempdir().unwrap();
        let a_plugin_skills = proj_a.path().join("plugin-a/skills");
        let global_plugin_skills = proj_a.path().join("plugin-g/skills");
        std::fs::create_dir_all(&a_plugin_skills).unwrap();
        std::fs::create_dir_all(&global_plugin_skills).unwrap();
        publish_plugin_skill_dirs(vec![
            (a_plugin_skills.clone(), ScopeKey::project(proj_a.path())),
            (global_plugin_skills.clone(), ScopeKey::Global),
        ]);

        let from_a = get_all_skills_dirs(Some(proj_a.path())).unwrap();
        assert!(from_a.contains(&a_plugin_skills));
        assert!(from_a.contains(&global_plugin_skills));

        let from_b = get_all_skills_dirs(Some(proj_b.path())).unwrap();
        assert!(!from_b.contains(&a_plugin_skills), "project A's plugin skills leaked into B");
        assert!(from_b.contains(&global_plugin_skills));

        publish_plugin_skill_dirs(Vec::new());
    }
```

`src/extension/mod.rs` `mod tests`:

```rust
    /// The projection publishes each plugin's skills dir WITH its key: the
    /// derivation of "which dirs" and "whose dirs" is one pass over the
    /// registry, so the two cannot disagree.
    #[tokio::test]
    async fn projection_publishes_skill_dirs_with_their_scope_keys() {
        let dir = tempfile::tempdir().unwrap();
        write_project_plugin(dir.path(), "p2-skilled");
        std::fs::create_dir_all(dir.path().join("plugins/p2-skilled/skills/hello")).unwrap();
        std::fs::write(
            dir.path().join("plugins/p2-skilled/skills/hello/SKILL.md"),
            "---\nname: hello\ndescription: hi\n---\nbody\n",
        )
        .unwrap();
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let published = crate::utils::paths::plugin_skill_dirs();
        let mine = published
            .iter()
            .find(|(d, _)| d.ends_with("p2-skilled/skills"))
            .expect("plugin skills dir published");
        assert_eq!(mine.1, crate::extension::visibility::ScopeKey::project(dir.path()));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib retain_visible_plugin_skills -- --nocapture` → FAIL to compile (`cannot find function`).
Run: `cargo test -p alephcore --lib get_all_skills_dirs_drops_plugin_dirs -- --nocapture` → FAIL to compile (`publish_plugin_skill_dirs` expects `Vec<PathBuf>`).

- [ ] **Step 3: Write minimal implementation**

`src/extension/visibility.rs` — first the one generic filter both this task and P2.8 delegate to (so "keep unowned, keep owned-and-visible" is written once):

```rust
/// The shape shared by every "list" face: items that no plugin owns pass;
/// an owned item passes iff its owner is visible. `owner_of` names the owner
/// (a `SkillSource::Plugin` id, a catalog row's `plugin_id`); `visible` is
/// `ExtensionManager::plugin_visible` in production.
#[must_use]
pub fn retain_visible_owned<T>(
    items: Vec<T>,
    ctx: &VisibilityCtx,
    owner_of: impl Fn(&T) -> Option<&str>,
    visible: impl Fn(&str, &VisibilityCtx) -> bool,
) -> Vec<T> {
    items
        .into_iter()
        .filter(|item| owner_of(item).map_or(true, |owner| visible(owner, ctx)))
        .collect()
}
```

then the skills wrapper (this introduces `extension → domain::skill`, a leaf; `extension` had no such edge before — recorded in Contract deltas):

```rust
/// Face ②a: the `<available_skills>` index. Non-plugin skills pass through;
/// a plugin skill is kept only when the registry can name its plugin AND
/// that plugin is visible to `ctx`. `lookup` is `ExtensionManager::plugin_scope_key`
/// in production and a closure in tests, so the filter stays a pure function.
#[must_use]
pub fn retain_visible_plugin_skills(
    manifests: Vec<crate::domain::skill::SkillManifest>,
    ctx: &VisibilityCtx,
    lookup: impl Fn(&str) -> Option<ScopeKey>,
) -> Vec<crate::domain::skill::SkillManifest> {
    retain_visible_owned(
        manifests,
        ctx,
        |m| match m.source() {
            crate::domain::skill::SkillSource::Plugin(id) => Some(id.as_str()),
            _ => None,
        },
        |owner, ctx| lookup(owner).is_some_and(|key| visible_to(&key, ctx)),
    )
}
```

`src/utils/paths.rs:642-660` — the static and its two accessors change element type (quote of today's static: `static PLUGIN_SKILL_DIRS: RwLock<Vec<PathBuf>> = RwLock::new(Vec::new());`):

```rust
static PLUGIN_SKILL_DIRS: RwLock<Vec<(PathBuf, crate::extension::visibility::ScopeKey)>> =
    RwLock::new(Vec::new());

/// Publish the installed plugins' skill base directories, each with the
/// owning plugin's visibility key, for skill discovery. …(rest of doc unchanged)
pub fn publish_plugin_skill_dirs(dirs: Vec<(PathBuf, crate::extension::visibility::ScopeKey)>) {
    let mut guard = PLUGIN_SKILL_DIRS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = dirs;
}

/// Snapshot the currently-published plugin skill base directories with keys.
#[must_use]
pub fn plugin_skill_dirs() -> Vec<(PathBuf, crate::extension::visibility::ScopeKey)> {
    PLUGIN_SKILL_DIRS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}
```

`src/utils/paths.rs:918-928` (quote: `for plugin_dir in plugin_skill_dirs() { push_if_new_skills_dir(&mut dirs, &plugin_dir, "plugin skills dir"); }`) becomes:

```rust
    //    b) Base dirs published by the extension manager (`<plugin_root>/skills`)
    //       via `publish_plugin_skill_dirs` — covers plugin roots outside the
    //       well-known locations (e.g. `plugins/cache/<market>/<id>/skills`).
    //       Empty until the first `ExtensionManager::load_all`. Each dir
    //       carries its plugin's visibility key: `project_dir` here is the
    //       same fact `VisibilityCtx::from_project_root` reads (the run's
    //       `workspace_override`, else CWD — see `start_dir` above), so this
    //       is the `skill_read` face of the one predicate.
    let visibility =
        crate::extension::visibility::VisibilityCtx::from_project_root(Some(start_dir.clone()));
    for (plugin_dir, key) in plugin_skill_dirs() {
        if crate::extension::visibility::visible_to(&key, &visibility) {
            push_if_new_skills_dir(&mut dirs, &plugin_dir, "plugin skills dir");
        }
    }
```

(`start_dir` is the `project_dir.unwrap_or(cwd)` computed at `:891-895`; passing `Some(start_dir)` makes `from_project_root` see the already-resolved value and canonicalise it, so the two fallbacks cannot drift.)

`src/extension/projection.rs`:

```rust
// :58 — field type
    pub(crate) plugin_skill_dirs: Vec<(PathBuf, super::visibility::ScopeKey)>,

// :80-87 — the derivation loop
        let mut plugin_skill_dirs: Vec<(PathBuf, super::visibility::ScopeKey)> = Vec::new();
        for record in registry.list_active_plugins() {
            let dir = record.root_dir.join("skills");
            if dir.is_dir()
                && !base_skill_dirs.contains(&dir)
                && !plugin_skill_dirs.iter().any(|(d, _)| *d == dir)
            {
                plugin_skill_dirs.push((dir, record.scope_key.clone()));
            }
        }

// :129 — the SkillSystem scan still takes bare dirs (it indexes everything;
// the per-request filter is `retain_visible_plugin_skills`)
        skill_dirs.extend(projection.plugin_skill_dirs.iter().map(|(d, _)| d.clone()));
// :136 unchanged in shape: `publish_plugin_skill_dirs(projection.plugin_skill_dirs.clone());`
```

`src/orchestrator/harness_bridge/prompt_build.rs:294-298` (quote):

```rust
        // Phase 1 — fetch the eligible-skill snapshot once; reused below.
        let skill_snapshot = match self.skill_system.as_ref() {
            Some(sys) => Some(sys.current_snapshot().await),
            None => None,
        };
```

becomes:

```rust
        // Phase 1 — fetch the eligible-skill snapshot once; reused below.
        // Narrowed HERE, before `has_skills` and `eligible_skills` read it,
        // to the plugins this session may see (face ② of
        // `extension::visibility`): a project-scoped plugin's skills must not
        // reach a session in another project. Non-plugin skills pass through.
        // No extension manager (cold tests, simulated boot) ⇒ no plugin skill
        // is shown, which is the fail-closed answer — the manager is the only
        // authority on where a plugin came from.
        let visibility = crate::extension::visibility::VisibilityCtx::for_session();
        let skill_snapshot = match self.skill_system.as_ref() {
            Some(sys) => {
                let mut snap = sys.current_snapshot().await;
                snap.eligible_manifests = crate::extension::visibility::retain_visible_plugin_skills(
                    std::mem::take(&mut snap.eligible_manifests),
                    &visibility,
                    |id| crate::extension::try_extension_manager().and_then(|m| m.plugin_scope_key(id)),
                );
                Some(snap)
            }
            None => None,
        };
```

`src/capability/census.rs:136` doc row: `RwLock<Vec<PathBuf>>` → `RwLock<Vec<(PathBuf, ScopeKey)>>` (prose only; the shape the census matches — a `RwLock<Vec<…>>` container static — is unchanged, and `cargo test -p alephcore --bins` / the census tests must stay green: the number it pins does not move).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib retain_visible_plugin_skills -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib get_all_skills_dirs -- --nocapture` → PASS (the new test and the pre-existing `test_get_all_skills_dirs*`).
Run: `cargo test -p alephcore --lib projection_publishes_skill_dirs_with_their_scope_keys -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib extension::projection` → the census `publishing_plugin_projections_has_exactly_one_author` stays green (the publish call did not move).
Run: `cargo test -p alephcore --lib capability::census` → green (count unchanged).

**Mutation step:** in `get_all_skills_dirs` drop the `if visible_to(...)` guard → `get_all_skills_dirs_drops_plugin_dirs_invisible_to_the_project` red ("project A's plugin skills leaked into B"). Restore.

- [ ] **Step 5: Commit**

```bash
git add src/extension/visibility.rs src/extension/projection.rs src/utils/paths.rs src/orchestrator/harness_bridge/prompt_build.rs src/capability/census.rs src/extension/mod.rs
git commit -m "skills: plugin skill dirs carry their ScopeKey; index and skill_read search set filter by visibility

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.7: Face ③ — plugin sub-agents

**Files:**
- Modify: `src/agents/registry.rs:9-46` (`PLUGIN_SUBAGENTS` holds `PluginSubagent { scope_key, def }`), `:105-150` (`available_agent_ids` / `spawnable_agent_ids` take `project_root`), `:280-294` (`resolve`'s plugin fallback filters), tests `:846-899`
- Modify: `src/agents/mod.rs:41` (export `PluginSubagent`)
- Modify: `src/extension/projection.rs:59-60,89-101,137` (`subagents: Vec<PluginSubagent>`)
- Modify: `src/orchestrator/harness_bridge/prompt_build.rs:542-545` (catalog fold filters)
- Modify: `src/agents/subagent_tool/loop_tool.rs:864,881,1190,1367`; `src/builtin_tools/agent_manage/info.rs:117`; `src/agents/subagent_tool/tests.rs:2754` (pass `project_root`)
- Modify (doc row): `src/capability/census.rs:137`
- Test: `src/agents/registry.rs` (tests module)

**Interfaces:**
- Consumes: `ScopeKey`, `VisibilityCtx::from_project_root`, `visible_to` (P2.1/P2.2); `PluginRecord.scope_key` (P2.3).
- Produces:
  ```rust
  // src/agents/registry.rs
  pub struct PluginSubagent { pub scope_key: crate::extension::visibility::ScopeKey, pub def: AgentDef }
  pub fn publish_plugin_subagents(agents: Vec<PluginSubagent>);
  pub fn plugin_subagents() -> Arc<[PluginSubagent]>;
  /// Plugin defs visible to `ctx` — the ONE filter the three faces below share.
  pub fn visible_plugin_subagents(ctx: &VisibilityCtx) -> Vec<AgentDef>;
  impl AgentRegistry {
      pub fn available_agent_ids(&self, project_root: Option<&Path>) -> Vec<String>;
      pub fn spawnable_agent_ids(&self, project_root: Option<&Path>) -> Vec<String>;
      // resolve / resolve_spawnable: signature unchanged; the plugin fallback now filters
  }
  ```

`resolve(id, project_root)` already takes the root (used for the `.aleph/agents` overlay); it feeds the same value through `VisibilityCtx::from_project_root` for the plugin fallback instead of re-reading the task-local, so an explicit root and the ctx cannot disagree. The two id-listing faces gain the same parameter because `registry.rs:160-176` already insists they share `resolve_spawnable`'s predicate.

- [ ] **Step 1: Write the failing test**

`src/agents/registry.rs` `mod tests`:

```rust
    /// Face ③: a Project(p) plugin agent resolves and is listed only for a
    /// session in p. `resolve`, `available_agent_ids`, `spawnable_agent_ids`
    /// are three faces of one verb and must answer alike.
    #[test]
    fn plugin_subagents_are_visible_only_to_their_project() {
        use crate::extension::visibility::ScopeKey;
        let p = tempfile::tempdir().unwrap();
        let q = tempfile::tempdir().unwrap();
        publish_plugin_subagents(vec![
            PluginSubagent {
                scope_key: ScopeKey::project(p.path()),
                def: AgentDef::new("p2-proj-agent", AgentMode::SubAgent),
            },
            PluginSubagent {
                scope_key: ScopeKey::Global,
                def: AgentDef::new("p2-global-agent", AgentMode::SubAgent),
            },
        ]);
        let registry = AgentRegistry::with_builtins();

        assert!(registry.resolve("p2-proj-agent", Some(p.path())).is_some());
        assert!(registry.resolve("p2-proj-agent", Some(q.path())).is_none());
        assert!(registry.resolve("p2-global-agent", Some(q.path())).is_some());

        let in_p = registry.available_agent_ids(Some(p.path()));
        let in_q = registry.available_agent_ids(Some(q.path()));
        assert!(in_p.contains(&"p2-proj-agent".to_string()));
        assert!(!in_q.contains(&"p2-proj-agent".to_string()));
        assert!(in_q.contains(&"p2-global-agent".to_string()));
        assert_eq!(
            registry.spawnable_agent_ids(Some(q.path())).contains(&"p2-proj-agent".to_string()),
            false,
            "spawnable and resolve must agree"
        );

        publish_plugin_subagents(Vec::new());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib plugin_subagents_are_visible_only_to_their_project -- --nocapture`
Expected: FAIL to compile — `cannot find struct `PluginSubagent``.

- [ ] **Step 3: Write minimal implementation**

`src/agents/registry.rs:21-46` (quote of today's static + accessors: `static PLUGIN_SUBAGENTS: OnceLock<RwLock<Arc<[AgentDef]>>> = OnceLock::new();` … `pub fn publish_plugin_subagents(agents: Vec<AgentDef>)` … `pub fn plugin_subagents() -> Arc<[AgentDef]>`) becomes:

```rust
/// One plugin-shipped sub-agent with the visibility key of the plugin that
/// shipped it. `AgentDef` carries no plugin id (its `id` is the agent name),
/// so the key rides beside it rather than being re-derived from a name.
#[derive(Debug, Clone)]
pub struct PluginSubagent {
    pub scope_key: crate::extension::visibility::ScopeKey,
    pub def: AgentDef,
}

static PLUGIN_SUBAGENTS: OnceLock<RwLock<Arc<[PluginSubagent]>>> = OnceLock::new();

fn plugin_subagents_lock() -> &'static RwLock<Arc<[PluginSubagent]>> {
    PLUGIN_SUBAGENTS.get_or_init(|| RwLock::new(Arc::new([])))
}

pub fn publish_plugin_subagents(agents: Vec<PluginSubagent>) {
    let mut guard = plugin_subagents_lock()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *guard = Arc::from(agents.into_boxed_slice());
}

#[must_use]
pub fn plugin_subagents() -> Arc<[PluginSubagent]> {
    Arc::clone(
        &plugin_subagents_lock()
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

/// The plugin sub-agents a session in `ctx` may delegate to. Every reader of
/// [`plugin_subagents`] that reaches the model goes through this — the
/// prompt catalog, `resolve`, and the two id listings — so they cannot
/// disagree about which plugin agents exist for a given project.
#[must_use]
pub fn visible_plugin_subagents(
    ctx: &crate::extension::visibility::VisibilityCtx,
) -> Vec<AgentDef> {
    plugin_subagents()
        .iter()
        .filter(|p| crate::extension::visibility::visible_to(&p.scope_key, ctx))
        .map(|p| p.def.clone())
        .collect()
}
```

`registry.rs:116-124` / `:133-150` / `:290-293`:

```rust
    pub fn available_agent_ids(&self, project_root: Option<&std::path::Path>) -> Vec<String> {
        let ctx = crate::extension::visibility::VisibilityCtx::from_project_root(
            project_root.map(std::path::Path::to_path_buf),
        );
        let mut ids = self.list_ids();
        ids.extend(visible_plugin_subagents(&ctx).into_iter().map(|a| a.id));
        ids.sort();
        ids.dedup();
        ids
    }

    pub fn spawnable_agent_ids(&self, project_root: Option<&std::path::Path>) -> Vec<String> {
        let ctx = crate::extension::visibility::VisibilityCtx::from_project_root(
            project_root.map(std::path::Path::to_path_buf),
        );
        let mut ids: Vec<String> = self.list_subagents().into_iter().map(|a| a.id).collect();
        ids.extend(
            visible_plugin_subagents(&ctx)
                .into_iter()
                .filter(|a| a.mode == AgentMode::SubAgent)
                .map(|a| a.id),
        );
        ids.sort();
        ids.dedup();
        ids
    }

    // in `resolve`, replace the last statement
    //   plugin_subagents().iter().find(|a| a.id == id).cloned()
    // with
        let ctx = crate::extension::visibility::VisibilityCtx::from_project_root(
            project_root.map(std::path::Path::to_path_buf),
        );
        visible_plugin_subagents(&ctx).into_iter().find(|a| a.id == id)
```

(The B1-07 comment block at `:133-140` stays; only the iterator source changes.)

`src/agents/mod.rs:41`: `pub use registry::{builtin_agents, plugin_subagents, publish_plugin_subagents, visible_plugin_subagents, AgentRegistry, PluginSubagent};`

`src/extension/projection.rs`:

```rust
// :60
    pub(crate) subagents: Vec<crate::agents::PluginSubagent>,
// :92-101
        let subagents = registry
            .list_agents()
            .into_iter()
            .filter_map(|agent| {
                let plugin = registry.get_plugin(&agent.plugin_id)?;
                if !plugin.status.is_active() {
                    return None;
                }
                super::plugin_agent_to_def(agent).map(|def| crate::agents::PluginSubagent {
                    scope_key: plugin.scope_key.clone(),
                    def,
                })
            })
            .collect();
// :137 unchanged: `crate::agents::publish_plugin_subagents(projection.subagents.clone());`
```

`src/orchestrator/harness_bridge/prompt_build.rs:542-545` (quote: `for a in crate::agents::plugin_subagents().iter() { by_id.entry(a.id.clone()).or_insert_with(|| a.clone()); }`) becomes:

```rust
            // Plugin sub-agents last, insert-if-absent (lowest precedence),
            // and only those this session may see — `visibility` was derived
            // above for the skill snapshot; same value, same run.
            for a in crate::agents::visible_plugin_subagents(&visibility) {
                by_id.entry(a.id.clone()).or_insert(a);
            }
```

(`visibility` is the binding P2.6 introduced at the top of this function, `:294`.)

Callers: `loop_tool.rs:864,881,1190,1367` → `.spawnable_agent_ids(project_root_ref)` (the binding is in scope at each site — `:851-852` and `:1358`); `agent_manage/info.rs:117` → `self.catalog.available_agent_ids(project_root.as_deref())`; `subagent_tool/tests.rs:2754` and `registry.rs:882` → pass `None`. The existing test `resolve_falls_back_to_plugin_subagents_but_registry_wins` (`:846-899`) wraps its two defs in `PluginSubagent { scope_key: ScopeKey::Global, def }`.

`src/capability/census.rs:137` doc row: `OnceLock<RwLock<Arc<[AgentDef]>>>` → `OnceLock<RwLock<Arc<[PluginSubagent]>>>` (prose).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib agents::registry -- --nocapture` → PASS (new test + `resolve_falls_back_to_plugin_subagents_but_registry_wins`).
Run: `cargo test -p alephcore --lib --no-run` → compiles (every `plugin_subagents()` reader is on the list above; the compiler catches a missed one).

**Mutation step:** in `visible_plugin_subagents` replace the `.filter(...)` with `.filter(|_| true)` → `plugin_subagents_are_visible_only_to_their_project` red at `resolve("p2-proj-agent", Some(q))`. Restore.

- [ ] **Step 5: Commit**

```bash
git add src/agents/registry.rs src/agents/mod.rs src/extension/projection.rs src/orchestrator/harness_bridge/prompt_build.rs src/agents/subagent_tool/loop_tool.rs src/agents/subagent_tool/tests.rs src/builtin_tools/agent_manage/info.rs src/capability/census.rs
git commit -m "agents: plugin sub-agents carry their ScopeKey; resolve/list/catalog filter by project visibility

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.8: Face ④ — the slash list and the slash fast path

**Files:**
- Modify: `src/skill/compat.rs:17-55` (`SkillInfo.plugin_id: Option<String>`)
- Modify: `src/tool_metadata/types/conflict.rs:116-121` (`ToolSource::Skill { id, plugin_id: Option<String> }`), match/construct sites `src/tool_metadata/types/unified/conversions.rs:19`, `src/command/parser.rs:144`, `src/gateway/handlers/tools_visibility.rs:64,223,275`, `src/tool_metadata/types/tool_info.rs:99`, `src/tool_metadata/types/conflict.rs:265,285,302,316`, `src/tool_metadata/registry/registration.rs:266-272`
- Modify: `src/extension/slash_effect.rs` (plan-P1 P1.7 `plugin_command_skill_info(&SkillRegistration) -> SkillInfo` — the one producer of a plugin command's `SkillInfo` (`plugin_command_skill_infos` maps over it); it replaced 3ddc1f2e7 `tool_catalog_init.rs:214-247`, which P1.10 deleted); the `skill_manifests` → `SkillInfo` block of `init_tool_catalog` (3ddc1f2e7 `tool_catalog_init.rs:173-195`, line numbers shift after P1.8 — cite by the `list_skills().await` call); test literals `src/tool_metadata/registry/tests.rs:74,82`, `src/gateway/execution_engine/slash_skill_scope.rs:288,417,526,534,559`, and any `SkillInfo {` literal P1.7's tests added in `slash_effect.rs`
- Modify: `src/command/parser.rs:9-34,102-118` (`ParsedCommand.owning_plugin`)
- Modify: `src/gateway/inbound_router/command_handler.rs:156-194` (`serialize_parsed_command` writes `owning_plugin`)
- Modify: `src/gateway/execution_engine/slash_command.rs:171-194` (gate before the `match`)
- Modify: `src/gateway/handlers/commands.rs:277-306` (`commands.list` filters owned entries — **no new param**, R2.3) + its registration closure in `init_tool_catalog` (3ddc1f2e7 `tool_catalog_init.rs:282-291`; after P1.8 cite by the `register("commands.list", …)` call)
- Modify: `src/extension/visibility.rs` (two pure helpers)
- Test: `src/extension/visibility.rs`, `src/command/parser.rs` (tests), `src/gateway/inbound_router/command_handler.rs` (tests), `src/gateway/execution_engine/tests.rs`, `src/gateway/handlers/commands.rs` (tests)

**Interfaces:**
- Consumes: `ExtensionManager::plugin_visible` (P2.4), `VisibilityCtx::from_project_root` (P2.2).
- Produces:
  ```rust
  pub struct SkillInfo { /* … */ #[serde(default, skip_serializing_if = "Option::is_none")] pub plugin_id: Option<String> }
  pub enum ToolSource { /* … */ Skill { id: String, #[serde(default, skip_serializing_if = "Option::is_none")] plugin_id: Option<String> }, /* … */ }
  pub struct ParsedCommand { /* … */ pub owning_plugin: Option<String> }   // Plugin{plugin_id} → Some; Skill{plugin_id} → as is; else None
  // src/extension/visibility.rs
  /// Face ④ list: drop catalog entries whose owning plugin is not visible. Non-owned entries pass.
  pub fn retain_visible_owned_commands(tools: Vec<UnifiedTool>, ctx: &VisibilityCtx, visible: impl Fn(&str, &VisibilityCtx) -> bool) -> Vec<UnifiedTool>;
  /// Face ④ dispatch: `Ok(())` when the mode JSON names no owner or a visible one; `Err(reason)` otherwise.
  pub fn slash_owner_admits(mode: &serde_json::Value, ctx: &VisibilityCtx, visible: impl Fn(&str, &VisibilityCtx) -> bool) -> Result<(), String>;
  ```

**Where the owner lives.** The catalog row is the only object every slash surface shares (`CommandParser::parse_async` → `ToolCatalog::resolve_command`, `parser.rs:102`), and today it forgets which plugin registered a `commands/*.md` command (`register_skills` builds `ToolSource::Skill { id }`, `registration.rs:270`). `plugin_id` on `SkillInfo` → copied onto `ToolSource::Skill` is the cheapest attachment; `ToolSource::Plugin { plugin_id }` already carries it. **P1 does not need this field** (R1.8): its `slash_command` disposer owns the exact ids it registered (`ToolCatalog::unregister_skills(&[String])`); `plugin_id` here is for visibility only.

**Where the gate sits.** Three surfaces stamp `SLASH_COMMAND_MODE_KEY` (`server_init.rs:272,466`, `execute.rs:676`, `inbound_router/mod.rs:895` via the router's own serialisation) and one consumer runs it: `execute_slash_command_fast_path` (`slash_command.rs:171`, called from `execute.rs:788`). Gating at the consumer covers every producer with one check and has `request.workspace_override` in hand (`slash_command.rs:175`). The owner travels in the mode JSON (`serialize_parsed_command` is its single producer, `command_handler.rs:139`), so no producer has to know about visibility.

**`commands.list`.** Callers today: TUI (`interfaces/tui/src/tui/commands.rs:1724`, sends `{"interface":"tui"}`) and CLI. Neither has a project; the Panel does not call it. So the handler takes **no** `project_root` param (R2.3, 判据 §9: a parameter with zero clients is not a delivered capability); it derives the ctx server-side with `VisibilityCtx::from_project_root(None)` — daemon CWD, the same answer a project-less run gets — and filters plugin-owned rows through the manager-backed closure. No client change in this task.

- [ ] **Step 1: Write the failing tests**

`src/extension/visibility.rs` `mod tests`:

```rust
    fn owned_tool(id: &str, source: crate::tool_metadata::ToolSource) -> crate::tool_metadata::UnifiedTool {
        crate::tool_metadata::UnifiedTool::new(id, id, "d", source)
    }

    #[test]
    fn retain_visible_owned_commands_drops_only_invisible_owners() {
        use crate::tool_metadata::ToolSource;
        let p = tempdir().unwrap();
        let tools = vec![
            owned_tool("builtin:help", ToolSource::Builtin),
            owned_tool("skill:user-skill", ToolSource::Skill { id: "user-skill".into(), plugin_id: None }),
            owned_tool("skill:proj:cmd", ToolSource::Skill { id: "proj:cmd".into(), plugin_id: Some("proj".into()) }),
            owned_tool("plugin:proj:tool", ToolSource::Plugin { plugin_id: "proj".into() }),
            owned_tool("plugin:glob:tool", ToolSource::Plugin { plugin_id: "glob".into() }),
        ];
        let visible = |id: &str, ctx: &VisibilityCtx| match id {
            "proj" => visible_to(&ScopeKey::project(p.path()), ctx),
            "glob" => true,
            _ => false,
        };
        let ids = |ctx: &VisibilityCtx| -> Vec<String> {
            retain_visible_owned_commands(tools.clone(), ctx, visible)
                .into_iter()
                .map(|t| t.id)
                .collect()
        };
        let in_p = VisibilityCtx { project_root: Some(canonical_root(p.path())) };
        let nowhere = VisibilityCtx { project_root: None };
        assert_eq!(ids(&in_p), vec!["builtin:help", "skill:user-skill", "skill:proj:cmd", "plugin:proj:tool", "plugin:glob:tool"]);
        assert_eq!(ids(&nowhere), vec!["builtin:help", "skill:user-skill", "plugin:glob:tool"]);
    }

    #[test]
    fn slash_owner_admits_is_a_no_op_without_an_owner_and_refuses_an_invisible_one() {
        let p = tempdir().unwrap();
        let visible = |id: &str, ctx: &VisibilityCtx| id == "proj" && visible_to(&ScopeKey::project(p.path()), ctx);
        let in_p = VisibilityCtx { project_root: Some(canonical_root(p.path())) };
        let nowhere = VisibilityCtx { project_root: None };

        let unowned = serde_json::json!({"type": "direct_tool", "tool_id": "help", "args": ""});
        assert!(slash_owner_admits(&unowned, &nowhere, visible).is_ok());

        let owned = serde_json::json!({"type": "skill", "skill_id": "proj:cmd", "owning_plugin": "proj", "args": ""});
        assert!(slash_owner_admits(&owned, &in_p, visible).is_ok());
        let refusal = slash_owner_admits(&owned, &nowhere, visible).unwrap_err();
        assert!(refusal.contains("proj"), "the refusal names the plugin: {refusal}");
    }
```

`src/command/parser.rs` `mod tests` (the module exists at `:174`; it builds a `CommandParser` over a `ToolCatalog` — follow its `test_parse_*` helpers):

```rust
    #[tokio::test]
    async fn parse_async_records_the_owning_plugin_for_plugin_owned_entries() {
        use crate::tool_metadata::{ToolSource, UnifiedTool};
        let catalog = Arc::new(ToolCatalog::new());
        for (id, name, source) in [
            ("skill:proj:cmd", "proj:cmd", ToolSource::Skill { id: "proj:cmd".into(), plugin_id: Some("proj".into()) }),
            ("plugin:diag:ping", "ping", ToolSource::Plugin { plugin_id: "diag".into() }),
            ("skill:plain", "plain", ToolSource::Skill { id: "plain".into(), plugin_id: None }),
        ] {
            catalog
                .register_with_conflict_resolution(UnifiedTool::new(id, name, "d", source))
                .await;
        }
        let parser = CommandParser::new(Arc::clone(&catalog));
        assert_eq!(parser.parse_async("/proj:cmd x").await.unwrap().owning_plugin.as_deref(), Some("proj"));
        assert_eq!(parser.parse_async("/ping").await.unwrap().owning_plugin.as_deref(), Some("diag"));
        assert_eq!(parser.parse_async("/plain").await.unwrap().owning_plugin, None);
    }
```

`src/gateway/inbound_router/command_handler.rs` `mod tests` (add; the file's existing tests show how a `ParsedCommand` is built — if none exists, construct one literally with `owning_plugin: Some("proj".into())`):

```rust
    #[test]
    fn serialize_parsed_command_carries_the_owner_into_the_mode_json() {
        use crate::command::{CommandContext, ParsedCommand};
        use crate::tool_metadata::ToolSourceType;
        let parsed = ParsedCommand {
            source_type: ToolSourceType::Skill,
            command_name: "proj:cmd".into(),
            tool_id: "skill:proj:cmd".into(),
            arguments: Some("x".into()),
            context: CommandContext::Skill {
                skill_id: "proj:cmd".into(),
                display_name: "cmd".into(),
                allowed_tools: None,
            },
            owning_plugin: Some("proj".into()),
        };
        let json: serde_json::Value =
            serde_json::from_str(&serialize_parsed_command(&parsed).unwrap()).unwrap();
        assert_eq!(json["owning_plugin"], "proj");
        // An unowned command writes no key at all — the consumer treats
        // "absent" as "no owner", never as "owner named by an empty string".
        let mut unowned = parsed;
        unowned.owning_plugin = None;
        let json: serde_json::Value =
            serde_json::from_str(&serialize_parsed_command(&unowned).unwrap()).unwrap();
        assert!(json.get("owning_plugin").is_none());
    }
```

`src/gateway/execution_engine/tests.rs` (next to `guest_slash_command_for_a_dangerous_tool_never_reaches_the_registry`, `:1643`):

```rust
/// Face ④ dispatch: a `/cmd` owned by a plugin the session cannot see is
/// refused at the one consumer of the mode JSON, whichever surface stamped
/// it. No process-global extension manager is installed in this test, so
/// the manager-backed lookup answers "unknown" — the fail-closed branch.
#[tokio::test]
async fn a_slash_command_owned_by_an_invisible_plugin_is_refused_before_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let agent = gate_test_agent(&temp, "slash-owner").await;
    let registry = Arc::new(CountingToolRegistry::new());
    let engine = slash_engine(Arc::clone(&registry));
    let emitter = Arc::new(TestEmitter::new());
    let session = SessionKey::main("slash-owner");
    let request = slash_request(&session, Some("operator"));
    let mode = serde_json::json!({
        "type": "direct_tool", "tool_id": "plugin:proj:ping", "args": "",
        "owning_plugin": "proj"
    })
    .to_string();
    let err = engine
        .execute_slash_command_fast_path("slash-run", &mode, &request, agent, emitter)
        .await
        .expect_err("an invisible owner must be refused");
    match err {
        ExecutionError::Failed(msg) => assert!(msg.contains("proj"), "{msg}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(registry.calls(), 0);
}
```

`src/gateway/handlers/commands.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn list_from_registry_hides_commands_of_plugins_the_daemon_cannot_see() {
        use crate::tool_metadata::ToolSource;
        let p = tempfile::tempdir().unwrap();
        let registry = ToolCatalog::new();
        for (id, name, source) in [
            ("skill:proj:cmd", "proj:cmd", ToolSource::Skill { id: "proj:cmd".into(), plugin_id: Some("proj".into()) }),
            ("skill:glob:cmd", "glob:cmd", ToolSource::Skill { id: "glob:cmd".into(), plugin_id: Some("glob".into()) }),
            ("builtin:help", "help", ToolSource::Builtin),
        ] {
            registry
                .register_with_conflict_resolution(UnifiedTool::new(id, name, "d", source))
                .await;
        }
        // `proj` is scoped to a tempdir — never the daemon CWD the handler
        // derives its ctx from — while `glob` is visible everywhere. The
        // handler has no project parameter (R2.3), so the tempdir plugin is
        // the "other project" case by construction.
        let visible = |id: &str, ctx: &crate::extension::visibility::VisibilityCtx| match id {
            "glob" => true,
            "proj" => crate::extension::visibility::visible_to(
                &crate::extension::visibility::ScopeKey::project(p.path()),
                ctx,
            ),
            _ => false,
        };
        let request = JsonRpcRequest::with_id("commands.list", None, json!(1));
        let resp = handle_list_from_registry(request, &registry, &visible).await;
        let got: Vec<String> = resp.result.unwrap()["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!got.contains(&"proj:cmd".to_string()), "{got:?}");
        assert!(got.contains(&"glob:cmd".to_string()), "{got:?}");
        assert!(got.contains(&"help".to_string()), "{got:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib retain_visible_owned_commands -- --nocapture` → FAIL to compile (`no field plugin_id` on `ToolSource::Skill`).

- [ ] **Step 3: Write minimal implementation**

`src/skill/compat.rs:17-40` — add after `allowed_tools`:

```rust
    /// The plugin that registered this entry (`commands/*.md` of a plugin, or a
    /// plugin-shipped skill). `None` for user / bundled skills. Carried onto
    /// `ToolSource::Skill` so the slash list and fast path can ask
    /// `extension::visibility` whether this session may see the owner, and so
    /// the plugin lifecycle can retract the entry by owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_id: Option<String>,
```

and in `From<SkillManifest>` (`:44-54`):

```rust
            plugin_id: match manifest.source() {
                crate::domain::skill::SkillSource::Plugin(id) => Some(id.as_str().to_string()),
                _ => None,
            },
```

`src/tool_metadata/types/conflict.rs:118-121`:

```rust
    Skill {
        /// Skill directory ID (e.g., "refine-text")
        id: String,
        /// Owning plugin, when a plugin registered this entry. See
        /// `skill::SkillInfo::plugin_id`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        plugin_id: Option<String>,
    },
```

`registration.rs:266-272`: `ToolSource::Skill { id: skill.id.clone(), plugin_id: skill.plugin_id.clone() }`. Pattern sites `conversions.rs:19`, `parser.rs:144`, `tools_visibility.rs:64` → `ToolSource::Skill { id, .. }`; literal sites `tool_info.rs:99`, `conflict.rs:265,285,302,316`, `tools_visibility.rs:223,275` → add `plugin_id: None`. `SkillInfo` producers: the `skill_manifests` → `SkillInfo` block in `init_tool_catalog` (3ddc1f2e7 `tool_catalog_init.rs:173-195`) becomes `.map(alephcore::skill::SkillInfo::from)` — the literal there is field-for-field the `From<&SkillManifest>` impl, so this removes a second derivation of the same six fields and picks up `plugin_id` from `SkillSource::Plugin`; P1.7's `slash_effect::plugin_command_skill_info(cmd: &SkillRegistration) -> SkillInfo` (quote of its literal: `SkillInfo { id: cmd.qualified_name(), name: cmd.name.clone(), description: cmd.description.clone(), scope: PromptScope::System, version: None, allowed_tools: None }`) gains `plugin_id: Some(cmd.plugin_id.clone()),` — this is the ONLY producer of a plugin command's `SkillInfo` (R1.3; `plugin_command_skill_infos` only maps over it), so the owner reaches every mount's slash entries and `unmount`'s removal is untouched (P1 removes by the ids it registered — Contract deltas); the seven test literals → `plugin_id: None`.

`src/command/parser.rs`:

```rust
// :10-34 — add the field
    /// The plugin that owns the resolved entry (`ToolSource::Plugin` or a
    /// plugin-registered `ToolSource::Skill`), for the visibility gate at the
    /// mode consumer. `None` = not plugin-owned.
    pub owning_plugin: Option<String>,

// :107-118
        let source_type = ToolSourceType::from(&resolved.tool.source);
        let owning_plugin = match &resolved.tool.source {
            ToolSource::Plugin { plugin_id } => Some(plugin_id.clone()),
            ToolSource::Skill { plugin_id, .. } => plugin_id.clone(),
            _ => None,
        };
        let command_name = resolved.tool.name.clone();
        let tool_id = resolved.tool.id.clone();
        let context = tool_to_command_context(resolved.tool);

        Some(ParsedCommand {
            source_type,
            command_name,
            tool_id,
            arguments: resolved.arguments,
            context,
            owning_plugin,
        })
```

`src/gateway/inbound_router/command_handler.rs:156-194`: `let value = match …` → `let mut value = match …;` and before `serde_json::to_string(&value).ok()`:

```rust
    if let Some(owner) = &parsed.owning_plugin {
        value["owning_plugin"] = serde_json::Value::String(owner.clone());
    }
```

`src/extension/visibility.rs`:

```rust
/// Face ④ (list): the owner of a catalog row, if a plugin registered it.
fn catalog_owner(tool: &crate::tool_metadata::UnifiedTool) -> Option<&str> {
    match &tool.source {
        crate::tool_metadata::ToolSource::Plugin { plugin_id } => Some(plugin_id.as_str()),
        crate::tool_metadata::ToolSource::Skill { plugin_id, .. } => plugin_id.as_deref(),
        _ => None,
    }
}

/// Face ④ (list): keep every row that no plugin owns, and an owned row only
/// when `visible(owner, ctx)`. `visible` is `ExtensionManager::plugin_visible`
/// in production; a closure in tests.
#[must_use]
pub fn retain_visible_owned_commands(
    tools: Vec<crate::tool_metadata::UnifiedTool>,
    ctx: &VisibilityCtx,
    visible: impl Fn(&str, &VisibilityCtx) -> bool,
) -> Vec<crate::tool_metadata::UnifiedTool> {
    retain_visible_owned(tools, ctx, catalog_owner, visible)
}

/// Face ④ (dispatch): admit a resolved slash command unless its owner is a
/// plugin this session may not see. `mode` is the JSON
/// `serialize_parsed_command` produced; an absent `owning_plugin` key is
/// "no owner". The refusal names the plugin so the operator knows which
/// project to enter — fail-closed, not fail-dead.
pub fn slash_owner_admits(
    mode: &serde_json::Value,
    ctx: &VisibilityCtx,
    visible: impl Fn(&str, &VisibilityCtx) -> bool,
) -> Result<(), String> {
    match mode.get("owning_plugin").and_then(serde_json::Value::as_str) {
        None => Ok(()),
        Some(owner) if visible(owner, ctx) => Ok(()),
        Some(owner) => Err(format!(
            "this command belongs to plugin `{owner}`, which is not available to this \
             session's project; enter the project that installed it to use it"
        )),
    }
}
```

`src/gateway/execution_engine/slash_command.rs:185-190` — after `let mode: serde_json::Value = …?;` and before `let mode_type = …`:

```rust
        // Face ④ of `extension::visibility`: whichever surface stamped this
        // mode (Panel/CLI resolver, channel router, TUI), the owner it names
        // is judged here, once, against the request's own project. No
        // installed extension manager ⇒ unknown owner ⇒ refused.
        let visibility = crate::extension::visibility::VisibilityCtx::from_project_root(
            request.workspace_override.clone(),
        );
        crate::extension::visibility::slash_owner_admits(&mode, &visibility, |owner, ctx| {
            crate::extension::try_extension_manager().is_some_and(|m| m.plugin_visible(owner, ctx))
        })
        .map_err(ExecutionError::Failed)?;
```

`src/gateway/handlers/commands.rs:277-306`:

```rust
pub async fn handle_list_from_registry(
    request: JsonRpcRequest,
    tool_registry: &ToolCatalog,
    owner_visible: &(dyn Fn(&str, &crate::extension::visibility::VisibilityCtx) -> bool + Sync),
) -> JsonRpcResponse {
    // (channel hint unchanged)
    // No project parameter (R2.3: no client sends one). The ctx is the one a
    // project-less run gets — `from_project_root(None)` = daemon CWD — so the
    // list the TUI/CLI see agrees with what a run started from them can use.
    let visibility = crate::extension::visibility::VisibilityCtx::from_project_root(None);

    let tools: Vec<UnifiedTool> = match channel {
        Some(ch) => tool_registry.list_for_channel(ch).await,
        None => tool_registry.list_root_commands().await,
    };
    let tools = crate::extension::visibility::retain_visible_owned_commands(tools, &visibility, owner_visible);
    let tree = build_command_tree(tools);
    // (response unchanged)
```

The registration closure in `init_tool_catalog` (3ddc1f2e7 `tool_catalog_init.rs:282-291`) passes the manager-backed closure:

```rust
                alephcore::gateway::handlers::commands::handle_list_from_registry(req, &registry, &|owner, ctx| {
                    alephcore::extension::try_extension_manager().is_some_and(|m| m.plugin_visible(owner, ctx))
                })
                .await
```

and the 7 existing `handle_list_from_registry(request, &registry)` test calls in `commands.rs` pass `&|_, _| true`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib visibility -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib command::parser -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib serialize_parsed_command_carries_the_owner -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib a_slash_command_owned_by_an_invisible_plugin -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib gateway::handlers::commands -- --nocapture` → PASS.
Run: `cargo test -p alephcore --bins --no-run` (`init_tool_catalog` compiles) and `cargo test -p alephcore --lib --no-run` (P1.7's `slash_effect` tests still compile with the new field).
Run: `cargo test -p aleph-panel --lib --no-run` — `ToolSource` is in `alephcore`, not `aleph_protocol`; the Panel does not deserialise it. If the Panel build breaks here, the row type IS on the wire and Contract deltas must say so.

**Mutation step:** in `slash_owner_admits` make the `Some(owner) => Err(..)` arm return `Ok(())` → `a_slash_command_owned_by_an_invisible_plugin_is_refused_before_dispatch` red (`expect_err` panics). Restore.

- [ ] **Step 5: Commit**

```bash
git add src/skill/compat.rs src/tool_metadata src/command/parser.rs src/gateway/inbound_router/command_handler.rs src/gateway/execution_engine/slash_command.rs src/gateway/execution_engine/tests.rs src/gateway/execution_engine/slash_skill_scope.rs src/gateway/handlers/commands.rs src/gateway/handlers/tools_visibility.rs src/bin/aleph-server/commands/start/builder/agent_init/tool_catalog_init.rs src/extension/slash_effect.rs src/extension/visibility.rs
git commit -m "slash: catalog rows carry their owning plugin; commands.list and the fast path gate on visibility

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.9: Face ⑤ — MCP tools of plugin-owned servers

**Files:**
- Modify: `src/extension/mcp_config.rs:180-182` (server-id codec: one encoder, one decoder)
- Modify: `src/gateway/execution_engine/run_loop/inner.rs:797-821` (the MCP join gates on the owner)
- Test: `src/extension/mcp_config.rs` (tests module), `src/gateway/execution_engine/run_loop/` (a unit test on the extracted predicate)

**Interfaces:**
- Consumes: `ExtensionManager::plugin_visible` (P2.4); `ToolHandler::definition()` (`tools/handlers/mod.rs:24`) whose `source` is `tools::service::ToolSource::Mcp { server_id }` (`service.rs:110-114`, produced by `McpHandler::definition`, `handlers/mcp.rs:158-168`).
- Produces:
  ```rust
  // src/extension/mcp_config.rs
  pub fn plugin_server_id(plugin_id: &str, server_name: &str) -> String;        // "plugin:{plugin_id}/{server_name}"
  pub fn owning_plugin_of_server_id(server_id: &str) -> Option<&str>;          // inverse; None for non-plugin servers
  // src/gateway/execution_engine/run_loop/inner.rs (private, tested)
  fn mcp_handler_admitted(handler: &dyn ToolHandler, visibility: &VisibilityCtx, ext: Option<&ExtensionManager>) -> bool;
  ```

**Cheapest attachment (lead's question):** none — the plugin id is already in the server id (`mcp_config.rs:181`), the bridge registers handlers whose `definition().source` carries that id verbatim (`handlers/mcp.rs:163-165`), and the request-time join at `inner.rs:797` already iterates handlers with two gates (`agent.is_tool_allowed`, `slash_skill_scope::admits`). Adding a third gate there needs no config metadata and no change to `src/mcp/`. The decoder is the encoder's inverse in the same file (judgment §12), with a round-trip test. Non-plugin servers (`~/.aleph/mcp.json`) have no `plugin:` prefix and pass.

- [ ] **Step 1: Write the failing tests**

`src/extension/mcp_config.rs` `mod tests`:

```rust
    #[test]
    fn plugin_server_id_round_trips_through_its_decoder() {
        let id = plugin_server_id("media-office", "office");
        assert_eq!(id, "plugin:media-office/office");
        assert_eq!(owning_plugin_of_server_id(&id), Some("media-office"));
        // A server name with a slash still decodes to the plugin id: the
        // plugin id itself cannot contain one (`validate_plugin_id`).
        assert_eq!(owning_plugin_of_server_id("plugin:x/a/b"), Some("x"));
        // Not plugin-owned.
        assert_eq!(owning_plugin_of_server_id("github"), None);
        assert_eq!(owning_plugin_of_server_id("plugin:"), None);
        assert_eq!(owning_plugin_of_server_id("plugin:nosl"), None);
    }

    /// `.mcp.json` parsing uses the encoder, not its own `format!`.
    #[test]
    fn parsed_server_ids_come_from_the_encoder() {
        let dir = tempfile::tempdir().unwrap();
        let configs = parse_mcp_json_content(
            r#"{"mcpServers":{"srv":{"command":"true"}}}"#,
            dir.path(),
            "plug",
            &serde_json::Value::Null,
        )
        .unwrap();
        assert!(configs.contains_key(&plugin_server_id("plug", "srv")));
    }
```

`src/gateway/execution_engine/run_loop/inner.rs` (or a new `#[cfg(test)] mod visibility_tests` at the file's foot — `inner.rs` is large; if it has no tests module, put this in `src/gateway/execution_engine/tests.rs` and make the predicate `pub(super)`):

```rust
    /// Face ⑤: a handler whose server is plugin-owned is admitted only when
    /// the owning plugin is visible; a non-plugin server passes; with no
    /// extension manager an owned server is refused (fail-closed).
    #[test]
    fn mcp_handler_admitted_gates_plugin_owned_servers_only() {
        use crate::tools::handlers::{ToolHandler, ToolOutput, ToolError};
        use crate::tools::service::{ToolDefinition, ToolSource};
        struct Fake(&'static str);
        #[async_trait::async_trait]
        impl ToolHandler for Fake {
            async fn invoke(&self, _: serde_json::Value) -> Result<ToolOutput, ToolError> { unreachable!() }
            fn definition(&self) -> ToolDefinition {
                ToolDefinition {
                    name: "t".into(), description: String::new(),
                    input_schema: serde_json::json!({}),
                    source: ToolSource::Mcp { server_id: self.0.into() },
                    metadata: Default::default(),
                }
            }
        }
        let nowhere = crate::extension::visibility::VisibilityCtx { project_root: None };
        assert!(mcp_handler_admitted(&Fake("github"), &nowhere, None), "non-plugin server passes");
        assert!(!mcp_handler_admitted(&Fake("plugin:proj/srv"), &nowhere, None), "owned + no manager ⇒ refused");
    }
```

plus, in `src/extension/mod.rs` tests (where a manager with a keyed row is cheap to build — reuse `plugin_tool_index_is_filtered_by_the_owning_plugins_visibility`'s seeding):

```rust
    #[tokio::test]
    async fn plugin_visible_decides_the_mcp_join_for_owned_servers() {
        use crate::extension::visibility::{canonical_root, ScopeKey, VisibilityCtx};
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let proj = tempfile::tempdir().unwrap();
        let manager = ExtensionManager::with_defaults().await.unwrap();
        {
            let mut registry = manager.get_plugin_registry_mut().await;
            let mut record = PluginRecord::new("proj".into(), "P".into(), PluginKind::Mcp, PluginOrigin::Workspace);
            record.scope_key = ScopeKey::project(proj.path());
            registry.register_plugin(record);
        }
        manager.sync_runtime_snapshots().await;
        let owner = crate::extension::mcp_config::owning_plugin_of_server_id("plugin:proj/srv").unwrap();
        assert!(manager.plugin_visible(owner, &VisibilityCtx { project_root: Some(canonical_root(proj.path())) }));
        assert!(!manager.plugin_visible(owner, &VisibilityCtx { project_root: None }));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib mcp_config::tests::plugin_server_id_round_trips -- --nocapture` → FAIL to compile (`cannot find function plugin_server_id`).

- [ ] **Step 3: Write minimal implementation**

`src/extension/mcp_config.rs` — above `parse_mcp_json_content`:

```rust
/// The transient server id of a plugin-declared MCP server. The plugin id is
/// embedded so every consumer that only has the server id (the tool bridge,
/// the request-time MCP join) can name the owner without a side
/// table. [`owning_plugin_of_server_id`] is the inverse; keep them together.
#[must_use]
pub fn plugin_server_id(plugin_id: &str, server_name: &str) -> String {
    format!("plugin:{plugin_id}/{server_name}")
}

/// Inverse of [`plugin_server_id`]. `None` for a server no plugin declared
/// (user `mcp.json` servers have no `plugin:` prefix). Splits at the FIRST
/// `/`: plugin ids are `[a-z0-9-]` (`manifest::validate_plugin_id`), so the
/// first slash is always the separator.
#[must_use]
pub fn owning_plugin_of_server_id(server_id: &str) -> Option<&str> {
    let rest = server_id.strip_prefix("plugin:")?;
    let (plugin_id, _server_name) = rest.split_once('/')?;
    (!plugin_id.is_empty()).then_some(plugin_id)
}
```

and `:181` (quote: `let server_id = format!("plugin:{plugin_id}/{server_name}");`) → `let server_id = plugin_server_id(plugin_id, &server_name);`.

`src/gateway/execution_engine/run_loop/inner.rs:797-821` — the join loop (quote of the two existing gates):

```rust
                for (name, handler) in mcp_registry.snapshot().iter() {
                    if !agent.is_tool_allowed(name) {
                        continue;
                    }
                    if !super::super::slash_skill_scope::admits(slash_skill_scope.as_ref(), name) {
                        continue;
                    }
```

gains a third gate directly after them:

```rust
                    // Face ⑤ of `extension::visibility`: a server a plugin
                    // declared joins only when that plugin is visible to this
                    // run. The server PROCESS stays global (one manager, one
                    // spawn); only its tools' presence on this run's surface is
                    // per-project. `visibility` is the value derived once near
                    // the top of this function for the plugin tool index.
                    if !mcp_handler_admitted(handler.as_ref(), &visibility, extension_manager.as_deref()) {
                        continue;
                    }
```

with the predicate at module level (`pub(super)` so `tests.rs` can reach it):

```rust
/// Face ⑤ gate. Non-plugin servers pass; a plugin-owned server passes iff
/// the extension manager can see its owner from `visibility`. No manager ⇒
/// an owned server is refused: the manager is the only authority on where a
/// plugin came from, and "I cannot tell" is not "yes".
pub(super) fn mcp_handler_admitted(
    handler: &dyn crate::tools::handlers::ToolHandler,
    visibility: &crate::extension::visibility::VisibilityCtx,
    ext: Option<&crate::extension::ExtensionManager>,
) -> bool {
    let def = handler.definition();
    let crate::tools::service::ToolSource::Mcp { server_id } = &def.source else {
        return true;
    };
    match crate::extension::mcp_config::owning_plugin_of_server_id(server_id) {
        None => true,
        Some(owner) => ext.is_some_and(|m| m.plugin_visible(owner, visibility)),
    }
}
```

(`extension_manager` is the `Option<Arc<ExtensionManager>>` parameter of `run_agent_loop_inner`; `visibility` is P2.5's binding. If `definition()` allocates noticeably — it clones the schema — read only `source` by adding a cheap `fn source(&self) -> ToolSource` default method to `ToolHandler`? **No**: that widens a trait 3 impls share for one caller; the join runs once per request over a few dozen handlers. Leave it.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib mcp_config -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib mcp_handler_admitted -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib plugin_visible_decides_the_mcp_join -- --nocapture` → PASS.

**Mutation step:** in `mcp_handler_admitted` change `Some(owner) => ext.is_some_and(..)` to `Some(_) => true` → `mcp_handler_admitted_gates_plugin_owned_servers_only` red. Restore.

- [ ] **Step 5: Commit**

```bash
git add src/extension/mcp_config.rs src/gateway/execution_engine/run_loop/inner.rs src/gateway/execution_engine/tests.rs src/extension/mod.rs
git commit -m "mcp: plugin server ids have one codec; the request-time MCP join gates on the owning plugin's visibility

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.10: Integration test `tests/plugin_visibility.rs`

**Files:**
- Create: `tests/plugin_visibility.rs`
- Test: itself (runs under `--features test-helpers`, like `tests/extension_watcher_integration.rs`)

**Interfaces:**
- Consumes (all `pub` in `alephcore`): `extension::{ExtensionConfig, ExtensionManager}`, `extension::visibility::{ScopeKey, VisibilityCtx, canonical_root}`, `discovery::DiscoveryConfig`, `projects::{ProjectStore, with_project_root}`, `agents::AgentRegistry`.
- Produces: nothing.

One `#[tokio::test]` per file, because the test sets `ALEPH_HOME` for its own process: the `cfg(test)` in-memory `ProjectStore` (`store.rs:251-257`) and the `cfg(test)` `extra_plugin_parents` seam are not compiled under `--features test-helpers`, so this test registers the project through the real store the daemon uses (`ProjectStore::shared().add`, `store.rs:1229`), which is exactly the producer `collect_plugin_dirs` reads (`mod.rs:1035`).

- [ ] **Step 1: Write the failing test**

```rust
//! A Project-scoped plugin is invisible to a session bound to no project and
//! visible to a session bound to that project — end to end, through the real
//! `ProjectStore` and the real discovery walk, on the two faces an integration
//! test can observe without a model: the registry row's key and sub-agent
//! resolution (face ③). Faces ①/②/④/⑤ are unit-tested at their chokepoints
//! and observed on a real daemon by `qa/plugins/run.sh visibility`.
//!
//! ONE test in this file on purpose: it pins `ALEPH_HOME` for the whole
//! process (the daemon's `ProjectStore` lives under it), and two tests in one
//! binary would race on that.

use std::path::PathBuf;

use alephcore::agents::AgentRegistry;
use alephcore::discovery::DiscoveryConfig;
use alephcore::extension::visibility::{canonical_root, ScopeKey, VisibilityCtx};
use alephcore::extension::{ExtensionConfig, ExtensionManager};

fn plant_project_plugin(project_root: &std::path::Path, id: &str) {
    let plugin = project_root.join(".aleph/plugins").join(id);
    std::fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
    std::fs::write(
        plugin.join(".claude-plugin/plugin.toml"),
        format!("name = \"{id}\"\nversion = \"1.0.0\"\n"),
    )
    .unwrap();
    std::fs::create_dir_all(plugin.join("agents")).unwrap();
    std::fs::write(
        plugin.join("agents").join(format!("{id}-agent.md")),
        format!("---\nname: {id}-agent\ndescription: scoped helper\n---\nYou help.\n"),
    )
    .unwrap();
}

#[tokio::test]
async fn a_project_plugin_is_invisible_without_the_project_and_visible_inside_it() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    // Point every HOME-derived path (plugins root, data dir → projects.db) at
    // the scratch home. Process-local; this is the only test in this binary.
    std::env::set_var("ALEPH_HOME", home.path());
    std::env::set_var("HOME", home.path());

    plant_project_plugin(project.path(), "vis-proj");
    alephcore::projects::ProjectStore::shared()
        .add(project.path(), Some("vis".into()))
        .expect("register the project the way the Panel picker does");

    let cfg = ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: home.path().to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
        },
        plugins_config_path: Some(home.path().join("plugins.toml")),
    };
    let manager = ExtensionManager::new(cfg).await.expect("manager");
    manager.load_all().await.expect("load_all");

    // 1. The row carries the project key, derived from the registered root.
    let key = manager
        .plugin_scope_key("vis-proj")
        .expect("the project plugin was discovered through the registered project");
    assert_eq!(key, ScopeKey::project(project.path()));

    // 2. The predicate every face uses.
    let inside = VisibilityCtx {
        project_root: Some(canonical_root(project.path())),
    };
    let nowhere = VisibilityCtx { project_root: None };
    assert!(manager.plugin_visible("vis-proj", &inside));
    assert!(!manager.plugin_visible("vis-proj", &nowhere));

    // 3. Face ③ end to end: the plugin's sub-agent resolves for a run bound to
    //    the project and not for a run bound to another directory.
    let registry = AgentRegistry::with_builtins();
    let elsewhere = tempfile::tempdir().unwrap();
    assert!(
        registry.resolve("vis-proj-agent", Some(project.path())).is_some(),
        "inside the project the plugin agent is delegatable"
    );
    assert!(
        registry.resolve("vis-proj-agent", Some(elsewhere.path())).is_none(),
        "from another project it does not exist"
    );
    let _keep: PathBuf = project.path().to_path_buf();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --features test-helpers --test plugin_visibility -- --nocapture`
Expected (against a tree with P2.1–P2.9 applied, the test should already PASS; run it BEFORE P2.7's commit to see it FAIL at assertion 3 with `from another project it does not exist`, which is the behaviour change this round makes). If run with only P2.1 applied it fails to compile on `plugin_scope_key`.

- [ ] **Step 3: Write minimal implementation**

None beyond P2.1–P2.9. If assertion 1 fails with `None`, the cause is one of: `ProjectStore::add` refusing the tempdir (check `workspace_path` on the returned row), or `collect_plugin_dirs` reading a different store than the test wrote (both go through `get_data_dir()` → `$ALEPH_HOME/data`; confirm with `ls "$ALEPH_HOME"/data/projects.db`). Do not add a second discovery seam to make it pass.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --features test-helpers --test plugin_visibility -- --nocapture` → PASS.
Run: `cargo test -p alephcore --features test-helpers --test '*' --no-run` → every integration test still compiles.

- [ ] **Step 5: Commit**

```bash
git add tests/plugin_visibility.rs
git commit -m "tests: a project plugin is invisible without its project and visible inside it

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P2.11: `qa/plugins/run.sh visibility` — the real daemon shows a project plugin only to that project's session

**Files:**
- Modify: `qa/plugins/run.sh:3-12` (usage header), `:142` (`case`), `:411-412` (the `unknown scenario` line lists the new stage)
- Create: `qa/plugins/drive_visibility.py`
- Modify: `qa/README.md:618-624` (one new line under the `plugins` block)
- Test: the stage itself (`./qa/plugins/run.sh visibility` on this worktree's binary)

**Interfaces:**
- Consumes: RPC `projects.add {path}` (`src/gateway/handlers/projects.rs:386-425`, loopback = operator, passes `require_directory_choice`); RPC `chat.send {message, session_key, project_root?}` (`server_init.rs:435`); the mock provider's `request_log` (`qa/busy_input/mock_anthropic.py`, usage line: `mock_anthropic.py [port] [probe_path] [plan_name] [tool_spec] [request_log]`).
- Produces: nothing.

**What it proves.** Two `chat.send`s against one daemon that has a registered project with a `.aleph/plugins/qa-vis/` plugin shipping one skill and one agent. The run bound to the project must produce a model request whose `system` names `qa-vis-skill` and `qa-vis-agent`; the run with no `project_root` must not. The daemon is started from `$QA_ROOT` (not the project), so the CWD fallback resolves to a directory with no plugins — the no-project run really is project-less. This is the only observation that reaches the model side: `plugins.list` shows every row regardless (the management face is not project-gated), so it cannot be the oracle.

- [ ] **Step 1: Write the driver (it fails until the stage exists)**

`qa/plugins/drive_visibility.py`:

```python
#!/usr/bin/env python3
"""Face ②/③ of plugin visibility on a real daemon.

  python3 drive_visibility.py <ws-url> <project_root> <request_log> register
      -> projects.add the folder (loopback = operator); prints the row.
  python3 drive_visibility.py <ws-url> <project_root> <request_log> probe
      -> two chat.send runs, one with project_root and one without, then
         reads what the MOCK PROVIDER saw in each run's system prompt.

The oracle is the mock's request_log, never the RPC reply: the claim is about
what the model was shown, and the only witness to that is the request body.
"""
import asyncio
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "browser_managed"))
from qa_rpc import Ledger  # noqa: E402

import websockets  # noqa: E402

WS, PROJECT, REQ_LOG, PHASE = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4]
L = Ledger()
SKILL, AGENT = "qa-vis-skill", "qa-vis-agent"


async def rpc(ws, method, params, rid):
    await ws.send(json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}))
    while True:
        msg = json.loads(await ws.recv())
        if msg.get("id") == rid:
            return msg


async def run_to_completion(ws, message, session_key, project_root, rid):
    payload = {"message": message, "session_key": session_key, "stream": True}
    if project_root is not None:
        payload["project_root"] = str(project_root)
    sent = await rpc(ws, "chat.send", payload, rid)
    if "error" in sent:
        return None, sent["error"]
    run_id = sent["result"]["run_id"]
    deadline = asyncio.get_event_loop().time() + 60
    while asyncio.get_event_loop().time() < deadline:
        try:
            raw = await asyncio.wait_for(ws.recv(), timeout=1.0)
        except asyncio.TimeoutError:
            continue
        msg = json.loads(raw)
        if msg.get("method") in ("stream.run_complete", "stream.run_error") \
                and (msg.get("params") or {}).get("run_id") == run_id:
            return run_id, msg["method"]
    return run_id, "no_terminal_frame"


def systems_seen(before):
    """Every `system` field the mock recorded after line `before`."""
    lines = REQ_LOG.read_text().splitlines()[before:] if REQ_LOG.exists() else []
    out = []
    for line in lines:
        body = json.loads(line)["body"]
        out.append(json.dumps(body.get("system", "")))
    return out


async def main():
    async with websockets.connect(WS) as ws:
        await rpc(ws, "connect", {"client_type": "cli"}, 0)
        if PHASE == "register":
            res = await rpc(ws, "projects.add", {"path": str(PROJECT), "name": "qa-vis"}, 1)
            L.check("projects.add registers the folder", "result" in res, json.dumps(res)[:200])
            wp = ((res.get("result") or {}).get("project") or {}).get("workspace_path")
            L.check("the row carries the workspace_path discovery reads",
                    wp is not None and Path(wp).resolve() == PROJECT.resolve(), f"workspace_path={wp!r}")
            return L.verdict()

        # probe
        n0 = len(REQ_LOG.read_text().splitlines()) if REQ_LOG.exists() else 0
        rid, outcome = await run_to_completion(ws, "hello from inside", "agent:main:qa-vis-in", PROJECT, 2)
        L.check("project-bound run reached a terminal frame", outcome == "stream.run_complete", outcome)
        inside = systems_seen(n0)
        L.check("project-bound run: the model saw the plugin skill",
                any(SKILL in s for s in inside), f"{len(inside)} request(s)")
        L.check("project-bound run: the model saw the plugin agent",
                any(AGENT in s for s in inside), f"{len(inside)} request(s)")

        n1 = len(REQ_LOG.read_text().splitlines())
        rid, outcome = await run_to_completion(ws, "hello from nowhere", "agent:main:qa-vis-out", None, 3)
        L.check("project-less run reached a terminal frame", outcome == "stream.run_complete", outcome)
        outside = systems_seen(n1)
        L.check("project-less run: at least one request was recorded", bool(outside))
        L.check("project-less run: the plugin skill is NOT in the prompt",
                not any(SKILL in s for s in outside), f"{len(outside)} request(s)")
        L.check("project-less run: the plugin agent is NOT in the prompt",
                not any(AGENT in s for s in outside), f"{len(outside)} request(s)")
        return L.verdict()


sys.exit(asyncio.run(main()))
```

- [ ] **Step 2: Run to verify it fails**

Run: `./qa/plugins/run.sh visibility`
Expected: `unknown scenario 'visibility' (manifest | scaffold | trust | browse | marketplaces | panel)` exit 2.

- [ ] **Step 3: Write the stage**

`qa/plugins/run.sh` — usage header (`:3-12`) gains:

```bash
#   ./qa/plugins/run.sh visibility # a project's .aleph/plugins/<p> plugin reaches the
#                                  # model only for a run bound to that project
```

`case` arm, before `*)` (`:411`):

```bash
visibility)
  # The claim is about what the MODEL is shown, so the oracle is the mock
  # provider's request log — `plugins.list` shows every row regardless (the
  # management face is not project-gated) and cannot tell the two runs apart.
  #
  # Two facts make the "no project" run genuinely project-less: the daemon is
  # started from $QA_ROOT (the CWD fallback then names a directory with no
  # `.aleph/plugins`), and the project is registered through `projects.add`,
  # which is the very producer `collect_plugin_dirs` reads — a plugin planted
  # anywhere else would prove discovery, not visibility.
  PROJECT="$QA_ROOT/proj-vis"
  REQ_LOG="$QA_ROOT/requests.jsonl"
  mkdir -p "$PROJECT/.aleph/plugins/qa-vis/.claude-plugin" \
           "$PROJECT/.aleph/plugins/qa-vis/skills/qa-vis-skill" \
           "$PROJECT/.aleph/plugins/qa-vis/agents"
  printf 'name = "qa-vis"\nversion = "1.0.0"\n' > "$PROJECT/.aleph/plugins/qa-vis/.claude-plugin/plugin.toml"
  printf -- '---\nname: qa-vis-skill\ndescription: a skill that exists only in this project\n---\nDo the project thing.\n' \
    > "$PROJECT/.aleph/plugins/qa-vis/skills/qa-vis-skill/SKILL.md"
  printf -- '---\nname: qa-vis-agent\ndescription: a helper that exists only in this project\n---\nYou help.\n' \
    > "$PROJECT/.aleph/plugins/qa-vis/agents/qa-vis-agent.md"

  say "start mock provider (single-shot plan, recording every request)"
  python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname single-shot "" "$REQ_LOG" \
    >"$QA_ROOT/mock.log" 2>&1 &
  MOCK_PID=$!
  sleep 1

  say "start server from a non-project directory and register the project"
  SERVER_CWD="$QA_ROOT"
  start_server || exit 1
  python3 "$HERE/drive_visibility.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$PROJECT" "$REQ_LOG" register || RC=$?

  say "restart so discovery walks the registered project's .aleph/plugins"
  stop_server
  start_server || exit 1

  say "probe: one run inside the project, one run with no project"
  python3 "$HERE/drive_visibility.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$PROJECT" "$REQ_LOG" probe || RC=$?
  kill "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null
  ;;
```

`start_server` (`:129-140`) runs `"$BIN" start` in the caller's CWD, and a subshell around it would lose `SERVER_PID`. So the two `( cd "$QA_ROOT" && start_server )` lines above are written as plain `start_server` calls, and `start_server` itself gains a CWD knob — add `SERVER_CWD="${SERVER_CWD:-$REPO}"` next to `GATEWAY_PORT` (`:47`) and change the spawn line at `:133` to:

```bash
  ( cd "$SERVER_CWD" && exec "$BIN" start ) >>"$QA_ROOT/server.log" 2>&1 &
```

The stage sets `SERVER_CWD="$QA_ROOT"` once, before its first `start_server`. The `*)` line becomes `(manifest | scaffold | trust | browse | marketplaces | panel | visibility)`.

`qa/README.md`, after the `trust` line (`:623-624`):

```
./qa/plugins/run.sh visibility   # a plugin under a registered project's .aleph/plugins
                                 # reaches the model's prompt (skill index + agent catalog)
                                 # only for a run bound to that project; the oracle is the
                                 # mock provider's request log, never plugins.list
```

- [ ] **Step 4: Run to verify it passes**

Run: `./qa/plugins/run.sh visibility` (this worktree; no `CARGO_TARGET_DIR`).
Expected: every `[PASS]` line from both phases; `verdict: rc=0`. Then the **mutation**: rebuild with `retain_visible_plugin_skills`'s filter forced to `true` (`SKIP_BUILD=0`) → `project-less run: the plugin skill is NOT in the prompt` prints `[FAIL]`. Restore and re-run to green.

- [ ] **Step 5: Commit**

```bash
git add qa/plugins/run.sh qa/plugins/drive_visibility.py qa/README.md
git commit -m "qa/plugins: visibility stage — a project plugin reaches the model only for a run bound to that project

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## P3 — terminal status, `Pending`, activation gate

### Task P3.1: `PluginStatus::Pending { waiting_on }` in; `Overridden` out — core, wire, clients, doc in one commit

**Files:**
- Modify: `src/extension/types/plugins.rs:174-208` (enum + `is_active` + `label`), `:452-461` (`inactive` stays; new `with_pending`)
- Modify: `shared/protocol/src/plugins.rs:137-183` (`PluginRuntimeStatus`), tests `:574-604`
- Modify: `src/gateway/handlers/plugins/types.rs:56-69` (`parse_status`)
- Modify: `src/extension/plugin_ops.rs:293-317` (`get_plugin_info` — `waiting_on` rendered into `error`/`status_detail`)
- Modify: `src/hub/reconcile.rs:52` (`enabled` = `is_active()`)
- Modify: `interfaces/webchat/src/platform/wide/views/settings/plugins.rs:612-616` (no code change needed — verified below; a comment line)
- Modify: `interfaces/cli/src/commands/plugins_cmd.rs:495` (doc comment names `overridden`)
- Modify (comments that name `Overridden`): `src/extension/projection.rs:74`, `src/extension/mod.rs:558-564`
- Modify: `docs/reference/PLUGIN_SYSTEM.md:149-165`
- Test: `src/extension/types/plugins.rs` (tests module), `shared/protocol/src/plugins.rs` (tests), `src/gateway/handlers/plugins/types.rs` (tests)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  ```rust
  // src/extension/types/plugins.rs
  pub enum PluginStatus {
      Loaded, Disabled,
      Error(String), Blocked(String),
      /// Mounted, but a declared dependency has not reached its terminal state. `waiting_on` names
      /// each one (`mcp:manager`, `mcp:plugin:<id>/<server>`); never empty.
      Pending { waiting_on: Vec<String> },
  }
  impl PluginStatus {
      pub const fn is_active(&self) -> bool;   // Loaded | Pending — the plugin's other capabilities are live
      pub const fn label(&self) -> &'static str;   // "pending" for Pending
  }
  impl PluginRecord { pub fn with_pending(self, waiting_on: Vec<String>) -> Self; }   // status + `error` detail
  // shared/protocol/src/plugins.rs
  pub enum PluginRuntimeStatus { Loaded, Disabled, Error, Blocked, Pending }   // Overridden removed
  ```

**`Overridden` census** (`rg -n 'Overridden' src interfaces shared qa crates docs/reference/PLUGIN_SYSTEM.md`, comment lines included so the doc side is not forgotten):
- `src/extension/types/plugins.rs:181` (variant), `:205` (label arm) — delete
- `shared/protocol/src/plugins.rs:154` (variant), `:169` (label arm), `:578`, `:597` (test lists) — delete
- `src/gateway/handlers/plugins/types.rs:65` (`"overridden" => …` arm) — delete
- `src/extension/projection.rs:74` (doc comment "`Disabled` / `Overridden` / `Error`") — reword
- `src/extension/mod.rs:561-562` (comment "and the `Overridden` status written for exactly this moment had zero producers") — reword to past tense: the variant is gone
- `interfaces/cli/src/commands/plugins_cmd.rs:495` (doc comment `"overridden" alone names a problem`) — reword to `"blocked"`
- `docs/reference/PLUGIN_SYSTEM.md:153` (table row) and `:159-165` (the block quote) — see below
- Zero producers: `rg -n 'PluginStatus::Overridden|PluginRuntimeStatus::Overridden' src interfaces shared` finds only the arms above.

**Wire compatibility.** `PluginStatus` itself never crosses a process boundary (`PluginRecord` is not persisted; `plugin_ops.rs:313` sends `label()` as a string). The wire enum is `PluginRuntimeStatus` (a `Copy` unit enum, lowercase strings); `parse_status` maps unknown labels to `Error` (`types.rs:67`), so an OLD Panel/CLI talking to a NEW server renders `pending` as `error` with the `waiting on …` detail — wrong severity, right text; the same crate change fixes both sides at once because they share `aleph_protocol` (judgment §10: one crate both sides depend on). `waiting_on` rides in the existing free-text `status_detail` (`PluginInfo.error` → `PluginRow.status_detail`), so no new wire key. The Panel's `status_note` (`settings/plugins.rs:612-616`) already renders `label: detail` for every non-`Loaded|Disabled` status, so `Pending` shows as `pending: waiting on mcp:…` with no Panel change; the CLI's `(s, Some(d)) => format!("{} ({d})", s.label())` (`plugins_cmd.rs:99-101`) likewise. Both are verified by building them (`cargo test -p aleph-panel --lib --no-run`, `cargo test -p aleph-cli --no-run`).

- [ ] **Step 1: Write the failing tests**

`src/extension/types/plugins.rs` `mod tests` (exists at the foot of the file):

```rust
    #[test]
    fn pending_is_active_and_labelled_and_carries_its_dependencies() {
        let s = PluginStatus::Pending {
            waiting_on: vec!["mcp:plugin:x/srv".into()],
        };
        assert!(s.is_active(), "a pending plugin's other capabilities are live");
        assert_eq!(s.label(), "pending");
        let rec = PluginRecord::new("x".into(), "X".into(), PluginKind::Mcp, PluginOrigin::Global)
            .with_pending(vec!["mcp:plugin:x/srv".into(), "mcp:plugin:x/aux".into()]);
        assert!(matches!(rec.status, PluginStatus::Pending { .. }));
        assert_eq!(
            rec.error.as_deref(),
            Some("waiting on mcp:plugin:x/aux, mcp:plugin:x/srv"),
            "the operator-facing detail names every dependency"
        );
    }

    /// `with_pending` refuses an empty list: "pending on nothing" is not a
    /// state, it is a predicate that can never go red (判据 §2).
    #[test]
    #[should_panic(expected = "waiting_on must name at least one dependency")]
    fn pending_on_nothing_is_rejected() {
        let _ = PluginRecord::new("x".into(), "X".into(), PluginKind::Mcp, PluginOrigin::Global)
            .with_pending(Vec::new());
    }
```

`shared/protocol/src/plugins.rs` tests — extend the two variant lists (`:575-581` and `:594-600`) with `PluginRuntimeStatus::Pending`, remove `Overridden` from both, and change `only_loaded_is_active` to:

```rust
    #[test]
    fn loaded_and_pending_are_active() {
        assert!(PluginRuntimeStatus::Loaded.is_active());
        assert!(PluginRuntimeStatus::Pending.is_active());
        for inactive in [
            PluginRuntimeStatus::Disabled,
            PluginRuntimeStatus::Error,
            PluginRuntimeStatus::Blocked,
        ] {
            assert!(!inactive.is_active(), "{inactive:?} must not be active");
        }
    }
```

`src/gateway/handlers/plugins/types.rs` `mod tests` (add if absent):

```rust
    /// Every core label maps onto a wire variant of the SAME name, and an
    /// unknown label still lands on `Error`, never `Loaded`.
    #[test]
    fn parse_status_covers_every_core_label() {
        use crate::extension::PluginStatus;
        let core = [
            PluginStatus::Loaded,
            PluginStatus::Disabled,
            PluginStatus::Error("e".into()),
            PluginStatus::Blocked("b".into()),
            PluginStatus::Pending { waiting_on: vec!["mcp:manager".into()] },
        ];
        for s in core {
            assert_eq!(parse_status(s.label()).label(), s.label(), "{s:?}");
        }
        assert_eq!(parse_status("overridden"), PluginRuntimeStatus::Error);
        assert_eq!(parse_status("garbage"), PluginRuntimeStatus::Error);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib pending_is_active -- --nocapture` → FAIL to compile (`no variant named Pending`).
Run: `cargo test -p aleph-protocol loaded_and_pending_are_active` → FAIL to compile.

- [ ] **Step 3: Write minimal implementation**

`src/extension/types/plugins.rs:174-208` (quote of today's enum body: `Loaded, Disabled, Overridden, Error(String), Blocked(String)`; `is_active` = `matches!(self, Self::Loaded)`; `label` arms incl. `Self::Overridden => "overridden"`) becomes:

```rust
/// Plugin status - the runtime state of a plugin
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginStatus {
    /// Plugin is loaded and active
    Loaded,
    /// Plugin is disabled by user
    Disabled,
    /// Plugin failed to load with an error
    Error(String),
    /// The owner trust policy refused this plugin's origin.
    ///
    /// Distinct from [`Self::Disabled`] on purpose: the remedy is an allowlist
    /// entry, not the per-plugin toggle. Collapsing the two would point the
    /// operator at a switch that cannot change the outcome.
    Blocked(String),
    /// Mounted, but a declared dependency has not reached its terminal state:
    /// the MCP manager is not attached yet, a declared MCP server has not
    /// answered `initialize`, a required runtime is not provisioned.
    ///
    /// `waiting_on` is derived from the plugin's declared MCP servers and
    /// their start reports (`extension::readiness::derive_readiness`), never
    /// hand-written, and is never empty. It changes only when a dependency
    /// reports — there is no timer that turns this into [`Self::Error`]
    /// (判据 §8: "not ready" is not "failed"). The boot activation gate and the
    /// `extension/plugins-activated` doctor check list every plugin still
    /// here after boot, with this field as the reason.
    Pending { waiting_on: Vec<String> },
}

impl PluginStatus {
    /// Whether the plugin's registered capabilities are live.
    ///
    /// `Pending` is active: the capabilities that do not depend on the
    /// outstanding dependency (skills, agents, hooks, commands) are already
    /// registered and usable; the ones that do (the pending server's tools)
    /// are simply not there yet. Making `Pending` inactive would unregister
    /// and re-register everything else around a slow `initialize`.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Loaded | Self::Pending { .. })
    }

    /// Stable lowercase label for client display / serialization — the wire
    /// vocabulary of [`aleph_protocol::plugins::PluginRuntimeStatus`].
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Disabled => "disabled",
            Self::Error(_) => "error",
            Self::Blocked(_) => "blocked",
            Self::Pending { .. } => "pending",
        }
    }
}
```

and after `inactive` (`:461`):

```rust
    /// Mark this record pending on the named dependencies (sorted, deduped).
    ///
    /// Not `inactive`: a pending plugin's other capabilities stay live (see
    /// [`PluginStatus::is_active`]). The detail is the operator's window onto
    /// WHAT is being waited for, so it lists every entry.
    #[must_use]
    pub fn with_pending(mut self, mut waiting_on: Vec<String>) -> Self {
        assert!(
            !waiting_on.is_empty(),
            "waiting_on must name at least one dependency"
        );
        waiting_on.sort();
        waiting_on.dedup();
        self.error = Some(format!("waiting on {}", waiting_on.join(", ")));
        self.status = PluginStatus::Pending { waiting_on };
        self
    }
```

`shared/protocol/src/plugins.rs:137-183`: delete the `Overridden` variant (`:151-154`) and its label arm (`:169`); add after `Blocked`:

```rust
    /// Mounted, but a declared dependency (MCP manager / server `initialize` /
    /// runtime provisioning) has not reached its terminal state.
    /// `status_detail` carries `waiting on …`. Active: the plugin's other
    /// capabilities are live.
    Pending,
```

with `Self::Pending => "pending"` in `label()` and `is_active` = `matches!(self, Self::Loaded | Self::Pending)`. Reword the doc comment at `:139-142` — it says "`Overridden` and `Error` existed as enum variants with zero producers"; that sentence stays as history but add: "`Overridden` was removed 2026-09 (still zero producers: a shadowed copy is recorded as a diagnostic on the winner, `extension/mod.rs`)."

`src/gateway/handlers/plugins/types.rs:61-69`:

```rust
fn parse_status(label: &str) -> PluginRuntimeStatus {
    match label {
        "loaded" => PluginRuntimeStatus::Loaded,
        "disabled" => PluginRuntimeStatus::Disabled,
        "blocked" => PluginRuntimeStatus::Blocked,
        "pending" => PluginRuntimeStatus::Pending,
        _ => PluginRuntimeStatus::Error,
    }
}
```

`src/hub/reconcile.rs:52`: `e.enabled = p.status.is_active();` (was `matches!(p.status, PluginStatus::Loaded)`; drop the now-unused `PluginStatus` import at `:11` if the compiler says so).

`src/extension/projection.rs:74`: "`Disabled` / `Overridden` / `Error`" → "`Disabled` / `Blocked` / `Error`". `src/extension/mod.rs:558-564` comment: replace "and the `Overridden` status written for exactly this moment had zero producers" with "(an `Overridden` status variant existed for this moment and had zero producers; it was removed in the 2026-09 scope round)".

`docs/reference/PLUGIN_SYSTEM.md:149-165` — delete the `overridden` row (`:153`), add:

```
| `pending` | 已 mount，但某个声明的依赖尚未到达终态（MCP manager 未接上 / server 未完成 `initialize` / 运行时未 provision）| `status_detail` 列出 `waiting on …`；`aleph doctor` 的 `extension/plugins-activated` 逐个点名 |
```

and rewrite the block quote at `:159-165` (quote of the false claim: "> 现在三者都有 registry 行 + `status_detail`。") to:

```
> **2026-08-16 之前只有前两个是真的。** `Overridden` / `Error` 是**零生产者**的枚举变体：
> 重名插件在 `load_all` 里被 `continue` 静默丢弃，manifest 解析失败只有一句 `debug!`，
> 两者都**不进 registry** ⇒ 在每一个面上「装了但坏了」与「从来没装过」逐字节相同。
>
> 2026-08-16 起 `error` / `blocked` 有了 registry 行 + `status_detail`。**`overridden` 从未有过生产者**：
> registry 按 id 键控，输家拿不到自己的行，`load_all` 只在赢家上记一条 `shadowed` 诊断
> （`extension/mod.rs`）——这一节曾写「三者都有 registry 行」，那句是假的；变体已于 2026-09 删除。
> 状态词表的单一源是 `aleph_protocol::plugins::PluginRuntimeStatus`。
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::types -- --nocapture` → PASS.
Run: `cargo test -p aleph-protocol plugins -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib gateway::handlers::plugins -- --nocapture` → PASS.
Run: `cargo test -p aleph-panel --lib --no-run && cargo test -p aleph-cli --no-run && cargo test -p aleph-tui --no-run` → compile (exhaustive matches on `PluginRuntimeStatus` in clients, if any, surface here).
Run: `rg -n 'Overridden|overridden' src interfaces shared qa crates docs/reference/PLUGIN_SYSTEM.md` → only the three history comments (`shared/protocol/src/plugins.rs:139-142`, `src/extension/mod.rs:558-564`, `PLUGIN_SYSTEM.md` block quote) and unrelated uses of the English word (`telegram/config.rs`, `generation/types`, `phone/shell.rs`, `composer.rs`, `qa_rpc.py`).

- [ ] **Step 5: Commit**

```bash
git add src/extension/types/plugins.rs shared/protocol/src/plugins.rs src/gateway/handlers/plugins/types.rs src/extension/plugin_ops.rs src/hub/reconcile.rs src/extension/projection.rs src/extension/mod.rs interfaces/cli/src/commands/plugins_cmd.rs docs/reference/PLUGIN_SYSTEM.md
git commit -m "extension: PluginStatus::Pending{waiting_on}; remove the producer-less Overridden on core, wire and doc

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P3.2: `readiness::derive_readiness` — `waiting_on` from the declared MCP servers' reports, not a boolean

**Files:**
- Create: `src/extension/readiness.rs`
- Modify: `src/extension/mod.rs:26-53` (`pub mod readiness;`)
- Test: `src/extension/readiness.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `PluginStatus` (P3.1).
- Produces:
  ```rust
  /// What one declared server's start request reported so far. The `oneshot` P1.4's
  /// `add_transient_server_detached` returns is the dependency's own report; nothing else is consulted.
  pub enum ServerStart { Started, Failed(String), Unanswered }
  pub struct ReadinessInputs<'a> {
      /// The `mcp_server` step ran (an MCP handle was attached). False = `scope.skipped()` contains "mcp_server".
      pub manager_attached: bool,
      /// One entry per server the mount enqueued: `(server_id, report)`. Empty for a plugin with no servers.
      pub servers: &'a [(String, ServerStart)],
  }
  pub fn derive_readiness(i: &ReadinessInputs<'_>) -> PluginStatus;
  ```

Rules, in order (each one is a row of the test below):
1. `!manager_attached` → `Pending { ["mcp:manager"] }` — the first declared dependency of every MCP plugin, and the only wait a daemon that never installed the MCP subsystem can be in (R3.2: written by `mount_parsed`'s skip branch, P3.3).
2. per server: `Unanswered` → wait `mcp:<server_id>`; `Failed(e)` → fail `mcp:<server_id>: <e>`; `Started` → reached.
3. any failure → `Error` (all reasons, sorted, joined); else any wait → `Pending` (sorted, deduped); else `Loaded`.

What is deliberately NOT here (R3.1): a `runtime:<name>` arm. `.mcp.json` never sets `requires_runtime` (`mcp_config.rs:148-230` builds `McpManagerConfig::stdio(..)` without it; only `mcp/presets` do), so the arm would have had zero producers (判据 §7). Nor a manager health poll: `Started` IS the actor's report that `start_server_internal` completed the handshake and `list_tools()` (`actor.rs:860-893`); a later crash is the tool bridge's concern (it unregisters the tools), not a readiness transition. A dropped sender (`Unanswered` after the watcher gives up on a receiver) is "I don't know", which 判据 §8 says may only be read as a wait, never as a failure.

The `ServerStart` match has **no wildcard arm** — a new report word is a compile error where a human decides wait / reached / fail.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(servers: &[(String, ServerStart)]) -> ReadinessInputs<'_> {
        ReadinessInputs {
            manager_attached: true,
            servers,
        }
    }

    #[test]
    fn no_manager_is_pending_on_the_manager_regardless_of_servers() {
        let servers = vec![("plugin:p/a".to_string(), ServerStart::Started)];
        let mut i = inputs(&servers);
        i.manager_attached = false;
        assert_eq!(
            derive_readiness(&i),
            PluginStatus::Pending { waiting_on: vec!["mcp:manager".into()] }
        );
    }

    #[test]
    fn every_report_word_is_classified() {
        let cases = [
            (ServerStart::Unanswered, Some("mcp:plugin:p/a")),
            (ServerStart::Started, None),
        ];
        for (report, expect_wait) in cases {
            let servers = vec![("plugin:p/a".to_string(), report)];
            let got = derive_readiness(&inputs(&servers));
            match expect_wait {
                Some(w) => assert_eq!(got, PluginStatus::Pending { waiting_on: vec![w.into()] }),
                None => assert_eq!(got, PluginStatus::Loaded),
            }
        }
        let servers = vec![("plugin:p/a".to_string(), ServerStart::Failed("spawn: ENOENT".into()))];
        match derive_readiness(&inputs(&servers)) {
            PluginStatus::Error(e) => assert!(e.contains("plugin:p/a") && e.contains("ENOENT"), "{e}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_failure_outranks_a_wait_and_names_only_the_failed_server() {
        let servers = vec![
            ("plugin:p/a".to_string(), ServerStart::Failed("ENOENT".into())),
            ("plugin:p/b".to_string(), ServerStart::Unanswered),
            ("plugin:p/c".to_string(), ServerStart::Started),
        ];
        match derive_readiness(&inputs(&servers)) {
            PluginStatus::Error(e) => {
                assert!(e.contains("plugin:p/a"), "{e}");
                assert!(!e.contains("plugin:p/b"), "a wait is not a failure: {e}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn waiting_on_is_sorted_and_deduplicated() {
        let servers = vec![
            ("plugin:p/b".to_string(), ServerStart::Unanswered),
            ("plugin:p/a".to_string(), ServerStart::Unanswered),
            ("plugin:p/b".to_string(), ServerStart::Unanswered),
        ];
        assert_eq!(
            derive_readiness(&inputs(&servers)),
            PluginStatus::Pending { waiting_on: vec!["mcp:plugin:p/a".into(), "mcp:plugin:p/b".into()] }
        );
    }

    #[test]
    fn no_servers_and_a_manager_is_loaded() {
        assert_eq!(derive_readiness(&inputs(&[])), PluginStatus::Loaded);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib extension::readiness -- --nocapture` → FAIL to compile (module missing).

- [ ] **Step 3: Write minimal implementation**

`src/extension/readiness.rs`:

```rust
//! Readiness: which terminal state a mounted plugin has reached, derived from
//! what its declared dependencies reported.
//!
//! dsh's `assertEntriesActivated` (evidence `scan-dsh-cordis.md` §5.1) is the
//! model: a unit that is neither ACTIVE nor FAILED must be able to NAME what
//! it is waiting for, and that name must come from a declaration, not a
//! boolean somebody set. Here the declarations are the plugin's `.mcp.json`
//! servers; each one's dependency is the MCP manager (attached or not) and
//! the actor's answer to its start request (`add_transient_server_detached`'s
//! receiver, P1.4). The answer is the report; nothing is polled.
//!
//! This module is pure: every input is a value, so the rules are tested
//! without a manager, a process or a clock. The two writers of the result —
//! `lifecycle.rs::mount_parsed` (the no-handle case) and
//! `lifecycle.rs::watch_server_starts` (before and after the receivers settle)
//! — both go through [`derive_readiness`] and `ExtensionManager::write_readiness`;
//! nothing else assigns `PluginStatus::Pending`.

use crate::extension::types::PluginStatus;

/// What one declared server's start request reported so far.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerStart {
    /// The actor answered `Ok`: handshake done, tools listed.
    Started,
    /// The actor answered `Err`: spawn / handshake failed, with its reason.
    Failed(String),
    /// No answer yet, or the sender was dropped. "I don't know" — a wait,
    /// never a failure (判据 §8).
    Unanswered,
}

/// Everything the derivation needs, as values.
pub struct ReadinessInputs<'a> {
    /// The `mcp_server` step ran. `false` when `mount` recorded
    /// `scope.skip("mcp_server", …)` because no MCP handle was attached.
    pub manager_attached: bool,
    /// `(server_id, report)` for every server the mount enqueued.
    pub servers: &'a [(String, ServerStart)],
}

/// The terminal (or not) state of one MCP-kind plugin. See the module doc
/// for the rules; each is a row of the tests below.
#[must_use]
pub fn derive_readiness(i: &ReadinessInputs<'_>) -> PluginStatus {
    if !i.manager_attached {
        return PluginStatus::Pending {
            waiting_on: vec!["mcp:manager".into()],
        };
    }
    let mut waiting: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for (server_id, report) in i.servers {
        // Exhaustive on purpose: a new report word must be placed here.
        match report {
            ServerStart::Started => {}
            ServerStart::Unanswered => waiting.push(format!("mcp:{server_id}")),
            ServerStart::Failed(e) => failed.push(format!("mcp:{server_id}: {e}")),
        }
    }
    if !failed.is_empty() {
        failed.sort();
        return PluginStatus::Error(failed.join("; "));
    }
    if !waiting.is_empty() {
        waiting.sort();
        waiting.dedup();
        return PluginStatus::Pending {
            waiting_on: waiting,
        };
    }
    PluginStatus::Loaded
}
```

`src/extension/mod.rs`, next to `mod projection;` (`:45`): `pub mod readiness;`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib extension::readiness -- --nocapture` → PASS (5 tests).

**Mutation step (census on the report vocabulary):** add a variant `Restarting` to `ServerStart` → `cargo check -p alephcore` fails with `non-exhaustive patterns: ServerStart::Restarting not covered` at `derive_readiness`. Revert.

- [ ] **Step 5: Commit**

```bash
git add src/extension/readiness.rs src/extension/mod.rs
git commit -m "extension/readiness: derive Pending/Loaded/Error from the declared MCP servers' start reports

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P3.3: `write_readiness` — the one writer of `Pending`; `mount_parsed` (no-handle) and `watch_server_starts` (receivers) feed it

> Anchors: plan-P1 P1.9 `lifecycle.rs::mount_parsed` (step 3 `mcp_server`), `watch_server_starts(&self, plugin_id: &str, receivers: Vec<(String, ServerStartReceiver)>)` and `activation_settled()` (drains `activation_watchers: StdMutex<Vec<tokio::task::JoinHandle<()>>>`, re-checking for watchers spawned meanwhile; no timer). `ServerStartReceiver = oneshot::Receiver<Result<(), String>>` (P1.4, `mcp_registrar.rs`). 3ddc1f2e7's `sync_mcp_plugin_servers` (`mod.rs:462-524`) and the boot catch-up task (`start/mod.rs:1443-1461`) no longer exist after P1.4/P1.10 and are not touched here.

**Files:**
- Modify: `src/extension/lifecycle.rs` (plan-P1 P1.9): `mount_parsed` step 3 + the `scope.skipped()` block + its return value; `watch_server_starts` — becomes `async`, writes moment 2 before `tokio::spawn`, the spawned task collects reports and writes moment 3; the `activation_watchers` push is unchanged
- Modify: `src/extension/readiness.rs` (P3.2): add `write_readiness` next to `derive_readiness`
- Test: `src/extension/lifecycle.rs` (tests module), `src/extension/readiness.rs` (tests)

**Interfaces:**
- Consumes: `readiness::{derive_readiness, ReadinessInputs, ServerStart}` (P3.2), `PluginRecord::with_pending` (P3.1), P1's `EffectScope::skipped()`, P1.4's receivers.
- Produces:
  ```rust
  // src/extension/readiness.rs
  /// Write `status` onto the row (status + operator detail). The ONLY assignment of `PluginStatus::Pending`;
  /// a free fn over the registry handle so the spawned watcher can call it with the `Arc` it captured.
  pub async fn write_readiness(registry: &Arc<RwLock<PluginRegistry>>, plugin_id: &str, status: PluginStatus);
  // src/extension/lifecycle.rs — SIGNATURE CHANGE to P1.9's fn (option (a)): sync → async.
  //   P1.9:  fn watch_server_starts(&self, plugin_id: &str, receivers: Vec<(String, ServerStartReceiver)>)
  //   P3.3:  async fn watch_server_starts(&self, plugin_id: &str, receivers: Vec<(String, ServerStartReceiver)>)
  // Its ONE call site (P1.9 `mount_parsed` step 3, `self.watch_server_starts(&id, receivers);`) gains `.await`.
  // `activation_settled()` and the `activation_watchers: StdMutex<Vec<JoinHandle<()>>>` push are untouched.
  ```

Three moments, one derivation (`derive_readiness`), one writer (`write_readiness`), no manager method in between — the watcher holds the registry `Arc` (`self.plugin_registry` is `Arc<RwLock<PluginRegistry>>`, `mod.rs:161`), so no accessor is needed:

| moment | where | inputs | result |
|---|---|---|---|
| no MCP handle | `mount_parsed`, after the `scope.skipped()` diagnostics | `manager_attached: false` | `Pending { ["mcp:manager"] }` |
| servers enqueued | `watch_server_starts`, before `tokio::spawn` (awaited — the fn becomes `async`) | every server `Unanswered` | `Pending { ["mcp:<id>", …] }` |
| receivers settled | end of the spawned task | each server `Started` / `Failed(e)` / `Unanswered` (sender dropped) | `Loaded` / `Error` / `Pending` |

`mount_parsed` today returns `Ok(PluginStatus::Loaded)` unconditionally (P1.9); after this task it returns the row's status, so `set_plugin_enabled(true)` and `plugins.enable` report `pending` when that is the truth.

- [ ] **Step 1: Write the failing tests**

`src/extension/lifecycle.rs` `mod tests` (P1.9's module; `isolated_manager` / `write_project_plugin` are its helpers):

```rust
    fn write_mcp_project_plugin(root: &std::path::Path, id: &str) {
        let plugin_dir = root.join("plugins").join(id);
        std::fs::create_dir_all(plugin_dir.join(".claude-plugin")).unwrap();
        // `[aleph] runtime = "mcp"` → PluginKind::Mcp (`cc_plugin_toml.rs:181-188`);
        // `.mcp.json` → one McpServer capability (`component_source.rs:127-134`).
        std::fs::write(
            plugin_dir.join(".claude-plugin/plugin.toml"),
            format!("name = \"{id}\"\nversion = \"1.0.0\"\n[aleph]\nruntime = \"mcp\"\n"),
        )
        .unwrap();
        std::fs::write(
            plugin_dir.join(".mcp.json"),
            r#"{"mcpServers":{"srv":{"command":"qa-nonexistent-mcp-binary-9f3a"}}}"#,
        )
        .unwrap();
    }

    /// No MCP handle attached (CLI paths, this test): the `mcp_server` step is
    /// a recorded skip (P1), and the row says so — `Pending` on the manager,
    /// not `Loaded`. Before this round the row said `loaded` for a plugin
    /// whose servers had never been spawned (evidence `scan-aleph-plugins.md`
    /// §2.5 row "MCP servers").
    #[tokio::test]
    async fn mount_without_an_mcp_handle_is_pending_on_the_manager() {
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-mcp");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        let rec = manager.get_plugin_record("p3-mcp").await.expect("registered");
        assert_eq!(rec.kind, PluginKind::Mcp, "the [aleph] runtime override must have applied");
        assert_eq!(rec.status, PluginStatus::Pending { waiting_on: vec!["mcp:manager".into()] });
        assert_eq!(rec.error.as_deref(), Some("waiting on mcp:manager"));
        assert!(rec.status.is_active(), "its skills/agents/hooks are live meanwhile");
        assert_eq!(manager.scope_skipped("p3-mcp").unwrap()[0].0, "mcp_server");
    }

    /// A static plugin has no dependency to wait on: `Loaded` after mount.
    #[tokio::test]
    async fn mount_of_a_static_plugin_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        write_project_plugin(dir.path(), "p3-static");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        manager.load_all().await.unwrap();
        assert_eq!(manager.get_plugin_record("p3-static").await.unwrap().status, PluginStatus::Loaded);
    }

    /// Moment 2, observed deterministically: the actor exists (its command
    /// channel accepts the enqueue — capacity 32, `actor.rs:113`) but is never
    /// run, so no receiver can be answered. The row is `Pending` on the
    /// server. Then the actor is dropped: every receiver resolves `Err`
    /// (sender gone) — "I don't know", which stays a wait, never a failure
    /// (判据 §8) — and `activation_settled` completes because the watcher has
    /// nothing left to await.
    #[tokio::test]
    async fn mount_with_a_silent_manager_is_pending_on_the_server_and_a_dropped_answer_stays_a_wait() {
        use crate::mcp::manager::McpManagerActor;
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-quiet");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        let (actor, handle) = McpManagerActor::new(Some(dir.path().join("mcp.json"))).await.unwrap();
        manager.set_mcp_handle(handle);

        manager.load_all().await.unwrap();
        let want = PluginStatus::Pending { waiting_on: vec!["mcp:plugin:p3-quiet/srv".into()] };
        assert_eq!(manager.get_plugin_record("p3-quiet").await.unwrap().status, want, "enqueued, unanswered");

        drop(actor);
        manager.activation_settled().await;
        assert_eq!(
            manager.get_plugin_record("p3-quiet").await.unwrap().status,
            want,
            "a dropped sender is not a report; the plugin is still waiting on the server"
        );
    }

    /// Moment 3 with a running actor: the binary does not exist, so the
    /// actor answers `Err` and the watcher writes `Error` naming the server.
    /// `activation_settled` is how the test — and the boot gate — waits for
    /// that without a timer.
    #[tokio::test]
    async fn mount_with_a_running_manager_ends_terminal_when_the_actor_answers() {
        use crate::mcp::manager::McpManagerActor;
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-live");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        let (actor, handle) = McpManagerActor::new(Some(dir.path().join("mcp.json"))).await.unwrap();
        tokio::spawn(actor.run());
        manager.set_mcp_handle(handle);

        manager.load_all().await.unwrap();
        manager.activation_settled().await;
        let end = manager.get_plugin_record("p3-live").await.unwrap();
        match end.status {
            PluginStatus::Error(ref e) => assert!(e.contains("plugin:p3-live/srv"), "{e}"),
            other => panic!("a nonexistent binary must end in Error, got {other:?}"),
        }
        assert_eq!(end.error.as_deref().map(|e| e.contains("p3-live/srv")), Some(true));
    }
```

`src/extension/readiness.rs` `mod tests`:

```rust
    #[tokio::test]
    async fn write_readiness_sets_status_and_the_operator_detail_together() {
        use crate::extension::{PluginKind, PluginOrigin, PluginRecord, PluginRegistry};
        use crate::sync_primitives::Arc;
        let registry = Arc::new(tokio::sync::RwLock::new(PluginRegistry::new()));
        registry.write().await.register_plugin(PluginRecord::new(
            "w".into(), "W".into(), PluginKind::Mcp, PluginOrigin::Global,
        ));
        write_readiness(&registry, "w", PluginStatus::Pending { waiting_on: vec!["mcp:plugin:w/a".into()] }).await;
        let row = registry.read().await.get_plugin("w").unwrap().clone();
        assert_eq!(row.error.as_deref(), Some("waiting on mcp:plugin:w/a"));
        write_readiness(&registry, "w", PluginStatus::Loaded).await;
        let row = registry.read().await.get_plugin("w").unwrap().clone();
        assert_eq!(row.status, PluginStatus::Loaded);
        assert_eq!(row.error, None, "a reached dependency leaves no stale detail");
        write_readiness(&registry, "w", PluginStatus::Error("mcp:plugin:w/a: ENOENT".into())).await;
        let row = registry.read().await.get_plugin("w").unwrap().clone();
        assert_eq!(row.error.as_deref(), Some("mcp:plugin:w/a: ENOENT"));
        // A row the operator switched off is never touched by readiness.
        registry.write().await.get_plugin_mut("w").unwrap().status = PluginStatus::Disabled;
        write_readiness(&registry, "w", PluginStatus::Loaded).await;
        assert_eq!(registry.read().await.get_plugin("w").unwrap().status, PluginStatus::Disabled);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib mount_without_an_mcp_handle_is_pending -- --nocapture`
Expected: FAIL — `left: Loaded, right: Pending { … }` (P1's `mount_parsed` writes `Loaded` regardless of the skip). `write_readiness_sets_status…` fails to compile (fn missing).

- [ ] **Step 3: Write minimal implementation**

`src/extension/readiness.rs` — append:

```rust
/// Write a derived status onto the row, together with the operator-facing
/// detail (`PluginRecord.error` → `PluginRow.status_detail`). The row's own
/// `Disabled` / `Blocked` outrank readiness: the operator's and the policy's
/// answers are not dependency reports and are left alone.
///
/// A free function over the registry handle — not a method — because the
/// task `lifecycle.rs::watch_server_starts` spawns has captured that
/// `Arc`, not the manager; `mount_parsed` calls it with `&self.plugin_registry`.
pub async fn write_readiness(
    registry: &crate::sync_primitives::Arc<tokio::sync::RwLock<crate::extension::PluginRegistry>>,
    plugin_id: &str,
    status: PluginStatus,
) {
    let mut reg = registry.write().await;
    let Some(row) = reg.get_plugin_mut(plugin_id) else {
        return;
    };
    // Exhaustive on the CURRENT status: the operator's / policy's answer
    // stands (G6's census reaches here too — a new variant must say whether
    // readiness may overwrite it).
    match row.status {
        PluginStatus::Disabled | PluginStatus::Blocked(_) => return,
        PluginStatus::Loaded | PluginStatus::Error(_) | PluginStatus::Pending { .. } => {}
    }
    if row.status != status {
        tracing::info!(plugin = %plugin_id, from = %row.status.label(), to = %status.label(), "plugin readiness");
    }
    match &status {
        PluginStatus::Pending { waiting_on } => {
            row.error = Some(format!("waiting on {}", waiting_on.join(", ")));
        }
        PluginStatus::Error(e) => row.error = Some(e.clone()),
        PluginStatus::Loaded => row.error = None,
        PluginStatus::Disabled | PluginStatus::Blocked(_) => {}
    }
    row.status = status;
}

`src/extension/lifecycle.rs` — P1.9's `mount_parsed`, step 3 (quote of P1's block):

```rust
                let handle = self.mcp_handle.read().unwrap_or_else(|e| e.into_inner()).clone();
                match handle {
                    None => scope.skip("mcp_server", "MCP manager not attached"),
                    Some(h) => match registrar::mcp_registrar::register_transient_servers(h, configs).await {
                        Ok(d) => scope.effect("mcp_server", d),
                        Err(e) => return Err(self.fail_mount(scope, shell, "mcp_server", e).await),
                    },
                }
```

becomes (R1.1 shape: the producer returns the receivers too):

```rust
                let handle = self.mcp_handle.read().unwrap_or_else(|e| e.into_inner()).clone();
                match handle {
                    None => scope.skip("mcp_server", "MCP manager not attached"),
                    Some(h) => match registrar::mcp_registrar::register_transient_servers(h, configs).await {
                        Ok((d, receivers)) => {
                            scope.effect("mcp_server", d);
                            // Writes `Pending { mcp:<id>… }` now and the terminal
                            // status when the actor has answered every start.
                            self.watch_server_starts(&id, receivers).await;
                        }
                        Err(e) => return Err(self.fail_mount(scope, shell, "mcp_server", e).await),
                    },
                }
```

and the tail of `mount_parsed` (quote of P1's: the `if !scope.skipped().is_empty() { … }` diagnostics block, then `self.scopes.lock()….insert(id.clone(), scope); tracing::info!(…"plugin mounted"); Ok(PluginStatus::Loaded)`) becomes:

```rust
        if !scope.skipped().is_empty() {
            let mut reg = self.plugin_registry.write().await;
            for (step, why) in scope.skipped() {
                tracing::warn!(plugin_id = %id, step, why, "effect skipped: handle not attached");
                reg.add_diagnostic(PluginDiagnostic {
                    level: DiagnosticLevel::Warn,
                    message: format!("{step} skipped: {why}"),
                    plugin_id: Some(id.clone()),
                    source: Some("mount".to_string()),
                });
            }
        }
        // The no-handle moment of readiness: an MCP plugin whose `mcp_server`
        // step could not run is pending on the manager, not loaded.
        if scope.skipped().iter().any(|(step, _)| *step == "mcp_server") {
            let status = readiness::derive_readiness(&readiness::ReadinessInputs {
                manager_attached: false,
                servers: &[],
            });
            readiness::write_readiness(&self.plugin_registry, &id, status).await;
        }

        self.scopes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), scope);
        let status = self
            .plugin_registry
            .read()
            .await
            .get_plugin(&id)
            .map_or(PluginStatus::Loaded, |r| r.status.clone());
        tracing::info!(plugin_id = %id, kind = ?kind, status = %status.label(), "plugin mounted");
        Ok(status)
```

`src/extension/lifecycle.rs` — P1.9's `watch_server_starts` (quote):

```rust
    fn watch_server_starts(&self, plugin_id: &str, receivers: Vec<(String, ServerStartReceiver)>) {
        if receivers.is_empty() {
            return;
        }
        let plugin_id = plugin_id.to_string();
        let handle = tokio::spawn(async move {
            for (server_id, rx) in receivers {
                match rx.await {
                    Ok(Ok(())) => {
                        tracing::info!(plugin_id = %plugin_id, server_id = %server_id, "plugin MCP server registered (transient)");
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, error = %e, "plugin MCP server failed to start");
                    }
                    Err(_) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, "MCP manager dropped the start request");
                    }
                }
            }
        });
        let mut watchers = self
            .activation_watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Finished tasks are dropped opportunistically so a long-lived daemon
        // that nobody ever `activation_settled()`s does not grow the vector.
        watchers.retain(|h| !h.is_finished());
        watchers.push(handle);
    }
```

becomes (`async`; its only caller, `mount_parsed`, is async and now awaits it — quoted above; the three log lines and the `activation_watchers` bookkeeping are byte-identical):

```rust
    /// One task per plugin that awaits every receiver the `mcp_server` step
    /// handed back, logs each outcome, and writes the plugin's readiness
    /// twice: `Pending { mcp:<id>… }` before the task exists (so no caller of
    /// `mount` can observe `Loaded` for servers still starting) and the
    /// terminal status once every receiver has answered. The handle is kept
    /// so [`Self::activation_settled`] can wait for it.
    async fn watch_server_starts(
        &self,
        plugin_id: &str,
        receivers: Vec<(String, ServerStartReceiver)>,
    ) {
        if receivers.is_empty() {
            return;
        }
        // Moment 2: enqueued, unanswered.
        let unanswered: Vec<(String, readiness::ServerStart)> = receivers
            .iter()
            .map(|(id, _)| (id.clone(), readiness::ServerStart::Unanswered))
            .collect();
        readiness::write_readiness(
            &self.plugin_registry,
            plugin_id,
            readiness::derive_readiness(&readiness::ReadinessInputs {
                manager_attached: true,
                servers: &unanswered,
            }),
        )
        .await;

        let registry = Arc::clone(&self.plugin_registry);
        let plugin_id = plugin_id.to_string();
        let handle = tokio::spawn(async move {
            let mut servers: Vec<(String, readiness::ServerStart)> = Vec::with_capacity(receivers.len());
            for (server_id, rx) in receivers {
                let report = match rx.await {
                    Ok(Ok(())) => {
                        tracing::info!(plugin_id = %plugin_id, server_id = %server_id, "plugin MCP server registered (transient)");
                        readiness::ServerStart::Started
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, error = %e, "plugin MCP server failed to start");
                        readiness::ServerStart::Failed(e)
                    }
                    Err(_) => {
                        tracing::warn!(plugin_id = %plugin_id, server_id = %server_id, "MCP manager dropped the start request");
                        readiness::ServerStart::Unanswered
                    }
                };
                servers.push((server_id, report));
            }
            // Moment 3: every receiver settled.
            readiness::write_readiness(
                &registry,
                &plugin_id,
                readiness::derive_readiness(&readiness::ReadinessInputs {
                    manager_attached: true,
                    servers: &servers,
                }),
            )
            .await;
        });
        let mut watchers = self
            .activation_watchers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Finished tasks are dropped opportunistically so a long-lived daemon
        // that nobody ever `activation_settled()`s does not grow the vector.
        watchers.retain(|h| !h.is_finished());
        watchers.push(handle);
    }
```

`activation_settled()` is untouched: it drains `activation_watchers` and awaits each `JoinHandle`, looping until none were added meanwhile — which is exactly why a test (or the boot gate) that awaits it observes moment 3.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::lifecycle::tests::mount_without_an_mcp_handle -- --nocapture`, `… mount_of_a_static_plugin_is_loaded`, `… mount_with_a_silent_manager_is_pending_on_the_server_and_a_dropped_answer_stays_a_wait`, `… mount_with_a_running_manager_ends_terminal_when_the_actor_answers` → PASS.
Run: `cargo test -p alephcore --lib extension::readiness -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib extension:: -- --nocapture` → every P1 lifecycle test still green. **Watch for**: P1 tests asserting `mount(..) == Ok(PluginStatus::Loaded)` on an MCP fixture with no handle (`load_all_mounts_enabled_plugins_and_records_their_scope`, P1.9) — such an assertion now reads `Pending { ["mcp:manager"] }`, which is the truth this task establishes; update the expectation there, not the derivation. `tests/plugin_lifecycle_roundtrip.rs` (P1.15) drives a real actor and must end `Loaded` after `activation_settled()`.
Run: `cargo test -p alephcore --features test-helpers --test '*' --no-run`.

**Mutation step:** delete the `if scope.skipped().iter().any(..) { … write_readiness … }` block in `mount_parsed` → `mount_without_an_mcp_handle_is_pending_on_the_manager` red (`left: Loaded`). Restore. Second: delete the moment-3 `write_readiness` in the watcher → `mount_with_a_running_manager_ends_terminal_when_the_actor_answers` red at `a nonexistent binary must end in Error, got Pending {..}`. Restore. Third: in `watch_server_starts` map `Err(_)` (sender dropped) to `ServerStart::Failed(..)` → `…a_dropped_answer_stays_a_wait` red (`Error` where `Pending` was expected). Restore.

- [ ] **Step 5: Commit**

```bash
git add src/extension/lifecycle.rs src/extension/readiness.rs
git commit -m "extension: readiness is written at the three moments a mount's MCP servers report; mount returns the row's status

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P3.4: `activation_gate::assess` + boot call + `ALEPH_ACTIVATION_GATE` + doctor `extension/plugins-activated`

**Files:**
- Create: `src/extension/activation_gate.rs`
- Modify: `src/extension/mod.rs:26-53` (`pub mod activation_gate;`)
- Modify: `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:596-602` (the `ensure_loaded()` block — after P1.8 the first `load_all` is handle-complete; the 3ddc1f2e7 catch-up task at `start/mod.rs:1441-1461` is gone after P1.10)
- Modify: `docs/reference/FEATURE_LOCATOR.md:2975` (§5.9 Doctor — the daemon-side check enumeration gains `extension/plugins-activated`; R3.4 / plan-P5P8 doc-code 同笔 matrix)
- Create: `src/diagnostics/checks/plugins_activated.rs`
- Modify: `src/diagnostics/checks/mod.rs:16-54` (mod + re-export), `src/diagnostics/mod.rs:257-266` (builder), `src/builtin_tools/doctor.rs:106-124` and `src/gateway/handlers/diagnostics.rs:77-89` (the two daemon faces register it), `src/builtin_tools/doctor.rs:283-299` (identity guard test extended)
- Test: `src/extension/activation_gate.rs`, `src/diagnostics/checks/plugins_activated.rs`, `src/builtin_tools/doctor.rs` (tests)

**Interfaces:**
- Consumes: `PluginRegistry::list_plugins` (`plugin_registry/mod.rs:96`), `PluginStatus::is_terminal` (**defined in this task**, census-guarded in P3.5), `ExtensionManager::activation_settled()` (P1, R1.1 — completes when every `watch_server_starts` task spawned so far has finished; no timer), `unknown_finding` / `Finding` / `HealthCheck` (`diagnostics/check.rs:214`, `finding.rs`), `try_extension_manager` (`manager_global.rs:69`).
- Produces:
  ```rust
  // src/extension/activation_gate.rs
  pub struct ActivationReport { pub non_terminal: Vec<(String /*plugin id*/, Vec<String> /*waiting_on*/)> }
  impl ActivationReport { pub fn is_clean(&self) -> bool; pub fn render_lines(&self) -> Vec<String>; }
  pub fn assess(registry: &PluginRegistry) -> ActivationReport;
  /// `ALEPH_ACTIVATION_GATE`: "log" (default) | "fatal". Parsed once at the boot call site.
  pub enum GatePosture { Log, Fatal }
  pub fn posture_from_env(value: Option<&str>) -> GatePosture;
  // src/extension/types/plugins.rs
  impl PluginStatus { pub const fn is_terminal(&self) -> bool; }   // exhaustive match, no wildcard
  ```
  Doctor check id: `extension/plugins-activated`.

**Boot call site (R3.3):** after P1.8 every handle `mount` needs is installed before the first `load_all`, which runs inside `ensure_loaded()` at `agent_init/mod.rs:600` (quote: `if let Err(e) = ext_manager.ensure_loaded().await { tracing::warn!("Failed to load extensions for plugin tools: {}", e); }`). Every MCP server is enqueued by then and its answer arrives on the receivers `watch_server_starts` awaits (P3.3), so the moment every dependency has reported is `manager.activation_settled().await` — awaited in a spawned task right after `ensure_loaded()` returns, so boot never blocks on a slow handshake. The gate **never writes status**; it reads what P3.3 wrote. Log style in that file: `tracing::warn!("…")`.

**Fatal switch:** no QA/test profile exists in the server (`rg 'ALEPH_QA|ALEPH_TEST_PROFILE' src` → only `ALEPH_QA_DRIVER` in `browser/manager.rs`). New env var `ALEPH_ACTIVATION_GATE=fatal`, read at the call site; `qa/plugins/run.sh` exports it for its stages in P7 (out of this phase). Exit code `78` (`EX_CONFIG`), the same family as `main.rs:127`'s `64`.

- [ ] **Step 1: Write the failing tests**

`src/extension/activation_gate.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{PluginKind, PluginOrigin, PluginRecord, PluginRegistry, PluginStatus};

    fn reg_with(statuses: &[(&str, PluginStatus)]) -> PluginRegistry {
        let mut r = PluginRegistry::new();
        for (id, s) in statuses {
            let mut rec = PluginRecord::new((*id).into(), (*id).into(), PluginKind::Mcp, PluginOrigin::Global);
            rec.status = s.clone();
            r.register_plugin(rec);
        }
        r
    }

    #[test]
    fn assess_lists_only_non_terminal_plugins_with_their_dependencies() {
        let r = reg_with(&[
            ("ok", PluginStatus::Loaded),
            ("off", PluginStatus::Disabled),
            ("bad", PluginStatus::Error("x".into())),
            ("no", PluginStatus::Blocked("policy".into())),
            ("wait", PluginStatus::Pending { waiting_on: vec!["mcp:plugin:wait/srv".into()] }),
        ]);
        let report = assess(&r);
        assert!(!report.is_clean());
        assert_eq!(
            report.non_terminal,
            vec![("wait".to_string(), vec!["mcp:plugin:wait/srv".to_string()])]
        );
        let lines = report.render_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("wait") && lines[0].contains("mcp:plugin:wait/srv"), "{lines:?}");
    }

    #[test]
    fn assess_is_clean_when_every_plugin_is_terminal() {
        let r = reg_with(&[("ok", PluginStatus::Loaded), ("bad", PluginStatus::Error("x".into()))]);
        assert!(assess(&r).is_clean());
        assert!(assess(&PluginRegistry::new()).is_clean());
    }

    #[test]
    fn posture_defaults_to_log_and_only_the_word_fatal_is_fatal() {
        assert!(matches!(posture_from_env(None), GatePosture::Log));
        assert!(matches!(posture_from_env(Some("log")), GatePosture::Log));
        assert!(matches!(posture_from_env(Some("fatal")), GatePosture::Fatal));
        assert!(matches!(posture_from_env(Some("FATAL")), GatePosture::Fatal));
        // An unrecognised value must not silently become fatal — nor silently
        // become "log": it is logged as unrecognised by the caller; here it
        // maps to Log so a typo cannot kill a daemon.
        assert!(matches!(posture_from_env(Some("yes")), GatePosture::Log));
    }
}
```

`src/diagnostics/checks/plugins_activated.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::check::{HealthCheck, Posture};
    use crate::diagnostics::finding::Severity;
    use crate::extension::{PluginKind, PluginOrigin, PluginRecord, PluginRegistry, PluginStatus};

    fn snapshot(statuses: &[(&str, PluginStatus)]) -> Vec<PluginRecord> {
        statuses
            .iter()
            .map(|(id, s)| {
                let mut rec = PluginRecord::new((*id).into(), (*id).into(), PluginKind::Mcp, PluginOrigin::Global);
                rec.status = s.clone();
                rec
            })
            .collect()
    }

    #[tokio::test]
    async fn no_manager_is_unknown_not_clean() {
        let check = PluginsActivatedCheck::from_records(None);
        let f = check.run(Posture::Inspect).await;
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].check_id, "extension/plugins-activated");
        assert!(matches!(f[0].severity, Severity::Warning), "{:?}", f[0]);
        assert!(f[0].title.contains("unknown"));
    }

    #[tokio::test]
    async fn a_pending_plugin_is_a_warning_that_names_the_dependency() {
        let check = PluginsActivatedCheck::from_records(Some(snapshot(&[
            ("ok", PluginStatus::Loaded),
            ("wait", PluginStatus::Pending { waiting_on: vec!["mcp:manager".into()] }),
        ])));
        let f = check.run(Posture::Inspect).await;
        assert_eq!(f.len(), 1);
        assert!(matches!(f[0].severity, Severity::Warning));
        assert!(f[0].detail.contains("wait") && f[0].detail.contains("mcp:manager"), "{}", f[0].detail);
    }

    #[tokio::test]
    async fn all_terminal_is_ok() {
        let check = PluginsActivatedCheck::from_records(Some(snapshot(&[
            ("ok", PluginStatus::Loaded),
            ("bad", PluginStatus::Error("e".into())),
        ])));
        let f = check.run(Posture::Inspect).await;
        assert_eq!(f.len(), 1);
        // `Finding::ok` is `Severity::Info` (`finding.rs:64-68`); there is no `is_ok()`.
        assert!(matches!(f[0].severity, Severity::Info), "{:?}", f[0]);
    }
}
```

`src/builtin_tools/doctor.rs` — extend `the_daemon_path_still_reports_the_two_log_backed_checks` (`:283-299`): add `let activated = crate::diagnostics::checks::PluginsActivatedCheck::from_records(None).id();` and include it in the `for id in [holes, log, activated]` loop; rename the test to `the_daemon_path_still_reports_the_three_handle_backed_checks`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib extension::activation_gate -- --nocapture` → FAIL to compile (module missing).
Run: `cargo test -p alephcore --lib diagnostics::checks::plugins_activated -- --nocapture` → FAIL to compile.

- [ ] **Step 3: Write minimal implementation**

`src/extension/types/plugins.rs`, in `impl PluginStatus` (next to `is_active`):

```rust
    /// Whether this is a state the plugin can stay in: everything except
    /// [`Self::Pending`]. Written as an exhaustive `match` with no wildcard
    /// so that a new variant is a compile error HERE, where a human decides
    /// whether it is terminal — not a silent "true". The activation gate and
    /// the doctor check derive their "still waiting" set from this.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        match self {
            Self::Loaded | Self::Disabled | Self::Error(_) | Self::Blocked(_) => true,
            Self::Pending { .. } => false,
        }
    }
```

`src/extension/activation_gate.rs`:

```rust
//! Boot-time activation gate: after every dependency has had its chance to
//! report, which plugins are still not in a terminal state, and what are
//! they waiting for.
//!
//! The dsh counterpart is `assertEntriesActivated` (evidence
//! `scan-dsh-cordis.md` §5.1, Top-8 #2): a PENDING unit names the services it
//! is still missing and boot fails. Aleph is a daemon, so the default posture
//! is one log line per plugin plus the `extension/plugins-activated` doctor
//! check; `ALEPH_ACTIVATION_GATE=fatal` (QA / test profiles) turns the same
//! report into a refusal to keep running.
//!
//! "Terminal" is [`PluginStatus::is_terminal`] — an exhaustive match on the
//! enum, so this gate cannot fall out of step with a new variant.

use crate::extension::registry::PluginRegistry;
use crate::extension::types::PluginStatus;

/// Every plugin that did not reach a terminal state, with what it waits for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ActivationReport {
    pub non_terminal: Vec<(String, Vec<String>)>,
}

impl ActivationReport {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.non_terminal.is_empty()
    }

    /// One operator-readable line per plugin, e.g.
    /// `plugin `x` did not activate: waiting on mcp:plugin:x/srv`.
    #[must_use]
    pub fn render_lines(&self) -> Vec<String> {
        self.non_terminal
            .iter()
            .map(|(id, waiting)| format!("plugin `{id}` did not activate: waiting on {}", waiting.join(", ")))
            .collect()
    }
}

/// Sweep the registry. Sorted by id so two boots render identically.
#[must_use]
pub fn assess(registry: &PluginRegistry) -> ActivationReport {
    let mut non_terminal: Vec<(String, Vec<String>)> = registry
        .list_plugins()
        .into_iter()
        .filter(|p| !p.status.is_terminal())
        .map(|p| {
            let waiting = match &p.status {
                PluginStatus::Pending { waiting_on } => waiting_on.clone(),
                // `is_terminal` is false only for Pending today; if a second
                // non-terminal variant appears, `is_terminal`'s exhaustive
                // match forces this arm to be revisited too.
                _ => Vec::new(),
            };
            (p.id.clone(), waiting)
        })
        .collect();
    non_terminal.sort();
    ActivationReport { non_terminal }
}

/// What boot does with a non-clean report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatePosture {
    /// One `warn!` line per plugin; the daemon keeps running (default).
    Log,
    /// `error!` + exit 78: a QA fixture that boots with a plugin stuck
    /// pending has found the thing this gate exists to find.
    Fatal,
}

/// `ALEPH_ACTIVATION_GATE`: only the word `fatal` (any case) is fatal;
/// anything else, including a typo, is `Log` — a misspelling must not be
/// able to kill a daemon.
#[must_use]
pub fn posture_from_env(value: Option<&str>) -> GatePosture {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("fatal") => GatePosture::Fatal,
        _ => GatePosture::Log,
    }
}
```

`src/extension/mod.rs`: `pub mod activation_gate;` next to `pub mod readiness;`.

`src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:596-602` — the block today:

```rust
        // Add plugin tools to both LLM tool list and BuiltinToolRegistry
        {
            use alephcore::gateway::handlers::plugins::get_extension_manager;
            if let Ok(ext_manager) = get_extension_manager() {
                if let Err(e) = ext_manager.ensure_loaded().await {
                    tracing::warn!("Failed to load extensions for plugin tools: {}", e);
                }
```

gains, immediately after the `ensure_loaded()` `if let`:

```rust
                // Activation gate (dsh `assertEntriesActivated`, evidence
                // scan-dsh-cordis.md §5.1): every plugin has been mounted
                // with every handle present (P1.8), so a plugin still
                // `Pending` once its servers have answered is waiting on
                // something that is not coming. `activation_settled` is the
                // watchers' own completion — no timer. Spawned so a slow
                // handshake never holds boot; the gate reads status and
                // never writes it. Daemon posture: one line per plugin + the
                // `extension/plugins-activated` doctor check; QA profiles set
                // ALEPH_ACTIVATION_GATE=fatal.
                let gate_manager = std::sync::Arc::clone(ext_manager);
                tokio::spawn(async move {
                    use alephcore::extension::activation_gate::{assess, posture_from_env, GatePosture};
                    gate_manager.activation_settled().await;
                    let report = assess(&gate_manager.get_plugin_registry().await);
                    if report.is_clean() {
                        tracing::info!("activation gate: every plugin reached a terminal status");
                        return;
                    }
                    let posture = posture_from_env(std::env::var("ALEPH_ACTIVATION_GATE").ok().as_deref());
                    for line in report.render_lines() {
                        match posture {
                            GatePosture::Log => tracing::warn!(gate = "extension/plugins-activated", "{line}"),
                            GatePosture::Fatal => tracing::error!(gate = "extension/plugins-activated", "{line}"),
                        }
                    }
                    if posture == GatePosture::Fatal {
                        tracing::error!(
                            count = report.non_terminal.len(),
                            "ALEPH_ACTIVATION_GATE=fatal: plugins did not activate; exiting"
                        );
                        std::process::exit(78);
                    }
                });
```

(`get_extension_manager()` returns the `&'static Arc<ExtensionManager>` from the capability slot, `manager_global.rs:69` via `gateway/handlers/plugins`; cloning the `Arc` is the only cost.)

`docs/reference/FEATURE_LOCATOR.md:2975` (§5.9 Doctor, the daemon-side enumeration: quote of the fragment to extend — `守护进程侧再加 provider 连通（\`with_runtime_checks\`，要 live config+vault）· \`ext/idle-extensions\`（要 MCP 句柄）· \`core/capability-wiring\`（要身处已 boot 的进程）`) gains one item: `· \`extension/plugins-activated\`（要 extension manager；boot 激活闸 \`extension::activation_gate\` 的 doctor 面——列出 mount 后仍 \`Pending\` 的插件及其 \`waiting_on\`）`. The `REGISTERED_CHECKS = 15` number in the same sentence is stale on 3ddc1f2e7 already (the count is derived by `doctor.rs::registered_checks()`, not pinned); do not "fix" it here — P8's FL pass owns that sentence's numbers.

`src/diagnostics/checks/plugins_activated.rs`:

```rust
//! `extension/plugins-activated` — which mounted plugins never reached a
//! terminal status, and what each is waiting for.
//!
//! The doctor face of `extension::activation_gate`: the boot task logs the
//! same report once; this check re-asks the live registry on every run so an
//! operator who missed the log line still gets the answer, with the
//! dependency named. It reads `PluginStatus::Pending { waiting_on }` as
//! written by `readiness::write_readiness` from `lifecycle.rs`; it never re-derives.
//!
//! Registered by the two daemon faces only (`doctor` tool, `diagnostics.run`)
//! through [`DiagnosticEngine::with_plugins_activated_check`]; the cold
//! `aleph-server doctor` has no extension manager and must not pretend the
//! plugin set is clean — hence UNKNOWN when the handle is absent.

use async_trait::async_trait;

use crate::diagnostics::check::{unknown_finding, HealthCheck, Posture};
use crate::diagnostics::finding::{Finding, Severity};
use crate::extension::activation_gate::{assess, ActivationReport};
use crate::extension::{PluginRecord, PluginRegistry};

const ID: &str = "extension/plugins-activated";
const SUBJECT: &str = "Plugin activation";

pub struct PluginsActivatedCheck {
    source: Source,
}

enum Source {
    /// The daemon: ask the live registry on every run.
    Live,
    /// A snapshot (tests), or `None` = no manager (the cold process).
    Records(Option<Vec<PluginRecord>>),
}

impl PluginsActivatedCheck {
    /// The daemon face: reads `extension::try_extension_manager()` at run time.
    #[must_use]
    pub const fn live() -> Self {
        Self { source: Source::Live }
    }

    /// A fixed snapshot; `None` reproduces "no extension manager".
    #[must_use]
    pub const fn from_records(records: Option<Vec<PluginRecord>>) -> Self {
        Self { source: Source::Records(records) }
    }

    async fn report(&self) -> Option<ActivationReport> {
        match &self.source {
            Source::Live => {
                let manager = crate::extension::try_extension_manager()?;
                Some(assess(&manager.get_plugin_registry().await))
            }
            Source::Records(None) => None,
            Source::Records(Some(records)) => {
                let mut registry = PluginRegistry::new();
                for r in records {
                    registry.register_plugin(r.clone());
                }
                Some(assess(&registry))
            }
        }
    }
}

#[async_trait]
impl HealthCheck for PluginsActivatedCheck {
    fn id(&self) -> &'static str {
        ID
    }

    fn title(&self) -> &'static str {
        "Plugin activation"
    }

    async fn run(&self, _posture: Posture) -> Vec<Finding> {
        let Some(report) = self.report().await else {
            return vec![unknown_finding(
                ID,
                SUBJECT,
                "this process has no extension manager, so no plugin's status could be \
                 read. Run `aleph doctor` against the running daemon rather than \
                 `aleph-server doctor`, which is a cold process.",
            )];
        };
        if report.is_clean() {
            return vec![Finding::ok(
                ID,
                "Every plugin reached a terminal status",
                "no plugin is still waiting on a dependency",
            )];
        }
        vec![Finding::problem(
            ID,
            Severity::Warning,
            "Plugins still waiting on a dependency",
            report.render_lines().join("; "),
        )
        .with_fix_hint(
            "each line names what the plugin waits for: `mcp:manager` = the MCP subsystem \
             never attached (is `[mcp]` enabled?); `mcp:<server>` = that server never \
             answered initialize (see `mcp.list` / the server's log). Statuses change only \
             when the dependency reports — there is no timeout.",
        )]
    }
}
```

(`Finding::ok` (`finding.rs:64`, severity `Info`), `Finding::problem` (`:78`), `with_fix_hint` (`:97`) and the pub fields `check_id` / `severity` / `title` / `detail` (`:40-45`) are the names used; `Finding` has no `is_ok()`.)

`src/diagnostics/checks/mod.rs`: `pub mod plugins_activated;` + `pub use plugins_activated::PluginsActivatedCheck;` (alphabetical, after `media_codecs`). `src/diagnostics/mod.rs`, after `with_extension_usage_check` (`:266`):

```rust
    /// Append `extension/plugins-activated`, the doctor face of the boot
    /// activation gate. Daemon faces only, same reason and same
    /// UNKNOWN-when-absent rule as [`Self::with_extension_usage_check`]: the
    /// cold `aleph-server doctor` has no extension manager.
    #[must_use]
    pub fn with_plugins_activated_check(mut self) -> Self {
        self.checks
            .push(Arc::new(checks::PluginsActivatedCheck::live()));
        self
    }
```

and both daemon faces chain `.with_plugins_activated_check()` after `.with_session_log_check()` (`doctor.rs:124`, `diagnostics.rs:89`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension::activation_gate -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib diagnostics::checks::plugins_activated -- --nocapture` → PASS.
Run: `cargo test -p alephcore --lib builtin_tools::doctor -- --nocapture` → PASS (`registered_checks()` derives the count, so no literal to bump; the identity guard now names three ids).
Run: `cargo test -p alephcore --bins` → the boot call site compiles and P1.8's `boot_order_tests::handles_are_installed_before_the_first_extension_load` still passes (the gate is spawned AFTER `ensure_loaded()`, which is exactly the order that guard reads).
Run: `rg -n 'presence_discipline' src/diagnostics/checks/mod.rs` → read that guard's `CONFLATING` list; the new check uses no `.exists()` / `read_dir` / `Err(_` so it passes.

**Mutation step:** in `assess` change `.filter(|p| !p.status.is_terminal())` to `.filter(|_| false)` → `assess_lists_only_non_terminal_plugins_with_their_dependencies` red and `a_pending_plugin_is_a_warning_that_names_the_dependency` red. Restore.

- [ ] **Step 5: Commit**

```bash
git add src/extension/activation_gate.rs src/extension/types/plugins.rs src/extension/mod.rs src/bin/aleph-server/commands/start/builder/agent_init/mod.rs src/diagnostics/checks/plugins_activated.rs src/diagnostics/checks/mod.rs src/diagnostics/mod.rs src/builtin_tools/doctor.rs src/gateway/handlers/diagnostics.rs docs/reference/FEATURE_LOCATOR.md
git commit -m "extension: boot activation gate (ALEPH_ACTIVATION_GATE) + doctor check extension/plugins-activated

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

### Task P3.5: G6 census — the terminal set is the enum; and `Pending` never times out

**Files:**
- Modify: `src/extension/types/plugins.rs` (tests module: the census test)
- Modify: `src/extension/lifecycle.rs` (tests module: the clock test)
- Test: both

**Interfaces:**
- Consumes: `PluginStatus::is_terminal` (P3.4), `readiness::write_readiness` / `watch_server_starts` (P3.3).
- Produces: nothing.

G6 as a guard has two layers. Layer 1 is the compiler: `is_terminal`, `label`, `write_readiness`'s two matches over `PluginStatus` and `derive_readiness`'s match over `ServerStart` are all wildcard-free, so a new `PluginStatus` (or `ServerStart`) variant does not build. Layer 2 is the test below, which exists so that the **decision** ("is the new variant terminal?") is recorded where a reviewer reads it, not only where the compiler stops.

- [ ] **Step 1: Write the failing tests**

`src/extension/types/plugins.rs` `mod tests`:

```rust
    /// G6: the activation gate's "terminal" set is derived from the enum.
    /// This test lists every variant ONCE with the expected answer; adding
    /// a variant to the enum without adding a row here does not compile
    /// (the `match` below has no wildcard), and adding a row with the wrong
    /// answer fails the assertion. Mutation record: adding `Paused` to the
    /// enum turned this red with `non-exhaustive patterns` before any row
    /// was written for it.
    #[test]
    fn g6_terminal_set_is_derived_from_the_enum() {
        let rows = [
            (PluginStatus::Loaded, true),
            (PluginStatus::Disabled, true),
            (PluginStatus::Error("e".into()), true),
            (PluginStatus::Blocked("b".into()), true),
            (PluginStatus::Pending { waiting_on: vec!["mcp:manager".into()] }, false),
        ];
        for (status, expected) in &rows {
            // The match is the census: every variant must appear.
            let by_match = match status {
                PluginStatus::Loaded
                | PluginStatus::Disabled
                | PluginStatus::Error(_)
                | PluginStatus::Blocked(_) => true,
                PluginStatus::Pending { .. } => false,
            };
            assert_eq!(status.is_terminal(), *expected, "{status:?}");
            assert_eq!(by_match, *expected, "{status:?}: the test's own census disagrees with is_terminal");
        }
        assert_eq!(rows.len(), 5, "one row per variant — update when the enum changes");
    }
```

`src/extension/lifecycle.rs` `mod tests` (next to P3.3's, which owns `write_mcp_project_plugin`):

```rust
    /// 判据 §8 / spec §3.3: `Pending` changes only when a dependency reports.
    /// Advance a paused tokio clock by an hour with NO dependency report and
    /// the status is byte-identical. Mutation record: a
    /// `tokio::spawn(async { sleep(10min); flip Pending→Error })` inside
    /// `watch_server_starts` (or `write_readiness`) turns this red.
    #[tokio::test(start_paused = true)]
    async fn pending_never_times_out_into_error() {
        let dir = tempfile::tempdir().unwrap();
        write_mcp_project_plugin(dir.path(), "p3-patient");
        let (manager, _cfg) = isolated_manager(dir.path()).await;
        // No MCP handle: the mount records the `mcp_server` skip and the row
        // is `Pending { ["mcp:manager"] }` (P3.3, moment 1). Nothing will ever
        // report for it in this test — the exact situation a timeout would
        // be tempted to "resolve".
        manager.load_all().await.unwrap();
        let before = manager.get_plugin_record("p3-patient").await.unwrap().status;
        assert_eq!(before, PluginStatus::Pending { waiting_on: vec!["mcp:manager".into()] });

        tokio::time::advance(std::time::Duration::from_secs(60 * 60)).await;
        tokio::task::yield_now().await;

        let after = manager.get_plugin_record("p3-patient").await.unwrap().status;
        assert_eq!(after, before, "an hour of silence is still silence, not failure");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib g6_terminal_set -- --nocapture` → PASS immediately (the enum and `is_terminal` already agree). **The red is the mutation, run now:** add `Paused,` to `PluginStatus` → `cargo test -p alephcore --lib g6_terminal_set --no-run` fails to compile at `is_terminal` (`plugins.rs`), at `label`, at the test's match, and at `write_readiness`'s two matches (`readiness.rs`); `derive_readiness` is untouched (it matches `ServerStart`, not `PluginStatus`). Record the red list in the commit message; remove `Paused`.
Run: `cargo test -p alephcore --lib pending_never_times_out -- --nocapture` → PASS. **Mutation:** in `lifecycle.rs::mount_parsed`, right after the moment-1 `write_readiness` (the `mcp_server`-skipped branch), add `tokio::spawn({ let reg = Arc::clone(&self.plugin_registry); let id = id.clone(); async move { tokio::time::sleep(std::time::Duration::from_secs(600)).await; if let Some(p) = reg.write().await.get_plugin_mut(&id) { if matches!(p.status, PluginStatus::Pending{..}) { p.status = PluginStatus::Error("timeout".into()) } } } });` → red with `left: Error("timeout"), right: Pending {..}`. Remove.

- [ ] **Step 3: Write minimal implementation**

None — both guards hold against the code from P3.1–P3.4. The only edit is the two tests.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib extension:: -- --nocapture` → PASS.
Then the six-command verification set for the whole phase:

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --bins
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo check -p aleph-desktop-macos          # (and -windows/-linux on those hosts)
just _stage-shell-placeholders && cargo clippy --workspace --all-targets
```

and `git diff --stat 3ddc1f2e7 -- src/harness/` → empty.

- [ ] **Step 5: Commit**

```bash
git add src/extension/types/plugins.rs src/extension/lifecycle.rs
git commit -m "extension: G6 census — terminal set derived from the enum; Pending is time-invariant

Mutation record: adding a Paused variant fails to compile at is_terminal, label,
write_readiness (both PluginStatus matches) and g6_terminal_set_is_derived_from_the_enum;
a 10-minute Pending→Error timer after mount_parsed's moment-1 write turns pending_never_times_out_into_error red.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

## Contract deltas

- **`scope.rs` content.** The spec/contract say `scope.rs` "today holds `project_scope_allows`". It holds `scope_install_dir` / `parse_scope`; `project_scope_allows` is a `HookExecutor` method (`hooks/executor.rs:362`). P2.1 renames the file and keeps both fns in `visibility.rs` (10 path references updated); the predicate stays a method (now `(hook, &VisibilityCtx)`).
- **`VisibilityCtx::for_session()` has a sibling `from_project_root(Option<PathBuf>)`.** Same derivation (task-local value, else daemon CWD, canonicalised); the sibling exists because slash resolution and `commands.list` run before the task-local is published. `AgentRegistry::resolve` feeds its existing `project_root` parameter through it.
- **`PluginId` is `String`** everywhere in P2/P3 (`PluginRecord.id`, `ToolRegistration.plugin_id`, the `ActivationReport` tuples). `domain::skill::PluginId` is a separate newtype used only by `SkillSource::Plugin`.
- **`PluginStatus` names** (G-2): `Loaded` / `Disabled` / `Blocked(String)` / `Error(String)` stay; P3 adds `Pending { waiting_on }` and removes `Overridden`. The contract's `Active` / `Failed { step, reason }` are dropped; mount failure is `Error("<step>: <reason>")` at P1's single write site.
- **`PluginStatus::is_active()` is true for `Pending`.** A pending plugin's other capabilities stay registered; `hub/reconcile.rs:52`'s `enabled` bit follows `is_active()` too.
- **`PluginRuntimeStatus` (wire)** gains `Pending`, loses `Overridden`; no new wire key — `waiting_on` rides in the existing `status_detail` string. Panel and CLI need no code change (verified against their `match` arms, which already have a catch-all `(status, detail)` arm).
- **`PluginRegistry::unregister_plugin` already exists** (`registry/plugin_registry/mod.rs:432`); the contract lists it as new. P1 can call it as-is.
- **Owner field on catalog rows is for visibility only** (R1.8). P2.8 adds `SkillInfo.plugin_id: Option<String>` and `ToolSource::Skill { id, plugin_id: Option<String> }` (`#[serde(default)]`) and sets it in P1.7's `slash_effect::plugin_command_skill_info`, the one producer of a plugin command's `SkillInfo` (R1.3; the plural `plugin_command_skill_infos` only maps over it). P1's `unregister_skills(&[String])` keeps removing by the ids its disposer recorded; nothing in P2 asks it to look at `plugin_id`.
- **Readiness is written inside P1's `lifecycle.rs`** (R3.2): `mount_parsed` (moment 1, the `mcp_server` skip) and `watch_server_starts` (moments 2 and 3, around the receivers R1.1 makes `register_transient_servers` return). `watch_server_starts` becomes `async` so moment 2 is written before the task is spawned; `mount_parsed` returns the row's status instead of a constant `Loaded`. P1's `activation_watchers: StdMutex<Vec<JoinHandle<()>>>` push and `activation_settled()` are untouched; P3.3 adds only the two `write_readiness` calls and the report vector.
- **`activation_gate::assess` runs in a task spawned right after boot's `ensure_loaded()`** (`agent_init/mod.rs:600`), gated on `manager.activation_settled().await` (R3.3); it never writes status. `ActivationReport` field is `non_terminal: Vec<(String, Vec<String>)>` as in the contract.
- **Doctor registration builder** is `DiagnosticEngine::with_plugins_activated_check()` (daemon faces only, UNKNOWN when no manager), following the `with_extension_usage_check` precedent; the cold `default_registry()` does not get it.
- **QA/test fatal switch** is a new env var `ALEPH_ACTIVATION_GATE=fatal` (exit 78, from the spawned gate task — accepted in R3.3); no QA profile existed to hook into. `qa/plugins/run.sh` should export it for every stage (P7's job; P2.11's `visibility` stage does not set it because that stage boots with an MCP-less config where the manager IS attached — verify on first run; if `mcp:manager` shows up pending there, the fixture's `[mcp]` config, not the gate, is what to fix).
- **No new process-global static.** The plugin-id→`ScopeKey` map lives on `ExtensionManager` (`plugin_scope_keys`), not in `utils/paths.rs` / `agents/registry.rs`, because `capability/census.rs` pins the count of such statics. `PLUGIN_SKILL_DIRS` and `PLUGIN_SUBAGENTS` change element type only (their census doc rows updated in P2.6/P2.7).
- **Management faces are not project-gated:** `plugins.list`, `plugins.callTool`, `plugin_manage` see every row regardless of project (operator verbs). Only model-facing faces filter. The QA stage's oracle is therefore the mock provider's request log.
- **`discover_plugins_with_extra` signature** changes from `&[PathBuf]` to `&[ProjectPluginParent { project_root, dir }]`; the `cfg(test)` `ExtensionConfig::extra_plugin_parents` follows. `DiscoveredPath` / `ScanDirectory` replace their stored `source: DiscoverySource` with `scope: DiscoveryScope { Global(GlobalRoot), Project { root } }` (R2.4: a project discovery without a root is unrepresentable); `source()` is derived. P4.10's `ClaudeCache` becomes a `GlobalRoot` variant and `ScopeKey::from_discovery` / `DiscoveryScope::source` are wildcard-free so the compiler demands its arm (R2.5).
- **`commands.list` takes no `project_root` param** (R2.3); the handler derives `VisibilityCtx::from_project_root(None)` and takes the manager-backed `owner_visible` closure as a parameter (tests pass `&|_, _| true`).
- **`derive_readiness` has no `runtime:<name>` arm** (R3.1) and no manager-health poll: its inputs are `manager_attached` + `(server_id, ServerStart)` per enqueued server, where `ServerStart` is the receiver's answer.
- **Two new module edges** from `src/extension/visibility.rs`: → `crate::domain::skill` (P2.6 wrapper) and → `crate::tool_metadata` (P2.8 wrapper). Neither module imports `extension` (`rg 'crate::extension' src/tool_metadata src/domain` → 0), so no cycle; the shared logic is the generic `retain_visible_owned<T>` which has no such edge. If the lead prefers `visibility.rs` to stay type-free, move the two 8-line wrappers to `src/skill/prompt.rs` and `src/gateway/handlers/commands.rs` respectively — tests move with them unchanged.

## Open questions for the lead

- All five earlier questions are closed by the rulings: `runtime:<name>` arm CUT (R3.1); `commands.list` param dropped (R2.3); `(Project, None)` made unrepresentable (R2.4); `Pending` counts as `is_active()` and the gate exits from a spawned task (R3.3).
- P3.3 makes `watch_server_starts` `async` (its only caller `mount_parsed` is async) so moment 2 is written before the spawn; P1's final text has it sync — a one-word signature change plus `.await` at the call site, flagged here so the P1 writer is not surprised. If it must stay sync, moment 2 moves inside the spawned task and the `mount_with_a_silent_manager…` test awaits `tokio::task::yield_now()` once before reading the row.

## Coverage map

- spec §3.4 (`ScopeKey`, `VisibilityCtx`, `visible_to`, rename) → P2.1, P2.2
- spec §3.4 "每条 registry 行带 ScopeKey，从发现时的 scope 派生" → P2.3
- spec §3.4 "hooks 的 `project_scope_allows` 改为调同一谓词" + "推导复用 hooks 今天那一份" → P2.2, P2.3
- spec §3.4 five faces ①–⑤ → P2.5 (tool index), P2.6 (skills index + `skill_read` search set), P2.7 (agents), P2.8 (slash list + fast path), P2.9 (MCP bridge)
- spec §3.4 behaviour change "无 project 的会话只见 Global" → P2.1 truth table row, P2.2 executor test, P2.10, P2.11
- spec §3.5 `visible_to` truth-table unit test → P2.1
- spec §5 integration "无项目会话看不见 Project 插件" → P2.10
- spec §6 `qa/plugins/run.sh visibility` → P2.11
- spec §3.3 `Pending { waiting_on }` from declared deps (the enqueued servers' reports; no runtime arm, R3.1) → P3.1, P3.2, P3.3
- spec §3.3 `Overridden` removed + `PLUGIN_SYSTEM.md:149-160` fixed same commit → P3.1
- spec §3.3 / §4 "`Pending` 不因超时变 `Failed`" → P3.5 (clock test), P3.2 (no timer in the derivation), P3.3 (a dropped receiver stays a wait)
- spec §3.3 activation gate: boot log (after `activation_settled()`, R3.3) + doctor `extension/plugins-activated` + QA fatal + FL §5.9 line → P3.4
- spec §3.5 G6 census (terminal set derived from the enum, mutation recorded) → P3.4 (`is_terminal`), P3.5
- evidence `scan-aleph-plugins.md` §7 row 16 (`Overridden` zero producers) → P3.1; row 20 (no per-project isolation) → P2.3–P2.9; §2.5 "MCP servers" row (`Loaded` before spawn) → P3.3; §2.7 (`scope.rs` is not a disposal scope) → P2.1 orientation note
- reconciliation R3.4 doc-code 同笔: FL §5.10 sentence → P2.2; FL §5.9 doctor line → P3.4; `PLUGIN_SYSTEM.md:149-171` minimal truth fix → P3.1 (P8.3(a) rewrites the table later)
- evidence `scan-dsh-cordis.md` §5.1 `assertEntriesActivated` / Top-8 #2 risk ("terminal state, `waiting_on` from declared deps, not a boolean") → P3.2, P3.3, P3.4
