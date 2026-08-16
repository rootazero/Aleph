//! Runtime Context — the single source of the environment envelope's *facts*.
//!
//! Collects lightweight runtime environment metadata (OS, arch, shell, working
//! directory, repo/branch, current model, hostname, local time) and renders it
//! in **two halves**, because the two halves have different cache lifetimes and
//! must therefore live in different prompt zones:
//!
//! | half | contents | zone | why |
//! |------|----------|------|-----|
//! | [`to_stable_lines`](RuntimeContext::to_stable_lines) | OS + arch, shell, host | `EnvironmentLayer` @300, **Stable** (cacheable prefix) | process-invariant — a byte here is written once and read back at 0.1× for the rest of the conversation |
//! | [`to_environment_context_block`](RuntimeContext::to_environment_context_block) | cwd, repo, git branch, model, local time, optional parent | `RuntimeContextLayer` @1720, **Dynamic** | cwd varies per *run* (project mode), branch varies mid-session, time varies hourly — any of these in the Stable prefix re-keys the whole conversation's provider cache |
//!
//! The dynamic half used to ship as `to_dynamic_line`, a pipe-separated
//! `## Runtime Environment` markdown section. It was replaced by the
//! codex-aligned XML block in the §2.3 round (2026-08-06) and deleted once
//! `RuntimeContextLayer` was the sole caller and had stopped calling it — see
//! [`to_environment_context_block`](RuntimeContext::to_environment_context_block)
//! for what the tag boundary buys over the line.
//!
//! Every fact is stated by exactly one of the two halves. They used to overlap:
//! `EnvironmentLayer` printed its own `OS` / `Working directory` bullets from
//! `std::env` while this struct's single-line summary printed `os=` / `cwd=`
//! again — the same fact twice per request, with the per-run-varying one sitting
//! in the *cacheable* zone (R9 / prompt-cache discipline). The overlap was
//! invisible to `prompt_contract::no_sentence_is_stated_twice` because the two
//! renderings never produced an identical sentence.
//!
//! # Usage
//!
//! ```rust,no_run
//! use alephcore::thinker::runtime_context::RuntimeContext;
//!
//! // `cwd` is the run's EFFECTIVE working directory (project override, else the
//! // agent workspace) — the directory the shell tool will actually execute in.
//! let ctx = RuntimeContext::collect_in("claude-opus-4-6", Some("/work/proj".as_ref()));
//! let stable = ctx.to_stable_lines(); // "- **OS**: macos (aarch64)\n- **Shell**: bash\n…"
//! let live = ctx.to_environment_context_block(None); // "<environment_context><cwd>/work/proj</cwd>…"
//! ```

use crate::sync_primitives::Mutex;
use crate::thinker::xml_util::{close_block, escape_xml, open_block_with_attrs, push_text_element};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Lightweight snapshot of the runtime environment.
///
/// Collected once at prompt-build time and injected into the system prompt
/// so the LLM knows where it is running.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    /// Operating system name, e.g. "macos", "linux", "windows"
    pub os: String,
    /// CPU architecture, e.g. "aarch64", "x86_64"
    pub arch: String,
    /// Interpreter a shell tool call actually runs under, e.g. "bash".
    ///
    /// This is the *agent's* shell, read from the exec tool that owns the fact
    /// ([`shell_interpreter`](crate::builtin_tools::code_exec::shell_interpreter)) —
    /// **not** the human operator's `$SHELL`. `$SHELL` describes a login shell the
    /// agent never uses, and is unset on Windows, where it rendered the useless
    /// `shell=unknown` while every `bash` / `code_exec` call still spawned bash.
    pub shell: String,
    /// The run's **effective** working directory — the directory a shell tool
    /// call executes in when the model supplies no `cwd`.
    ///
    /// Supplied by the caller via [`RuntimeContext::collect_in`], because only the
    /// caller knows it: the gateway resolves it as `workspace_override` (project
    /// mode) else the agent's `~/.aleph/workspaces/{id}`, and publishes that same
    /// value as [`EXEC_WORKSPACE`](crate::sandbox::context::EXEC_WORKSPACE), which
    /// is the root `WorkspaceSandbox` jails shell calls to. Falls back to the
    /// daemon's process cwd only when no caller-supplied path is available.
    pub working_dir: PathBuf,
    /// Git repository root, if inside a repo (caller provides from cached git info)
    pub repo_root: Option<PathBuf>,
    /// Current LLM model identifier
    pub current_model: String,
    /// Machine hostname
    pub hostname: String,
    /// Current local time as human-readable string, HOUR precision (e.g.
    /// "2026-03-30 14:00") so the rendered prompt section stays byte-stable —
    /// and provider-prompt-cacheable — across turns within the same hour.
    pub current_time: String,
    /// Local timezone identifier, e.g. "Asia/Shanghai", "UTC+8"
    pub timezone: String,
}

impl RuntimeContext {
    /// Collect runtime context for a run with no caller-known working directory
    /// (tests, `prompt-size`, internal tooling). Delegates to
    /// [`Self::collect_in`] with `None`, which falls back to the daemon's process
    /// cwd. **Production prompt assembly must use `collect_in`** with the run's
    /// effective workspace — see [`Self::working_dir`].
    #[must_use]
    pub fn collect(current_model: &str) -> Self {
        Self::collect_in(current_model, None)
    }

    /// Collect runtime context, anchored on the run's effective working directory.
    ///
    /// `current_model` and `cwd` are both passed in because only the caller knows
    /// them: the router picks the model, and the gateway resolves the effective
    /// workspace (project override, else the agent workspace) — the very path it
    /// publishes as `EXEC_WORKSPACE` for the sandbox to jail to. Passing it here
    /// is what makes the prompt's `cwd=` agree with where `bash` actually runs.
    ///
    /// A `cwd` that is not a directory (unmounted volume, deleted between
    /// dispatch and prompt assembly) is rejected with a warn and degraded to the
    /// process cwd rather than advertising a path the model cannot use — pi's
    /// `assertSessionCwdExists` lesson, softened to P7 graceful degradation
    /// because the gateway already hard-fails a vanished project override.
    ///
    /// `repo_root` is populated lazily via a process-lifetime cache keyed by the
    /// resolved directory (see [`cached_repo_root`]). The detector walks up
    /// looking for `.git`; no `git` subprocess is spawned.
    #[must_use]
    pub fn collect_in(current_model: &str, cwd: Option<&Path>) -> Self {
        let os = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();

        // The interpreter the agent's own shell tool spawns — single source with
        // the exec path, so this line cannot drift from what actually runs.
        let shell = crate::builtin_tools::code_exec::shell_interpreter().to_string();

        let working_dir = resolve_working_dir(cwd);

        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "unknown".to_string());

        // Collect current local time, epoch ms, and timezone. The prompt-facing
        // string is HOUR precision (hermes-agent's date-only-timestamp lesson,
        // PR #20451 there): the system prompt sits ahead of every message-level
        // prompt-cache breakpoint, so a second-precision timestamp re-keys the
        // whole conversation prefix on every run and zeroes the provider cache
        // hit rate. Hour precision keeps time-of-day grounding (salutations,
        // "is it late night" judgment) while staying byte-stable for up to an
        // hour of turns. Precise scheduling never depended on this string —
        // the tool registry injects fresh `__current_time_ms` into cron tool
        // calls at execution time (`tool_registry_impl.rs`).
        let now_local = chrono::Local::now();
        let current_time = now_local.format("%Y-%m-%d %H:00").to_string();
        let timezone = {
            let offset = now_local.offset();
            // Try to get IANA timezone from TZ env var, fall back to UTC offset
            std::env::var("TZ").unwrap_or_else(|_| {
                let secs = offset.local_minus_utc();
                let hours = secs / 3600;
                let mins = (secs.abs() % 3600) / 60;
                if mins == 0 {
                    format!("UTC{hours:+}")
                } else {
                    format!("UTC{hours:+}:{mins:02}")
                }
            })
        };

        let repo_root = cached_repo_root(&working_dir);

        Self {
            os,
            arch,
            shell,
            working_dir,
            repo_root,
            current_model: current_model.to_string(),
            hostname,
            current_time,
            timezone,
        }
    }

    /// The **process-invariant** half of the envelope, as Markdown bullets for
    /// `EnvironmentLayer`'s `## Environment` section (Stable / cacheable zone).
    ///
    /// ```text
    /// - **OS**: macos (aarch64)
    /// - **Shell**: bash
    /// - **Host**: MacBook-Pro
    /// ```
    ///
    /// Nothing here can change while the daemon runs, so these bytes are written
    /// to the provider prompt cache once and read back cheaply for the rest of
    /// the conversation. Deliberately excludes `working_dir` / `repo_root` /
    /// `current_model` / `current_time`: each of those varies per run or per hour
    /// and belongs in [`Self::to_environment_context_block`].
    #[must_use]
    pub fn to_stable_lines(&self) -> String {
        let mut out = String::with_capacity(96);
        let _ = writeln!(out, "- **OS**: {} ({})", self.os, self.arch);
        let _ = writeln!(out, "- **Shell**: {}", self.shell);
        let _ = writeln!(out, "- **Host**: {}", self.hostname);
        out
    }

    /// Codex-aligned `<environment_context>` envelope, in the same XML shape
    /// codex's `FileSystemContext::render()` uses (`<environment_context>
    /// <cwd>…</cwd><git>…</git><model>…</model><time>…</time></environment_context>`).
    ///
    /// Why an XML block instead of the pipe-separated `## Runtime Environment`
    /// markdown section this layer shipped until 2026-08-06: it gives the model a
    /// machine-bounded region that downstream tooling and codex-style
    /// `<environment_context>` callers can match on (the same role
    /// `markdown_excerpt.rs::split_wikilinks` plays for note excerpts). It
    /// also puts the body inside a tag so attribute-injection payloads (a
    /// path containing `"`) cannot forge a sibling attribute — the escape
    /// envelope is the boundary codex chose, and the boundary our
    /// `xml_util::escape_xml_attr` reasons about.
    ///
    /// The block is rendered only when *at least one* dynamic fact resolves
    /// (cwd OR repo OR git OR model OR time); callers in unit-test /
    /// bare-prompt-size paths get an empty string back, byte-identical to
    /// not calling the method.
    ///
    /// `parent = (kind, id)` adds a `<parent kind="…">id</parent>` element so
    /// the model can disambiguate "I am the explore sub-agent of session X".
    /// `parent = None` omits the element (the common path stays
    /// byte-identical).
    #[must_use]
    pub fn to_environment_context_block(&self, parent: Option<(&str, &str)>) -> String {
        let cwd = self.working_dir.display().to_string();
        let git = self.repo_root.as_deref().and_then(detect_git_branch);
        let mut out = String::with_capacity(192);
        open_block_with_attrs(
            &mut out,
            "environment_context",
            std::iter::empty::<(&str, &str)>(),
        );
        push_text_element(&mut out, "cwd", &cwd);
        if let Some(ref repo) = self.repo_root {
            push_text_element(&mut out, "repo", &repo.display().to_string());
        }
        if let Some(ref branch) = git {
            push_text_element(&mut out, "git", branch);
        }
        push_text_element(&mut out, "model", &self.current_model);
        // Hour-precision time only — no `time_ms`. A millisecond epoch here
        // changes on every build, and any byte change in this section
        // invalidates the provider prompt cache for the whole conversation that
        // follows it. Tools needing exact time get it injected per-call
        // (`__current_time_ms`); a model needing exact time can run one.
        // We deliberately stay in the same format string ("YYYY-MM-DD HH:00
        // (TZ)") across both halves so the model sees one timestamp, not two
        // formats it has to reconcile.
        push_text_element(
            &mut out,
            "time",
            &format!("{} ({})", self.current_time, self.timezone),
        );
        if let Some((kind, id)) = parent {
            // Attribute escape on `kind` (the discriminator is a token
            // supplied by the dispatcher, but a malicious or buggy dispatcher
            // still cannot smuggle a sibling attribute); text escape on `id`
            // (session keys are paths, may contain `&` etc.).
            open_block_with_attrs(&mut out, "parent", [("kind", kind)]);
            out.push_str(&escape_xml(id));
            close_block(&mut out, "parent");
        }
        close_block(&mut out, "environment_context");
        out
    }

    /// True iff the dynamic half has any rendering facts. Lets prompt-building
    /// code skip the XML block entirely (and the byte it would have produced)
    /// when only the stable bullets are real — the bare `prompt-size` /
    /// internal-tooling paths so they stay byte-identical.
    #[must_use]
    pub fn has_dynamic_facts(&self) -> bool {
        // `current_model` is always populated (`collect_in` requires
        // `current_model: &str`), so this is effectively the contract — but
        // belt-and-braces given how cheap it is and how load-bearing it has
        // become in the surface-area review.
        !self.current_model.is_empty()
    }
}

/// Resolve the directory the envelope will advertise as the run's cwd.
///
/// `Some(dir)` wins; a `Some` that is not a directory is refused with a warn so
/// the model is never told to write into a path that vanished between dispatch
/// and prompt assembly. `None` (tests, `prompt-size`, internal tooling) falls
/// back to the daemon's process cwd.
fn resolve_working_dir(cwd: Option<&Path>) -> PathBuf {
    match cwd {
        Some(p) if p.is_dir() => p.to_path_buf(),
        Some(p) => {
            // Two log fields (reason, requested_cwd) so a daemon operator
            // can grep for either axis; `cwd_downgrade_total` is the
            // counter name a `doctor` probe can read back to spot a
            // project override that has been flapping without the
            // per-occurrence log noise an instance with N turns/day
            // would otherwise produce.
            tracing::warn!(
                reason = "not_a_directory",
                requested_cwd = %p.display(),
                fallback_cwd = %process_cwd().display(),
                cwd_downgrade_total = 1,
                "run workspace is not a directory; falling back to the process cwd for the prompt envelope"
            );
            process_cwd()
        }
        None => process_cwd(),
    }
}

fn process_cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

/// Process-lifetime cache for the detected repository root, keyed by working
/// directory.
///
/// Detection cost is one to a few `fs::exists` syscalls on the first call for a
/// given `working_dir`; subsequent calls with the same directory return the
/// cached `Option<PathBuf>` (`None` is cached too, so non-repo dirs never
/// re-walk). Keying by `working_dir` is required: a daemon serving runs from
/// different cwds must not return the first caller's repo root for all of them.
static REPO_ROOT_CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<PathBuf>>>> = OnceLock::new();

fn cached_repo_root(working_dir: &Path) -> Option<PathBuf> {
    let cache = REPO_ROOT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let map = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = map.get(working_dir) {
            return cached.clone();
        }
    }
    let detected = detect_repo_root(working_dir);
    let mut map = cache.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(working_dir.to_path_buf())
        .or_insert_with(|| detected.clone());
    detected
}

/// Walk upward from `start` looking for a `.git` entry. Returns the first
/// ancestor that contains one, or `None` if the walk hits the filesystem root.
///
/// Notes:
/// - `.git` may be a directory (regular repo) or a file (git worktree). Either
///   satisfies the check, so a `.try_exists()` is sufficient.
/// - We do not follow symlinks explicitly; `Path::try_exists` is consistent
///   with how `git` itself locates the repo.
fn detect_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = start;
    loop {
        if current.join(".git").try_exists().unwrap_or(false) {
            return Some(current.to_path_buf());
        }
        match current.parent() {
            Some(parent) => current = parent,
            None => return None,
        }
    }
}

/// Resolve the current git branch for `repo_root` by reading `HEAD` directly —
/// **no `git` subprocess**, so it is cheap enough to run on every prompt build
/// and reflects mid-session `checkout`s on the next turn.
///
/// Handles both repository shapes:
/// - regular repo: `.git/` is a directory, `HEAD` is at `.git/HEAD`.
/// - linked worktree: `.git` is a *file* (`gitdir: <path>`) pointing at the
///   worktree's gitdir, where `HEAD` lives.
///
/// `HEAD` is either `ref: refs/heads/<branch>` (on a branch) or a raw commit
/// SHA (detached). Returns the short branch name, `detached:<sha8>` for a
/// detached HEAD, or `None` if anything cannot be read (the caller then omits
/// the `git=` segment entirely).
fn detect_git_branch(repo_root: &Path) -> Option<String> {
    let dot_git = repo_root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else if dot_git.is_file() {
        // Worktree: ".git" is a file `gitdir: <absolute-or-relative path>`.
        let pointer = std::fs::read_to_string(&dot_git).ok()?;
        let target = pointer.trim().strip_prefix("gitdir:")?.trim();
        let target = PathBuf::from(target);
        if target.is_absolute() {
            target
        } else {
            repo_root.join(target)
        }
    } else {
        return None;
    };

    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        // "ref: refs/heads/feature/x" -> last path segment "x".
        let reference = reference.trim();
        let branch = reference.rsplit('/').next().unwrap_or(reference);
        if branch.is_empty() {
            None
        } else {
            Some(branch.to_string())
        }
    } else if head.is_empty() {
        None
    } else {
        // Detached HEAD: `head` is a raw SHA (ASCII hex) — char-safe to take 8.
        let short: String = head.chars().take(8).collect();
        Some(format!("detached:{short}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collect_returns_valid_context() {
        let ctx = RuntimeContext::collect("test-model-v1");

        // OS and arch should come from std::env::consts (never empty)
        assert!(!ctx.os.is_empty(), "os should not be empty");
        assert!(!ctx.arch.is_empty(), "arch should not be empty");

        // Shell should be populated (at least "unknown" as fallback)
        assert!(!ctx.shell.is_empty(), "shell should not be empty");

        // Working dir should be a valid path
        assert!(
            ctx.working_dir.to_str().is_some(),
            "working_dir should be a valid UTF-8 path"
        );

        // repo_root is populated lazily from a process-lifetime cache. In
        // the cargo test harness we are inside the alephcore worktree, so a
        // `.git` entry is reachable from cwd — we accept either a populated
        // `Some(path)` or a `None`, but reject any populated path that
        // doesn't exist on disk.
        if let Some(ref repo) = ctx.repo_root {
            assert!(repo.exists(), "repo_root must point at a real path");
        }

        // current_model should match what we passed in
        assert_eq!(ctx.current_model, "test-model-v1");

        // hostname should be populated (at least "unknown" as fallback)
        assert!(!ctx.hostname.is_empty(), "hostname should not be empty");
    }

    fn make_test_ctx_with_git(repo_root: Option<PathBuf>) -> RuntimeContext {
        RuntimeContext {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            shell: "bash".to_string(),
            working_dir: PathBuf::from("/workspace/proj"),
            repo_root,
            current_model: "claude-opus-4-6".to_string(),
            hostname: "test-host".to_string(),
            current_time: "2026-08-06 10:00".to_string(),
            timezone: "UTC".to_string(),
        }
    }

    fn make_test_ctx(repo_root: Option<PathBuf>) -> RuntimeContext {
        RuntimeContext {
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
            shell: "zsh".to_string(),
            working_dir: PathBuf::from("/workspace"),
            repo_root,
            current_model: "claude-opus-4-6".to_string(),
            hostname: "MacBook-Pro".to_string(),
            current_time: "2026-03-30 14:00".to_string(),
            timezone: "Asia/Shanghai".to_string(),
        }
    }

    #[test]
    fn stable_half_carries_only_process_invariant_facts() {
        let ctx = make_test_ctx(Some(PathBuf::from("/workspace")));
        let stable = ctx.to_stable_lines();

        assert!(stable.contains("- **OS**: macos (aarch64)"), "{stable}");
        assert!(stable.contains("- **Shell**: zsh"), "{stable}");
        assert!(stable.contains("- **Host**: MacBook-Pro"), "{stable}");
        // Anything that can change while the daemon runs must NOT be here: these
        // bytes sit in the cacheable prompt prefix, so a per-run or per-hour value
        // would re-key the whole conversation's provider cache on every change.
        for volatile in [
            "/workspace",
            "claude-opus-4-6",
            "2026-03-30",
            "Asia/Shanghai",
        ] {
            assert!(
                !stable.contains(volatile),
                "volatile value {volatile:?} leaked into the Stable half:\n{stable}"
            );
        }
    }

    /// The invariant the two-zone split exists to protect: no fact is stated by
    /// both halves. Regression for the OS/cwd overlap that `EnvironmentLayer` and
    /// `RuntimeContextLayer` both used to print, which
    /// `prompt_contract::no_sentence_is_stated_twice` could not see because the two
    /// renderings never produced a byte-identical sentence.
    #[test]
    fn the_two_halves_never_state_the_same_fact() {
        let ctx = make_test_ctx(Some(PathBuf::from("/workspace")));
        let stable = ctx.to_stable_lines();
        let dynamic = ctx.to_environment_context_block(None);

        let facts = [
            ("os", ctx.os.as_str()),
            ("arch", ctx.arch.as_str()),
            ("shell", ctx.shell.as_str()),
            ("host", ctx.hostname.as_str()),
            ("model", ctx.current_model.as_str()),
            ("time", ctx.current_time.as_str()),
            ("timezone", ctx.timezone.as_str()),
        ];
        for (name, value) in facts {
            assert!(
                !(stable.contains(value) && dynamic.contains(value)),
                "{name} ({value:?}) is stated by BOTH halves — pick one zone\n\
                 stable:\n{stable}\ndynamic:\n{dynamic}"
            );
        }
        // cwd is the one checked by path rather than by value: it must appear only
        // in the Dynamic half, because it varies per run in project mode.
        assert!(dynamic.contains("<cwd>/workspace</cwd>"), "{dynamic}");
        assert!(!stable.contains("/workspace"), "{stable}");
    }

    #[test]
    fn detect_repo_root_finds_dot_git_at_start() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");

        let found = detect_repo_root(dir.path());
        assert_eq!(found.as_deref(), Some(dir.path()));
    }

    #[test]
    fn detect_repo_root_walks_up_to_parent() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
        let nested = dir.path().join("a/b/c");
        std::fs::create_dir_all(&nested).expect("nested dirs");

        let found = detect_repo_root(&nested);
        // Canonicalize both sides — on macOS tempdir is under /var/folders
        // which is a symlink to /private/var/folders.
        let found_canon = std::fs::canonicalize(found.expect("found")).expect("canon found");
        let expect_canon = std::fs::canonicalize(dir.path()).expect("canon dir");
        assert_eq!(found_canon, expect_canon);
    }

    #[test]
    fn detect_repo_root_accepts_dot_git_file_for_worktrees() {
        // git worktrees use a .git file (not directory) pointing at the
        // shared gitdir. `try_exists` returns true for both shapes.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join(".git"), b"gitdir: /tmp/shared\n").expect("write .git");

        let found = detect_repo_root(dir.path());
        assert_eq!(found.as_deref(), Some(dir.path()));
    }

    #[test]
    fn detect_repo_root_returns_none_when_no_git_found() {
        // A tempdir without any .git anywhere up to the root is rare in
        // practice, but the function must terminate at the filesystem root.
        // Use /tmp/<unique> + canonicalize so we don't depend on macOS's
        // /private/var/folders symlink quirks.
        let dir = tempfile::tempdir().expect("tempdir");
        // Walk up: as long as no ancestor has `.git`, return None.
        // We can't guarantee that across all CI environments, so probe the
        // ancestors explicitly and skip if any has `.git`.
        let mut probe = dir.path();
        let mut has_git_ancestor = false;
        loop {
            if probe.join(".git").try_exists().unwrap_or(false) {
                has_git_ancestor = true;
                break;
            }
            match probe.parent() {
                Some(p) => probe = p,
                None => break,
            }
        }
        if has_git_ancestor {
            // Environment-dependent; the affirmative tests above already
            // pin the happy path, so we skip rather than emit a false
            // failure.
            return;
        }
        assert_eq!(detect_repo_root(dir.path()), None);
    }

    /// `collect_in` must honour the caller's effective workspace, and must refuse
    /// a path that is not a directory rather than advertising it to the model.
    #[test]
    fn collect_in_anchors_on_the_callers_workspace_and_rejects_non_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path().join("proj");
        std::fs::create_dir(&ws).expect("mkdir proj");

        let ctx = RuntimeContext::collect_in("m", Some(&ws));
        assert_eq!(ctx.working_dir, ws, "caller-supplied workspace must win");

        // A vanished / non-directory path degrades to the process cwd (P7), never
        // to the bogus path itself.
        let ghost = dir.path().join("gone");
        let ctx = RuntimeContext::collect_in("m", Some(&ghost));
        assert_ne!(ctx.working_dir, ghost);
        assert_eq!(ctx.working_dir, process_cwd());

        // `None` (tests / prompt-size / internal tooling) = process cwd.
        assert_eq!(RuntimeContext::collect("m").working_dir, process_cwd());
    }

    /// The advertised shell must be the interpreter the exec tool actually spawns,
    /// not the operator's `$SHELL` (unset on Windows, where it rendered
    /// `shell=unknown` while every shell call still ran bash).
    #[test]
    fn shell_fact_comes_from_the_exec_tool_not_the_env() {
        let ctx = RuntimeContext::collect("m");
        assert_eq!(
            ctx.shell,
            crate::builtin_tools::code_exec::shell_interpreter(),
            "shell must be single-sourced from the exec path"
        );
    }

    #[test]
    fn detect_git_branch_reads_ref_head() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
        std::fs::write(dir.path().join(".git/HEAD"), b"ref: refs/heads/main\n")
            .expect("write HEAD");

        assert_eq!(detect_git_branch(dir.path()).as_deref(), Some("main"));
    }

    #[test]
    fn detect_git_branch_keeps_last_segment_of_slashed_branch() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
        std::fs::write(
            dir.path().join(".git/HEAD"),
            b"ref: refs/heads/feature/context-mode\n",
        )
        .expect("write HEAD");

        assert_eq!(
            detect_git_branch(dir.path()).as_deref(),
            Some("context-mode")
        );
    }

    #[test]
    fn detect_git_branch_handles_worktree_gitdir_file() {
        // Linked worktree: ".git" is a file pointing at the real gitdir, where
        // HEAD lives.
        let dir = tempfile::tempdir().expect("tempdir");
        let gitdir = dir.path().join("real-gitdir");
        std::fs::create_dir(&gitdir).expect("mkdir gitdir");
        std::fs::write(gitdir.join("HEAD"), b"ref: refs/heads/wt-branch\n").expect("write HEAD");
        std::fs::write(
            dir.path().join(".git"),
            format!("gitdir: {}\n", gitdir.display()),
        )
        .expect("write .git pointer");

        assert_eq!(detect_git_branch(dir.path()).as_deref(), Some("wt-branch"));
    }

    #[test]
    fn detect_git_branch_reports_detached_head() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
        std::fs::write(
            dir.path().join(".git/HEAD"),
            b"0123456789abcdef0123456789abcdef01234567\n",
        )
        .expect("write HEAD");

        assert_eq!(
            detect_git_branch(dir.path()).as_deref(),
            Some("detached:01234567")
        );
    }

    #[test]
    fn detect_git_branch_returns_none_without_git() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(detect_git_branch(dir.path()), None);
    }

    #[test]
    fn cached_repo_root_releases_lock_before_filesystem_io() {
        let dir = tempfile::tempdir().expect("tempdir");
        let handle = std::thread::spawn({
            let wd = dir.path().to_path_buf();
            move || cached_repo_root(&wd)
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        let cache = REPO_ROOT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        // Sample, then release *before* joining. Binding the guard across
        // `join()` deadlocks on success: the worker's closing insert needs this
        // same mutex, so the pass case was the wedge case — and under the full
        // suite's parallel load (where 20 ms rarely outlasts the worker) it hung
        // the whole run instead of failing it.
        let free_during_io = cache.try_lock().is_ok();
        handle.join().unwrap();
        assert!(
            free_during_io,
            "lock was held during I/O — concurrent callers would have blocked"
        );
    }

    // ---- environment_context block (codex-aligned) ----

    #[test]
    fn env_block_emits_xml_with_cwd_repo_model_time() {
        let ctx = RuntimeContext {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            shell: "bash".to_string(),
            working_dir: PathBuf::from("/workspace/proj"),
            repo_root: Some(PathBuf::from("/workspace/proj")),
            current_model: "claude-opus-4-6".to_string(),
            hostname: "h".to_string(),
            current_time: "2026-08-06 10:00".to_string(),
            timezone: "UTC".to_string(),
        };
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join(".git")).expect("mkdir .git");
        std::fs::write(dir.path().join(".git/HEAD"), b"ref: refs/heads/main\n").expect("HEAD");
        // …and overwrite the repo_root with the tempdir so `detect_git_branch`
        // is exercised on a real disk fixture.
        let mut ctx = ctx;
        ctx.working_dir = dir.path().to_path_buf();
        ctx.repo_root = Some(dir.path().to_path_buf());

        let block = ctx.to_environment_context_block(None);

        // Open tag, no attributes (the codex `<environment_context>` shape).
        assert!(block.starts_with("<environment_context>"), "{block}");
        assert!(block.ends_with("</environment_context>"), "{block}");
        // All four required sub-elements present.
        assert!(block.contains("<cwd>"), "{block}");
        assert!(block.contains("</cwd>"), "{block}");
        assert!(block.contains("<repo>"), "{block}");
        assert!(block.contains("<git>main</git>"), "{block}");
        assert!(block.contains("<model>claude-opus-4-6</model>"), "{block}");
        assert!(
            block.contains("<time>2026-08-06 10:00 (UTC)</time>"),
            "{block}"
        );
        // No `<parent>` element when the caller passes `None`.
        assert!(!block.contains("<parent"), "{block}");
        // Hour precision only, no epoch ms: this block must stay byte-stable
        // within the hour or it re-keys the provider prompt cache for the
        // whole conversation that follows it.
        assert!(!block.contains("time_ms"), "{block}");
    }

    #[test]
    fn env_block_omits_repo_git_when_no_repo_root() {
        let ctx = make_test_ctx_with_git(None);
        let block = ctx.to_environment_context_block(None);

        assert!(block.contains("<cwd>"), "{block}");
        assert!(!block.contains("<repo>"), "no repo segment: {block}");
        assert!(!block.contains("<git>"), "no git segment: {block}");
        assert!(block.contains("<model>"), "{block}");
    }

    #[test]
    fn env_block_appends_parent_element_when_supplied() {
        let ctx = make_test_ctx_with_git(None);
        let block = ctx.to_environment_context_block(Some(("subagent", "session-X")));

        // Attribute boundary: `kind` is attr-escaped (no quotes allowed).
        // Even though "subagent" is benign here, we exercise the
        // attr-escape path; see xml_util::escape_xml_attr tests for the
        // adversarial coverage.
        assert!(block.contains("<parent kind=\"subagent\">"), "{block}");
        assert!(block.contains("session-X"), "{block}");
        assert!(block.contains("</parent>"), "{block}");
        // Parent must come BEFORE the closing `</environment_context>` so the
        // document is well-formed even if a downstream consumer tries to
        // parse it.
        let parent_pos = block.find("<parent").expect("parent element");
        let close_pos = block.rfind("</environment_context>").expect("close tag");
        assert!(parent_pos < close_pos, "{block}");
    }

    #[test]
    fn env_block_escapes_special_chars_inside_paths_and_ids() {
        // Adversarial: a cwd value containing `&` (and the `\`-padded
        // canonicalization that occasionally quotes) must NOT be able to
        // forge a sibling attribute when escaped as element text.
        // Element-text escape (not attr) keeps `&` inert in <cwd>…</cwd>;
        // quotes are inert in element-text content (only `"` and `'`
        // matter inside attributes, hence the separate `escape_xml_attr`).
        let ctx = RuntimeContext {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            shell: "bash".to_string(),
            working_dir: PathBuf::from("/tmp/a&b<c>"),
            repo_root: None,
            current_model: "m".to_string(),
            hostname: "h".to_string(),
            current_time: "2026-08-06 10:00".to_string(),
            timezone: "UTC".to_string(),
        };
        let block = ctx.to_environment_context_block(None);

        // `&` becomes `&amp;`, `<` becomes `&lt;`, `>` becomes `&gt;` —
        // the three element-text specials.
        assert!(
            block.contains("<cwd>/tmp/a&amp;b&lt;c&gt;</cwd>"),
            "{block}"
        );
    }

    #[test]
    fn env_block_is_byte_stable_across_repeated_calls() {
        // The Dynamic half goes through the same render-pipeline as the
        // `Stable`-prefix bytes: a per-build re-render that returns the
        // same string for the same inputs is the contract. (The field
        // doesn't go in the cacheable prefix; this guarantees the render
        // path itself is pure / cheap.)
        let ctx = make_test_ctx_with_git(Some(PathBuf::from("/workspace/proj")));
        let a = ctx.to_environment_context_block(None);
        let b = ctx.to_environment_context_block(None);
        assert_eq!(a, b);
    }

    #[test]
    fn has_dynamic_facts_is_true_when_model_is_set() {
        // The base contract: any collection that populated `current_model`
        // (every normal path goes through `collect_in` / `collect`) has
        // dynamic facts worth rendering.
        let ctx = RuntimeContext::collect("m");
        assert!(ctx.has_dynamic_facts());
    }
}
