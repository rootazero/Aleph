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

    println!("Fingerprint: {}", entry.fingerprint);
    println!("Plugin:      {}", entry.plugin_name);
    println!("Event:       {}", entry.event);
    println!("Status:      {}", status_label(entry.status));
    println!("Command:");
    println!("  {}", entry.command);
    println!();

    if !prompt_yes("Run this command now to verify it? [y/N] ")? {
        println!("Skipped.");
        return Ok(());
    }

    let (shell, flag) = shell_invocation();
    let output = std::process::Command::new(shell)
        .arg(flag)
        .arg(&entry.command)
        .output()?;

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

    if entry.status == ConsentStatus::Approved {
        println!("Hook is already approved.");
        return Ok(());
    }

    if prompt_yes("Approve this hook so the server may run it? [y/N] ")? {
        match consent.approve(&entry.fingerprint)? {
            Some(_) => println!("Approved {}.", entry.fingerprint),
            None => println!("Could not approve — the hook is no longer in the registry."),
        }
    } else {
        println!("Left pending.");
    }
    Ok(())
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
    // `aleph hooks doctor` and `aleph doctor` never drift apart (熵减 — the
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

fn status_label(status: ConsentStatus) -> &'static str {
    match status {
        ConsentStatus::Pending => "pending",
        ConsentStatus::Approved => "approved",
    }
}

fn shell_invocation() -> (&'static str, &'static str) {
    if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
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
