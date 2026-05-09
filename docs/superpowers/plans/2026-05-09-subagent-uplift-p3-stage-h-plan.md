# Subagent Uplift P3 — Stage H Implementation Plan (Worktree Isolation)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add opt-in `isolation: Worktree` mode to subagent spawn so each subagent runs in its own git worktree at `$TMPDIR/aleph-subagent-<label>-<uuid>/` with strict separate `target/` dir; clean up on every exit path including panic.

**Architecture:** New file `src/sandbox/worktree.rs` provides the `WorktreeHandle` RAII primitive (create / cleanup / Drop safety net) plus a minimal `WorktreeSandbox` Sandbox impl that runs commands at the worktree path with `CARGO_TARGET_DIR` injected. `SpawnRequest` gains `isolation: Option<IsolationMode>`. `subagent_spawner::spawn` gets a thin pre-amble that creates the worktree, swaps `HarnessDeps.sandbox` to `WorktreeSandbox`, and an explicit `cleanup().await` on every termination path. Trace observability via two new `LoopTraceEvent` variants.

**Tech Stack:** Rust 2021, tokio, git CLI (≥ 2.20), `uuid` 1.7 (already in Cargo.toml), `tempfile` 3.8 (already in Cargo.toml).

**Source spec:** `docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md` § 2 + § 4 + § 6 + § 7

**R10 redline (must hold):**
- `src/harness/agent.rs` zero diff vs P2 closure (commit 009981ddd)
- `src/harness/*.rs` file count = 10 unchanged; only `trace.rs` grows by 2 enum variants (≤ 4 lines)
- Schema-only, backward-compatible additions; no new logic in harness loop

---

## File Structure

| Path | Action | Purpose | Estimated lines |
|---|---|---|---|
| `src/sandbox/worktree.rs` | **Create** | `WorktreeHandle` RAII + `WorktreeSandbox` impl + `WorktreeError` | ~180 |
| `src/sandbox/mod.rs` | Modify | Add `pub mod worktree;` + re-exports | +2 |
| `src/agents/types.rs` | Modify | Add `IsolationMode` enum | +12 |
| `src/agents/subagent_spawner.rs` | Modify | Add `isolation` field to `SpawnRequest`; add worktree branch in `spawn` | +60 |
| `src/harness/trace.rs` | Modify | Add `WorktreeCreated` + `WorktreeCleanedUp` variants | +4 |
| `tests/worktree_isolation.rs` | **Create** | 6 integration + unit tests | ~160 |
| `docs/reference/MULTI_AGENT_SYSTEM.md` | Modify | Add "Worktree Isolation (P3 Stage H)" section | +50 |

**Total: ~470 lines (≤ 500 budget per design § 2.5).**

---

## Architectural Scope Lock

The `WorktreeSandbox` is intentionally a **minimal** `Sandbox` impl (no seatbelt, no capability baseline). It exists to provide cwd isolation + `CARGO_TARGET_DIR` env injection for opt-in subagents. Parent's seatbelt-based `WorkspaceSandbox` is untouched (R3 core minimalism + scope discipline). This scope choice is documented in `docs/reference/MULTI_AGENT_SYSTEM.md` and called out in PR description.

---

### Task 1: Add `IsolationMode` enum to `src/agents/types.rs`

**Files:**
- Modify: `src/agents/types.rs` (append after the existing `AgentMode` enum)
- Test: `src/agents/types.rs` (existing `#[cfg(test)] mod tests` block)

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` block in `src/agents/types.rs`:
```rust
#[test]
fn isolation_mode_serde_round_trip_worktree() {
    let mode = IsolationMode::Worktree;
    let json = serde_json::to_string(&mode).expect("serialize");
    assert_eq!(json, r#"{"kind":"worktree"}"#);
    let parsed: IsolationMode = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, IsolationMode::Worktree);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agents::types::tests::isolation_mode_serde_round_trip_worktree`
Expected: FAIL — `cannot find type 'IsolationMode' in this scope`

- [ ] **Step 3: Add the enum**

Append after the existing `AgentMode` enum in `src/agents/types.rs`:
```rust
/// Subagent execution isolation mode (P3 Stage H).
///
/// `Worktree` runs the subagent in a fresh git worktree under `$TMPDIR`
/// with a separate `target/` dir; cleanup is guaranteed on every exit path.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IsolationMode {
    Worktree,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agents::types::tests::isolation_mode_serde_round_trip_worktree`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/agents/types.rs
git commit -m "agents: add IsolationMode enum for P3 Stage H

Future-extensible enum (single Worktree variant for now).
Tag-based serde with snake_case for forward-compat with future variants.

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.2.2"
```

---

### Task 2: Add `WorktreeError` + module skeleton

**Files:**
- Create: `src/sandbox/worktree.rs`
- Modify: `src/sandbox/mod.rs:13-25` (the `pub mod ...;` block)

- [ ] **Step 1: Write the failing test**

Create `src/sandbox/worktree.rs` with this content (test-first, no impl):
```rust
//! Git worktree isolation primitives for subagent strict isolation (P3 Stage H).
//!
//! `WorktreeHandle::create` provisions a fresh detached-HEAD worktree under
//! `$TMPDIR/aleph-subagent-<label>-<uuid>/`. Cleanup is RAII-guarded:
//! `cleanup()` is the explicit happy path; `Drop` is the safety net.
//!
//! See: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("git worktree add failed: {0}")]
    Create(String),
    #[error("git worktree remove failed at {path}: {source}")]
    Cleanup { path: PathBuf, source: String },
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("not a git repository: {0}")]
    NotAGitRepo(PathBuf),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_error_displays_create_message() {
        let e = WorktreeError::Create("git command not found".into());
        assert!(format!("{e}").contains("git worktree add failed"));
        assert!(format!("{e}").contains("git command not found"));
    }
}
```

In `src/sandbox/mod.rs`, locate the `pub mod ...;` block (around line 13–25). Add after `pub mod workspace;`:
```rust
pub mod worktree;
```

- [ ] **Step 2: Run test to verify it builds and passes**

Run: `cargo test -p alephcore --lib sandbox::worktree::tests::worktree_error_displays_create_message`
Expected: PASS — error message uses `thiserror` formatting correctly

- [ ] **Step 3: Commit**

```bash
git add src/sandbox/worktree.rs src/sandbox/mod.rs
git commit -m "sandbox: add worktree module skeleton + WorktreeError

Module declared, error variants stubbed; create/cleanup logic in next commits.

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.2.1"
```

---

### Task 3: Implement `WorktreeHandle::create` happy path

**Files:**
- Modify: `src/sandbox/worktree.rs`

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `src/sandbox/worktree.rs`:
```rust
#[tokio::test]
async fn create_succeeds_in_a_git_repo() {
    let repo_root = std::env::current_dir().expect("cwd");
    // Aleph repo is itself a git repo; safe to use as parent.
    let h = create(&repo_root, "task3-create", None)
        .await
        .expect("create");
    assert!(h.path().exists(), "worktree dir should exist");
    assert!(h.path().join(".git").exists(), "worktree must have .git pointer");
    // Cleanup so this test does not leak.
    h.cleanup().await.expect("cleanup");
}

#[tokio::test]
async fn create_in_non_git_dir_fails_with_not_a_git_repo() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let err = create(tmp.path(), "task3-non-git", None)
        .await
        .expect_err("must fail outside git repo");
    assert!(matches!(err, WorktreeError::NotAGitRepo(_)), "got {err:?}");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib sandbox::worktree::tests::create_succeeds_in_a_git_repo`
Expected: FAIL — `cannot find function 'create' in this scope`

- [ ] **Step 3: Implement `create` and `WorktreeHandle` (path/cleanup_marker fields only)**

Append below the `WorktreeError` enum in `src/sandbox/worktree.rs`:
```rust
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::process::Command;

/// RAII handle to a git worktree. Call `cleanup()` to remove it; `Drop` is the safety net.
pub struct WorktreeHandle {
    path: PathBuf,
    repo_root: PathBuf,
    cleaned_up: Arc<AtomicBool>,
    trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
}

impl WorktreeHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }
}

/// Create a fresh detached-HEAD worktree under `$TMPDIR/aleph-subagent-<label>-<uuid>/`.
///
/// Performance contract: ≤ 200ms typical (git worktree add).
/// Errors: `NotAGitRepo` if `repo_root` has no `.git`; `Create` for any git failure.
pub async fn create(
    repo_root: &Path,
    label: &str,
    trace_sink: Option<Arc<dyn crate::harness::TraceSink>>,
) -> Result<WorktreeHandle, WorktreeError> {
    if !repo_root.join(".git").exists() {
        return Err(WorktreeError::NotAGitRepo(repo_root.to_path_buf()));
    }

    let id = uuid::Uuid::new_v4();
    let safe_label: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    let path = std::env::temp_dir().join(format!("aleph-subagent-{safe_label}-{id}"));

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("worktree")
        .arg("add")
        .arg("--detach")
        .arg(&path)
        .arg("HEAD")
        .output()
        .await
        .map_err(|e| WorktreeError::Create(format!("spawn git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(WorktreeError::Create(stderr));
    }

    if let Some(sink) = trace_sink.as_ref() {
        sink.emit(crate::harness::trace::LoopTraceEvent::WorktreeCreated {
            path: path.clone(),
        });
    }

    Ok(WorktreeHandle {
        path,
        repo_root: repo_root.to_path_buf(),
        cleaned_up: Arc::new(AtomicBool::new(false)),
        trace_sink,
    })
}
```

> **Note**: `LoopTraceEvent::WorktreeCreated` does not exist yet (Task 6 adds it). For Task 3, comment out the `sink.emit(...)` block temporarily; uncomment in Task 6 after the variant exists.

Replace the `sink.emit(...)` block above with:
```rust
    // Trace event added in Task 6; placeholder here keeps signature stable.
    let _ = trace_sink.as_ref();
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib sandbox::worktree::tests::create_`
Expected: PASS for both `create_succeeds_in_a_git_repo` and `create_in_non_git_dir_fails_with_not_a_git_repo`

- [ ] **Step 5: Commit**

```bash
git add src/sandbox/worktree.rs
git commit -m "sandbox: WorktreeHandle::create — detached HEAD worktree under \$TMPDIR

Validates parent is a git repo before invoking 'git worktree add --detach'.
Path = \$TMPDIR/aleph-subagent-<safe_label>-<uuid>/. Trace emit placeholder
landed (variants added in next commit).

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.2.1"
```

---

### Task 4: Implement `WorktreeHandle::cleanup`

**Files:**
- Modify: `src/sandbox/worktree.rs`

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `src/sandbox/worktree.rs`:
```rust
#[tokio::test]
async fn cleanup_removes_worktree_dir() {
    let repo_root = std::env::current_dir().expect("cwd");
    let h = create(&repo_root, "task4-cleanup", None)
        .await
        .expect("create");
    let path = h.path().to_path_buf();
    assert!(path.exists(), "precondition: dir exists");
    h.cleanup().await.expect("cleanup");
    assert!(!path.exists(), "cleanup must remove worktree dir");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib sandbox::worktree::tests::cleanup_removes_worktree_dir`
Expected: FAIL — `no method named 'cleanup' found`

- [ ] **Step 3: Implement `cleanup`**

Add to the `impl WorktreeHandle` block in `src/sandbox/worktree.rs`:
```rust
    /// Explicit cleanup. Removes the worktree via `git worktree remove --force`,
    /// then marks the handle as cleaned up so `Drop` skips its safety-net work.
    /// Performance contract: ≤ 100ms typical.
    pub async fn cleanup(self) -> Result<(), WorktreeError> {
        let result = remove_worktree(&self.repo_root, &self.path).await;
        self.cleaned_up.store(true, Ordering::Release);

        if let Some(sink) = self.trace_sink.as_ref() {
            // Variant added in Task 6.
            let _ = sink;
        }

        result
    }
```

Add this private helper at the bottom of `src/sandbox/worktree.rs` (above `#[cfg(test)] mod tests`):
```rust
async fn remove_worktree(repo_root: &Path, path: &Path) -> Result<(), WorktreeError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("worktree")
        .arg("remove")
        .arg("--force")
        .arg(path)
        .output()
        .await
        .map_err(|e| WorktreeError::Cleanup {
            path: path.to_path_buf(),
            source: format!("spawn git: {e}"),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(WorktreeError::Cleanup {
            path: path.to_path_buf(),
            source: stderr,
        });
    }

    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib sandbox::worktree::tests::cleanup_removes_worktree_dir`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sandbox/worktree.rs
git commit -m "sandbox: WorktreeHandle::cleanup — explicit removal via git CLI

Marks cleaned_up=true after success; Drop safety-net (Task 5) skips when set.
Trace emit placeholder threaded; variant added in Task 6.

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.2.1"
```

---

### Task 5: Implement `Drop` safety net

**Files:**
- Modify: `src/sandbox/worktree.rs`

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `src/sandbox/worktree.rs`:
```rust
#[tokio::test]
async fn drop_without_cleanup_logs_and_removes_dir() {
    let repo_root = std::env::current_dir().expect("cwd");
    let path = {
        let h = create(&repo_root, "task5-drop", None)
            .await
            .expect("create");
        h.path().to_path_buf()
        // h dropped here without cleanup() called
    };
    // Drop spawns blocking removal; allow time for it.
    for _ in 0..50 {
        if !path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        !path.exists(),
        "Drop safety-net must remove leaked worktree at {path:?}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib sandbox::worktree::tests::drop_without_cleanup_logs_and_removes_dir`
Expected: FAIL — directory still exists after handle drop (no Drop impl yet).

- [ ] **Step 3: Implement `Drop`**

Add to `src/sandbox/worktree.rs` below the `impl WorktreeHandle` block:
```rust
impl Drop for WorktreeHandle {
    fn drop(&mut self) {
        if self.cleaned_up.load(Ordering::Acquire) {
            return;
        }
        // Safety net: spawn blocking task to run `git worktree remove --force`.
        // Errors are logged via tracing; we never panic from Drop.
        let repo_root = self.repo_root.clone();
        let path = self.path.clone();
        tracing::error!(
            path = %path.display(),
            "WorktreeHandle leaked — Drop safety-net removing"
        );
        // Variant added in Task 6.
        let _sink = self.trace_sink.clone();
        std::thread::spawn(move || {
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo_root)
                .arg("worktree")
                .arg("remove")
                .arg("--force")
                .arg(&path)
                .status();
            if let Err(e) = status {
                tracing::error!(
                    path = %path.display(),
                    error = %e,
                    "Drop safety-net cleanup failed"
                );
            }
        });
    }
}
```

> **Note**: Drop uses `std::thread::spawn` + `std::process::Command` (sync) instead of `tokio::task::spawn_blocking` to avoid requiring a tokio runtime — the handle may be dropped from any context.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib sandbox::worktree::tests::drop_without_cleanup_logs_and_removes_dir`
Expected: PASS (may take ≤ 5s due to retry loop)

- [ ] **Step 5: Commit**

```bash
git add src/sandbox/worktree.rs
git commit -m "sandbox: WorktreeHandle Drop safety-net for leak recovery

If cleanup() was never called, Drop fires-and-forgets a blocking
'git worktree remove --force' on a fresh OS thread (avoids tokio
runtime dependency). Errors logged via tracing; never panics.

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.3"
```

---

### Task 6: Add `LoopTraceEvent::WorktreeCreated` + `WorktreeCleanedUp`

**Files:**
- Modify: `src/harness/trace.rs:11-58` (the `LoopTraceEvent` enum)
- Modify: `src/sandbox/worktree.rs` (uncomment the trace emit blocks)

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` block in `src/harness/trace.rs` (or create one if absent):
```rust
#[cfg(test)]
mod p3_stage_h_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn worktree_created_serializes_with_path() {
        let event = LoopTraceEvent::WorktreeCreated {
            path: PathBuf::from("/tmp/aleph-subagent-x"),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"worktree_created""#));
        assert!(json.contains(r#""path":"/tmp/aleph-subagent-x""#));
    }

    #[test]
    fn worktree_cleaned_up_serializes_with_leaked_flag() {
        let event = LoopTraceEvent::WorktreeCleanedUp {
            path: PathBuf::from("/tmp/aleph-subagent-y"),
            leaked: true,
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains(r#""type":"worktree_cleaned_up""#));
        assert!(json.contains(r#""leaked":true"#));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p alephcore --lib harness::trace::p3_stage_h_tests`
Expected: FAIL — `no variant 'WorktreeCreated' / 'WorktreeCleanedUp'`

- [ ] **Step 3: Add the variants**

In `src/harness/trace.rs`, locate the `LoopTraceEvent` enum (line ~12). Append before the closing `}` (after `SessionCompleted { ... }`):
```rust
    /// Subagent worktree isolation primitive created (P3 Stage H).
    WorktreeCreated { path: std::path::PathBuf },
    /// Subagent worktree cleaned up; `leaked = true` means cleanup was via
    /// Drop safety-net rather than explicit `cleanup()` (P3 Stage H).
    WorktreeCleanedUp { path: std::path::PathBuf, leaked: bool },
```

In `src/sandbox/worktree.rs`, replace the placeholder in `create` (Task 3 step 3):
```rust
    // OLD:
    // let _ = trace_sink.as_ref();
    // NEW:
    if let Some(sink) = trace_sink.as_ref() {
        sink.emit(crate::harness::trace::LoopTraceEvent::WorktreeCreated {
            path: path.clone(),
        });
    }
```

In `src/sandbox/worktree.rs`, replace the placeholder in `cleanup` (Task 4 step 3):
```rust
    // OLD:
    // if let Some(sink) = self.trace_sink.as_ref() {
    //     let _ = sink;
    // }
    // NEW:
    if let Some(sink) = self.trace_sink.as_ref() {
        sink.emit(crate::harness::trace::LoopTraceEvent::WorktreeCleanedUp {
            path: self.path.clone(),
            leaked: false,
        });
    }
```

In `src/sandbox/worktree.rs`, replace the placeholder in `Drop` (Task 5 step 3):
```rust
    // OLD:
    // let _sink = self.trace_sink.clone();
    // NEW:
    if let Some(sink) = self.trace_sink.as_ref() {
        sink.emit(crate::harness::trace::LoopTraceEvent::WorktreeCleanedUp {
            path: self.path.clone(),
            leaked: true,
        });
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p alephcore --lib harness::trace::p3_stage_h_tests sandbox::worktree::tests`
Expected: PASS — all worktree tests + new trace tests

- [ ] **Step 5: Verify R10 line budget**

Run: `wc -l src/harness/*.rs`
Expected: `trace.rs` grew by ≤ 4 lines vs the prior commit. Total `src/harness/*.rs` line count growth ≤ 4. Other files unchanged.

- [ ] **Step 6: Commit**

```bash
git add src/harness/trace.rs src/sandbox/worktree.rs
git commit -m "harness/trace: add WorktreeCreated/CleanedUp variants (P3 Stage H)

Schema-only LoopTraceEvent extension. R10-safe: no logic added to harness
loop, just two backward-compatible enum variants. Wired into worktree.rs
create/cleanup/Drop paths.

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.2.3"
```

---

### Task 7: Add `WorktreeSandbox` Sandbox impl

**Files:**
- Modify: `src/sandbox/worktree.rs`

- [ ] **Step 1: Write the failing test**

Append to `#[cfg(test)] mod tests` in `src/sandbox/worktree.rs`:
```rust
#[tokio::test]
async fn worktree_sandbox_executes_at_worktree_path() {
    let repo_root = std::env::current_dir().expect("cwd");
    let h = create(&repo_root, "task7-sandbox", None)
        .await
        .expect("create");
    let expected_path = h.path().to_path_buf();
    let sandbox = WorktreeSandbox::new(h.path().to_path_buf());

    let cmd = crate::sandbox::SandboxCommand {
        program: "pwd".into(),
        args: vec![],
        cwd: None,
        env: Default::default(),
        capabilities: Default::default(),
    };
    use crate::sandbox::Sandbox as _;
    let out = sandbox.execute(cmd).await.expect("execute");

    let stdout_str = String::from_utf8_lossy(&out.stdout);
    let actual = stdout_str.trim();
    let expected = expected_path
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| expected_path.to_string_lossy().into_owned());
    assert!(
        actual.ends_with(expected_path.file_name().unwrap().to_str().unwrap())
            || actual == expected,
        "pwd output {actual:?} should match worktree path {expected:?}"
    );

    h.cleanup().await.expect("cleanup");
}
```

> **Note**: The exact `SandboxCommand` field names may differ; consult `src/sandbox/command.rs` and adjust the literal in the test to match (the `cwd: None` and `env` fields are the load-bearing ones).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib sandbox::worktree::tests::worktree_sandbox_executes_at_worktree_path`
Expected: FAIL — `cannot find struct 'WorktreeSandbox'`

- [ ] **Step 3: Implement `WorktreeSandbox`**

Append to `src/sandbox/worktree.rs` below the `impl Drop` block:
```rust
use async_trait::async_trait;

/// Minimal Sandbox implementation that executes commands at a fixed cwd
/// (the worktree path) with `CARGO_TARGET_DIR` injected. Does NOT apply
/// seatbelt/capability baseline — that is by design (P3 Stage H scope:
/// workspace isolation, not security sandboxing). For seatbelt-protected
/// subagents, run them outside Worktree mode and trust parent's sandbox.
pub struct WorktreeSandbox {
    worktree_path: PathBuf,
}

impl WorktreeSandbox {
    pub fn new(worktree_path: PathBuf) -> Self {
        Self { worktree_path }
    }
}

#[async_trait]
impl crate::sandbox::Sandbox for WorktreeSandbox {
    async fn execute(
        &self,
        mut command: crate::sandbox::SandboxCommand,
    ) -> Result<crate::sandbox::SandboxOutput, crate::sandbox::SandboxError> {
        // Override cwd to worktree (ignoring caller's cwd; we are isolated).
        command.cwd = Some(self.worktree_path.clone());
        // Inject CARGO_TARGET_DIR for strict build isolation.
        command.env.insert(
            "CARGO_TARGET_DIR".into(),
            self.worktree_path.join("target").to_string_lossy().into_owned(),
        );

        let mut tokio_cmd = tokio::process::Command::new(&command.program);
        tokio_cmd
            .args(&command.args)
            .current_dir(&self.worktree_path);
        for (k, v) in &command.env {
            tokio_cmd.env(k, v);
        }

        let output = tokio_cmd.output().await.map_err(|e| {
            crate::sandbox::SandboxError::ExecutionFailed(format!("spawn {} failed: {e}", command.program))
        })?;

        Ok(crate::sandbox::SandboxOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}
```

> **Implementer note**: Field names of `SandboxCommand` (`program`/`args`/`cwd`/`env`/`capabilities`) and `SandboxOutput` (`status`/`stdout`/`stderr`) and the variant name on `SandboxError` (`ExecutionFailed`) MAY differ. Read `src/sandbox/command.rs` and `src/sandbox/mod.rs:38-54` for current shape; adapt literals and constructor calls. The functional contract — "run `program` with `args` at `worktree_path` cwd, inject CARGO_TARGET_DIR, return captured stdout/stderr/status" — is what matters.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib sandbox::worktree::tests::worktree_sandbox_executes_at_worktree_path`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/sandbox/worktree.rs
git commit -m "sandbox: WorktreeSandbox — minimal Sandbox impl for Stage H

Runs commands at worktree path; injects CARGO_TARGET_DIR. Intentionally
does not apply seatbelt — Stage H scope is workspace isolation only.
Documented as a known scope choice in MULTI_AGENT_SYSTEM.md (Task 11).

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.2.1
       Architectural Scope Lock"
```

---

### Task 8: Wire `isolation` field into `SpawnRequest`

**Files:**
- Modify: `src/agents/subagent_spawner.rs:100-116` (`SpawnRequest` struct)

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block in `src/agents/subagent_spawner.rs`:
```rust
#[test]
fn spawn_request_default_isolation_is_none() {
    use crate::agents::types::IsolationMode;
    // Construct a SpawnRequest manually to ensure the new field defaults to None.
    let agent = AgentDef::new("test", AgentMode::SubAgent);
    let cancel = tokio_util::sync::CancellationToken::new();
    let req = SpawnRequest {
        agent_def: &agent,
        task: "x",
        context_summary: None,
        model: None,
        timeout_secs: 1,
        cancel,
        isolation: None,
    };
    assert!(matches!(req.isolation, None));

    let mode = Some(IsolationMode::Worktree);
    let _has_isolation = mode.is_some();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib agents::subagent_spawner::tests::spawn_request_default_isolation_is_none`
Expected: FAIL — `struct SpawnRequest has no field 'isolation'`

- [ ] **Step 3: Add the field**

In `src/agents/subagent_spawner.rs`, locate `SpawnRequest<'a>` (line ~100). Append a new field before the closing `}`:
```rust
    /// P3 Stage H — optional worktree isolation. `None` = legacy shared cwd
    /// behavior; `Some(IsolationMode::Worktree)` opts into a fresh git
    /// worktree under $TMPDIR for this spawn.
    pub isolation: Option<crate::agents::types::IsolationMode>,
```

Update all existing `SpawnRequest { ... }` constructions inside `mod tests` (lines 707+ and elsewhere) to include `isolation: None,`. Search via:
```bash
grep -n "SpawnRequest {" src/agents/subagent_spawner.rs
```

For each match, add `isolation: None,` as the last field.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib agents::subagent_spawner::tests::spawn_request_default_isolation_is_none`
Expected: PASS

Run: `cargo test -p alephcore --lib agents::subagent_spawner::tests`
Expected: All existing spawner tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/agents/subagent_spawner.rs
git commit -m "agents/spawner: add SpawnRequest.isolation field (P3 Stage H)

Optional Option<IsolationMode>; default None preserves legacy shared-cwd
behavior. Existing tests updated to set isolation: None explicitly.

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.2.2"
```

---

### Task 9: Wire worktree creation/cleanup into `spawn`

**Files:**
- Modify: `src/agents/subagent_spawner.rs:128-347` (the `spawn` function body)

- [ ] **Step 1: Write the failing test**

Create the integration-test file `tests/worktree_isolation.rs`:
```rust
//! Integration tests for P3 Stage H — Worktree isolation.

use std::sync::{Arc, Mutex};

use alephcore::agents::types::IsolationMode;
use alephcore::harness::trace::{LoopTraceEvent, TraceSink};

#[derive(Default, Clone)]
struct CapturingSink {
    events: Arc<Mutex<Vec<LoopTraceEvent>>>,
}

impl TraceSink for CapturingSink {
    fn emit(&self, event: LoopTraceEvent) {
        self.events.lock().unwrap().push(event);
    }
}

impl CapturingSink {
    fn snapshot(&self) -> Vec<LoopTraceEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn h_t1_happy_path_creates_and_cleans_up_worktree() {
    let sink = CapturingSink::default();
    let arc_sink: Arc<dyn TraceSink> = Arc::new(sink.clone());
    let repo = std::env::current_dir().unwrap();

    let h = alephcore::sandbox::worktree::create(&repo, "h-t1", Some(arc_sink.clone()))
        .await
        .expect("create");
    let path = h.path().to_path_buf();
    assert!(path.exists());
    h.cleanup().await.expect("cleanup");
    assert!(!path.exists());

    let events = sink.snapshot();
    assert!(
        events.iter().any(|e| matches!(e, LoopTraceEvent::WorktreeCreated { .. })),
        "expected WorktreeCreated event"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            LoopTraceEvent::WorktreeCleanedUp { leaked: false, .. }
        )),
        "expected WorktreeCleanedUp(leaked=false) event"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --test worktree_isolation h_t1_happy_path_creates_and_cleans_up_worktree`
Expected: FAIL or compile error — `alephcore::sandbox::worktree` may not be re-exported yet

- [ ] **Step 3: Add re-exports for integration tests**

In `src/sandbox/mod.rs`, after `pub mod worktree;`, ensure types are accessible:
```rust
pub use worktree::{WorktreeError, WorktreeHandle, WorktreeSandbox};
```

- [ ] **Step 4: Wire into `subagent_spawner::spawn`**

In `src/agents/subagent_spawner.rs:243-271` (the `let deps = HarnessDeps { ... };` block), prepend a worktree creation block right after step 6 (the AllowlistToolService wrap, line ~234):
```rust
        // P3 Stage H — worktree isolation (opt-in via SpawnRequest.isolation).
        // Cleaned up explicitly at the bottom of this async block; Drop is the
        // safety net if we early-return before that point.
        let worktree = match req.isolation {
            Some(crate::agents::types::IsolationMode::Worktree) => {
                let repo_root = std::env::current_dir()
                    .map_err(|e| format!("sub-agent failed: get cwd: {e}"))?;
                let handle = crate::sandbox::worktree::create(
                    &repo_root,
                    &req.agent_def.id,
                    base.trace_sink.clone(),
                )
                .await
                .map_err(|e| format!("sub-agent failed: isolation setup: {e}"))?;
                Some(handle)
            }
            None => None,
        };
```

In the same function, modify the `let deps = HarnessDeps { ... };` block (line ~243): replace the existing `sandbox: base.sandbox.clone(),` line with:
```rust
            sandbox: match worktree.as_ref() {
                Some(h) => Arc::new(crate::sandbox::WorktreeSandbox::new(h.path().to_path_buf())),
                None => base.sandbox.clone(),
            },
```

After step 8 (just before the `Ok(result)` return at line ~332), add explicit cleanup:
```rust
                // P3 Stage H — explicit worktree cleanup on success path.
                if let Some(h) = worktree.take() {
                    if let Err(e) = h.cleanup().await {
                        tracing::warn!(error = %e, "worktree cleanup on success path failed; Drop safety-net will retry");
                    }
                }
```

> **Note**: `worktree` is moved into the inner `async { ... }` block. To `take()` it, declare it as `let mut worktree: Option<WorktreeHandle> = match req.isolation { ... }` BEFORE the inner async block, then use `worktree.take()` inside.

For Err paths (`Err(_elapsed)`, `Ok(Err(...))`, `Ok(Ok(Err(_)))`): the `Drop` impl of `WorktreeHandle` handles cleanup automatically when the `Option<WorktreeHandle>` goes out of scope at function end. No code change needed for those paths.

> **Implementer note**: Where the `worktree` variable lives (inside vs outside the `async { ... }` closure on line 161) matters for `Drop` ordering. Place it **outside** the inner async block so it survives until the outer function returns; this guarantees `Drop` fires on every exit path including timeout and panic.

Concretely: move `let mut worktree: Option<...> = ...` to right before `let result: Result<LoopRunResult, String> = async { ... }.await;` on line 161, and inside the async block use a captured `&mut worktree` via `&mut` borrow. Since `async move` would move it, switch to `async { ... }` (no move) and reference via outer `worktree.as_ref()`.

- [ ] **Step 5: Run integration test to verify it passes**

Run: `cargo test --test worktree_isolation h_t1_happy_path_creates_and_cleans_up_worktree`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add tests/worktree_isolation.rs src/sandbox/mod.rs src/agents/subagent_spawner.rs
git commit -m "agents/spawner: wire worktree isolation into spawn (P3 Stage H)

When SpawnRequest.isolation = Some(Worktree), create WorktreeHandle, swap
HarnessDeps.sandbox to WorktreeSandbox, cleanup explicitly on success;
Drop safety-net handles error/timeout/panic paths automatically.

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.2.2"
```

---

### Task 10: Add cancel/panic/leak/perf integration tests

**Files:**
- Modify: `tests/worktree_isolation.rs`

- [ ] **Step 1: Write the failing tests**

Append to `tests/worktree_isolation.rs`:
```rust
#[tokio::test]
async fn h_t2_cancel_path_still_cleans_up() {
    let sink = CapturingSink::default();
    let arc_sink: Arc<dyn TraceSink> = Arc::new(sink.clone());
    let repo = std::env::current_dir().unwrap();

    // Create a handle; simulate cancellation by dropping the handle without explicit cleanup
    // (matches what spawn() does on Err paths).
    let path = {
        let h = alephcore::sandbox::worktree::create(&repo, "h-t2", Some(arc_sink.clone()))
            .await
            .expect("create");
        h.path().to_path_buf()
        // h dropped here — Drop safety-net cleans up
    };
    // Wait up to 5s for safety-net cleanup.
    for _ in 0..50 {
        if !path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(!path.exists(), "Drop safety-net must clean up cancelled worktree");
}

#[tokio::test]
async fn h_t3_panic_path_emits_leaked_true_event() {
    let sink = CapturingSink::default();
    let arc_sink: Arc<dyn TraceSink> = Arc::new(sink.clone());
    let repo = std::env::current_dir().unwrap();

    let path = {
        let h = alephcore::sandbox::worktree::create(&repo, "h-t3", Some(arc_sink.clone()))
            .await
            .expect("create");
        h.path().to_path_buf()
        // h dropped without cleanup — same as panic path
    };
    // Wait up to 5s.
    for _ in 0..50 {
        if !path.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let events = sink.snapshot();
    assert!(
        events.iter().any(|e| matches!(
            e,
            LoopTraceEvent::WorktreeCleanedUp { leaked: true, .. }
        )),
        "expected WorktreeCleanedUp(leaked=true) on Drop path; got events: {events:?}"
    );
}

#[tokio::test]
async fn h_t4_no_leaked_dirs_after_10_random_cancellations() {
    use rand::Rng;
    let repo = std::env::current_dir().unwrap();
    let mut paths = Vec::new();
    for i in 0..10 {
        let h = alephcore::sandbox::worktree::create(&repo, &format!("h-t4-{i}"), None)
            .await
            .expect("create");
        paths.push(h.path().to_path_buf());
        if rand::thread_rng().gen_bool(0.5) {
            h.cleanup().await.expect("explicit cleanup");
        }
        // else: drop and let safety-net handle it
    }
    // Wait up to 10s for all safety-net spawns.
    for _ in 0..100 {
        let any_remaining = paths.iter().any(|p| p.exists());
        if !any_remaining {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let leftover: Vec<_> = paths.iter().filter(|p| p.exists()).collect();
    assert!(leftover.is_empty(), "leaked worktrees: {leftover:?}");
}

#[tokio::test]
async fn h_t5_create_and_cleanup_within_perf_budget() {
    let repo = std::env::current_dir().unwrap();
    let t0 = std::time::Instant::now();
    let h = alephcore::sandbox::worktree::create(&repo, "h-t5", None)
        .await
        .expect("create");
    let create_ms = t0.elapsed().as_millis();
    let t1 = std::time::Instant::now();
    h.cleanup().await.expect("cleanup");
    let cleanup_ms = t1.elapsed().as_millis();
    // Loose budgets per spec § 7 (allow 4× headroom for slow CI).
    assert!(create_ms < 800, "create took {create_ms}ms, budget 200ms (4× CI headroom)");
    assert!(cleanup_ms < 400, "cleanup took {cleanup_ms}ms, budget 100ms (4× CI headroom)");
}
```

> **Note**: The `rand` crate must be in `Cargo.toml [dev-dependencies]`. Verify with:
> ```bash
> grep -E "^rand" Cargo.toml
> ```
> If absent, add `rand = "0.8"` to `[dev-dependencies]`.

- [ ] **Step 2: Run tests to verify they fail (or pass — Drop logic landed in Task 5/6 should already make T2/T3 pass)**

Run: `cargo test --test worktree_isolation`
Expected: H-T1 PASS (from Task 9); H-T2/T3/T4/T5 FAIL only if `rand` dep missing or impl bug.

- [ ] **Step 3: Add `rand` to dev-dependencies if missing**

Edit `Cargo.toml` `[dev-dependencies]` section:
```toml
rand = "0.8"
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test worktree_isolation`
Expected: All 5 tests (H-T1 through H-T5) PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/worktree_isolation.rs Cargo.toml Cargo.lock
git commit -m "tests(worktree): cancel/panic/leak/perf coverage (P3 Stage H)

H-T2: Drop safety-net on cancel-equivalent path
H-T3: leaked=true emission on Drop
H-T4: 10× random cancel — zero leftover \$TMPDIR entries
H-T5: create ≤200ms / cleanup ≤100ms (4× CI headroom = 800/400ms)

Adds 'rand' to dev-dependencies for randomized cancel choice.

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2.4 (H-T1..H-T5)"
```

---

### Task 11: Doc update + R10 baseline check

**Files:**
- Modify: `docs/reference/MULTI_AGENT_SYSTEM.md`

- [ ] **Step 1: Add a "Worktree Isolation (P3 Stage H)" section**

Append to `docs/reference/MULTI_AGENT_SYSTEM.md` (find existing P1/P2 stage sections; mirror their style). Section text:

````markdown
## Worktree Isolation (P3 Stage H)

Subagent spawns can opt into git worktree isolation:

```rust
let req = SpawnRequest {
    agent_def: &agent_def,
    task: "refactor module X",
    context_summary: None,
    model: None,
    timeout_secs: 600,
    cancel: cancel_token,
    isolation: Some(IsolationMode::Worktree), // P3 Stage H
};
```

When set to `Worktree`, the spawner creates a fresh detached-HEAD git
worktree at `$TMPDIR/aleph-subagent-<safe_label>-<uuid>/` before running
the child harness. The child's `Sandbox` is replaced with `WorktreeSandbox`,
which executes commands at the worktree path and injects `CARGO_TARGET_DIR`
for strict build isolation.

Cleanup is RAII-guaranteed:
- **Success path**: explicit `cleanup().await` after harness returns.
- **Error/timeout/panic path**: `Drop` safety-net spawns a blocking
  `git worktree remove --force` and emits
  `LoopTraceEvent::WorktreeCleanedUp { leaked: true }`.

### Scope

`WorktreeSandbox` provides **workspace isolation only** — it does not apply
seatbelt or capability baseline. For seatbelt-protected subagents, omit
`isolation` (or set to `None`) and trust the parent's `WorkspaceSandbox`.
This is a deliberate Stage H scope choice; combining seatbelt + worktree
is a follow-up.

### Trace events

`LoopTraceEvent::WorktreeCreated { path }` and
`LoopTraceEvent::WorktreeCleanedUp { path, leaked: bool }` flow into
the parent's `trace_sink`. Use `leaked` to distinguish explicit cleanup
from Drop safety-net cleanup in monitoring dashboards.

### Performance contract

- `create`: ≤ 200ms typical (`git worktree add` cost)
- `cleanup`: ≤ 100ms typical (`git worktree remove --force` cost)

### Failure mode

Worktree creation failure is **fail-loud**: spawner returns
`"sub-agent failed: isolation setup: ..."`. There is no fallback to shared
cwd — isolation declared must be isolation honored.
````

- [ ] **Step 2: Run R10 baseline check**

Run:
```bash
wc -l src/harness/*.rs
ls src/harness/*.rs | wc -l
git diff 009981ddd -- src/harness/agent.rs | wc -l
```
Expected:
- `src/harness/agent.rs` line count unchanged from P2 closure (commit `009981ddd`)
- File count = 10
- `git diff` of `agent.rs` outputs `0`
- `trace.rs` grew by ≤ 4 lines vs P2 closure

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p alephcore --lib && cargo test --test worktree_isolation && cargo test --test subagent_progress && cargo test --test recursion_guard && cargo test --test cancellation_chain`
Expected: All green (worktree tests + existing P1/P2 integration tests still passing).

- [ ] **Step 4: Run clippy on the touched scope**

Run: `cargo clippy -p alephcore --lib --tests -- -D warnings 2>&1 | grep -E "src/sandbox/worktree|src/agents/subagent_spawner|src/agents/types|src/harness/trace|tests/worktree_isolation" || echo "scope clean"`
Expected: `scope clean` (or only pre-existing errors in unrelated files; verify with `git stash; cargo clippy ...; git stash pop`)

- [ ] **Step 5: Commit**

```bash
git add docs/reference/MULTI_AGENT_SYSTEM.md
git commit -m "docs(multi-agent): document Worktree Isolation (P3 Stage H)

Covers SpawnRequest.isolation usage, RAII cleanup contract, scope choice
(no seatbelt), trace events, performance budget, fail-loud failure mode.

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md § 2"
```

---

### Task 12: Final closure — roadmap status update

**Files:**
- Modify: `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`

- [ ] **Step 1: Capture the latest commit hash**

Run: `git log -1 --format=%H` to get the final Stage H commit hash. Save it as `<HASH>` for the next step.

- [ ] **Step 2: Update the roadmap**

In `docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md`:

1. Locate the Stage H entry (line ~488 — `### Stage H — Worktree isolation`).
2. Change the `**Status**:` line from `📋 Planned · plan: TBD` to:
   ```markdown
   **Status**: ✅ Shipped: <HASH> on 2026-05-09 · plan: docs/superpowers/plans/2026-05-09-subagent-uplift-p3-stage-h-plan.md
   ```
3. At the top of the file (line ~12), append after the existing P2 line:
   ```markdown
   ✅ P3 Stage H Shipped: <HASH> on 2026-05-09
   ```

- [ ] **Step 3: Run final verification**

Run: `cargo build -p alephcore && cargo test --test worktree_isolation`
Expected: Build clean; all 5 H-T tests pass.

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/specs/2026-05-08-subagent-uplift-roadmap-design.md
git commit -m "docs(roadmap): mark P3 Stage H ✅ Shipped

Worktree isolation primitive + WorktreeSandbox + spawner integration
shipped via this PR. Stage I and Stage J remain 📋 Planned.

R10 baseline preserved: src/harness/agent.rs zero diff vs P2 closure;
trace.rs +4 lines (schema-only enum variants).

Refs: docs/superpowers/specs/2026-05-09-subagent-uplift-p3-design.md"
```

---

## Self-Review Checklist (executed after writing this plan)

**1. Spec coverage:**
- [x] § 2.1 Problem — addressed by IsolationMode + WorktreeHandle (Tasks 1, 3-5)
- [x] § 2.2.1 New file `src/sandbox/worktree.rs` — Tasks 2-7
- [x] § 2.2.2 Wiring — Tasks 8-9
- [x] § 2.2.3 trace events — Task 6
- [x] § 2.3 Failure modes — covered in Tasks 5 (Drop), 9 (spawner err propagation)
- [x] § 2.4 Tests H-T1..H-T6 — Task 9 (T1), Task 10 (T2-T5), Task 3 (T6 via `create_in_non_git_dir_fails_with_not_a_git_repo`)
- [x] § 2.5 File budget — Task 11 includes verification
- [x] § 4 Cross-stage invariants R10 — Task 11 step 2 verifies
- [x] § 7 Acceptance criteria — covered by tests + Task 11 docs

**2. Placeholder scan:**
- No "TBD", "TODO", "fill in details", or vague handoffs.
- All "Implementer note" callouts point to *specific* known-uncertain points (SandboxCommand field names, where to declare `worktree` variable) with concrete pivots.

**3. Type consistency:**
- `IsolationMode::Worktree` referenced consistently in Tasks 1, 8, 9, 11.
- `WorktreeHandle` / `WorktreeError` / `WorktreeSandbox` capitalization consistent throughout.
- `LoopTraceEvent::WorktreeCreated { path }` and `LoopTraceEvent::WorktreeCleanedUp { path, leaked }` field shapes match design § 2.2.3 and trace.rs additions in Task 6.
- `SpawnRequest.isolation: Option<IsolationMode>` matches design § 1.4 schema table.

**Plan complete.**
