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
//! ([`inline_shell_command`](crate::extension::inline_shell_command)) — and
//! only for an operator, on a turn whose tool gate does not deny the model
//! `bash` ([`inline_shell_refusal`]). A skill's inline commands
//! (`skill::preprocess`) take the same rule, consent check, placeholder and
//! process run from the same place (`extension::inline_shell`).
//!
//! **`model:`** pins this turn's model when the request carries none
//! ([`command_model_pin`]) — applied before the run is admitted
//! ([`pin_model`]), so the busy lane sees the model the command runs on.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use crate::extension::hooks::{ShellHookConsent, INLINE_COMMAND_EVENT};
use crate::extension::inline_shell::{
    run_inline_process, withheld, InlineConsent, Withheld, NO_RUN_DIRECTORY,
};
use crate::extension::plugin_secrets::SettingsForm;
use crate::extension::visibility::ScopeKey;
use crate::extension::{
    inline_shell_command, ExtensionError, ExtensionManager, InlineArgs, InlineShell, InlineSite,
    PluginRecord, PluginRegistry, SkillRegistration, SkillTemplate, SkillType, TemplateCtx,
};
use crate::gateway::inbound_router::SLASH_COMMAND_MODE_KEY;
use crate::gateway::model_override::ModelOverride;
use crate::sync_primitives::Arc;

use super::turn_permissions::TurnPermissions;
use super::{ExecutionError, RunRequest};

/// Request-metadata key naming the plugin command [`admit`] admitted for this
/// turn (its qualified id). Written by [`admit`], read by [`render_admitted`],
/// removed by [`strip`]; nothing outside this module spells it.
const ADMITTED_KEY: &str = "slash_command_admitted";

/// Request-metadata key present when `model_override` is the command's
/// `model:` rather than the request's own pick, so [`strip`] can tell the two
/// apart once both sit in the same field.
const MODEL_PIN_KEY: &str = "slash_command_model";

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
    let Some(reg) =
        owned_command(&mode, &*manager.get_plugin_registry().await).map(|(reg, _)| reg.clone())
    else {
        // No command to admit — so no pin [`pin_model`] may have made before
        // admission is this turn's either.
        request.model_override = strip(&mut request.metadata, request.model_override.take());
        return Ok(());
    };
    let qualified = reg.qualified_name();
    let pin = command_model_pin(request.model_override.as_ref(), reg.model.as_deref())
        .map_err(|why| format!("/{qualified} was not run: {why}"))?;
    request.metadata.insert(ADMITTED_KEY.to_string(), qualified);
    if let Some(pin) = pin {
        apply_pin(request, pin);
    }
    Ok(())
}

/// Pin this turn's model to its plugin command's `model:` BEFORE the run is
/// admitted, so the run's registered copy — what the busy lane folds a
/// follow-up message against while this run parks for a slot — names that
/// model from the start. `execute()` calls this just before `admit_run`.
///
/// Changes nothing but the request, and only on a pin: the same
/// [`owned_command`] and [`command_model_pin`] [`admit`] runs. A `model:`
/// that must refuse the turn is left for [`admit`] to refuse, after the
/// owner gate; a pin whose command is then not admitted is dropped there. A
/// mode stamped after admission (the producers that never pass through a
/// handler) is pinned by [`admit`] alone, and the run's copy is updated by
/// `mark_fallen_through`.
pub(super) async fn pin_model(request: &mut RunRequest) {
    let Some(manager) = crate::extension::try_extension_manager() else {
        return;
    };
    pin_model_with(request, manager).await;
}

/// [`pin_model`] with the extension manager handed in.
pub(super) async fn pin_model_with(request: &mut RunRequest, manager: &ExtensionManager) {
    let Some(mode) = slash_mode(&request.metadata) else {
        return;
    };
    let Some(declared) = owned_command(&mode, &*manager.get_plugin_registry().await)
        .map(|(reg, _)| reg.model.clone())
    else {
        return;
    };
    if let Ok(Some(pin)) = command_model_pin(request.model_override.as_ref(), declared.as_deref()) {
        apply_pin(request, pin);
    }
}

/// The command's `model:` as this turn's override, marked as the command's
/// ([`MODEL_PIN_KEY`]) so [`strip`] can tell it from the request's own pick.
fn apply_pin(request: &mut RunRequest, pin: ModelOverride) {
    request
        .metadata
        .insert(MODEL_PIN_KEY.to_string(), pin.model().to_string());
    request.model_override = Some(pin);
}

/// Render the command [`admit`] admitted for this turn: its `<command>`
/// block, or `None` when the turn carries no command. The run loop calls
/// this after its turn-start seams let the turn go ahead.
///
/// `run_dir` is the run's own directory (`run_loop::run_workspace`); inline
/// commands run in it only if it is there. `permissions` are this turn's, as
/// the run loop resolved them for its tool gate ([`inline_shell_refusal`]).
/// The render races `cancel`: a stopped turn stops waiting on its inline
/// commands (whose children are killed on drop). An admitted command that can
/// no longer be found under the same key, or whose body cannot be rendered,
/// fails the turn visibly.
pub(super) async fn render_admitted(
    request: &RunRequest,
    run_dir: &Path,
    permissions: &TurnPermissions,
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
    let refusal = inline_shell_refusal(&request.metadata, permissions);
    let render = render_command(&mode, cwd, refusal, manager, consent);
    let rendered = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(ExecutionError::Cancelled),
        rendered = render => rendered.map_err(|why| refused(&why))?,
    };
    match rendered {
        Some((qualified, block)) if qualified == *admitted => Ok(Some(block)),
        _ => Err(refused("it is no longer an active plugin's command")),
    }
}

/// Why this turn may run none of its command's inline commands, or `None`:
/// the one rule every inline face applies
/// ([`inline_shell_refusal`](crate::extension::inline_shell::inline_shell_refusal)
/// has it and why), on this turn's caller role and on what this turn's tool
/// gate answers for `bash` ([`TurnPermissions::builtin_permission`] — the
/// permissions the run's tool gate is built from). A skill's inline commands
/// get the same rule on the same two facts, as the tool gate publishes them
/// (`tools::turn_context::TURN_INLINE_SHELL`).
fn inline_shell_refusal(
    metadata: &HashMap<String, String>,
    permissions: &TurnPermissions,
) -> Option<&'static str> {
    use crate::tools::AlephTool;
    crate::extension::inline_shell::inline_shell_refusal(
        metadata.get("caller_role").map(String::as_str),
        permissions.builtin_permission(crate::builtin_tools::BashExecTool::NAME),
    )
}

/// The slash-mode JSON a request carries, if any.
fn slash_mode(metadata: &HashMap<String, String>) -> Option<serde_json::Value> {
    metadata
        .get(SLASH_COMMAND_MODE_KEY)
        .and_then(|m| serde_json::from_str(m).ok())
}

/// Render the plugin command a slash mode names ([`owned_command`]): its
/// qualified id and its `<command>` block, or `Ok(None)` when the mode names
/// none. `refusal` withholds every inline command, naming the reason
/// ([`inline_shell_refusal`]). `Err` is the render's own failure (the
/// file-reference cap).
pub(super) async fn render_command(
    mode: &serde_json::Value,
    cwd: Option<PathBuf>,
    refusal: Option<&'static str>,
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
    let (reg, plugin_root, scope) = {
        let registry = manager.get_plugin_registry().await;
        let Some((reg, record)) = owned_command(mode, &registry) else {
            return Ok(None);
        };
        (
            reg.clone(),
            record.root_dir.clone(),
            record.scope_key.clone(),
        )
    };
    let qualified = reg.qualified_name();
    let shell: Box<dyn InlineShell> = match refusal {
        Some(reason) => Box::new(Withheld(reason)),
        None => Box::new(ConsentedShell {
            settings_env: manager
                .plugin_settings_env(&reg.plugin_id, SettingsForm::WithoutSecrets)
                .await,
            plugin_id: reg.plugin_id.clone(),
            scope,
            plugin_root,
            cwd,
            consent,
        }),
    };
    let rendered = render_registration(&reg, args, Some(&*shell))
        .await
        .map_err(|e| e.to_string())?;
    let block = wrap_block(&qualified, &reg.plugin_id, &rendered);
    Ok(Some((qualified, block)))
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
///   under that exact qualified name;
/// - that plugin must have a record here, and an active one: disabling a
///   plugin leaves its registrations in place
///   (`PluginRegistry::disable_plugin`), and `get_skill` answers an exact
///   key without asking. The record comes back with the registration — the
///   install root and visibility key its inline commands are consented
///   under.
fn owned_command<'r>(
    mode: &serde_json::Value,
    registry: &'r PluginRegistry,
) -> Option<(&'r SkillRegistration, &'r PluginRecord)> {
    let owner = mode.get("owning_plugin")?.as_str()?;
    let skill_id = mode.get("skill_id")?.as_str()?;
    if !skill_id.contains(':') {
        return None;
    }
    let reg = registry.get_skill(skill_id).filter(|reg| {
        reg.skill_type == SkillType::Command
            && reg.plugin_id == owner
            && reg.qualified_name() == skill_id
    })?;
    let record = registry
        .get_plugin(&reg.plugin_id)
        .filter(|record| record.status.is_active())?;
    Some((reg, record))
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
/// - Otherwise the declared `model:` goes through the one policy a plugin
///   agent's `model:` goes through too
///   ([`declared_model_pin`](crate::extension::declared_model::declared_model_pin)):
///   `inherit`, a blank and the aliases `sonnet` / `opus` / `haiku` pin
///   nothing (the agent's model serves the turn); a retired id refuses the
///   turn, its successor named; any other id is pinned as written (`Raw`).
///   One no provider serves then fails at the provider, or the fallback walk
///   serves the turn on another model and says so on every surface
///   (`helpers::emit_route_correction`) — never silently.
pub(super) fn command_model_pin(
    requested: Option<&ModelOverride>,
    declared: Option<&str>,
) -> Result<Option<ModelOverride>, String> {
    if requested.is_some() {
        return Ok(None);
    }
    let pin = crate::extension::declared_model::declared_model_pin("command", declared)?;
    Ok(pin.map(|model| ModelOverride::Raw { model }))
}

/// Drop a command turn's residue from metadata that is being re-driven as a
/// plain loop continuation (the steering rescue), or whose command turned
/// out not to be admissible after [`pin_model`] pinned it ([`admit`]): the
/// admitted command — a second render would be a second instruction, and
/// would run its inline commands again — and the model pin the command
/// declared, which was for its own turn. Returns the model override the
/// request keeps: its own pick, never the command's.
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
/// `aleph-server hooks list` / `aleph-server hooks test` review both — and a
/// skill's inline commands (`skill::preprocess::SkillShell`, under
/// [`SKILL_INLINE_EVENT`](crate::extension::hooks::SKILL_INLINE_EVENT)): the
/// one review surface for every inline shell a body can hold, whoever wrote
/// the body. The entry is filed under [`INLINE_COMMAND_EVENT`] with the
/// template's own text; only an approval that says so covers it
/// ([`InlineConsent::admit`]). A command with a relative word ahead of the
/// script consent binds is never filed or run
/// ([`ShellHookConsent::unbindable_script_word`], which also names what it
/// does not look at).
///
/// Approved, the command runs through the production builder in the run's
/// directory, with the plugin's settings minus every secret
/// ([`SettingsForm::WithoutSecrets`]), stdout capped like a hook's, stderr
/// discarded, [`INLINE_SHELL_TIMEOUT`](crate::extension::inline_shell::INLINE_SHELL_TIMEOUT)
/// ([`run_inline_process`]); a failure is a visible placeholder.
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
        let project = match &self.scope {
            ScopeKey::Project(root) => Some(root.as_path()),
            ScopeKey::Global => None,
        };
        // Consent would review a relative script in the plugin's root while
        // this runs the session's copy: never filed, never run.
        if let Some(word) = ShellHookConsent::unbindable_script_word(
            &self.plugin_id,
            project,
            Some(&self.plugin_root),
            cmd,
        ) {
            return Err(format!(
                "[!`{cmd}` not run: `{word}` is a relative path — consent would review the \
                 plugin's copy while this runs the session's; the plugin should write \
                 `${{CLAUDE_PLUGIN_ROOT}}/{word}` for its own script]"
            ));
        }
        InlineConsent {
            consent: &self.consent,
            owner: &self.plugin_id,
            scope: &self.scope,
            root: &self.plugin_root,
            event: INLINE_COMMAND_EVENT,
        }
        .admit(cmd)?;
        let Some(cwd) = self.cwd.as_deref() else {
            return Err(withheld(cmd, NO_RUN_DIRECTORY));
        };
        let site = InlineSite {
            cwd,
            plugin: Some((&self.plugin_id, &self.plugin_root)),
            skill_dir: None,
        };
        let mut command = inline_shell_command(cmd, args, &site);
        command.envs(self.settings_env.iter().map(|(k, v)| (k, v)));
        run_inline_process(cmd, command).await
    }
}

#[cfg(test)]
mod tests;
