//! `aleph-server bootstrap-runtime` — install managed runtimes via `ensure_capability`.
//!
//! Spec C policy: **`NoLock`**. Bootstrap-runtime is invoked as a child
//! process by `start` while the parent already holds the singleton
//! lock; touching `~/.aleph/data/` is the parent's responsibility, so
//! this child does not contend. The marker `run_no_lock` call at
//! `run` entry preserves classification for the reverse-regression
//! check (Task 25).

use std::io::Write;

use alephcore::runtimes::{self, ensure_capability, find_spec, CapabilityLedger};
use alephcore::sync_primitives::{Arc, AsyncRwLock as RwLock};

use crate::cli::BootstrapRuntimeArgs;

/// Default target set when neither `--only` nor `--skip` is given.
///
/// `cargo` and `git` are install-capable now but intentionally excluded from
/// the default set — auto-triggering `xcode-select --install` (GUI dialog) or
/// `sudo apt-get` on every `bootstrap-runtime` invocation is too aggressive.
/// Users opt in via `--only cargo` / `--only git` or the Panel Install button.
const DEFAULT_TARGETS: &[&str] = &["uv", "playwright-cli"];

/// Run the `bootstrap-runtime` subcommand. Returns a POSIX-style exit code.
pub async fn run(args: BootstrapRuntimeArgs) -> i32 {
    // Spec C Task 19: NoLock policy marker — parent owns the lock.
    if alephcore::cli::policy::run_no_lock(|| Ok::<(), anyhow::Error>(())).is_err() {
        return 2;
    }
    let runtimes_dir = match runtimes::get_runtimes_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: cannot locate ~/.aleph/runtimes/: {e}");
            return 2;
        }
    };
    if let Err(e) = tokio::fs::create_dir_all(&runtimes_dir).await {
        eprintln!(
            "error: cannot create runtimes dir {}: {e}",
            runtimes_dir.display()
        );
        return 2;
    }
    let ledger_path = runtimes_dir.join("ledger.json");
    let ledger = Arc::new(RwLock::new(CapabilityLedger::load_or_create(ledger_path)));

    let targets = resolve_targets(&args);
    if targets.is_empty() {
        eprintln!("error: no targets to install (after --only / --skip filtering)");
        return 2;
    }

    for t in &targets {
        if find_spec(t).is_none() {
            eprintln!("error: unknown capability '{t}'");
            return 2;
        }
        if !runtimes::supported_on_current_os(t) {
            if args.best_effort {
                if args.json {
                    let _ = writeln!(
                        std::io::stderr(),
                        r#"{{"event":"step_skipped","capability":"{t}","reason":"unsupported platform"}}"#
                    );
                } else {
                    eprintln!("[skip] {t}: unsupported on current platform");
                }
                continue;
            }
            eprintln!("error: capability '{t}' not supported on current platform");
            return 3;
        }
    }

    let mut printer = ProgressPrinter::new(args.json, args.quiet);
    let mut ready_count: usize = 0;
    let mut failed_count: usize = 0;

    for (idx, cap) in targets.iter().enumerate() {
        if args.force {
            let mut g = ledger.write().await;
            // `mark_missing` clears bin_path/version in addition to flipping
            // status, so a subsequent refresh or probe doesn't carry stale
            // values into the next probe's view.
            g.mark_missing(cap);
        }
        printer.step_start(idx + 1, targets.len(), cap);
        match ensure_capability(cap, &ledger).await {
            Ok(path) => {
                let version = ledger
                    .read()
                    .await
                    .entries
                    .get(cap.as_str())
                    .map(|e| e.version.clone())
                    .unwrap_or_default();
                printer.step_done(cap, &path.display().to_string(), &version);
                ready_count += 1;
            }
            Err(e) => {
                printer.step_failed(cap, &e.to_string());
                failed_count += 1;
                if !args.best_effort {
                    break;
                }
            }
        }
    }

    let any_failed = failed_count > 0;
    printer.summary(targets.len(), ready_count, failed_count);

    i32::from(any_failed && !args.best_effort)
}

fn resolve_targets(args: &BootstrapRuntimeArgs) -> Vec<String> {
    let base: Vec<String> = if args.only.is_empty() {
        DEFAULT_TARGETS
            .iter()
            .map(std::string::ToString::to_string)
            .collect()
    } else {
        args.only.clone()
    };
    base.into_iter()
        .filter(|t| !args.skip.contains(t))
        .collect()
}

/// JSON-encode a string (quoted + escaped) for the NDJSON progress events.
/// Hand-rolled `replace()` escaping missed backslashes (e.g. Windows paths)
/// and control characters, producing invalid JSON for the install.sh /
/// install.ps1 consumers.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

struct ProgressPrinter {
    json: bool,
    quiet: bool,
}

impl ProgressPrinter {
    const fn new(json: bool, quiet: bool) -> Self {
        Self { json, quiet }
    }

    fn step_start(&mut self, idx: usize, total: usize, cap: &str) {
        if self.quiet {
            return;
        }
        if self.json {
            eprintln!(
                r#"{{"event":"step_start","capability":"{cap}","index":{idx},"total":{total}}}"#
            );
        } else {
            eprintln!("[{idx}/{total}] {cap} ...");
        }
    }

    fn step_done(&mut self, cap: &str, path: &str, version: &str) {
        if self.quiet {
            return;
        }
        if self.json {
            eprintln!(
                r#"{{"event":"step_done","capability":"{cap}","version":{},"path":{}}}"#,
                json_str(version),
                json_str(path)
            );
        } else {
            eprintln!("  ✓ {cap} {version} ({path})");
        }
    }

    fn step_failed(&mut self, cap: &str, err: &str) {
        if self.json {
            eprintln!(
                r#"{{"event":"step_failed","capability":"{cap}","error":{}}}"#,
                json_str(err)
            );
        } else {
            eprintln!("  ✗ {cap} failed:");
            for line in err.lines() {
                eprintln!("    {line}");
            }
        }
    }

    fn summary(&mut self, total: usize, ready: usize, failed: usize) {
        if self.json {
            // Report the actual success/failure counts. In `--best-effort`
            // mode several targets can fail in one run; reporting `failed` as a
            // 0/1 bool undercounted multi-failure runs and overcounted `ready`.
            eprintln!(r#"{{"event":"summary","ready":{ready},"failed":{failed},"total":{total}}}"#);
        } else if !self.quiet {
            eprintln!();
            if failed > 0 {
                eprintln!("Runtime bootstrap finished with errors. Re-run to retry.");
            } else {
                eprintln!("Runtime ready. Ledger: ~/.aleph/runtimes/ledger.json");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_targets_default() {
        let args = BootstrapRuntimeArgs::default();
        assert_eq!(resolve_targets(&args), vec!["uv", "playwright-cli"]);
    }

    #[test]
    fn test_resolve_targets_only_replaces_default() {
        let args = BootstrapRuntimeArgs {
            only: vec!["uv".into()],
            ..Default::default()
        };
        assert_eq!(resolve_targets(&args), vec!["uv"]);
    }

    #[test]
    fn test_resolve_targets_skip_filters_default() {
        let args = BootstrapRuntimeArgs {
            skip: vec!["uv".into()],
            ..Default::default()
        };
        assert_eq!(resolve_targets(&args), vec!["playwright-cli"]);
    }

    #[test]
    fn test_resolve_targets_skip_filters_only() {
        let args = BootstrapRuntimeArgs {
            only: vec!["uv".into(), "playwright-cli".into()],
            skip: vec!["playwright-cli".into()],
            ..Default::default()
        };
        assert_eq!(resolve_targets(&args), vec!["uv"]);
    }
}
