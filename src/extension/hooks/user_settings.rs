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
//! Layers 2–4 are tagged `user:project` / `user:project-local` (logs) and
//! stamped `ScopeKey::Project(root)`; the
//! [`HookExecutor`](super::executor::HookExecutor) gates them at fire time
//! through `visibility::visible_to` so a hook checked into project A never
//! runs while the agent works inside project B, and consent keys them by
//! that same root, so approving it in A does not approve B's copy
//! (`consent.rs`). Layer 1 (`user:global`) is stamped `ScopeKey::Global` and
//! always fires.
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
use crate::extension::visibility::{canonical_root, ScopeKey};

/// One contiguous group from a `hooks.json` file.
#[derive(Debug, Clone, Deserialize)]
struct UserHookGroup {
    /// Optional regex tested against the event's subject
    /// (`HookEvent::match_subject`: the tool name, SessionStart's source, or
    /// nothing — then it is ignored). Empty / `*` / missing = match all.
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
        /// Claude Code spells it `timeout`, as a plugin's `hooks.json` does
        /// (`manifest::parsers::HookAction`) — take either.
        #[serde(default, alias = "timeout")]
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
        #[serde(default, alias = "timeout")]
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

    // `ALEPH_HOME`-aware, like every other reader of Aleph state — and like the
    // sibling `ShellHookConsent::default_path` that gates these same hooks.
    // A user-global layer that reads the real home under a relocated one is a
    // silently empty layer.
    if let Ok(home) = crate::utils::paths::get_config_dir() {
        let p = home.join("hooks.json");
        load_into(&p, "user:global", &ScopeKey::Global, &mut out);
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

/// Best-effort canonicalisation for path-equality bookkeeping. A thin alias
/// of the shared visibility derivation so the dedup key and the
/// [`ScopeKey`] stamped on each row cannot canonicalise differently.
fn canonical(p: &Path) -> PathBuf {
    canonical_root(p)
}

/// The owner labels of a project's two hook files, checked-in and gitignored.
/// Every project's files load under these same two labels, so a label never
/// says WHICH project a hook belongs to — its `ScopeKey::Project` does, and
/// that key is what the fire-time gate and consent (`consent.rs`) read.
pub(crate) const PROJECT_LABELS: [&str; 2] = ["user:project", "user:project-local"];

/// Load a project directory's checked-in + gitignored hook files. Both are
/// tagged `user:project*` for logs and stamped `Project(root)` so the
/// executor's fire-time gate (`visible_to`) and the consent key bind them to
/// this project.
fn load_project_layer(root: &Path, out: &mut Vec<HookConfig>) {
    let key = ScopeKey::project(root);
    let [checked_in, local] = PROJECT_LABELS;
    load_into(&root.join(".aleph/hooks.json"), checked_in, &key, out);
    load_into(&root.join(".aleph/hooks.local.json"), local, &key, out);
}

fn load_into(path: &Path, source_label: &str, scope: &ScopeKey, out: &mut Vec<HookConfig>) {
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
            if g.hooks.is_empty() {
                continue;
            }

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
            // A matcher that never fires, or one this event ignores, is said
            // at load time — the same notice a plugin's `hooks.json` gives.
            super::warn_on_matcher(
                &path.display().to_string(),
                &event_str,
                event,
                matcher.as_deref(),
            );
            // Second foot-gun: interceptor-kind hooks only run on events whose
            // fire-sites dispatch interceptors; the global fire-and-forget
            // seams (messages / provider / gateway / subagent…) run observers
            // only, so an explicit `"kind": "interceptor"` there is dead.
            if kind == HookKind::Interceptor && !event.supports_interceptor() {
                warn!(
                    path = %path.display(),
                    event = %event_str,
                    "Hook kind `interceptor` set on an event whose fire-site runs \
                     observers only — this hook will never execute; use \
                     `\"kind\": \"observer\"` (or drop the kind) for this event"
                );
            }

            // Emit ONE `HookConfig` per action, each carrying its OWN
            // `timeout_secs`. `HookConfig` holds a single timeout, so folding a
            // multi-action group into one registration made the FIRST action's
            // timeout leak onto its siblings — a group of
            // `[{cmd: fast, timeout_secs: 5}, {cmd: slow, timeout_secs: 600}]`
            // gave both 5s and the slow one always "timed out". The plugin
            // `hooks.json` path (`manifest/parsers.rs`) was fixed this way
            // already; this is the same fix on the user-config path.
            for a in &g.hooks {
                let (action, timeout_secs) = match a {
                    UserHookAction::Command {
                        command,
                        timeout_secs,
                    } => (
                        HookAction::Command {
                            command: command.clone(),
                        },
                        timeout_secs.or(g.timeout_secs),
                    ),
                    UserHookAction::Http {
                        url,
                        headers,
                        timeout_secs,
                    } => (
                        HookAction::Http {
                            url: url.clone(),
                            headers: headers.clone(),
                        },
                        timeout_secs.or(g.timeout_secs),
                    ),
                    // Prompt / Agent actions resolve in-process: they never
                    // spawn or await anything, so a timeout is meaningless.
                    UserHookAction::Prompt { prompt } => (
                        HookAction::Prompt {
                            prompt: prompt.clone(),
                        },
                        None,
                    ),
                    UserHookAction::Agent { agent } => (
                        HookAction::Agent {
                            agent: agent.clone(),
                        },
                        None,
                    ),
                };

                out.push(HookConfig {
                    event,
                    kind,
                    priority,
                    matcher: matcher.clone(),
                    actions: vec![action],
                    plugin_name: source_label.to_string(),
                    plugin_root: plugin_root.clone(),
                    handler: None,
                    timeout_secs,
                    declared_event: Some(event_str.clone()),
                    scope_key: scope.clone(),
                });
            }
        }
    }
}

/// Map both Claude Code-style (`PreToolUse`) and Aleph-style
/// (`before_tool_call`) event names to [`HookEvent`].
///
/// Shared by the three file readers — this loader, a plugin's `hooks.json`
/// (`manifest::parsers::parse_hooks_content`) and `aleph.plugin.toml`
/// `[[hooks]]` (`parse_v2_hooks`) — so a name one of them accepts, the
/// others accept too.
pub(crate) fn parse_event(name: &str) -> Option<HookEvent> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// The subject census: what a matcher is tested against, per event.
    #[test]
    fn a_matcher_is_tested_against_the_events_subject() {
        use crate::extension::types::MatchSubject::{Ignored, SessionSource, ToolName};
        for event in [
            HookEvent::BeforeToolCall,
            HookEvent::AfterToolCall,
            HookEvent::AfterToolCallFailure,
            HookEvent::ToolResultPersist,
            HookEvent::PermissionRequest,
            HookEvent::PermissionDenied,
            HookEvent::Notification,
        ] {
            assert_eq!(event.match_subject(), ToolName, "{event:?}");
        }
        assert_eq!(HookEvent::SessionStart.match_subject(), SessionSource);
        for event in [
            HookEvent::BeforeAgentStart,
            HookEvent::UserPromptSubmit,
            HookEvent::AgentEnd,
            HookEvent::Stop,
            HookEvent::BeforeCompaction,
        ] {
            assert_eq!(event.match_subject(), Ignored, "{event:?}");
        }
    }

    /// Retired `matcher_on_non_tool_event_still_loads_but_is_a_footgun`: a
    /// matcher on an event with nothing to match is no longer a hook that
    /// never fires. It loads with its matcher as written, and the executor
    /// ignores it — the hook runs on every occurrence.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_matcher_on_an_event_with_nothing_to_match_is_ignored() {
        let dir = tempdir().unwrap();
        let marker = dir.path().join("fired");
        let cfg = dir.path().join(".aleph/hooks.json");
        write(
            &cfg,
            &serde_json::json!({"hooks": {"UserPromptSubmit": [{
                "matcher": "anything",
                "hooks": [{"type": "command", "command": format!("touch '{}'", marker.display())}]
            }]}})
            .to_string(),
        );
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &ScopeKey::Global, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].matcher.as_deref(), Some("anything"));
        crate::extension::hooks::HookExecutor::new(out)
            .execute_interceptors(
                HookEvent::UserPromptSubmit,
                crate::extension::hooks::HookContext::new("s"),
            )
            .await
            .expect("the hook runs");
        assert!(marker.exists(), "an ignored matcher must not stop the hook");
    }

    /// P4.14 F-3: Claude Code spells an action's timeout `timeout`, and a
    /// plugin's `hooks.json` already takes it. `~/.aleph/hooks.json` takes
    /// both spellings, for `command` and `http` alike.
    #[test]
    fn an_actions_timeout_takes_either_spelling() {
        for key in ["timeout", "timeout_secs"] {
            let dir = tempdir().unwrap();
            let cfg = dir.path().join(".aleph/hooks.json");
            write(
                &cfg,
                &serde_json::json!({"hooks": {"PreToolUse": [{"hooks": [
                    {"type": "command", "command": "true", key: 30},
                    {"type": "http", "url": "http://127.0.0.1:9/h", key: 31}
                ]}]}})
                .to_string(),
            );
            let mut out = Vec::new();
            load_into(&cfg, "user:global", &ScopeKey::Global, &mut out);
            let timeouts: Vec<Option<u64>> = out.iter().map(|h| h.timeout_secs).collect();
            assert_eq!(timeouts, [Some(30), Some(31)], "{key}");
        }
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
        load_into(&cfg, "user:project", &ScopeKey::Global, &mut out);
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
        load_into(&cfg, "user:project", &ScopeKey::Global, &mut out);
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
    fn each_action_keeps_its_own_timeout() {
        // Regression lock: folding a multi-action group into ONE HookConfig
        // leaked the first action's `timeout_secs` onto its siblings, so the
        // slow command inherited the fast one's 5s deadline and always
        // "timed out". Each action must become its own registration.
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(
            &cfg,
            r#"{
                "hooks": {
                    "PreToolUse": [
                        { "hooks": [
                            { "type": "command", "command": "fast", "timeout_secs": 5 },
                            { "type": "command", "command": "slow", "timeout_secs": 600 }
                          ]
                        }
                    ]
                }
            }"#,
        );
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &ScopeKey::Global, &mut out);
        assert_eq!(out.len(), 2, "one registration per action");
        assert_eq!(out[0].timeout_secs, Some(5));
        assert_eq!(out[1].timeout_secs, Some(600));
        // Group-level metadata is copied onto every split registration.
        assert!(out
            .iter()
            .all(|h| h.event == HookEvent::BeforeToolCall && h.kind == HookKind::Interceptor));
    }

    #[test]
    fn action_timeout_falls_back_to_the_group_default() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(
            &cfg,
            r#"{
                "hooks": {
                    "PreToolUse": [
                        { "timeout_secs": 42,
                          "hooks": [
                            { "type": "command", "command": "inherits" },
                            { "type": "command", "command": "overrides", "timeout_secs": 7 },
                            { "type": "prompt", "prompt": "no timeout here" }
                          ]
                        }
                    ]
                }
            }"#,
        );
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &ScopeKey::Global, &mut out);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].timeout_secs, Some(42), "inherits group default");
        assert_eq!(out[1].timeout_secs, Some(7), "own value wins");
        assert_eq!(
            out[2].timeout_secs, None,
            "prompt actions resolve in-process; a timeout is meaningless"
        );
    }

    #[test]
    fn group_with_no_actions_registers_nothing() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(
            &cfg,
            r#"{ "hooks": { "PreToolUse": [ { "matcher": "Write", "hooks": [] } ] } }"#,
        );
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &ScopeKey::Global, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn malformed_file_is_skipped() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(&cfg, "not json");
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &ScopeKey::Global, &mut out);
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
        load_into(&cfg, "user:project", &ScopeKey::Global, &mut out);
        assert!(out.is_empty());
    }

    /// U-b: the key as written is what the hook's payload will echo, so the
    /// loader keeps it — both spellings, side by side, each on its own row.
    #[test]
    fn each_row_keeps_the_event_spelling_its_author_wrote() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".aleph/hooks.json");
        write(
            &cfg,
            r#"{ "hooks": {
                "PreToolUse": [{ "hooks": [{ "type": "command", "command": "a" }] }],
                "before_tool_call": [{ "hooks": [{ "type": "command", "command": "b" }] }]
            } }"#,
        );
        let mut out = Vec::new();
        load_into(&cfg, "user:project", &ScopeKey::Global, &mut out);
        let mut seen: Vec<(HookEvent, Option<String>)> = out
            .iter()
            .map(|h| (h.event, h.declared_event.clone()))
            .collect();
        seen.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(
            seen,
            vec![
                (HookEvent::BeforeToolCall, Some("PreToolUse".to_string())),
                (
                    HookEvent::BeforeToolCall,
                    Some("before_tool_call".to_string())
                ),
            ]
        );
        assert_eq!(out[0].event_name(), out[0].declared_event.clone().unwrap());
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
        // Picked by layer, not by position: the global layer loads first and
        // reads the real config dir, which may hold a `hooks.json` of its own
        // (another test's isolated home, or a developer's `~/.aleph`).
        let project_hook = hooks
            .iter()
            .find(|h| h.plugin_name == "user:project")
            .expect("the project hook");
        assert_eq!(
            project_hook.plugin_root,
            proj.path().join(".aleph"),
            "project hook must carry its own .aleph as plugin_root for variable substitution"
        );
        assert_eq!(
            project_hook.scope_key,
            ScopeKey::project(proj.path()),
            "project hook must carry its project's key for the fire-time gate"
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

    /// The producer stamps the key: a project layer file yields `Project(root)`
    /// rows, the global file yields `Global` rows. Without the stamp the
    /// executor's gate has nothing to compare and every project hook would
    /// silently become global (fail-open).
    #[test]
    fn project_layer_rows_carry_their_project_key_and_global_rows_carry_global() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".aleph")).unwrap();
        std::fs::write(
            root.path().join(".aleph/hooks.json"),
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        load_project_layer(root.path(), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].scope_key, ScopeKey::project(root.path()));

        let global = root.path().join("hooks.json");
        std::fs::write(
            &global,
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"true"}]}]}}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        load_into(&global, "user:global", &ScopeKey::Global, &mut out);
        assert_eq!(out[0].scope_key, ScopeKey::Global);
    }
}
