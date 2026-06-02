//! `aleph doctor` — top-level installation / runtime health diagnostic.
//!
//! Inspired by `codex doctor`, but tailored to Aleph's layout: this command
//! is intentionally read-only and never mutates user state, so it is safe
//! to run before filing a support issue or while diagnosing a broken
//! installation.
//!
//! Checks are grouped into four categories:
//!
//! - **system**:   binary location, version, sibling `aleph-server` presence
//! - **config**:   `~/.aleph/config.toml` exists + parses
//! - **runtime**:  Gateway daemon reachable, providers/MCP servers OK,
//!   vault present
//! - **sandbox**:  active sandbox profile can be summarised
//!
//! The plugin-specific `aleph plugin doctor` checks remain separate; this
//! command focuses on the *host* installation rather than the
//! plugin-developer toolchain.
//!
//! Output: human-readable by default, or `--json` for machine consumption.
//! Each check carries `name`, `category`, `passed`, `required`, `message`.
//! `passed=false && required=true` returns a non-zero exit code so CI/cron
//! harnesses can react.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use aleph_client::{AlephClient, CliResult};

use super::daemon::find_server_binary;

/// One diagnostic check result. Shape is shared between the human and
/// machine output paths so the JSON view is just a serialisation of the
/// same struct list.
#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub category: String,
    pub name: String,
    pub description: String,
    pub passed: bool,
    pub required: bool,
    pub message: String,
}

impl DoctorCheck {
    fn ok(
        category: &str,
        name: &str,
        description: &str,
        required: bool,
        message: impl Into<String>,
    ) -> Self {
        Self {
            category: category.into(),
            name: name.into(),
            description: description.into(),
            passed: true,
            required,
            message: message.into(),
        }
    }

    fn fail(
        category: &str,
        name: &str,
        description: &str,
        required: bool,
        message: impl Into<String>,
    ) -> Self {
        Self {
            category: category.into(),
            name: name.into(),
            description: description.into(),
            passed: false,
            required,
            message: message.into(),
        }
    }
}

/// Top-level entry. `server_url` is forwarded from the global `--server` flag.
pub async fn run(server_url: &str, json: bool) -> CliResult<()> {
    let mut checks: Vec<DoctorCheck> = vec![
        // 1. System
        check_cli_binary(),
        check_server_binary(),
        check_aleph_home(),
        // 2. Config
        check_config_file(),
        check_logs_dir(),
    ];

    // 3. Runtime (only meaningful if the daemon is reachable)
    let gateway_check = check_gateway_reachable(server_url).await;
    let gateway_reachable = gateway_check.passed;
    checks.push(gateway_check);

    if gateway_reachable {
        checks.push(check_providers(server_url).await);
        checks.push(check_mcp_servers(server_url).await);
        checks.push(check_vault(server_url).await);
    }

    // 4. Sandbox (independent of daemon — uses sibling binary directly)
    checks.push(check_sandbox_summary());

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&checks).unwrap_or_default()
        );
    } else {
        render_human(&checks);
    }

    let required_failed = checks.iter().filter(|c| !c.passed && c.required).count();
    if required_failed > 0 {
        std::process::exit(2);
    }
    Ok(())
}

// ── Rendering ────────────────────────────────────────────────────────────

fn render_human(checks: &[DoctorCheck]) {
    println!("Aleph Doctor (v{})", env!("CARGO_PKG_VERSION"));
    println!();

    let mut current_category = "";
    for check in checks {
        if check.category != current_category {
            current_category = &check.category;
            println!("[{}]", current_category);
        }
        let status = if check.passed {
            "OK"
        } else if check.required {
            "FAIL"
        } else {
            "WARN"
        };
        let icon = if check.passed { "+" } else { "-" };
        println!(
            "  [{}] {:<22} {:<6} — {}",
            icon, check.name, status, check.description
        );
        if !check.passed || !check.message.is_empty() {
            println!("       {}", check.message);
        }
    }

    println!();
    let failed = checks.iter().filter(|c| !c.passed && c.required).count();
    let warned = checks.iter().filter(|c| !c.passed && !c.required).count();
    if failed == 0 && warned == 0 {
        println!("All checks passed.");
    } else if failed == 0 {
        println!(
            "All required checks passed. {} optional warning(s).",
            warned
        );
    } else {
        println!(
            "{} required check(s) failed, {} optional warning(s).",
            failed, warned
        );
    }
}

// ── Checks: system ───────────────────────────────────────────────────────

fn check_cli_binary() -> DoctorCheck {
    match std::env::current_exe() {
        Ok(exe) => DoctorCheck::ok(
            "system",
            "aleph-cli",
            "Path of the running aleph binary",
            true,
            format!("{} (v{})", exe.display(), env!("CARGO_PKG_VERSION")),
        ),
        Err(e) => DoctorCheck::fail(
            "system",
            "aleph-cli",
            "Path of the running aleph binary",
            true,
            format!("current_exe() failed: {}", e),
        ),
    }
}

fn check_server_binary() -> DoctorCheck {
    let binary = find_server_binary();
    if binary.is_absolute() && binary.exists() {
        DoctorCheck::ok(
            "system",
            "aleph-server",
            "Daemon binary that backs the Gateway",
            false,
            binary.display().to_string(),
        )
    } else if !binary.is_absolute() {
        // PATH-resolved bare name — accept if it resolves via PATH probe.
        match std::process::Command::new(&binary)
            .arg("--version")
            .output()
        {
            Ok(o) if o.status.success() => DoctorCheck::ok(
                "system",
                "aleph-server",
                "Daemon binary that backs the Gateway",
                false,
                format!("{} (PATH-resolved)", binary.display()),
            ),
            _ => DoctorCheck::fail(
                "system",
                "aleph-server",
                "Daemon binary that backs the Gateway",
                false,
                format!(
                    "{} not found on PATH; set ALEPH_SERVER_BIN to override",
                    binary.display()
                ),
            ),
        }
    } else {
        DoctorCheck::fail(
            "system",
            "aleph-server",
            "Daemon binary that backs the Gateway",
            false,
            format!("{} does not exist", binary.display()),
        )
    }
}

fn check_aleph_home() -> DoctorCheck {
    let home = aleph_home();
    if home.exists() {
        DoctorCheck::ok(
            "system",
            "aleph-home",
            "~/.aleph data directory",
            true,
            home.display().to_string(),
        )
    } else {
        DoctorCheck::fail(
            "system",
            "aleph-home",
            "~/.aleph data directory",
            true,
            format!(
                "{} missing — run `aleph daemon start` once to bootstrap",
                home.display()
            ),
        )
    }
}

// ── Checks: config ───────────────────────────────────────────────────────

fn check_config_file() -> DoctorCheck {
    let path = aleph_home().join("config.toml");
    if !path.exists() {
        return DoctorCheck::fail(
            "config",
            "config.toml",
            "Aleph configuration file",
            false,
            format!("{} not present (defaults will be used)", path.display()),
        );
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<toml::Value>(&content) {
            Ok(_) => DoctorCheck::ok(
                "config",
                "config.toml",
                "Aleph configuration file",
                false,
                format!("{} parses", path.display()),
            ),
            Err(e) => DoctorCheck::fail(
                "config",
                "config.toml",
                "Aleph configuration file",
                true,
                format!("TOML parse error in {}: {}", path.display(), e),
            ),
        },
        Err(e) => DoctorCheck::fail(
            "config",
            "config.toml",
            "Aleph configuration file",
            true,
            format!("read failure on {}: {}", path.display(), e),
        ),
    }
}

fn check_logs_dir() -> DoctorCheck {
    let path = aleph_home().join("logs");
    if path.exists() && path.is_dir() {
        DoctorCheck::ok(
            "config",
            "logs",
            "Component log directory",
            false,
            path.display().to_string(),
        )
    } else {
        DoctorCheck::fail(
            "config",
            "logs",
            "Component log directory",
            false,
            format!("{} missing (created on first daemon start)", path.display()),
        )
    }
}

// ── Checks: runtime ──────────────────────────────────────────────────────

async fn check_gateway_reachable(server_url: &str) -> DoctorCheck {
    match tokio::time::timeout(Duration::from_secs(5), AlephClient::connect(server_url)).await {
        Ok(Ok((client, _events))) => {
            let outcome: Result<Value, _> = client.call("health", None::<()>).await;
            let _ = client.close().await;
            match outcome {
                Ok(_) => DoctorCheck::ok(
                    "runtime",
                    "gateway",
                    "Aleph Gateway daemon (JSON-RPC over WS)",
                    false,
                    format!("reachable at {}", server_url),
                ),
                Err(e) => DoctorCheck::fail(
                    "runtime",
                    "gateway",
                    "Aleph Gateway daemon (JSON-RPC over WS)",
                    false,
                    format!("connected to {} but `health` failed: {}", server_url, e),
                ),
            }
        }
        Ok(Err(e)) => DoctorCheck::fail(
            "runtime",
            "gateway",
            "Aleph Gateway daemon (JSON-RPC over WS)",
            false,
            format!(
                "cannot reach {}: {} (start with `aleph daemon start`)",
                server_url, e
            ),
        ),
        Err(_) => DoctorCheck::fail(
            "runtime",
            "gateway",
            "Aleph Gateway daemon (JSON-RPC over WS)",
            false,
            format!("timeout connecting to {} (5s)", server_url),
        ),
    }
}

async fn check_providers(server_url: &str) -> DoctorCheck {
    match call_rpc(server_url, "providers.list").await {
        Ok(value) => {
            let count = count_array_or_obj_array(&value, "providers");
            DoctorCheck::ok(
                "runtime",
                "providers",
                "Configured LLM providers",
                false,
                format!("{} configured", count),
            )
        }
        Err(e) => DoctorCheck::fail(
            "runtime",
            "providers",
            "Configured LLM providers",
            false,
            format!("providers.list failed: {}", e),
        ),
    }
}

async fn check_mcp_servers(server_url: &str) -> DoctorCheck {
    // Best-effort: many deployments don't enable MCP. Failure here is
    // surfaced as a warning, not a hard error.
    match call_rpc(server_url, "mcp.list").await {
        Ok(value) => {
            let count = count_array_or_obj_array(&value, "servers");
            DoctorCheck::ok(
                "runtime",
                "mcp",
                "Connected MCP servers",
                false,
                format!("{} configured", count),
            )
        }
        Err(_) => DoctorCheck::ok(
            "runtime",
            "mcp",
            "Connected MCP servers",
            false,
            "no `mcp.list` endpoint (MCP not enabled — OK)",
        ),
    }
}

async fn check_vault(server_url: &str) -> DoctorCheck {
    match call_rpc(server_url, "vault.status").await {
        Ok(value) => {
            let summary = value
                .get("status")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("state").and_then(|v| v.as_str()))
                .unwrap_or("ok");
            DoctorCheck::ok(
                "runtime",
                "vault",
                "Secret vault status",
                false,
                summary.to_string(),
            )
        }
        Err(e) => DoctorCheck::fail(
            "runtime",
            "vault",
            "Secret vault status",
            false,
            format!("vault.status failed: {}", e),
        ),
    }
}

// ── Checks: sandbox ──────────────────────────────────────────────────────

fn check_sandbox_summary() -> DoctorCheck {
    let binary = find_server_binary();
    let output = std::process::Command::new(&binary)
        .arg("sandbox-debug")
        .arg("--show-summary")
        .output();
    match output {
        Ok(o) if o.status.success() => {
            // Squash multi-line output to a single line for the row.
            let first_line = String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or("ok")
                .to_string();
            DoctorCheck::ok(
                "sandbox",
                "profile",
                "Active sandbox summary (aleph-server sandbox-debug)",
                false,
                first_line,
            )
        }
        Ok(o) => DoctorCheck::fail(
            "sandbox",
            "profile",
            "Active sandbox summary (aleph-server sandbox-debug)",
            false,
            format!(
                "exit={} stderr={}",
                o.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&o.stderr).trim()
            ),
        ),
        Err(e) => DoctorCheck::fail(
            "sandbox",
            "profile",
            "Active sandbox summary (aleph-server sandbox-debug)",
            false,
            format!("failed to spawn {}: {}", binary.display(), e),
        ),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn aleph_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".aleph")
}

async fn call_rpc(server_url: &str, method: &str) -> CliResult<Value> {
    let (client, _events) = AlephClient::connect(server_url).await?;
    let result: Value = client.call(method, None::<()>).await?;
    let _ = client.close().await;
    Ok(result)
}

/// Count items in either a top-level JSON array `[...]` or an object
/// containing a named array field `{ "<key>": [...] }`. Returns 0 for any
/// other shape so the check still reports cleanly.
fn count_array_or_obj_array(value: &Value, key: &str) -> usize {
    if let Some(arr) = value.as_array() {
        return arr.len();
    }
    value
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0)
}
