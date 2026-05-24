//! `aleph sandbox` — thin client-side wrapper around
//! `aleph-server sandbox-debug`.
//!
//! Aleph already has a rich sandbox subsystem (`src/sandbox/`) wired into the
//! server-side `aleph-server sandbox-debug` subcommand, but that surface is
//! awkward for users: it lives on a daemon binary that they normally don't
//! invoke directly. This module exposes a codex-parity `aleph sandbox`
//! surface that simply forwards to the existing server-side implementation,
//! so we get a single canonical command name without re-implementing the
//! sandbox runtime in the CLI binary.
//!
//! Two subcommands:
//! - `aleph sandbox info` — print the active sandbox summary
//! - `aleph sandbox run [flags] -- <command>` — run a command under the
//!   active sandbox profile
//!
//! Forwarded flags mirror `aleph-server sandbox-debug`:
//!   `--network`, `--fs-write`, `--fs-read`, `--log-denials`.

use std::process::Command;

use aleph_client::{CliError, CliResult};

use super::daemon::find_server_binary;

/// Forward to `aleph-server sandbox-debug --show-summary`.
pub fn info() -> CliResult<()> {
    let binary = find_server_binary();
    let status = Command::new(&binary)
        .arg("sandbox-debug")
        .arg("--show-summary")
        .status()
        .map_err(|e| {
            CliError::Other(format!(
                "failed to invoke {}: {}",
                binary.display(),
                e
            ))
        })?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Forward to `aleph-server sandbox-debug [flags] -- <command>`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    network: Option<&str>,
    fs_write: &[String],
    fs_read: &[String],
    log_denials: bool,
    command: &[String],
) -> CliResult<()> {
    if command.is_empty() {
        return Err(CliError::Other(
            "aleph sandbox run: missing command to execute (use `--` to separate flags)".into(),
        ));
    }

    let binary = find_server_binary();
    let mut cmd = Command::new(&binary);
    cmd.arg("sandbox-debug");

    if let Some(policy) = network {
        cmd.arg("--network").arg(policy);
    }
    for path in fs_write {
        cmd.arg("--fs-write").arg(path);
    }
    for path in fs_read {
        cmd.arg("--fs-read").arg(path);
    }
    if log_denials {
        cmd.arg("--log-denials");
    }
    cmd.arg("--");
    for arg in command {
        cmd.arg(arg);
    }

    let status = cmd.status().map_err(|e| {
        CliError::Other(format!(
            "failed to invoke {}: {}",
            binary.display(),
            e
        ))
    })?;

    // Propagate the inner exit code so shell pipelines can react.
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}
