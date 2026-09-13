//! Runtime specification table — single source of truth for probe/install/LLM-hint.

use super::os::TargetOs;

pub struct RuntimeSpec {
    pub name: &'static str,
    pub binaries: &'static [&'static str],
    pub version_flag: &'static str,
    pub version_regex: &'static str,
    pub min_version: Option<&'static str>,
    pub deps: &'static [&'static str],
    pub install: &'static [OsInstall],
    pub post_install: &'static [PostInstallAction],
    pub llm_hint: Option<&'static str>,
    /// Concrete manual-install command(s), surfaced as the "Install manually"
    /// fix option when auto-install fails. `None` falls back to `llm_hint` —
    /// which is *usage* guidance, not an install command. Phrased for the user
    /// to copy-paste; Aleph never runs this string (the `install` strategies
    /// above are the real ones).
    pub install_hint: Option<&'static str>,
}

pub struct OsInstall {
    pub os: TargetOs,
    pub strategy: InstallStrategy,
}

pub enum InstallStrategy {
    Shell(&'static str),
    PowerShell(&'static str),
    Via {
        parent: &'static str,
        subcommand: &'static [&'static str],
    },
    /// A globally installed npm CLI.
    ///
    /// Its own variant rather than `Via { parent: "node", subcommand: ["npm",
    /// "install", "-g", …] }` because the install *location* is a runtime,
    /// per-platform decision and therefore cannot live in a `&'static` argv:
    /// see [`super::npm_global::prefix`] for why the node installation's own
    /// tree is the one place it must not go.
    NpmGlobal {
        package: &'static str,
    },
    /// A checksummed binary pulled from a GitHub release asset.
    ///
    /// Its own variant rather than `Shell("curl -L … | tar xz")` for the same
    /// reason `NpmGlobal` is not `Via`: the decision is per-`(os, arch)` and
    /// therefore cannot live in a `&'static` argv. The `Shell` spelling would
    /// also carry no digest — and an unverified ~90 MB binary that then gets
    /// `chmod 755` and executed is a supply-chain hole with the checksum
    /// sitting unread in the release metadata, one HTTP request away.
    ///
    /// `asset` returns `None` for a platform the upstream release matrix does
    /// not build. That is a *stated* answer ("this platform has no obscura"),
    /// never a fallback to some other asset.
    GithubRelease {
        /// `owner/name`.
        repo: &'static str,
        /// The pinned release tag. Bumping it is the whole upgrade: the
        /// installer's target directory and [`super::probe`]'s search
        /// directory both derive from this one string.
        tag: &'static str,
        /// Archive name for `(std::env::consts::OS, std::env::consts::ARCH)`.
        asset: fn(os: &str, arch: &str) -> Option<&'static str>,
        /// The single member to extract, compared against whole archive paths.
        binary_in_archive: &'static str,
    },
}

/// The ledger name of the obscura engine runtime. Spelled once so the doctor,
/// `runtime_manage`, the probe's search directory and this table cannot drift.
pub const OBSCURA_RUNTIME: &str = "obscura";

/// The pinned obscura release, as a macro so that the ONE place it is spelled
/// can also be pasted into a `concat!`.
///
/// A plain `const` would have been enough for every consumer but one: the
/// spec's `install_hint` is operator-facing prose carrying a
/// `…/releases/tag/<tag>` URL, and `&'static str` prose cannot interpolate a
/// `const`. Written as a literal there, the tag would have TWO authors — and
/// `the_obscura_tag_has_one_author` would still have passed, because the URL
/// spells it without the surrounding quotes the first draft of that test
/// looked for (判据 §3: a guard's green only covers the shape it recognises).
macro_rules! obscura_tag {
    () => {
        "v0.2.2"
    };
}

/// The pinned obscura release. Bump this — and only this — when the upstream
/// `DOMSnapshot` layout fix lands (spec §6.1); everything else follows.
pub const OBSCURA_TAG: &str = obscura_tag!();

/// The five-platform release matrix: measured for `aarch64-macos`, inferred by
/// symmetry for the other four (see this task's "before you start" check).
///
/// `aarch64-windows` is genuinely absent upstream (`release.yml:15-42`,
/// `obscura-source-survey.md` §9), so it answers `None`.
///
/// The `default` archive is chosen deliberately: it is the one built with
/// `--features render`. A `no-render` archive answers `DOM.getBoxModel` and
/// `DOMSnapshot.captureSnapshot` with fabricated geometry and puts no marker
/// on the wire (`obscura-source-survey.md` §3), so it would silently turn the
/// page-state tree's rectangles into lies. `stealth` is a profile-level
/// opt-in (`[general.browser.obscura] variant`), not a ledger decision.
fn obscura_asset(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("macos", "aarch64") => Some("obscura-aarch64-macos.tar.gz"),
        ("macos", "x86_64") => Some("obscura-x86_64-macos.tar.gz"),
        ("linux", "aarch64") => Some("obscura-aarch64-linux.tar.gz"),
        ("linux", "x86_64") => Some("obscura-x86_64-linux.tar.gz"),
        ("windows", "x86_64") => Some("obscura-x86_64-windows.zip"),
        _ => None,
    }
}

/// A process-environment variable a post-install action needs, named by the
/// config key that supplies it.
///
/// An enum rather than a `(&str, &str)` pair because the value is not static:
/// it comes out of the running config, and the resolver
/// (`post_install::config_env_from`) is the single place that maps a variant to
/// a key. Both consumers — the post-install runner and the R8 install tool —
/// go through it, so the mirror cannot be honoured on one path and dropped on
/// the other (判据 §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvFromConfig {
    /// `PLAYWRIGHT_DOWNLOAD_HOST` ← `[general.browser.runtime] download_host`.
    ///
    /// Until 2026-09-13 this line named the section `browser.runtime`, which
    /// `Config` has never had (`GeneralConfig::browser` is not
    /// `#[serde(flatten)]`, so there is no top-level `browser` table). It
    /// survived because `dead_keys`'s census scanned four files and this was
    /// not one of them; adding `specs.rs` to that list found it on the first
    /// run (判据 §5 — a scan covers exactly what it enumerates). The wrong
    /// spelling is deliberately not repeated in brackets here: this file is
    /// now scanned, and a bracketed example would be indistinguishable from
    /// the defect.
    PlaywrightDownloadHost,
}

pub enum PostInstallAction {
    RunSubcommand {
        args: &'static [&'static str],
        target_dir: Option<&'static str>,
        /// Environment this subcommand needs from the running config. Empty for
        /// every action but the Chromium download.
        env: &'static [EnvFromConfig],
    },
    FnmAlias {
        alias_name: &'static str,
    },
    AssetProbe {
        path: &'static str,
        repair: &'static [&'static str],
    },
}

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
                    // Pin --install-dir so the binary lands in the exact dir the
                    // re-probe (enrich_path_for_reprobe) and cold-probe candidate
                    // sets look in (`$HOME/.fnm`). Without the pin the installer's
                    // XDG default (`~/.local/share/fnm`) diverges from those, so
                    // `which fnm` fails post-install → spurious "binary not found".
                    "curl -fsSL https://fnm.vercel.app/install | bash -s -- --skip-shell --install-dir \"$HOME/.fnm\"",
                ),
            },
            OsInstall {
                os: TargetOs::Windows,
                strategy: InstallStrategy::PowerShell(
                    // --accept-package-agreements is required alongside the source
                    // flag: without it winget can prompt for the package license
                    // even under --silent, and there is no TTY here (run via
                    // `powershell -Command` + cmd.output()), so it would hang until
                    // the 600s bootstrap timeout. Mirrors the cargo/git specs.
                    "winget install Schniz.fnm --silent --accept-package-agreements --accept-source-agreements",
                ),
            },
        ],
        post_install: &[],
        llm_hint: Some("Node version manager (fnm). Used implicitly by `node`."),
        install_hint: Some(
            "macOS/Linux: `curl -fsSL https://fnm.vercel.app/install | bash`. \
             Windows: `winget install Schniz.fnm`.",
        ),
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
        install_hint: Some("Install fnm first, then `fnm install --lts`."),
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
            "irm https://astral.sh/uv/install.ps1 | iex",
        ),
            },
        ],
        post_install: &[PostInstallAction::AssetProbe {
            path: "$HOME/.aleph/.venv/bin/python",
            repair: &["venv", "$HOME/.aleph/.venv"],
        }],
        llm_hint: Some(
            "Python package manager (uv). Run scripts via `uv run <file.py>`; install packages via `uv pip install <pkg>`.",
        ),
        install_hint: Some(
            "macOS/Linux: `curl -LsSf https://astral.sh/uv/install.sh | sh`. \
             Windows: `irm https://astral.sh/uv/install.ps1 | iex`.",
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
            strategy: InstallStrategy::NpmGlobal {
                package: "@playwright/cli@latest",
            },
        }],
        // v0.1.14 renamed the browser-install subcommand: the legacy
        // `install chromium` became `install-browser chromium` (`install` now
        // only "initializes a workspace"). The old `install --skills --target
        // <dir>` action was dropped: v0.1.14 removed `--target` (skills are
        // written CWD-relative to `.claude/skills/`, not a redirectable dir), so
        // it can't be ported 1:1. The model already knows the command set and
        // gets usage from `llm_hint`, so we don't stage a filesystem skill.
        post_install: &[PostInstallAction::RunSubcommand {
            args: &["install-browser", "chromium"],
            target_dir: None,
            env: &[EnvFromConfig::PlaywrightDownloadHost],
        }],
        llm_hint: Some(
            "Browser automation CLI. Use `playwright-cli -s=<session> <command>`.",
        ),
        install_hint: Some(
            "Requires Node, then `npm install -g --prefix ~/.local @playwright/cli@latest` \
             (Windows: `--prefix %APPDATA%\\npm`). The prefix keeps it out of the node \
             version manager's tree, where the next node upgrade would delete it.",
        ),
    },
    // Cargo / Rust toolchain. Detection-first: if `cargo` is on PATH (user
    // installed rustup themselves, distro `rust` package, or `nix-shell`), we
    // use it as-is. Falls back to platform-recommended rustup install when
    // missing. Bootstrap re-probe relies on `enrich_path_for_reprobe` adding
    // `$HOME/.cargo/bin` (Unix) or `%USERPROFILE%\.cargo\bin` (Windows) to PATH.
    RuntimeSpec {
        name: "cargo",
        binaries: &["cargo"],
        version_flag: "--version",
        version_regex: r"cargo (\d+\.\d+\.\d+)",
        min_version: None,
        deps: &[],
        install: &[
            OsInstall {
                os: TargetOs::AnyUnix,
                strategy: InstallStrategy::Shell(
                    "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal",
                ),
            },
            OsInstall {
                os: TargetOs::Windows,
                strategy: InstallStrategy::PowerShell(
                    "winget install --id Rustlang.Rustup --silent --accept-package-agreements --accept-source-agreements",
                ),
            },
        ],
        post_install: &[],
        llm_hint: Some(
            "Rust toolchain (cargo). Use `cargo <subcommand>` (build, test, run, fmt, clippy). Installed via rustup; binaries land in `~/.cargo/bin`.",
        ),
        install_hint: Some(
            "macOS/Linux: `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`. \
             Windows: `winget install Rustlang.Rustup`.",
        ),
    },
    // Git — version control. Detection-first: respects any pre-existing system
    // git (Xcode CLT, distro package, scoop/winget). Falls back to OS-native
    // install when missing.
    //
    // Caveats:
    // - macOS without Homebrew triggers Apple's CLT GUI installer (async).
    //   The shell command returns immediately; re-probe likely fails and the
    //   user must finish the GUI flow before retrying.
    // - Linux requires sudo. Aleph inherits the daemon's effective UID; if
    //   passwordless sudo isn't configured the install will fail and the
    //   actionable error guides the user to the manual command.
    RuntimeSpec {
        name: "git",
        binaries: &["git"],
        version_flag: "--version",
        version_regex: r"git version (\d+\.\d+\.\d+)",
        min_version: None,
        deps: &[],
        install: &[
            OsInstall {
                os: TargetOs::MacOs,
                strategy: InstallStrategy::Shell(
                    "if command -v brew >/dev/null 2>&1; then brew install git; else xcode-select --install >/dev/null 2>&1 || true; fi",
                ),
            },
            OsInstall {
                os: TargetOs::Linux,
                strategy: InstallStrategy::Shell(
                    "if command -v apt-get >/dev/null 2>&1; then sudo apt-get update && sudo apt-get install -y git; \
                     elif command -v dnf >/dev/null 2>&1; then sudo dnf install -y git; \
                     elif command -v pacman >/dev/null 2>&1; then sudo pacman -S --noconfirm git; \
                     elif command -v apk >/dev/null 2>&1; then sudo apk add --no-cache git; \
                     elif command -v zypper >/dev/null 2>&1; then sudo zypper -n install git; \
                     else echo 'no supported package manager (apt/dnf/pacman/apk/zypper) on PATH' >&2; exit 1; fi",
                ),
            },
            OsInstall {
                os: TargetOs::Windows,
                strategy: InstallStrategy::PowerShell(
                    "winget install --id Git.Git -e --source winget --silent --accept-package-agreements --accept-source-agreements",
                ),
            },
        ],
        post_install: &[],
        llm_hint: Some(
            "Git — version control. Use `git <subcommand>` (clone, status, diff, commit, log).",
        ),
        install_hint: Some(
            "macOS: `brew install git`, or `xcode-select --install` then finish the \
             Command Line Tools dialog and retry (don't re-run while it downloads). \
             Linux: `sudo apt-get install -y git` (or your distro's package manager: \
             dnf / pacman / apk / zypper). Windows: `winget install Git.Git`.",
        ),
    },
    // obscura — Aleph's default browser engine. An external runtime like every
    // other entry here: Aleph spawns it and speaks CDP to it, never links it
    // (R1/R3; spec §2.2 records that obscura's `Page` is not `Send`, which
    // settles the crate-linking alternative on its own).
    RuntimeSpec {
        name: OBSCURA_RUNTIME,
        binaries: &["obscura"],
        version_flag: "--version",
        // A source build reports `0.1.0` — its version string comes from
        // `OBSCURA_VERSION` → the git tag ref → `CARGO_PKG_VERSION`, and the
        // workspace version is frozen at 0.1.0 (`obscura-source-survey.md` §9).
        // The ledger only ever installs a release archive, so the tag is what
        // this regex sees; a developer's own build trips `min_version` and gets
        // a `version_warning`, which is the correct answer for it.
        version_regex: r"obscura (\d+\.\d+\.\d+)",
        min_version: Some("0.2.2"),
        deps: &[],
        install: &[OsInstall {
            os: TargetOs::AnyOs,
            strategy: InstallStrategy::GithubRelease {
                repo: "h4ckf0r0day/obscura",
                tag: OBSCURA_TAG,
                asset: obscura_asset,
                binary_in_archive: "obscura",
            },
        }],
        // `obscura-worker` is deliberately NOT installed: only `obscura scrape`
        // uses it, Aleph drives `serve`, and it is another ~86 MB.
        post_install: &[],
        llm_hint: Some(
            "Aleph's default browser engine. Launched by the browser subsystem \
             as `obscura serve`; not something to run by hand.",
        ),
        // The tag comes from `obscura_tag!()`, not from a second literal: see
        // that macro's doc for the guard this would otherwise have slipped past.
        install_hint: Some(concat!(
            "Ask Aleph to run `runtime_manage{action:\"install\", capability:\"obscura\"}`, ",
            "or download the release archive for your platform from ",
            "https://github.com/h4ckf0r0day/obscura/releases/tag/",
            obscura_tag!(),
            " and pin the extracted binary with [general.browser.obscura] binary_path.",
        )),
    },
];

#[must_use]
pub fn find_spec(name: &str) -> Option<&'static RuntimeSpec> {
    SPECS.iter().find(|s| s.name == name)
}

#[must_use]
pub fn select_install(installs: &[OsInstall], current: TargetOs) -> Option<&OsInstall> {
    installs.iter().find(|oi| oi.os.matches(current))
}

#[must_use]
pub fn supported_on_current_os(name: &str) -> bool {
    let Some(os) = TargetOs::current() else {
        return false;
    };
    supported_on(name, os, std::env::consts::OS, std::env::consts::ARCH)
}

/// The whole "is there something to install here" predicate, with the platform
/// as **parameters**.
///
/// [`supported_on_current_os`] is this function with the machine's own answers
/// filled in. Separated for one reason: with the platform read from `consts`
/// inside, the arch axis is unreachable from a test — on an aarch64 mac,
/// obscura is supported whichever way the body is written, so restoring the
/// old `select_install(..).is_some()` body would leave every test green
/// (判据 §2 — a guard that cannot go red for the reason it names). With the
/// platform as arguments, `("windows", "aarch64")` is one call away.
///
/// It is also the ONE derivation of this predicate. `runtime_manage`'s
/// `obscura_row` and the `browser/obscura-missing` check both need the same
/// answer, and both used to compose `TargetOs::current` + `select_install` +
/// `strategy_supported_here` by hand — three authors for one fact (判据 §1).
#[must_use]
pub fn supported_on(name: &str, target: TargetOs, os: &str, arch: &str) -> bool {
    let Some(spec) = find_spec(name) else {
        return false;
    };
    let Some(oi) = select_install(spec.install, target) else {
        return false;
    };
    strategy_supported_here(&oi.strategy, os, arch)
}

/// Whether a strategy selected for this OS actually has something to install
/// on this **architecture**.
///
/// [`select_install`] answers a question about the OS arm only, and for the
/// four original strategies that is the whole question. `GithubRelease` adds a
/// second axis: `TargetOs::AnyOs` matches aarch64-windows, where the upstream
/// release matrix has no asset. Folding that into `Some(..)` makes
/// `runtime_manage{list}`'s `supported_here` a constant `true` on exactly the
/// platform where it is false — and that column is what an operator reads
/// before deciding to wait for a ~90 MB download (判据 §2).
///
/// `os` / `arch` are parameters rather than `consts` read inside, so the whole
/// table can be exercised from one machine.
#[must_use]
pub fn strategy_supported_here(strategy: &InstallStrategy, os: &str, arch: &str) -> bool {
    match strategy {
        InstallStrategy::GithubRelease { asset, .. } => asset(os, arch).is_some(),
        InstallStrategy::Shell(_)
        | InstallStrategy::PowerShell(_)
        | InstallStrategy::Via { .. }
        | InstallStrategy::NpmGlobal { .. } => true,
    }
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
    fn test_all_specs_have_nonempty_binaries() {
        for spec in SPECS {
            assert!(
                !spec.binaries.is_empty(),
                "spec '{}' has no binaries; probe would silently return NotFound",
                spec.name
            );
        }
    }

    #[test]
    fn test_find_spec_known() {
        assert!(find_spec("fnm").is_some());
        assert!(find_spec("node").is_some());
        assert!(find_spec("uv").is_some());
        assert!(find_spec("playwright-cli").is_some());
        assert!(find_spec("cargo").is_some());
        assert!(find_spec("git").is_some());
    }

    #[test]
    fn test_git_has_install_strategy_on_every_concrete_os() {
        // git is auto-installable on all three platforms via the OS-native
        // package manager (xcode-select/brew, apt/dnf/pacman/..., winget).
        let spec = find_spec("git").unwrap();
        assert!(!spec.install.is_empty());
        assert!(select_install(spec.install, TargetOs::MacOs).is_some());
        assert!(select_install(spec.install, TargetOs::Linux).is_some());
        assert!(select_install(spec.install, TargetOs::Windows).is_some());
        assert!(spec.llm_hint.is_some());
    }

    #[test]
    fn test_find_spec_unknown() {
        assert!(find_spec("does-not-exist").is_none());
    }

    /// The five-platform table, asserted per pair rather than by counting —
    /// a count stays green if two rows swap their archives.
    #[test]
    fn the_obscura_asset_table_covers_five_platforms_and_admits_it_has_no_sixth() {
        let spec = find_spec(OBSCURA_RUNTIME).expect("obscura spec must exist");
        let InstallStrategy::GithubRelease {
            asset,
            repo,
            tag,
            binary_in_archive,
        } = &spec.install[0].strategy
        else {
            panic!("obscura must install from a GitHub release, not a shell script");
        };
        assert_eq!(*repo, "h4ckf0r0day/obscura");
        assert_eq!(*tag, OBSCURA_TAG);
        assert_eq!(*binary_in_archive, "obscura");
        for (os, arch, want) in [
            ("macos", "aarch64", "obscura-aarch64-macos.tar.gz"),
            ("macos", "x86_64", "obscura-x86_64-macos.tar.gz"),
            ("linux", "aarch64", "obscura-aarch64-linux.tar.gz"),
            ("linux", "x86_64", "obscura-x86_64-linux.tar.gz"),
            ("windows", "x86_64", "obscura-x86_64-windows.zip"),
        ] {
            assert_eq!(asset(os, arch), Some(want), "{os}/{arch}");
        }
        // The one pair the upstream release matrix does not build. Every
        // surface must say "this platform only has chromium" rather than
        // offering an install that cannot happen.
        assert_eq!(asset("windows", "aarch64"), None);
        assert_eq!(asset("freebsd", "x86_64"), None);
    }

    /// The default archive carries the render engine. A `no-render` build
    /// answers `DOM.getBoxModel` and `DOMSnapshot.captureSnapshot` with
    /// fabricated geometry and puts no marker on the wire
    /// (`obscura-source-survey.md` §3), so choosing one would silently turn
    /// every rectangle in the page-state tree into a lie. `stealth` is a
    /// profile-level opt-in, not a ledger decision.
    ///
    /// This is not hypothetical arithmetic: the v0.2.2 release really does
    /// carry all four flavours of each platform (20 assets, verified), so the
    /// wrong one is exactly one character away from the right one.
    #[test]
    fn no_obscura_asset_is_a_no_render_or_stealth_archive() {
        let spec = find_spec(OBSCURA_RUNTIME).unwrap();
        let InstallStrategy::GithubRelease { asset, .. } = &spec.install[0].strategy else {
            panic!("shape checked by the sibling test");
        };
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "aarch64"),
            ("linux", "x86_64"),
            ("windows", "x86_64"),
        ] {
            let name = asset(os, arch).unwrap();
            assert!(
                !name.contains("no-render"),
                "{name} would give fabricated geometry"
            );
            assert!(
                !name.contains("stealth"),
                "{name}: stealth is a profile-level opt-in, not the ledger's archive"
            );
        }
    }

    /// `select_install` answering `Some` is not the same fact as "there is
    /// something to install here". `TargetOs::AnyOs` matches aarch64-windows,
    /// where the asset table has no row — and `supported_here: true` is the
    /// column an operator reads before deciding to wait for a ~90 MB download
    /// that will never start. 判据 §2's question ("when does this go red?")
    /// has a concrete answer: aarch64-windows.
    #[test]
    fn supported_on_consults_the_asset_table_not_just_the_os_arm() {
        let spec = find_spec(OBSCURA_RUNTIME).unwrap();
        let oi = select_install(spec.install, TargetOs::Windows)
            .expect("the windows arm exists — that is exactly the trap");
        assert!(matches!(oi.strategy, InstallStrategy::GithubRelease { .. }));
        assert!(
            !strategy_supported_here(&oi.strategy, "windows", "aarch64"),
            "no asset for aarch64-windows, so nothing is installable there"
        );
        assert!(strategy_supported_here(&oi.strategy, "windows", "x86_64"));
        // Every other strategy is unconditional on arch.
        let fnm = find_spec("fnm").unwrap();
        assert!(strategy_supported_here(
            &select_install(fnm.install, TargetOs::MacOs)
                .unwrap()
                .strategy,
            "macos",
            "aarch64",
        ));

        // And the composed predicate, which is what every caller actually
        // reads. Restoring the old `select_install(..).is_some()` body turns
        // the first of these red; on an aarch64 mac NOTHING else would, which
        // is why `supported_on` takes the platform rather than reading it.
        assert!(
            !supported_on("obscura", TargetOs::Windows, "windows", "aarch64"),
            "the windows OS arm matches, but there is no aarch64-windows asset"
        );
        assert!(supported_on(
            "obscura",
            TargetOs::Windows,
            "windows",
            "x86_64"
        ));
        assert!(supported_on("fnm", TargetOs::MacOs, "macos", "aarch64"));
        assert!(!supported_on(
            "does-not-exist",
            TargetOs::MacOs,
            "macos",
            "aarch64"
        ));
    }

    /// The pinned tag is spelled once. Two copies is how a bumped installer
    /// and an unbumped probe end up looking in different directories, and the
    /// symptom is "it installed and it is still Missing" (判据 §1).
    ///
    /// Counts the BARE spelling, not `"v0.2.2"` with quotes: the spec's
    /// `install_hint` carries a `…/releases/tag/v0.2.2` URL, where the tag has
    /// no surrounding quotes, so a quote-anchored count would have certified a
    /// second author as one (判据 §3 — a guard only covers the shapes it
    /// recognises). Falsified by hand: inlining the tag into that URL instead
    /// of `obscura_tag!()` turns this red at 2.
    #[test]
    fn the_obscura_tag_has_one_author() {
        let src = include_str!("specs.rs");
        let occurrences = src
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains(OBSCURA_TAG))
            .count();
        assert_eq!(
            occurrences, 1,
            "the pinned obscura tag must be spelled once (obscura_tag!); every \
             other site refers to the macro or to OBSCURA_TAG"
        );
    }

    /// The ledger must not pull down `obscura-worker`: another ~86 MB, used
    /// only by `obscura scrape`, which Aleph never runs (it drives `serve`).
    #[test]
    fn the_obscura_spec_installs_one_binary_and_runs_nothing_afterwards() {
        let spec = find_spec(OBSCURA_RUNTIME).unwrap();
        assert!(
            spec.post_install.is_empty(),
            "a post-install action here would be a second download"
        );
        assert_eq!(spec.binaries, &["obscura"]);
        assert_eq!(spec.deps, &[] as &[&str]);
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
        assert!(supported_on_current_os("fnm"));
        // cargo and git are auto-installable on every supported OS.
        assert!(supported_on_current_os("cargo"));
        assert!(supported_on_current_os("git"));
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

    /// Global npm CLIs must go through `NpmGlobal`, never a hand-rolled
    /// `Via { parent: "node", subcommand: ["npm", "install", "-g", …] }`.
    ///
    /// That argv is `&'static`, so it cannot carry a `--prefix`, so npm falls
    /// back to its own default — which under a version manager is *inside the
    /// current node version's tree*. The package then evaporates on the next
    /// node upgrade. The variant exists so the prefix can be computed at
    /// runtime; this guard is what stops the old shape coming back.
    #[test]
    fn no_spec_installs_a_global_npm_package_through_via() {
        for spec in SPECS {
            for oi in spec.install {
                if let InstallStrategy::Via { subcommand, .. } = &oi.strategy {
                    let is_npm_global = subcommand.first() == Some(&"npm")
                        && subcommand.contains(&"install")
                        && (subcommand.contains(&"-g") || subcommand.contains(&"--global"));
                    assert!(
                        !is_npm_global,
                        "spec '{}' installs a global npm package via a static argv; \
                         use InstallStrategy::NpmGlobal so the prefix is chosen per platform",
                        spec.name,
                    );
                }
            }
        }
    }

    #[test]
    fn playwright_cli_is_a_global_npm_package() {
        let spec = find_spec("playwright-cli").expect("playwright-cli spec must exist");
        let strategies: Vec<_> = spec.install.iter().map(|oi| &oi.strategy).collect();
        assert!(
            strategies
                .iter()
                .any(|s| matches!(s, InstallStrategy::NpmGlobal { package } if package.starts_with("@playwright/cli"))),
            "playwright-cli must install as a global npm package",
        );
    }

    #[test]
    fn test_uv_spec_has_venv_post_install() {
        let spec = find_spec("uv").expect("uv spec must exist");
        assert_eq!(
            spec.post_install.len(),
            1,
            "uv should have exactly one post-install action"
        );
        match spec.post_install[0] {
            PostInstallAction::AssetProbe { path, repair } => {
                assert!(
                    path.contains(".aleph/.venv"),
                    "uv post-install should probe for ~/.aleph/.venv, got: {path}"
                );
                assert!(
                    path.ends_with("python") || path.ends_with("python.exe"),
                    "probe path should end at the python binary, got: {path}"
                );
                assert_eq!(
                    repair,
                    &["venv", "$HOME/.aleph/.venv"],
                    "repair must be `uv venv $HOME/.aleph/.venv`",
                );
            }
            _ => panic!("expected AssetProbe post-install for uv"),
        }
    }

    /// Regression for the v0.1.14 CLI drift: `@playwright/cli@latest` renamed
    /// `install chromium` → `install-browser chromium` and removed the
    /// `install --skills --target <dir>` shape. The spec must use the new
    /// browser subcommand and carry no stale skills action (which errored with
    /// "too many arguments" / "Unknown option --target" on install).
    #[test]
    fn test_playwright_cli_post_install_uses_install_browser() {
        let spec = find_spec("playwright-cli").expect("playwright-cli spec must exist");
        assert_eq!(
            spec.post_install.len(),
            1,
            "playwright-cli should have exactly one post-install action (browser install)"
        );
        match spec.post_install[0] {
            PostInstallAction::RunSubcommand {
                args,
                target_dir,
                env,
            } => {
                assert_eq!(
                    args,
                    &["install-browser", "chromium"],
                    "browser install must use the v0.1.14 `install-browser` subcommand"
                );
                assert!(
                    target_dir.is_none(),
                    "`install-browser chromium` takes no appended target dir"
                );
                assert_eq!(
                    env,
                    &[EnvFromConfig::PlaywrightDownloadHost],
                    "the chromium download is the one post-install action a mirror applies to"
                );
            }
            _ => panic!("expected a RunSubcommand post-install for playwright-cli"),
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn test_uv_post_install_creates_venv_idempotently() {
        use crate::runtimes::post_install::run;
        use crate::runtimes::post_install::HomeEnvGuard;
        use std::os::unix::fs::PermissionsExt;
        use tempfile::TempDir;

        let dir = TempDir::new().unwrap();
        let _home = HomeEnvGuard::acquire_and_set(dir.path());

        // Fake uv: a shell script that responds to `venv <path>` by mkdir-ing the
        // expected layout, mimicking `uv venv` semantics.
        let fake_uv = dir.path().join("fake-uv.sh");
        tokio::fs::write(
            &fake_uv,
            concat!(
                "#!/bin/sh\n",
                "if [ \"$1\" = \"venv\" ]; then\n",
                "  mkdir -p \"$2/bin\"\n",
                "  : > \"$2/bin/python\"\n",
                "  chmod +x \"$2/bin/python\"\n",
                "  exit 0\n",
                "fi\n",
                "exit 1\n",
            ),
        )
        .await
        .unwrap();
        let mut perms = tokio::fs::metadata(&fake_uv).await.unwrap().permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&fake_uv, perms).await.unwrap();

        let spec = find_spec("uv").unwrap();
        let action = &spec.post_install[0];

        // Round 1: venv doesn't exist → repair fires.
        run(action, &fake_uv).await.unwrap();
        let venv_python = dir.path().join(".aleph/.venv/bin/python");
        assert!(
            venv_python.exists(),
            "venv python should exist after first run"
        );

        // Round 2: venv exists → repair should be skipped (we detect by
        // removing the fake uv binary — if run re-invokes it, it will fail).
        tokio::fs::remove_file(&fake_uv).await.unwrap();
        run(action, &fake_uv).await.unwrap();
        assert!(
            venv_python.exists(),
            "venv python should still exist after idempotent second run"
        );
    }
}
