//! `aleph sandbox-debug` — smoke-test the active sandbox profile.
//!
//! Codex parity: `codex debug sandbox --seatbelt|--landlock|--windows ...`.
//! Aleph's CLI auto-selects the platform driver, so we expose a single
//! subcommand that runs an arbitrary command under the platform default
//! sandbox and prints stdout/stderr/exit-code/duration.
//!
//! The `--show-summary` mode prints the LLM-facing sandbox summary
//! (backend tag, policy tier, writable roots, network state) without
//! executing anything — useful for verifying `SecurityLayer` rendering
//! before paying tokens.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use alephcore::sandbox::capabilities::{NetworkPolicy, SandboxCapabilities};
use alephcore::sandbox::command::SandboxCommand;
use alephcore::sandbox::config::SandboxConfig;
use alephcore::sandbox::denial_logger::DenialLogger;
use alephcore::sandbox::exec_approval::gate::{
    ApprovalGate, ApprovalOutcome, ApprovalRequester, ApprovalResponse,
};

/// Auto-approve every capability elevation. Used only inside the
/// `sandbox-debug` CLI so the operator does not have to hit Enter for
/// each command they smoke-test. Never wired into the daemon.
struct DebugAutoApprover;

#[async_trait::async_trait]
impl ApprovalRequester for DebugAutoApprover {
    async fn request_approval(
        &self,
        _action: &alephcore::sandbox::exec_approval::ApprovalAction,
    ) -> ApprovalResponse {
        ApprovalOutcome::Approved.into()
    }
}
use alephcore::sandbox::factory::build_sandbox;
use alephcore::sandbox::platforms::create_platform_driver_from_config;
use alephcore::sandbox::rate_limit::SandboxRateLimitConfig;

/// CLI handler for `aleph sandbox-debug`. Constructs the same
/// `WorkspaceSandbox` that production boot uses, then either prints
/// its summary or executes a single command under it.
pub async fn handle_sandbox_debug(
    network: Option<String>,
    fs_write: Vec<PathBuf>,
    fs_read: Vec<PathBuf>,
    show_summary: bool,
    log_denials: bool,
    command: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let cfg = SandboxConfig::default();
    let driver = create_platform_driver_from_config(&cfg);
    // Debug mode auto-approves every capability elevation so the
    // operator does not get prompted for each smoke test. The real
    // daemon never wires this requester; only the debug CLI does.
    let gate = Arc::new(ApprovalGate::new(Some(
        Arc::new(DebugAutoApprover) as Arc<dyn ApprovalRequester>
    )));
    let sandbox = build_sandbox(
        &cfg,
        driver,
        gate,
        SandboxRateLimitConfig::default(),
        &alephcore::ShellSecurityConfig::default(),
    );

    // 1. Print summary.
    println!("=== Sandbox summary ===");
    match sandbox.summary() {
        Some(summary) => {
            for line in summary.to_prompt_lines() {
                println!("  {line}");
            }
        }
        None => println!("  (no summary — sandbox impl declined to introspect)"),
    }

    if show_summary || command.is_empty() {
        if !show_summary && command.is_empty() {
            eprintln!(
                "\nno command given (pass after `--`); use --show-summary to suppress this warning"
            );
        }
        return Ok(());
    }

    let network_policy = parse_network(network.as_deref())?;
    let capabilities = SandboxCapabilities {
        fs_read,
        fs_write,
        network: network_policy,
        spawn_subprocess: true,
        max_memory_mb: None,
        timeout_secs: None,
    };

    let denial = DenialLogger::new();
    if log_denials {
        if DenialLogger::supported() {
            match denial.start().await {
                Ok(()) => println!("\n[denial logger started]"),
                Err(e) => eprintln!("\n[denial logger failed: {e}]"),
            }
        } else {
            eprintln!("\n[denial logger: only supported on macOS; skipping]");
        }
    }

    // 2. Build + execute the command.
    let session_id = alephcore::routing::session_key::SessionKey::ephemeral("sandbox-debug");
    let (program, args) = if let Some(p) = split_program_args(&command) {
        p
    } else {
        eprintln!("Error: no command given (pass after `--`)");
        std::process::exit(2);
    };
    let cmd = SandboxCommand {
        session_id,
        // No tool asked — an operator typed this at a terminal. Naming the
        // subcommand keeps the audit trail honest about that (and keeps it out
        // of whichever rate-limit bucket `program` would have fallen into).
        tool_name: "sandbox_debug".to_string(),
        program: program.to_string(),
        args: args.to_vec(),
        env: HashMap::new(),
        stdin: None,
        cwd: None,
        capabilities,
        timeout: None,
    };
    println!("\n=== Executing: {program} {args:?} ===");
    let started = std::time::Instant::now();
    let result = sandbox.execute(cmd).await;
    let elapsed = started.elapsed();

    if log_denials {
        // Give the logger a moment to flush — `log stream` is line-buffered
        // but stderr-redirect happens after the child exits.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let lines = denial.take_lines().await;
        denial.stop().await;
        if DenialLogger::supported() {
            if lines.is_empty() {
                println!("\n=== Sandbox denials: none observed ===");
            } else {
                println!("\n=== Sandbox denials ({} lines) ===", lines.len());
                for line in lines.iter().take(50) {
                    println!("  {line}");
                }
                if lines.len() > 50 {
                    println!("  ... and {} more lines", lines.len() - 50);
                }
            }
        }
        // Unsupported platforms already warned at start; nothing to report.
    }

    match result {
        Ok(out) => {
            println!("\n=== Output ===");
            println!("exit_code: {:?}", out.exit_code);
            println!("signal: {:?}", out.signal);
            println!(
                "duration: {} ms (wall: {} ms)",
                out.duration_ms,
                elapsed.as_millis()
            );
            println!("truncated: {}", out.truncated);
            if !out.stdout.is_empty() {
                println!("--- stdout ---");
                print!("{}", String::from_utf8_lossy(&out.stdout));
                if !out.stdout.ends_with(b"\n") {
                    println!();
                }
            }
            if !out.stderr.is_empty() {
                println!("--- stderr ---");
                print!("{}", String::from_utf8_lossy(&out.stderr));
                if !out.stderr.ends_with(b"\n") {
                    println!();
                }
            }
            // A signal-killed child has `exit_code == None`; mapping that to 0
            // would report success for a SIGKILL/SIGSEGV. Use the POSIX
            // `128 + signal` convention so scripted callers see the failure.
            let code = out
                .exit_code
                .or_else(|| out.signal.map(|s| 128 + s))
                .unwrap_or(1);
            std::process::exit(code);
        }
        Err(e) => {
            eprintln!("\n=== Error ===\n{e}");
            std::process::exit(2);
        }
    }
}

fn parse_network(s: Option<&str>) -> Result<NetworkPolicy, String> {
    match s.map(str::to_ascii_lowercase) {
        None => Ok(NetworkPolicy::AllowAll),
        Some(v) if v == "none" => Ok(NetworkPolicy::None),
        Some(v) if v == "all" || v == "allow" => Ok(NetworkPolicy::AllowAll),
        Some(other) => Err(format!(
            "--network must be `none` or `all`, got `{other}` (AllowHosts not supported via CLI yet)"
        )),
    }
}

fn split_program_args(cmd: &[String]) -> Option<(&str, &[String])> {
    if cmd.is_empty() {
        return None;
    }
    Some((cmd[0].as_str(), &cmd[1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_network_defaults_to_allow_all() {
        assert!(matches!(
            parse_network(None).unwrap(),
            NetworkPolicy::AllowAll
        ));
    }

    #[test]
    fn parse_network_none() {
        assert!(matches!(
            parse_network(Some("none")).unwrap(),
            NetworkPolicy::None
        ));
        assert!(matches!(
            parse_network(Some("NONE")).unwrap(),
            NetworkPolicy::None
        ));
    }

    #[test]
    fn parse_network_all() {
        assert!(matches!(
            parse_network(Some("all")).unwrap(),
            NetworkPolicy::AllowAll
        ));
        assert!(matches!(
            parse_network(Some("allow")).unwrap(),
            NetworkPolicy::AllowAll
        ));
    }

    #[test]
    fn parse_network_rejects_garbage() {
        assert!(parse_network(Some("kaboom")).is_err());
    }

    #[test]
    fn split_program_args_splits_first_from_rest() {
        let cmd = vec!["ls".to_string(), "-la".to_string(), "/tmp".to_string()];
        let (prog, args) = split_program_args(&cmd).unwrap();
        assert_eq!(prog, "ls");
        assert_eq!(args, &["-la".to_string(), "/tmp".to_string()]);
    }

    #[test]
    fn split_program_args_returns_none_for_empty() {
        let cmd: Vec<String> = vec![];
        assert!(split_program_args(&cmd).is_none());
    }
}
