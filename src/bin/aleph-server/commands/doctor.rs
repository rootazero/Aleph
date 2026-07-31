//! `aleph doctor` — unified self-diagnosis + mechanical repair.
//!
//! Thin I/O shell over [`alephcore::diagnostics::DiagnosticEngine`]: build
//! the registry, run the chosen posture, render, and map "unresolved
//! problems" onto a non-zero exit code so CI can gate on it.

use alephcore::diagnostics::{DiagnosticEngine, Posture};

type CmdResult = Result<(), Box<dyn std::error::Error>>;

pub async fn handle_doctor_command(
    fix: bool,
    json: bool,
    only: Vec<String>,
    skip: Vec<String>,
) -> CmdResult {
    let engine = DiagnosticEngine::default_registry()?;
    let posture = if fix {
        Posture::Fix
    } else if json {
        Posture::Lint
    } else {
        Posture::Inspect
    };

    let only_ref = if only.is_empty() {
        None
    } else {
        Some(only.as_slice())
    };
    let report = engine.run_with_filter(posture, only_ref, &skip).await;

    if json {
        println!("{}", report.to_json());
    } else {
        print!("{}", report.render_human());
    }

    // Exit non-zero when problems remain unresolved (CI / preflight gate).
    if !report.ok() {
        std::process::exit(1);
    }
    Ok(())
}
