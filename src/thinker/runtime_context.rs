//! Runtime Context - Micro-environmental awareness for prompt injection
//!
//! Collects lightweight runtime environment metadata (OS, arch, shell, working directory,
//! current model, hostname) and formats it as a compact prompt section. This gives the
//! LLM grounding in the physical execution environment without heavy dependencies.
//!
//! # Usage
//!
//! ```rust,no_run
//! use alephcore::thinker::runtime_context::RuntimeContext;
//!
//! let ctx = RuntimeContext::collect("claude-opus-4-6");
//! let section = ctx.to_prompt_section();
//! // => "## Runtime Environment\nos=macos | arch=aarch64 | shell=zsh | cwd=/workspace | model=claude-opus-4-6 | host=MacBook-Pro"
//! ```

use crate::sync_primitives::Mutex;
use std::collections::HashMap;
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
    /// CPU architecture, e.g. "aarch64", "`x86_64`"
    pub arch: String,
    /// User's default shell, e.g. "zsh", "bash"
    pub shell: String,
    /// Current working directory
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
    /// Collect runtime context from the current environment.
    ///
    /// `current_model` is passed in because the caller (prompt builder) knows which
    /// model was selected by the router.
    ///
    /// `repo_root` is populated lazily via a process-lifetime `OnceLock` cache
    /// (see [`cached_repo_root`]). The detector walks up from the working
    /// directory looking for `.git`; no `git` subprocess is spawned. aleph-server
    /// is a long-running daemon whose working directory is stable, so caching at
    /// process scope is sufficient.
    #[must_use]
    pub fn collect(current_model: &str) -> Self {
        let os = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());

        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));

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

    /// Format as a compact prompt section for system prompt injection.
    ///
    /// Output example (with repo):
    /// ```text
    /// ## Runtime Environment
    /// os=macos | arch=aarch64 | shell=zsh | cwd=/workspace | repo=/workspace | git=main | model=claude-opus-4-6 | host=MacBook-Pro
    /// ```
    ///
    /// The `repo=` / `git=` segments are omitted when `repo_root` is `None` or
    /// the branch cannot be resolved. The `git=` branch is read fresh from
    /// `.git/HEAD` at render time (a single small file read — no `git`
    /// subprocess) so a mid-session `checkout` is reflected on the next turn,
    /// matching what codex's `<environment_context>` and hermes-agent's
    /// workspace block surface.
    #[must_use]
    pub fn to_prompt_section(&self) -> String {
        let mut parts = vec![
            format!("os={}", self.os),
            format!("arch={}", self.arch),
            format!("shell={}", self.shell),
            format!("cwd={}", self.working_dir.display()),
        ];

        if let Some(ref repo) = self.repo_root {
            parts.push(format!("repo={}", repo.display()));
            if let Some(branch) = detect_git_branch(repo) {
                parts.push(format!("git={branch}"));
            }
        }

        parts.push(format!("model={}", self.current_model));
        parts.push(format!("host={}", self.hostname));
        // Hour-precision time only — no `time_ms=`. A millisecond epoch here
        // changed every build, and any byte change in this section invalidates
        // the provider prompt cache for the entire conversation that follows
        // it. Tools that need exact time get it injected per-call
        // (`__current_time_ms`); the model needing exact time can run one.
        parts.push(format!("time={} ({})", self.current_time, self.timezone));

        format!("## Runtime Environment\n{}", parts.join(" | "))
    }
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
    fn test_to_prompt_section_format() {
        let ctx = make_test_ctx(Some(PathBuf::from("/workspace")));

        let section = ctx.to_prompt_section();

        assert!(section.starts_with("## Runtime Environment\n"));
        assert!(section.contains("os=macos"));
        assert!(section.contains("arch=aarch64"));
        assert!(section.contains("shell=zsh"));
        assert!(section.contains("cwd=/workspace"));
        assert!(section.contains("repo=/workspace"));
        assert!(section.contains("model=claude-opus-4-6"));
        assert!(section.contains("host=MacBook-Pro"));
        assert!(section.contains("time=2026-03-30 14:00 (Asia/Shanghai)"));
        // No sub-hour precision and no epoch ms — the section must stay
        // byte-stable within the hour so it never re-keys the provider
        // prompt cache for the conversation that follows it.
        assert!(!section.contains("time_ms="));

        // Verify pipe-separated format on the data line
        let lines: Vec<&str> = section.lines().collect();
        assert_eq!(lines.len(), 2, "should have header + data line");
        assert_eq!(lines[0], "## Runtime Environment");
        assert!(
            lines[1].contains(" | "),
            "data line should use pipe separators"
        );
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

    #[test]
    fn test_to_prompt_section_no_repo() {
        let ctx = RuntimeContext {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            shell: "bash".to_string(),
            working_dir: PathBuf::from("/home/user"),
            repo_root: None,
            current_model: "gpt-4".to_string(),
            hostname: "server-01".to_string(),
            current_time: "2026-03-30 02:00".to_string(),
            timezone: "UTC".to_string(),
        };

        let section = ctx.to_prompt_section();

        assert!(section.starts_with("## Runtime Environment\n"));
        assert!(section.contains("os=linux"));
        assert!(section.contains("arch=x86_64"));
        assert!(section.contains("shell=bash"));
        assert!(section.contains("cwd=/home/user"));
        assert!(
            !section.contains("repo="),
            "should NOT contain repo= when repo_root is None"
        );
        assert!(section.contains("model=gpt-4"));
        assert!(section.contains("host=server-01"));
        assert!(section.contains("time="));
        assert!(!section.contains("time_ms="));
        assert!(
            !section.contains("git="),
            "should NOT contain git= when repo_root is None"
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
        let try_result = cache.try_lock();
        handle.join().unwrap();
        assert!(
            try_result.is_ok(),
            "lock was held during I/O — concurrent callers would have blocked"
        );
    }
}
