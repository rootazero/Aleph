//! `aleph-server hooks` — shell-hook consent management.
//!
//! Shell-command hooks shipped by plugins execute arbitrary code. The server
//! gates them behind the consent allowlist
//! (`~/.aleph/shell-hooks-allowlist.json`): an un-approved shell hook is
//! skipped and recorded as `pending`. These subcommands let the operator
//! review, test, approve, and revoke those hooks — and a plugin command's
//! inline shell commands (`` !`cmd` ``), which the same allowlist gates under
//! the event `SlashCommand`.
//!
//! The allowlist file lives outside `~/.aleph/data/` and the consent module
//! guards it with an `fs2` lock + atomic rename, so these commands are safe
//! to run while the server is up — no instance lock is needed.

use std::io::{self, Write};

use alephcore::diagnostics::checks::HooksConsentCheck;
use alephcore::extension::hooks::{
    CommandHookInvocation, ConsentEntry, ConsentStatus, ShellHookConsent, INLINE_COMMAND_EVENT,
    PLUGIN_DATA_VARIABLES, PLUGIN_ROOT_VARIABLES,
};
use alephcore::utils::no_window::NoWindow;

use crate::cli::HooksAction;

type CmdResult = Result<(), Box<dyn std::error::Error>>;

/// Entry point for `aleph-server hooks <action>`.
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
        if let Some(note) = project_note(e) {
            println!("{:<18} project: {note}", "");
        }
        if let Some(note) = e.invoker_arguments_note() {
            println!("{:<18} {note}", "");
        }
    }

    let pending = entries
        .iter()
        .filter(|e| e.status == ConsentStatus::Pending)
        .count();
    if pending > 0 {
        println!();
        println!(
            "{pending} hook(s) pending approval — review with `aleph-server hooks test <fingerprint>`."
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
    // The same template in two projects is two entries; this line is what
    // tells them apart before approving one.
    if let Some(note) = project_note(&entry) {
        println!("Project:     {note}");
    }
    println!("Event:       {}", entry.event);
    if let Some(note) = entry.invoker_arguments_note() {
        println!("Note:        {note}");
    }
    println!("Status:      {}", status_label(entry.status));
    // The directory the run below uses, and the one an approval binds to.
    match &entry.plugin_root {
        Some(root) => println!("Root:        {}", root.display()),
        None => println!("Root:        (not recorded — path variables are unset)"),
    }
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
        run_for_review(&entry, &std::env::current_dir()?)?;
    }

    if entry.status == ConsentStatus::Approved {
        // An approved entry is never rebound, so a hook refused since — its
        // script edited, or run from another root — is re-reviewed by
        // revoking first: the next fire records what it runs now.
        println!(
            "Hook is already approved. If the server refuses it (its script or root changed), \
             run `aleph-server hooks revoke {}` and review it again after it next fires.",
            entry.fingerprint
        );
        return Ok(());
    }

    // `approve` mints no approval without a root; say so rather than ask.
    if entry.plugin_root.is_none() {
        println!(
            "This entry has no recorded root, so it cannot be approved yet: let the hook fire \
             once (that records the directory it runs from), then run `aleph-server hooks test {}` \
             again.",
            entry.fingerprint
        );
        return Ok(());
    }

    let approve_prompt = if http_url.is_some() {
        "Approve this URL so the server may POST hook events to it? [y/N] "
    } else {
        "Approve this hook so the server may run it? [y/N] "
    };
    if prompt_yes(approve_prompt)? {
        // Approves the root shown above, or nothing: a fire may have moved
        // this pending entry to another root while it was being reviewed.
        match consent.approve(&entry.fingerprint, entry.plugin_root.as_deref())? {
            Some(_) => println!("Approved {}.", entry.fingerprint),
            None => println!(
                "Could not approve — the hook is no longer in the registry, or it now runs \
                 from another root. Run `aleph-server hooks test {}` again.",
                entry.fingerprint
            ),
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
/// approved on first prompt. Checked on the line the shell runs
/// ([`gated_text`]), where a path-variable reference such as
/// `${CLAUDE_PLUGIN_ROOT}/x.sh` does not count: it chains nothing.
const SHELL_METACHARS: &[char] = &[';', '&', '|', '$', '`', '>', '<', '\n', '\r'];

/// `line` as the metacharacter gate reads it: without its path-variable
/// references. On unix the shell expands `${CLAUDE_PLUGIN_ROOT}` from the
/// hook's environment as one word of data; on Windows the line already holds
/// the path in its place, which is judged as written.
fn gated_text(line: &str) -> String {
    PLUGIN_ROOT_VARIABLES
        .iter()
        .chain(&PLUGIN_DATA_VARIABLES)
        .fold(line.to_string(), |text, name| {
            text.replace(&format!("${{{name}}}"), "")
        })
}

/// Refuse to hand `line` to a shell when it chains, substitutes or redirects
/// ([`SHELL_METACHARS`], read through [`gated_text`]) — unless the operator
/// set `ALEPH_HOOK_ALLOW_SHELL_METACHARS=1`.
fn refuse_shell_metachars(line: &str) -> CmdResult {
    if !matches!(
        std::env::var("ALEPH_HOOK_ALLOW_SHELL_METACHARS")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    ) && gated_text(line)
        .chars()
        .any(|c| SHELL_METACHARS.contains(&c))
    {
        return Err("hook command contains shell metacharacters; \
             refusing to invoke 'sh -c' / 'cmd /C' on it. \
             Set ALEPH_HOOK_ALLOW_SHELL_METACHARS=1 to override."
            .to_string()
            .into());
    }
    Ok(())
}

/// Print a review run's exit status and output.
fn print_output(output: &std::process::Output) {
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
}

/// The review run of `entry`: an inline command (a plugin command's
/// `` !`cmd` ``) the way the slash-command runner spawns it, a hook the way
/// the hook executor does. `cwd` is where an inline command runs.
fn run_for_review(entry: &ConsentEntry, cwd: &std::path::Path) -> CmdResult {
    if entry.event == INLINE_COMMAND_EVENT {
        run_inline_command(entry, cwd)
    } else {
        run_command_with_payload(entry)
    }
}

/// Run a recorded inline command through the production builder
/// ([`inline_shell_command`](alephcore::extension::inline_shell_command)),
/// with the entry's recorded plugin root — the process the slash-command
/// runner spawns for `/command` sent with NO arguments, in `cwd` (this
/// shell's directory; production runs it in the session's working directory,
/// with the sender's arguments). Not a hook run: no stdin payload, no
/// `TOOL_NAME`, no synthetic `ARGUMENTS`. Not production's timeout, output
/// cap, plugin settings or daemon environment either.
fn run_inline_command(entry: &ConsentEntry, cwd: &std::path::Path) -> CmdResult {
    use alephcore::extension::{inline_shell_command, InlineArgs, InlineSite};

    // The server never runs such a command (it withholds it before consent),
    // so it is never reviewed or approved here either.
    if let Some(word) = ShellHookConsent::root_relative_script(
        &entry.plugin_name,
        entry.project_root.as_deref(),
        entry.plugin_root.as_deref(),
        &entry.command,
    ) {
        return Err(format!(
            "the inline command names the relative path `{word}`: consent would review the \
             plugin's copy while it runs the session's, so the server never runs it and it \
             cannot be approved. The plugin should write `${{CLAUDE_PLUGIN_ROOT}}/{word}`."
        )
        .into());
    }
    refuse_shell_metachars(&entry.command)?;
    println!(
        "(inline command: run in {} with no arguments — production runs it in the session's \
         working directory, with the arguments of whoever sends the command)",
        cwd.display()
    );
    let site = InlineSite {
        cwd,
        plugin: entry
            .plugin_root
            .as_deref()
            .map(|root| (entry.plugin_name.as_str(), root)),
    };
    let mut command = inline_shell_command(
        &entry.command,
        &InlineArgs {
            raw: "",
            positional: &[],
        },
        &site,
    );
    let output = command
        .as_std_mut()
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()?;
    print_output(&output);
    Ok(())
}

/// Run a recorded hook the way production runs it, on a synthetic tool call:
/// the child is built by the same derivation
/// ([`command_hook_invocation`](alephcore::extension::hooks::command_hook_invocation)),
/// so the path variables resolve against the hook's recorded plugin root,
/// the data variables (`"$ARGUMENTS"`, `"$TOOL_NAME"`, …) are in its
/// environment, the event JSON — under the spelling the hook is dispatched
/// with — is on its stdin, and it runs in its plugin root. A script reviewed
/// here sees the variables, directory and payload production gives it from
/// that root — not production's timeout, output cap or daemon environment.
fn run_command_with_payload(entry: &ConsentEntry) -> CmdResult {
    let invocation = test_invocation(entry);
    if entry.plugin_root.is_none() {
        println!(
            "(recorded before Aleph kept a hook's plugin root: path variables such as \
             ${{CLAUDE_PLUGIN_ROOT}} are unset in this run)"
        );
    }
    refuse_shell_metachars(&invocation.line)?;
    println!("(stdin payload: {})", invocation.stdin);

    let mut cmd = std::process::Command::new(invocation.program);
    cmd.arg(invocation.flag).arg(&invocation.line);
    if let Some(dir) = &invocation.current_dir {
        cmd.current_dir(dir);
    }
    for (key, value) in &invocation.env {
        match value {
            Some(value) => {
                cmd.env(key, value);
            }
            None => {
                cmd.env_remove(key);
            }
        }
    }
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_window()
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort: a hook that never reads stdin may close the pipe early.
        let _ = stdin.write_all(invocation.stdin.as_bytes());
    }
    let output = child.wait_with_output()?;
    print_output(&output);
    Ok(())
}

/// The child `aleph-server hooks test` spawns for `entry`: a synthetic tool call,
/// through the production derivation.
///
/// `hook_event_name` is the entry's recorded name verbatim — the spelling the
/// hook is dispatched with (`ConsentEntry::event`), not a re-derivation from
/// the enum, which would lose it. An entry recorded before that field held
/// the spelling carries the enum's Rust name instead; an entry with no event
/// at all gets `before_tool_call`, the shape being representative either way.
fn test_invocation(entry: &ConsentEntry) -> CommandHookInvocation {
    use alephcore::extension::hooks::{command_hook_invocation, HookContext};
    use alephcore::extension::HookEvent;

    let event_name = if entry.event.is_empty() {
        HookEvent::BeforeToolCall.canonical_name()
    } else {
        entry.event.clone()
    };
    // The fields a tool-dispatch hook gets (`build_hook_context` sets both
    // `arguments` and `tool_input` from the call's input).
    let input = r#"{"example":true}"#;
    let ctx = HookContext::new("hooks-cli-test")
        .with_tool_name("ExampleTool")
        .with_arguments(input)
        .with_tool_input(input)
        .with_permission_mode(alephcore::orchestrator::ExecTier::default().cc_permission_mode())
        .with_env("ALEPH_HOOKS_TEST", "1".to_string());
    // (no transcript and no run directory for a synthetic session — both keys
    // are omitted, which is the truth.)
    command_hook_invocation(
        &entry.command,
        &event_name,
        &ctx,
        entry.plugin_root.as_deref(),
        &entry.plugin_name,
    )
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
    // An old `user:project*` entry authorises nothing and never fires again:
    // counted apart, not as an approval or as awaiting one.
    let (kept, live): (Vec<_>, Vec<_>) = entries.iter().partition(|e| e.predates_project_binding());
    let approved = live
        .iter()
        .filter(|e| e.status == ConsentStatus::Approved)
        .count();
    let pending = live.len() - approved;
    println!(
        "  Entries: {} ({approved} approved, {pending} pending, {} from before project \
         binding — authorise nothing)",
        entries.len(),
        kept.len()
    );

    // Issue detection is owned by the unified diagnostics check so that
    // `aleph-server hooks doctor` and `aleph doctor` never drift apart (entropy reduction — the
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

/// Which project an entry's approval is bound to, or — for a project-settings
/// entry recorded before approvals were bound to a project — that it
/// authorises nothing. `None` for a hook that fires everywhere.
fn project_note(entry: &ConsentEntry) -> Option<String> {
    if let Some(root) = &entry.project_root {
        return Some(root.display().to_string());
    }
    entry.predates_project_binding().then(|| {
        "no project — recorded before approvals were bound to one; authorises nothing. \
         Approve the new pending entry of each project instead."
            .to_string()
    })
}

const fn status_label(status: ConsentStatus) -> &'static str {
    match status {
        ConsentStatus::Pending => "pending",
        ConsentStatus::Approved => "approved",
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

    /// The review run is the production run on a synthetic tool call: the
    /// recorded spelling on `hook_event_name`, the path variable reading the
    /// recorded root (which is also the working directory) from the
    /// environment, as the data variables do — and the metacharacter gate
    /// letting that reference through. Driven through
    /// `run_command_with_payload`, the function `aleph-server hooks test` calls.
    #[cfg(unix)]
    #[test]
    fn a_reviewed_hook_runs_as_production_runs_it() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("probe.sh"),
            "cat > stdin.json\n\
             printf '%s' \"$ARGUMENTS\" > arguments.txt\n\
             printf '%s' \"$TOOL_NAME\" > tool_name.txt\n\
             pwd -P > pwd.txt\n",
        )
        .unwrap();
        let entry = pending_entry("sh ${CLAUDE_PLUGIN_ROOT}/probe.sh", root.path());

        run_command_with_payload(&entry).expect("a path-variable reference is not a metacharacter");

        let read = |name: &str| std::fs::read_to_string(root.path().join(name)).unwrap();
        let stdin: serde_json::Value = serde_json::from_str(&read("stdin.json")).unwrap();
        assert_eq!(stdin["hook_event_name"], "PreToolUse");
        assert_eq!(read("arguments.txt"), r#"{"example":true}"#);
        assert_eq!(read("tool_name.txt"), "ExampleTool");
        assert_eq!(
            read("pwd.txt").trim_end(),
            std::fs::canonicalize(root.path())
                .unwrap()
                .to_string_lossy()
        );
    }

    /// The gate's other direction: a template that chains a second command
    /// is refused before anything runs — the path-variable reference in it
    /// does not excuse the `;`.
    #[cfg(unix)]
    #[test]
    fn a_reviewed_hook_that_chains_a_command_is_refused_before_it_runs() {
        let root = tempfile::tempdir().unwrap();
        let entry = pending_entry(
            "sh ${CLAUDE_PLUGIN_ROOT}/probe.sh; touch chained",
            root.path(),
        );

        assert!(run_command_with_payload(&entry).is_err());
        assert!(
            !root.path().join("chained").exists(),
            "the chained command ran"
        );
    }

    /// A plugin command's inline command is reviewed the way the
    /// slash-command runner spawns it — through the inline builder — not as a
    /// hook: `ARGUMENTS` set and empty (no arguments sent), no `TOOL_NAME`,
    /// nothing on stdin, in the directory given, with the recorded root as
    /// its path variable. Driven through `run_for_review`, the function
    /// `aleph-server hooks test` calls.
    #[cfg(unix)]
    #[test]
    fn a_reviewed_inline_command_runs_as_the_slash_command_runner_spawns_it() {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("probe.sh"),
            "printf '%s' \"${ARGUMENTS-unset}\" > \"$CLAUDE_PLUGIN_ROOT/arguments.txt\"\n\
             printf '%s' \"${TOOL_NAME-unset}\" > \"$CLAUDE_PLUGIN_ROOT/tool_name.txt\"\n\
             cat > \"$CLAUDE_PLUGIN_ROOT/stdin.txt\"\n\
             pwd -P > \"$CLAUDE_PLUGIN_ROOT/pwd.txt\"\n",
        )
        .unwrap();
        let entry = ConsentEntry {
            plugin_name: "plug".into(),
            event: INLINE_COMMAND_EVENT.into(),
            ..pending_entry("sh ${CLAUDE_PLUGIN_ROOT}/probe.sh", root.path())
        };

        run_for_review(&entry, cwd.path())
            .expect("a path-variable reference is not a metacharacter");

        let read = |name: &str| std::fs::read_to_string(root.path().join(name)).unwrap();
        assert_eq!(
            read("arguments.txt"),
            "",
            "no synthetic tool-call arguments"
        );
        assert_eq!(read("tool_name.txt"), "unset");
        assert_eq!(read("stdin.txt"), "", "no hook payload on stdin");
        assert_eq!(
            read("pwd.txt").trim_end(),
            std::fs::canonicalize(cwd.path()).unwrap().to_string_lossy()
        );
    }

    /// An inline command naming a relative script is refused before it runs:
    /// the review would run whatever `scripts/x.sh` this shell's directory
    /// holds while consent hashes the plugin's copy.
    #[cfg(unix)]
    #[test]
    fn a_reviewed_inline_command_naming_a_relative_script_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        for dir in [root.path(), cwd.path()] {
            std::fs::create_dir_all(dir.join("scripts")).unwrap();
            std::fs::write(dir.join("scripts/x.sh"), "touch RAN\n").unwrap();
        }
        let entry = ConsentEntry {
            plugin_name: "plug".into(),
            event: INLINE_COMMAND_EVENT.into(),
            ..pending_entry("sh scripts/x.sh", root.path())
        };

        let refusal = run_for_review(&entry, cwd.path()).expect_err("a relative script is refused");
        assert!(refusal.to_string().contains("scripts/x.sh"), "{refusal}");
        for dir in [root.path(), cwd.path()] {
            assert!(!dir.join("RAN").exists(), "the relative script ran");
        }
    }

    /// A pending `user:global` entry for `command`, recorded from `root`.
    #[cfg(unix)]
    fn pending_entry(command: &str, root: &std::path::Path) -> ConsentEntry {
        ConsentEntry {
            fingerprint: "0123456789abcdef".into(),
            plugin_name: "user:global".into(),
            project_root: None,
            command: command.into(),
            event: "PreToolUse".into(),
            plugin_root: Some(root.to_path_buf()),
            status: ConsentStatus::Pending,
            first_seen: 0,
            approved_at: None,
            script_fingerprint: None,
        }
    }
}
