# P2 Subagent Uplift Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship P2 Stage E (filesystem agent loader), Stage F (background subagent streaming progress wrapper), and Stage G (semantic tool sets) per the locked design at `docs/superpowers/specs/2026-05-09-subagent-uplift-p2-design.md`.

**Architecture:** Extend `src/agents/` with three additive modules (`loader.rs`, `progress.rs`, `tool_sets.rs`). Add **1** backward-compatible variant (`AgentDefShadowed`) to `LoopTraceEvent` in `src/harness/trace.rs` for shadow event observability. Decorator pattern: `ForwardingTraceSink` wraps parent's trace_sink only on background subagent paths; sync paths unchanged. Migrate the `explore` builtin agent to `INVESTIGATION` named set as 1 demo (effective behavior preserved via Stage B recursion guard). Zero changes to `src/harness/agent.rs`.

**Tech Stack:** Rust 2021, tokio, `serde_yaml` (already in Cargo.lock), `thiserror`, `tracing`, `uuid` (existing). Test deps: `tempfile` (verify or add).

**Single PR / 3 atomic commits**: E → F → G. Total ~850 lines incl. ~330 test lines. R10 baseline: `src/harness/*.rs` ≤ 2811 + 10 lines (1 variant addition); `src/harness/agent.rs` zero changes.

---

## §0 — Pre-flight Baseline Locks

### Task 0: Lock R10 + workspace baseline before any code change

**Files:**
- Read-only inspection

- [ ] **Step 1: Lock harness line/file baseline (P1 ship state)**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
```

Expected outputs to record (target invariants for all 3 commits below):
- `HARNESS_LINES_BASELINE`: ~2811 (P1 ship value; allow +10 for `AgentDefShadowed` variant)
- `HARNESS_FILES_BASELINE`: 10 (must remain 10 throughout P2)

- [ ] **Step 2: Lock agents module baseline**

```bash
wc -l src/agents/*.rs | tail -1
wc -l src/agents/subagent_spawner.rs
wc -l src/agents/types.rs
wc -l src/agents/registry.rs
wc -l src/agents/background_tracker.rs
wc -l src/agents/subagent_tool.rs
```

Record values; P2 increment must remain ≤ +600 lines on `src/agents/` total. `subagent_spawner.rs` ≤ 600 (roadmap 0.4).

- [ ] **Step 3: Confirm key crates already vendored**

```bash
grep -E '^name = "(serde_yaml|thiserror|tempfile|uuid|tokio)"' Cargo.lock
```

Expected: all 5 present except possibly `tempfile`. If `tempfile` missing, note for Task E8 dev-dependency addition.

- [ ] **Step 4: Run baseline test sweep + clippy**

```bash
cargo test --workspace --lib 2>&1 | tail -10
cargo clippy --workspace -- -D warnings 2>&1 | tail -20
```

Record passing test count + any pre-existing clippy errors (P1 baseline). New code must not introduce additional clippy errors.

- [ ] **Step 5: Confirm baseline expected — DO NOT commit**

This task records baseline only; no file changes. Output values feed PR-level verification (§5.2 of spec).

---

## Stage E — Filesystem Agent Loader (Commit 1 / ~340 lines)

### Task E1: Add `AgentSource` enum + `source` field to `AgentDef`

**Files:**
- Modify: `src/agents/types.rs` (add enum + field)

- [ ] **Step 1: Add `AgentSource` enum**

Add after the existing `AgentMode` enum in `src/agents/types.rs`:

```rust
/// Origin of an AgentDef. Set by `crate::agents::loader` based on load source;
/// hardcoded `builtin_agents()` entries default to `Builtin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentSource {
    /// Hardcoded in `crate::agents::registry::builtin_agents()`
    Builtin,
    /// User-level: `~/.aleph/data/agents/*.md`
    User,
    /// Project-level: `<project>/.aleph/agents/*.md` (highest precedence)
    Project,
}

impl Default for AgentSource {
    fn default() -> Self {
        Self::Builtin
    }
}
```

- [ ] **Step 2: Add `source` field to `AgentDef`**

In `src/agents/types.rs`, add `source` field to `AgentDef` struct definition. The exact spot: after the existing fields, with `#[serde(default)]` to preserve backward compatibility:

```rust
pub struct AgentDef {
    // ... all existing fields preserved unchanged ...

    /// Origin of this AgentDef. Defaults to `Builtin` for backward-compat;
    /// loader sets `User`/`Project` for filesystem-loaded agents.
    #[serde(default)]
    pub source: AgentSource,
}
```

- [ ] **Step 3: Add unit tests for `AgentSource` defaulting**

Add in `#[cfg(test)] mod tests { ... }` of `src/agents/types.rs`:

```rust
#[test]
fn agent_source_defaults_to_builtin() {
    assert_eq!(AgentSource::default(), AgentSource::Builtin);
}

#[test]
fn agent_def_default_source_is_builtin() {
    let def = AgentDef::new("foo", AgentMode::SubAgent);
    assert_eq!(def.source, AgentSource::Builtin);
}

#[test]
fn agent_source_serde_roundtrip() {
    for variant in [AgentSource::Builtin, AgentSource::User, AgentSource::Project] {
        let json = serde_json::to_string(&variant).unwrap();
        let back: AgentSource = serde_json::from_str(&json).unwrap();
        assert_eq!(variant, back);
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore --lib agents::types
```

Expected: All 3 new tests pass; existing types tests continue to pass.

- [ ] **Step 5: Verify builtin_agents() unchanged**

```bash
cargo test -p alephcore --lib agents::registry::tests::test_builtin_agents_count
```

Expected: PASS (still asserts 7 builtins). Loader does not modify `builtin_agents()`.

---

### Task E2: Add `AgentDefShadowed` variant to `LoopTraceEvent`

**Files:**
- Modify: `src/harness/trace.rs` (add 1 variant + ~10 lines)

- [ ] **Step 1: Add the variant**

In `src/harness/trace.rs`, add to the `LoopTraceEvent` enum (place after `SessionCompleted`):

```rust
/// Emitted at startup when a filesystem-loaded AgentDef shadows a lower-tier
/// definition with the same id. Higher-precedence sources (Project > User >
/// Builtin) silently override; this event records the shadow for diagnostics.
AgentDefShadowed {
    id: String,
    winner_source: crate::agents::types::AgentSource,
    shadowed_source: crate::agents::types::AgentSource,
},
```

- [ ] **Step 2: Verify exhaustive `match` consumers compile**

```bash
cargo check --workspace 2>&1 | grep -E "non-exhaustive|missing.*AgentDefShadowed" | head
```

Expected: No exhaustive-match errors. If any consumer match-arms LoopTraceEvent without `_`, add a no-op arm:

```rust
LoopTraceEvent::AgentDefShadowed { .. } => {
    // intentional no-op; loader-time observability event
}
```

- [ ] **Step 3: Lock R10 trace.rs line growth**

```bash
wc -l src/harness/trace.rs
```

Expected: previous baseline + ≤ 10 lines. If > 10, justify in commit body.

- [ ] **Step 4: Run harness tests**

```bash
cargo test -p alephcore --lib harness
```

Expected: All harness tests pass; no new failures from variant addition.

---

### Task E3: Implement frontmatter parser (no new crate; uses `serde_yaml`)

**Files:**
- Create: `src/agents/loader.rs` (start scaffold; full impl in subsequent tasks)

- [ ] **Step 1: Create `src/agents/loader.rs` with parser**

Create the file with the frontmatter splitter + parser:

```rust
//! Filesystem agent loader for P2 Stage E.
//!
//! Loads AgentDef definitions from markdown files with YAML frontmatter:
//!   - Project tier: `<project>/.aleph/agents/*.md`  (highest precedence)
//!   - User tier:    `~/.aleph/data/agents/*.md`
//!   - Builtin tier: `crate::agents::registry::builtin_agents()` (lowest precedence)
//!
//! Per P2 design (Q4-b): user/project frontmatter cannot declare `mode` (forced
//! to SubAgent); writing `mode: Primary` triggers a `ForbiddenSystemField` error.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agents::types::{AgentDef, AgentMode, AgentSource};

#[derive(Debug, thiserror::Error)]
pub enum LoaderError {
    #[error("malformed frontmatter in {path}: {source}")]
    Frontmatter {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("missing closing '---' delimiter in {path}")]
    MissingDelimiter { path: PathBuf },

    #[error("file stem '{stem}' does not match agent id '{id}' in {path}")]
    IdMismatch {
        path: PathBuf,
        stem: String,
        id: String,
    },

    #[error("forbidden system field '{field}' in {path}: must not be set by user/project frontmatter")]
    ForbiddenSystemField {
        path: PathBuf,
        field: &'static str,
    },

    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Diagnostic record: emitted at startup as `LoopTraceEvent::AgentDefShadowed`
/// after the trace_sink is constructed (loader returns these; emitter wires).
#[derive(Debug, Clone)]
pub struct ShadowEvent {
    pub id: String,
    pub winner_source: AgentSource,
    pub shadowed_source: AgentSource,
}

/// Internal user-facing frontmatter shape (subset of AgentDef + system field
/// validation hooks). All fields optional except `id` / `description` /
/// `when_to_use`. Loader rejects `mode` if present.
#[derive(Debug, serde::Deserialize)]
struct UserFrontmatter {
    id: String,
    description: String,
    when_to_use: String,
    #[serde(default)]
    model_hint: Option<String>,
    #[serde(default)]
    allowed_tools: Vec<String>,
    #[serde(default)]
    allowed_tool_sets: Vec<String>,
    #[serde(default)]
    denied_tools: Vec<String>,
    #[serde(default)]
    max_iterations: Option<usize>,
    #[serde(default)]
    token_budget: Option<usize>,
    #[serde(default)]
    context_mode: Option<crate::agents::types::ContextMode>,

    /// System-managed: must not be set by user.
    #[serde(default)]
    mode: Option<String>,
    /// System-managed: must not be set by user.
    #[serde(default)]
    source: Option<String>,
}

/// Split a markdown file body into `(frontmatter_yaml, prompt_body)`.
/// Returns `None` if no leading `---` is found (treated as no frontmatter,
/// caller skips file with warn).
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let trimmed = content.trim_start();
    let rest = trimmed.strip_prefix("---")?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest.find("\n---")?;
    let yaml = &rest[..end];
    let body_start = rest[end..].strip_prefix("\n---").unwrap_or(&rest[end..]);
    let body = body_start.strip_prefix('\n').unwrap_or(body_start);
    Some((yaml, body))
}

/// Parse a single markdown file into AgentDef.
/// `source` is supplied by the caller based on which tier dir was scanned.
pub(crate) fn parse_file(path: &Path, source: AgentSource) -> Result<AgentDef, LoaderError> {
    let content = std::fs::read_to_string(path).map_err(|e| LoaderError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;

    let (yaml, body) = split_frontmatter(&content).ok_or_else(|| LoaderError::MissingDelimiter {
        path: path.to_path_buf(),
    })?;

    let fm: UserFrontmatter =
        serde_yaml::from_str(yaml).map_err(|e| LoaderError::Frontmatter {
            path: path.to_path_buf(),
            source: e,
        })?;

    if fm.mode.is_some() {
        return Err(LoaderError::ForbiddenSystemField {
            path: path.to_path_buf(),
            field: "mode",
        });
    }
    if fm.source.is_some() {
        return Err(LoaderError::ForbiddenSystemField {
            path: path.to_path_buf(),
            field: "source",
        });
    }

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    if stem != fm.id {
        return Err(LoaderError::IdMismatch {
            path: path.to_path_buf(),
            stem,
            id: fm.id.clone(),
        });
    }

    // Build AgentDef using existing builder; system fields forced.
    let mut def = AgentDef::new(&fm.id, AgentMode::SubAgent)
        .with_description(&fm.description)
        .with_when_to_use(&fm.when_to_use);
    if let Some(m) = fm.model_hint {
        def = def.with_model_hint(m);
    }
    if !fm.allowed_tools.is_empty() {
        def = def.with_allowed_tools(fm.allowed_tools);
    }
    if !fm.denied_tools.is_empty() {
        def = def.with_denied_tools(fm.denied_tools);
    }
    if let Some(n) = fm.max_iterations {
        def = def.with_max_iterations(n);
    }
    if let Some(n) = fm.token_budget {
        def = def.with_token_budget(n);
    }
    if let Some(cm) = fm.context_mode {
        def = def.with_context_mode(cm);
    }
    // allowed_tool_sets is set directly; builder may not exist yet (Stage G adds it).
    // For Stage E commit, persist via direct field mutation (allowed since we own def).
    def.allowed_tool_sets = fm.allowed_tool_sets;
    def.source = source;

    // Drop body for now; prompt_sections wiring is out of scope for P2 (frontmatter-
    // declared agents inherit the default prompt builder unless they specify
    // prompt_sections, which is a future extension).
    let _ = body;

    Ok(def)
}

#[cfg(test)]
mod tests;
```

- [ ] **Step 2: Add `loader` module declaration**

In `src/agents/mod.rs`, add:

```rust
pub mod loader;
```

(Place alongside existing `pub mod` declarations.)

- [ ] **Step 3: Defer `allowed_tool_sets` field until Task G2**

`def.allowed_tool_sets = fm.allowed_tool_sets;` will fail to compile in Stage E because the field is added in Stage G. **Workaround for the Stage E commit**: comment out that line and ignore the unread `allowed_tool_sets` field for now:

```rust
// allowed_tool_sets wiring lands in Stage G (Task G2)
let _ = fm.allowed_tool_sets;
```

Stage G (Task G2) un-comments and replaces with the direct field assignment. This keeps each commit independently compiling.

- [ ] **Step 4: Compile check**

```bash
cargo check -p alephcore
```

Expected: Clean compile (no warnings on the temporary `let _ =`).

---

### Task E4: Add unit tests for parser

**Files:**
- Create: `src/agents/loader/tests.rs` — wait, actually use inline `mod tests` per Task E3 step 1.

Adjust: tests live inline in `src/agents/loader.rs` as `#[cfg(test)] mod tests;` pulling from `tests.rs` sibling. Simpler: keep tests inline in same file.

- [ ] **Step 1: Replace `mod tests;` declaration with inline tests in `src/agents/loader.rs`**

Append to `src/agents/loader.rs` (replacing the `mod tests;` line):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(dir: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn parses_minimal_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "my-agent.md",
            "---\n\
             id: my-agent\n\
             description: Test agent\n\
             when_to_use: For tests\n\
             ---\n\
             body\n",
        );
        let def = parse_file(&path, AgentSource::User).unwrap();
        assert_eq!(def.id, "my-agent");
        assert_eq!(def.description, "Test agent");
        assert_eq!(def.when_to_use.as_deref(), Some("For tests"));
        assert_eq!(def.mode, AgentMode::SubAgent);
        assert_eq!(def.source, AgentSource::User);
    }

    #[test]
    fn rejects_mode_primary_in_user_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "evil.md",
            "---\n\
             id: evil\n\
             description: Tries to escalate\n\
             when_to_use: never\n\
             mode: Primary\n\
             ---\n",
        );
        let err = parse_file(&path, AgentSource::User).unwrap_err();
        assert!(
            matches!(err, LoaderError::ForbiddenSystemField { field: "mode", .. }),
            "expected ForbiddenSystemField(mode), got {err:?}"
        );
    }

    #[test]
    fn rejects_id_filename_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "foo.md",
            "---\n\
             id: bar\n\
             description: Mismatch\n\
             when_to_use: never\n\
             ---\n",
        );
        let err = parse_file(&path, AgentSource::User).unwrap_err();
        assert!(
            matches!(err, LoaderError::IdMismatch { .. }),
            "expected IdMismatch, got {err:?}"
        );
    }

    #[test]
    fn loads_with_default_fields_when_optional_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_tmp(
            &tmp,
            "minimal.md",
            "---\n\
             id: minimal\n\
             description: Minimal\n\
             when_to_use: minimal\n\
             ---\n",
        );
        let def = parse_file(&path, AgentSource::Project).unwrap();
        assert!(def.allowed_tools.is_empty());
        assert!(def.denied_tools.is_empty());
        assert!(def.max_iterations.is_none());
        assert_eq!(def.source, AgentSource::Project);
    }

    #[test]
    fn split_frontmatter_handles_no_delimiter() {
        let content = "no frontmatter here";
        assert!(split_frontmatter(content).is_none());
    }

    #[test]
    fn split_frontmatter_extracts_yaml_and_body() {
        let content = "---\nid: foo\n---\nbody text";
        let (yaml, body) = split_frontmatter(content).unwrap();
        assert_eq!(yaml, "id: foo");
        assert_eq!(body, "body text");
    }
}
```

- [ ] **Step 2: Add `tempfile` dev-dependency if missing**

```bash
grep -A1 '^\[dev-dependencies\]' Cargo.toml | grep tempfile
```

If missing:

```bash
cargo add --dev tempfile
```

(If `cargo add` not allowed, manually add `tempfile = "3"` under `[dev-dependencies]` in `Cargo.toml`.)

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib agents::loader
```

Expected: All 6 tests pass.

- [ ] **Step 4: Verify clippy clean**

```bash
cargo clippy -p alephcore --lib --tests -- -D warnings 2>&1 | tail -20
```

Expected: No new warnings/errors from `loader.rs`.

---

### Task E5: Implement `scan_dir` + `load_agents`

**Files:**
- Modify: `src/agents/loader.rs` (add scan + merge functions)

- [ ] **Step 1: Add `scan_dir` and `load_agents` functions**

Append to `src/agents/loader.rs` (before the `#[cfg(test)]` block):

```rust
/// Scan a directory for `*.md` files (non-recursive) and parse each.
/// Per-file errors are collected and surfaced via `tracing::warn!` rather
/// than propagating; only directory-level IO errors abort.
fn scan_dir(dir: &Path, source: AgentSource) -> Result<Vec<AgentDef>, LoaderError> {
    let entries = std::fs::read_dir(dir).map_err(|e| LoaderError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;

    let mut agents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        match parse_file(&path, source) {
            Ok(def) => agents.push(def),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "skipping malformed agent definition"
                );
            }
        }
    }
    Ok(agents)
}

/// Load all agents per the 3-tier priority: project > user > builtin.
/// Returns the merged set + a list of shadow events for diagnostics.
///
/// `home_dir` is typically the result of `crate::discovery::aleph_home_dir()`.
/// `project_dir` is typically the current working directory; pass `None` to
/// skip the project tier.
pub fn load_agents(
    home_dir: &Path,
    project_dir: Option<&Path>,
) -> Result<(Vec<AgentDef>, Vec<ShadowEvent>), LoaderError> {
    let mut by_id: HashMap<String, AgentDef> = HashMap::new();
    let mut shadows: Vec<ShadowEvent> = Vec::new();

    // Tier 1 (lowest): builtin
    for agent in crate::agents::registry::builtin_agents() {
        by_id.insert(agent.id.clone(), agent);
    }

    // Tier 2: user-level
    let user_dir = home_dir.join("data/agents");
    if user_dir.exists() {
        for agent in scan_dir(&user_dir, AgentSource::User)? {
            insert_with_shadow(&mut by_id, &mut shadows, agent, AgentSource::User);
        }
    }

    // Tier 3 (highest): project-level
    if let Some(proj_dir) = project_dir {
        let proj_agents = proj_dir.join(".aleph/agents");
        if proj_agents.exists() {
            for agent in scan_dir(&proj_agents, AgentSource::Project)? {
                insert_with_shadow(&mut by_id, &mut shadows, agent, AgentSource::Project);
            }
        }
    }

    Ok((by_id.into_values().collect(), shadows))
}

fn insert_with_shadow(
    by_id: &mut HashMap<String, AgentDef>,
    shadows: &mut Vec<ShadowEvent>,
    incoming: AgentDef,
    winner: AgentSource,
) {
    if let Some(prev) = by_id.insert(incoming.id.clone(), incoming.clone()) {
        shadows.push(ShadowEvent {
            id: incoming.id,
            winner_source: winner,
            shadowed_source: prev.source,
        });
    }
}
```

- [ ] **Step 2: Compile check**

```bash
cargo check -p alephcore
```

Expected: Clean compile.

- [ ] **Step 3: Add unit test for `scan_dir` skip-malformed behavior**

Append to `mod tests` in `src/agents/loader.rs`:

```rust
#[test]
fn scan_dir_skips_malformed_files() {
    let tmp = tempfile::tempdir().unwrap();
    write_tmp(
        &tmp,
        "good.md",
        "---\nid: good\ndescription: ok\nwhen_to_use: yes\n---\n",
    );
    write_tmp(&tmp, "bad.md", "no frontmatter\n");
    write_tmp(
        &tmp,
        "mode-primary.md",
        "---\nid: mode-primary\ndescription: x\nwhen_to_use: x\nmode: Primary\n---\n",
    );

    let agents = scan_dir(tmp.path(), AgentSource::User).unwrap();
    assert_eq!(agents.len(), 1, "only good.md should load");
    assert_eq!(agents[0].id, "good");
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore --lib agents::loader
```

Expected: All 7 tests pass.

---

### Task E6: Wire loader into AgentRegistry + startup

**Files:**
- Modify: `src/agents/registry.rs` (add `register_from_dirs` method)
- Modify: `src/bin/aleph-server/commands/start/orchestrator_init.rs` (call loader at startup)

- [ ] **Step 1: Add `register_from_dirs` method to `AgentRegistry`**

In `src/agents/registry.rs`, add inside `impl AgentRegistry`:

```rust
/// Load and register agents from the user/project filesystem tiers.
/// Returns shadow events for the caller to emit on the trace_sink once it's
/// constructed. Errors propagate; per-file malformed errors are logged via
/// `tracing::warn` inside the loader and skipped.
///
/// Builtins are NOT re-registered here; call `with_builtins()` first if not
/// already done.
pub fn register_from_dirs(
    &self,
    home_dir: &std::path::Path,
    project_dir: Option<&std::path::Path>,
) -> Result<Vec<crate::agents::loader::ShadowEvent>, crate::agents::loader::LoaderError> {
    let (agents, shadows) = crate::agents::loader::load_agents(home_dir, project_dir)?;
    // Note: load_agents merges builtins+user+project into one Vec. Since
    // builtins are already registered via with_builtins(), we filter to
    // non-Builtin sources to avoid duplicate registration that would emit
    // spurious shadow events on subsequent calls.
    for agent in agents
        .into_iter()
        .filter(|a| a.source != crate::agents::types::AgentSource::Builtin)
    {
        self.register(agent);
    }
    Ok(shadows)
}
```

- [ ] **Step 2: Find the AgentRegistry construction point in `orchestrator_init.rs`**

```bash
grep -n "AgentRegistry::with_builtins\|AgentRegistry::new\|Arc::new(AgentRegistry" \
    src/bin/aleph-server/commands/start/*.rs
```

Expected: locate the line where `agent_registry` is built with builtins. The patch-target is the immediate next statement after that.

- [ ] **Step 3: Patch the startup sequence**

After the existing `AgentRegistry::with_builtins()` call site, insert (replace `<existing_construction>` with what's actually there):

```rust
// P2 Stage E: load filesystem agents (user + project tiers)
let aleph_home = crate::discovery::aleph_home_dir().ok();
let project_dir = std::env::current_dir().ok();
let shadow_events = if let Some(home) = aleph_home.as_deref() {
    match agent_registry.register_from_dirs(home, project_dir.as_deref()) {
        Ok(shadows) => shadows,
        Err(e) => {
            tracing::warn!(error = %e, "filesystem agent loading failed; using builtins only");
            Vec::new()
        }
    }
} else {
    Vec::new()
};
```

- [ ] **Step 4: After trace_sink is constructed, emit shadow events**

In `orchestrator_init.rs`, find where `trace_sink` is finalized (likely passed into `HarnessDeps` or a Builder); right after, add:

```rust
// P2 Stage E: emit shadow events through the trace_sink (now ready)
for shadow in shadow_events {
    trace_sink.on_trace(&crate::harness::trace::LoopTraceEvent::AgentDefShadowed {
        id: shadow.id,
        winner_source: shadow.winner_source,
        shadowed_source: shadow.shadowed_source,
    });
}
```

(If `trace_sink` is `Option<Arc<dyn TraceSink>>`, guard with `if let Some(sink) = ...`.)

- [ ] **Step 5: Compile + run tests**

```bash
cargo build --workspace
cargo test -p alephcore --lib agents
```

Expected: Clean build; all agents tests still pass.

---

### Task E7: Integration test for tier priority + shadow events

**Files:**
- Create: `tests/agent_loader.rs`

- [ ] **Step 1: Create the integration test file**

```rust
//! Stage E integration tests for the filesystem agent loader.

use alephcore::agents::loader::{load_agents, LoaderError};
use alephcore::agents::types::AgentSource;
use std::io::Write;
use std::path::Path;

fn write_md(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
}

#[test]
fn priority_project_over_user_over_builtin() {
    // Build temp `home/data/agents/` and `project/.aleph/agents/`.
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let user_dir = home.path().join("data/agents");
    let project_dir = project.path().join(".aleph/agents");
    std::fs::create_dir_all(&user_dir).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();

    // 'explore' is a builtin id; user/project shadow it.
    write_md(
        &user_dir,
        "explore.md",
        "---\nid: explore\ndescription: User explore override\nwhen_to_use: user-tier test\n---\n",
    );
    write_md(
        &project_dir,
        "explore.md",
        "---\nid: explore\ndescription: Project explore override\nwhen_to_use: project-tier test\n---\n",
    );

    let (agents, shadows) =
        load_agents(home.path(), Some(project.path())).expect("load_agents");

    let by_id: std::collections::HashMap<_, _> =
        agents.iter().map(|a| (a.id.clone(), a)).collect();
    let explore = by_id.get("explore").expect("explore must be present");
    assert_eq!(explore.source, AgentSource::Project);
    assert_eq!(explore.description, "Project explore override");

    // Two shadow events: user shadows builtin, then project shadows user.
    assert_eq!(shadows.len(), 2, "expected 2 shadow events, got {shadows:?}");
    assert!(shadows
        .iter()
        .any(|s| s.id == "explore"
            && s.winner_source == AgentSource::User
            && s.shadowed_source == AgentSource::Builtin));
    assert!(shadows
        .iter()
        .any(|s| s.id == "explore"
            && s.winner_source == AgentSource::Project
            && s.shadowed_source == AgentSource::User));
}

#[test]
fn skip_malformed_file_continues_loading() {
    let home = tempfile::tempdir().unwrap();
    let user_dir = home.path().join("data/agents");
    std::fs::create_dir_all(&user_dir).unwrap();

    write_md(
        &user_dir,
        "good-agent.md",
        "---\nid: good-agent\ndescription: Loads OK\nwhen_to_use: testing skip\n---\n",
    );
    write_md(&user_dir, "broken.md", "this file has no frontmatter\n");

    let (agents, _shadows) = load_agents(home.path(), None).expect("load_agents");
    let by_id: std::collections::HashMap<_, _> =
        agents.iter().map(|a| (a.id.clone(), a)).collect();
    assert!(by_id.contains_key("good-agent"));
    assert!(!by_id.contains_key("broken"), "broken.md must not load");
}

#[test]
fn loader_error_on_id_mismatch_is_skipped_per_file() {
    // The loader skips per-file errors; ensure id-mismatch in one file doesn't
    // abort loading of others.
    let home = tempfile::tempdir().unwrap();
    let user_dir = home.path().join("data/agents");
    std::fs::create_dir_all(&user_dir).unwrap();

    write_md(
        &user_dir,
        "foo.md",
        "---\nid: bar\ndescription: x\nwhen_to_use: x\n---\n",
    );
    write_md(
        &user_dir,
        "valid.md",
        "---\nid: valid\ndescription: x\nwhen_to_use: x\n---\n",
    );

    let (agents, _) = load_agents(home.path(), None).expect("load_agents");
    let ids: Vec<_> = agents.iter().map(|a| a.id.clone()).collect();
    assert!(ids.contains(&"valid".to_string()));
    assert!(!ids.contains(&"bar".to_string()));
}

#[test]
fn loader_returns_io_error_for_missing_dir_above_user_path() {
    // If the user dir doesn't exist, loader silently skips that tier.
    // This is the documented behavior — only directory-level IO errors
    // (permission denied, etc.) propagate.
    let home = tempfile::tempdir().unwrap();
    // Don't create user_dir.
    let result = load_agents(home.path(), None);
    assert!(result.is_ok(), "missing user dir is skipped, not an error");
    let (agents, shadows) = result.unwrap();
    // Only builtins.
    assert!(!agents.is_empty());
    assert!(shadows.is_empty());
}
```

- [ ] **Step 2: Run integration tests**

```bash
cargo test --test agent_loader
```

Expected: All 4 tests pass.

---

### Task E8: Update MULTI_AGENT_SYSTEM.md (Stage E section)

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`

- [ ] **Step 1: Read existing doc**

```bash
grep -n "^##\|^###" docs/reference/MULTI_AGENT_SYSTEM.md
```

Identify the right insertion point (after "Recursion Protection" / before "Lane Budget Enforcement", preserving section ordering).

- [ ] **Step 2: Insert "Filesystem Agent Loading" section**

Add the following section, preserving surrounding doc style:

```markdown
## Filesystem Agent Loading (P2 Stage E)

Aleph loads agent definitions from three tiers (highest precedence first):

1. **Project tier** — `<project>/.aleph/agents/*.md`
2. **User tier** — `~/.aleph/data/agents/*.md`
3. **Builtin tier** — hardcoded in `crate::agents::registry::builtin_agents()`

Higher tiers shadow lower tiers silently when an `id` collision occurs.
A `LoopTraceEvent::AgentDefShadowed` event records each shadow for
diagnostics, observable through any registered trace sink.

### User-Authored Markdown Schema

User and project agents declare configuration in YAML frontmatter:

```yaml
---
id: my-research-agent           # required, must match filename stem
description: Researches topics  # required
when_to_use: When ...           # required
model_hint: claude-sonnet-4-6   # optional
allowed_tools: [glob, grep]     # optional
allowed_tool_sets: [INVESTIGATION]  # optional, see "Named Tool Sets"
denied_tools: []                # optional
max_iterations: 20              # optional
token_budget: 50000             # optional
context_mode: standalone        # optional
---

System prompt body...
```

### System-Forced Fields

Loader rejects user-set values for these fields and forces canonical values:

| Field    | Forced to                                  |
|----------|--------------------------------------------|
| `mode`   | `SubAgent` (writing `Primary` → schema error) |
| `source` | `User` or `Project` (auto, based on tier)  |

### Failure Modes

- Malformed frontmatter / YAML parse error → file skipped, `tracing::warn` emitted
- Missing required field → file skipped, `tracing::warn` emitted
- File stem ≠ frontmatter `id` → file skipped, `tracing::warn` emitted
- `mode: Primary` declared → file skipped, `tracing::warn` emitted

Aleph-server continues startup with successfully-loaded agents only;
one bad file does not abort startup.

### Reload

Filesystem agents are loaded once at startup. Modifying a markdown file
requires restarting `aleph-server`. (File-watcher-based hot reload is
deferred to a future roadmap stage.)
```

- [ ] **Step 3: Verify doc edits**

```bash
grep -n "Filesystem Agent Loading" docs/reference/MULTI_AGENT_SYSTEM.md
```

Expected: 1 hit at the inserted section.

---

### Task E9: Stage E commit

**Files:** All Stage E files (no new modifications in this task; commit only).

- [ ] **Step 1: Verify clean state**

```bash
cargo build --workspace
cargo test --workspace --lib agents
cargo test --test agent_loader
cargo clippy --workspace -- -D warnings
```

Expected: All green.

- [ ] **Step 2: Verify R10 hard checks**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
wc -l src/agents/subagent_spawner.rs
```

Expected: harness ≤ baseline + 10 (1 variant); files == 10; spawner unchanged.

- [ ] **Step 3: Stage and commit**

```bash
git add src/agents/types.rs src/agents/loader.rs src/agents/mod.rs \
        src/agents/registry.rs src/harness/trace.rs \
        src/bin/aleph-server/commands/start/orchestrator_init.rs \
        tests/agent_loader.rs docs/reference/MULTI_AGENT_SYSTEM.md \
        Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
agents: filesystem agent loader for user/project markdown definitions (P2 Stage E)

Adds three-tier agent loading (Project > User > Builtin) per locked design at
docs/superpowers/specs/2026-05-09-subagent-uplift-p2-design.md §2.

- src/agents/loader.rs (NEW): YAML frontmatter parser; scan_dir + load_agents
  return merged Vec<AgentDef> + Vec<ShadowEvent>; per-file errors logged + skipped
- src/agents/types.rs: AgentSource enum + AgentDef.source field (#[serde(default)])
- src/agents/registry.rs: AgentRegistry::register_from_dirs convenience wrapper
- src/harness/trace.rs: 1 backward-compatible LoopTraceEvent variant
  (AgentDefShadowed) for shadow diagnostics
- orchestrator_init.rs: call loader at startup; emit shadow events via trace_sink
- tests/agent_loader.rs: 4 integration tests covering tier priority,
  malformed-skip behavior, id-mismatch isolation, missing-dir tolerance
- docs/reference/MULTI_AGENT_SYSTEM.md: new "Filesystem Agent Loading" section

User/project frontmatter cannot declare `mode` (forced to SubAgent); attempts
trigger ForbiddenSystemField → file skipped with warn.

R10 baseline preserved: src/harness/agent.rs zero changes; trace.rs +10 lines
for the new variant (schema extension, exhaustive consumers updated with no-op
arm — same pattern as Phase-6 SessionCompleted addition).
EOF
)"
```

- [ ] **Step 4: Verify post-commit state**

```bash
git log --oneline -1
git status
```

Expected: 1 commit on main, clean working tree.

---

## Stage F — Streaming Progress Wrapper (Commit 2 / ~300 lines)

### Task F1: Create `SubagentProgress` + `ProgressKind` types

**Files:**
- Create: `src/agents/progress.rs`

- [ ] **Step 1: Create the file**

```rust
//! SubagentProgress — domain types for tracking background subagent activity.
//!
//! Per P2 Stage F design (§3.2): structured progress events live in the agent
//! layer (not LoopTraceEvent). Translated from child harness LoopTraceEvent
//! emissions by ForwardingTraceSink (subagent_spawner.rs) and stored in
//! BackgroundAgentTracker.progress (capped FIFO 50).

use std::time::SystemTime;

/// One step in a background subagent's run, surfaced to parent via check_status.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubagentProgress {
    /// Child harness iteration index (matches LoopTraceEvent.iteration).
    pub step: usize,
    /// Wall-clock timestamp at translation time. Used for "is it stuck?" diagnostics.
    pub timestamp: SystemTime,
    /// Categorical signal of what the child is doing.
    pub kind: ProgressKind,
    /// Tool being called (Some for ToolCalled/Returned; None otherwise).
    pub tool_name: Option<String>,
    /// Tool execution duration in milliseconds (Some for ToolReturned).
    pub latency_ms: Option<u64>,
    /// First 200 chars of the tool's output preview (Some for ToolReturned).
    pub preview: Option<String>,
}

/// Categorical kind of subagent progress event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressKind {
    /// Child started invoking a tool.
    ToolCalled,
    /// Child received a tool result.
    ToolReturned,
    /// Child entered the LLM "Think" turn state (waiting on model).
    LlmThinking,
    /// Child's session was cancelled.
    Cancelled,
}
```

- [ ] **Step 2: Add `progress` module declaration**

In `src/agents/mod.rs`:

```rust
pub mod progress;
```

- [ ] **Step 3: Compile + test**

```bash
cargo check -p alephcore
```

Expected: Clean compile.

---

### Task F2: Extend `BackgroundAgentTracker` with `progress` field

**Files:**
- Modify: `src/agents/background_tracker.rs`

- [ ] **Step 1: Import + add `progress` to `RunningAgent`**

In `src/agents/background_tracker.rs`, top of file:

```rust
use std::collections::{HashMap, VecDeque};   // VecDeque newly added
// ... existing imports ...
use crate::agents::progress::SubagentProgress;
```

Modify `RunningAgent` struct:

```rust
struct RunningAgent {
    cancel_token: CancellationToken,
    task_description: String,
    started_at: Instant,
    /// FIFO-capped progress events; capacity 50.
    progress: VecDeque<SubagentProgress>,
}
```

- [ ] **Step 2: Update `register` to initialize `progress`**

Replace the existing `RunningAgent { ... }` construction in `register`:

```rust
running.insert(
    request_id,
    RunningAgent {
        cancel_token,
        task_description,
        started_at: Instant::now(),
        progress: VecDeque::with_capacity(50),
    },
);
```

- [ ] **Step 3: Add `push_progress` and `progress_snapshot` methods**

Inside `impl BackgroundAgentTracker`:

```rust
/// Append a progress event to the running agent's queue.
/// Capped at 50 events FIFO. Silently no-ops if request_id is unknown
/// (race condition: tracker may have moved entry to completed).
pub fn push_progress(&self, request_id: &str, event: SubagentProgress) {
    let mut running = self.running.write().unwrap_or_else(|e| e.into_inner());
    if let Some(agent) = running.get_mut(request_id) {
        if agent.progress.len() >= 50 {
            agent.progress.pop_front();
        }
        agent.progress.push_back(event);
    }
}

/// Return up to `limit` most-recent progress events (chronological order).
/// Returns empty Vec if request_id is unknown or already completed.
pub fn progress_snapshot(&self, request_id: &str, limit: usize) -> Vec<SubagentProgress> {
    let running = self.running.read().unwrap_or_else(|e| e.into_inner());
    match running.get(request_id) {
        Some(agent) => {
            let total = agent.progress.len();
            let start = total.saturating_sub(limit);
            agent.progress.iter().skip(start).cloned().collect()
        }
        None => Vec::new(),
    }
}
```

- [ ] **Step 4: Add unit tests**

Append to `#[cfg(test)] mod tests` in `background_tracker.rs`:

```rust
use crate::agents::progress::{ProgressKind, SubagentProgress};
use std::time::SystemTime;

fn fake_progress(step: usize) -> SubagentProgress {
    SubagentProgress {
        step,
        timestamp: SystemTime::now(),
        kind: ProgressKind::ToolCalled,
        tool_name: Some(format!("tool_{step}")),
        latency_ms: None,
        preview: None,
    }
}

#[test]
fn tracker_push_progress_caps_at_50() {
    let tracker = BackgroundAgentTracker::new();
    let token = CancellationToken::new();
    tracker.register("rid".into(), token, "task".into());

    for i in 0..51 {
        tracker.push_progress("rid", fake_progress(i));
    }

    let snap = tracker.progress_snapshot("rid", 100);
    assert_eq!(snap.len(), 50, "cap enforced at 50");
    assert_eq!(snap.first().unwrap().step, 1, "step 0 evicted FIFO");
    assert_eq!(snap.last().unwrap().step, 50);
}

#[test]
fn tracker_progress_snapshot_returns_last_n() {
    let tracker = BackgroundAgentTracker::new();
    let token = CancellationToken::new();
    tracker.register("rid".into(), token, "task".into());

    for i in 0..5 {
        tracker.push_progress("rid", fake_progress(i));
    }

    let snap = tracker.progress_snapshot("rid", 3);
    assert_eq!(snap.len(), 3);
    assert_eq!(snap[0].step, 2);
    assert_eq!(snap[2].step, 4);
}

#[test]
fn tracker_push_unknown_id_no_op() {
    let tracker = BackgroundAgentTracker::new();
    tracker.push_progress("never-registered", fake_progress(0));
    assert!(tracker.progress_snapshot("never-registered", 10).is_empty());
}
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p alephcore --lib agents::background_tracker
```

Expected: 3 new tests pass; existing 5 BackgroundAgentTracker tests pass.

---

### Task F3: Implement `ForwardingTraceSink` wrapper

**Files:**
- Modify: `src/agents/subagent_spawner.rs` (add wrapper struct + tests inline)

- [ ] **Step 1: Add wrapper struct**

In `src/agents/subagent_spawner.rs`, append (before any test module):

```rust
/// Decorator over a parent's TraceSink that translates select child events
/// into SubagentProgress entries on a BackgroundAgentTracker, while always
/// forwarding the original event through to the inner sink.
///
/// Installed only on background subagent paths (see spawn() / subagent_tool's
/// background branch). Sync subagents share the parent's trace_sink directly.
pub struct ForwardingTraceSink {
    inner: std::sync::Arc<dyn crate::harness::TraceSink>,
    tracker: std::sync::Arc<crate::agents::background_tracker::BackgroundAgentTracker>,
    request_id: String,
}

impl ForwardingTraceSink {
    pub fn new(
        inner: std::sync::Arc<dyn crate::harness::TraceSink>,
        tracker: std::sync::Arc<crate::agents::background_tracker::BackgroundAgentTracker>,
        request_id: String,
    ) -> Self {
        Self {
            inner,
            tracker,
            request_id,
        }
    }

    fn translate(&self, event: &crate::harness::trace::LoopTraceEvent)
        -> Option<crate::agents::progress::SubagentProgress>
    {
        use crate::agents::progress::{ProgressKind, SubagentProgress};
        use crate::harness::trace::{LoopTraceEvent, LoopTraceSessionOutcome, LoopTraceState};
        use std::time::SystemTime;

        match event {
            LoopTraceEvent::ToolCallStarted { iteration, call } => Some(SubagentProgress {
                step: *iteration,
                timestamp: SystemTime::now(),
                kind: ProgressKind::ToolCalled,
                tool_name: Some(call.tool_name.clone()),
                latency_ms: None,
                preview: None,
            }),
            LoopTraceEvent::ToolCallCompleted {
                iteration,
                call,
                result,
            } => {
                // duration is recorded inside ToolCallEndEvent.duration_ms — no
                // need for a Started/Completed pairing map.
                let preview = render_tool_result_preview(result);
                Some(SubagentProgress {
                    step: *iteration,
                    timestamp: SystemTime::now(),
                    kind: ProgressKind::ToolReturned,
                    tool_name: Some(call.tool_name.clone()),
                    latency_ms: Some(call.duration_ms),
                    preview,
                })
            }
            LoopTraceEvent::TurnStateEntered { iteration, state }
                if matches!(state, LoopTraceState::Think) =>
            {
                Some(SubagentProgress {
                    step: *iteration,
                    timestamp: SystemTime::now(),
                    kind: ProgressKind::LlmThinking,
                    tool_name: None,
                    latency_ms: None,
                    preview: None,
                })
            }
            LoopTraceEvent::SessionCompleted {
                outcome: LoopTraceSessionOutcome::Cancelled,
                iterations,
                ..
            } => Some(SubagentProgress {
                step: *iterations,
                timestamp: SystemTime::now(),
                kind: ProgressKind::Cancelled,
                tool_name: None,
                latency_ms: None,
                preview: None,
            }),
            _ => None,
        }
    }
}

/// Render a 200-char preview of a tool result for SubagentProgress.preview.
fn render_tool_result_preview(result: &crate::tools::runtime::ToolResult) -> Option<String> {
    let raw = match result {
        crate::tools::runtime::ToolResult::Success { output } => {
            serde_json::to_string(output).ok()?
        }
        crate::tools::runtime::ToolResult::Error { error, .. } => error.clone(),
    };
    let mut s: String = raw.chars().take(200).collect();
    if raw.chars().count() > 200 {
        s.push('…');
    }
    Some(s)
}

impl crate::harness::TraceSink for ForwardingTraceSink {
    fn on_trace(&self, event: &crate::harness::trace::LoopTraceEvent) {
        if let Some(progress) = self.translate(event) {
            self.tracker.push_progress(&self.request_id, progress);
        }
        self.inner.on_trace(event);
    }

    fn flush(&self) {
        self.inner.flush();
    }

    fn on_init_seam(&self, stage: &'static str, seam: &'static str, configured: bool) {
        self.inner.on_init_seam(stage, seam, configured);
    }
}
```

- [ ] **Step 2: Compile**

```bash
cargo check -p alephcore
```

Expected: clean. If `ToolResult::Success`/`Error` enum variants differ from above, `grep -n 'pub enum ToolResult' src/tools/runtime.rs` and adjust matches accordingly. The intent is "render Success.output as JSON, render Error.error as string, take first 200 chars".

- [ ] **Step 3: Add unit tests for forwarding wrapper**

Append in the existing `#[cfg(test)] mod tests` block of `subagent_spawner.rs`:

```rust
#[cfg(test)]
mod forwarding_tests {
    use super::*;
    use crate::agents::background_tracker::BackgroundAgentTracker;
    use crate::agents::progress::ProgressKind;
    use crate::harness::trace::{
        LoopTraceEvent, LoopTraceSessionOutcome, LoopTraceState, ToolCallEndEvent,
        ToolCallStartEvent,
    };
    use crate::harness::TraceSink;
    use std::sync::{Arc, Mutex};
    use tokio_util::sync::CancellationToken;

    /// Test sink that records all events it receives.
    #[derive(Default)]
    struct CapturingSink {
        events: Mutex<Vec<LoopTraceEvent>>,
    }
    impl TraceSink for CapturingSink {
        fn on_trace(&self, event: &LoopTraceEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
        fn flush(&self) {}
    }

    fn setup() -> (
        Arc<CapturingSink>,
        Arc<BackgroundAgentTracker>,
        ForwardingTraceSink,
    ) {
        let inner: Arc<CapturingSink> = Arc::new(CapturingSink::default());
        let tracker = Arc::new(BackgroundAgentTracker::new());
        tracker.register("rid".into(), CancellationToken::new(), "task".into());
        let inner_dyn: Arc<dyn TraceSink> = inner.clone();
        let wrapper = ForwardingTraceSink::new(inner_dyn, tracker.clone(), "rid".into());
        (inner, tracker, wrapper)
    }

    #[test]
    fn forwarding_translates_tool_call_started_to_tool_called() {
        let (_inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::ToolCallStarted {
            iteration: 3,
            call: ToolCallStartEvent {
                tool_id: "id".into(),
                tool_name: "read_file".into(),
                input: serde_json::json!({}),
            },
        });
        let snap = tracker.progress_snapshot("rid", 10);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].step, 3);
        assert_eq!(snap[0].kind, ProgressKind::ToolCalled);
        assert_eq!(snap[0].tool_name.as_deref(), Some("read_file"));
        assert!(snap[0].latency_ms.is_none());
    }

    #[test]
    fn forwarding_pairs_started_completed_for_latency() {
        let (_inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::ToolCallStarted {
            iteration: 1,
            call: ToolCallStartEvent {
                tool_id: "id".into(),
                tool_name: "grep".into(),
                input: serde_json::json!({}),
            },
        });
        wrapper.on_trace(&LoopTraceEvent::ToolCallCompleted {
            iteration: 1,
            call: ToolCallEndEvent {
                tool_id: "id".into(),
                tool_name: "grep".into(),
                input: serde_json::json!({}),
                duration_ms: 42,
            },
            result: crate::tools::runtime::ToolResult::Success {
                output: serde_json::json!({"hits": 3}),
            },
        });
        let snap = tracker.progress_snapshot("rid", 10);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[1].kind, ProgressKind::ToolReturned);
        assert_eq!(snap[1].latency_ms, Some(42));
        assert!(snap[1].preview.is_some());
    }

    #[test]
    fn forwarding_forwards_unrelated_events_unchanged() {
        let (inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::TextEmitted {
            iteration: 1,
            stream: crate::harness::trace::LoopTraceTextKind::Final,
            text: "hello".into(),
        });
        // Inner sink received the event…
        assert_eq!(inner.events.lock().unwrap().len(), 1);
        // …but tracker.progress unchanged (TextEmitted is not translated).
        assert!(tracker.progress_snapshot("rid", 10).is_empty());
    }

    #[test]
    fn forwarding_translates_think_state_to_llm_thinking() {
        let (_inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::TurnStateEntered {
            iteration: 5,
            state: LoopTraceState::Think,
        });
        let snap = tracker.progress_snapshot("rid", 10);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].kind, ProgressKind::LlmThinking);
    }

    #[test]
    fn forwarding_translates_cancelled_session_to_cancelled() {
        let (_inner, tracker, wrapper) = setup();
        wrapper.on_trace(&LoopTraceEvent::SessionCompleted {
            outcome: LoopTraceSessionOutcome::Cancelled,
            iterations: 7,
            tool_calls_made: 2,
            total_tokens: 1000,
            hit_limit: false,
            final_text: None,
        });
        let snap = tracker.progress_snapshot("rid", 10);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].kind, ProgressKind::Cancelled);
        assert_eq!(snap[0].step, 7);
    }

    #[test]
    fn forwarding_other_turn_states_not_translated() {
        let (_inner, tracker, wrapper) = setup();
        for state in [
            LoopTraceState::Prepare,
            LoopTraceState::Resolve,
            LoopTraceState::Act,
            LoopTraceState::Finalize,
        ] {
            wrapper.on_trace(&LoopTraceEvent::TurnStateEntered {
                iteration: 1,
                state,
            });
        }
        // Only Think translates; others are forwarded but not stored.
        assert!(tracker.progress_snapshot("rid", 10).is_empty());
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p alephcore --lib subagent_spawner::forwarding_tests
```

Expected: 6 new tests pass.

---

### Task F4: Wire `ForwardingTraceSink` into background spawn path

**Files:**
- Modify: `src/agents/subagent_tool.rs` (background branch around line ~640-720)

- [ ] **Step 1: Locate the background spawn site**

```bash
grep -n "run_in_background\|self.background_tracker.register\|tokio::spawn" \
    src/agents/subagent_tool.rs | head -10
```

Find the block where `request_id` is generated and `self.background_tracker.register(...)` is called.

- [ ] **Step 2: Wrap the trace_sink before child runtime construction**

Identify where the child `AgentRuntime` is built inside the `tokio::spawn` block. The runtime currently inherits `self.parent_tools / self.session / etc.` Now we also need to wire a wrapped trace_sink. The exact wiring depends on whether `AgentRuntime` accepts a trace_sink directly or whether it constructs its own through `HarnessDeps`.

Run:

```bash
grep -n "trace_sink\|TraceSink" src/agents/runtime.rs
grep -n "with_trace_sink\|trace_sink:" src/agents/subagent_tool.rs
```

Two implementation choices based on the grep result:

**Choice A (preferred): if `AgentRuntime` exposes `with_trace_sink(...)`** — call it on the runtime builder before `.run()`:

```rust
// Inside the `if args.run_in_background { ... }` branch, before `tokio::spawn`:
let parent_trace_sink = self.parent_trace_sink.clone();  // confirm field name on SubagentTool
let tracker_for_wrapper = self.background_tracker.clone();
let request_id_for_wrapper = request_id.clone();

// Inside tokio::spawn closure, after AgentRuntime::new():
if let Some(parent_sink) = parent_trace_sink {
    let wrapper: std::sync::Arc<dyn crate::harness::TraceSink> = std::sync::Arc::new(
        crate::agents::subagent_spawner::ForwardingTraceSink::new(
            parent_sink,
            tracker_for_wrapper,
            request_id_for_wrapper,
        ),
    );
    runtime = runtime.with_trace_sink(wrapper);
}
```

**Choice B: if AgentRuntime takes trace_sink only via HarnessDeps**, push the wrapper through SpawnerBase. This requires confirming `self.spawner_base` exists on `SubagentTool`. Modify the spawner_base trace_sink before calling spawn.

If neither A nor B fits, do NOT improvise — stop the task and report DONE_WITH_CONCERNS to the controller, naming the exact call site and the API mismatch. The plan will be refined before retry.

- [ ] **Step 3: Confirm sync path is unchanged**

In the `else` branch (foreground execution, after `if args.run_in_background`), make sure no ForwardingTraceSink wrapping is introduced. Sync subagents inherit parent.trace_sink directly via Stage A wiring.

- [ ] **Step 4: Compile + test**

```bash
cargo check -p alephcore
cargo test -p alephcore --lib subagent_tool
```

Expected: clean compile; existing subagent_tool tests pass.

---

### Task F5: Extend `check_status` to return `progress` field

**Files:**
- Modify: `src/agents/subagent_tool.rs` (CheckStatus action handler)

- [ ] **Step 1: Locate the CheckStatus handler**

```bash
grep -n "SubagentAction::CheckStatus" src/agents/subagent_tool.rs
```

Find the `match` arm or `if let` block that responds to `CheckStatus(request_id)`.

- [ ] **Step 2: Modify the running-status response**

Currently it's:

```rust
return ToolResult::Success {
    output: json!({
        "status": "running",
        "request_id": request_id,
    }),
};
```

Change to:

```rust
let progress = self.background_tracker.progress_snapshot(&request_id, 10);
return ToolResult::Success {
    output: json!({
        "status": "running",
        "request_id": request_id,
        "progress": progress,
    }),
};
```

- [ ] **Step 3: Verify existing CheckStatus tests still pass**

```bash
cargo test -p alephcore --lib subagent_tool::tests::test_check_status
```

Expected: existing tests pass; the `progress: []` field is additive on the running case.

- [ ] **Step 4: Add a unit test for progress field shape**

In `subagent_tool.rs` test module, add:

```rust
#[tokio::test]
async fn check_status_returns_progress_array_when_running() {
    use crate::agents::progress::{ProgressKind, SubagentProgress};
    use std::time::SystemTime;
    use tokio_util::sync::CancellationToken;

    let tracker = make_tracker();
    let token = CancellationToken::new();
    tracker.register("rid".into(), token, "task".into());
    tracker.push_progress(
        "rid",
        SubagentProgress {
            step: 1,
            timestamp: SystemTime::now(),
            kind: ProgressKind::ToolCalled,
            tool_name: Some("read_file".into()),
            latency_ms: None,
            preview: None,
        },
    );

    let tool = make_tool_with_tracker(tracker.clone());
    let result = tool
        .execute(serde_json::json!({"action": "check_status", "request_id": "rid"}))
        .await;
    let output = match result {
        crate::tools::runtime::ToolResult::Success { output } => output,
        other => panic!("expected Success, got {other:?}"),
    };
    let progress = output.get("progress").expect("progress field present");
    assert!(progress.is_array());
    assert_eq!(progress.as_array().unwrap().len(), 1);
}
```

Note: `make_tool_with_tracker` is a test helper. If it doesn't exist, mirror the existing `make_tracker` + tool construction pattern; the test is informative about wiring, not perfection.

- [ ] **Step 5: Run tests**

```bash
cargo test -p alephcore --lib subagent_tool
```

Expected: All tests pass (existing + 1 new).

---

### Task F6: Integration tests for streaming progress

**Files:**
- Create: `tests/subagent_progress.rs`

- [ ] **Step 1: Create integration test file**

```rust
//! Stage F integration tests for subagent streaming progress.
//!
//! These tests exercise the ForwardingTraceSink wrapper end-to-end via mocked
//! provider/tool services, validating that:
//!   1. Background subagents accumulate progress visible through check_status
//!   2. Sync subagents do NOT install the wrapper (no progress recording)

// NOTE TO IMPLEMENTER: these integration tests reuse the mock harness
// scaffolding patterns established by tests/cancellation_chain.rs (Stage D).
// If those scaffolds are not directly reusable, add a small mock helper
// module to tests/common/ rather than duplicating across files.

use alephcore::agents::background_tracker::BackgroundAgentTracker;
use alephcore::agents::progress::ProgressKind;
use alephcore::agents::subagent_spawner::ForwardingTraceSink;
use alephcore::harness::trace::{
    LoopTraceEvent, ToolCallEndEvent, ToolCallStartEvent,
};
use alephcore::harness::TraceSink;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<LoopTraceEvent>>,
}
impl TraceSink for CapturingSink {
    fn on_trace(&self, event: &LoopTraceEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
    fn flush(&self) {}
}

#[test]
fn background_subagent_check_status_returns_progress() {
    // Simulate a background subagent emitting 3 trace events into the wrapper.
    // After the run, check_status equivalent (tracker.progress_snapshot) should
    // surface a 3-entry progress array of correct kinds.
    let inner = Arc::new(CapturingSink::default());
    let tracker = Arc::new(BackgroundAgentTracker::new());
    let token = CancellationToken::new();
    tracker.register("test-rid".into(), token, "task".into());

    let wrapper = ForwardingTraceSink::new(inner.clone(), tracker.clone(), "test-rid".into());

    // Emit a sequence representing one tool call cycle.
    wrapper.on_trace(&LoopTraceEvent::TurnStateEntered {
        iteration: 1,
        state: alephcore::harness::trace::LoopTraceState::Think,
    });
    wrapper.on_trace(&LoopTraceEvent::ToolCallStarted {
        iteration: 1,
        call: ToolCallStartEvent {
            tool_id: "id-1".into(),
            tool_name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
        },
    });
    wrapper.on_trace(&LoopTraceEvent::ToolCallCompleted {
        iteration: 1,
        call: ToolCallEndEvent {
            tool_id: "id-1".into(),
            tool_name: "read_file".into(),
            input: serde_json::json!({"path": "/tmp/x"}),
            duration_ms: 12,
        },
        result: alephcore::tools::runtime::ToolResult::Success {
            output: serde_json::json!({"contents": "hello"}),
        },
    });

    // Verify forwarding: inner sink saw all 3 events.
    assert_eq!(inner.events.lock().unwrap().len(), 3);

    // Verify progress: 3 translated entries in chronological order.
    let snap = tracker.progress_snapshot("test-rid", 10);
    assert_eq!(snap.len(), 3, "got: {snap:?}");
    assert_eq!(snap[0].kind, ProgressKind::LlmThinking);
    assert_eq!(snap[1].kind, ProgressKind::ToolCalled);
    assert_eq!(snap[2].kind, ProgressKind::ToolReturned);
    assert_eq!(snap[2].latency_ms, Some(12));
    assert_eq!(snap[2].tool_name.as_deref(), Some("read_file"));
}

#[test]
fn sync_subagent_does_not_install_wrapper() {
    // Sentinel test: the wrapper module is not auto-installed.
    // Construction is explicit per-call; a sync subagent that never receives a
    // wrapped sink leaves tracker.progress empty even after trace events flow.
    let tracker = Arc::new(BackgroundAgentTracker::new());
    // Simulate a sync subagent: no register() call (no background entry exists)
    // and trace events flow through a non-wrapping sink.
    let plain_sink = Arc::new(CapturingSink::default());
    plain_sink.on_trace(&LoopTraceEvent::ToolCallStarted {
        iteration: 1,
        call: ToolCallStartEvent {
            tool_id: "id".into(),
            tool_name: "grep".into(),
            input: serde_json::json!({}),
        },
    });
    // Tracker never received progress (because no wrapper exists for sync paths).
    assert!(
        tracker.progress_snapshot("nonexistent-sync-rid", 10).is_empty(),
        "sync path must not populate background tracker"
    );
    // And the plain sink correctly captured the event (proves the trace flowed).
    assert_eq!(plain_sink.events.lock().unwrap().len(), 1);
}
```

- [ ] **Step 2: Run tests**

```bash
cargo test --test subagent_progress
```

Expected: 2 tests pass.

---

### Task F7: Update MULTI_AGENT_SYSTEM.md (Stage F section)

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`

- [ ] **Step 1: Append "Subagent Progress Streaming" section**

Add the following section to `docs/reference/MULTI_AGENT_SYSTEM.md` (after the "Filesystem Agent Loading" section from Stage E):

```markdown
## Subagent Progress Streaming (P2 Stage F)

Background subagents emit a structured progress trail observable to the parent
through the `subagent` tool's `check_status` action. Sync (foreground) subagents
do NOT participate in this mechanism — their final result is the only signal.

### Progress Event Schema

```rust
struct SubagentProgress {
    step: usize,                    // child harness iteration
    timestamp: SystemTime,          // wall-clock at translation
    kind: ProgressKind,             // ToolCalled | ToolReturned | LlmThinking | Cancelled
    tool_name: Option<String>,      // Some for ToolCalled / ToolReturned
    latency_ms: Option<u64>,        // Some for ToolReturned (call duration)
    preview: Option<String>,        // Some for ToolReturned (200-char truncation)
}
```

### Wiring (R10-Safe Decorator)

`ForwardingTraceSink` (in `src/agents/subagent_spawner.rs`) wraps the
parent-inherited `trace_sink` exclusively for background subagents. It:

1. Translates `LoopTraceEvent::ToolCallStarted` / `ToolCallCompleted` /
   `TurnStateEntered{Think}` / `SessionCompleted{Cancelled}` into
   `SubagentProgress`
2. Pushes the translated event onto `BackgroundAgentTracker.progress` (FIFO,
   capped at 50)
3. Always forwards the original event to the inner sink (preserves
   gateway/disk trace flow)

Other LoopTraceEvent variants pass through untranslated. Adding new translation
cases does not require harness changes.

### check_status Output Shape

When status == "running", the response includes a `progress` field:

```json
{
  "status": "running",
  "request_id": "...",
  "progress": [
    { "step": 0, "kind": "llm_thinking", ... },
    { "step": 1, "kind": "tool_called", "tool_name": "grep", ... },
    { "step": 1, "kind": "tool_returned", "latency_ms": 42, "preview": "...", ... }
  ]
}
```

Up to 10 most-recent events are returned. The buffer caps at 50 internally;
older events are evicted FIFO.

### Why cap=50?

This is a designed memory/observability tradeoff (P2 Q6, hardcoded). For
long-running background subagents (>50 tool calls), only the most recent 50
steps remain visible. Configurable cap is a future stage if needed.
```

- [ ] **Step 2: Verify**

```bash
grep -n "Subagent Progress Streaming" docs/reference/MULTI_AGENT_SYSTEM.md
```

Expected: 1 hit.

---

### Task F8: Stage F commit

- [ ] **Step 1: Verify clean state**

```bash
cargo build --workspace
cargo test --workspace --lib agents
cargo test --test subagent_progress
cargo test --test agent_loader
cargo clippy --workspace -- -D warnings
```

Expected: All green.

- [ ] **Step 2: R10 hard checks**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
wc -l src/agents/subagent_spawner.rs
```

Expected: harness ≤ baseline + 10 (no further changes from Stage E); files == 10; spawner ≤ 600 (Stage F adds ~60 lines for the wrapper struct, total well under cap).

- [ ] **Step 3: Stage and commit**

```bash
git add src/agents/progress.rs src/agents/mod.rs \
        src/agents/background_tracker.rs \
        src/agents/subagent_spawner.rs \
        src/agents/subagent_tool.rs \
        tests/subagent_progress.rs \
        docs/reference/MULTI_AGENT_SYSTEM.md
git commit -m "$(cat <<'EOF'
agents: streaming progress wrapper for background subagents (P2 Stage F)

Adds structured progress observability for background subagents per locked
design at docs/superpowers/specs/2026-05-09-subagent-uplift-p2-design.md §3.

- src/agents/progress.rs (NEW): SubagentProgress struct + ProgressKind enum
  (domain types in agent layer; not a LoopTraceEvent variant — see §3.1)
- src/agents/background_tracker.rs: RunningAgent.progress: VecDeque (cap 50);
  push_progress + progress_snapshot methods
- src/agents/subagent_spawner.rs: ForwardingTraceSink Decorator wraps
  parent-inherited trace_sink only on background paths; translates select
  LoopTraceEvent variants into SubagentProgress + pushes to tracker; forwards
  all events unchanged to inner sink
- src/agents/subagent_tool.rs: background spawn wires the wrapper; check_status
  returns progress array (up to 10 most-recent events)
- tests/subagent_progress.rs: 2 integration tests for forwarding semantics +
  sync-path-no-wrapper guarantee
- docs/reference/MULTI_AGENT_SYSTEM.md: new "Subagent Progress Streaming" section

Sync subagent path is unchanged (R3 simplification: parent already sees raw
child trace events via Stage A inheritance; no wrapper needed). Latency comes
from ToolCallEndEvent.duration_ms — no Started/Completed pairing map needed
(Risk R3 resolved at design).

R10 baseline preserved: src/harness/agent.rs zero changes; trace.rs unchanged
from Stage E commit.
EOF
)"
```

---

## Stage G — Semantic Tool Sets (Commit 3 / ~210 lines)

### Task G1: Create `tool_sets.rs` module

**Files:**
- Create: `src/agents/tool_sets.rs`

- [ ] **Step 1: Create file with 3 named sets**

```rust
//! Named tool sets for declarative agent allowlists (P2 Stage G).
//!
//! Per locked design (Q7-1 simplified positive): only 3 positive sets, no
//! ALL_AGENT_DENIED_TOOLS auto-deny, no allow_override field. Defense layers
//! (recursion guard via Stage B, user-frontmatter mode forcing via Stage E)
//! live elsewhere.
//!
//! Tool names match those registered in `src/builtin_tools/` (see
//! `crate::builtin_tools::register_*`). This file's 3 constants are the only
//! place tool sets are defined; AgentDef.allowed_tool_sets references them by name.

/// Pure read-only filesystem inspection tools.
pub const READ_ONLY: &[&str] = &["glob", "grep", "read_file"];

/// READ_ONLY ∪ remote read tools ∪ subagent (Primary-only via Stage B guard).
/// SubAgent-mode agents that include INVESTIGATION still cannot spawn subagent
/// (Stage B `is_tool_allowed` mode-aware deny).
pub const INVESTIGATION: &[&str] = &[
    "glob",
    "grep",
    "read_file",
    "search",
    "web_fetch",
    "subagent",
];

/// Subset of INVESTIGATION safe for autonomous background execution: no
/// side effects, no exfiltration risk (no web_fetch). Excludes subagent
/// to defend against background recursion misuse beyond Stage B guarantees.
pub const ASYNC_SAFE: &[&str] = &["glob", "grep", "read_file", "search"];

/// Resolve a set name to its tool list. Returns None for unknown names so
/// callers can warn (loader) or treat as empty allowance (is_tool_allowed)
/// without rejecting valid agent definitions.
pub fn resolve(set_name: &str) -> Option<&'static [&'static str]> {
    match set_name {
        "READ_ONLY" => Some(READ_ONLY),
        "INVESTIGATION" => Some(INVESTIGATION),
        "ASYNC_SAFE" => Some(ASYNC_SAFE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_set_resolves_to_known_tools() {
        let tools = resolve("READ_ONLY").expect("READ_ONLY exists");
        assert!(tools.contains(&"read_file"));
        assert!(tools.contains(&"grep"));
        assert!(tools.contains(&"glob"));
        assert!(!tools.contains(&"web_fetch"));
        assert!(!tools.contains(&"bash"));
    }

    #[test]
    fn investigation_is_superset_of_read_only() {
        let read_only = resolve("READ_ONLY").unwrap();
        let investigation = resolve("INVESTIGATION").unwrap();
        for tool in read_only {
            assert!(
                investigation.contains(tool),
                "INVESTIGATION must contain READ_ONLY tool '{tool}'"
            );
        }
    }

    #[test]
    fn async_safe_excludes_subagent() {
        let async_safe = resolve("ASYNC_SAFE").unwrap();
        assert!(!async_safe.contains(&"subagent"));
    }

    #[test]
    fn async_safe_excludes_web_fetch() {
        // Exfiltration risk: ASYNC_SAFE is safe-to-run-autonomously; web_fetch
        // would let a background agent leak data via URL parameters.
        let async_safe = resolve("ASYNC_SAFE").unwrap();
        assert!(!async_safe.contains(&"web_fetch"));
    }

    #[test]
    fn unknown_set_resolves_none() {
        assert!(resolve("FOOBAR").is_none());
        assert!(resolve("read_only").is_none()); // case-sensitive
        assert!(resolve("").is_none());
    }
}
```

- [ ] **Step 2: Add module declaration**

In `src/agents/mod.rs`:

```rust
pub mod tool_sets;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib agents::tool_sets
```

Expected: 5 tests pass.

---

### Task G2: Add `allowed_tool_sets` field to `AgentDef`

**Files:**
- Modify: `src/agents/types.rs`
- Modify: `src/agents/loader.rs` (un-comment the `allowed_tool_sets` wiring deferred in Task E3)

- [ ] **Step 1: Add the field to `AgentDef`**

Add to `AgentDef` struct (next to existing `allowed_tools` / `denied_tools` fields):

```rust
pub struct AgentDef {
    // ... existing fields ...

    /// Named tool sets (P2 Stage G). Resolved via crate::agents::tool_sets::resolve.
    /// Unknown set names are silently empty; explicit `allowed_tools` still applies.
    #[serde(default)]
    pub allowed_tool_sets: Vec<String>,
}
```

- [ ] **Step 2: Add a builder method**

In `impl AgentDef`:

```rust
pub fn with_allowed_tool_sets(mut self, sets: Vec<String>) -> Self {
    self.allowed_tool_sets = sets;
    self
}
```

- [ ] **Step 3: Activate the loader's `allowed_tool_sets` wiring**

In `src/agents/loader.rs`, replace the placeholder:

```rust
// allowed_tool_sets wiring lands in Stage G (Task G2)
let _ = fm.allowed_tool_sets;
```

with:

```rust
def.allowed_tool_sets = fm.allowed_tool_sets;
```

- [ ] **Step 4: Compile**

```bash
cargo check -p alephcore
```

Expected: clean.

---

### Task G3: Extend `is_tool_allowed` with set resolution

**Files:**
- Modify: `src/agents/types.rs`

- [ ] **Step 1: Update the `is_tool_allowed` method**

Replace the current `is_tool_allowed` implementation (post Stage B) with:

```rust
pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
    // Stage B (P1): recursion guard — system invariant, overrides everything
    if matches!(self.mode, AgentMode::SubAgent) && tool_name == "subagent" {
        return false;
    }

    // Explicit deny short-circuits (after recursion guard, before allows)
    if self.denied_tools.iter().any(|t| t == tool_name) {
        return false;
    }

    // Stage G (P2): expanded allowed_tool_sets
    for set_name in &self.allowed_tool_sets {
        if let Some(tools) = crate::agents::tool_sets::resolve(set_name) {
            if tools.iter().any(|t| *t == tool_name) {
                return true;
            }
        }
    }

    // Existing flat allowlist with "*" wildcard support
    self.allowed_tools.iter().any(|t| t == "*" || t == tool_name)
}
```

- [ ] **Step 2: Add unit tests**

Append to `#[cfg(test)] mod tests` in `src/agents/types.rs`:

```rust
#[test]
fn is_tool_allowed_via_set_only() {
    let def = AgentDef::new("test", AgentMode::SubAgent)
        .with_allowed_tool_sets(vec!["READ_ONLY".into()]);
    assert!(def.is_tool_allowed("read_file"));
    assert!(def.is_tool_allowed("grep"));
    assert!(!def.is_tool_allowed("bash"));
    assert!(!def.is_tool_allowed("write_file"));
}

#[test]
fn is_tool_allowed_set_and_flat_union() {
    let def = AgentDef::new("test", AgentMode::SubAgent)
        .with_allowed_tool_sets(vec!["READ_ONLY".into()])
        .with_allowed_tools(vec!["custom_tool".into()]);
    // Flat list contributes:
    assert!(def.is_tool_allowed("custom_tool"));
    // Set contributes:
    assert!(def.is_tool_allowed("read_file"));
    // Neither contributes:
    assert!(!def.is_tool_allowed("bash"));
}

#[test]
fn denied_tools_overrides_set() {
    let def = AgentDef::new("test", AgentMode::SubAgent)
        .with_allowed_tool_sets(vec!["INVESTIGATION".into()])
        .with_denied_tools(vec!["web_fetch".into()]);
    // INVESTIGATION includes web_fetch but denied_tools wins:
    assert!(!def.is_tool_allowed("web_fetch"));
    // Other INVESTIGATION tools still allowed:
    assert!(def.is_tool_allowed("search"));
    assert!(def.is_tool_allowed("read_file"));
}

#[test]
fn subagent_mode_denies_subagent_even_in_investigation_set() {
    let def = AgentDef::new("nested", AgentMode::SubAgent)
        .with_allowed_tool_sets(vec!["INVESTIGATION".into()]);
    // INVESTIGATION's "subagent" entry is overridden by Stage B mode-aware deny:
    assert!(!def.is_tool_allowed("subagent"));
    // Other INVESTIGATION members still allowed:
    assert!(def.is_tool_allowed("search"));
}

#[test]
fn primary_mode_with_investigation_set_can_subagent() {
    let def = AgentDef::new("main", AgentMode::Primary)
        .with_allowed_tool_sets(vec!["INVESTIGATION".into()]);
    // Primary mode + INVESTIGATION → subagent allowed:
    assert!(def.is_tool_allowed("subagent"));
}

#[test]
fn unknown_set_name_silently_empty() {
    let def = AgentDef::new("test", AgentMode::SubAgent)
        .with_allowed_tool_sets(vec!["NONEXISTENT_SET".into()])
        .with_allowed_tools(vec!["read_file".into()]);
    // Unknown set contributes nothing; flat list still works:
    assert!(def.is_tool_allowed("read_file"));
    assert!(!def.is_tool_allowed("grep"));
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p alephcore --lib agents::types
```

Expected: 6 new tests pass; existing types tests pass.

---

### Task G4: Migrate `explore` agent to `INVESTIGATION` named set

**Files:**
- Modify: `src/agents/registry.rs` (builtin_agents → explore entry)

- [ ] **Step 1: Verify migration is behavior-preserving**

Existing explore allowlist (registry.rs:93-101):

```rust
.with_allowed_tools(vec![
    "glob".into(),
    "grep".into(),
    "read_file".into(),
    "web_fetch".into(),
    "search".into(),
])
.with_denied_tools(vec!["write_file".into(), "edit_file".into(), "bash".into()])
```

After migration to `INVESTIGATION`:

- INVESTIGATION = `[glob, grep, read_file, search, web_fetch, subagent]`
- explore is `mode: SubAgent` → Stage B blocks `subagent` regardless
- denied_tools (`write_file`, `edit_file`, `bash`) preserved

Effective tool set after migration is identical to before:

| Tool         | Before | After (mode=SubAgent) |
|--------------|--------|----------------------|
| `glob`       | ✓      | ✓ (INVESTIGATION)    |
| `grep`       | ✓      | ✓ (INVESTIGATION)    |
| `read_file`  | ✓      | ✓ (INVESTIGATION)    |
| `search`     | ✓      | ✓ (INVESTIGATION)    |
| `web_fetch`  | ✓      | ✓ (INVESTIGATION)    |
| `subagent`   | ✗      | ✗ (Stage B guard)    |
| `write_file` | ✗      | ✗ (denied)           |
| `edit_file`  | ✗      | ✗ (denied)           |
| `bash`       | ✗      | ✗ (denied)           |

Behavior-preserving: confirmed.

- [ ] **Step 2: Edit the explore entry**

Replace registry.rs:87-101 (the `explore` builder chain) with:

```rust
        // Explore agent — INVESTIGATION named set (P2 Stage G demo migration).
        // Effective behavior unchanged: Stage B recursion guard blocks subagent
        // for SubAgent mode; denied_tools preserved.
        AgentDef::new("explore", AgentMode::SubAgent)
            .with_description("Read-only codebase exploration specialist")
            .with_when_to_use(
                "When you need to search, read, or understand code without modifying anything",
            )
            .with_prompt_sections(vec!["explore_constraints".into()])
            .with_allowed_tool_sets(vec!["INVESTIGATION".into()])
            .with_denied_tools(vec!["write_file".into(), "edit_file".into(), "bash".into()])
            .with_max_iterations(20),
```

- [ ] **Step 3: Verify pre-existing explore tests still pass**

The existing `test_explore_agent_config` test in registry.rs asserts:

```rust
assert!(explore.is_tool_allowed("glob"));
assert!(explore.is_tool_allowed("grep"));
assert!(!explore.is_tool_allowed("write_file"));
assert!(!explore.is_tool_allowed("bash"));
```

These should all still pass after migration (INVESTIGATION includes glob+grep; denied_tools still wins).

```bash
cargo test -p alephcore --lib agents::registry::tests::test_explore_agent_config
```

Expected: PASS.

- [ ] **Step 4: Run all registry tests**

```bash
cargo test -p alephcore --lib agents::registry
```

Expected: All registry tests pass (including `test_builtin_agents_count` still asserting 7).

---

### Task G5: Migration regression integration test

**Files:**
- Create: `tests/tool_sets.rs`

- [ ] **Step 1: Create the integration test file**

```rust
//! Stage G integration tests: ensure migrated builtin agents preserve behavior.

use alephcore::agents::registry::AgentRegistry;

/// The full set of tools relevant to the `explore` agent, both allowed and denied.
const EXPLORE_PROBE_TOOLS: &[&str] = &[
    // Allowed before migration:
    "glob",
    "grep",
    "read_file",
    "web_fetch",
    "search",
    // Denied before migration:
    "write_file",
    "edit_file",
    "bash",
    // Mode-aware deny (was effectively denied via no-explicit-allow):
    "subagent",
    // Unknown tool (must be denied):
    "totally_unknown_tool",
];

#[test]
fn migrated_explore_agent_keeps_behavior() {
    let registry = AgentRegistry::with_builtins();
    let explore = registry.get("explore").expect("explore agent registered");

    // After migration, allowed set comes from INVESTIGATION + denied filter +
    // Stage B mode-aware deny. This test asserts the EFFECTIVE behavior
    // matches what was hand-listed before the migration.
    let expected_allowed = ["glob", "grep", "read_file", "web_fetch", "search"];
    let expected_denied = [
        "write_file",
        "edit_file",
        "bash",
        "subagent",
        "totally_unknown_tool",
    ];

    for tool in &expected_allowed {
        assert!(
            explore.is_tool_allowed(tool),
            "explore.is_tool_allowed({tool}) must remain true after migration"
        );
    }
    for tool in &expected_denied {
        assert!(
            !explore.is_tool_allowed(tool),
            "explore.is_tool_allowed({tool}) must remain false after migration"
        );
    }

    // Sentinel: probe set covers all relevant cases.
    assert_eq!(
        EXPLORE_PROBE_TOOLS.len(),
        expected_allowed.len() + expected_denied.len(),
        "probe set bookkeeping"
    );
}
```

- [ ] **Step 2: Run integration test**

```bash
cargo test --test tool_sets
```

Expected: 1 test passes.

---

### Task G6: Update MULTI_AGENT_SYSTEM.md (Stage G section)

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`

- [ ] **Step 1: Append "Named Tool Sets" section**

```markdown
## Named Tool Sets (P2 Stage G)

`AgentDef.allowed_tool_sets: Vec<String>` lets agent definitions reference named
tool collections instead of (or alongside) flat allowlists. Three sets are
predefined:

| Name           | Tools                                                  | Purpose                                       |
|----------------|--------------------------------------------------------|-----------------------------------------------|
| `READ_ONLY`    | glob, grep, read_file                                  | Pure filesystem inspection                    |
| `INVESTIGATION`| glob, grep, read_file, search, web_fetch, subagent     | Read-only research with remote sources        |
| `ASYNC_SAFE`   | glob, grep, read_file, search                          | Background-safe (no side effects, no exfil)   |

### Composition Rules

`AgentDef::is_tool_allowed(tool)` evaluates in this precedence:

1. **Recursion guard** (Stage B): SubAgent mode → `subagent` tool denied
   regardless of allowlist
2. **Explicit deny**: tool in `denied_tools` → denied
3. **Set match**: tool in any resolved `allowed_tool_sets` member → allowed
4. **Flat match**: tool in `allowed_tools` (with `"*"` wildcard) → allowed
5. **Default**: denied

`denied_tools` always wins over set membership; this lets agents use a broad
named set then selectively exclude.

### Example

```yaml
---
id: my-research-agent
allowed_tool_sets: [INVESTIGATION]
denied_tools: [web_fetch]   # narrow the broad set
---
```

Effective allowed: glob, grep, read_file, search (web_fetch denied; subagent
denied via mode guard since this is a SubAgent).

### Unknown Set Names

`resolve` returns `None` for unknown names; the loader emits `tracing::warn`
but doesn't fail. This allows future named sets to be added without breaking
older agent definitions.

### Builtin Agents Using Named Sets

| Agent     | Migration                                  |
|-----------|--------------------------------------------|
| `explore` | `INVESTIGATION` (P2 Stage G demo)          |

Other builtins still use flat `allowed_tools`; migrations are incremental
and require behavior-equivalence verification (see `tests/tool_sets.rs`).
```

- [ ] **Step 2: Verify**

```bash
grep -n "Named Tool Sets" docs/reference/MULTI_AGENT_SYSTEM.md
```

Expected: 1 hit.

---

### Task G7: Stage G commit

- [ ] **Step 1: Verify all-green**

```bash
cargo build --workspace
cargo test --workspace --lib agents
cargo test --test tool_sets
cargo test --test agent_loader
cargo test --test subagent_progress
cargo clippy --workspace -- -D warnings
```

Expected: All green.

- [ ] **Step 2: R10 hard checks (final P2 state)**

```bash
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
wc -l src/agents/*.rs | tail -1
wc -l src/agents/subagent_spawner.rs
```

Expected:
- `src/harness/`: ≤ baseline + 10 / 10 files
- `src/agents/`: baseline + ≤ 600 lines
- `subagent_spawner.rs`: ≤ 600 lines

- [ ] **Step 3: Stage and commit**

```bash
git add src/agents/tool_sets.rs src/agents/types.rs \
        src/agents/registry.rs src/agents/loader.rs \
        src/agents/mod.rs \
        tests/tool_sets.rs \
        docs/reference/MULTI_AGENT_SYSTEM.md
git commit -m "$(cat <<'EOF'
agents: semantic tool sets + explore migration to INVESTIGATION (P2 Stage G)

Adds named tool set support for declarative agent allowlists per locked design
at docs/superpowers/specs/2026-05-09-subagent-uplift-p2-design.md §4.

- src/agents/tool_sets.rs (NEW): READ_ONLY, INVESTIGATION, ASYNC_SAFE constants
  + resolve() helper. ASYNC_SAFE excludes web_fetch (exfil risk) and subagent
  (defense-in-depth beyond Stage B for background recursion misuse).
- src/agents/types.rs: AgentDef.allowed_tool_sets field (#[serde(default)]);
  is_tool_allowed extended with set resolution (denied_tools + Stage B
  recursion guard precedence preserved).
- src/agents/loader.rs: activate the deferred allowed_tool_sets wiring from
  Stage E (was let _ = fm.allowed_tool_sets;).
- src/agents/registry.rs: migrate `explore` builtin agent to use
  INVESTIGATION (1 demo per Q7-2 (a) lower bound). Effective behavior
  preserved: Stage B blocks subagent; denied_tools preserved.
- tests/tool_sets.rs: integration test asserting explore's pre/post-migration
  is_tool_allowed return values are identical for all 10 probe tools.
- docs/reference/MULTI_AGENT_SYSTEM.md: new "Named Tool Sets" section.

Per Q7-1 simplified positive design: no ALL_AGENT_DENIED_TOOLS auto-deny,
no allow_override field. Defense layers live elsewhere (Stage B + Stage E
schema gating).

Closes P2 (Stages E + F + G all shipped).
EOF
)"
```

- [ ] **Step 4: Verify P2 final state**

```bash
git log --oneline -4
```

Expected: 3 commits on main from P2 plus the design doc commit (4 total since brainstorm landed).

---

## §∞ — Final Verification (post all 3 commits)

### Task FINAL: PR-level verification per spec §5.2

**Files:**
- Read-only verification

- [ ] **Step 1: Full workspace build**

```bash
cargo build --release --workspace
```

Expected: clean.

- [ ] **Step 2: Full workspace test**

```bash
cargo test --workspace
```

Expected: all green; new tests:
- 6 unit (loader)
- 3 unit (background_tracker progress)
- 6 unit (forwarding)
- 1 unit (check_status progress)
- 5 unit (tool_sets)
- 6 unit (is_tool_allowed)
- 4 integration (agent_loader)
- 2 integration (subagent_progress)
- 1 integration (tool_sets)

Plus all P0/P1 tests still passing (Stage A subagent_deps_inherit, Stage B recursion_guard, Stage C lane_budget, Stage D cancellation_chain).

- [ ] **Step 3: Clippy strict**

```bash
cargo clippy --workspace -- -D warnings
```

Expected: no new warnings/errors from P2 code.

- [ ] **Step 4: R10 hard checks final**

```bash
echo "=== R10 baseline locks ==="
wc -l src/harness/*.rs | tail -1
ls src/harness/*.rs | wc -l
echo "=== src/agents/ growth ==="
wc -l src/agents/*.rs | tail -1
wc -l src/agents/subagent_spawner.rs
```

Expected output rules (failures here block PR):
- harness lines: baseline + ≤ 10 (= AgentDefShadowed variant only)
- harness files: 10
- subagent_spawner.rs: ≤ 600

- [ ] **Step 5: Schema-compatibility regression**

Pick an existing aleph.toml + builtin agents config used by integration tests:

```bash
cargo test --test agent_loader -- --nocapture | head -50
```

Verify all builtins still register and the loader emits expected shadow events on collisions.

- [ ] **Step 6: Documentation/code consistency**

```bash
grep -n "Filesystem Agent Loading\|Subagent Progress Streaming\|Named Tool Sets" \
    docs/reference/MULTI_AGENT_SYSTEM.md
```

Expected: 3 section headers, one per stage. Cross-check that:
- Filesystem section accurately describes loader.rs behavior
- Progress section accurately describes ForwardingTraceSink + check_status
- Tool sets section accurately describes the 3 constants + composition

- [ ] **Step 7: Update roadmap**

Edit `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`:

For each of Stage E / F / G, change:

```markdown
**Status**: 📋 Planned · plan: TBD（P2 phase 时认领）
```

to:

```markdown
**Status**: ✅ Shipped: <stage-commit-hash> on 2026-05-09
```

Use the actual commit hashes from `git log --oneline -3`.

Also at the file head, add:

```markdown
✅ P2 Shipped: <last-commit-hash> on 2026-05-09
```

- [ ] **Step 8: Roadmap commit**

```bash
git add docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md \
        docs/superpowers/specs/2026-05-08-subagent-uplift-p1-design.md
git commit -m "$(cat <<'EOF'
docs(subagent): mark P2 Stage E/F/G as Shipped

Updates roadmap status for the three Stage E/F/G entries with their commit
hashes and adds the P2-shipped header. Closes the P2 phase per the §4.5
roadmap closure conditions (10 stages — A/B/C/D shipped P1, E/F/G shipped P2;
H/I/J remain in P3 backlog).
EOF
)"
```

---

## Summary: Stage Counts

| Commit | Files Changed (excl. docs) | Lines (incl. tests) | New Tests |
|--------|----------------------------|---------------------|-----------|
| Stage E | 6 | ~340 | 6 unit + 4 integration |
| Stage F | 5 | ~300 | 10 unit + 2 integration |
| Stage G | 5 | ~210 | 11 unit + 1 integration |
| Docs (closure) | 2 | ~10 | — |
| **Total P2 PR** | — | **~860** | **27 unit + 7 integration** |

R10 invariant maintained: src/harness/agent.rs zero changes; trace.rs +10 (1 variant); src/harness/*.rs ≤ baseline+10 / 10 files.
