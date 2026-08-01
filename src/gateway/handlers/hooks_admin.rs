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
//!   → appends one entry; returns the same shape as `hooks.list`
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
    let home = dirs::home_dir().ok_or_else(|| "cannot resolve home directory".to_string())?;
    Ok(home.join(".aleph/hooks.json"))
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
    match read_hooks_file() {
        Ok((path, exists, events)) => {
            JsonRpcResponse::success(request.id, list_response(path, exists, events))
        }
        Err(e) => JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
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

/// Append one action (optionally matcher-scoped) to `event` in the global
/// hooks file, then trigger a reload. Extracted from `handle_hooks_add` so
/// the RPC and the tool share one write path — including the atomic write,
/// the self-write marker, and the reload trigger.
pub(crate) fn append_user_hook(
    event: &str,
    action: Value,
    matcher: Option<&str>,
) -> Result<(), String> {
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

/// Drop entries from `event`. `needle` filters by substring across the
/// group's action fields; `None` removes every entry for the event. Returns
/// how many groups were removed.
pub(crate) fn remove_user_hooks(event: &str, needle: Option<&str>) -> Result<usize, String> {
    let (path, _exists, mut events) = read_hooks_file()?;
    let arr = events
        .get_mut(event)
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| format!("no entries for event {event}"))?;

    let before = arr.len();
    match needle {
        Some(n) => arr.retain(|grp| !group_matches_substring(grp, n)),
        None => arr.clear(),
    }
    let removed = before - arr.len();

    // Don't leave empty arrays cluttering the file.
    if arr.is_empty() {
        events.remove(event);
    }
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

    match read_hooks_file() {
        Ok((path, exists, events)) => {
            JsonRpcResponse::success(request.id, list_response(path, exists, events))
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
    /// Zero-based index of the group inside `events.<event>` to drop.
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
    if params.command.is_none() && params.index.is_none() {
        return JsonRpcResponse::error(
            request.id,
            INVALID_PARAMS,
            "must set either `command` (substring filter) or `index`",
        );
    }

    let (path, _exists, mut events) = match read_hooks_file() {
        Ok(v) => v,
        Err(e) => return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e),
    };

    let arr = match events.get_mut(&params.event).and_then(|v| v.as_array_mut()) {
        Some(a) => a,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("no entries for event {}", params.event),
            );
        }
    };

    let before_len = arr.len();
    if let Some(idx) = params.index {
        if idx >= arr.len() {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("index {idx} out of range (len={})", arr.len()),
            );
        }
        arr.remove(idx);
    } else if let Some(needle) = &params.command {
        arr.retain(|grp| !group_matches_substring(grp, needle));
    }
    let removed = before_len - arr.len();

    // Don't leave empty arrays cluttering the file.
    if arr.is_empty() {
        events.remove(&params.event);
    }

    if let Err(e) = write_hooks_file(&path, &events) {
        return JsonRpcResponse::error(request.id, INTERNAL_ERROR, e);
    }
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
        Ok(summary) => {
            let count = summary.plugins_loaded;
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
    // Round-trip each canonical name through serde so the wire surface
    // exactly matches what user_settings.rs accepts. The event list itself
    // comes from `HookEvent::ALL` — previously this handler kept its own
    // hand-maintained copy, which is how a new variant ends up missing from
    // one surface but not another.
    let events: Vec<String> = HookEvent::ALL
        .iter()
        .map(|e| {
            serde_json::to_string(e)
                .unwrap_or_default()
                .trim_matches('"')
                .to_string()
        })
        .collect();
    JsonRpcResponse::success(request.id, json!({ "events": events }))
}

/// Cheap check: try to parse the event name via the same serde path that
/// `user_settings` uses. Accepts both `snake_case` (`before_tool_call`) and
/// `PascalCase` aliases (`PreToolUse`).
fn is_known_event(name: &str) -> bool {
    let attempts = [
        format!("\"{name}\""),
        format!("\"{}\"", name.to_lowercase().replace('-', "_")),
    ];
    attempts
        .iter()
        .any(|s| serde_json::from_str::<HookEvent>(s).is_ok())
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
}
