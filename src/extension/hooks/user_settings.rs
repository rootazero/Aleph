//! User-level hook configuration loader.
//!
//! Reads Claude Code-compatible hook definitions from these layers:
//!
//! 1. `~/.aleph/hooks.json` — applies to every Aleph session on this host
//! 2. `<cwd>/.aleph/hooks.json` — project-scoped, intended to be checked in
//! 3. `<cwd>/.aleph/hooks.local.json` — project-scoped, gitignored
//! 4. `<project>/.aleph/hooks.{json,local.json}` for every folder the user has
//!    registered as an Aleph project (the desktop-App "Enter Project" picker
//!    targets). In App mode the daemon CWD is meaningless, so project hooks
//!    are loaded from the registry rather than (only) the launch directory.
//!
//! Layers 2–4 are tagged `user:project` / `user:project-local`; the
//! [`HookExecutor`](super::executor::HookExecutor) gates them at fire time so a
//! hook checked into project A never runs while the agent works inside project
//! B. Layer 1 (`user:global`) always fires.
//!
//! The format mirrors Claude Code's `settings.json` `hooks` block so users
//! can copy a working config across both tools without translation:
//!
//! ```json
//! {
//!   "hooks": {
//!     "PreToolUse": [
//!       {
//!         "matcher": "Edit|Write",
//!         "hooks": [
//!           { "type": "command", "command": "echo hi", "timeout_secs": 30 }
//!         ]
//!       }
//!     ]
//!   }
//! }
//! ```
//!
//! Each `(event, matcher, [actions])` triple flattens to one
//! [`HookConfig`]. Unknown event names are skipped with a warning so a
//! stale config never crashes the server boot.
//!
//! The loader is intentionally fail-soft: a missing file is a no-op, a
//! malformed file produces a warning and an empty result. The user-config
//! layer must never wedge plugin hooks.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::warn;

use crate::extension::types::{HookAction, HookConfig, HookEvent, HookKind, HookPriority};

/// One contiguous group from a `hooks.json` file.
#[derive(Debug, Clone, Deserialize)]
struct UserHookGroup {
    /// Optional regex matched against `tool_name` for tool-related events.
    /// Empty / missing = match all.
    #[serde(default)]
    matcher: Option<String>,

    /// Optional explicit kind override (`observer` | `interceptor`).
    /// When absent, defaults are derived from the event (interceptor for
    /// events that can block, observer otherwise). Unknown values (including
    /// the retired `resolver`) fall back to `observer`.
    #[serde(default)]
    kind: Option<String>,

    /// Optional priority bucket (`system` | `high` | `normal` | `low`).
    #[serde(default)]
    priority: Option<String>,

    /// Per-group default timeout (seconds). Each action inherits this if it
    /// doesn't carry its own.
    #[serde(default)]
    timeout_secs: Option<u64>,

    /// One or more action specs to run when the matcher fires.
    #[serde(default, alias = "actions")]
    hooks: Vec<UserHookAction>,
}

/// A single action inside a hook group.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum UserHookAction {
    Command {
        command: String,
        #[serde(default)]
        timeout_secs: Option<u64>,
    },
    Prompt {
        prompt: String,
    },
    Agent {
        agent: String,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        timeout_secs: Option<u64>,
    },
}

/// Top-level wire format of a `hooks.json` file.
#[derive(Debug, Clone, Deserialize, Default)]
struct UserHooksFile {
    #[serde(default)]
    hooks: HashMap<String, Vec<UserHookGroup>>,
}

/// Load and merge user-level hook configs from the three layers documented
/// above. Layer order is irrelevant for semantics — every hook is added to
/// the executor independently — but it determines the `plugin_name` label
/// surfaced in logs / consent flows.
#[must_use]
pub fn load_user_hooks(cwd: Option<&Path>, project_roots: &[PathBuf]) -> Vec<HookConfig> {
    let mut out = Vec::new();

    if let Some(home) = dirs::home_dir() {
        let p = home.join(".aleph/hooks.json");
        load_into(&p, "user:global", &mut out);
    }

    // Track project roots already loaded (by canonical path) so a folder that
    // is both the daemon CWD and a registered project is not loaded twice —
    // duplicate registration would fire its commands twice per matching event.
    let mut seen: HashSet<PathBuf> = HashSet::new();

    if let Some(cwd) = cwd {
        seen.insert(canonical(cwd));
        load_project_layer(cwd, &mut out);
    }

    for root in project_roots {
        if seen.insert(canonical(root)) {
            load_project_layer(root, &mut out);
        }
    }

    out
}

/// Best-effort canonicalisation for path-equality bookkeeping. Falls back to
/// the path as-given when the directory does not resolve (e.g. not yet
/// created) so dedup stays stable instead of panicking.
fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Load a project directory's checked-in + gitignored hook files. Both are
/// tagged `user:project*` so the executor's fire-time gate binds them to this
/// project root (`<root>/.aleph` → `plugin_root`, parent recovers `<root>`).
fn load_project_layer(root: &Path, out: &mut Vec<HookConfig>) {
    load_into(&root.join(".aleph/hooks.json"), "user:project", out);
    load_into(
        &root.join(".aleph/hooks.local.json"),
        "user:project-local",
        out,
    );
}

fn load_into(path: &Path, source_label: &str, out: &mut Vec<HookConfig>) {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Failed to read user hook config");
            return;
        }
    };
    let parsed: UserHooksFile = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Skipping malformed user hook config");
            return;
        }
    };

    let plugin_root = path.parent().map(PathBuf::from).unwrap_or_default();

    for (event_str, groups) in parsed.hooks {
        let event = match parse_event(&event_str) {
            Some(e) => e,
            None => {
                warn!(path = %path.display(), event = %event_str, "Unknown hook event; skipping");
                continue;
            }
        };
        for g in groups {
            let actions: Vec<HookAction> = g
                .hooks
                .iter()
                .map(|a| match a {
                    UserHookAction::Command { command, .. } => HookAction::Command {
                        command: command.clone(),
                    },
                    UserHookAction::Prompt { prompt } => HookAction::Prompt {
                        prompt: prompt.clone(),
                    },
                    UserHookAction::Agent { agent } => HookAction::Agent {
                        agent: agent.clone(),
                    },
                    UserHookAction::Http { url, headers, .. } => HookAction::Http {
                        url: url.clone(),
                        headers: headers.clone(),
                    },
                })
                .collect();
            if actions.is_empty() {
                continue;
            }

            // First action's per-action timeout (or group default) wins for
            // the whole HookConfig — multiple actions in a group already
            // share state via the group; mixing per-action timeouts inside
            // a single group is rare and easy to express by splitting.
            let timeout_secs = a_or_group_timeout(&g);

            let kind = g.kind.as_deref().map_or_else(
                || default_kind_for_event(event),
                HookKind::from_str_or_default,
            );
            let priority = g
                .priority
                .as_deref()
                .map(HookPriority::from_str_or_default)
                .unwrap_or_default();

            let matcher = g.matcher.clone().filter(|s| !s.is_empty());
            // Foot-gun guard: matchers test `tool_name` only, so a matcher on
            // an event whose context has no tool name silently never fires.
            // Warn at load time rather than leaving a mysteriously-dead hook.
            if matcher.is_some() && !event_supports_matcher(event) {
                warn!(
                    path = %path.display(),
                    event = %event_str,
                    "Hook `matcher` set on an event with no tool name; matchers test \
                     tool_name only, so this hook will never fire — drop the matcher \
                     to fire on every occurrence of this event"
                );
            }
            // Second foot-gun: interceptor-kind hooks only run on events whose
            // fire-sites dispatch interceptors; the global fire-and-forget
            // seams (messages / provider / gateway / subagent…) run observers
            // only, so an explicit `"kind": "interceptor"` there is dead.
            if kind == HookKind::Interceptor && !event_supports_interceptor(event) {
                warn!(
                    path = %path.display(),
                    event = %event_str,
                    "Hook kind `interceptor` set on an event whose fire-site runs \
                     observers only — this hook will never execute; use \
                     `\"kind\": \"observer\"` (or drop the kind) for this event"
                );
            }

            out.push(HookConfig {
                event,
                kind,
                priority,
                matcher,
                actions,
                plugin_name: source_label.to_string(),
                plugin_root: plugin_root.clone(),
                handler: None,
                timeout_secs,
            });
        }
    }
}

fn a_or_group_timeout(g: &UserHookGroup) -> Option<u64> {
    g.hooks
        .iter()
        .find_map(|a| match a {
            UserHookAction::Command { timeout_secs, .. } => *timeout_secs,
            UserHookAction::Http { timeout_secs, .. } => *timeout_secs,
            _ => None,
        })
        .or(g.timeout_secs)
}

/// Map both Claude Code-style (`PreToolUse`) and Aleph-style
/// (`before_tool_call`) event names to [`HookEvent`].
fn parse_event(name: &str) -> Option<HookEvent> {
    // Re-uses the serde aliases on HookEvent. snake_case → primary; PascalCase
    // → alias. Falls back to `from_str` via JSON deserialization.
    let attempts = [
        name.to_string(),
        name.to_lowercase().replace('-', "_"),
        format!("\"{name}\""),
    ];
    for s in &attempts {
        if let Ok(ev) = serde_json::from_str::<HookEvent>(s) {
            return Some(ev);
        }
        if let Ok(ev) = serde_json::from_str::<HookEvent>(&format!("\"{s}\"")) {
            return Some(ev);
        }
    }
    None
}

/// Pick a sensible default `HookKind` based on the event semantics:
/// blocking-capable events default to Interceptor so a `block:` line in
/// the hook output actually stops the relevant flow; passive lifecycle
/// events default to Observer so they don't accidentally short-circuit
/// when the user just wants logging.
///
/// `Stop` defaults to Interceptor because its whole purpose is gating the
/// loop's stop (`ExtensionStopHookVerifier`).
///
/// `SessionStart` deliberately stays **Observer** by default even though its
/// fire-site now also harvests interceptor output: a pre-existing SessionStart
/// hook that omitted `kind` was fire-and-forget with stdout discarded, and
/// silently flipping it to Interceptor would start injecting that stdout into
/// the model context (and run it sequentially before the first turn). A user
/// who WANTS SessionStart context injection opts in with `"kind":
/// "interceptor"`.
pub(crate) const fn default_kind_for_event(event: HookEvent) -> HookKind {
    use HookEvent::*;
    match event {
        BeforeToolCall | BeforeAgentStart | UserPromptSubmit | Stop => HookKind::Interceptor,
        _ => HookKind::Observer,
    }
}

/// Whether an event's production fire-site dispatches interceptor-kind
/// hooks. The global fire-and-forget seams (`fire_global_observer`: message
/// / provider / gateway / subagent / permission events) run observers only —
/// an interceptor registered there never executes. Used for a load-time
/// foot-gun warning, mirroring `event_supports_matcher`.
const fn event_supports_interceptor(event: HookEvent) -> bool {
    use HookEvent::*;
    matches!(
        event,
        BeforeToolCall
            | AfterToolCall
            | AfterToolCallFailure
            | BeforeAgentStart
            | UserPromptSubmit
            | SessionStart
            | AgentEnd
            | BeforeCompaction
            | Stop
    )
}

/// Whether an event's [`HookContext`] carries a `tool_name`, so a `matcher`
/// regex can meaningfully select among invocations. The hook executor matches
/// the `matcher` against `tool_name` only; on any other event the matcher can
/// never match and the hook silently never fires. Used to surface that
/// foot-gun as a load-time warning.
const fn event_supports_matcher(event: HookEvent) -> bool {
    use HookEvent::*;
    matches!(
        event,
        BeforeToolCall
            | AfterToolCall
            | AfterToolCallFailure
            | ToolResultPersist
            | PermissionRequest
            | Notification
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn event_supports_matcher_only_for_tool_name_events() {
        // Tool-name-bearing events accept a matcher.
        assert!(event_supports_matcher(HookEvent::BeforeToolCall));
        assert!(event_supports_matcher(HookEvent::AfterToolCall));
        assert!(event_supports_matcher(HookEvent::AfterToolCallFailure));
        assert!(event_supports_matcher(HookEvent::ToolResultPersist));
        assert!(event_supports_matcher(HookEvent::PermissionRequest));
        assert!(event_supports_matcher(HookEvent::Notification));
        // Lifecycle events have no tool name; a matcher there never fires.
        assert!(!event_supports_matcher(HookEvent::SessionStart));
        assert!(!event_supports_matcher(HookEvent::BeforeAgentStart));
        assert!(!event_supports_matcher(HookEvent::UserPromptSubmit));
        assert!(!event_supports_matcher(HookEvent::AgentEnd));
    }

    #[test]
    fn matcher_on_non_tool_event_still_loads_but_is_a_footgun() {
        // The hook still loads (back-compat); we only warn. The matcher is
        // preserved so behavior is unchanged — it simply will not match.
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(
            &cfg,
            r#"{
                "hooks": {
                    "SessionStart": [
                        { "matcher": "anything",
                          "hooks": [
                            { "type": "command", "command": "echo hi" }
                          ]
                        }
                    ]
                }
            }"#,
        );
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].event, HookEvent::SessionStart);
        assert_eq!(out[0].matcher.as_deref(), Some("anything"));
    }

    #[test]
    fn loads_pre_tool_use_with_matcher() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(
            &cfg,
            r#"{
                "hooks": {
                    "PreToolUse": [
                        { "matcher": "Edit|Write",
                          "hooks": [
                            { "type": "command", "command": "echo hi", "timeout_secs": 30 }
                          ]
                        }
                    ]
                }
            }"#,
        );
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &mut out);
        assert_eq!(out.len(), 1);
        let h = &out[0];
        assert_eq!(h.event, HookEvent::BeforeToolCall);
        assert_eq!(h.kind, HookKind::Interceptor);
        assert_eq!(h.matcher.as_deref(), Some("Edit|Write"));
        assert_eq!(h.timeout_secs, Some(30));
        assert!(matches!(h.actions[0], HookAction::Command { .. }));
    }

    #[test]
    fn loads_http_hook() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(
            &cfg,
            r#"{
                "hooks": {
                    "PostToolUse": [
                        { "hooks": [
                            { "type": "http", "url": "https://audit.example/log",
                              "headers": { "x-token": "secret-redacted" } }
                          ]
                        }
                    ]
                }
            }"#,
        );
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].event, HookEvent::AfterToolCall);
        assert_eq!(out[0].kind, HookKind::Observer);
        match &out[0].actions[0] {
            HookAction::Http { url, headers } => {
                assert_eq!(url, "https://audit.example/log");
                assert_eq!(
                    headers.get("x-token").map(String::as_str),
                    Some("secret-redacted")
                );
            }
            _ => panic!("expected http action"),
        }
    }

    #[test]
    fn malformed_file_is_skipped() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(&cfg, "not json");
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn unknown_event_is_skipped() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(
            &cfg,
            r#"{ "hooks": { "BogusEvent": [
                { "hooks": [{ "type": "command", "command": "x" }] }
            ] } }"#,
        );
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &mut out);
        assert!(out.is_empty());
    }

    const ONE_HOOK: &str = r#"{ "hooks": { "PreToolUse": [
        { "hooks": [{ "type": "command", "command": "echo p" }] }
    ] } }"#;

    fn count_project_hooks(hooks: &[HookConfig]) -> usize {
        hooks
            .iter()
            .filter(|h| h.plugin_name == "user:project")
            .count()
    }

    #[test]
    fn loads_hooks_from_registered_projects() {
        // App mode: no project hooks in the daemon CWD, but a registered
        // project carries one — it must still be discovered.
        let cwd = tempdir().unwrap();
        let proj = tempdir().unwrap();
        write(&proj.path().join(".aleph/hooks.json"), ONE_HOOK);

        let roots = vec![proj.path().to_path_buf()];
        let hooks = load_user_hooks(Some(cwd.path()), &roots);
        assert_eq!(count_project_hooks(&hooks), 1);
        assert_eq!(
            hooks[0].plugin_root,
            proj.path().join(".aleph"),
            "project hook must carry its own .aleph as plugin_root for the fire-time gate"
        );
    }

    #[test]
    fn cwd_that_is_also_a_registered_project_loads_once() {
        // The daemon CWD and a registered project resolve to the same folder;
        // a double-load would fire its commands twice per event.
        let proj = tempdir().unwrap();
        write(&proj.path().join(".aleph/hooks.json"), ONE_HOOK);

        let roots = vec![proj.path().to_path_buf()];
        let hooks = load_user_hooks(Some(proj.path()), &roots);
        assert_eq!(count_project_hooks(&hooks), 1, "must dedup CWD vs registry");
    }

    #[test]
    fn distinct_cwd_and_project_both_contribute() {
        let cwd = tempdir().unwrap();
        let proj = tempdir().unwrap();
        write(&cwd.path().join(".aleph/hooks.json"), ONE_HOOK);
        write(&proj.path().join(".aleph/hooks.json"), ONE_HOOK);

        let roots = vec![proj.path().to_path_buf()];
        let hooks = load_user_hooks(Some(cwd.path()), &roots);
        assert_eq!(count_project_hooks(&hooks), 2);
    }
}
