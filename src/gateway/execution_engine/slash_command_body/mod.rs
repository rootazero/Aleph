//! `/command args` → the command's rendered markdown body, as this turn's
//! transient user content.
//!
//! A Claude Code plugin's `commands/<name>.md` body is "literally Claude's
//! instructions when invoked" (plugin-dev skill, verbatim). Until this module
//! nothing on the `/command` path put that text in front of the model: the
//! slash resolver fell through to the agent loop with the raw `/command args`
//! and the body sat in `SkillRegistration.content`, parsed and never read.
//!
//! **Two steps, two places.**
//! - [`admit`] runs in the fast path's fallthrough arm in `execute.rs`, right
//!   after `execute_slash_command_fast_path` judged the command's owning
//!   plugin visible to this session (`extension::visibility` face ④). It
//!   finds the registration from the same fact the gate judged
//!   ([`owned_command`]: the mode's owner, the exact qualified key), judges
//!   its `model:` — a refused model fails the turn before anything runs — and
//!   marks the request as carrying that command. It spawns nothing.
//! - [`render_admitted`] runs in the run loop, AFTER the turn-start seams
//!   (`BeforeAgentStart`, `UserPromptSubmit`) let the turn go ahead, so a deny
//!   hook stops the command's inline shell too. It renders the body — the
//!   `` !`cmd` `` expansions and `@file` reads happen there, raced against the
//!   run's cancel token — and the run loop puts the `<command>` block FIRST
//!   in its transient blocks.
//!
//! The inline commands of a plugin the session cannot see therefore never
//! run — not even through a bundled skill that shares the command's bare
//! name.
//!
//! **Not persisted.** The transient channel (`transient_blocks` →
//! `HarnessDeps::recall_context`) is delivered to the model every Think and
//! never written to the session log, so the stored user turn — and the
//! session title derived from it — stays the raw `/command args`. Claude Code
//! persists the expansion instead; `PLUGIN_SYSTEM.md` records the difference.
//! A steering rescue strips the command ([`strip`]); a crash resume re-drives
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
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::extension::hooks::{
    read_capped, ShellHookConsent, INLINE_COMMAND_EVENT, MAX_HOOK_OUTPUT_BYTES,
};
use crate::extension::plugin_secrets::SettingsForm;
use crate::extension::visibility::ScopeKey;
use crate::extension::{
    inline_shell_command, ExtensionError, ExtensionManager, InlineArgs, InlineShell, InlineSite,
    PluginRegistry, SkillRegistration, SkillTemplate, SkillType, TemplateCtx,
};
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::gateway::model_override::ModelOverride;
use crate::sync_primitives::Arc;

use super::{ExecutionError, RunRequest};

/// Request-metadata key naming the plugin command [`admit`] admitted for this
/// turn (its qualified id). Written by [`admit`], read by [`render_admitted`],
/// removed by [`strip`]; nothing outside this module spells it.
const ADMITTED_KEY: &str = "slash_command_admitted";

/// Request-metadata key present when `model_override` is the command's
/// `model:` rather than the request's own pick, so [`strip`] can tell the two
/// apart once both sit in the same field.
const MODEL_PIN_KEY: &str = "slash_command_model";

/// Bound on one `` !`cmd` `` expansion. Well under the hook ceiling: this
/// runs BEFORE the turn's first Think, and a command that takes longer is one
/// that belongs in the body as an instruction to the model, not in an inline
/// expansion.
const INLINE_SHELL_TIMEOUT: Duration = Duration::from_secs(30);

/// Admit this turn's plugin command, if its slash mode names one: judge its
/// `model:` into `model_override` and mark the request for
/// [`render_admitted`]. The fast path's fallthrough arm calls this, right
/// after the owning plugin was judged visible. Spawns nothing.
///
/// `Err` is the user-facing reason the turn cannot go ahead as the command
/// wrote it (a retired `model:`); the caller fails the run with it rather
/// than sending the raw `/command` alone.
pub(super) async fn admit(request: &mut RunRequest) -> Result<(), String> {
    let Some(manager) = crate::extension::try_extension_manager() else {
        return Ok(());
    };
    admit_with(request, manager).await
}

/// [`admit`] with the extension manager handed in.
pub(super) async fn admit_with(
    request: &mut RunRequest,
    manager: &ExtensionManager,
) -> Result<(), String> {
    let Some(mode) = slash_mode(&request.metadata) else {
        return Ok(());
    };
    let Some(reg) = owned_command(&mode, &*manager.get_plugin_registry().await).cloned() else {
        return Ok(());
    };
    let qualified = reg.qualified_name();
    let pin = command_model_pin(request.model_override.as_ref(), reg.model.as_deref())
        .map_err(|why| format!("/{qualified} was not run: {why}"))?;
    request.metadata.insert(ADMITTED_KEY.to_string(), qualified);
    if let Some(pin) = pin {
        request
            .metadata
            .insert(MODEL_PIN_KEY.to_string(), pin.model().to_string());
        request.model_override = Some(pin);
    }
    Ok(())
}

/// Render the command [`admit`] admitted for this turn: its `<command>`
/// block, or `None` when the turn carries no command. The run loop calls
/// this after its turn-start seams let the turn go ahead.
///
/// `run_dir` is the run's own directory (`run_loop::run_workspace`); inline
/// commands run in it only if it is there. The render races `cancel`: a
/// stopped turn stops waiting on its inline commands (whose children are
/// killed on drop). An admitted command that can no longer be found under the
/// same key, or whose body cannot be rendered, fails the turn visibly.
pub(super) async fn render_admitted(
    request: &RunRequest,
    run_dir: &Path,
    manager: Option<&ExtensionManager>,
    consent: Arc<ShellHookConsent>,
    cancel: &CancellationToken,
) -> Result<Option<String>, ExecutionError> {
    let Some(admitted) = request.metadata.get(ADMITTED_KEY) else {
        return Ok(None);
    };
    let refused =
        |why: &str| ExecutionError::Failed(format!("/{admitted} could not be rendered: {why}"));
    let (Some(manager), Some(mode)) = (manager, slash_mode(&request.metadata)) else {
        return Err(refused("the extension manager is gone"));
    };
    let cwd = Some(run_dir.to_path_buf()).filter(|d| d.is_dir());
    let render = render_command(&mode, cwd, manager, consent);
    let rendered = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(ExecutionError::Cancelled),
        rendered = render => rendered.map_err(|why| refused(&why))?,
    };
    match rendered {
        Some((qualified, block)) if qualified == *admitted => Ok(Some(block)),
        _ => Err(refused("it is no longer registered")),
    }
}

/// The slash-mode JSON a request carries, if any.
fn slash_mode(metadata: &HashMap<String, String>) -> Option<serde_json::Value> {
    metadata
        .get(SLASH_COMMAND_MODE_KEY)
        .and_then(|m| serde_json::from_str(m).ok())
}

/// Render the plugin command a slash mode names ([`owned_command`]): its
/// qualified id and its `<command>` block, or `Ok(None)` when the mode names
/// none. `Err` is the render's own failure (the file-reference cap).
pub(super) async fn render_command(
    mode: &serde_json::Value,
    cwd: Option<PathBuf>,
    manager: &ExtensionManager,
    consent: Arc<ShellHookConsent>,
) -> Result<Option<(String, String)>, String> {
    if mode.get("type").and_then(serde_json::Value::as_str) != Some("skill") {
        return Ok(None);
    }
    let args = mode
        .get("args")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    // One registry read for the registration and its plugin's record: the
    // install root (not `reg.base_dir()`, which is `<root>/commands`) and the
    // visibility key the consent entry is filed under.
    let (reg, plugin) = {
        let registry = manager.get_plugin_registry().await;
        let Some(reg) = owned_command(mode, &registry).cloned() else {
            return Ok(None);
        };
        let plugin = registry
            .get_plugin(&reg.plugin_id)
            .map(|record| (record.root_dir.clone(), record.scope_key.clone()));
        (reg, plugin)
    };
    let qualified = reg.qualified_name();
    let shell: Box<dyn InlineShell> = match plugin {
        Some((plugin_root, scope)) => Box::new(ConsentedShell {
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
        // install root or a consent key.
        None => Box::new(Withheld(
            "the plugin that ships this command has no record here",
        )),
    };
    let rendered = render_registration(&reg, args, Some(&*shell))
        .await
        .map_err(|e| e.to_string())?;
    let block = wrap_block(&qualified, &reg.plugin_id, &rendered);
    Ok(Some((qualified, block)))
}

/// An inline-command runner that runs nothing: every `` !`cmd` `` becomes a
/// visible placeholder naming why.
struct Withheld(&'static str);

#[async_trait::async_trait]
impl InlineShell for Withheld {
    async fn run(&self, cmd: &str, _args: &InlineArgs<'_>) -> Result<String, String> {
        Err(format!("[!`{cmd}` not run: {}]", self.0))
    }
}

/// The plugin command a slash mode names — the one the fast path's owner gate
/// judged — or `None`.
///
/// The gate (`extension::visibility::slash_owner_admits`) judges
/// `mode["owning_plugin"]`: the owner of the catalog row the parser resolved.
/// This finds the registration by that same fact, never by a second lookup
/// that could land elsewhere:
/// - no owner ⇒ the row is not a plugin's (a bundled or user skill), so no
///   plugin command renders for it;
/// - a bare `skill_id` is refused before any lookup: `PluginRegistry::get_skill`
///   answers a bare name by scanning every active plugin, so the bundled
///   `/code-review` skill would render a same-named plugin command — its
///   body, its `model:`, its approved inline commands — whether or not the
///   session can see that plugin;
/// - the registration at the exact key must be a command of that owner,
///   under that exact qualified name.
fn owned_command<'r>(
    mode: &serde_json::Value,
    registry: &'r PluginRegistry,
) -> Option<&'r SkillRegistration> {
    let owner = mode.get("owning_plugin")?.as_str()?;
    let skill_id = mode.get("skill_id")?.as_str()?;
    if !skill_id.contains(':') {
        return None;
    }
    registry.get_skill(skill_id).filter(|reg| {
        reg.skill_type == SkillType::Command
            && reg.plugin_id == owner
            && reg.qualified_name() == skill_id
    })
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

/// Drop a command turn's residue from metadata that is being re-driven as a
/// plain loop continuation (the steering rescue): the admitted command — a
/// second render would be a second instruction, and would run its inline
/// commands again — and the model pin the command declared, which was for its
/// own turn. Returns the model override the continuation keeps: the
/// request's own pick, never the command's.
pub(super) fn strip(
    metadata: &mut HashMap<String, String>,
    model_override: Option<ModelOverride>,
) -> Option<ModelOverride> {
    metadata.remove(ADMITTED_KEY);
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
