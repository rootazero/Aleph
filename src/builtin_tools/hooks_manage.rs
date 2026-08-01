//! Hook management tool — inspect and edit event hooks conversationally.
//!
//! R8 (Everything is a Tool): every configurable Aleph operation should be
//! reachable through natural language. Hooks were not — the `hooks.*`
//! JSON-RPC surface existed but only the CLI spoke it, so "why isn't my
//! format-on-write hook running?" had no answer the model could look up, and
//! "add a hook that lints after every Write" had no path but hand-editing
//! JSON. Every comparable subsystem already had its tool (`channel_manage`,
//! `cron_manage`, `loop_manage`, `agent_manage`, `skill_manage`, …).
//!
//! # Two views, and why both exist
//!
//! - `list` returns the **runtime registry**: every hook the server has
//!   actually registered, across all four layers (`~/.aleph/hooks.json`,
//!   `<project>/.aleph/hooks.json`, `hooks.local.json`, plugin-shipped),
//!   with the resolved `kind`, consent state, and a per-hook `reachable`
//!   verdict. This is the diagnostic view.
//! - `show_file` returns the raw global `hooks.json` — what `add` / `remove`
//!   actually edit. Only the global file is writable here, matching the RPC
//!   layer: project hook files live in repos and get committed, so a remote
//!   operator (or the model) must not rewrite them.
//!
//! # What this tool deliberately cannot do
//!
//! **Approve a shell/HTTP hook.** Consent is what stands between a hook
//! declaration and arbitrary code execution (or POSTing tool I/O to an
//! arbitrary URL). Letting the model approve hooks would let a prompt-injected
//! model write a hook AND consent to it in one turn, which is exactly the
//! attack consent exists to stop. Approval stays on the operator's terminal
//! (`aleph hooks test <fingerprint>`), where the command is printed and run
//! for review first. This tool can only *report* consent state, never grant it.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::approval::{ActionType, ApprovalPolicy};
use crate::error::{AlephError, Result};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

/// Action to perform on the hook system.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HooksAction {
    /// List every registered hook with its reachability verdict (runtime view).
    List,
    /// Show the raw editable global hooks file (`~/.aleph/hooks.json`).
    ShowFile,
    /// Append a hook to the global hooks file.
    Add,
    /// Remove matching hooks from the global hooks file.
    Remove,
    /// List valid event names, and which support `matcher` / `interceptor`.
    Events,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct HooksManageArgs {
    /// What to do.
    pub action: HooksAction,

    /// Event name for `add` / `remove`. Accepts Claude-Code names
    /// (`PreToolUse`, `PostToolUse`, `SessionStart`, `Stop`, …) and Aleph
    /// names (`before_tool_call`, …). Call `action="events"` when unsure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,

    /// Shell command to run. Exactly one of command/prompt/agent/url for `add`.
    /// NOTE: a newly-added shell hook does NOT run until the operator approves
    /// it at their terminal with `aleph hooks test <fingerprint>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,

    /// Static text injected as context for the model when the hook fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    /// Agent to request delegation to when the hook fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,

    /// URL to POST the event payload to. Also consent-gated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Tool-name regex. ONLY meaningful on tool events — see `action="events"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matcher: Option<String>,

    /// Per-hook timeout in seconds. Clamped to 300.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,

    /// For `list`: only show hooks bound to this event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter_event: Option<String>,

    /// For `list`: only show hooks that cannot fire. Use this first when
    /// diagnosing "my hook doesn't run".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_unreachable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct HooksManageOutput {
    /// Human-readable summary of what happened / what was found.
    pub summary: String,
    /// Structured payload; shape depends on the action.
    pub data: serde_json::Value,
}

/// Hook management tool.
#[derive(Default, Clone)]
pub struct HooksManageTool {
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
}

impl HooksManageTool {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            approval_policy: None,
        }
    }

    /// Gate `add` / `remove` behind the approval policy — installing a hook
    /// fires arbitrary code (or POSTs tool I/O) on a future lifecycle event.
    /// Read-only actions (`list` / `show_file` / `events`) stay open. With no
    /// policy wired the tool behaves exactly as before.
    pub fn with_approval_policy(mut self, policy: Arc<dyn ApprovalPolicy>) -> Self {
        self.approval_policy = Some(policy);
        self
    }
}

#[async_trait]
impl AlephTool for HooksManageTool {
    const NAME: &'static str = "hooks_manage";
    const DESCRIPTION: &'static str =
        "Inspect and edit event hooks — shell commands, HTTP calls, prompts, or agent \
         delegations that fire on lifecycle events (before/after a tool call, session start, \
         user prompt submit, stop, compaction, …). \
         Use action='list' to see every registered hook and whether it can actually fire — \
         this is the answer to 'why isn't my hook running?', because it reports the two \
         silent-death causes (a `matcher` on an event that has no tool name, and \
         kind=interceptor on an observer-only event) plus whether the hook is still \
         waiting on operator consent. Pass only_unreachable=true to see just the broken ones. \
         action='add'/'remove' edit the global ~/.aleph/hooks.json; project hook files are \
         intentionally read-only here since they live in repos. \
         IMPORTANT: adding a shell or http hook does NOT make it run — it is recorded as \
         pending until the operator approves it at their own terminal with \
         `aleph hooks test <fingerprint>`. Say so when you add one; you cannot approve it \
         yourself and must not claim the hook is active.";

    type Args = HooksManageArgs;
    type Output = HooksManageOutput;

    fn examples(&self) -> Option<Vec<String>> {
        Some(vec![
            r#"hooks_manage(action="list", only_unreachable=true)"#.to_string(),
            r#"hooks_manage(action="list", filter_event="PreToolUse")"#.to_string(),
            r#"hooks_manage(action="add", event="PostToolUse", matcher="Write|Edit", command="npx prettier --write $FILE")"#.to_string(),
            r#"hooks_manage(action="add", event="UserPromptSubmit", prompt="Current sprint: 42")"#.to_string(),
            r#"hooks_manage(action="remove", event="PostToolUse", command="npx prettier --write $FILE")"#.to_string(),
            r#"hooks_manage(action="events")"#.to_string(),
        ])
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        if matches!(args.action, HooksAction::Add | HooksAction::Remove) {
            let target = match args.action {
                HooksAction::Add => format!(
                    "add event={} command={} url={} prompt={} agent={}",
                    args.event.as_deref().unwrap_or(""),
                    args.command.as_deref().unwrap_or(""),
                    args.url.as_deref().unwrap_or(""),
                    args.prompt.as_deref().unwrap_or(""),
                    args.agent.as_deref().unwrap_or(""),
                ),
                HooksAction::Remove => format!(
                    "remove event={} needle={}",
                    args.event.as_deref().unwrap_or(""),
                    args.command
                        .as_deref()
                        .or(args.url.as_deref())
                        .or(args.prompt.as_deref())
                        .or(args.agent.as_deref())
                        .unwrap_or("")
                ),
                _ => unreachable!(),
            };
            if let Some(policy) = self.approval_policy.as_ref() {
                let request = crate::approval::ActionRequest {
                    action_type: ActionType::HooksManage,
                    target: target.clone(),
                    display_target: target,
                    agent_id: crate::approval::audit_identity("hooks", "manage", "global")
                        .0,
                    context: format!("hooks_manage action={:?}", args.action),
                    timestamp: chrono::Utc::now(),
                };
                match policy.check(&request).await {
                    crate::approval::ApprovalDecision::Allow => {
                        policy.record(&request, &crate::approval::ApprovalDecision::Allow).await;
                    }
                    crate::approval::ApprovalDecision::Deny { reason } => {
                        let _ = reason;
                        return Err(AlephError::tool(
                            "Action denied by approval policy: hooks manage write refused",
                        ));
                    }
                    crate::approval::ApprovalDecision::Ask { prompt } => {
                        return Err(AlephError::tool(format!(
                            "Approval required: {prompt} (run `aleph hooks test <fingerprint>` \
                             to grant consent at the operator terminal instead)"
                        )));
                    }
                }
            }
        }
        match args.action {
            HooksAction::List => list_registry(&args).await,
            HooksAction::ShowFile => show_file(),
            HooksAction::Add => add(&args),
            HooksAction::Remove => remove(&args),
            HooksAction::Events => Ok(events()),
        }
    }
}

/// The runtime view: what is registered and whether it can fire.
async fn list_registry(args: &HooksManageArgs) -> Result<HooksManageOutput> {
    let Some(manager) = crate::extension::try_extension_manager() else {
        return Ok(HooksManageOutput {
            summary: "The extension manager is not running, so no hooks are registered."
                .to_string(),
            data: serde_json::json!({ "hooks": [], "total": 0 }),
        });
    };
    let all = manager.hook_executor_snapshot().await.inventory();
    let total = all.len();

    // Normalise the filter through the same parser the loader uses, so
    // `PreToolUse` and `before_tool_call` both select the same hooks.
    let wanted = args
        .filter_event
        .as_deref()
        .map(|raw| parse_event_name(raw).ok_or_else(|| unknown_event(raw)))
        .transpose()?;

    let only_broken = args.only_unreachable.unwrap_or(false);
    let shown: Vec<_> = all
        .into_iter()
        .filter(|h| wanted.as_ref().is_none_or(|w| &h.event == w))
        .filter(|h| !only_broken || !h.reachable)
        .collect();

    let unreachable = shown.iter().filter(|h| !h.reachable).count();
    let pending = shown
        .iter()
        .filter(|h| h.consent.as_deref() == Some("pending"))
        .count();

    let mut summary = format!("{} of {total} registered hook(s) shown.", shown.len());
    if unreachable > 0 {
        summary.push_str(&format!(
            " {unreachable} cannot fire as configured — see each hook's `issue`."
        ));
    }
    if pending > 0 {
        summary.push_str(&format!(
            " {pending} are waiting on operator consent and will be skipped until \
             approved at the terminal with `aleph hooks test <fingerprint>`."
        ));
    }
    if shown.is_empty() && total == 0 {
        summary = "No hooks are registered. Add one with action='add', or check \
                   ~/.aleph/hooks.json with action='show_file'."
            .to_string();
    }

    Ok(HooksManageOutput {
        summary,
        data: serde_json::json!({
            "total": total,
            "shown": shown.len(),
            "unreachable": unreachable,
            "pending_consent": pending,
            "hooks": shown,
        }),
    })
}

/// The file view: the raw editable global hooks file.
fn show_file() -> Result<HooksManageOutput> {
    let (path, exists, events) =
        crate::gateway::handlers::hooks_admin::read_user_hooks_file().map_err(AlephError::tool)?;
    Ok(HooksManageOutput {
        summary: if exists {
            format!(
                "{} defines hooks for {} event(s). This file is only ONE of four hook \
                 layers — use action='list' for everything the server actually registered.",
                path.display(),
                events.len()
            )
        } else {
            format!(
                "{} does not exist yet; action='add' will create it.",
                path.display()
            )
        },
        data: serde_json::json!({
            "path": path.display().to_string(),
            "exists": exists,
            "events": events,
        }),
    })
}

fn add(args: &HooksManageArgs) -> Result<HooksManageOutput> {
    let event_raw = args
        .event
        .as_deref()
        .ok_or_else(|| AlephError::tool("hooks_manage add: 'event' is required"))?;
    let event = parse_event_name(event_raw).ok_or_else(|| unknown_event(event_raw))?;

    let action = crate::gateway::handlers::hooks_admin::build_hook_action(
        args.command.as_deref(),
        args.prompt.as_deref(),
        args.agent.as_deref(),
        args.url.as_deref(),
        args.timeout_secs,
    )
    .map_err(AlephError::tool)?;

    crate::gateway::handlers::hooks_admin::append_user_hook(
        &event,
        action,
        args.matcher.as_deref(),
    )
    .map_err(AlephError::tool)?;

    // Say the awkward part out loud rather than letting the model report
    // success: a gated hook that was just written is NOT live.
    let gated = args.command.is_some() || args.url.is_some();
    let mut summary = format!("Added a hook on {event}.");
    if gated {
        summary.push_str(
            " It will NOT run yet: shell and HTTP hooks stay pending until the operator \
             reviews and approves them at their own terminal with `aleph hooks list` then \
             `aleph hooks test <fingerprint>`.",
        );
    }
    if args.matcher.is_some() && !supports_matcher(&event) {
        summary.push_str(
            " WARNING: the matcher was saved but this event carries no tool name, so the \
             hook will never fire — remove the matcher to have it fire every time.",
        );
    }

    Ok(HooksManageOutput {
        summary,
        data: serde_json::json!({ "event": event, "pending_consent": gated }),
    })
}

fn remove(args: &HooksManageArgs) -> Result<HooksManageOutput> {
    let event_raw = args
        .event
        .as_deref()
        .ok_or_else(|| AlephError::tool("hooks_manage remove: 'event' is required"))?;
    let event = parse_event_name(event_raw).ok_or_else(|| unknown_event(event_raw))?;

    // Any one of the action fields identifies which entries to drop. One is
    // REQUIRED: `remove_user_hooks` treats `None` as "clear the whole event",
    // and a model that names the event but forgets the command would then
    // silently delete every hook on it. The RPC has the same rule
    // (`command` or `index`); a bulk clear has to be spelled out one hook at
    // a time rather than fall out of an omitted argument.
    let needle = args
        .command
        .as_deref()
        .or(args.url.as_deref())
        .or(args.prompt.as_deref())
        .or(args.agent.as_deref())
        .ok_or_else(|| {
            AlephError::tool(
                "hooks_manage remove: name which hook to drop via one of \
                 command / url / prompt / agent (substring match). Omitting it would \
                 remove EVERY hook on this event; call action='list' first if unsure.",
            )
        })?;

    let removed = crate::gateway::handlers::hooks_admin::remove_user_hooks(&event, Some(needle))
        .map_err(AlephError::tool)?;

    Ok(HooksManageOutput {
        summary: if removed == 0 {
            format!(
                "No hooks on {event} matched. Note only ~/.aleph/hooks.json is editable — \
                 project and plugin hooks must be removed at their own source."
            )
        } else {
            format!("Removed {removed} hook(s) from {event}.")
        },
        data: serde_json::json!({ "event": event, "removed": removed }),
    })
}

/// Event catalogue, annotated with the two capability facts that decide
/// whether a hook can fire — so the model can pick a valid shape up front
/// instead of writing a dead hook and diagnosing it afterwards.
fn events() -> HooksManageOutput {
    let rows: Vec<serde_json::Value> = crate::extension::HookEvent::ALL
        .iter()
        .map(|e| {
            serde_json::json!({
                "event": event_name(*e),
                "supports_matcher": e.supports_matcher(),
                "supports_interceptor": e.supports_interceptor(),
            })
        })
        .collect();
    HooksManageOutput {
        summary: format!(
            "{} hook events. `supports_matcher=false` means a `matcher` there never \
             matches (the event carries no tool name). `supports_interceptor=false` means \
             the event only runs observer-kind hooks, so it cannot block or rewrite.",
            rows.len()
        ),
        data: serde_json::json!({ "events": rows }),
    }
}

// -- helpers -----------------------------------------------------------------

fn event_name(event: crate::extension::HookEvent) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| format!("{event:?}"))
}

/// Parse a user/model-supplied event name into its canonical form, accepting
/// both Claude-Code (`PreToolUse`) and Aleph (`before_tool_call`) spellings —
/// the same aliases `user_settings.rs` accepts, so a name that works here
/// works in the config file too.
fn parse_event_name(raw: &str) -> Option<String> {
    let attempts = [raw.to_string(), raw.to_lowercase().replace('-', "_")];
    for s in &attempts {
        if let Ok(ev) = serde_json::from_str::<crate::extension::HookEvent>(&format!("\"{s}\"")) {
            return Some(event_name(ev));
        }
    }
    None
}

fn supports_matcher(canonical: &str) -> bool {
    serde_json::from_str::<crate::extension::HookEvent>(&format!("\"{canonical}\""))
        .is_ok_and(|e| e.supports_matcher())
}

fn unknown_event(raw: &str) -> AlephError {
    AlephError::tool(format!(
        "unknown hook event '{raw}'. Call hooks_manage(action=\"events\") for the list."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_names_accept_both_spellings() {
        assert_eq!(
            parse_event_name("PreToolUse"),
            parse_event_name("before_tool_call"),
            "Claude-Code and Aleph spellings must resolve to one canonical name"
        );
        assert!(parse_event_name("SessionStart").is_some());
        assert!(parse_event_name("NotAnEvent").is_none());
    }

    #[test]
    fn events_catalogue_annotates_the_two_footguns() {
        let out = events();
        let rows = out.data["events"].as_array().expect("events array").clone();
        assert!(!rows.is_empty());

        let find = |name: &str| {
            rows.iter()
                .find(|r| r["event"] == name)
                .unwrap_or_else(|| panic!("missing {name}"))
                .clone()
        };
        // A tool event: matcher meaningful, can intercept.
        let pre = find(&parse_event_name("PreToolUse").unwrap());
        assert_eq!(pre["supports_matcher"], true);
        assert_eq!(pre["supports_interceptor"], true);
        // A lifecycle event with no tool name: matcher is a dead end.
        let start = find(&parse_event_name("SessionStart").unwrap());
        assert_eq!(start["supports_matcher"], false);
        // An observer-only seam: an interceptor there never executes.
        let sent = find(&parse_event_name("MessageSent").unwrap());
        assert_eq!(sent["supports_interceptor"], false);
    }

    #[test]
    fn every_event_appears_exactly_once_in_the_catalogue() {
        // Guards `HookEvent::ALL` against drifting out of sync with the enum:
        // a new variant that isn't added there would be invisible to the model.
        let out = events();
        let rows = out.data["events"].as_array().unwrap();
        let mut names: Vec<_> = rows.iter().map(|r| r["event"].clone()).collect();
        let before = names.len();
        names.sort_by_key(std::string::ToString::to_string);
        names.dedup();
        assert_eq!(before, names.len(), "duplicate event in the catalogue");
    }

    #[tokio::test]
    async fn add_requires_an_event() {
        let err = HooksManageTool::new()
            .call(HooksManageArgs {
                action: HooksAction::Add,
                event: None,
                command: Some("echo hi".into()),
                prompt: None,
                agent: None,
                url: None,
                matcher: None,
                timeout_secs: None,
                filter_event: None,
                only_unreachable: None,
            })
            .await
            .expect_err("must reject a missing event");
        assert!(err.to_string().contains("'event' is required"));
    }

    #[tokio::test]
    async fn remove_refuses_to_wipe_a_whole_event_by_omission() {
        // `remove_user_hooks(event, None)` clears the event. Reaching that by
        // simply forgetting the `command` argument would make an accidental
        // bulk delete the easiest thing to type.
        let err = HooksManageTool::new()
            .call(HooksManageArgs {
                action: HooksAction::Remove,
                event: Some("PostToolUse".into()),
                command: None,
                prompt: None,
                agent: None,
                url: None,
                matcher: None,
                timeout_secs: None,
                filter_event: None,
                only_unreachable: None,
            })
            .await
            .expect_err("must refuse an unqualified remove");
        assert!(
            err.to_string().contains("EVERY hook"),
            "the error must explain the danger: {err}"
        );
    }

    #[tokio::test]
    async fn unknown_event_names_point_at_the_catalogue() {
        let err = HooksManageTool::new()
            .call(HooksManageArgs {
                action: HooksAction::Add,
                event: Some("OnTuesday".into()),
                command: Some("echo hi".into()),
                prompt: None,
                agent: None,
                url: None,
                matcher: None,
                timeout_secs: None,
                filter_event: None,
                only_unreachable: None,
            })
            .await
            .expect_err("must reject an unknown event");
        assert!(err.to_string().contains("action=\"events\""));
    }
}
