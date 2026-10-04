//! The one reader of a plugin's MCP server declarations.
//!
//! A plugin's servers have two faces — the count on its `plugins.list` row and
//! the spawn in `lifecycle.rs::mount_parsed` — and both come from the list
//! [`parse_declared_servers`] returns: the manifest adapters wrap each entry as
//! a `CapabilityDeclaration::McpServer`; the row counts them and the mount
//! spawns exactly those capabilities — both only when the plugin's kind
//! `starts_mcp_servers()` (`lifecycle.rs::build_record` applies that one gate
//! to the count; on any other kind the row counts 0 and its detail says how
//! many were declared and why they are not started). There used to be two
//! readers with two policies: the count
//! dropped an absolute command outside the plugin root while the spawn ran it,
//! and the spawn read only `<root>/.mcp.json`, so an inline `mcpServers`
//! object was counted and never started.
//!
//! Where the JSON lives — `<root>/.mcp.json`, a path the manifest names, or an
//! object inlined in the manifest — is the adapters' business
//! (`manifest::component_source::resolve_mcp_servers`,
//! `manifest::parsers::parse_mcp_config_file`); what it declares is this
//! module's.
//!
//! # Format
//!
//! Both the wrapped file shape (`{"mcpServers": {...}}`) and the bare server
//! map (Claude Code's inline `mcpServers` object) are accepted.
//!
//! ## stdio transport (default)
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "server-name": {
//!       "command": "node",
//!       "args": ["${ALEPH_PLUGIN_ROOT}/src/server.js"],
//!       "env": { "KEY": "value" }
//!     }
//!   }
//! }
//! ```
//!
//! ## remote transports: `http` and `sse`
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "remote-server": {
//!       "type": "http",
//!       "url": "https://mcp.example.com/api",
//!       "headers": { "Authorization": "Bearer ${ALEPH_PLUGIN_ROOT}/token" }
//!     },
//!     "event-server": {
//!       "type": "sse",
//!       "url": "https://events.example.com/sse"
//!     }
//!   }
//! }
//! ```
//!
//! `type` is the only transport discriminator, and its three legal values are
//! `stdio` | `http` | `sse` — the same vocabulary `.mcp.json` uses elsewhere in
//! the ecosystem. There is no `"remote"` value: "remote" names the *category*
//! (`http` and `sse` both dial a URL), not a spelling you can put on the wire.
//! Anything else is a hard parse error rather than a silently dropped server.
//!
//! The `type` field defaults to `stdio` when omitted, so existing plugin
//! manifests continue to work unchanged.
//!
//! # Variable Substitution
//!
//! `${CLAUDE_PLUGIN_ROOT}` / `${ALEPH_PLUGIN_ROOT}` (the plugin directory) and
//! `${CLAUDE_PLUGIN_DATA}` / `${ALEPH_PLUGIN_DATA}` (its persistent data
//! directory, created by the mount just before a server that names it is
//! started — [`provision_data_dir`]) are expanded in `command`, `args`,
//! `url`, `env` and `headers` — by `PluginVars::expand`, the subsystem's one
//! expander, once, so the containment check below and the spawn see the same
//! string.
//!
//! # Which program a stdio server runs
//!
//! [`resolve_command`] answers it once for both faces:
//! - a bare name (`node`, `npx`, `uvx`, `python3`) is kept as written and
//!   looked up on `PATH` at spawn;
//! - an absolute path not written with a `_PLUGIN_ROOT` variable is kept as
//!   written — an absolute interpreter is the normal Claude Code shape;
//! - a relative path, or one written with `${CLAUDE_PLUGIN_ROOT}` /
//!   `${ALEPH_PLUGIN_ROOT}`, must name an existing file (not a directory) that canonicalizes
//!   inside the plugin root, and is rewritten to that absolute path (the
//!   spawn has no working directory of its own, so an unresolved `./server.js`
//!   would run against the daemon's). `${PLUGIN_ROOT}` is not a variable here
//!   and is not expanded; a command using it is a relative path and meets the
//!   same rule.
//!
//! A refused server is a parse error of the plugin, like a stdio server with no
//! `command`: the plugin's row carries it as its error status, naming the
//! server, and nothing of the plugin is mounted. Values are never
//! shell-parsed: `StdioTransport::spawn` is argv-based
//! (`Command::new(command).args(args)`).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::extension::plugin_vars::PluginVars;
use crate::mcp::{McpManagerConfig, McpTransportType};

/// A single server entry.
///
/// Either a stdio entry (`command` + `args` + `env`) or a remote entry
/// (`url` + `headers`). The `type` discriminator defaults to `stdio` when
/// absent so existing plugins continue to parse.
#[derive(Debug, Deserialize)]
struct McpJsonServerEntry {
    /// The `type` discriminator. Named `transport` in Rust because that is
    /// what it selects, but the **JSON key is `type`** — a sibling `transport`
    /// key in `.mcp.json` is not read, and serde drops unknown keys silently,
    /// so do not let prose here grow a second spelling for this one field.
    #[serde(default = "default_transport", rename = "type")]
    transport: String,
    // stdio fields
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    // remote fields
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
}

fn default_transport() -> String {
    "stdio".to_string()
}

/// The variables that name the plugin root. A command written with one of
/// them is the plugin's own program and must stay inside the root.
const ROOT_VARIABLES: [&str; 2] = ["${CLAUDE_PLUGIN_ROOT}", "${ALEPH_PLUGIN_ROOT}"];

/// Parse a plugin's MCP server declarations — the one reader (module doc).
///
/// Returns the servers in name order, each with its id from
/// [`plugin_server_id`], every variable expanded and every stdio `command`
/// resolved by [`resolve_command`]. `Err` names the server when one entry is
/// malformed or refused; the caller fails the whole plugin with it.
pub(crate) fn parse_declared_servers(
    content: &str,
    plugin_dir: &Path,
    plugin_id: &str,
) -> Result<Vec<McpManagerConfig>, String> {
    let value: serde_json::Value =
        serde_json::from_str(content).map_err(|e| format!("JSON parse error: {e}"))?;
    let servers = match value.get("mcpServers") {
        Some(wrapped) => wrapped.clone(),
        None => value,
    };
    let entries: BTreeMap<String, McpJsonServerEntry> =
        serde_json::from_value(servers).map_err(|e| format!("JSON parse error: {e}"))?;

    // Nothing is provisioned here: this parse runs on every discovery pass,
    // for rows nobody enabled. The data directory is the spawn's to create
    // ([`provision_data_dir`]).
    let vars = PluginVars::new(plugin_id, plugin_dir);
    entries
        .into_iter()
        .map(|(name, entry)| declared_server(plugin_id, &name, entry, &vars))
        .collect()
}

/// One entry → the config the mount spawns.
fn declared_server(
    plugin_id: &str,
    server_name: &str,
    entry: McpJsonServerEntry,
    vars: &PluginVars,
) -> Result<McpManagerConfig, String> {
    let server_id = plugin_server_id(plugin_id, server_name);
    let display_name = format!("{server_name} ({plugin_id})");
    let expand_map = |map: &HashMap<String, String>| -> HashMap<String, String> {
        map.iter()
            .map(|(k, v)| (k.clone(), vars.expand(v)))
            .collect()
    };

    let transport = match entry.transport.as_str() {
        "stdio" => McpTransportType::Stdio,
        "http" => McpTransportType::Http,
        "sse" => McpTransportType::Sse,
        other => {
            return Err(format!(
                "unknown MCP transport type '{other}' for server '{server_name}' \
                 (expected one of: stdio, http, sse)"
            ))
        }
    };

    match transport {
        McpTransportType::Stdio => {
            // stdio entries require `command`. Refuse ambiguous configs
            // rather than spawning a phantom process.
            let written = entry.command.ok_or_else(|| {
                format!(
                    "MCP stdio server '{server_name}' is missing 'command' \
                     (either add it or set `\"type\": \"http\"` with a `url`)"
                )
            })?;
            let command = resolve_command(&written, vars)
                .map_err(|why| format!("MCP server '{server_name}' refused: {why}"))?;
            let args: Vec<String> = entry.args.iter().map(|a| vars.expand(a)).collect();
            Ok(McpManagerConfig::stdio(&server_id, &display_name, &command)
                .with_args(args)
                .with_env(expand_map(&entry.env))
                .with_auto_start(true))
        }
        McpTransportType::Http | McpTransportType::Sse => {
            // remote entries require `url`. Refuse ambiguous configs.
            let url = entry.url.ok_or_else(|| {
                format!(
                    "MCP remote server '{server_name}' is missing 'url' \
                     (either add it or set `\"type\": \"stdio\"` with a `command`)"
                )
            })?;
            let url = vars.expand(&url);
            let mut config = if transport == McpTransportType::Sse {
                McpManagerConfig::sse(&server_id, &display_name, &url)
            } else {
                McpManagerConfig::http(&server_id, &display_name, &url)
            };
            config.headers = expand_map(&entry.headers);
            config.auto_start = true;
            Ok(config)
        }
    }
}

/// Which program a stdio server runs — one answer for the containment check
/// and the spawn, so the two can never see different paths (module doc).
fn resolve_command(written: &str, vars: &PluginVars) -> Result<String, String> {
    let expanded = vars.expand(written);
    if expanded.is_empty() {
        return Err("the command is empty".to_string());
    }
    let rooted = ROOT_VARIABLES.iter().any(|v| written.contains(v));
    if !rooted && (is_bare(&expanded) || Path::new(&expanded).is_absolute()) {
        return Ok(expanded);
    }
    // `join` with an absolute path yields that path: a rooted command is
    // checked as written, a relative one against the root.
    let candidate = vars.root_dir().join(&expanded);
    inside_root(&candidate, vars.root_dir())
        .map(|resolved| resolved.to_string_lossy().into_owned())
        .map_err(|why| format!("command {written:?} {why}"))
}

/// A program name `Command::new` looks up on `PATH`: no separator at all
/// (a name with a `/` in it is resolved against the working directory).
fn is_bare(command: &str) -> bool {
    !command.contains('/') && !command.contains('\\') && command != "." && command != ".."
}

/// `candidate`, canonicalized, if it is a file inside `root`. Fails closed: a
/// path that does not exist or cannot be resolved is refused — a canonicalize
/// error is not a verdict, and there is nothing to run there anyway — and so
/// is a directory, which is not a program.
fn inside_root(candidate: &Path, root: &Path) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(root)
        .map_err(|e| format!("cannot be checked: the plugin root does not resolve ({e})"))?;
    let resolved = std::fs::canonicalize(candidate).map_err(|e| {
        format!(
            "does not resolve to a file inside the plugin root ({}: {e})",
            candidate.display()
        )
    })?;
    if !resolved.starts_with(&root) {
        return Err(format!(
            "resolves to {}, outside the plugin root {}",
            resolved.display(),
            root.display()
        ));
    }
    if !resolved.is_file() {
        return Err(format!(
            "resolves to {}, which is not a file",
            resolved.display()
        ));
    }
    Ok(resolved)
}

/// Create the plugin's data directory when a server about to be started names
/// it (after expansion, in its command, args, env, url or headers). Called by
/// the mount, not the parse: a row nobody enabled provisions nothing. A
/// failure to create is a `warn!` — the server is still worth starting, and it
/// fails loudly at the moment it writes.
pub(crate) fn provision_data_dir(plugin_id: &str, plugin_dir: &Path, servers: &[McpManagerConfig]) {
    let vars = PluginVars::new(plugin_id, plugin_dir);
    let data = vars.data_dir().to_string_lossy().into_owned();
    let names_data = |s: &McpManagerConfig| {
        s.command
            .iter()
            .chain(s.url.iter())
            .chain(s.args.iter())
            .chain(s.env.values())
            .chain(s.headers.values())
            .any(|v| v.contains(&data))
    };
    if !servers.iter().any(names_data) {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(vars.data_dir()) {
        tracing::warn!(
            plugin_id, path = %vars.data_dir().display(), error = %e,
            "could not create the plugin data directory its MCP server names"
        );
    }
}

/// Key the declared servers by server id and layer the operator's plugin
/// configuration (`plugin_vars::settings_env`) under each stdio server's own
/// `env` — the author's explicit value wins over a convention. Spawn-time,
/// because the configuration is the operator's, not the manifest's; the
/// server list itself is the capability list the row counted.
pub(crate) fn with_operator_env(
    servers: Vec<McpManagerConfig>,
    settings: &serde_json::Value,
) -> HashMap<String, McpManagerConfig> {
    let config_env = crate::extension::plugin_vars::settings_env(settings);
    servers
        .into_iter()
        .map(|server| {
            let server = if server.transport == McpTransportType::Stdio && !config_env.is_empty() {
                let mut env: HashMap<String, String> = config_env.iter().cloned().collect();
                env.extend(server.env.clone());
                server.with_env(env)
            } else {
                server
            };
            (server.id.clone(), server)
        })
        .collect()
}

/// The transient server id of a plugin-declared MCP server. The plugin id is
/// embedded so every consumer that only has the server id (the tool bridge,
/// the request-time MCP join) can name the owner without a side table.
/// [`owning_plugin_of_server_id`] is the inverse; keep them together.
#[must_use]
pub(crate) fn plugin_server_id(plugin_id: &str, server_name: &str) -> String {
    format!("plugin:{plugin_id}/{server_name}")
}

/// Inverse of [`plugin_server_id`]. `None` for a server no plugin declared
/// (user `mcp.json` servers have no `plugin:` prefix). Splits at the FIRST
/// `/`: plugin ids are `[a-z0-9-]` (`manifest::validate_plugin_id` /
/// `sanitize_plugin_id`), so the first slash is always the separator.
///
/// Feed it ONLY an MCP server id — the handler side's
/// `tools::service::ToolSource::Mcp { server_id }` or the catalog side's
/// `tool_metadata::ToolSource::Mcp { server }`. Other `plugin:`-prefixed
/// id spaces use a colon grammar (`plugin:{id}:{name}` tool ids,
/// `plugin:{id}` usage / visibility groups) — same prefix, different
/// grammar — and decoding one of those here names the wrong thing.
#[must_use]
pub(crate) fn owning_plugin_of_server_id(server_id: &str) -> Option<&str> {
    let rest = server_id.strip_prefix("plugin:")?;
    let (plugin_id, _server_name) = rest.split_once('/')?;
    (!plugin_id.is_empty()).then_some(plugin_id)
}

#[cfg(test)]
mod tests;
