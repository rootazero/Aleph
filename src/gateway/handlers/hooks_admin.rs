//! User-level hooks file CRUD over JSON-RPC.
//!
//! Reads and writes `~/.aleph/hooks.json` — the global user hooks file
//! that `extension::hooks::load_user_hooks` parses on every reload.
//! Project-scoped hook files (`<cwd>/.aleph/hooks.json` etc.) are
//! intentionally edit-by-hand: they live inside repos, get committed,
//! and shouldn't be mutated by a remote operator.
//!
//! All writes go through [`ExtensionManager::mark_self_write`] first so
//! the disk-watcher doesn't trigger a redundant reload — the handler
//! triggers the reload itself once the write succeeds.
//!
//! Two views, deliberately separate:
//!
//! - `hooks.list` is the **file** view — exactly what is on disk in
//!   `~/.aleph/hooks.json`, which is what `hooks.add` / `hooks.remove` edit.
//! - `hooks.registry` is the **runtime** view — every hook the running server
//!   has actually registered, across all four layers (global / project /
//!   project-local / plugin-shipped), with the resolved `kind`, consent state,
//!   and a per-hook reachability verdict. This is the one that answers "why
//!   doesn't my hook fire?"; the file view structurally cannot, because three
//!   of the four layers never appear in it.
//!
//! RPCs:
//! - `hooks.list` → `{ events: { EventName: [Group] }, path, exists }`
//! - `hooks.registry` → `{ hooks: [HookInventoryEntry], total, unreachable }`
//! - `hooks.add` (params: `{ event, command|prompt|agent|http, matcher?, timeout_secs? }`)
//!   → appends one entry; returns the same shape as `hooks.list`, plus
//!   `matcher_warning` when the matcher never fires or its event ignores it
//!   (`hooks::matcher_notice`, the text `hooks_manage add` gives too)
//! - `hooks.remove` (params: `{ event, command? | index? }`)
//!   → filters out matching entries; returns the new view
//! - `hooks.reload` → forces a full extension-manager reload from disk;
//!   returns `{ user_hooks_count }`
//! - `hooks.events` → `{ events: [String] }` — list of valid event names

use crate::sync_primitives::Arc;
use std::fs;
use std::path::PathBuf;

use crate::extension::{try_extension_manager, ExtensionManager, HookEvent};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use tracing::warn;

const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;
const SERVICE_UNAVAILABLE: i32 = -32011;

/// Resolve the global hooks file path. Returns the path even if the
/// file does not yet exist, so writes can create it.
fn user_hooks_path() -> Result<PathBuf, String> {
    // `get_config_dir`, not `dirs::home_dir()`: the *reader* of this same file
    // (`extension::hooks::user_settings::load_user_hooks`) is `ALEPH_HOME`-aware
    // and its own comment names the failure — "a user-global layer that reads
    // the real home under a relocated one is a silently empty layer". Writing
    // it by hand here made this RPC the writing half of exactly that split:
    // every hook added through the admin surface landed in a file nothing
    // loaded, with no error on either side.
    let home = crate::utils::paths::get_config_dir().map_err(|e| e.to_string())?;
    Ok(home.join("hooks.json"))
}

/// Read `~/.aleph/hooks.json` as `{ "hooks": { ... } }`. A missing file
/// returns an empty hooks map instead of an error so first-write paths
/// don't need a separate codepath.
fn read_hooks_file() -> Result<(PathBuf, bool, Map<String, Value>), String> {
    let path = user_hooks_path()?;
    if !path.exists() {
        return Ok((path, false, Map::new()));
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let parsed: Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
    let events = parsed
        .get("hooks")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    Ok((path, true, events))
}

/// Write the events map back out, preserving any other top-level keys
/// the user may have added (forwards-compatibility). The write is
/// announced to the extension watcher *before* the disk operation so
/// the hot-reload doesn't double-fire.
fn write_hooks_file(path: &PathBuf, events: &Map<String, Value>) -> Result<(), String> {
    let mut top = Map::new();
    if path.exists() {
        if let Ok(raw) = fs::read_to_string(path) {
            if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(&raw) {
                top = obj;
            }
        }
    }
    top.insert("hooks".into(), Value::Object(events.clone()));
    let serialised = serde_json::to_string_pretty(&Value::Object(top))
        .map_err(|e| format!("serialise hooks: {e}"))?;

    if let Some(mgr) = try_extension_manager() {
        let mgr: &Arc<ExtensionManager> = mgr;
        mgr.mark_self_write(path.as_path());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    // Write atomically: temp + rename so a partial write never corrupts
    // the file readers see during the swap.
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serialised).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))?;
    Ok(())
}

fn list_response(path: PathBuf, exists: bool, events: Map<String, Value>) -> Value {
    json!({
        "path": path.display().to_string(),
        "exists": exists,
        "events": Value::Object(events),
    })
}

pub async fn handle_hooks_list(request: JsonRpcRequest) -> JsonRpcResponse {
    match tokio::task::spawn_blocking(read_hooks_file).await {
        Ok(Ok((path, exists, events))) => {
            JsonRpcResponse::success(request.id, list_response(path, exists, events))
        }
        Ok(Err(e)) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("task join: {e}")),
    }
}

#[derive(Debug, Deserialize)]
struct AddParams {
    event: String,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    matcher: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

/// Build a single action JSON object from the (command|prompt|agent|url)
/// quadruple. Exactly one of those four must be set.
///
/// Shared with the `hooks_manage` tool so the RPC and the conversational
/// path write byte-identical entries and validate identically — the tool
/// must not grow a second, subtly-different notion of a valid hook action.
pub(crate) fn build_hook_action(
    command: Option<&str>,
    prompt: Option<&str>,
    agent: Option<&str>,
    url: Option<&str>,
    timeout_secs: Option<u64>,
) -> Result<Value, String> {
    let count = [command, prompt, agent, url]
        .iter()
        .filter(|f| f.is_some())
        .count();
    if count == 0 {
        return Err("must set one of: command, prompt, agent, url".into());
    }
    if count > 1 {
        return Err("only one of command/prompt/agent/url may be set".into());
    }
    if let Some(c) = command {
        let mut obj = Map::new();
        obj.insert("type".into(), json!("command"));
        obj.insert("command".into(), json!(c));
        if let Some(t) = timeout_secs {
            obj.insert("timeout_secs".into(), json!(t));
        }
        return Ok(Value::Object(obj));
    }
    if let Some(pr) = prompt {
        return Ok(json!({ "type": "prompt", "prompt": pr }));
    }
    if let Some(a) = agent {
        return Ok(json!({ "type": "agent", "agent": a }));
    }
    let u = url.ok_or_else(|| "must set one of: command, prompt, agent, url".to_string())?;
    let mut obj = Map::new();
    obj.insert("type".into(), json!("http"));
    obj.insert("url".into(), json!(u));
    if let Some(t) = timeout_secs {
        obj.insert("timeout_secs".into(), json!(t));
    }
    Ok(Value::Object(obj))
}

/// Read the global hooks file. Public alias for the `hooks_manage` tool's
/// `show_file` action.
pub(crate) fn read_user_hooks_file() -> Result<(PathBuf, bool, Map<String, Value>), String> {
    read_hooks_file()
}

/// Process-wide guard serializing every hooks-file mutation (append or
/// remove) so concurrent writers cannot clobber each other's read-modify-write
/// cycles. Shared by `append_user_hook`, `remove_user_hooks`, and
/// `handle_hooks_remove`.
static HOOKS_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Append one action (optionally matcher-scoped) to `event` in the global
/// hooks file, then trigger a reload. Extracted from `handle_hooks_add` so
/// the RPC and the tool share one write path — including the atomic write,
/// the self-write marker, and the reload trigger.
pub(crate) fn append_user_hook(
    event: &str,
    action: Value,
    matcher: Option<&str>,
) -> Result<(), String> {
    let _guard = HOOKS_MUTEX.lock().unwrap();

    let (path, _exists, mut events) = read_hooks_file()?;

    let group = {
        let mut g = Map::new();
        if let Some(m) = matcher.filter(|m| !m.is_empty()) {
            g.insert("matcher".into(), Value::String(m.to_string()));
        }
        g.insert("hooks".into(), Value::Array(vec![action]));
        Value::Object(g)
    };

    let entry = events
        .entry(event.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let arr = entry
        .as_array_mut()
        .ok_or_else(|| format!("hooks.{event} is not an array in {}", path.display()))?;
    arr.push(group);

    write_hooks_file(&path, &events)?;
    trigger_reload();
    Ok(())
}

/// What to remove from an event's entries.
pub(crate) enum HookRemoval {
    /// Every group filed under the event.
    All,
    /// The groups whose action fields contain the needle.
    Command(String),
    /// The group at this position in the flattened, canonically ordered list
    /// of the event's groups.
    Index(usize),
}

/// Drop groups from every key of the hooks file that names `event`.
///
/// Keys are written as given (`PreToolUse` and `before_tool_call` can both
/// exist), so an exact-key lookup would miss the groups filed under the
/// other spelling. Both removal faces — the RPC and the `hooks_manage` tool
/// — resolve the event the same way here, so they cannot drift apart.
/// Returns how many groups were removed.
pub(crate) fn remove_event_groups(
    events: &mut Map<String, Value>,
    event: &str,
    removal: HookRemoval,
) -> Result<usize, String> {
    let target = crate::extension::hooks::parse_event(event)
        .ok_or_else(|| format!("unknown event: {event}"))?;
    let mut keys: Vec<String> = events
        .keys()
        .filter(|key| crate::extension::hooks::parse_event(key) == Some(target))
        .cloned()
        .collect();
    // Canonical order: the `index` arm has to address the same group no
    // matter how the file happens to be ordered.
    keys.sort();
    if keys.is_empty() {
        return Err(format!("no entries for event {event}"));
    }

    let mut removed = 0;
    match removal {
        HookRemoval::All => {
            for key in &keys {
                let Some(arr) = events.get_mut(key).and_then(|v| v.as_array_mut()) else {
                    continue;
                };
                removed += arr.len();
                arr.clear();
            }
        }
        HookRemoval::Command(needle) => {
            for key in &keys {
                let Some(arr) = events.get_mut(key).and_then(|v| v.as_array_mut()) else {
                    continue;
                };
                let before = arr.len();
                arr.retain(|grp| !group_matches_substring(grp, &needle));
                removed += before - arr.len();
            }
        }
        HookRemoval::Index(idx) => {
            // The index addresses the flattened list of the event's groups:
            // every group under every key that names the event.
            let total: usize = keys
                .iter()
                .map(|key| {
                    events
                        .get(key)
                        .and_then(|v| v.as_array())
                        .map_or(0, |arr| arr.len())
                })
                .sum();
            if idx >= total {
                return Err(format!("index {idx} out of range (len={total})"));
            }
            let mut seen = 0;
            for key in &keys {
                let len = events
                    .get(key)
                    .and_then(|v| v.as_array())
                    .map_or(0, |arr| arr.len());
                if idx < seen + len {
                    if let Some(arr) = events.get_mut(key).and_then(|v| v.as_array_mut()) {
                        arr.remove(idx - seen);
                        removed += 1;
                    }
                    break;
                }
                seen += len;
            }
        }
    }

    // Don't leave empty arrays cluttering the file.
    for key in &keys {
        let empty = events
            .get(key)
            .and_then(|v| v.as_array())
            .is_some_and(|arr| arr.is_empty());
        if empty {
            events.remove(key);
        }
    }
    Ok(removed)
}

/// Remove entries for `event` from the global user hooks file.
///
/// `needle` filters by substring across the group's action fields; `None`
/// removes every entry for the event. Returns how many groups were removed.
pub(crate) fn remove_user_hooks(event: &str, needle: Option<&str>) -> Result<usize, String> {
    // Same guard as `append_user_hook`: the read-modify-write of the hooks
    // file must not interleave with a concurrent append.
    let _guard = HOOKS_MUTEX.lock().unwrap();

    let (path, _exists, mut events) = read_hooks_file()?;
    let removal = match needle {
        Some(n) => HookRemoval::Command(n.to_string()),
        None => HookRemoval::All,
    };
    let removed = remove_event_groups(&mut events, event, removal)?;
    write_hooks_file(&path, &events)?;
    trigger_reload();
    Ok(removed)
}

pub async fn handle_hooks_add(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: AddParams = match request
        .params
        .as_ref()
        .map(|p| serde_json::from_value::<AddParams>(p.clone()))
    {
        Some(Ok(p)) => p,
        Some(Err(e)) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            );
        }
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "missing params"),
    };

    // Validate the event name against the canonical HookEvent enum so we
    // don't write entries the loader will silently drop.
    if !is_known_event(&params.event) {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            format!("unknown event: {}", params.event),
        );
    }

    let action = match build_hook_action(
        params.command.as_deref(),
        params.prompt.as_deref(),
        params.agent.as_deref(),
        params.url.as_deref(),
        params.timeout_secs,
    ) {
        Ok(a) => a,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };

    if let Err(e) = append_user_hook(&params.event, action, params.matcher.as_deref()) {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e);
    }
    // A matcher that never fires, or one its event ignores, is said — the
    // same notice the tool twin (`hooks_manage add`) and every loader give.
    let matcher_warning = crate::extension::hooks::parse_event(&params.event).and_then(|event| {
        crate::extension::hooks::matcher_notice(event, params.matcher.as_deref())
    });

    match read_hooks_file() {
        Ok((path, exists, events)) => {
            let mut body = list_response(path, exists, events);
            if let Some(warning) = matcher_warning {
                body["matcher_warning"] = Value::String(warning);
            }
            JsonRpcResponse::success(request.id, body)
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    }
}

#[derive(Debug, Deserialize)]
struct RemoveParams {
    event: String,
    /// Substring match against the command/url/prompt/agent payload.
    /// When omitted, `index` must be set.
    #[serde(default)]
    command: Option<String>,
    /// Zero-based index of the group to drop, counted over the flattened
    /// list of the event's groups: every group under every key that names
    /// the event, the keys in sorted order.
    #[serde(default)]
    index: Option<usize>,
}

pub async fn handle_hooks_remove(request: JsonRpcRequest) -> JsonRpcResponse {
    let params: RemoveParams = match request
        .params
        .as_ref()
        .map(|p| serde_json::from_value::<RemoveParams>(p.clone()))
    {
        Some(Ok(p)) => p,
        Some(Err(e)) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            );
        }
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "missing params"),
    };
    // `index` wins over `command`: it addresses one specific group.
    let removal = match (params.index, params.command) {
        (Some(idx), _) => HookRemoval::Index(idx),
        (None, Some(needle)) => HookRemoval::Command(needle),
        (None, None) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "must set either `command` (substring filter) or `index`",
            );
        }
    };

    // The read-modify-write has to be one critical section. `hooks.add`
    // appends under this same lock, so a concurrent add that lands between
    // our read and our write would have its hook overwritten by ours — both
    // calls would report success and one hook would be silently gone.
    let _guard = HOOKS_MUTEX.lock().unwrap();

    let (path, _exists, mut events) = match read_hooks_file() {
        Ok(v) => v,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    };

    // The removal resolves the event aliases the same way the `hooks_manage`
    // tool does, so a hook filed under `PreToolUse` is also removed by the
    // canonical `before_tool_call` spelling.
    let removed = match remove_event_groups(&mut events, &params.event, removal) {
        Ok(n) => n,
        Err(e) => return JsonRpcResponse::error(request.id, INVALID_PARAMS, e),
    };

    if let Err(e) = write_hooks_file(&path, &events) {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e);
    }
    drop(_guard);
    trigger_reload();
    let mut payload = list_response(path, true, events);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("removed".into(), json!(removed));
    }
    JsonRpcResponse::success(request.id, payload)
}

/// Return true if any action inside `group.hooks` contains `needle` in
/// its `command` / `url` / `prompt` / `agent` field.
fn group_matches_substring(group: &Value, needle: &str) -> bool {
    let actions = group
        .get("hooks")
        .or_else(|| group.get("actions"))
        .and_then(|v| v.as_array());
    let Some(actions) = actions else {
        return false;
    };
    for a in actions {
        for key in ["command", "url", "prompt", "agent"] {
            if a.get(key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.contains(needle))
            {
                return true;
            }
        }
    }
    false
}

pub async fn handle_hooks_reload(request: JsonRpcRequest) -> JsonRpcResponse {
    let mgr: &Arc<ExtensionManager> = match try_extension_manager() {
        Some(m) => m,
        None => {
            return JsonRpcResponse::error(
                request.id,
                SERVICE_UNAVAILABLE,
                "extension manager not initialised",
            );
        }
    };
    match mgr.reload().await {
        Ok(report) => {
            let count = report.mounted.len();
            JsonRpcResponse::success(
                request.id,
                json!({
                    "reloaded": true,
                    "loaded_plugins": count,
                }),
            )
        }
        Err(e) => {
            warn!(error = %e, "hooks.reload failed");
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("reload failed: {e}"))
        }
    }
}

pub async fn handle_hooks_events(request: JsonRpcRequest) -> JsonRpcResponse {
    // Each event's canonical name (`HookEvent::canonical_name`, serde's
    // rename), which the loader's parser accepts. The event list itself
    // comes from `HookEvent::ALL` — previously this handler kept its own
    // hand-maintained copy, which is how a new variant ends up missing from
    // one surface but not another.
    let events: Vec<String> = HookEvent::ALL.iter().map(|e| e.canonical_name()).collect();
    JsonRpcResponse::success(request.id, json!({ "events": events }))
}

/// Runtime hook view — every hook the running server has registered across
/// all four layers (global / project / project-local / plugin-shipped),
/// with the resolved `kind`, consent state, and a per-hook reachability
/// verdict. This is the answer to "why doesn't my hook fire?"; the file
/// view (`hooks.list`) structurally cannot, because three of the four
/// layers never appear in `~/.aleph/hooks.json`.
pub async fn handle_hooks_registry(request: JsonRpcRequest) -> JsonRpcResponse {
    let mgr: &Arc<ExtensionManager> = match try_extension_manager() {
        Some(m) => m,
        None => {
            return JsonRpcResponse::error(
                request.id,
                SERVICE_UNAVAILABLE,
                "extension manager not initialised",
            );
        }
    };
    let hooks = mgr.hook_executor_snapshot().await.inventory();
    let total = hooks.len();
    let unreachable = hooks.iter().filter(|h| !h.reachable).count();
    let hooks_value = serde_json::to_value(&hooks).unwrap_or(Value::Array(Vec::new()));
    JsonRpcResponse::success(
        request.id,
        json!({
            "hooks": hooks_value,
            "total": total,
            "unreachable": unreachable,
        }),
    )
}

/// Whether `name` is an event the loader accepts — asked of the loader's own
/// parser (`hooks::parse_event`), so this face and the file cannot disagree.
/// Accepts both `snake_case` (`before_tool_call`) and `PascalCase` aliases
/// (`PreToolUse`).
fn is_known_event(name: &str) -> bool {
    crate::extension::hooks::parse_event(name).is_some()
}

/// Best-effort: schedule a reload on the global manager if it exists.
/// Failures are logged and swallowed; the watcher path is a backup, so
/// the worst case is a one-second delay before the hook becomes live.
fn trigger_reload() {
    let Some(mgr) = try_extension_manager() else {
        return;
    };
    let mgr: Arc<ExtensionManager> = Arc::clone(mgr);
    tokio::spawn(async move {
        if let Err(e) = mgr.reload().await {
            warn!(error = %e, "hooks.* triggered reload failed");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_hook_writers_preserve_appends_and_both_removal_faces() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let path = user_hooks_path().unwrap();
        fs::write(&path, r#"{"keep":"metadata","hooks":{}}"#).unwrap();
        for i in 0..16 {
            append_user_hook(
                "PreToolUse",
                json!({"command": format!("remove-{i}!")}),
                None,
            )
            .unwrap();
            append_user_hook("PostToolUse", json!({"command": "indexed"}), None).unwrap();
        }
        let mut tasks = Vec::new();
        for i in 0..16 {
            tasks.push(tokio::spawn(async move {
                let response = handle_hooks_add(JsonRpcRequest::with_id(
                    "hooks.add",
                    Some(json!({"event": "PreToolUse", "command": format!("keep-{i}")})),
                    json!(i),
                ))
                .await;
                assert!(response.is_success(), "{response:?}");
            }));
            tasks.push(tokio::spawn(async move {
                tokio::task::spawn_blocking(move || {
                    assert_eq!(
                        remove_user_hooks("before_tool_call", Some(&format!("remove-{i}!")))
                            .unwrap(),
                        1
                    );
                })
                .await
                .unwrap();
            }));
            tasks.push(tokio::spawn(async move {
                let response = handle_hooks_remove(JsonRpcRequest::with_id(
                    "hooks.remove",
                    Some(json!({"event": "after_tool_call", "index": 0})),
                    json!(i),
                ))
                .await;
                assert!(response.is_success(), "{response:?}");
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let (_, _, events) = read_hooks_file().unwrap();
        let remaining = events["PreToolUse"].as_array().unwrap();
        assert_eq!(remaining.len(), 16);
        for i in 0..16 {
            assert!(remaining
                .iter()
                .any(|g| g["hooks"][0]["command"] == format!("keep-{i}")));
        }
        assert!(!events.contains_key("PostToolUse"));
        let top: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(top["keep"], "metadata");
    }

    #[test]
    fn is_known_event_accepts_snake_and_pascal() {
        assert!(is_known_event("before_tool_call"));
        assert!(is_known_event("PreToolUse"));
        assert!(is_known_event("after_tool_call"));
        assert!(is_known_event("PostToolUse"));
        assert!(!is_known_event("BogusEvent"));
        assert!(!is_known_event(""));
    }

    #[test]
    fn build_action_requires_exactly_one() {
        // Zero set
        assert!(build_hook_action(None, None, None, None, None).is_err());

        // Two set
        assert!(build_hook_action(Some("a"), Some("b"), None, None, None).is_err());

        // Exactly one set
        let action = build_hook_action(Some("echo"), None, None, None, Some(15)).unwrap();
        assert_eq!(action["type"], "command");
        assert_eq!(action["command"], "echo");
        assert_eq!(action["timeout_secs"], 15);
    }

    #[test]
    fn build_action_covers_every_action_kind() {
        // Shared with the `hooks_manage` tool, so each shape the tool can
        // emit must round-trip into the wire format `user_settings.rs` parses.
        let prompt = build_hook_action(None, Some("note"), None, None, None).unwrap();
        assert_eq!(prompt["type"], "prompt");
        assert_eq!(prompt["prompt"], "note");

        let agent = build_hook_action(None, None, Some("reviewer"), None, None).unwrap();
        assert_eq!(agent["type"], "agent");
        assert_eq!(agent["agent"], "reviewer");

        let http = build_hook_action(None, None, None, Some("https://x/y"), Some(9)).unwrap();
        assert_eq!(http["type"], "http");
        assert_eq!(http["url"], "https://x/y");
        assert_eq!(http["timeout_secs"], 9);

        // Timeout is omitted (not null) when unset, so the loader's serde
        // default applies rather than a literal null failing to parse.
        let no_timeout = build_hook_action(Some("echo"), None, None, None, None).unwrap();
        assert!(no_timeout.get("timeout_secs").is_none());
    }

    /// The writing half of the user-global hooks layer must land where the
    /// reading half looks.
    ///
    /// `extension::hooks::user_settings::load_user_hooks` resolves
    /// `<config_dir>/hooks.json`; this RPC resolved the real home by hand, so
    /// under a relocated `ALEPH_HOME` every hook added through the admin
    /// surface went into a file nothing ever loaded — with a success response
    /// on the way out and an empty layer on the way in.
    #[test]
    fn the_admin_writes_the_same_hooks_file_the_loader_reads() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let written = user_hooks_path().expect("hooks path resolves");
        let read = crate::utils::paths::get_config_dir()
            .expect("config dir resolves")
            .join("hooks.json");
        assert_eq!(
            written,
            read,
            "hooks.add writes {} but load_user_hooks reads {} — a silently empty layer",
            written.display(),
            read.display()
        );
    }

    /// P4.16 review M-5: the RPC face of `hooks_manage add` says what the
    /// tool face says about a matcher — from the same `matcher_notice` —
    /// and says nothing about one that simply works.
    #[tokio::test]
    async fn hooks_add_warns_about_a_matcher_the_way_its_tool_twin_does() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        async fn add(params: Value) -> Value {
            handle_hooks_add(JsonRpcRequest::with_id("hooks.add", Some(params), json!(1)))
                .await
                .result
                .expect("hooks.add succeeds")
        }
        let ignored =
            add(json!({"event": "UserPromptSubmit", "command": "true", "matcher": "x"})).await;
        let warning = ignored["matcher_warning"].as_str().unwrap_or_default();
        assert!(warning.contains("ignored"), "{ignored}");
        let broken = add(json!({"event": "PreToolUse", "command": "true", "matcher": "(("})).await;
        let warning = broken["matcher_warning"].as_str().unwrap_or_default();
        assert!(warning.contains("not a valid regex"), "{broken}");
        let fine = add(json!({"event": "PreToolUse", "command": "true", "matcher": "Bash"})).await;
        assert!(fine.get("matcher_warning").is_none(), "{fine}");
    }

    #[test]
    fn group_substring_match_searches_all_action_kinds() {
        let group = json!({
            "hooks": [
                { "type": "command", "command": "rm -rf /tmp/whatever" },
                { "type": "http", "url": "https://example.com/hook" }
            ]
        });
        assert!(group_matches_substring(&group, "rm -rf"));
        assert!(group_matches_substring(&group, "example.com"));
        assert!(!group_matches_substring(&group, "nothing-here"));
    }

    /// A hook filed under the PascalCase alias is removed by the canonical
    /// spelling too. The RPC face used to look the exact key up, so it
    /// answered "no entries for event" while the tool face removed it.
    #[test]
    fn a_hook_filed_under_an_alias_is_removed_by_its_canonical_name() {
        let mut events: Map<String, Value> = serde_json::from_str(
            r#"{
                "PreToolUse": [{ "hooks": [{ "type": "command", "command": "echo one" }] }],
                "before_tool_call": [{ "hooks": [{ "type": "command", "command": "echo two" }] }]
            }"#,
        )
        .unwrap();

        let removed = remove_event_groups(
            &mut events,
            "before_tool_call",
            HookRemoval::Command("echo one".to_string()),
        )
        .unwrap();
        assert_eq!(removed, 1);
        assert!(!events.contains_key("PreToolUse"));
        assert_eq!(events["before_tool_call"].as_array().unwrap().len(), 1);
    }

    /// The index arm addresses the flattened list of the event's groups, so
    /// it names one group instead of one group under one spelling.
    #[test]
    fn the_index_arm_spans_every_key_that_names_the_event() {
        let mut events: Map<String, Value> = serde_json::from_str(
            r#"{
                "before_tool_call": [{ "hooks": [{ "type": "command", "command": "one" }] }],
                "PreToolUse": [{ "hooks": [{ "type": "command", "command": "two" }] }]
            }"#,
        )
        .unwrap();

        let removed =
            remove_event_groups(&mut events, "before_tool_call", HookRemoval::Index(0)).unwrap();
        assert_eq!(removed, 1);
        // `PreToolUse` sorts before `before_tool_call`, so index 0 is the
        // group filed under the alias.
        assert!(!events.contains_key("PreToolUse"));
        assert_eq!(events["before_tool_call"].as_array().unwrap().len(), 1);
    }
}
