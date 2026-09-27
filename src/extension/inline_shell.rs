//! What every face that runs an inline `` !`cmd` `` shares: who may run one
//! ([`inline_shell_refusal`]), the consent entry it answers to
//! ([`InlineConsent`]), the placeholder a withheld one leaves
//! ([`withheld`]), and how an approved one's process is run
//! ([`run_inline_process`]).
//!
//! Two faces run inline commands, and they read all four from here:
//! - a plugin command's body, rendered for `/command`
//!   (`gateway::execution_engine::slash_command_body::ConsentedShell`), whose
//!   entries carry [`INLINE_COMMAND_EVENT`];
//! - a skill's body, preprocessed for `skill_read`
//!   (`skill::preprocess::SkillShell`), whose entries carry
//!   [`SKILL_INLINE_EVENT`].
//!
//! Which text is a command is [`inline_commands`](super::template::inline_commands)'s
//! answer on both, and the process is [`inline_shell_command`]'s.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use super::hooks::{read_capped, ShellHookConsent, MAX_HOOK_OUTPUT_BYTES};
use super::visibility::ScopeKey;
use super::{InlineArgs, InlineShell, PermissionAction};

#[cfg(doc)]
use super::hooks::{INLINE_COMMAND_EVENT, SKILL_INLINE_EVENT};
#[cfg(doc)]
use super::inline_shell_command;

/// Bound on one `` !`cmd` `` expansion. Well under the hook ceiling: an
/// inline command runs before the model reads the text it is part of, and a
/// command that takes longer is one that belongs in the text as an
/// instruction to the model, not in an inline expansion.
pub(crate) const INLINE_SHELL_TIMEOUT: Duration = Duration::from_secs(30);

/// The reason a command is withheld while its consent entry is pending.
pub(crate) const PENDING_APPROVAL: &str =
    "pending operator approval — `aleph-server hooks list` / `aleph-server hooks test`";

/// Why this turn may run none of its inline commands, or `None` — the one
/// rule both faces apply.
///
/// An inline command is a shell run as the daemon's user, outside the
/// sandbox and the `[sandbox.command_policy]` floor, whose output the model
/// reads. So — as `/moa` arms only for an operator on its channel face — it
/// runs only for an OPERATOR (`role_is_operator`: loopback and authorized
/// clients; a channel's chat-tier sender is a guest), and only on a turn
/// whose tool gate would not deny the model its own shell (`bash`): an
/// approved command would otherwise be the very shell the operator switched
/// off.
///
/// `bash` is the tool gate's own answer for `bash` on this turn — the global,
/// agent and channel policies merged, and the tier, so a `plan` turn and a
/// `/btw` side question run none — never a second reading of any one layer:
/// the command face hands in `TurnPermissions::builtin_permission`, the one
/// its run's tool gate is built from; the skill face reads what the tool gate
/// itself answered (`ScopedToolService::permission_for`, published as
/// `tools::turn_context::TURN_INLINE_SHELL`). `Ask` on `bash` does not
/// withhold: the command's consent entry is its own question, asked once per
/// text. The body still renders; each inline command is a placeholder naming
/// the reason.
#[must_use]
pub(crate) fn inline_shell_refusal(
    caller_role: Option<&str>,
    bash: PermissionAction,
) -> Option<&'static str> {
    if !crate::tools::turn_context::role_is_operator(caller_role) {
        return Some("inline commands run only for an operator");
    }
    (bash == PermissionAction::Deny).then_some("this turn's permissions deny `bash`")
}

/// What a withheld `` !`cmd` `` leaves in the text: the command as written
/// and why it did not run. The model reads it; it names no other way to run
/// the command.
#[must_use]
pub(crate) fn withheld(cmd: &str, reason: &str) -> String {
    format!("[!`{cmd}` not run: {reason}]")
}

/// An inline-command runner that runs nothing: every `` !`cmd` `` becomes a
/// visible placeholder naming why ([`withheld`]).
pub(crate) struct Withheld(pub(crate) &'static str);

#[async_trait::async_trait]
impl InlineShell for Withheld {
    async fn run(&self, cmd: &str, _args: &InlineArgs<'_>) -> Result<String, String> {
        Err(withheld(cmd, self.0))
    }
}

/// The consent entry an inline command answers to: the registry a plugin's
/// `hooks.json` commands go through, keyed `(owner, scope, text)` and bound
/// to `root`, filed under `event`.
pub(crate) struct InlineConsent<'a> {
    pub(crate) consent: &'a ShellHookConsent,
    /// The consent owner label: a plugin id (a command), or a skill's label
    /// (`ShellHookConsent::skill_owner`).
    pub(crate) owner: &'a str,
    pub(crate) scope: &'a ScopeKey,
    /// The directory the approval is bound to.
    pub(crate) root: &'a Path,
    /// [`INLINE_COMMAND_EVENT`] or [`SKILL_INLINE_EVENT`]: what the entry is
    /// filed under, and the only approval that covers it.
    pub(crate) event: &'static str,
}

impl InlineConsent<'_> {
    /// `Ok` when an approval covers `cmd` (the text as written); otherwise
    /// the placeholder to leave in its place. An unapproved text is filed as
    /// pending under [`Self::event`] for `aleph-server hooks list` / `test`.
    ///
    /// The key has no event, so a `hooks.json` command with the same text and
    /// owner shares it — and an approval given to that hook, reviewed without
    /// arguments in its root, does not cover an inline command. Judged on the
    /// entry that approved, from the same read.
    pub(crate) fn admit(&self, cmd: &str) -> Result<(), String> {
        let approval = self
            .consent
            .approved_entry(self.owner, self.scope, self.root, cmd);
        let Some(approval) = approval else {
            self.consent
                .record_pending(self.owner, self.scope, cmd, self.event, self.root);
            tracing::warn!(
                owner = %self.owner,
                event = self.event,
                command = %cmd,
                "inline command not approved — withheld; review with `aleph-server hooks list`"
            );
            return Err(withheld(cmd, PENDING_APPROVAL));
        };
        if approval.event != self.event {
            return Err(withheld(
                cmd,
                &format!(
                    "its approval was given to a hook with the same text — `aleph-server hooks \
                     revoke {}`, then review it as an inline command",
                    approval.fingerprint
                ),
            ));
        }
        Ok(())
    }
}

/// Run an approved inline command's process (built by
/// [`inline_shell_command`]): its stdout, capped like a hook's; stderr
/// discarded; [`INLINE_SHELL_TIMEOUT`]. A failure — it cannot start, exits
/// non-zero, times out — is a visible placeholder.
pub(crate) async fn run_inline_process(
    cmd: &str,
    mut command: tokio::process::Command,
) -> Result<String, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let run = async {
        let mut child = command
            .spawn()
            .map_err(|e| format!("[!`{cmd}` failed to start: {e}]"))?;
        let (buf, truncated) = match child.stdout.take() {
            Some(stdout) => read_capped(stdout, MAX_HOOK_OUTPUT_BYTES).await,
            None => (Vec::new(), false),
        };
        let status = child
            .wait()
            .await
            .map_err(|e| format!("[!`{cmd}` failed: {e}]"))?;
        if !status.success() {
            let code = status
                .code()
                .map_or_else(|| "by signal".to_string(), |c| c.to_string());
            return Err(format!("[!`{cmd}` exited {code}]"));
        }
        let mut text = String::from_utf8_lossy(&buf).into_owned();
        if truncated {
            text.push_str(&format!(
                "\n...[truncated: output exceeds {} KiB]",
                MAX_HOOK_OUTPUT_BYTES / 1024
            ));
        }
        Ok(text)
    };
    match tokio::time::timeout(INLINE_SHELL_TIMEOUT, run).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "[!`{cmd}` timed out after {}s]",
            INLINE_SHELL_TIMEOUT.as_secs()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_operator_on_a_turn_that_does_not_deny_bash_runs_inline_commands() {
        use PermissionAction::{Allow, Ask, Deny};
        assert_eq!(inline_shell_refusal(None, Allow), None);
        assert_eq!(inline_shell_refusal(Some("operator"), Ask), None);
        assert_eq!(
            inline_shell_refusal(Some("guest"), Allow),
            Some("inline commands run only for an operator")
        );
        assert_eq!(
            inline_shell_refusal(Some("member"), Allow),
            Some("inline commands run only for an operator")
        );
        assert_eq!(
            inline_shell_refusal(Some("operator"), Deny),
            Some("this turn's permissions deny `bash`")
        );
    }

    #[test]
    fn a_withheld_command_names_itself_and_the_reason() {
        assert_eq!(withheld("date", "why"), "[!`date` not run: why]");
    }
}
