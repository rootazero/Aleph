//! `/command args` → the command's rendered markdown body, as this turn's
//! transient user content.
//!
//! A Claude Code plugin's `commands/<name>.md` body is "literally Claude's
//! instructions when invoked" (plugin-dev skill, verbatim). Until this module
//! nothing on the `/command` path put that text in front of the model: the
//! slash resolver fell through to the agent loop with the raw `/command args`
//! and the body sat in `SkillRegistration.content`, parsed and never read.
//!
//! **Where it renders.** The fast path's fallthrough arm in `execute.rs`
//! calls [`stamp`] — after `execute_slash_command_fast_path` has judged the
//! command's owning plugin visible to this session (`extension::visibility`
//! face ④), so the inline commands of a plugin the session cannot see never
//! run. [`stamp`] renders the body once — the `` !`cmd` `` expansions and
//! `@file` reads happen there — and leaves the `<command>` block in the
//! request's metadata; the run loop pushes it FIRST into its transient
//! blocks ([`transient_block`]).
//!
//! **Not persisted.** The transient channel (`transient_blocks` →
//! `HarnessDeps::recall_context`) is delivered to the model every Think and
//! never written to the session log, so the stored user turn — and the
//! session title derived from it — stays the raw `/command args`. Claude Code
//! persists the expansion instead; `PLUGIN_SYSTEM.md` records the difference.
//! A steering rescue strips the block ([`strip`]); a crash resume re-drives
//! the log, which never held it.
//!
//! **Inline commands** run through [`ConsentedShell`]: the consent registry a
//! plugin's `hooks.json` command goes through, keyed by the command's own
//! template text, then the production builder
//! ([`inline_shell_command`](crate::extension::inline_shell_command)).
//!
//! **`model:`** pins this turn's model when the request carries none
//! ([`command_model_pin`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::extension::hooks::{
    read_capped, ShellHookConsent, INLINE_COMMAND_EVENT, MAX_HOOK_OUTPUT_BYTES,
};
use crate::extension::plugin_secrets::SettingsForm;
use crate::extension::visibility::ScopeKey;
use crate::extension::{
    inline_shell_command, ExtensionError, ExtensionManager, InlineArgs, InlineShell, InlineSite,
    SkillRegistration, SkillTemplate, SkillType, TemplateCtx,
};
use crate::gateway::agent_instance::AgentInstance;
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::gateway::model_override::ModelOverride;
use crate::sync_primitives::Arc;

use super::RunRequest;

/// Request-metadata key carrying a `/command` turn's rendered `<command>`
/// block. Written by [`stamp`], read by [`transient_block`], removed by
/// [`strip`]; nothing outside this module spells it.
const BODY_KEY: &str = "slash_command_body";

/// Request-metadata key present when `model_override` is the command's
/// `model:` rather than the request's own pick, so [`strip`] can tell the two
/// apart once both sit in the same field.
const MODEL_PIN_KEY: &str = "slash_command_model";

/// Bound on one `` !`cmd` `` expansion. Well under the hook ceiling: this
/// runs BEFORE the turn's first Think, and a command that takes longer is one
/// that belongs in the body as an instruction to the model, not in an inline
/// expansion.
const INLINE_SHELL_TIMEOUT: Duration = Duration::from_secs(30);

/// What a resolved `/command` contributes to its turn.
pub(super) struct CommandTurn {
    /// The `<command …>…</command>` block for the transient blocks.
    pub block: String,
    /// The command's `model:` as this turn's override, when it applies
    /// ([`command_model_pin`]).
    pub model_pin: Option<ModelOverride>,
}

/// Render this turn's plugin command into the request, if its slash mode
/// names one: the `<command>` block into metadata, the command's `model:`
/// into `model_override`. The fast path's fallthrough arm calls this, after
/// the owning plugin was judged visible.
///
/// `Err` is the user-facing reason the turn cannot go ahead as the command
/// wrote it (a retired `model:`, a body over the file-reference cap); the
/// caller fails the run with it rather than sending the raw `/command` alone.
pub(super) async fn stamp(request: &mut RunRequest, agent: &AgentInstance) -> Result<(), String> {
    let Some(manager) = crate::extension::try_extension_manager() else {
        return Ok(());
    };
    stamp_with(request, agent, manager, ShellHookConsent::shared()).await
}

/// [`stamp`] with its two process globals handed in.
pub(super) async fn stamp_with(
    request: &mut RunRequest,
    agent: &AgentInstance,
    manager: &ExtensionManager,
    consent: Arc<ShellHookConsent>,
) -> Result<(), String> {
    let Some(mode) = request
        .metadata
        .get(SLASH_COMMAND_MODE_KEY)
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
    else {
        return Ok(());
    };
    // The run's own directory — the value its tools and exec jail get, from
    // the same derivation — and only if it is there to run in.
    let cwd = Some(super::run_loop::run_workspace(request, agent)).filter(|d| d.is_dir());
    let Some(turn) = resolve_command_turn(
        &mode,
        cwd,
        request.model_override.as_ref(),
        manager,
        consent,
    )
    .await?
    else {
        return Ok(());
    };
    request.metadata.insert(BODY_KEY.to_string(), turn.block);
    if let Some(pin) = turn.model_pin {
        request
            .metadata
            .insert(MODEL_PIN_KEY.to_string(), pin.model().to_string());
        request.model_override = Some(pin);
    }
    Ok(())
}

/// Resolve the slash-mode JSON to a command turn: `Ok(None)` when the mode is
/// not a `type: "skill"` envelope naming a [`SkillType::Command`]
/// registration. The model is judged BEFORE the body renders, so a turn that
/// is refused runs none of its inline commands.
pub(super) async fn resolve_command_turn(
    mode: &serde_json::Value,
    cwd: Option<PathBuf>,
    requested_model: Option<&ModelOverride>,
    manager: &ExtensionManager,
    consent: Arc<ShellHookConsent>,
) -> Result<Option<CommandTurn>, String> {
    if mode.get("type").and_then(serde_json::Value::as_str) != Some("skill") {
        return Ok(None);
    }
    let Some(skill_id) = mode.get("skill_id").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let args = mode
        .get("args")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // One registry read for the registration and its plugin's record: the
    // install root (not `reg.base_dir()`, which is `<root>/commands`) and the
    // visibility key the consent entry is filed under.
    let (reg, plugin) = {
        let registry = manager.get_plugin_registry().await;
        let Some(reg) = registry
            .get_skill(skill_id)
            .filter(|s| s.skill_type == SkillType::Command)
            .cloned()
        else {
            return Ok(None);
        };
        let plugin = registry
            .get_plugin(&reg.plugin_id)
            .map(|record| (record.root_dir.clone(), record.scope_key.clone()));
        (reg, plugin)
    };
    let model_pin = command_model_pin(requested_model, reg.model.as_deref())
        .map_err(|why| format!("/{skill_id} was not run: {why}"))?;
    let shell = match plugin {
        Some((plugin_root, scope)) => Some(ConsentedShell {
            settings_env: manager
                .plugin_settings_env(&reg.plugin_id, SettingsForm::WithoutSecrets)
                .await,
            plugin_id: reg.plugin_id.clone(),
            scope,
            plugin_root,
            cwd,
            consent,
        }),
        // A registration whose plugin has no record cannot be tied to an
        // install root or a consent key: its inline commands are withheld.
        None => None,
    };
    let rendered = render_registration(&reg, args, shell.as_ref().map(|s| s as &dyn InlineShell))
        .await
        .map_err(|e| format!("/{skill_id} could not be rendered: {e}"))?;
    Ok(Some(CommandTurn {
        block: wrap_block(skill_id, &reg.plugin_id, &rendered),
        model_pin,
    }))
}

/// Render one registration's body with the Claude Code / pi grammar. `@./file`
/// resolves against the command file's directory
/// ([`SkillRegistration::base_dir`]); `shell: None` withholds every inline
/// command.
pub(super) async fn render_registration(
    reg: &SkillRegistration,
    args: &str,
    shell: Option<&dyn InlineShell>,
) -> Result<String, ExtensionError> {
    SkillTemplate::with_base_dir(&reg.content, reg.base_dir())
        .render(args, &TemplateCtx { shell })
        .await
}

/// The block the model reads. Named so the model can tell "the user invoked a
/// command" from "the user typed this"; the raw `/…` text is the persisted
/// turn.
pub(super) fn wrap_block(qualified: &str, plugin_id: &str, rendered: &str) -> String {
    let name = qualified.rsplit(':').next().unwrap_or(qualified);
    format!(
        "<command name=\"{}\" plugin=\"{}\" invoked=\"/{}\">\n{}\n</command>",
        attribute(name),
        attribute(plugin_id),
        attribute(qualified),
        rendered.trim()
    )
}

/// `text` safe inside a double-quoted attribute.
fn attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The command's `model:` as this turn's override: `Ok(None)` when it pins
/// nothing, `Err(why)` when the turn must not go ahead on it.
///
/// - The request's own pick (the composer's) wins: the command's is not
///   applied.
/// - `inherit` and a blank value pin nothing.
/// - Claude Code's aliases `sonnet` / `opus` / `haiku` name a model family,
///   not an id any provider routes, and Aleph has no alias table: not applied
///   — the agent's model serves the turn — and logged.
/// - An id the model catalog records as retired is refused, with its
///   successor named: the provider would fail it. The same `lifecycle_for`
///   table and rule as `select_model`, the other face that pins a model.
/// - Any other id is pinned as written (`Raw`), as `select_model` accepts an
///   id the catalog does not know. One no provider serves then fails at the
///   provider, or the fallback walk serves the turn on another model and
///   says so on every surface (`helpers::emit_route_correction`) — never
///   silently.
pub(super) fn command_model_pin(
    requested: Option<&ModelOverride>,
    declared: Option<&str>,
) -> Result<Option<ModelOverride>, String> {
    if requested.is_some() {
        return Ok(None);
    }
    let Some(model) = declared.map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    match model {
        "inherit" => return Ok(None),
        "sonnet" | "opus" | "haiku" => {
            tracing::warn!(
                model,
                "command declares a Claude Code model alias; Aleph routes no aliases, so the \
                 turn runs on the agent's model"
            );
            return Ok(None);
        }
        _ => {}
    }
    let life = crate::providers::model_catalog::lifecycle_for(None, model);
    if life.is_deprecated() {
        let mut why = format!("its `model: {model}` has been retired by its vendor");
        if let Some(note) = life.note {
            why.push_str(&format!(" ({note})"));
        }
        if let Some(successor) = life.successor {
            why.push_str(&format!("; the plugin should declare `{successor}`"));
        }
        return Err(why);
    }
    Ok(Some(ModelOverride::Raw {
        model: model.to_string(),
    }))
}

/// The rendered `<command>` block [`stamp`] left for this turn, if any.
pub(super) fn transient_block(metadata: &HashMap<String, String>) -> Option<String> {
    metadata.get(BODY_KEY).cloned()
}

/// Drop a command turn's residue from metadata that is being re-driven as a
/// plain loop continuation (the steering rescue): the rendered body — a
/// second delivery would be a second instruction — and the model pin the
/// command declared, which was for its own turn. Returns the model override
/// the continuation keeps: the request's own pick, never the command's.
pub(super) fn strip(
    metadata: &mut HashMap<String, String>,
    model_override: Option<ModelOverride>,
) -> Option<ModelOverride> {
    metadata.remove(BODY_KEY);
    if metadata.remove(MODEL_PIN_KEY).is_some() {
        None
    } else {
        model_override
    }
}

/// The `` !`cmd` `` runner: the SAME consent registry, and the same
/// `(plugin, scope, command)` key and root binding, as a plugin's
/// `hooks.json` shell command (`HookExecutor::execute_command`), so
/// `aleph hooks list` / `aleph hooks test` are the one review surface for
/// every shell a plugin can reach. The entry is filed under
/// [`INLINE_COMMAND_EVENT`] with the template's own text.
///
/// Approved, the command runs through the production builder in the run's
/// directory, with the plugin's settings minus every secret
/// ([`SettingsForm::WithoutSecrets`]), stdout capped like a hook's, stderr
/// discarded, [`INLINE_SHELL_TIMEOUT`]; a failure is a visible placeholder.
pub(super) struct ConsentedShell {
    plugin_id: String,
    scope: ScopeKey,
    /// The plugin's install root — what the approval is bound to, and its
    /// path variables' value.
    plugin_root: PathBuf,
    /// The run's directory; `None` when it is not there to run in.
    cwd: Option<PathBuf>,
    settings_env: Vec<(String, String)>,
    consent: Arc<ShellHookConsent>,
}

#[async_trait::async_trait]
impl InlineShell for ConsentedShell {
    async fn run(&self, cmd: &str, args: &InlineArgs<'_>) -> Result<String, String> {
        if !self
            .consent
            .is_approved(&self.plugin_id, &self.scope, &self.plugin_root, cmd)
        {
            self.consent.record_pending(
                &self.plugin_id,
                &self.scope,
                cmd,
                INLINE_COMMAND_EVENT,
                &self.plugin_root,
            );
            tracing::warn!(
                plugin = %self.plugin_id,
                command = %cmd,
                "inline command not approved — withheld; review with `aleph hooks list`"
            );
            return Err(format!(
                "[!`{cmd}` not run: pending operator approval — `aleph hooks list` / `aleph hooks test`]"
            ));
        }
        let Some(cwd) = self.cwd.as_deref() else {
            return Err(format!(
                "[!`{cmd}` not run: no working directory is known for this turn]"
            ));
        };
        let site = InlineSite {
            cwd,
            plugin: Some((&self.plugin_id, &self.plugin_root)),
        };
        let mut command = inline_shell_command(cmd, args, &site);
        command
            .envs(self.settings_env.iter().map(|(k, v)| (k, v)))
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
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
}

#[cfg(test)]
mod tests;
