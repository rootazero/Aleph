//! `aleph doctor` — top-level installation / runtime health diagnostic.
//!
//! Inspired by `codex doctor`, but tailored to Aleph's layout. Diagnosis
//! itself is read-only, so the bare command is safe to run before filing a
//! support issue or while diagnosing a broken installation.
//!
//! Beyond the reference doctors (codex / openclaw / hermes all stop at
//! mechanical repair + human-readable hints), this command can hand the
//! failing checks to the daemon agent for an AI-assisted repair: press `f`
//! when prompted (or pass `--fix`). That path forwards a brief over
//! `agent.run`; all reasoning and any state mutation happen in Core, never
//! in this thin I/O shell (R4). After the agent finishes, the same check
//! battery re-runs in-process and reports the resolved/remaining delta —
//! the verification loop the reference doctors leave to a manual re-run.
//!
//! Two halves, one report:
//!
//! - **Client-side environment checks** (system / config / runtime / sandbox
//!   categories): binary locations, `~/.aleph` layout, config file parses,
//!   gateway reachable, client/daemon version match, MCP server count,
//!   sandbox profile summary.
//! - **Daemon engine battery** (`engine` / `core` / `providers` / …
//!   categories): when the gateway is reachable the CLI calls the
//!   `diagnostics.run` RPC and folds each returned finding into a row
//!   (category = `check_id` prefix, severity → required mapping). The checks
//!   themselves — core paths, vault, provider connectivity, … — live in
//!   Core's `DiagnosticEngine`; this shell never duplicates them (R4). A
//!   daemon that predates the method yields a single upgrade-hint info row.
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

use serde::{Deserialize, Serialize};
use serde_json::Value;

use aleph_client::{AlephClient, CliConfig, CliError, CliResult};
use aleph_protocol::jsonrpc::METHOD_NOT_FOUND;
use aleph_protocol::{AgentTraceEvent, StreamEvent};

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
///
/// `fix` (and the interactive `[f]` keypress) launch an AI-assisted repair:
/// the failing checks are folded into a brief and forwarded to the daemon
/// agent over `agent.run`, which fixes what it can via its own tools. This
/// is the capability the reference doctors (codex / openclaw / hermes) lack —
/// they stop at mechanical repair plus human-readable hints.
pub async fn run(server_url: &str, json: bool, fix: bool, config: &CliConfig) -> CliResult<()> {
    let checks = run_all_checks(server_url, config).await;

    // JSON / CI path: non-interactive and byte-identical to the original —
    // emit the report and gate the exit code. Repair is never offered here so
    // machine consumers stay deterministic.
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&checks).unwrap_or_default()
        );
        if checks.iter().any(|c| !c.passed && c.required) {
            std::process::exit(2);
        }
        return Ok(());
    }

    render_human(&checks);

    // Human path: collect the actionable problems (any failed check), then
    // decide whether to launch the AI-assisted repair.
    let failing: Vec<&DoctorCheck> = checks.iter().filter(|c| !c.passed).collect();
    if failing.is_empty() {
        return Ok(());
    }
    let required_failed = failing.iter().filter(|c| c.required).count();

    let launch = fix || prompt_for_repair(failing.len());
    if launch {
        let brief = build_repair_brief(&failing);
        let before_failing = failing.len();
        match launch_llm_repair(server_url, &brief, config).await {
            Ok(()) => {
                // Close the loop: re-run the same check battery so the user
                // sees exactly what the repair resolved — the reference
                // doctors stop at "re-run to verify"; we verify in-process.
                eprintln!();
                eprintln!("Verifying repairs…");
                let after = run_all_checks(server_url, config).await;
                render_verification(before_failing, &after);
                if after.iter().any(|c| !c.passed && c.required) {
                    std::process::exit(2);
                }
                return Ok(());
            }
            Err(e) => {
                eprintln!("AI-assisted repair could not run: {e}");
                // Fall through to the standard exit gate below.
            }
        }
    }

    if required_failed > 0 {
        std::process::exit(2);
    }
    Ok(())
}

/// Run the full diagnostic battery once. Shared by the initial report and
/// the post-repair verification pass so both observe identical checks.
async fn run_all_checks(server_url: &str, config: &CliConfig) -> Vec<DoctorCheck> {
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
    let gateway_check = check_gateway_reachable(server_url, config).await;
    let gateway_reachable = gateway_check.passed;
    checks.push(gateway_check);

    if gateway_reachable {
        checks.push(check_daemon_version(server_url, config).await);
        checks.extend(check_daemon_diagnostics(server_url, config).await);
        checks.push(check_mcp_servers(server_url, config).await);
    }

    // 4. Sandbox (independent of daemon — uses sibling binary directly)
    checks.push(check_sandbox_summary());
    checks
}

/// Render the post-repair verification delta: how many of the originally
/// failing checks are now green, and what still needs attention.
fn render_verification(before_failing: usize, after: &[DoctorCheck]) {
    let still_failing: Vec<&DoctorCheck> = after.iter().filter(|c| !c.passed).collect();
    if still_failing.is_empty() {
        println!("Verification: all {before_failing} problem(s) resolved.");
        return;
    }
    let resolved = before_failing.saturating_sub(still_failing.len());
    println!(
        "Verification: {resolved} of {before_failing} problem(s) resolved, {} remaining:",
        still_failing.len()
    );
    for c in &still_failing {
        let sev = if c.required { "ERROR" } else { "WARN" };
        println!("  - [{sev}] {}/{}: {}", c.category, c.name, c.message);
    }
    println!("Remaining issues may need manual action — see the agent's report above.");
}

// ── AI-assisted repair (the [f] path) ──────────────────────────────────────

/// Offer the interactive AI-repair affordance. Returns `true` only when the
/// session is fully interactive (both stdin and stdout are TTYs) and the user
/// answers with `f`/`F`. Non-interactive sessions (piped stdin, redirected
/// output, CI) always return `false`, preserving deterministic behaviour.
fn prompt_for_repair(failing: usize) -> bool {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return false;
    }
    print!(
        "\n{failing} problem(s) detected. Press [f] then Enter to launch \
         AI-assisted repair, or Enter to skip: "
    );
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim(), "f" | "F")
}

/// Fold the failing checks into an LLM repair brief. Pure string transform:
/// the intelligence lives in the prompt (R9) — the agent does all reasoning
/// and repair via its own tools. Host-testable.
fn build_repair_brief(failing: &[&DoctorCheck]) -> String {
    let mut out = String::new();
    out.push_str(
        "`aleph doctor` found problems with this Aleph installation. Diagnose \
         and repair what you can safely fix using your available tools (for \
         example `doctor` with fix=true for mechanical repairs, `self_config` \
         / `self_manage` for configuration, `vault_store` for API keys, and \
         filesystem tools). Do not touch anything that is already healthy. \
         After repairing, call the `doctor` tool once more (it also probes \
         live LLM provider connectivity) to verify your fixes. When done, \
         report concisely what you changed and what still needs the user.\n\n",
    );
    out.push_str("Failing checks:\n");
    for c in failing {
        let sev = if c.required { "ERROR" } else { "WARN" };
        out.push_str(&format!(
            "- [{sev}] {}/{}: {}\n",
            c.category, c.name, c.message
        ));
    }
    out
}

/// Forward the repair brief to the daemon agent over `agent.run` and stream
/// its work back. Mirrors the rendering of `aleph ask` (R4: pure I/O — the CLI
/// formats the detected problems into a request and renders the reply; all
/// reasoning and repair happen in Core).
async fn launch_llm_repair(server_url: &str, brief: &str, config: &CliConfig) -> CliResult<()> {
    use crate::output::exec_echo;

    eprintln!();
    eprintln!("Launching AI-assisted repair…");

    let (client, mut events) = AlephClient::connect(server_url, config).await?;

    let session_key = config
        .default_session
        .clone()
        .unwrap_or_else(|| "default".to_string());

    #[derive(Serialize)]
    struct RunParams {
        session_key: Option<String>,
        input: String,
    }
    let params = RunParams {
        session_key: Some(session_key),
        input: brief.to_string(),
    };
    let _: Value = client.call("agent.run", Some(params)).await?;

    let mut response_text = String::new();
    let mut agent_trace_seen = false;
    let mut run_error: Option<String> = None;
    let verbose = std::env::var("ALEPH_VERBOSE").is_ok();

    while let Some(event) = events.recv().await {
        match event {
            // Rich path: hierarchical agent-loop trace (tool args + duration).
            StreamEvent::AgentTrace { event, .. } => {
                agent_trace_seen = true;
                match event {
                    AgentTraceEvent::ToolCallStarted { call, .. } => {
                        eprintln!(
                            "{}",
                            exec_echo::render_tool_start(&call.tool_name, &call.input)
                        );
                    }
                    AgentTraceEvent::ToolCallCompleted { call, result, .. } => {
                        if let Some(line) = exec_echo::render_tool_end(
                            &call.tool_name,
                            &result,
                            call.duration_ms,
                            verbose,
                        ) {
                            eprintln!("{line}");
                        }
                    }
                    _ => {}
                }
            }
            // Fallback path: coarse tool events for minimal emitters.
            StreamEvent::ToolStart {
                tool_name, params, ..
            } => {
                if !agent_trace_seen {
                    eprintln!("{}", exec_echo::render_tool_start(&tool_name, &params));
                }
            }
            StreamEvent::ResponseChunk {
                content, is_final, ..
            } => {
                response_text.push_str(&content);
                if is_final {
                    break;
                }
            }
            StreamEvent::RunComplete { summary, .. } => {
                eprintln!();
                eprintln!("{}", exec_echo::render_summary_footer(&summary));
                break;
            }
            StreamEvent::RunError { error, .. } => {
                eprintln!("Repair run error: {error}");
                run_error = Some(error);
                break;
            }
            _ => {}
        }
    }

    if !response_text.is_empty() {
        println!("{}", crate::output::markdown::render(&response_text));
    }

    client.close().await?;
    if let Some(error) = run_error {
        return Err(CliError::Other(error));
    }
    Ok(())
}

// ── Rendering ────────────────────────────────────────────────────────────

fn render_human(checks: &[DoctorCheck]) {
    println!("Aleph Doctor (v{})", env!("ALEPH_VERSION"));
    println!();

    let mut current_category = "";
    for check in checks {
        if check.category != current_category {
            current_category = &check.category;
            println!("[{current_category}]");
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
        println!("All required checks passed. {warned} optional warning(s).");
    } else {
        println!("{failed} required check(s) failed, {warned} optional warning(s).");
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
            format!("{} (v{})", exe.display(), env!("ALEPH_VERSION")),
        ),
        Err(e) => DoctorCheck::fail(
            "system",
            "aleph-cli",
            "Path of the running aleph binary",
            true,
            format!("current_exe() failed: {e}"),
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

async fn check_gateway_reachable(server_url: &str, config: &CliConfig) -> DoctorCheck {
    match tokio::time::timeout(
        Duration::from_secs(5),
        AlephClient::connect(server_url, config),
    )
    .await
    {
        Ok(Ok((client, _events))) => {
            let outcome: Result<Value, _> = client.call("health", None::<()>).await;
            let _ = client.close().await;
            match outcome {
                Ok(_) => DoctorCheck::ok(
                    "runtime",
                    "gateway",
                    "Aleph Gateway daemon (JSON-RPC over WS)",
                    false,
                    format!("reachable at {server_url}"),
                ),
                Err(e) => DoctorCheck::fail(
                    "runtime",
                    "gateway",
                    "Aleph Gateway daemon (JSON-RPC over WS)",
                    false,
                    format!("connected to {server_url} but `health` failed: {e}"),
                ),
            }
        }
        Ok(Err(e)) => DoctorCheck::fail(
            "runtime",
            "gateway",
            "Aleph Gateway daemon (JSON-RPC over WS)",
            false,
            format!("cannot reach {server_url}: {e} (start with `aleph daemon start`)"),
        ),
        Err(_) => DoctorCheck::fail(
            "runtime",
            "gateway",
            "Aleph Gateway daemon (JSON-RPC over WS)",
            false,
            format!("timeout connecting to {server_url} (5s)"),
        ),
    }
}

/// Compare the CLI's own version with the version reported by the running
/// daemon. A mismatch is the classic "upgraded the binary but never restarted
/// the daemon" footgun (the supervisor keeps the old process alive).
///
/// Both sides MUST read the same authoritative source for the comparison to be
/// meaningful: the daemon's `gateway.identity.get` reports `env!("ALEPH_VERSION")`
/// (injected by `build.rs` from the `VERSION` file — the `CalVer` single source of
/// truth), so the CLI compares against the same `ALEPH_VERSION` rather than
/// `CARGO_PKG_VERSION`. The two coincide after `just release` (which mirrors
/// VERSION into Cargo.toml) but diverge whenever VERSION is bumped ahead of the
/// manifest — using `CARGO_PKG_VERSION` there would compare different sources and
/// fire a spurious "restart the daemon" warning against an identical build.
async fn check_daemon_version(server_url: &str, config: &CliConfig) -> DoctorCheck {
    let cli = env!("ALEPH_VERSION");
    match call_rpc(server_url, config, "gateway.identity.get").await {
        Ok(value) => {
            let daemon = value.get("version").and_then(|v| v.as_str()).unwrap_or("");
            if daemon.is_empty() {
                DoctorCheck::ok(
                    "runtime",
                    "version",
                    "Client/daemon version match",
                    false,
                    format!("CLI v{cli}; daemon did not report a version"),
                )
            } else if daemon == cli {
                DoctorCheck::ok(
                    "runtime",
                    "version",
                    "Client/daemon version match",
                    false,
                    format!("CLI and daemon both v{cli}"),
                )
            } else {
                DoctorCheck::fail(
                    "runtime",
                    "version",
                    "Client/daemon version match",
                    false,
                    format!(
                        "CLI v{cli} but the running daemon is v{daemon} — restart it to pick up the upgrade (`aleph daemon restart`)"
                    ),
                )
            }
        }
        // Older daemons may not expose identity.get — treat as informational
        // rather than a failure so doctor stays useful against any version.
        Err(_) => DoctorCheck::ok(
            "runtime",
            "version",
            "Client/daemon version match",
            false,
            format!("CLI v{cli}; daemon version unavailable"),
        ),
    }
}

/// Run the daemon's unified diagnostic engine over `diagnostics.run` and fold
/// the returned findings into doctor rows. This replaces the old client-side
/// provider/vault battery: the checks (core paths, vault, provider
/// connectivity, …) live in Core's `DiagnosticEngine` — one source of truth,
/// and the CLI stays a pure I/O shell (R4).
///
/// Backward compatible: a daemon that predates the method yields a single
/// info row advising an upgrade; the deleted battery is never resurrected.
async fn check_daemon_diagnostics(server_url: &str, config: &CliConfig) -> Vec<DoctorCheck> {
    match call_rpc(server_url, config, "diagnostics.run").await {
        Ok(report) => report_to_checks(&report),
        Err(e) => vec![diagnostics_error_row(&e)],
    }
}

/// One-row outcome for a failed `diagnostics.run` call: old daemons get an
/// upgrade hint (method-not-found), everything else is a non-fatal failure.
fn diagnostics_error_row(err: &CliError) -> DoctorCheck {
    match err {
        CliError::Rpc { code, message }
            if *code == METHOD_NOT_FOUND
                && message
                    .strip_prefix("Method not found:")
                    .is_some_and(|method| method.trim() == "diagnostics.run") =>
        {
            DoctorCheck::ok(
                "engine",
                "diagnostics",
                "Daemon diagnostic engine",
                false,
                "daemon predates `diagnostics.run` — upgrade/restart the daemon \
                 for the full diagnostic battery",
            )
        }
        _ => DoctorCheck::fail(
            "engine",
            "diagnostics",
            "Daemon diagnostic engine",
            false,
            format!("diagnostics.run failed: {err}"),
        ),
    }
}

/// Wire shape of one finding in the `diagnostics.run` report envelope.
/// Mirrors Core's `diagnostics::Finding` (serde, lowercase severity) without
/// depending on alephcore — the CLI crate is protocol-only by design.
#[derive(Debug, Deserialize)]
struct FindingRow {
    check_id: String,
    severity: String,
    title: String,
    detail: String,
    fix_hint: Option<String>,
    #[serde(default)]
    repairable: bool,
}

/// Fold a `diagnostics.run` report envelope into doctor rows: one row per
/// problem finding (info findings are noise — every healthy check emits one);
/// a single summary row when the whole battery is green.
fn report_to_checks(report: &Value) -> Vec<DoctorCheck> {
    let findings: Vec<FindingRow> = report
        .get("findings")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let problems: Vec<DoctorCheck> = findings
        .iter()
        .filter(|f| f.severity != "info")
        .map(finding_row_to_check)
        .collect();
    if !problems.is_empty() {
        return problems;
    }
    let checks_run = report.get("checksRun").and_then(Value::as_u64).unwrap_or(0);
    vec![DoctorCheck::ok(
        "engine",
        "diagnostics",
        "Daemon diagnostic engine",
        false,
        format!("{checks_run} checks passed"),
    )]
}

/// Map one daemon finding onto a doctor check. Pure transform, host-testable.
/// Severity → verdict: `error` fails a required check (exit code 2),
/// `warning` fails a non-required one, `info` passes; an unknown severity
/// fails safe as a warning.
fn finding_row_to_check(row: &FindingRow) -> DoctorCheck {
    let category = row.check_id.split('/').next().unwrap_or("engine");
    let (passed, required) = match row.severity.as_str() {
        "info" => (true, false),
        "error" => (false, true),
        _ => (false, false),
    };
    let mut message = row.detail.clone();
    if let Some(hint) = &row.fix_hint {
        message.push_str(&format!(" — fix: {hint}"));
    }
    if row.repairable {
        message.push_str(" (mechanically repairable)");
    }
    if passed {
        DoctorCheck::ok(category, &row.check_id, &row.title, required, message)
    } else {
        DoctorCheck::fail(category, &row.check_id, &row.title, required, message)
    }
}

async fn check_mcp_servers(server_url: &str, config: &CliConfig) -> DoctorCheck {
    // Best-effort: many deployments don't enable MCP. Failure here is
    // surfaced as a warning, not a hard error.
    match call_rpc(server_url, config, "mcp.list").await {
        Ok(value) => {
            let count = count_array_or_obj_array(&value, "servers");
            DoctorCheck::ok(
                "runtime",
                "mcp",
                "Connected MCP servers",
                false,
                format!("{count} configured"),
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

/// Aleph's state root, honouring `ALEPH_HOME`.
///
/// `interfaces/cli` may not depend on `alephcore`, so it cannot call
/// `utils::paths::get_config_dir`. It can and must read the same environment
/// variable: a hand-rolled `dirs::home_dir().join(".aleph")` reports on the
/// wrong tree in every `ALEPH_HOME`-redirected deployment — which is every QA
/// run, and any operator running more than one Aleph state directory.
pub(crate) fn aleph_home() -> PathBuf {
    if let Some(home) = std::env::var_os("ALEPH_HOME") {
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".aleph")
}

async fn call_rpc(server_url: &str, config: &CliConfig, method: &str) -> CliResult<Value> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;
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
        .map_or(0, std::vec::Vec::len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finding_info_maps_to_passed_row() {
        let row: FindingRow = serde_json::from_value(json!({
            "check_id": "core/data-dir",
            "severity": "info",
            "title": "Data directory",
            "detail": "/home/u/.aleph present",
            "fix_hint": null,
            "repairable": false
        }))
        .unwrap();
        let check = finding_row_to_check(&row);
        assert!(check.passed);
        assert!(!check.required);
        assert_eq!(check.category, "core");
        assert_eq!(check.name, "core/data-dir");
        assert_eq!(check.description, "Data directory");
        assert_eq!(check.message, "/home/u/.aleph present");
    }

    #[test]
    fn finding_warning_fails_non_required() {
        let row: FindingRow = serde_json::from_value(json!({
            "check_id": "core/stale-lock",
            "severity": "warning",
            "title": "Stale lock",
            "detail": "gateway.lock is 3 days old",
            "fix_hint": "delete the stale lock file",
            "repairable": true
        }))
        .unwrap();
        let check = finding_row_to_check(&row);
        assert!(!check.passed);
        assert!(!check.required, "warnings are not fatal");
        assert!(check.message.contains("gateway.lock is 3 days old"));
        assert!(check.message.contains("fix: delete the stale lock file"));
        assert!(check.message.contains("(mechanically repairable)"));
    }

    #[test]
    fn finding_error_fails_required() {
        let row: FindingRow = serde_json::from_value(json!({
            "check_id": "core/config-parse",
            "severity": "error",
            "title": "Config parse",
            "detail": "TOML parse error at line 12",
            "fix_hint": null,
            "repairable": false
        }))
        .unwrap();
        let check = finding_row_to_check(&row);
        assert!(!check.passed);
        assert!(check.required, "errors must gate the exit code");
    }

    #[test]
    fn finding_unknown_severity_fails_safe_as_warning() {
        let row: FindingRow = serde_json::from_value(json!({
            "check_id": "core/x",
            "severity": "critical",
            "title": "t",
            "detail": "d",
            "fix_hint": null,
            "repairable": false
        }))
        .unwrap();
        let check = finding_row_to_check(&row);
        assert!(!check.passed);
        assert!(!check.required);
    }

    #[test]
    fn report_with_problems_emits_only_problem_rows() {
        let report = json!({
            "ok": false,
            "checksRun": 10,
            "findings": [
                { "check_id": "core/data-dir", "severity": "info", "title": "t",
                  "detail": "fine", "repairable": false },
                { "check_id": "providers/connectivity", "severity": "warning", "title": "t",
                  "detail": "openai unreachable", "fix_hint": "store a fresh key",
                  "repairable": false }
            ]
        });
        let checks = report_to_checks(&report);
        assert_eq!(checks.len(), 1, "info findings are not emitted as rows");
        assert_eq!(checks[0].category, "providers");
        assert!(!checks[0].passed);
    }

    #[test]
    fn report_all_green_emits_summary_row() {
        let report = json!({
            "ok": true,
            "checksRun": 10,
            "findings": [
                { "check_id": "core/data-dir", "severity": "info", "title": "t",
                  "detail": "fine", "repairable": false }
            ]
        });
        let checks = report_to_checks(&report);
        assert_eq!(checks.len(), 1);
        assert!(checks[0].passed);
        assert_eq!(checks[0].name, "diagnostics");
        assert_eq!(checks[0].message, "10 checks passed");
    }

    #[test]
    fn method_not_found_yields_upgrade_hint_row() {
        let err = CliError::Rpc {
            code: METHOD_NOT_FOUND,
            message: "Method not found: diagnostics.run".to_string(),
        };
        let check = diagnostics_error_row(&err);
        assert!(check.passed, "old daemon is an info row, not a failure");
        assert!(check.message.contains("upgrade"));

        let unrelated = CliError::Rpc {
            code: METHOD_NOT_FOUND,
            message: "Method not found: providers.healthcheck".to_string(),
        };
        let check = diagnostics_error_row(&unrelated);
        assert!(!check.passed);
        assert!(check.message.contains("diagnostics.run failed"));

        let other = CliError::Connection("refused".to_string());
        let check = diagnostics_error_row(&other);
        assert!(!check.passed);
        assert!(!check.required);
        assert!(check.message.contains("diagnostics.run failed"));
    }

    #[test]
    fn repair_brief_lists_failing_checks_with_severity() {
        let required =
            DoctorCheck::fail("config", "config.toml", "Aleph config", true, "parse error");
        let optional = DoctorCheck::fail(
            "runtime",
            "vault",
            "Secret vault",
            false,
            "secrets.list failed",
        );
        let failing = vec![&required, &optional];

        let brief = build_repair_brief(&failing);

        // The brief instructs the agent to use its own repair tools (R9)
        // and to close the loop by re-running doctor afterwards.
        assert!(brief.contains("doctor"));
        assert!(brief.contains("self_config"));
        assert!(brief.contains("vault_store"));
        assert!(brief.contains("verify your fixes"));
        // Required failures are ERROR; optional are WARN.
        assert!(brief.contains("[ERROR] config/config.toml: parse error"));
        assert!(brief.contains("[WARN] runtime/vault: secrets.list failed"));
    }

    #[test]
    fn repair_brief_renders_each_failing_check() {
        let a = DoctorCheck::fail("system", "aleph-home", "home", true, "missing");
        let failing = vec![&a];
        let brief = build_repair_brief(&failing);
        assert_eq!(brief.matches("- [").count(), 1);
        assert!(brief.contains("aleph-home"));
    }
}
