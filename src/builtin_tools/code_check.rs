//! `code_check` — run the project's type-checker / linter and return structured
//! diagnostics.
//!
//! # Why this exists
//!
//! opencode's signature coding optimisation is its *post-edit diagnostics loop*:
//! after every edit it runs a language server and feeds the resulting errors
//! back to the model so it can self-correct. Aleph had **no** equivalent — the
//! `file_edit` tool returns only success/count, and there is no LSP, tree-sitter
//! or typecheck-after-edit anywhere in core.
//!
//! A full LSP client (persistent language servers + JSON-RPC) would violate R1
//! (brain–limb separation) and R3 (core minimalism). Instead we map the *value*
//! of that loop onto Aleph's existing infrastructure:
//!
//! - **R8 (everything is a tool):** diagnostics are exposed as a tool the model
//!   invokes when it wants verification.
//! - **R7 / R10 (LLM sovereignty / thin harness):** the model decides *when* to
//!   check — there is no automatic middleware in the edit path. Parsing the
//!   compiler's machine-generated output is mechanical, not reasoning (P8).
//! - **Connect-first:** execution reuses the same `Arc<dyn Sandbox>` seam that
//!   `code_exec` / `bash` already route through — capability approval, the
//!   per-session workspace cwd, the seatbelt/bwrap profile, and the audit ledger
//!   all come for free. This tool is a thin adapter that picks the right command
//!   and normalises the output.
//!
//! # Mechanism
//!
//! Toolchain detection runs *inside* the sandboxed shell (so it sees the session
//! workspace, not the host CWD): a small `if`-cascade probes for `Cargo.toml`,
//! `tsconfig.json`, `go.mod`, or `pyproject.toml`/`ruff.toml`, prints an
//! `ALEPH_TC=<kind>` marker, then runs the matching checker. The marker tells the
//! host-side parser which format to expect — `cargo --message-format=json`
//! (parsed via `serde_json`) or a generic `file:line:col` scan (tsc / mypy / go /
//! clang style).

use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::Result;
use crate::sandbox::capabilities::{NetworkPolicy, SandboxCapabilities};
use crate::sandbox::command::{SandboxCommand, SandboxError, SandboxOutput};
use crate::sandbox::{current_session, Sandbox};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Default per-check timeout. Type-checkers can be slow (cargo check on a cold
/// crate), so this is higher than `code_exec`'s default; still capped by the
/// sandbox tool budget.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Hard cap on diagnostics returned to the model — keeps the tool result bounded
/// even when a single edit cascades into hundreds of downstream errors.
const MAX_DIAGNOSTICS: usize = 50;

/// Bytes of raw combined output preserved as a fallback for the model when the
/// structured parse misses something (unusual checker formats, build-script
/// panics, etc.).
const RAW_TAIL_BYTES: usize = 4000;

// =============================================================================
// Args & Output
// =============================================================================

/// Arguments for the `code_check` tool.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct CodeCheckArgs {
    /// File or directory hint (relative to the session workspace). Only used to
    /// bias detection by extension; the checker still runs at the workspace
    /// root. Defaults to the workspace root.
    #[serde(default)]
    pub path: Option<String>,
    /// Explicit check command, overriding auto-detection — e.g.
    /// `"cargo clippy --message-format=json"` or `"npx eslint ."`. Runs via the
    /// sandbox shell at the workspace root.
    #[serde(default)]
    pub command: Option<String>,
    /// Timeout in seconds (default 120). Accepts the legacy `timeout`
    /// spelling.
    #[serde(default, alias = "timeout")]
    pub timeout_seconds: Option<u64>,
}

/// A single normalised diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    /// File path as reported by the checker (workspace-relative or absolute).
    pub file: String,
    /// 1-based line number (0 if the checker did not report one).
    pub line: u32,
    /// 1-based column (0 if not reported).
    pub col: u32,
    /// `"error"`, `"warning"`, or `"note"`.
    pub severity: String,
    /// Human-readable message.
    pub message: String,
    /// Diagnostic code when available (e.g. `E0432`, `TS2304`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Output from `code_check`.
#[derive(Debug, Clone, Serialize)]
pub struct CodeCheckOutput {
    /// `true` when no errors were found (warnings do not flip this).
    pub ok: bool,
    /// The command that actually ran (after detection), or `"(none)"` when no
    /// toolchain was recognised.
    pub command: String,
    /// Number of `error`-severity diagnostics.
    pub error_count: usize,
    /// Number of `warning`-severity diagnostics.
    pub warning_count: usize,
    /// Normalised diagnostics, capped at [`MAX_DIAGNOSTICS`].
    pub diagnostics: Vec<Diagnostic>,
    /// Tail of the raw combined stdout+stderr, for cases the parser missed.
    pub raw_tail: String,
    /// `true` when diagnostics were dropped to satisfy [`MAX_DIAGNOSTICS`].
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

// =============================================================================
// Tool struct
// =============================================================================

/// Type-checker / linter tool. Holds the shared `Arc<dyn Sandbox>` so every run
/// routes through the same enforcement seam as `code_exec`.
#[derive(Clone, Default)]
pub struct CodeCheckTool {
    sandbox: Option<Arc<dyn Sandbox>>,
}

impl CodeCheckTool {
    /// Create a new tool without a sandbox wired in yet. Boot wiring injects the
    /// real `Arc<dyn Sandbox>` via [`CodeCheckTool::with_sandbox`]; unconfigured
    /// instances refuse execution with a structured error rather than spawning
    /// directly.
    #[must_use]
    pub fn new() -> Self {
        Self { sandbox: None }
    }

    /// Attach the shared sandbox (boot wiring threads the single
    /// `WorkspaceSandbox` through here, mirroring `CodeExecTool`).
    pub fn with_sandbox(mut self, sandbox: Arc<dyn Sandbox>) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    async fn run(&self, args: CodeCheckArgs) -> Result<CodeCheckOutput> {
        // Prefer a worktree-isolated subagent's scoped sandbox override; fall
        // back to the construction-time sandbox outside that scope.
        let sandbox =
            match crate::sandbox::context::current_sandbox_override()
                .or_else(|| self.sandbox.clone())
            {
                Some(s) => s,
                None => return Ok(unconfigured(
                    "code_check: sandbox not configured — boot wiring must inject Arc<dyn Sandbox>",
                )),
            };
        let session_id =
            match current_session() {
                Some(sid) => sid,
                None => return Ok(unconfigured(
                    "code_check: no active session context — invoke via invoke_with_session_trace",
                )),
            };

        let plan = build_plan(args.command.as_deref(), args.path.as_deref());
        let timeout_secs = args.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECS);

        info!(
            command = %plan.display_command,
            parser = ?plan.parser,
            "code_check: running checker via sandbox"
        );

        // PATH enhancement so cargo / node / go resolve under the cleared
        // sandbox environment — same trick code_exec uses.
        let mut env = std::collections::HashMap::new();
        if let Ok(enhanced) = crate::runtimes::ledger::build_enhanced_path() {
            env.insert("PATH".to_string(), enhanced);
        }
        for name in ["HOME", "USER", "LANG", "LC_ALL"] {
            if let Ok(v) = std::env::var(name) {
                env.insert(name.to_string(), v);
            }
        }

        let cmd = SandboxCommand {
            session_id,
            program: "bash".to_string(),
            args: vec!["-c".to_string(), plan.script.clone()],
            env,
            stdin: None,
            cwd: None, // workspace root — where the project markers live
            capabilities: SandboxCapabilities {
                fs_read: Vec::new(),
                fs_write: Vec::new(),
                network: NetworkPolicy::None,
                // Type-checkers fork child processes (rustc, tsc workers, go
                // tool); without this the sandbox would block them.
                spawn_subprocess: true,
                max_memory_mb: None,
                timeout_secs: None,
            },
            timeout: Some(Duration::from_secs(timeout_secs)),
        };

        Ok(interpret(sandbox.execute(cmd).await, plan))
    }
}

// =============================================================================
// Detection / planning (pure)
// =============================================================================

/// Which parser to apply to a checker's output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parser {
    /// `cargo --message-format=json` line-delimited JSON.
    CargoJson,
    /// Generic `file:line:col` / `file(line,col)` textual scan.
    Generic,
}

/// A resolved checker invocation.
struct Plan {
    /// The shell script handed to `bash -c`.
    script: String,
    /// Human-readable command for the output (detection cascade collapses to
    /// `"auto-detect"` until the marker reveals the real one at parse time).
    display_command: String,
    /// Parser to apply (refined from the `ALEPH_TC=` marker at interpret time).
    parser: Parser,
}

/// Build the run plan from an optional explicit command and a path hint.
fn build_plan(command: Option<&str>, path: Option<&str>) -> Plan {
    if let Some(cmd) = command.map(str::trim).filter(|c| !c.is_empty()) {
        let parser = if cmd.contains("message-format=json") || cmd.contains("message-format json") {
            Parser::CargoJson
        } else {
            Parser::Generic
        };
        // Prefix a marker so interpret() can still tell the two apart.
        let marker = if parser == Parser::CargoJson {
            "cargo"
        } else {
            "explicit"
        };
        return Plan {
            script: format!("echo 'ALEPH_TC={marker}'\n{cmd}"),
            display_command: cmd.to_string(),
            parser,
        };
    }

    // Auto-detect inside the sandbox so it sees the session workspace root.
    // The `ALEPH_TC=` marker tells the host-side parser which format to expect.
    let py_target = path
        .map(str::trim)
        .filter(|p| p.ends_with(".py") && !p.is_empty())
        .map(shell_single_quote);
    let py_fallback = match py_target {
        Some(q) => format!("python3 -m py_compile {q}"),
        None => "echo 'ALEPH_TC=none'".to_string(),
    };

    let script = format!(
        "if [ -f Cargo.toml ]; then echo 'ALEPH_TC=cargo'; cargo check --message-format=json --quiet; \
         elif [ -f tsconfig.json ]; then echo 'ALEPH_TC=tsc'; npx --no-install tsc --noEmit --pretty false; \
         elif [ -f go.mod ]; then echo 'ALEPH_TC=go'; go vet ./...; \
         elif [ -f pyproject.toml ] || [ -f ruff.toml ]; then echo 'ALEPH_TC=ruff'; ruff check --output-format=concise; \
         else echo 'ALEPH_TC=py'; {py_fallback}; fi"
    );

    Plan {
        script,
        display_command: "auto-detect".to_string(),
        parser: Parser::Generic, // refined at interpret time from the marker
    }
}

/// Quote a string for safe single-argument use in a POSIX shell.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// =============================================================================
// Interpretation (pure)
// =============================================================================

/// Turn a sandbox result into the tool output, choosing the parser from the
/// `ALEPH_TC=` marker the script emitted.
fn interpret(
    result: std::result::Result<SandboxOutput, SandboxError>,
    plan: Plan,
) -> CodeCheckOutput {
    let (stdout, stderr, exit_code) = match result {
        Ok(out) => (
            String::from_utf8_lossy(&out.stdout).to_string(),
            String::from_utf8_lossy(&out.stderr).to_string(),
            out.exit_code,
        ),
        Err(SandboxError::Timeout {
            partial_stdout,
            partial_stderr,
            ..
        }) => (
            String::from_utf8_lossy(&partial_stdout).to_string(),
            String::from_utf8_lossy(&partial_stderr).to_string(),
            None,
        ),
        Err(SandboxError::CapabilityDenied { reason }) => {
            return error_output("(denied)", format!("Capability denied: {reason}"))
        }
        Err(err) => return error_output("(error)", format!("Sandbox error: {err}")),
    };

    let marker = detect_marker(&stdout);
    let (parser, command) = match marker.as_deref() {
        Some("cargo") => (Parser::CargoJson, "cargo check".to_string()),
        Some("tsc") => (Parser::Generic, "tsc --noEmit".to_string()),
        Some("go") => (Parser::Generic, "go vet ./...".to_string()),
        Some("ruff") => (Parser::Generic, "ruff check".to_string()),
        Some("none") => {
            return CodeCheckOutput {
                ok: true,
                command: "(none)".to_string(),
                error_count: 0,
                warning_count: 0,
                diagnostics: Vec::new(),
                raw_tail: "No recognised toolchain (Cargo.toml / tsconfig.json / go.mod / \
                           pyproject.toml). Pass an explicit `command` to run a checker."
                    .to_string(),
                truncated: false,
            }
        }
        _ => (plan.parser, plan.display_command),
    };

    let mut diags = match parser {
        Parser::CargoJson => parse_cargo_json(&stdout),
        Parser::Generic => parse_generic(&format!("{stdout}\n{stderr}")),
    };

    let total = diags.len();
    let truncated = total > MAX_DIAGNOSTICS;
    if truncated {
        diags.truncate(MAX_DIAGNOSTICS);
    }

    let error_count = diags.iter().filter(|d| d.severity == "error").count();
    let warning_count = diags.iter().filter(|d| d.severity == "warning").count();

    // A non-zero process exit with no parsed diagnostics means the checker
    // failed in a way our parser didn't recognise (e.g. cargo link-stage
    // failure, py_compile SyntaxError, a custom command's error format).
    // Reporting ok:true there would tell the model the code compiles when it
    // does not — the exact failure mode this tool exists to catch. Warnings
    // that merely make a linter exit non-zero are still ok (they produce
    // parsed diagnostics, so total > 0).
    let exit_failed = exit_code.is_some_and(|c| c != 0);

    CodeCheckOutput {
        ok: error_count == 0 && !(exit_failed && total == 0),
        command,
        error_count,
        warning_count,
        diagnostics: diags,
        raw_tail: tail(&format!("{stdout}\n{stderr}"), RAW_TAIL_BYTES),
        truncated,
    }
}

fn error_output(command: &str, message: String) -> CodeCheckOutput {
    CodeCheckOutput {
        ok: false,
        command: command.to_string(),
        error_count: 0,
        warning_count: 0,
        diagnostics: Vec::new(),
        raw_tail: message,
        truncated: false,
    }
}

fn unconfigured(message: &str) -> CodeCheckOutput {
    error_output("(unconfigured)", message.to_string())
}

/// Find the first `ALEPH_TC=<kind>` marker line in stdout.
fn detect_marker(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .take(8)
        .find_map(|l| l.trim().strip_prefix("ALEPH_TC=").map(|s| s.to_string()))
}

/// Keep the last `max_bytes` bytes of `s`, on a char boundary.
fn tail(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.trim().to_string();
    }
    let start = s.len() - max_bytes;
    let start = (start..s.len())
        .find(|&i| s.is_char_boundary(i))
        .unwrap_or(s.len());
    s[start..].trim().to_string()
}

// =============================================================================
// Parsers (pure)
// =============================================================================

/// Parse `cargo --message-format=json` line-delimited output.
fn parse_cargo_json(stdout: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("reason").and_then(|r| r.as_str()) != Some("compiler-message") {
            continue;
        }
        let msg = match v.get("message") {
            Some(m) => m,
            None => continue,
        };
        let level = msg.get("level").and_then(|l| l.as_str()).unwrap_or("");
        if level != "error" && level != "warning" {
            continue; // skip note/help/failure-note at the top level
        }
        let text = msg
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        if text.is_empty() {
            continue;
        }
        let code = msg
            .get("code")
            .and_then(|c| c.get("code"))
            .and_then(|c| c.as_str())
            .map(String::from);
        let (file, line_no, col) = msg
            .get("spans")
            .and_then(|s| s.as_array())
            .and_then(|spans| {
                spans
                    .iter()
                    .find(|sp| {
                        sp.get("is_primary")
                            .and_then(|p| p.as_bool())
                            .unwrap_or(false)
                    })
                    .or_else(|| spans.first())
            })
            .map(|sp| {
                (
                    sp.get("file_name")
                        .and_then(|f| f.as_str())
                        .unwrap_or("")
                        .to_string(),
                    sp.get("line_start").and_then(|l| l.as_u64()).unwrap_or(0) as u32,
                    sp.get("column_start").and_then(|c| c.as_u64()).unwrap_or(0) as u32,
                )
            })
            .unwrap_or_default();
        out.push(Diagnostic {
            file,
            line: line_no,
            col,
            severity: level.to_string(),
            message: text,
            code,
        });
    }
    out
}

/// Generic scan covering tsc (`file(line,col): error TSxxxx: msg`) and
/// unix-style (`file:line:col: severity: msg`, or `file:line:col: msg` with no
/// severity — go vet, clang). Lines that match nothing are ignored.
fn parse_generic(text: &str) -> Vec<Diagnostic> {
    // tsc: path(line,col): error TS1234: message
    let tsc = match Regex::new(
        r"^(?P<file>[^(\s][^(]*)\((?P<line>\d+),(?P<col>\d+)\):\s*(?P<sev>error|warning)\s+(?P<code>[A-Za-z]+\d+):\s*(?P<msg>.+)$",
    ) {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };
    // unix with severity: path:line:col: severity: message
    let unix_sev = match Regex::new(
        r"^(?P<file>[^:\s][^:]*):(?P<line>\d+):(?P<col>\d+):\s*(?P<sev>error|warning|note):\s*(?P<msg>.+)$",
    ) {
        Ok(re) => re,
        Err(_) => return Vec::new(),
    };
    // unix without severity (go vet / clang): path:line:col: message
    let unix_plain =
        match Regex::new(r"^(?P<file>[^:\s][^:]*):(?P<line>\d+):(?P<col>\d+):\s*(?P<msg>.+)$") {
            Ok(re) => re,
            Err(_) => return Vec::new(),
        };

    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.is_empty() {
            continue;
        }
        if let Some(c) = tsc.captures(line) {
            out.push(Diagnostic {
                file: c["file"].trim().to_string(),
                line: c["line"].parse().unwrap_or(0),
                col: c["col"].parse().unwrap_or(0),
                severity: c["sev"].to_string(),
                message: c["msg"].trim().to_string(),
                code: Some(c["code"].to_string()),
            });
            continue;
        }
        if let Some(c) = unix_sev.captures(line) {
            out.push(Diagnostic {
                file: c["file"].trim().to_string(),
                line: c["line"].parse().unwrap_or(0),
                col: c["col"].parse().unwrap_or(0),
                severity: c["sev"].to_string(),
                message: c["msg"].trim().to_string(),
                code: None,
            });
            continue;
        }
        if let Some(c) = unix_plain.captures(line) {
            out.push(Diagnostic {
                file: c["file"].trim().to_string(),
                line: c["line"].parse().unwrap_or(0),
                col: c["col"].parse().unwrap_or(0),
                severity: "error".to_string(),
                message: c["msg"].trim().to_string(),
                code: None,
            });
        }
    }
    out
}

// =============================================================================
// AlephTool impl
// =============================================================================

#[async_trait]
impl AlephTool for CodeCheckTool {
    const NAME: &'static str = "code_check";
    const DESCRIPTION: &'static str = r#"Run the project's type-checker / linter and return structured diagnostics.

Auto-detects the toolchain at the session workspace root:
- Rust  (Cargo.toml)      -> `cargo check` (parsed from JSON: file/line/col/code)
- TS/JS (tsconfig.json)   -> `tsc --noEmit`
- Go    (go.mod)          -> `go vet ./...`
- Python(pyproject/ruff)  -> `ruff check`  (or `python -m py_compile <path>`)

Override detection with `command` (e.g. "cargo clippy --message-format=json", "npx eslint .").
`path` biases detection by extension (and selects the file for py_compile).

Returns: {ok, command, error_count, warning_count, diagnostics:[{file,line,col,severity,message,code}], raw_tail}.
Use this AFTER editing code to confirm it still compiles before moving on. Note: a full
`cargo check` can be slow on a cold crate — call it deliberately, not after every tiny edit."#;

    type Args = CodeCheckArgs;
    type Output = CodeCheckOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        self.run(args).await
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// TDD RED: `timeout_seconds` is the canonical spelling; the legacy bare
    /// `timeout` must still parse via `#[serde(alias = "timeout")]` so saved
    /// calls / prompts don't break (deserialize-only, no schema change).
    #[test]
    fn code_check_timeout_accepts_canonical_and_legacy_alias() {
        let a: CodeCheckArgs =
            serde_json::from_value(serde_json::json!({ "timeout_seconds": 30 })).unwrap();
        assert_eq!(a.timeout_seconds, Some(30));
        let b: CodeCheckArgs =
            serde_json::from_value(serde_json::json!({ "timeout": 30 })).unwrap();
        assert_eq!(b.timeout_seconds, Some(30));
    }

    #[test]
    fn cargo_json_extracts_primary_span() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"error","message":"cannot find value `foo`","code":{"code":"E0425"},"spans":[{"is_primary":true,"file_name":"src/lib.rs","line_start":12,"column_start":5}]}}
{"reason":"compiler-artifact","target":{"name":"x"}}"#;
        let d = parse_cargo_json(stdout);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "src/lib.rs");
        assert_eq!(d[0].line, 12);
        assert_eq!(d[0].col, 5);
        assert_eq!(d[0].severity, "error");
        assert_eq!(d[0].code.as_deref(), Some("E0425"));
        assert!(d[0].message.contains("cannot find value"));
    }

    #[test]
    fn cargo_json_skips_notes_and_non_messages() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"note","message":"note text","spans":[]}}
{"reason":"build-finished","success":false}
not json at all"#;
        assert!(parse_cargo_json(stdout).is_empty());
    }

    #[test]
    fn cargo_json_prefers_primary_over_first_span() {
        let stdout = r#"{"reason":"compiler-message","message":{"level":"warning","message":"unused","code":null,"spans":[{"is_primary":false,"file_name":"a.rs","line_start":1,"column_start":1},{"is_primary":true,"file_name":"b.rs","line_start":9,"column_start":3}]}}"#;
        let d = parse_cargo_json(stdout);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "b.rs");
        assert_eq!(d[0].line, 9);
        assert_eq!(d[0].severity, "warning");
        assert_eq!(d[0].code, None);
    }

    #[test]
    fn generic_parses_tsc_format() {
        let text = "src/app.ts(42,10): error TS2304: Cannot find name 'foo'.";
        let d = parse_generic(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "src/app.ts");
        assert_eq!(d[0].line, 42);
        assert_eq!(d[0].col, 10);
        assert_eq!(d[0].severity, "error");
        assert_eq!(d[0].code.as_deref(), Some("TS2304"));
        assert!(d[0].message.contains("Cannot find name"));
    }

    #[test]
    fn generic_parses_unix_with_severity() {
        let text = "main.py:7:5: error: Name 'x' is not defined";
        let d = parse_generic(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "main.py");
        assert_eq!(d[0].line, 7);
        assert_eq!(d[0].severity, "error");
    }

    #[test]
    fn generic_parses_unix_without_severity_as_error() {
        let text = "pkg/main.go:3:2: undefined: bar";
        let d = parse_generic(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "pkg/main.go");
        assert_eq!(d[0].line, 3);
        assert_eq!(d[0].col, 2);
        assert_eq!(d[0].severity, "error");
        assert!(d[0].message.contains("undefined"));
    }

    #[test]
    fn generic_ignores_noise_lines() {
        let text = "Compiling foo v0.1.0\n   ok\nALEPH_TC=tsc\nrandom prose without a location";
        assert!(parse_generic(text).is_empty());
    }

    #[test]
    fn detect_marker_reads_first_marker() {
        assert_eq!(
            detect_marker("ALEPH_TC=cargo\n{...}").as_deref(),
            Some("cargo")
        );
        assert_eq!(detect_marker("noise\nALEPH_TC=go\n").as_deref(), Some("go"));
        assert_eq!(detect_marker("no marker here").as_deref(), None);
    }

    #[test]
    fn explicit_command_with_json_uses_cargo_parser() {
        let plan = build_plan(Some("cargo clippy --message-format=json"), None);
        assert_eq!(plan.parser, Parser::CargoJson);
        assert!(plan.script.contains("ALEPH_TC=cargo"));
        assert!(plan.script.contains("cargo clippy"));
        assert_eq!(plan.display_command, "cargo clippy --message-format=json");
    }

    #[test]
    fn explicit_plain_command_uses_generic_parser() {
        let plan = build_plan(Some("npx eslint ."), None);
        assert_eq!(plan.parser, Parser::Generic);
        assert!(plan.script.contains("ALEPH_TC=explicit"));
        assert!(plan.script.contains("npx eslint ."));
    }

    #[test]
    fn auto_detect_script_probes_all_toolchains() {
        let plan = build_plan(None, None);
        for marker in ["Cargo.toml", "tsconfig.json", "go.mod", "pyproject.toml"] {
            assert!(plan.script.contains(marker), "missing probe for {marker}");
        }
        assert!(plan.script.contains("ALEPH_TC=none"));
        assert_eq!(plan.display_command, "auto-detect");
    }

    #[test]
    fn py_path_hint_builds_py_compile_fallback() {
        let plan = build_plan(None, Some("scripts/run.py"));
        assert!(plan
            .script
            .contains("python3 -m py_compile 'scripts/run.py'"));
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn tail_keeps_suffix_on_char_boundary() {
        let s = "héllo wörld ".repeat(500);
        let t = tail(&s, 50);
        assert!(t.len() <= 50);
        assert!(!t.is_empty());
        assert!(s.contains(&t));
    }

    #[test]
    fn interpret_none_marker_is_ok_with_hint() {
        let out = interpret(
            Ok(SandboxOutput {
                stdout: b"ALEPH_TC=none\n".to_vec(),
                exit_code: Some(0),
                ..Default::default()
            }),
            build_plan(None, None),
        );
        assert!(out.ok);
        assert_eq!(out.command, "(none)");
        assert!(out.raw_tail.contains("explicit `command`"));
    }

    #[test]
    fn interpret_caps_diagnostics() {
        // 60 cargo error lines -> capped at MAX_DIAGNOSTICS, truncated=true.
        let mut s = String::from("ALEPH_TC=cargo\n");
        for i in 0..60 {
            s.push_str(&format!(
                r#"{{"reason":"compiler-message","message":{{"level":"error","message":"e{i}","code":null,"spans":[{{"is_primary":true,"file_name":"x.rs","line_start":{i},"column_start":1}}]}}}}"#,
            ));
            s.push('\n');
        }
        let out = interpret(
            Ok(SandboxOutput {
                stdout: s.into_bytes(),
                exit_code: Some(1),
                ..Default::default()
            }),
            build_plan(None, None),
        );
        assert_eq!(out.diagnostics.len(), MAX_DIAGNOSTICS);
        assert!(out.truncated);
        assert_eq!(out.error_count, MAX_DIAGNOSTICS);
        assert!(!out.ok);
    }

    /// F3 regression: `code_check` must resolve a worktree-isolated subagent's
    /// scoped sandbox override in preference to its construction-time sandbox,
    /// so a subagent's type-check runs inside its own checkout. This is the
    /// second exec-tool resolution site (identical `.or_else` chain to
    /// `code_exec`); the deterministic two-sandbox check pins it without a
    /// subprocess.
    #[tokio::test]
    async fn run_prefers_scoped_sandbox_override() {
        use crate::routing::session_key::SessionKey;
        use crate::sandbox::context::{with_sandbox_override, SESSION_ID};
        use crate::sandbox::test_util::MockSandbox;

        let ok = || SandboxOutput {
            exit_code: Some(0),
            ..Default::default()
        };
        // Distinct sandboxes so we can tell which one actually ran.
        let parent = MockSandbox::new(ok());
        let scoped = MockSandbox::new(ok());
        let parent_dyn: Arc<dyn Sandbox> = parent.clone();
        let scoped_dyn: Arc<dyn Sandbox> = scoped.clone();
        let tool = CodeCheckTool::new().with_sandbox(parent_dyn);

        let session = SessionKey::ephemeral("code-check-override");
        SESSION_ID
            .scope(
                session,
                with_sandbox_override(Some(scoped_dyn), async {
                    tool.call(CodeCheckArgs {
                        path: None,
                        command: Some("true".to_string()),
                        timeout_seconds: Some(5),
                    })
                    .await
                    .expect("tool call");
                }),
            )
            .await;

        assert_eq!(
            parent.calls.lock().await.len(),
            0,
            "construction-time sandbox must be bypassed when an override is scoped"
        );
        assert_eq!(
            scoped.calls.lock().await.len(),
            1,
            "the scoped override must receive the check command"
        );
    }
}
