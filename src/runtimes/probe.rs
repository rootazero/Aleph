//! Probe module — detects installed runtimes by checking PATH.
//!
//! Reads spec data from `super::specs::SPECS`. Does NOT install anything —
//! only reports whether a binary is present and its version.

use crate::sync_primitives::Mutex;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, trace, warn};

use crate::runtimes::ledger::CapabilitySource;
use crate::runtimes::specs::{find_spec, RuntimeSpec};
use crate::utils::no_window::NoWindow;

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
    const fn not_found() -> Self {
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
    let bin_name = spec.binaries.iter().next()?;
    let search_path = enriched_search_path();
    let bin_path = find_on_path(bin_name, &search_path)?;
    let version = get_version(
        &bin_path,
        spec.version_flag,
        spec.version_regex,
        &search_path,
    );
    let version_warning = check_version_warning(spec, version.as_deref());
    Some(ProbeResult {
        found: true,
        bin_path: Some(bin_path),
        version,
        source: CapabilitySource::System,
        version_warning,
    })
}

/// Build the enriched search PATH: the inherited PATH plus well-known runtime
/// install directories (see [`install_dir_candidates`]).
///
/// A GUI-launched daemon (macOS `.app` / launchd) inherits a minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`) that excludes Homebrew (`/opt/homebrew/bin`),
/// cargo (`~/.cargo/bin`), and the fnm-managed node dir — so probing the raw
/// inherited PATH reports installed runtimes as missing. We therefore search the
/// inherited PATH *plus* those known install dirs, without mutating the global
/// process env (thread-safe; each probe gets a private enriched PATH).
///
/// Shared by both binary lookup ([`find_on_path`]) and version detection
/// ([`get_version`]): a node-shebang tool (`playwright-cli`, `#!/usr/bin/env
/// node`) resolves its interpreter via PATH, so its `--version` probe must run
/// with the same enriched PATH or `node` is "not found" and the version comes
/// back empty.
fn enriched_search_path() -> OsString {
    extend_path(
        &std::env::var_os("PATH").unwrap_or_default(),
        &install_dir_candidates(),
    )
}

/// Locate a binary on the (pre-enriched) `search_path`.
///
/// First tries the platform-native locator (`which` on Unix, `where` on Windows).
/// Falls back to a manual PATH walk for minimal environments (e.g. containers
/// without the `which` binary).
fn find_on_path(bin_name: &str, search_path: &OsStr) -> Option<PathBuf> {
    // 1. Try the native locator first (more reliable, handles aliases, etc.)
    let locator = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    trace!("looking for '{}' via {}", bin_name, locator);
    let mut cmd = Command::new(locator);
    cmd.arg(bin_name).env("PATH", search_path);
    if let Ok(output) = cmd.no_window().output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let path_str = stdout.lines().next().unwrap_or("").trim();
            if !path_str.is_empty() {
                return Some(PathBuf::from(path_str));
            }
        }
    }

    // 2. Fallback: manual traversal of the enriched PATH.
    trace!("falling back to manual PATH search for '{}'", bin_name);
    find_in_dirs(bin_name, search_path)
}

/// Manually walk `search_path` (a joined PATH string) looking for `bin_name`.
fn find_in_dirs(bin_name: &str, search_path: &OsStr) -> Option<PathBuf> {
    for dir in std::env::split_paths(search_path) {
        let candidates: Vec<PathBuf> = if cfg!(target_os = "windows") {
            vec![
                dir.join(format!("{bin_name}.exe")),
                dir.join(format!("{bin_name}.bat")),
                dir.join(format!("{bin_name}.cmd")),
                dir.join(bin_name),
            ]
        } else {
            vec![dir.join(bin_name)]
        };
        for candidate in candidates {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Prepend known runtime install dirs (that exist and aren't already present)
/// to `base`, returning a PATH string for `Command::env("PATH", ..)`. Pure and
/// side-effect free — does NOT touch the global process env.
fn extend_path(base: &OsStr, candidates: &[PathBuf]) -> OsString {
    let base_paths: Vec<PathBuf> = std::env::split_paths(base).collect();
    let mut seen: HashSet<&Path> = base_paths.iter().map(|p| p.as_path()).collect();
    let mut prepended: Vec<PathBuf> = Vec::new();
    for cand in candidates {
        if cand.is_dir() && seen.insert(cand.as_path()) {
            // rust-doctor-disable-next-line excessive-clone
            prepended.push(cand.clone());
        }
    }
    if prepended.is_empty() {
        return base.to_os_string();
    }
    drop(seen);
    prepended.extend(base_paths);
    std::env::join_paths(&prepended).unwrap_or_else(|_| base.to_os_string())
}

/// Directories where runtimes commonly install but a GUI-launched daemon's
/// minimal PATH won't include: Homebrew (incl. the keg-only `rustup` formula),
/// MacPorts, cargo/rustup (`$CARGO_HOME`), fnm + fnm-managed node, asdf shims,
/// Nix profiles, Xcode CLT, winget shims. Covers `uv` (`~/.local/bin`), `cargo`
/// (`~/.cargo/bin`), `fnm` (its data-dir root), and `node` / `playwright-cli`
/// (the fnm-managed `<root>/aliases/<alias>/bin`).
fn install_dir_candidates() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    // Where *we* put global npm CLIs. Read from the same helper the installer
    // uses rather than restating its answer: the platform defaults below happen
    // to coincide with it, but an operator-set `npm_config_prefix` does not, and
    // then the install succeeds somewhere this probe never looks.
    if let Some(dir) = super::npm_global::bin_dir() {
        dirs.push(dir);
    }
    // Explicit `CARGO_HOME` override: rustup honors it and drops binaries in
    // `$CARGO_HOME/bin` instead of `~/.cargo/bin`. Honor it before the default.
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        dirs.push(PathBuf::from(cargo_home).join("bin"));
    }
    // asdf version manager routes every managed tool (node, python, rust, …)
    // through a shim in `<data-dir>/shims`. `$ASDF_DATA_DIR` overrides the
    // `~/.asdf` default (handled in the HOME block below).
    if let Some(asdf_data) = std::env::var_os("ASDF_DATA_DIR") {
        dirs.push(PathBuf::from(asdf_data).join("shims"));
    }
    if let Some(h) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(h);
        dirs.push(home.join(".cargo").join("bin"));
        dirs.push(home.join(".local").join("bin"));
        // asdf default data dir + Nix per-user profile.
        dirs.push(home.join(".asdf").join("shims"));
        dirs.push(home.join(".nix-profile").join("bin"));
    }
    // fnm: the binary sits directly in its data-dir root, and fnm-managed node
    // (plus npm-global CLIs such as playwright-cli) live under
    // `<root>/aliases/{default,lts}/bin`. The root differs by install method —
    // Aleph pins `~/.fnm`, a manual install uses the XDG default
    // `~/.local/share/fnm`, `$FNM_DIR` overrides both — so probe every known
    // root. Resolving only the XDG default (the old behaviour) silently missed
    // node/playwright-cli whenever fnm was the Aleph-pinned `~/.fnm`.
    for root in fnm_data_dir_candidates() {
        dirs.extend(fnm_node_bin_dirs(&root));
        dirs.push(root);
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        // Homebrew's `rustup` formula keeps cargo/rustc proxies in its keg
        // (`$(brew --prefix)/opt/rustup/bin`) and links ONLY rustup itself into
        // `…/bin`, so the standard Homebrew bin above misses cargo. Cover both
        // the Apple-Silicon (`/opt/homebrew`) and Intel (`/usr/local`) prefixes.
        dirs.push(PathBuf::from("/opt/homebrew/opt/rustup/bin"));
        dirs.push(PathBuf::from("/usr/local/opt/rustup/bin"));
        // MacPorts default prefix (alternative package manager to Homebrew).
        dirs.push(PathBuf::from("/opt/local/bin"));
        dirs.push(PathBuf::from("/Library/Developer/CommandLineTools/usr/bin"));
        // Nix: multi-user default profile + nix-darwin system profile.
        dirs.push(PathBuf::from("/nix/var/nix/profiles/default/bin"));
        dirs.push(PathBuf::from("/run/current-system/sw/bin"));
    }
    #[cfg(target_os = "linux")]
    {
        dirs.push(PathBuf::from("/usr/local/bin"));
        // Nix: multi-user default profile + NixOS system profile.
        dirs.push(PathBuf::from("/nix/var/nix/profiles/default/bin"));
        dirs.push(PathBuf::from("/run/current-system/sw/bin"));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(h) = std::env::var_os("USERPROFILE") {
            dirs.push(
                PathBuf::from(h)
                    .join("AppData")
                    .join("Local")
                    .join("Microsoft")
                    .join("WinGet")
                    .join("Links"),
            );
        }
        // npm's default global-bin on Windows for any non-fnm node (official
        // MSI, winget, scoop, choco). `npm install -g @playwright/cli` drops
        // `playwright-cli.cmd` here, so it must be on the probe search path.
        if let Some(p) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(p).join("npm"));
        }
        // scoop shims (e.g. a scoop-installed node / fnm / standalone CLI).
        if let Some(scoop) = scoop_root() {
            dirs.push(scoop.join("shims"));
        }
        // Chocolatey shim dir and Volta's managed bin — other common managers.
        dirs.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin"));
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(p).join("Volta").join("bin"));
        }
        dirs.push(PathBuf::from(r"C:\Program Files\Git\cmd"));
    }
    dirs
}

/// Scoop install root: honours an explicit `$SCOOP`, else the per-user default
/// `%USERPROFILE%\scoop`. Used to locate scoop-managed shims and the
/// scoop-installed fnm data dir.
#[cfg(target_os = "windows")]
fn scoop_root() -> Option<PathBuf> {
    if let Some(s) = std::env::var_os("SCOOP") {
        return Some(PathBuf::from(s));
    }
    std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("scoop"))
}

/// All fnm data-dir roots to probe *without invoking fnm* (fnm itself may be off
/// PATH). The root holds the `fnm` binary directly and the managed node under
/// `<root>/aliases/<alias>/bin`. fnm's location depends on how it was installed:
/// `$FNM_DIR` (explicit override), `~/.fnm` (Aleph pins this via
/// `--install-dir`), or the XDG default (`~/.local/share/fnm` on Unix,
/// `%LOCALAPPDATA%\fnm` on Windows). A service-launched Windows daemon has no
/// `HOME` (only `USERPROFILE`), so fall back accordingly.
fn fnm_data_dir_candidates() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("FNM_DIR") {
        roots.push(PathBuf::from(d));
    }
    if let Some(h) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(h);
        roots.push(home.join(".fnm"));
        #[cfg(not(target_os = "windows"))]
        roots.push(home.join(".local").join("share").join("fnm"));
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(p) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(p).join("fnm"));
        }
        // fnm installed via scoop pins FNM_DIR into the scoop tree
        // (`<scoop>\apps\fnm\current`), with node-versions/aliases persisted at
        // `<scoop>\persist\fnm`. A service-launched daemon that doesn't inherit
        // the session FNM_DIR still resolves node here.
        if let Some(scoop) = scoop_root() {
            roots.push(scoop.join("apps").join("fnm").join("current"));
            roots.push(scoop.join("persist").join("fnm"));
        }
    }
    roots
}

/// fnm-managed node bin dirs under one data-dir `root`. fnm keeps each node —
/// and every npm-global CLI installed through it, such as `playwright-cli` —
/// under `<root>/node-versions/<v>/installation[/bin]`, with `<root>/aliases/<name>`
/// symlinked to a chosen installation. On Unix the bins live in a `bin` subdir;
/// on Windows `node.exe` and the `.cmd` shims sit directly in the installation dir.
///
/// We enumerate *every* alias (not just `default`/`lts` — a user-managed fnm may
/// name its blessed node `lts-latest` or anything else) and then *every* installed
/// version, so a global CLI is found regardless of which alias/version holds it.
/// Aliases are searched first (the blessed/active node), versions as a fallback.
fn fnm_node_bin_dirs(root: &Path) -> Vec<PathBuf> {
    fn read_sorted(dir: PathBuf) -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
            Err(_) => Vec::new(),
        };
        entries.sort(); // deterministic order: `default` precedes `lts-latest`
        entries
    }
    fn push_install(install_dir: PathBuf, out: &mut Vec<PathBuf>) {
        let bin = install_dir.join("bin");
        if bin.is_dir() {
            out.push(bin);
        } else if install_dir.is_dir() {
            out.push(install_dir);
        }
    }
    let mut dirs = Vec::new();
    // Aliases first (`default`, `lts`, `lts-latest`, or any user-named alias).
    for alias_dir in read_sorted(root.join("aliases")) {
        push_install(alias_dir, &mut dirs);
    }
    // Then every installed version: <root>/node-versions/<v>/installation[/bin].
    for version_dir in read_sorted(root.join("node-versions")) {
        push_install(version_dir.join("installation"), &mut dirs);
    }
    dirs
}

static REGEX_CACHE: Lazy<Mutex<HashMap<&'static str, Regex>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn get_compiled_regex(pattern: &'static str) -> Option<Regex> {
    let mut cache = REGEX_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(re) = cache.get(pattern) {
        // rust-doctor-disable-next-line excessive-clone
        return Some(re.clone());
    }
    match Regex::new(pattern) {
        Ok(re) => {
            // rust-doctor-disable-next-line excessive-clone
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
    search_path: &OsStr,
) -> Option<String> {
    let mut cmd = Command::new(bin_path);
    // Run the version probe with the enriched PATH: a node-shebang tool
    // (`playwright-cli`, `#!/usr/bin/env node`) resolves its interpreter via
    // PATH, so on a daemon's minimal PATH `node` would be "not found" and the
    // version would come back empty even though the binary itself was located.
    cmd.arg(version_flag).env("PATH", search_path);
    let output = cmd.no_window().output().ok()?;
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

/// Compare two version strings, checking only major.minor.
///
/// Patch and pre-release components are ignored — this is intentional
/// because runtime specs declare `min_version` at the minor level
/// (e.g. "18.0" for Node). If finer-grained comparison is needed in
/// the future, this should be replaced by a proper semver parser.
fn version_lt(actual: &str, minimum: &str) -> bool {
    let parse = |s: &str| -> (u64, u64) {
        let mut parts = s.split('.');
        let major = parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(0);
        let minor = parts
            .next()
            .and_then(|p| p.parse::<u64>().ok())
            .unwrap_or(0);
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

    #[test]
    fn test_extend_path_prepends_existing_candidate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cand = tmp.path().to_path_buf();
        // Build the base PATH with the platform separator (':' on POSIX, ';' on
        // Windows) so split_paths round-trips it correctly on every platform.
        let base =
            std::env::join_paths([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]).unwrap();
        let out = extend_path(&base, std::slice::from_ref(&cand));
        let dirs: Vec<PathBuf> = std::env::split_paths(&out).collect();
        assert_eq!(
            dirs.first(),
            Some(&cand),
            "existing candidate must be prepended ahead of the base PATH"
        );
        assert!(dirs.contains(&PathBuf::from("/usr/bin")));
    }

    #[test]
    fn test_extend_path_skips_nonexistent() {
        let base = OsString::from("/usr/bin");
        let ghost = PathBuf::from("/no/such/dir/xyz123");
        let out = extend_path(&base, std::slice::from_ref(&ghost));
        assert_eq!(out, base, "non-existent candidate must not be added");
    }

    #[test]
    fn test_extend_path_skips_duplicate() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cand = tmp.path().to_path_buf();
        let base = std::env::join_paths([cand.clone(), PathBuf::from("/bin")]).unwrap();
        let out = extend_path(&base, std::slice::from_ref(&cand));
        let count = std::env::split_paths(&out).filter(|d| d == &cand).count();
        assert_eq!(count, 1, "candidate already on PATH must not be duplicated");
    }

    /// fnm-managed node lives under `<root>/aliases/<alias>/bin` (Unix). The
    /// resolver must find it under whichever data-dir root fnm was installed to
    /// — the Aleph-pinned `~/.fnm` just as much as the XDG default. This is the
    /// regression that hid node/playwright-cli on Linux when fnm was `~/.fnm`.
    #[test]
    fn test_fnm_node_bin_dirs_resolves_alias_bin_under_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        // No aliases yet → nothing resolved.
        assert!(fnm_node_bin_dirs(root).is_empty());

        // Lay out `<root>/aliases/lts/bin` as fnm would after `fnm alias .. lts`.
        let lts_bin = root.join("aliases").join("lts").join("bin");
        std::fs::create_dir_all(&lts_bin).unwrap();
        assert_eq!(
            fnm_node_bin_dirs(root),
            vec![lts_bin],
            "the `lts` alias bin dir under the given root must be resolved"
        );
    }

    /// Regression: cargo installed via Homebrew's `rustup` formula lives in the
    /// keg's opt dir (`/opt/homebrew/opt/rustup/bin`), NOT `/opt/homebrew/bin`
    /// — which holds only `rustup`/`rustup-init` symlinks. A GUI-launched daemon
    /// on a minimal PATH must still discover it, so the keg path (plus the Intel
    /// prefix and MacPorts) has to be among the probe candidates.
    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_candidates_cover_homebrew_rustup_keg_and_macports() {
        let cands = install_dir_candidates();
        for expected in [
            "/opt/homebrew/opt/rustup/bin",
            "/usr/local/opt/rustup/bin",
            "/opt/local/bin",
        ] {
            assert!(
                cands.contains(&PathBuf::from(expected)),
                "macOS probe candidates must include {expected}; got {cands:?}"
            );
        }
    }

    /// asdf shims and Nix profiles are version-manager / package-manager install
    /// targets that a GUI-launched (or service-launched) daemon's minimal PATH
    /// omits. They must be among the probe candidates on every Unix platform.
    #[cfg(unix)]
    #[test]
    fn test_unix_candidates_cover_asdf_and_nix() {
        // `install_dir_candidates()` derives the home-based candidates from the
        // process-global `$HOME`, and the assertions below re-read it. Without
        // the guard a concurrently-running `post_install` test can swap `$HOME`
        // between those two reads, so the candidate list is built against its
        // tempdir while the expectation is built against the real home — a flake
        // that fires only under the full parallel suite. `HomeEnvGuard::acquire`
        // is the read-only arm of the crate's single HOME mutual-exclusion source.
        let _home_guard = crate::runtimes::post_install::HomeEnvGuard::acquire();

        let cands = install_dir_candidates();
        // System Nix profiles are unconditional on macOS/Linux.
        assert!(
            cands.contains(&PathBuf::from("/nix/var/nix/profiles/default/bin")),
            "Nix default profile must be a probe candidate; got {cands:?}"
        );
        assert!(
            cands.contains(&PathBuf::from("/run/current-system/sw/bin")),
            "NixOS / nix-darwin system profile must be a probe candidate; got {cands:?}"
        );
        // Home-based asdf shims + Nix per-user profile (only when HOME is known).
        if let Some(h) = std::env::var_os("HOME") {
            let home = PathBuf::from(h);
            assert!(
                cands.contains(&home.join(".asdf").join("shims")),
                "asdf shims dir must be a probe candidate"
            );
            assert!(
                cands.contains(&home.join(".nix-profile").join("bin")),
                "Nix per-user profile must be a probe candidate"
            );
        }
    }

    /// A user-managed fnm may name its blessed node alias anything (`lts-latest`,
    /// not just `default`/`lts`). The resolver must enumerate every alias, else
    /// node/playwright-cli hide under a non-standard alias name — the Windows +
    /// scoop regression observed in the field.
    #[test]
    fn test_fnm_node_bin_dirs_enumerates_arbitrary_alias() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let bin = root.join("aliases").join("lts-latest").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        assert!(
            fnm_node_bin_dirs(root).contains(&bin),
            "an arbitrarily named alias's bin dir must be resolved"
        );
    }

    /// When no alias points at a version, the installed version's dir must still
    /// be searched so a global CLI installed there is found. Mirrors the Windows
    /// layout where the bins sit directly in `installation` (no `bin` subdir).
    #[test]
    fn test_fnm_node_bin_dirs_falls_back_to_node_versions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let install = root
            .join("node-versions")
            .join("v20.0.0")
            .join("installation");
        std::fs::create_dir_all(&install).unwrap();
        // No `bin` subdir → the installation dir itself is the bin dir (Windows).
        assert!(
            fnm_node_bin_dirs(root).contains(&install),
            "an installed version's dir must be searched even without an alias"
        );
    }

    /// Reproduces the GUI-minimal-PATH root cause: a binary that lives in a dir
    /// absent from the inherited PATH is invisible until that dir is folded into
    /// the enriched search path.
    #[test]
    fn test_find_in_dirs_finds_binary_only_after_enrichment() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bin = tmp.path().join("mytool");
        std::fs::write(&bin, b"#!/bin/sh\ntrue\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&bin).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&bin, p).unwrap();
        }

        // Minimal base PATH does not include tmp → not found (the bug).
        let minimal = OsString::from("/usr/bin:/bin");
        assert!(
            find_in_dirs("mytool", &minimal).is_none(),
            "precondition: tool must be invisible on the minimal PATH"
        );

        // Enriched PATH includes tmp → found (the fix).
        let enriched = extend_path(&minimal, std::slice::from_ref(&tmp.path().to_path_buf()));
        assert_eq!(
            find_in_dirs("mytool", &enriched).as_deref(),
            Some(bin.as_path()),
            "tool must be found once its dir is part of the enriched search path"
        );
    }

    /// Regression: a node-shebang tool (`playwright-cli`, `#!/usr/bin/env node`)
    /// resolves its interpreter via PATH, so `get_version` must run the version
    /// probe with the *enriched* PATH. On a daemon's minimal PATH the interpreter
    /// is missing and the version comes back empty (found=true, version=None) —
    /// the bug this fix closes. Modelled with a tool that execs a helper which
    /// only exists in the enriched dir.
    #[cfg(unix)]
    #[test]
    fn test_get_version_runs_probe_with_enriched_path() {
        use std::os::unix::fs::PermissionsExt;
        let set_exec = |p: &Path| {
            let mut perms = std::fs::metadata(p).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(p, perms).unwrap();
        };

        let interp_dir = tempfile::TempDir::new().unwrap();
        let tool_dir = tempfile::TempDir::new().unwrap();

        // A helper that lives ONLY in the enriched dir; prints a version banner.
        let helper = interp_dir.path().join("aleph-fake-interp");
        std::fs::write(&helper, b"#!/bin/sh\necho 'tool version 9.8.7'\n").unwrap();
        set_exec(&helper);

        // The tool resolves `aleph-fake-interp` via PATH (mimics env-shebang).
        let tool = tool_dir.path().join("mytool");
        std::fs::write(&tool, b"#!/bin/sh\nexec aleph-fake-interp\n").unwrap();
        set_exec(&tool);

        let re = r"version (\d+\.\d+\.\d+)";

        // Minimal PATH: interpreter unresolvable → no version captured (the bug).
        let minimal = OsString::from("/usr/bin:/bin");
        assert_eq!(
            get_version(&tool, "--version", re, &minimal),
            None,
            "precondition: interpreter must be unresolvable on the minimal PATH"
        );

        // Enriched PATH includes the interpreter dir → version captured (the fix).
        let enriched = extend_path(
            &minimal,
            std::slice::from_ref(&interp_dir.path().to_path_buf()),
        );
        assert_eq!(
            get_version(&tool, "--version", re, &enriched).as_deref(),
            Some("9.8.7"),
            "version must be captured once the interpreter dir is on the enriched PATH"
        );
    }
}
