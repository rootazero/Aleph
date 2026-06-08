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
    Unsupported {
        reason: &'static str,
    },
}

pub enum PostInstallAction {
    RunSubcommand {
        args: &'static [&'static str],
        target_dir: Option<&'static str>,
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
        post_install: &[PostInstallAction::AssetProbe {
            path: "$HOME/.aleph/.venv/bin/python",
            repair: &["venv", "$HOME/.aleph/.venv"],
        }],
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
                subcommand: &["npm", "install", "-g", "@playwright/cli@latest"],
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
            "Git — version control. Use `git <subcommand>` (clone, status, diff, commit, log). Auto-installed via the platform's native package manager when missing.",
        ),
    },
];

pub fn find_spec(name: &str) -> Option<&'static RuntimeSpec> {
    SPECS.iter().find(|s| s.name == name)
}

pub fn select_install(installs: &[OsInstall], current: TargetOs) -> Option<&OsInstall> {
    installs.iter().find(|oi| oi.os.matches(current))
}

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
