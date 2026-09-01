//! `aleph hooks` — shell-hook consent management.
//!
//! Shell-command hooks shipped by plugins execute arbitrary code. The server
//! gates them behind the consent allowlist
//! (`~/.aleph/shell-hooks-allowlist.json`): an un-approved shell hook is
//! skipped and recorded as `pending`. These subcommands let the operator
//! review, test, approve, and revoke those hooks.
//!
//! The allowlist file lives outside `~/.aleph/data/` and the consent module
//! guards it with an `fs2` lock + atomic rename, so these commands are safe
//! to run while the server is up — no instance lock is needed.

use std::io::{self, Write};

use alephcore::diagnostics::checks::HooksConsentCheck;
use alephcore::extension::hooks::{ConsentStatus, ShellHookConsent};
use alephcore::utils::no_window::NoWindow;

use crate::cli::HooksAction;

type CmdResult = Result<(), Box<dyn std::error::Error>>;

/// Entry point for `aleph hooks <action>`.
pub fn handle_hooks_command(action: HooksAction) -> CmdResult {
    let consent = ShellHookConsent::with_path(ShellHookConsent::default_path());
    match action {
        HooksAction::List => list(&consent),
        HooksAction::Test { fingerprint } => test(&consent, &fingerprint),
        HooksAction::Revoke { fingerprint } => revoke(&consent, &fingerprint),
        HooksAction::Doctor => doctor(&consent),
    }
}

fn list(consent: &ShellHookConsent) -> CmdResult {
    let entries = consent.entries();
    if entries.is_empty() {
        println!("No shell-command hooks recorded.");
        println!("Hooks register here the first time the server runs a turn that triggers them.");
        return Ok(());
    }

    println!("Shell-command hooks ({}):", entries.len());
    println!(
        "{:<18} {:<9} {:<20} COMMAND",
        "FINGERPRINT", "STATUS", "PLUGIN"
    );
    println!("{}", "-".repeat(88));
    for e in &entries {
        println!(
            "{:<18} {:<9} {:<20} {}",
            e.fingerprint,
            status_label(e.status),
            truncate(&e.plugin_name, 20),
            truncate(&e.command, 44),
        );
    }

    let pending = entries
        .iter()
        .filter(|e| e.status == ConsentStatus::Pending)
        .count();
    if pending > 0 {
        println!();
        println!(
            "{pending} hook(s) pending approval — review with `aleph hooks test <fingerprint>`."
        );
    }
    Ok(())
}

fn test(consent: &ShellHookConsent, prefix: &str) -> CmdResult {
    let entry = consent
        .find(prefix)
        .ok_or_else(|| format!("No hook matches fingerprint '{prefix}'."))?;

    // HTTP hook entries are namespaced `http:<url>` in the consent registry.
    // They are reviewed (and approved) here but never shell-executed.
    let http_url = entry.command.strip_prefix("http:");

    println!("Fingerprint: {}", entry.fingerprint);
    println!("Plugin:      {}", entry.plugin_name);
    println!("Event:       {}", entry.event);
    println!("Status:      {}", status_label(entry.status));
    if let Some(url) = http_url {
        println!("HTTP URL (event payload is POSTed here when the hook fires):");
        println!("  {url}");
    } else {
        println!("Command:");
        println!("  {}", entry.command);
    }
    println!();

    if http_url.is_none() {
        if !prompt_yes("Run this command now to verify it? [y/N] ")? {
            // Approval requires actually running and inspecting the command
            // first — declining the test run ends the flow with the hook
            // left safely pending (no approve-sight-unseen path).
            println!("Skipped — hook left pending.");
            return Ok(());
        }
        run_command_with_payload(&entry.command, &entry.event)?;
    }

    if entry.status == ConsentStatus::Approved {
        println!("Hook is already approved.");
        return Ok(());
    }

    let approve_prompt = if http_url.is_some() {
        "Approve this URL so the server may POST hook events to it? [y/N] "
    } else {
        "Approve this hook so the server may run it? [y/N] "
    };
    if prompt_yes(approve_prompt)? {
        match consent.approve(&entry.fingerprint)? {
            Some(_) => println!("Approved {}.", entry.fingerprint),
            None => println!("Could not approve — the hook is no longer in the registry."),
        }
    } else {
        println!("Left pending.");
    }
    Ok(())
}

/// Shell metacharacters that, when present in a hook command, indicate
/// the user is composing a shell pipeline / substitution / chain rather
/// than invoking a single binary. The `test` subcommand rejects these
/// unless `ALEPH_HOOK_ALLOW_SHELL_METACHARS=1` is set — a malicious plugin
/// should not be able to deliver a `; rm -rf ~` payload that gets
/// approved on first prompt.
const SHELL_METACHARS: &[char] = &[';', '&', '|', '$', '`', '>', '<', '\n', '\r'];

/// Run a hook command with a synthetic event payload piped to stdin — the
/// SAME serializer production uses (`event_payload_json`), so a hook that
/// reads stdin (`jq -r '.tool_name'`) behaves identically here and at
/// runtime instead of hanging on the CLI's inherited stdin.
fn run_command_with_payload(command: &str, event: &str) -> CmdResult {
    if !matches!(
        std::env::var("ALEPH_HOOK_ALLOW_SHELL_METACHARS")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    ) && command.chars().any(|c| SHELL_METACHARS.contains(&c))
    {
        return Err("hook command contains shell metacharacters; \
             refusing to invoke 'sh -c' / 'cmd /C' on it. \
             Set ALEPH_HOOK_ALLOW_SHELL_METACHARS=1 to override."
            .to_string()
            .into());
    }
    let payload = synthetic_payload(event);
    println!("(stdin payload: {payload})");

    let (shell, flag) = shell_invocation();
    let mut child = std::process::Command::new(shell)
        .arg(flag)
        .arg(command)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_window()
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort: a hook that never reads stdin may close the pipe early.
        let _ = stdin.write_all(payload.as_bytes());
    }
    let output = child.wait_with_output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("--- exit: {} ---", output.status);
    if !stdout.trim().is_empty() {
        println!("stdout:\n{}", stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        println!("stderr:\n{}", stderr.trim_end());
    }
    println!("----------------");
    Ok(())
}

/// Build the synthetic test payload for a consent entry's recorded event.
/// The event string is stored in `{:?}` (PascalCase) form, which the
/// `HookEvent` serde aliases parse directly; unknown/legacy strings fall
/// back to `BeforeToolCall` — the payload shape is representative either way.
fn synthetic_payload(event: &str) -> String {
    use alephcore::extension::hooks::{event_payload_json, HookContext};
    use alephcore::extension::HookEvent;

    let parsed: HookEvent =
        serde_json::from_str(&format!("\"{event}\"")).unwrap_or(HookEvent::BeforeToolCall);
    let ctx = HookContext::new("hooks-cli-test")
        .with_tool_name("ExampleTool")
        .with_tool_input(r#"{"example":true}"#)
        .with_env("ALEPH_HOOKS_TEST", "1".to_string());
    event_payload_json(parsed, &ctx)
}

fn revoke(consent: &ShellHookConsent, target: &str) -> CmdResult {
    if target.eq_ignore_ascii_case("all") {
        let count = consent.revoke_all()?;
        println!("Revoked {count} approved hook(s).");
        return Ok(());
    }

    match consent.revoke(target)? {
        Some(e) => {
            println!(
                "Revoked consent for {} (plugin '{}').",
                e.fingerprint, e.plugin_name
            );
            Ok(())
        }
        None => Err(format!("No hook matches fingerprint '{target}'.").into()),
    }
}

fn doctor(consent: &ShellHookConsent) -> CmdResult {
    let path = consent.path();
    println!("Shell-hook consent registry");
    println!("  Path:    {}", path.display());
    println!("  Exists:  {}", path.exists());

    let entries = consent.entries();
    let approved = entries
        .iter()
        .filter(|e| e.status == ConsentStatus::Approved)
        .count();
    let pending = entries.len() - approved;
    println!(
        "  Entries: {} ({approved} approved, {pending} pending)",
        entries.len()
    );

    // Issue detection is owned by the unified diagnostics check so that
    // `aleph hooks doctor` and `aleph doctor` never drift apart (entropy reduction — the
    // pending/empty/fingerprint-drift logic now lives in exactly one place).
    let check = HooksConsentCheck::new(ShellHookConsent::with_path(path.to_path_buf()));
    let mut issues = 0usize;
    for finding in check.diagnose() {
        if !finding.is_problem() {
            continue;
        }
        issues += 1;
        println!("  [!] {}", finding.detail);
        if let Some(hint) = &finding.fix_hint {
            println!("      → {hint}");
        }
    }

    if issues == 0 {
        println!("  [ok] No issues found.");
    }
    Ok(())
}

// -- helpers ----------------------------------------------------------------

const fn status_label(status: ConsentStatus) -> &'static str {
    match status {
        ConsentStatus::Pending => "pending",
        ConsentStatus::Approved => "approved",
    }
}

const fn shell_invocation() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    }
}

/// Hard cap: these feed fixed-width table columns, so the ellipsis must fit
/// inside `max` rather than push the column wider.
fn truncate(s: &str, max: usize) -> String {
    alephcore::utils::text_format::truncate_reserving(s, max, "…")
}

fn prompt_yes(prompt: &str) -> io::Result<bool> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("echo hi", 20), "echo hi");
    }

    #[test]
    fn truncate_shortens_long_strings_with_ellipsis() {
        let out = truncate("0123456789", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn status_labels_are_stable() {
        assert_eq!(status_label(ConsentStatus::Pending), "pending");
        assert_eq!(status_label(ConsentStatus::Approved), "approved");
    }
}
