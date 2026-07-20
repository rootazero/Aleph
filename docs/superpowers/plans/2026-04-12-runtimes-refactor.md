# Runtimes Module Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify `src/runtimes/` and `src/browser/bootstrap.rs` into a single cross-OS runtime manager driven by one `SPECS` table; expose read-only Runtimes dashboard view in Panel; preserve existing `ensure_capability()` API and LLM prompt injection.

**Architecture:** Spec-table-driven — one `RuntimeSpec` per capability with per-OS install strategies (Shell / PowerShell / Via-parent) and chained post-install actions. Probe first, install on demand, persist via ledger. Browser `playwright-cli` becomes a capability with `node → fnm` deps and `chromium + skills` as post-install actions, eliminating the duplicated bootstrap machinery.

**Tech Stack:** Rust (alephcore), tokio::process, serde + schemars, Leptos 0.8 (aleph-panel webchat crate), axum gateway, fnm / uv / future rustup as language-native runtimes.

---

## Spec Reference

`docs/superpowers/specs/2026-04-12-runtimes-refactor-design.md`

## File Structure

### Created (6 files)

| Path | Responsibility |
|---|---|
| `src/runtimes/os.rs` | `TargetOs` enum + `select_install()` OS matcher |
| `src/runtimes/specs.rs` | `SPECS` static table + `RuntimeSpec` / `OsInstall` / `InstallStrategy` / `PostInstallAction` types |
| `src/runtimes/post_install.rs` | Three post-install runners (RunSubcommand / FnmAlias / AssetProbe) |
| `src/gateway/handlers/runtimes.rs` | `runtimes.list` / `runtimes.install` / `runtimes.refresh` RPCs |
| `interfaces/webchat/src/views/runtimes.rs` | Leptos Runtimes dashboard view + RuntimeCard component |
| `interfaces/webchat/src/api/runtimes.rs` | RPC client types + helpers |

### Rewritten (3 files)

| Path | Change |
|---|---|
| `src/runtimes/probe.rs` | Read `SPECS` instead of hardcoded `PROBE_SPECS`; drop `aleph_paths`; support Windows `where` |
| `src/runtimes/bootstrap.rs` | Read `SPECS` instead of hardcoded `BOOTSTRAP_SPECS`; `InstallStrategy` dispatcher; post-install chain |
| `src/runtimes/capability.rs` | Delete hardcoded `get_usage_hints()`; read `SPECS[name].llm_hint` |

### Modified (6 files)

| Path | Change |
|---|---|
| `src/runtimes/mod.rs` | Export new modules (`os`, `specs`, `post_install`); re-export `RuntimeSpec` |
| `src/runtimes/ensure.rs` | Adapt to new `bootstrap::install` signature; deps from `SPECS[name].deps` |
| `src/browser/playwright_cli.rs` | `resolve_binary()` calls `ensure_capability("playwright-cli", ledger)` on miss |
| `src/gateway/handlers/mod.rs` | Register `runtimes.*`; unregister `browser.runtime_*` |
| `src/gateway/event_bus.rs` | Rename `BrowserInstallProgressEvent` → `RuntimeInstallProgressEvent` |
| `interfaces/webchat/src/views/mod.rs` | Export `runtimes`; add sidebar entry |

### Deleted (4 files/sections)

- `src/browser/bootstrap.rs`
- `src/gateway/handlers/browser_runtime.rs`
- `interfaces/webchat/src/views/settings/browser_runtime.rs`
- `interfaces/webchat/src/api/browser.rs` section: `BrowserRuntimeApi`, `RuntimeStatusResponse`, `ComponentStatus` (moved to `api/runtimes.rs`)

---

## Global Conventions

- **Branch**: `main` (single-branch policy per project CLAUDE.md; do NOT create worktrees)
- **Crate names**: `alephcore` (core Rust), `aleph-panel` (webchat Leptos crate at `interfaces/webchat/`)
- **Commit style**: `<scope>: <description>` — e.g. `runtimes: add TargetOs abstraction`
- **Lock primitives**: use `crate::sync_primitives::{RwLock, Arc}` (existing in codebase)
- **After every task**: run `cargo check -p alephcore 2>&1 | tail -20` to confirm tree compiles
- **Current HEAD before start**: spec commit `d2c7100d` (`docs: add design spec for runtimes module refactor`)

---

## Task 1: Add `TargetOs` abstraction + `select_install`

**Files:**
- Create: `src/runtimes/os.rs`
- Modify: `src/runtimes/mod.rs`

- [ ] **Step 1: Write failing tests**

Create `src/runtimes/os.rs`:

```rust
//! OS detection and cross-OS install selection.

/// Target operating system for runtime install strategies.
///
/// Concrete variants (`MacOs`, `Linux`, `Windows`) are returned by `current()`.
/// Wildcard variants (`AnyUnix`, `AnyOs`) are only valid inside `OsInstall`
/// spec entries — they match multiple concrete OSes for DRY specs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    MacOs,
    Linux,
    Windows,
    /// Matches MacOs or Linux
    AnyUnix,
    /// Matches any concrete OS
    AnyOs,
}

impl TargetOs {
    /// Detect the current OS at runtime.
    ///
    /// Panics on unsupported OSes at compile time via `cfg!` gating.
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            panic!("unsupported OS for Aleph runtimes")
        }
    }

    /// Check whether this (possibly wildcard) target matches a concrete OS.
    pub fn matches(&self, current: TargetOs) -> bool {
        match (*self, current) {
            (Self::AnyOs, _) => true,
            (Self::AnyUnix, Self::MacOs | Self::Linux) => true,
            (a, b) => a == b,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_concrete_matches_self() {
        assert!(TargetOs::MacOs.matches(TargetOs::MacOs));
        assert!(TargetOs::Linux.matches(TargetOs::Linux));
        assert!(TargetOs::Windows.matches(TargetOs::Windows));
    }

    #[test]
    fn test_concrete_does_not_match_other_concrete() {
        assert!(!TargetOs::MacOs.matches(TargetOs::Linux));
        assert!(!TargetOs::Linux.matches(TargetOs::Windows));
        assert!(!TargetOs::Windows.matches(TargetOs::MacOs));
    }

    #[test]
    fn test_any_unix_matches_mac_and_linux() {
        assert!(TargetOs::AnyUnix.matches(TargetOs::MacOs));
        assert!(TargetOs::AnyUnix.matches(TargetOs::Linux));
        assert!(!TargetOs::AnyUnix.matches(TargetOs::Windows));
    }

    #[test]
    fn test_any_os_matches_all() {
        assert!(TargetOs::AnyOs.matches(TargetOs::MacOs));
        assert!(TargetOs::AnyOs.matches(TargetOs::Linux));
        assert!(TargetOs::AnyOs.matches(TargetOs::Windows));
    }

    #[test]
    fn test_current_returns_concrete() {
        let os = TargetOs::current();
        assert!(matches!(os, TargetOs::MacOs | TargetOs::Linux | TargetOs::Windows));
    }
}
```

- [ ] **Step 2: Register in `mod.rs`**

Edit `src/runtimes/mod.rs`. Add near other `pub mod` lines:

```rust
pub mod os;
pub use os::TargetOs;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib runtimes::os 2>&1 | tail -15`
Expected: 5 tests pass.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: zero errors.

- [ ] **Step 5: Commit**

```bash
git add src/runtimes/os.rs src/runtimes/mod.rs
git commit -m "runtimes: add TargetOs abstraction with wildcard matching"
```

---

## Task 2: Add `SPECS` table + `RuntimeSpec` types

**Files:**
- Create: `src/runtimes/specs.rs`
- Modify: `src/runtimes/mod.rs`

- [ ] **Step 1: Create `specs.rs` with types**

Create `src/runtimes/specs.rs`:

```rust
//! Runtime specification table — single source of truth for probe/install/LLM-hint.

use super::os::TargetOs;

/// Single runtime capability description.
pub struct RuntimeSpec {
    /// Capability name — matches ledger key.
    pub name: &'static str,
    /// Binary names to probe in PATH (priority order).
    pub binaries: &'static [&'static str],
    /// Version flag (e.g. "--version").
    pub version_flag: &'static str,
    /// Regex to extract version from binary output.
    pub version_regex: &'static str,
    /// Minimum acceptable version as "major.minor"; None = accept any.
    pub min_version: Option<&'static str>,
    /// Capabilities that must be Ready before installing this one.
    pub deps: &'static [&'static str],
    /// Per-OS install strategies (first match wins).
    pub install: &'static [OsInstall],
    /// Post-install actions (run in order after binary probes Ready).
    pub post_install: &'static [PostInstallAction],
    /// LLM system-prompt usage hint (markdown snippet).
    pub llm_hint: Option<&'static str>,
}

/// One install strategy scoped to a specific OS (or wildcard).
pub struct OsInstall {
    pub os: TargetOs,
    pub strategy: InstallStrategy,
}

/// How to install a runtime on a particular OS.
pub enum InstallStrategy {
    /// Raw shell script (POSIX). Invoked via `sh -c`.
    Shell(&'static str),
    /// PowerShell invocation (Windows).
    PowerShell(&'static str),
    /// Delegate to a parent capability's subcommand.
    /// Parent must be in this spec's `deps`.
    Via {
        parent: &'static str,
        subcommand: &'static [&'static str],
    },
    /// This OS is explicitly unsupported; surface a clear error.
    Unsupported {
        reason: &'static str,
    },
}

/// Action to run after a capability's binary is installed and probes Ready.
pub enum PostInstallAction {
    /// Run the just-installed binary with args; optional `target_dir` with
    /// `$HOME` expansion is appended after `args`.
    RunSubcommand {
        args: &'static [&'static str],
        target_dir: Option<&'static str>,
    },
    /// After `fnm install --lts`, create an `<alias_name>` alias pointing at
    /// the newly installed version (fnm doesn't do this automatically).
    FnmAlias {
        alias_name: &'static str,
    },
    /// Verify an asset file/dir exists; if missing, run `repair` args to fix.
    AssetProbe {
        path: &'static str,
        repair: &'static [&'static str],
    },
}

/// The authoritative runtime spec table.
pub const SPECS: &[RuntimeSpec] = &[
    RuntimeSpec {
        name: "fnm",
        binaries: &["fnm"],
        version_flag: "--version",
        version_regex: r"fnm (\d+\.\d+\.\d+)",
        min_version: None,
        deps: &[],
        install: &[
            OsInstall {
                os: TargetOs::AnyUnix,
                strategy: InstallStrategy::Shell(
                    "curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell",
                ),
            },
            OsInstall {
                os: TargetOs::Windows,
                strategy: InstallStrategy::PowerShell(
                    "winget install Schniz.fnm --silent --accept-source-agreements",
                ),
            },
        ],
        post_install: &[],
        llm_hint: Some("Node version manager (fnm). Used implicitly by `node`."),
    },
    RuntimeSpec {
        name: "node",
        binaries: &["node"],
        version_flag: "--version",
        version_regex: r"v(\d+\.\d+\.\d+)",
        min_version: Some("18.0"),
        deps: &["fnm"],
        install: &[OsInstall {
            os: TargetOs::AnyOs,
            strategy: InstallStrategy::Via {
                parent: "fnm",
                subcommand: &["install", "--lts"],
            },
        }],
        post_install: &[PostInstallAction::FnmAlias { alias_name: "lts" }],
        llm_hint: Some(
            "Node.js runtime. Use via `fnm exec --using lts -- node <script.js>`.",
        ),
    },
    RuntimeSpec {
        name: "uv",
        binaries: &["uv"],
        version_flag: "--version",
        version_regex: r"uv (\d+\.\d+\.\d+)",
        min_version: None,
        deps: &[],
        install: &[
            OsInstall {
                os: TargetOs::AnyUnix,
                strategy: InstallStrategy::Shell(
                    "curl -LsSf https://astral.sh/uv/install.sh | sh",
                ),
            },
            OsInstall {
                os: TargetOs::Windows,
                strategy: InstallStrategy::PowerShell(
                    "powershell -ExecutionPolicy ByPass -c \"irm https://astral.sh/uv/install.ps1 | iex\"",
                ),
            },
        ],
        post_install: &[],
        llm_hint: Some(
            "Python package manager (uv). Run scripts via `uv run <file.py>`; install packages via `uv pip install <pkg>`.",
        ),
    },
    RuntimeSpec {
        name: "playwright-cli",
        binaries: &["playwright-cli"],
        version_flag: "--version",
        version_regex: r"(\d+\.\d+\.\d+)",
        min_version: None,
        deps: &["node"],
        install: &[OsInstall {
            os: TargetOs::AnyOs,
            strategy: InstallStrategy::Via {
                parent: "node",
                subcommand: &["--", "npm", "install", "-g", "@playwright/cli@latest"],
            },
        }],
        post_install: &[
            PostInstallAction::RunSubcommand {
                args: &["install", "chromium"],
                target_dir: None,
            },
            PostInstallAction::RunSubcommand {
                args: &["install", "--skills", "--target"],
                target_dir: Some("$HOME/.aleph/skills/playwright-cli"),
            },
        ],
        llm_hint: Some(
            "Browser automation CLI. Use `playwright-cli -s=<session> <command>`.",
        ),
    },
    // Placeholder for future cargo/Rust support — empty install array.
    RuntimeSpec {
        name: "cargo",
        binaries: &["cargo"],
        version_flag: "--version",
        version_regex: r"cargo (\d+\.\d+\.\d+)",
        min_version: None,
        deps: &[],
        install: &[],
        post_install: &[],
        llm_hint: None,
    },
];

/// Look up a spec by capability name.
pub fn find_spec(name: &str) -> Option<&'static RuntimeSpec> {
    SPECS.iter().find(|s| s.name == name)
}

/// Select the first `OsInstall` in `installs` whose `os` matches `current`.
pub fn select_install<'a>(
    installs: &'a [OsInstall],
    current: TargetOs,
) -> Option<&'a OsInstall> {
    installs.iter().find(|oi| oi.os.matches(current))
}

/// Whether the given capability has an installable strategy on the current OS.
pub fn supported_on_current_os(name: &str) -> bool {
    find_spec(name)
        .and_then(|s| select_install(s.install, TargetOs::current()))
        .map(|oi| !matches!(oi.strategy, InstallStrategy::Unsupported { .. }))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_specs_have_nonempty_name() {
        for spec in SPECS {
            assert!(!spec.name.is_empty(), "spec name must not be empty");
        }
    }

    #[test]
    fn test_find_spec_known() {
        assert!(find_spec("fnm").is_some());
        assert!(find_spec("node").is_some());
        assert!(find_spec("uv").is_some());
        assert!(find_spec("playwright-cli").is_some());
        assert!(find_spec("cargo").is_some());
    }

    #[test]
    fn test_find_spec_unknown() {
        assert!(find_spec("does-not-exist").is_none());
    }

    #[test]
    fn test_select_install_first_match() {
        let spec = find_spec("fnm").unwrap();
        let sel = select_install(spec.install, TargetOs::MacOs).unwrap();
        assert!(matches!(sel.strategy, InstallStrategy::Shell(_)));
    }

    #[test]
    fn test_select_install_windows() {
        let spec = find_spec("fnm").unwrap();
        let sel = select_install(spec.install, TargetOs::Windows).unwrap();
        assert!(matches!(sel.strategy, InstallStrategy::PowerShell(_)));
    }

    #[test]
    fn test_supported_on_current_os_for_real_specs() {
        // fnm has Shell on AnyUnix + PowerShell on Windows — all 3 OSes supported.
        assert!(supported_on_current_os("fnm"));
    }

    #[test]
    fn test_supported_on_current_os_for_cargo_placeholder() {
        // cargo has empty install array — not supported anywhere yet.
        assert!(!supported_on_current_os("cargo"));
    }

    #[test]
    fn test_deps_reference_known_specs() {
        for spec in SPECS {
            for dep in spec.deps {
                assert!(
                    find_spec(dep).is_some(),
                    "spec '{}' references unknown dep '{}'",
                    spec.name,
                    dep,
                );
            }
        }
    }

    #[test]
    fn test_via_parent_in_deps() {
        for spec in SPECS {
            for oi in spec.install {
                if let InstallStrategy::Via { parent, .. } = &oi.strategy {
                    assert!(
                        spec.deps.contains(parent),
                        "spec '{}' uses Via {{ parent: '{}' }} but '{}' is not in deps",
                        spec.name,
                        parent,
                        parent,
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 2: Register in `mod.rs`**

Edit `src/runtimes/mod.rs`. Add:

```rust
pub mod specs;
pub use specs::{
    find_spec, select_install, supported_on_current_os, InstallStrategy, OsInstall,
    PostInstallAction, RuntimeSpec, SPECS,
};
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib runtimes::specs 2>&1 | tail -20`
Expected: 9 tests pass.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: zero errors.

- [ ] **Step 5: Commit**

```bash
git add src/runtimes/specs.rs src/runtimes/mod.rs
git commit -m "runtimes: add SPECS table with cross-OS install strategies"
```

---

## Task 3: Add `post_install` module

**Files:**
- Create: `src/runtimes/post_install.rs`
- Modify: `src/runtimes/mod.rs`

- [ ] **Step 1: Create post_install.rs**

Create `src/runtimes/post_install.rs`:

```rust
//! Post-install action runners for runtime specs.

use std::path::PathBuf;

use tokio::process::Command;

use super::specs::PostInstallAction;

/// Errors from post-install actions.
#[derive(Debug, thiserror::Error)]
pub enum PostInstallError {
    #[error("post-install subcommand failed: {stderr}")]
    SubcommandFailed { stderr: String },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not determine Node version for fnm alias")]
    NoNodeVersion,
    #[error("repair command failed for missing asset")]
    RepairFailed,
}

/// Expand `$HOME` in a template path.
fn expand_home(template: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        template.replace("$HOME", &home)
    } else {
        template.to_string()
    }
}

/// Run a single post-install action. `bin_path` is the just-installed
/// capability binary (used for `RunSubcommand` and `AssetProbe`).
pub async fn run(
    action: &PostInstallAction,
    bin_path: &PathBuf,
) -> Result<(), PostInstallError> {
    match action {
        PostInstallAction::RunSubcommand { args, target_dir } => {
            run_subcommand(bin_path, args, *target_dir).await
        }
        PostInstallAction::FnmAlias { alias_name } => create_fnm_alias(alias_name).await,
        PostInstallAction::AssetProbe { path, repair } => {
            verify_or_repair(bin_path, path, repair).await
        }
    }
}

async fn run_subcommand(
    bin_path: &PathBuf,
    args: &[&str],
    target_dir: Option<&str>,
) -> Result<(), PostInstallError> {
    let mut cmd = Command::new(bin_path);
    cmd.args(args);
    if let Some(td) = target_dir {
        let expanded = expand_home(td);
        if let Some(parent) = PathBuf::from(&expanded).parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        cmd.arg(&expanded);
    }
    let output = cmd.output().await?;
    if !output.status.success() {
        return Err(PostInstallError::SubcommandFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        });
    }
    Ok(())
}

async fn create_fnm_alias(alias_name: &str) -> Result<(), PostInstallError> {
    // Parse `fnm list` output to find the just-installed version token.
    let list = Command::new("fnm").args(["list"]).output().await?;
    let text = String::from_utf8_lossy(&list.stdout);
    let version = text
        .lines()
        .filter_map(|l| {
            l.split_whitespace()
                .find(|t| t.starts_with('v'))
                .map(String::from)
        })
        .last()
        .ok_or(PostInstallError::NoNodeVersion)?;
    // Best-effort: failure is not fatal; caller logs it.
    let _ = Command::new("fnm")
        .args(["alias", &version, alias_name])
        .output()
        .await;
    Ok(())
}

async fn verify_or_repair(
    bin_path: &PathBuf,
    path_template: &str,
    repair: &[&str],
) -> Result<(), PostInstallError> {
    let expanded = PathBuf::from(expand_home(path_template));
    if expanded.exists() {
        return Ok(());
    }
    let output = Command::new(bin_path).args(repair).output().await?;
    if !output.status.success() {
        return Err(PostInstallError::RepairFailed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_home_with_var() {
        std::env::set_var("HOME", "/tmp/fake-home");
        let out = expand_home("$HOME/.aleph/skills");
        assert_eq!(out, "/tmp/fake-home/.aleph/skills");
    }

    #[test]
    fn test_expand_home_no_placeholder() {
        let out = expand_home("/absolute/no/expansion");
        assert_eq!(out, "/absolute/no/expansion");
    }
}
```

- [ ] **Step 2: Register in `mod.rs`**

Add:

```rust
pub mod post_install;
pub use post_install::PostInstallError;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib runtimes::post_install 2>&1 | tail -15`
Expected: 2 tests pass.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: zero errors.

- [ ] **Step 5: Commit**

```bash
git add src/runtimes/post_install.rs src/runtimes/mod.rs
git commit -m "runtimes: add post_install action runners"
```

---

## Task 4: Rewrite `probe.rs` to read SPECS + support Windows

**Files:**
- Modify: `src/runtimes/probe.rs` (full rewrite)

- [ ] **Step 1: Rewrite file**

Replace `src/runtimes/probe.rs` entirely with:

```rust
//! Probe module — detects installed runtimes by checking PATH.
//!
//! Reads spec data from `super::specs::SPECS`. Does NOT install anything —
//! only reports whether a binary is present and its version.

use crate::sync_primitives::Mutex;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, trace, warn};

use crate::runtimes::ledger::CapabilitySource;
use crate::runtimes::specs::{find_spec, RuntimeSpec};

/// Result of probing for a capability.
#[derive(Debug)]
pub struct ProbeResult {
    pub found: bool,
    pub bin_path: Option<PathBuf>,
    pub version: Option<String>,
    pub source: CapabilitySource,
    pub version_warning: Option<String>,
}

impl ProbeResult {
    fn not_found() -> Self {
        Self {
            found: false,
            bin_path: None,
            version: None,
            source: CapabilitySource::System,
            version_warning: None,
        }
    }
}

/// Probe for a named capability. Returns a `ProbeResult` describing what
/// was found on the system PATH (or nothing).
pub fn probe(name: &str) -> ProbeResult {
    let spec = match find_spec(name) {
        Some(s) => s,
        None => {
            debug!("no spec for capability '{}'", name);
            return ProbeResult::not_found();
        }
    };

    if let Some(result) = probe_system_path(spec) {
        debug!(
            "found '{}' on system PATH: {:?}",
            name,
            result.bin_path.as_deref().unwrap_or(Path::new("?"))
        );
        return result;
    }

    debug!("capability '{}' not found", name);
    ProbeResult::not_found()
}

fn probe_system_path(spec: &RuntimeSpec) -> Option<ProbeResult> {
    let locator = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    for bin_name in spec.binaries {
        trace!("looking for '{}' via {}", bin_name, locator);
        let output = Command::new(locator).arg(bin_name).output().ok()?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            // On Windows, `where` can return multiple lines; take the first.
            let path_str = stdout.lines().next().unwrap_or("").trim().to_string();
            if path_str.is_empty() {
                continue;
            }
            let bin_path = PathBuf::from(&path_str);
            let version = get_version(&bin_path, spec.version_flag, spec.version_regex);
            let version_warning = check_version_warning(spec, version.as_deref());
            return Some(ProbeResult {
                found: true,
                bin_path: Some(bin_path),
                version,
                source: CapabilitySource::System,
                version_warning,
            });
        }
    }
    None
}

static REGEX_CACHE: Lazy<Mutex<HashMap<&'static str, Regex>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn get_compiled_regex(pattern: &'static str) -> Option<Regex> {
    let mut cache = REGEX_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(re) = cache.get(pattern) {
        return Some(re.clone());
    }
    match Regex::new(pattern) {
        Ok(re) => {
            cache.insert(pattern, re.clone());
            Some(re)
        }
        Err(e) => {
            warn!("invalid version regex '{}': {}", pattern, e);
            None
        }
    }
}

fn get_version(
    bin_path: &Path,
    version_flag: &str,
    version_regex: &'static str,
) -> Option<String> {
    let output = Command::new(bin_path).arg(version_flag).output().ok()?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let re = get_compiled_regex(version_regex)?;
    re.captures(&combined)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
}

fn check_version_warning(spec: &RuntimeSpec, version: Option<&str>) -> Option<String> {
    let min = spec.min_version?;
    let actual = version?;
    if version_lt(actual, min) {
        Some(format!(
            "{} version {} is below minimum {} — some features may not work",
            spec.name, actual, min
        ))
    } else {
        None
    }
}

/// Simple semver comparison on major.minor only.
fn version_lt(actual: &str, minimum: &str) -> bool {
    let parse = |s: &str| -> (u64, u64) {
        let mut parts = s.split('.');
        let major = parts.next().and_then(|p| p.parse::<u64>().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|p| p.parse::<u64>().ok()).unwrap_or(0);
        (major, minor)
    };
    parse(actual) < parse(minimum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_lt_basics() {
        assert!(version_lt("3.9", "3.10"));
        assert!(!version_lt("3.12", "3.10"));
        assert!(!version_lt("3.10", "3.10"));
    }

    #[test]
    fn test_version_lt_ignores_patch() {
        assert!(version_lt("3.9.7", "3.10"));
        assert!(!version_lt("3.12.1", "3.10"));
    }

    #[test]
    fn test_probe_unknown_returns_not_found() {
        let r = probe("nonexistent_capability_xyz");
        assert!(!r.found);
        assert!(r.bin_path.is_none());
    }

    #[test]
    fn test_probe_known_spec_consistency() {
        // fnm may or may not be on the test machine; just assert the contract.
        let r = probe("fnm");
        if r.found {
            assert!(r.bin_path.is_some());
        } else {
            assert!(r.bin_path.is_none());
        }
    }

    #[test]
    fn test_probe_result_not_found_defaults() {
        let r = ProbeResult::not_found();
        assert!(!r.found);
        assert_eq!(r.source, CapabilitySource::System);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib runtimes::probe 2>&1 | tail -20`
Expected: 5 tests pass.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: zero errors. If `ensure.rs` or `capability.rs` break (they import old probe internals), the fix goes in Task 6/7 — for now note the errors but proceed; the next tasks will resolve them.

If compile breaks because `ensure.rs` references removed items, temporarily stub in `ensure.rs` with `todo!()` on the broken lines so the tree keeps compiling. This is temporary; Task 7 rewrites `ensure.rs`.

- [ ] **Step 4: Commit**

```bash
git add src/runtimes/probe.rs
git commit -m "runtimes: rewrite probe to read SPECS and support Windows"
```

---

## Task 5: Rewrite `bootstrap.rs` with InstallStrategy dispatcher

**Files:**
- Modify: `src/runtimes/bootstrap.rs` (full rewrite)
- Modify: `src/runtimes/mod.rs` (adjust re-exports)

- [ ] **Step 1: Rewrite file**

Replace `src/runtimes/bootstrap.rs` entirely with:

```rust
//! Runtime install dispatcher driven by `super::specs::SPECS`.

use std::path::PathBuf;

use tokio::process::Command;

use super::os::TargetOs;
use super::post_install;
use super::probe;
use super::specs::{find_spec, select_install, InstallStrategy, RuntimeSpec};

/// Result of a bootstrap attempt.
#[derive(Debug)]
pub enum BootstrapResult {
    Success { bin_path: PathBuf, version: String },
    PathNotFound { expected: String },
    Failed { stderr: String },
    Unsupported { capability: String, reason: String },
    UnknownCapability { capability: String },
}

/// Errors raised by the dispatcher itself (not captured in BootstrapResult).
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("post-install action failed: {0}")]
    PostInstall(#[from] post_install::PostInstallError),
    #[error("unknown capability: {0}")]
    Unknown(String),
}

/// Install a capability according to its spec. Assumes `deps` are already Ready
/// (caller handles dep resolution).
pub async fn install(name: &str) -> Result<BootstrapResult, BootstrapError> {
    let spec = match find_spec(name) {
        Some(s) => s,
        None => {
            return Ok(BootstrapResult::UnknownCapability {
                capability: name.into(),
            });
        }
    };

    if spec.install.is_empty() {
        return Ok(BootstrapResult::Unsupported {
            capability: name.into(),
            reason: "no install strategy defined for this capability".into(),
        });
    }

    let current = TargetOs::current();
    let os_install = match select_install(spec.install, current) {
        Some(oi) => oi,
        None => {
            return Ok(BootstrapResult::Unsupported {
                capability: name.into(),
                reason: format!("no install strategy for {:?}", current),
            });
        }
    };

    // 1. Run the install command.
    let cmd_result = match &os_install.strategy {
        InstallStrategy::Shell(script) => run_shell(script).await?,
        InstallStrategy::PowerShell(script) => run_powershell(script).await?,
        InstallStrategy::Via { parent, subcommand } => run_via_parent(parent, subcommand).await?,
        InstallStrategy::Unsupported { reason } => {
            return Ok(BootstrapResult::Unsupported {
                capability: name.into(),
                reason: (*reason).into(),
            });
        }
    };

    if let CmdOutcome::Failed { stderr } = cmd_result {
        return Ok(BootstrapResult::Failed { stderr });
    }

    // 2. Re-probe to get binary path + version.
    let probe_result = probe::probe(name);
    if !probe_result.found {
        return Ok(BootstrapResult::PathNotFound {
            expected: format!("binary '{}' on PATH after install", name),
        });
    }
    let bin_path = probe_result.bin_path.clone().unwrap();

    // 3. Run post-install actions.
    for action in spec.post_install {
        post_install::run(action, &bin_path).await?;
    }

    Ok(BootstrapResult::Success {
        bin_path,
        version: probe_result.version.unwrap_or_default(),
    })
}

/// Whether a bootstrap spec exists for this capability.
pub fn has_spec(capability: &str) -> bool {
    find_spec(capability).is_some()
}

/// Dependencies that must be Ready before installing this capability.
pub fn dependencies(capability: &str) -> &'static [&'static str] {
    find_spec(capability).map(|s| s.deps).unwrap_or(&[])
}

enum CmdOutcome {
    Success,
    Failed { stderr: String },
}

async fn run_shell(script: &str) -> Result<CmdOutcome, BootstrapError> {
    let output = Command::new("sh").args(["-c", script]).output().await?;
    if output.status.success() {
        Ok(CmdOutcome::Success)
    } else {
        Ok(CmdOutcome::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        })
    }
}

async fn run_powershell(script: &str) -> Result<CmdOutcome, BootstrapError> {
    let output = Command::new("powershell")
        .args(["-Command", script])
        .output()
        .await?;
    if output.status.success() {
        Ok(CmdOutcome::Success)
    } else {
        Ok(CmdOutcome::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        })
    }
}

async fn run_via_parent(
    parent: &str,
    subcommand: &[&str],
) -> Result<CmdOutcome, BootstrapError> {
    let output = match parent {
        "fnm" => {
            Command::new("fnm")
                .args(subcommand)
                .output()
                .await?
        }
        "node" => {
            // Wrap in `fnm exec --using lts --` to get a Node shell with PATH.
            let mut args: Vec<&str> = vec!["exec", "--using", "lts"];
            args.extend(subcommand.iter().copied());
            Command::new("fnm").args(&args).output().await?
        }
        "uv" => Command::new("uv").args(subcommand).output().await?,
        "cargo" => Command::new("cargo").args(subcommand).output().await?,
        _ => {
            return Ok(CmdOutcome::Failed {
                stderr: format!("unknown Via parent: {}", parent),
            });
        }
    };
    if output.status.success() {
        Ok(CmdOutcome::Success)
    } else {
        Ok(CmdOutcome::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_spec_known() {
        assert!(has_spec("fnm"));
        assert!(has_spec("node"));
        assert!(has_spec("playwright-cli"));
    }

    #[test]
    fn test_has_spec_unknown() {
        assert!(!has_spec("ruby"));
    }

    #[test]
    fn test_dependencies_from_specs() {
        assert_eq!(dependencies("fnm"), &[] as &[&str]);
        assert_eq!(dependencies("node"), &["fnm"]);
        assert_eq!(dependencies("playwright-cli"), &["node"]);
    }

    #[tokio::test]
    async fn test_install_unknown_capability() {
        let result = install("totally-unknown-capability").await.unwrap();
        assert!(matches!(result, BootstrapResult::UnknownCapability { .. }));
    }

    #[tokio::test]
    async fn test_install_empty_install_array_returns_unsupported() {
        // cargo has empty install array — should return Unsupported, not error.
        let result = install("cargo").await.unwrap();
        assert!(matches!(result, BootstrapResult::Unsupported { .. }));
    }
}
```

- [ ] **Step 2: Update mod.rs exports**

Edit `src/runtimes/mod.rs`. Ensure `bootstrap` re-exports include `BootstrapResult` and `BootstrapError`:

```rust
pub use bootstrap::{dependencies, has_spec, BootstrapError, BootstrapResult};
```

Remove any reference to old `BootstrapSpec` type from re-exports.

- [ ] **Step 3: Run tests**

Run: `cargo test -p alephcore --lib runtimes::bootstrap 2>&1 | tail -20`
Expected: 5 tests pass.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -30`

Expected errors: `src/runtimes/ensure.rs` still imports the old `BootstrapResult::Success { bin_path }` shape (no `version` field) and calls the old `bootstrap::bootstrap()` (renamed to `install()`). Fix in Task 6.

For now, if needed, temporarily mark `ensure.rs::ensure_capability` body with `unimplemented!()` so the crate compiles. Restore in Task 6.

- [ ] **Step 5: Commit**

```bash
git add src/runtimes/bootstrap.rs src/runtimes/mod.rs src/runtimes/ensure.rs
git commit -m "runtimes: rewrite bootstrap with InstallStrategy dispatcher"
```

---

## Task 6: Adapt `ensure.rs` to new bootstrap signature

**Files:**
- Modify: `src/runtimes/ensure.rs`

- [ ] **Step 1: Rewrite ensure.rs**

Replace the body of `ensure_capability` in `src/runtimes/ensure.rs`:

```rust
//! Capability orchestration — Probe → Bootstrap → Ledger update.

use crate::error::AlephError;
use crate::runtimes::bootstrap::{self, BootstrapResult};
use crate::runtimes::ledger::{
    CapabilityEntry, CapabilityLedger, CapabilitySource, CapabilityStatus,
};
use crate::runtimes::probe;
use crate::sync_primitives::Arc;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Ensure a capability is Ready, probing and bootstrapping as needed.
pub async fn ensure_capability(
    capability: &str,
    ledger: &Arc<RwLock<CapabilityLedger>>,
) -> Result<PathBuf, AlephError> {
    // Fast path: already Ready in ledger.
    {
        let mut guard = ledger.write().await;
        if guard.status(capability) == CapabilityStatus::Ready {
            if let Some(path) = guard.executable(capability) {
                if path.exists() {
                    return Ok(path.to_path_buf());
                }
                warn!("capability {} path missing, marking stale", capability);
                guard.update_status(capability, CapabilityStatus::Stale);
            }
        }
    }

    // Probe phase.
    info!("probing for capability: {}", capability);
    {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Probing);
    }
    let probe_result = probe::probe(capability);

    if probe_result.found {
        let bin_path = probe_result
            .bin_path
            .clone()
            .ok_or_else(|| AlephError::other(format!("{} found but no bin_path", capability)))?;
        if let Some(ref w) = probe_result.version_warning {
            warn!("{}", w);
        }
        let now = now_secs();
        let mut guard = ledger.write().await;
        guard.update(CapabilityEntry {
            name: capability.into(),
            bin_path: bin_path.clone(),
            version: probe_result.version.unwrap_or_default(),
            status: CapabilityStatus::Ready,
            source: probe_result.source,
            last_probed: now,
        });
        let _ = guard.persist();
        return Ok(bin_path);
    }

    // Dependencies first.
    for dep in bootstrap::dependencies(capability) {
        Box::pin(ensure_capability(dep, ledger)).await?;
    }

    if !bootstrap::has_spec(capability) {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Missing);
        return Err(AlephError::runtime(
            capability,
            format!("no bootstrap spec for '{}'", capability),
        ));
    }

    info!("bootstrapping capability: {}", capability);
    {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Bootstrapping);
    }

    let cap_owned = capability.to_string();
    let bootstrap_result = bootstrap::install(&cap_owned).await.map_err(|e| {
        AlephError::runtime(capability, format!("bootstrap dispatcher error: {}", e))
    })?;

    let now = now_secs();
    match bootstrap_result {
        BootstrapResult::Success { bin_path, version } => {
            let mut guard = ledger.write().await;
            guard.update(CapabilityEntry {
                name: capability.into(),
                bin_path: bin_path.clone(),
                version,
                status: CapabilityStatus::Ready,
                source: CapabilitySource::System,
                last_probed: now,
            });
            let _ = guard.persist();
            info!("capability {} ready at {}", capability, bin_path.display());
            Ok(bin_path)
        }
        BootstrapResult::PathNotFound { expected } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(AlephError::runtime(
                capability,
                format!("install completed but binary missing: {}", expected),
            ))
        }
        BootstrapResult::Failed { stderr } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(AlephError::runtime(
                capability,
                format!("install failed: {}", stderr),
            ))
        }
        BootstrapResult::Unsupported { capability: cap, reason } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(AlephError::runtime(
                &cap,
                format!("not supported: {}", reason),
            ))
        }
        BootstrapResult::UnknownCapability { capability: cap } => {
            Err(AlephError::runtime(&cap, "unknown capability"))
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_ensure_already_ready_returns_cached_path() {
        let dir = TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let mut ledger = CapabilityLedger::load_or_create(ledger_path);
        let bin = PathBuf::from("/bin/sh");
        ledger.update(CapabilityEntry {
            name: "test-shell".into(),
            bin_path: bin.clone(),
            version: "1.0".into(),
            status: CapabilityStatus::Ready,
            source: CapabilitySource::System,
            last_probed: now_secs(),
        });
        let ledger = Arc::new(RwLock::new(ledger));
        let result = ensure_capability("test-shell", &ledger).await;
        assert_eq!(result.unwrap(), bin);
    }

    #[tokio::test]
    async fn test_ensure_unknown_capability_errors() {
        let dir = TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(ledger_path)));
        let result = ensure_capability("totally-unknown-xyz", &ledger).await;
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p alephcore --lib runtimes::ensure 2>&1 | tail -20`
Expected: 2 tests pass.

- [ ] **Step 3: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -30`
Expected: `ensure.rs` no longer has compile errors. `capability.rs` may still reference the old `get_usage_hints` — fix in Task 7.

- [ ] **Step 4: Commit**

```bash
git add src/runtimes/ensure.rs
git commit -m "runtimes: adapt ensure.rs to new bootstrap signature"
```

---

## Task 7: Wire `capability.rs` LLM hints from SPECS

**Files:**
- Modify: `src/runtimes/capability.rs`

- [ ] **Step 1: Remove `get_usage_hints` hardcoded match**

Edit `src/runtimes/capability.rs`. Delete the entire standalone `fn get_usage_hints(runtime_id: &str) -> String { match ... }` function (around lines 73-90 in current file).

Also delete the method `RuntimeCapability::get_usage_hints` that calls into it.

- [ ] **Step 2: Replace with SPECS-driven lookup**

Add this helper in its place:

```rust
/// Get the LLM usage hint for a runtime from its spec.
fn get_hint_from_spec(runtime_id: &str) -> Option<&'static str> {
    crate::runtimes::specs::find_spec(runtime_id).and_then(|s| s.llm_hint)
}
```

Update `format_entries_for_prompt` to use the new helper. Find the block that previously called `get_usage_hints(&entry.name)` and replace with:

```rust
        // Reuse existing usage hints from SPECS
        if let Some(hint) = get_hint_from_spec(&entry.name) {
            output.push_str("- ");
            output.push_str(hint);
            output.push('\n');
        }
```

Similarly update `RuntimeCapability::format_for_prompt` to call `get_hint_from_spec(&cap.id)` and append similarly.

- [ ] **Step 3: Update existing tests**

The existing tests `test_usage_hints` and `test_format_single_capability` may fail because the old hardcoded hints are gone. Update test assertions to match what's in SPECS:

- `get_hint_from_spec("uv")` should match `Some("Python package manager (uv). ...")` → test for substring `"Python package manager"` and `"uv pip install"`
- `get_hint_from_spec("node")` should contain `"Node.js runtime"`
- Replace `assert!(RuntimeCapability::get_usage_hints("uv").contains("Python"))` with `assert!(get_hint_from_spec("uv").unwrap_or("").contains("Python"))`.
- Delete any test that referenced `ffmpeg` or `yt-dlp` hints if they no longer exist in SPECS (we intentionally dropped them — the new SPECS focuses on runtime managers, not general-purpose tools).

If `ffmpeg` / `yt-dlp` test assertions exist, just delete those tests; they're no longer part of the refactored scope.

- [ ] **Step 4: Run tests**

Run: `cargo test -p alephcore --lib runtimes::capability 2>&1 | tail -20`
Expected: tests pass (possibly fewer than before; that's OK — we're dropping tests for removed ffmpeg/yt-dlp hints).

- [ ] **Step 5: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -10`
Expected: zero errors.

- [ ] **Step 6: Commit**

```bash
git add src/runtimes/capability.rs
git commit -m "runtimes: read LLM hints from SPECS instead of hardcoded match"
```

---

## Task 8: Browser — delete bootstrap.rs, route through `ensure_capability`

**Files:**
- Delete: `src/browser/bootstrap.rs`
- Modify: `src/browser/playwright_cli.rs`
- Modify: `src/browser/mod.rs`

- [ ] **Step 1: Delete file**

```bash
rm src/browser/bootstrap.rs
```

- [ ] **Step 2: Remove module registration in mod.rs**

Edit `src/browser/mod.rs`. Remove:

```rust
pub mod bootstrap;
pub use bootstrap::{BootstrapStatus, ComponentStatus};
```

- [ ] **Step 3: Route `PlaywrightCliDriver::resolve_binary` through ensure_capability**

Edit `src/browser/playwright_cli.rs`. Find `resolve_binary` method. At its top, after the fast-path cache check and before `resolve_via_fnm()`, add a call to `ensure_capability`. The existing `resolve_binary` ends by calling `resolve_via_fnm()` which does `fnm exec --using lts which playwright-cli` — replace that with:

```rust
// Use runtimes module to ensure playwright-cli + node + fnm + chromium + skills are installed.
// Returns the path to the playwright-cli binary.
use crate::runtimes::{ensure_capability, CapabilityLedger};

// Resolve ledger path on demand; construct a shared ledger.
let runtimes_dir = crate::runtimes::get_runtimes_dir()
    .map_err(|e| BrowserError::PlaywrightCliError(format!("runtimes dir: {e}")))?;
let ledger_path = runtimes_dir.join("ledger.json");
let ledger = std::sync::Arc::new(tokio::sync::RwLock::new(
    CapabilityLedger::load_or_create(ledger_path),
));

let resolved = ensure_capability("playwright-cli", &ledger)
    .await
    .map_err(|e| BrowserError::PlaywrightCliError(format!("ensure playwright-cli: {e}")))?;

*self.binary_path.write().unwrap_or_else(|e| e.into_inner()) = Some(resolved.clone());
Ok(resolved)
```

Delete the private `resolve_via_fnm()` function at the bottom of the file — no longer needed.

- [ ] **Step 4: Compile check**

Run: `cargo check -p alephcore 2>&1 | tail -30`

Expected errors: callers of `browser::bootstrap::*` in `src/gateway/handlers/browser_runtime.rs` and `interfaces/webchat/src/...`. These get handled in Tasks 9 and 11/12 respectively. For now:

Run: `grep -rn "browser::bootstrap" src/ 2>&1`

For each hit, either delete the line (if it's just an import) or stub with `todo!()` so the crate still compiles.

- [ ] **Step 5: Commit**

```bash
git add src/browser/playwright_cli.rs src/browser/mod.rs
git commit -m "browser: delete bootstrap.rs; route playwright-cli through ensure_capability"
```

---

## Task 9: Gateway — new `runtimes.*` RPCs + rename event

**Files:**
- Create: `src/gateway/handlers/runtimes.rs`
- Delete: `src/gateway/handlers/browser_runtime.rs`
- Modify: `src/gateway/handlers/mod.rs`
- Modify: `src/gateway/event_bus.rs`

- [ ] **Step 1: Rename event in event_bus.rs**

Edit `src/gateway/event_bus.rs`.

Rename struct `BrowserInstallProgressEvent` → `RuntimeInstallProgressEvent`. Rename enum variant `GatewayEvent::BrowserInstallProgress(...)` → `GatewayEvent::RuntimeInstallProgress(...)`.

- [ ] **Step 2: Create runtimes RPC handler**

Create `src/gateway/handlers/runtimes.rs`:

```rust
//! Runtime RPC handlers: list + install + refresh.

use std::sync::Arc;

use serde::Serialize;

use crate::gateway::event_bus::{
    GatewayEvent, GatewayEventBus, RuntimeInstallProgressEvent,
};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::runtimes::ledger::{CapabilityLedger, CapabilityStatus};
use crate::runtimes::{ensure_capability, find_spec, supported_on_current_os, SPECS};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub name: String,
    pub status: CapabilityStatus,
    pub bin_path: Option<String>,
    pub version: Option<String>,
    pub llm_hint: Option<String>,
    pub deps: Vec<String>,
    pub supported_on_current_os: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimesListResponse {
    pub runtimes: Vec<RuntimeInfo>,
}

fn build_list(ledger: &CapabilityLedger) -> RuntimesListResponse {
    let runtimes = SPECS
        .iter()
        .map(|spec| {
            let entry = ledger.entries.get(spec.name);
            let status = entry.map(|e| e.status).unwrap_or(CapabilityStatus::Missing);
            let bin_path = entry
                .filter(|e| !e.bin_path.as_os_str().is_empty())
                .map(|e| e.bin_path.to_string_lossy().to_string());
            let version = entry
                .filter(|e| !e.version.is_empty())
                .map(|e| e.version.clone());
            RuntimeInfo {
                name: spec.name.to_string(),
                status,
                bin_path,
                version,
                llm_hint: spec.llm_hint.map(str::to_string),
                deps: spec.deps.iter().map(|d| d.to_string()).collect(),
                supported_on_current_os: supported_on_current_os(spec.name),
            }
        })
        .collect();
    RuntimesListResponse { runtimes }
}

pub async fn handle_list(
    request: JsonRpcRequest,
    ledger: Arc<RwLock<CapabilityLedger>>,
) -> JsonRpcResponse {
    let guard = ledger.read().await;
    let response = build_list(&guard);
    match serde_json::to_value(&response) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("serialize: {e}")),
    }
}

pub async fn handle_refresh(
    request: JsonRpcRequest,
    ledger: Arc<RwLock<CapabilityLedger>>,
) -> JsonRpcResponse {
    // Re-probe each known capability; update ledger.
    for spec in SPECS {
        let probe_result = crate::runtimes::probe::probe(spec.name);
        let mut guard = ledger.write().await;
        if probe_result.found {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            guard.update(crate::runtimes::ledger::CapabilityEntry {
                name: spec.name.to_string(),
                bin_path: probe_result.bin_path.unwrap_or_default(),
                version: probe_result.version.unwrap_or_default(),
                status: CapabilityStatus::Ready,
                source: probe_result.source,
                last_probed: now,
            });
        } else {
            guard.update_status(spec.name, CapabilityStatus::Missing);
        }
    }
    let guard = ledger.read().await;
    let response = build_list(&guard);
    match serde_json::to_value(&response) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("serialize: {e}")),
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct InstallParams {
    pub capability: String,
}

pub async fn handle_install(
    request: JsonRpcRequest,
    ledger: Arc<RwLock<CapabilityLedger>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params: InstallParams = match request.params.clone() {
        Some(p) => match serde_json::from_value(p) {
            Ok(v) => v,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("invalid params: {e}"),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                "missing 'capability' param".into(),
            );
        }
    };

    if find_spec(&params.capability).is_none() {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("unknown capability: {}", params.capability),
        );
    }

    let cap = params.capability.clone();
    let cap_for_event = params.capability.clone();
    let bus = event_bus.clone();

    // Fire-and-forget install.
    tokio::spawn(async move {
        let _ = bus.publish_json(&GatewayEvent::RuntimeInstallProgress(
            RuntimeInstallProgressEvent {
                step: cap_for_event.clone(),
                status: "started".into(),
                log_line: None,
                error: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        ));
        let result = ensure_capability(&cap, &ledger).await;
        let bus2 = bus.clone();
        let event = match result {
            Ok(_) => RuntimeInstallProgressEvent {
                step: cap_for_event,
                status: "done".into(),
                log_line: None,
                error: None,
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
            Err(e) => RuntimeInstallProgressEvent {
                step: cap_for_event,
                status: "failed".into(),
                log_line: None,
                error: Some(e.to_string()),
                timestamp: chrono::Utc::now().timestamp_millis(),
            },
        };
        let _ = bus2.publish_json(&GatewayEvent::RuntimeInstallProgress(event));
    });

    JsonRpcResponse::success(request.id, serde_json::json!({ "accepted": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_handle_list_returns_all_specs() {
        let dir = tempfile::TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(ledger_path)));
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "runtimes.list".into(),
            params: None,
            id: Some(serde_json::json!(1)),
        };
        let resp = handle_list(req, ledger).await;
        assert!(resp.result.is_some());
        let v = resp.result.unwrap();
        let runtimes = v.get("runtimes").unwrap().as_array().unwrap();
        assert!(runtimes.len() >= 5);
        let names: Vec<String> = runtimes
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"fnm".to_string()));
        assert!(names.contains(&"node".to_string()));
        assert!(names.contains(&"uv".to_string()));
        assert!(names.contains(&"playwright-cli".to_string()));
        assert!(names.contains(&"cargo".to_string()));
    }
}
```

Adapt the `JsonRpcRequest` test literal to the actual field shape (prior migration observed `id: Option<Value>`).

- [ ] **Step 3: Delete `browser_runtime.rs`**

```bash
rm src/gateway/handlers/browser_runtime.rs
```

- [ ] **Step 4: Update handlers/mod.rs**

Edit `src/gateway/handlers/mod.rs`:
- Delete `pub mod browser_runtime;`
- Add `pub mod runtimes;`
- In the RPC dispatcher: remove `"browser.runtime_status"`, `"browser.install_runtime"`, `"browser.refresh_runtime"` routes. Add `"runtimes.list"`, `"runtimes.install"`, `"runtimes.refresh"` routes calling the new handlers. The dispatcher signature already passes `Arc<RwLock<CapabilityLedger>>` if it exists; if not, create it at app startup and thread it to the dispatcher (check how other state like Config is wired).

- [ ] **Step 5: Compile check + tests**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: zero errors.

Run: `cargo test -p alephcore --lib gateway::handlers::runtimes 2>&1 | tail -15`
Expected: 1 test passes.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "gateway: add runtimes.* RPCs; delete browser_runtime; rename event"
```

---

## Task 10: Webchat API — `api/runtimes.rs` + remove browser runtime types

**Files:**
- Create: `interfaces/webchat/src/api/runtimes.rs`
- Modify: `interfaces/webchat/src/api/browser.rs` (remove runtime types)
- Modify: `interfaces/webchat/src/api/mod.rs` (add new module)

- [ ] **Step 1: Create `api/runtimes.rs`**

Create `interfaces/webchat/src/api/runtimes.rs`:

```rust
//! RPC client for runtimes.* gateway methods.

use serde::{Deserialize, Serialize};

use crate::context::DashboardState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Missing,
    Probing,
    Bootstrapping,
    Ready,
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RuntimeInfo {
    pub name: String,
    pub status: RuntimeStatus,
    pub bin_path: Option<String>,
    pub version: Option<String>,
    pub llm_hint: Option<String>,
    pub deps: Vec<String>,
    pub supported_on_current_os: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimesListResponse {
    pub runtimes: Vec<RuntimeInfo>,
}

pub struct RuntimesApi;

impl RuntimesApi {
    pub async fn list(state: &DashboardState) -> Result<RuntimesListResponse, String> {
        let v = state
            .rpc_call("runtimes.list", serde_json::Value::Null)
            .await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    pub async fn refresh(state: &DashboardState) -> Result<RuntimesListResponse, String> {
        let v = state
            .rpc_call("runtimes.refresh", serde_json::Value::Null)
            .await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    pub async fn install(state: &DashboardState, capability: &str) -> Result<(), String> {
        let _ = state
            .rpc_call(
                "runtimes.install",
                serde_json::json!({ "capability": capability }),
            )
            .await?;
        Ok(())
    }
}
```

- [ ] **Step 2: Register in `api/mod.rs`**

Edit `interfaces/webchat/src/api/mod.rs`. Add `pub mod runtimes;`.

- [ ] **Step 3: Remove old runtime types from `api/browser.rs`**

In `interfaces/webchat/src/api/browser.rs`, delete these blocks:
- `ComponentStatus` enum
- `RuntimeStatusResponse` struct
- `BrowserRuntimeApi` struct and its impl

- [ ] **Step 4: Compile check**

Run: `cargo check -p aleph-panel 2>&1 | tail -20`
Expected errors: any consumers of deleted types will surface. Fix in Task 11.

If `settings/browser.rs` or `settings/browser_runtime.rs` import the deleted types, leave them broken — Task 11 deletes those files.

Temporarily add `#[allow(dead_code)]` or stub where needed. Do not commit the broken state — run step 5 after Task 11 unblocks.

Actually — since Task 11 comes immediately next, just skip the compile check in this task and proceed to Task 11. The intermediate state is OK as long as we commit after Task 11 cleans it up.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/api/runtimes.rs interfaces/webchat/src/api/mod.rs interfaces/webchat/src/api/browser.rs
git commit -m "webchat: add api/runtimes.rs; remove BrowserRuntimeApi"
```

(This commit may leave aleph-panel temporarily broken; Task 11 restores it.)

---

## Task 11: Webchat — delete `settings/browser_runtime.rs` + create `views/runtimes.rs`

**Files:**
- Delete: `interfaces/webchat/src/views/settings/browser_runtime.rs`
- Create: `interfaces/webchat/src/views/runtimes.rs`
- Modify: `interfaces/webchat/src/views/mod.rs`
- Modify: `interfaces/webchat/src/views/settings/mod.rs`
- Modify: `interfaces/webchat/src/views/settings/browser.rs` (remove BrowserRuntimeCard mount + import)

- [ ] **Step 1: Delete old file**

```bash
rm interfaces/webchat/src/views/settings/browser_runtime.rs
```

- [ ] **Step 2: Create new Runtimes top-level view**

Create `interfaces/webchat/src/views/runtimes.rs`:

```rust
//! Runtimes dashboard view — read-only runtime status + one-click install.

use crate::api::runtimes::{RuntimeInfo, RuntimeStatus, RuntimesApi};
use crate::context::DashboardState;
use leptos::prelude::*;
use leptos::task::spawn_local;

#[component]
pub fn RuntimesView() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let runtimes = RwSignal::new(Vec::<RuntimeInfo>::new());
    let loading = RwSignal::new(true);
    let error_msg = RwSignal::new(Option::<String>::None);

    // Initial load on mount.
    {
        let state = state.clone();
        spawn_local(async move {
            match RuntimesApi::list(&state).await {
                Ok(r) => {
                    runtimes.set(r.runtimes);
                    error_msg.set(None);
                }
                Err(e) => error_msg.set(Some(e)),
            }
            loading.set(false);
        });
    }

    let refresh = {
        let state = state.clone();
        move |_| {
            loading.set(true);
            let state = state.clone();
            spawn_local(async move {
                match RuntimesApi::refresh(&state).await {
                    Ok(r) => {
                        runtimes.set(r.runtimes);
                        error_msg.set(None);
                    }
                    Err(e) => error_msg.set(Some(e)),
                }
                loading.set(false);
            });
        }
    };

    view! {
        <div class="p-6 space-y-4">
            <div class="flex items-center justify-between">
                <div>
                    <h1 class="text-2xl font-bold text-text-primary">"Runtimes"</h1>
                    <p class="text-sm text-text-tertiary mt-1">
                        "Language runtimes and CLI tools available to Aleph. Read-only; \
                         click \"Install\" on missing components."
                    </p>
                </div>
                <button
                    on:click=refresh
                    class="px-4 py-2 border border-border rounded-lg text-text-primary text-sm font-medium"
                >
                    "Refresh"
                </button>
            </div>

            {move || error_msg.get().map(|msg| view! {
                <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                    {msg}
                </div>
            })}

            {move || {
                if loading.get() {
                    view! { <div class="text-text-tertiary text-sm py-8">"Loading..."</div> }.into_any()
                } else {
                    view! {
                        <div class="space-y-3">
                            <For
                                each=move || runtimes.get()
                                key=|r| r.name.clone()
                                children=move |r| view! { <RuntimeCard info=r /> }
                            />
                        </div>
                    }.into_any()
                }
            }}
        </div>
    }
}

#[component]
fn RuntimeCard(info: RuntimeInfo) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let installing = RwSignal::new(false);
    let name = info.name.clone();

    let (icon, icon_class) = match info.status {
        RuntimeStatus::Ready => ("✓", "text-success"),
        RuntimeStatus::Missing if info.supported_on_current_os => ("✗", "text-text-tertiary"),
        RuntimeStatus::Missing => ("⊘", "text-text-tertiary"),
        RuntimeStatus::Probing => ("…", "text-text-tertiary"),
        RuntimeStatus::Bootstrapping => ("…", "text-info"),
        RuntimeStatus::Stale => ("?", "text-warning"),
    };

    let can_install =
        matches!(info.status, RuntimeStatus::Missing) && info.supported_on_current_os;

    let install_handler = {
        let state = state.clone();
        let name_clone = name.clone();
        move |_| {
            installing.set(true);
            let state = state.clone();
            let n = name_clone.clone();
            spawn_local(async move {
                let _ = RuntimesApi::install(&state, &n).await;
                // Optimistic — UI will update on next refresh click.
                installing.set(false);
            });
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-4">
            <div class="flex items-start justify-between gap-4">
                <div class="flex items-start gap-3 flex-1 min-w-0">
                    <span class=format!("w-5 text-center font-mono text-lg {icon_class}")>{icon}</span>
                    <div class="flex-1 min-w-0">
                        <div class="flex items-baseline gap-2">
                            <span class="font-medium text-text-primary">{info.name.clone()}</span>
                            {info.version.clone().map(|v| view! {
                                <span class="text-xs text-text-tertiary">{v}</span>
                            })}
                        </div>
                        {info.bin_path.clone().map(|p| view! {
                            <div class="text-xs text-text-tertiary font-mono truncate mt-1">{p}</div>
                        })}
                        {info.llm_hint.clone().map(|h| view! {
                            <div class="text-xs text-text-tertiary mt-1">{h}</div>
                        })}
                        {(!info.deps.is_empty()).then(|| view! {
                            <div class="text-xs text-text-tertiary mt-1">
                                "deps: " {info.deps.join(", ")}
                            </div>
                        })}
                    </div>
                </div>
                {can_install.then(|| view! {
                    <button
                        on:click=install_handler
                        disabled=move || installing.get()
                        class="px-3 py-1.5 bg-primary text-white rounded text-sm font-medium disabled:opacity-50"
                    >
                        {move || if installing.get() { "Installing..." } else { "Install" }}
                    </button>
                })}
                {(!info.supported_on_current_os).then(|| view! {
                    <span class="text-xs text-text-tertiary italic">"not supported yet"</span>
                })}
            </div>
        </div>
    }
}
```

- [ ] **Step 3: Register in `views/mod.rs`**

Edit `interfaces/webchat/src/views/mod.rs`. Add `pub mod runtimes;`. Find the sidebar/router enum (commonly `Route` or `Page`) and add a `Runtimes` variant. Match existing top-level entries' pattern (look at how `Cron`, `Logs`, `Tasks` are registered — likely an enum variant + label + route path).

- [ ] **Step 4: Remove from settings/mod.rs**

Edit `interfaces/webchat/src/views/settings/mod.rs`. Remove:

```rust
pub mod browser_runtime;
```

- [ ] **Step 5: Remove BrowserRuntimeCard mount from Browser settings**

Edit `interfaces/webchat/src/views/settings/browser.rs`:
- Delete the import `use super::browser_runtime::BrowserRuntimeCard;`
- Delete the `<BrowserRuntimeCard />` line inside `BrowserView`'s render tree

- [ ] **Step 6: Compile**

Run: `cargo check -p aleph-panel 2>&1 | tail -30`
Expected: zero errors.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/
git commit -m "webchat: add Runtimes dashboard view; delete settings/browser_runtime"
```

---

## Task 12: Docs + CHANGELOG

**Files:**
- Modify: `docs/reference/*` (any that reference runtimes/bootstrap)
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Find doc references**

Run: `grep -rln "browser/bootstrap\|BrowserInstallProgress\|BrowserRuntimeApi\|RuntimeStatusResponse" docs/ 2>&1 | head -10`

Expected matches: likely none or few (most doc references live in spec/plan files under `docs/superpowers/` which we don't touch).

For each hit outside `docs/superpowers/`, surgically update references to new names (`RuntimeInstallProgress`, `RuntimesApi`, etc.).

- [ ] **Step 2: Update CHANGELOG**

Edit `CHANGELOG.md`. Under the `[Unreleased]` section (same version as the playwright-cli migration, if still open), append:

```markdown
### Changed
- `src/runtimes/` unified with `src/browser/bootstrap.rs` into one cross-OS runtime manager driven by a single `SPECS` table. Probe/install/LLM-hint now share one data source.
- Panel "Runtime Status" card moved from Settings → Browser to a new top-level "Runtimes" dashboard view (read-only informational).
- Gateway RPCs renamed from `browser.runtime_status`/`install_runtime`/`refresh_runtime` to `runtimes.list`/`install`/`refresh`.
- Event `BrowserInstallProgressEvent` renamed to `RuntimeInstallProgressEvent`.
- Runtime install paths no longer use `~/.aleph/runtimes/` bespoke locations; defers to language-native tool defaults (`~/.local/share/fnm/`, `~/.local/bin/uv`, etc.).

### Added
- Windows install support via PowerShell (`winget install Schniz.fnm`, `irm astral.sh/uv/install.ps1 | iex`).
- `cargo`/`rustup` placeholder spec entry (empty install array; reserves the name for future work).
- `fnm alias <version> lts` post-install action, fixing the Q6 lts-alias bug from the prior playwright-cli migration.

### Removed
- `src/browser/bootstrap.rs` (merged into `src/runtimes/`).
- `src/gateway/handlers/browser_runtime.rs` (replaced by `handlers/runtimes.rs`).
- `interfaces/webchat/src/views/settings/browser_runtime.rs` (moved to `views/runtimes.rs`).
- Legacy `aleph_paths` probe fields in `src/runtimes/probe.rs`; old `~/.aleph/runtimes/python/default/` or `~/.aleph/runtimes/uv/uv` directories will no longer be detected.

### Migration Notes
- If you previously had Aleph install Python/uv to `~/.aleph/runtimes/`, those bundled installs are now orphans. Probe will not find them. To clean up: `rm -rf ~/.aleph/runtimes/python ~/.aleph/runtimes/uv`; then open Panel → Runtimes → Install to reinstall into standard user paths.
- Runtime status UI moved: Panel → Runtimes (sidebar). No longer under Settings → Browser.
```

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.md docs/
git commit -m "docs: update CHANGELOG and references for runtimes refactor"
```

---

## Task 13: Final verification pass

**Files:** none (verification only)

- [ ] **Step 1: Verify deleted files gone**

Run:
```
ls src/browser/bootstrap.rs src/gateway/handlers/browser_runtime.rs interfaces/webchat/src/views/settings/browser_runtime.rs 2>&1
```
Expected: all three report "No such file or directory".

- [ ] **Step 2: Verify new files present**

Run:
```
ls src/runtimes/os.rs src/runtimes/specs.rs src/runtimes/post_install.rs src/gateway/handlers/runtimes.rs interfaces/webchat/src/views/runtimes.rs interfaces/webchat/src/api/runtimes.rs
```
Expected: all 6 present.

- [ ] **Step 3: Verify no stale browser-bootstrap references**

Run: `grep -rn "browser::bootstrap\|BrowserInstallProgress\|BrowserRuntimeApi" src/ interfaces/ 2>&1 | head -10`
Expected: zero matches.

- [ ] **Step 4: Verify runtimes RPC registered**

Run: `grep -n "runtimes.list\|runtimes.install\|runtimes.refresh" src/gateway/handlers/mod.rs 2>&1`
Expected: 3 lines matching.

- [ ] **Step 5: Full workspace build + test**

Run: `cargo check --workspace 2>&1 | tail -10`
Expected: zero errors.

Run: `cargo test --workspace --lib 2>&1 | tail -20`
Expected: all pass.

Run: `cargo clippy -p alephcore --lib 2>&1 | grep "^error" | head -5`
Expected: zero error lines.

- [ ] **Step 6: LLM prompt injection smoke test (manual)**

If a live Aleph instance with `fnm` / `node` / `uv` installed is available:
- Start `target/debug/aleph-server`
- Open Panel → Chat
- Ask: "What runtimes do you have available?"
- Expected response: AI mentions fnm (Node version manager), node (via `fnm exec --using lts`), uv (Python package manager), playwright-cli — proving SPECS hints flow into the system prompt.

If no live instance: skip this step and note in verification report. Success criterion 6 stays pending until manually verified post-merge.

- [ ] **Step 7: Commit verification log (optional)**

If verification revealed fixups, fix them in a dedicated commit:
```bash
git commit -m "runtimes: fix <specific issue> found in verification"
```

---

## Self-Review

**Spec coverage:**

- [x] C Hybrid refactor: keep ensure_capability API, rewrite internals → Tasks 1-7
- [x] fnm / node / uv / playwright-cli / cargo placeholder → Task 2 SPECS table
- [x] Cross-OS (macOS/Linux/Windows) → Task 1 TargetOs + Task 2 OsInstall
- [x] Language-native tool strategy (B) → Task 2 specs all use Shell/Via/PowerShell, no system package managers
- [x] No more `~/.aleph/runtimes/` bespoke paths (D) → Task 4 drops `aleph_paths` from probe
- [x] Static LLM injection → Task 7 routes hints through SPECS
- [x] UI in Dashboard not Settings → Task 11 creates `views/runtimes.rs` as top-level
- [x] Keep `runtimes/` plural name → tasks use `src/runtimes/`
- [x] Drop legacy data (b3) → Task 4 removes `aleph_paths`; Task 12 CHANGELOG documents orphans
- [x] Delete browser/bootstrap + integrate → Tasks 8 (browser) + 11 (webchat)
- [x] Post-install for playwright-cli (B: capability + chromium/skills post-install) → Task 2 SPECS + Task 3 runner
- [x] RPC rename browser.* → runtimes.* → Task 9
- [x] Event rename BrowserInstallProgress → RuntimeInstallProgress → Task 9
- [x] `Via { parent: "node", ... }` auto-wraps with fnm exec --using lts → Task 5 `run_via_parent`
- [x] `supported_on_current_os` computation → Task 2 `supported_on_current_os()`
- [x] Success criteria 1-7 → Task 13 verifies 1-5; 6 (manual smoke) and 7 (CHANGELOG mention) are covered in Task 12

**Placeholder scan:** Every step contains concrete code, commands, and expected outputs. No "TBD" / "add error handling" / "similar to Task N" references.

**Type consistency:**
- `RuntimeSpec` (Task 2) is referenced in Tasks 4, 5, 7, 9 consistently.
- `BootstrapResult { Success, PathNotFound, Failed, Unsupported, UnknownCapability }` defined in Task 5; matched in Task 6's `ensure.rs` rewrite.
- `RuntimeStatus` / `RuntimeInfo` shape is Task 9 gateway → Task 10 webchat API — field names match (`name`, `status`, `bin_path`, `version`, `llm_hint`, `deps`, `supported_on_current_os`).
- `supported_on_current_os` function in Task 2 is called from Task 9's `build_list`.
- `find_spec` signature defined in Task 2 is used in Tasks 4, 5, 7, 9.
- `PostInstallAction` variants defined in Task 2; matched in Task 3 `post_install::run`.

**Tradeoffs noted during execution:**

1. Task 7's `capability.rs` test updates may drop some existing tests for `ffmpeg`/`yt-dlp` hints. These capabilities aren't in the new SPECS (spec scope is "runtime managers", not general tools). If the team needs them back as generic tool entries later, add new SPECS entries; the architecture supports it.
2. Task 8 constructs a fresh `CapabilityLedger` inside `PlaywrightCliDriver::resolve_binary`. If agent_loop already holds a shared ledger (read its startup code), wire the shared instance in instead — avoids duplicate file reads. If no shared instance exists today, this local construction is a bridge; a follow-up pass can hoist the ledger to `AppContext`.
3. Task 11's router integration (adding Runtimes to the sidebar enum/route) is codebase-specific. The exact enum name and helper functions depend on the current Leptos routing pattern in `views/mod.rs` — read it and match the existing convention.
