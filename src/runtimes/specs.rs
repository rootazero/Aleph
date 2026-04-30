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
    // Detection-only: git has no language-native manager; users install via
    // OS-standard channels (Xcode CLT, apt/dnf, winget). If missing, Aleph
    // surfaces an actionable error; it does not auto-install.
    RuntimeSpec {
        name: "git",
        binaries: &["git"],
        version_flag: "--version",
        version_regex: r"git version (\d+\.\d+\.\d+)",
        min_version: None,
        deps: &[],
        install: &[],
        post_install: &[],
        llm_hint: Some(
            "Git — version control. Use `git <subcommand>` (clone, status, diff, commit, log). Available as a system binary; Aleph does not manage its version.",
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
    fn test_git_is_detection_only() {
        // git has no install strategy — users install it through OS channels.
        let spec = find_spec("git").unwrap();
        assert!(spec.install.is_empty());
        assert!(!supported_on_current_os("git"));
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
    }

    #[test]
    fn test_supported_on_current_os_for_cargo_placeholder() {
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
        use crate::runtimes::post_install::HOME_LOCK;
        use std::os::unix::fs::PermissionsExt;
        use tempfile::TempDir;

        let _lock = HOME_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());

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
