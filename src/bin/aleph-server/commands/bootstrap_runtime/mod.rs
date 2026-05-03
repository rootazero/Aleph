//! `aleph-server bootstrap-runtime` — install managed runtimes via ensure_capability.
//!
//! Spec C policy: **NoLock**. Bootstrap-runtime is invoked as a child
//! process by `start` while the parent already holds the singleton
//! lock; touching `~/.aleph/data/` is the parent's responsibility, so
//! this child does not contend. The marker `run_no_lock` call at
//! `run` entry preserves classification for the reverse-regression
//! check (Task 25).

use std::io::Write;
use std::sync::Arc;

use alephcore::runtimes::{
    self, ensure_capability, find_spec, probe, CapabilityLedger, CapabilityStatus,
};
use tokio::sync::RwLock;

use crate::cli::BootstrapRuntimeArgs;

/// Default target set when neither `--only` nor `--skip` is given.
const DEFAULT_TARGETS: &[&str] = &["uv", "playwright-cli"];

/// Detect-only runtimes — probed but never installed.
const DETECT_ONLY: &[&str] = &["git", "cargo"];

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
    if let Err(e) = std::fs::create_dir_all(&runtimes_dir) {
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
    let mut any_failed = false;

    for (idx, cap) in targets.iter().enumerate() {
        if args.force {
            let mut g = ledger.write().await;
            g.update_status(cap, CapabilityStatus::Missing);
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
            }
            Err(e) => {
                printer.step_failed(cap, &e.to_string());
                any_failed = true;
                if !args.best_effort {
                    break;
                }
            }
        }
    }

    printer.section("System runtimes (detect-only):");
    for cap in DETECT_ONLY {
        let r = probe::probe(cap);
        printer.detect(cap, r.found, r.version.as_deref());
    }

    printer.summary(&targets, any_failed);

    if any_failed && !args.best_effort {
        1
    } else {
        0
    }
}

fn resolve_targets(args: &BootstrapRuntimeArgs) -> Vec<String> {
    let base: Vec<String> = if args.only.is_empty() {
        DEFAULT_TARGETS.iter().map(|s| s.to_string()).collect()
    } else {
        args.only.clone()
    };
    base.into_iter()
        .filter(|t| !args.skip.contains(t))
        .collect()
}

struct ProgressPrinter {
    json: bool,
    quiet: bool,
}

impl ProgressPrinter {
    fn new(json: bool, quiet: bool) -> Self {
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
                r#"{{"event":"step_done","capability":"{cap}","version":"{version}","path":"{path}"}}"#
            );
        } else {
            eprintln!("  ✓ {cap} {version} ({path})");
        }
    }

    fn step_failed(&mut self, cap: &str, err: &str) {
        let err_escaped = err.replace('"', "\\\"").replace('\n', "\\n");
        if self.json {
            eprintln!(r#"{{"event":"step_failed","capability":"{cap}","error":"{err_escaped}"}}"#);
        } else {
            eprintln!("  ✗ {cap} failed:");
            for line in err.lines() {
                eprintln!("    {line}");
            }
        }
    }

    fn section(&mut self, title: &str) {
        if self.quiet || self.json {
            return;
        }
        eprintln!();
        eprintln!("{title}");
    }

    fn detect(&mut self, cap: &str, found: bool, version: Option<&str>) {
        if self.quiet {
            return;
        }
        if self.json {
            let v = version.unwrap_or("");
            eprintln!(
                r#"{{"event":"detect","capability":"{cap}","found":{found},"version":"{v}"}}"#
            );
        } else {
            let mark = if found { "✓" } else { "✗" };
            let v = version.unwrap_or("(not installed)");
            eprintln!("  {mark} {cap} {v}");
        }
    }

    fn summary(&mut self, targets: &[String], any_failed: bool) {
        if self.json {
            let ready = targets.len().saturating_sub(any_failed as usize);
            eprintln!(
                r#"{{"event":"summary","ready":{ready},"failed":{},"total":{}}}"#,
                any_failed as usize,
                targets.len(),
            );
        } else if !self.quiet {
            eprintln!();
            if any_failed {
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
