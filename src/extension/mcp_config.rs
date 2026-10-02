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
mod tests {
    use super::*;

    /// The parser keyed by server id, for the lookups these tests make.
    fn parse(
        content: &str,
        dir: &Path,
        plugin_id: &str,
    ) -> Result<HashMap<String, McpManagerConfig>, String> {
        parse_declared_servers(content, dir, plugin_id)
            .map(|servers| servers.into_iter().map(|s| (s.id.clone(), s)).collect())
    }

    /// The command one stdio server named `srv` resolves to under `root`.
    fn command_of(written: &str, root: &Path) -> Result<String, String> {
        let content = serde_json::json!({"mcpServers": {"srv": {"command": written}}});
        parse(&content.to_string(), root, "p")
            .map(|m| m["plugin:p/srv"].command.clone().expect("stdio"))
    }

    #[test]
    fn plugin_server_id_round_trips_through_its_decoder() {
        let id = plugin_server_id("media-office", "office");
        assert_eq!(id, "plugin:media-office/office");
        assert_eq!(owning_plugin_of_server_id(&id), Some("media-office"));
        // A server name with a slash still decodes to the plugin id: the
        // plugin id itself cannot contain one (`validate_plugin_id`).
        assert_eq!(owning_plugin_of_server_id("plugin:x/a/b"), Some("x"));
        // Not plugin-owned.
        assert_eq!(owning_plugin_of_server_id("github"), None);
        assert_eq!(owning_plugin_of_server_id("plugin:"), None);
        assert_eq!(owning_plugin_of_server_id("plugin:nosl"), None);
        // An empty plugin id names no plugin (the only input that reaches
        // the empty-id guard: the two above exit at the `/` split).
        assert_eq!(owning_plugin_of_server_id("plugin:/srv"), None);
        // Same prefix, different grammar: the colon-form tool ids
        // (`plugin:{id}:{name}`) are not server ids and must not decode.
        assert_eq!(owning_plugin_of_server_id("plugin:x:tool"), None);
    }

    /// `.mcp.json` parsing uses the encoder, not its own `format!`.
    #[test]
    fn parsed_server_ids_come_from_the_encoder() {
        let dir = tempfile::tempdir().unwrap();
        let configs = parse(
            r#"{"mcpServers":{"srv":{"command":"true"}}}"#,
            dir.path(),
            "plug",
        )
        .unwrap();
        assert!(configs.contains_key(&plugin_server_id("plug", "srv")));
    }

    #[test]
    fn test_parse_mcp_json_basic() {
        let content = r#"{
            "mcpServers": {
                "my-server": {
                    "command": "node",
                    "args": ["${ALEPH_PLUGIN_ROOT}/src/server.js", "--port", "3000"],
                    "env": {
                        "NODE_ENV": "production",
                        "PLUGIN_DIR": "${CLAUDE_PLUGIN_ROOT}"
                    }
                }
            }
        }"#;

        let result = parse(content, Path::new("/plugins/test-plugin"), "test-plugin").unwrap();

        assert_eq!(result.len(), 1);

        let config = result.get("plugin:test-plugin/my-server").unwrap();
        assert_eq!(config.command, Some("node".to_string()));
        assert_eq!(
            config.args,
            vec!["/plugins/test-plugin/src/server.js", "--port", "3000"]
        );
        assert_eq!(
            config.env.get("PLUGIN_DIR"),
            Some(&"/plugins/test-plugin".to_string())
        );
        assert_eq!(config.env.get("NODE_ENV"), Some(&"production".to_string()));
        assert!(config.auto_start);
    }

    #[test]
    fn test_parse_mcp_json_multiple_servers() {
        let content = r#"{
            "mcpServers": {
                "alpha": {
                    "command": "python",
                    "args": ["-m", "server_a"]
                },
                "beta": {
                    "command": "node",
                    "args": ["server_b.js"]
                }
            }
        }"#;

        let result = parse(content, Path::new("/plugins/multi"), "multi").unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.contains_key("plugin:multi/alpha"));
        assert!(result.contains_key("plugin:multi/beta"));
    }

    /// The inline `mcpServers` object is the bare map, and reads the same.
    #[test]
    fn the_bare_server_map_and_the_wrapped_file_read_the_same() {
        let wrapped = parse(
            r#"{"mcpServers":{"s":{"command":"npx"}}}"#,
            Path::new("/p/x"),
            "p",
        )
        .unwrap();
        let bare = parse(r#"{"s":{"command":"npx"}}"#, Path::new("/p/x"), "p").unwrap();
        assert_eq!(wrapped.len(), 1);
        assert_eq!(
            wrapped.keys().collect::<Vec<_>>(),
            bare.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_parse_mcp_json_empty_servers() {
        let result = parse(
            r#"{ "mcpServers": {} }"#,
            Path::new("/plugins/empty"),
            "empty",
        )
        .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_mcp_json_invalid_json() {
        assert!(parse("not json at all", Path::new("/plugins/bad"), "bad").is_err());
    }

    #[test]
    fn test_server_id_namespacing() {
        let content = r#"{
            "mcpServers": {
                "srv": { "command": "echo", "args": [] }
            }
        }"#;

        let result = parse(content, Path::new("/p/a"), "my-plugin").unwrap();

        // Server ID should be namespaced with plugin ID
        assert!(result.contains_key("plugin:my-plugin/srv"));
        let config = &result["plugin:my-plugin/srv"];
        assert_eq!(config.id, "plugin:my-plugin/srv");
        assert!(config.name.contains("my-plugin"));
    }

    #[test]
    fn test_parse_mcp_json_remote_http_transport() {
        let content = r#"{
            "mcpServers": {
                "remote-srv": {
                    "type": "http",
                    "url": "https://mcp.example.com/api",
                    "headers": { "Authorization": "Bearer ${ALEPH_PLUGIN_ROOT}/token" }
                }
            }
        }"#;

        let result = parse(content, Path::new("/p/x"), "remote-plugin").unwrap();

        let config = result
            .get("plugin:remote-plugin/remote-srv")
            .expect("server must be registered");
        assert_eq!(config.transport, McpTransportType::Http);
        assert_eq!(config.url.as_deref(), Some("https://mcp.example.com/api"));
        assert_eq!(
            config.command, None,
            "remote transport must not carry a command"
        );
        assert!(config.args.is_empty());
        assert!(config.auto_start, "remote servers auto-start by default");
        assert_eq!(
            config.headers.get("Authorization").map(String::as_str),
            Some("Bearer /p/x/token"),
            "expected plugin_root + /token substitution in the Authorization header"
        );
    }

    /// `${ALEPH_PLUGIN_DATA}` is expanded here, alongside its `_ROOT` twin.
    ///
    /// The test this replaces asserted the **opposite** — that the value was
    /// handed onward verbatim for "the manager actor's spawn-time pass". No
    /// such pass existed, so the assertion pinned the bug in place: a plugin
    /// using the documented variable received the literal string. A test that
    /// encodes a mechanism's absence as its contract is worse than no test.
    #[test]
    fn plugin_data_variable_is_expanded() {
        // Held for the whole body: `plugin_data_dir` is read twice (inside the
        // parser and for `expected`), and both reads must see one `ALEPH_HOME`.
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let content = r#"{
            "mcpServers": {
                "srv": {
                    "type": "http",
                    "url": "https://mcp.example.com/api",
                    "headers": { "Authorization": "Bearer ${ALEPH_PLUGIN_DATA}/token" }
                }
            }
        }"#;

        let result = parse(content, Path::new("/p/x"), "p").unwrap();
        let header = result
            .get("plugin:p/srv")
            .unwrap()
            .headers
            .get("Authorization")
            .cloned()
            .unwrap();
        assert!(
            !header.contains("${ALEPH_PLUGIN_DATA}"),
            "the variable must not reach the server as a literal: {header}"
        );
        let expected = format!(
            "Bearer {}/token",
            crate::extension::plugin_data_dir("p").display()
        );
        assert_eq!(header, expected);
    }

    /// The two variables are distinct: the data dir lives outside the install
    /// tree precisely so `plugin update`'s atomic swap cannot take it with it.
    #[test]
    fn root_and_data_are_distinct_substitutions() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let vars = PluginVars::new("p", Path::new("/install/p"));
        assert_eq!(
            vars.expand("${ALEPH_PLUGIN_ROOT}|${CLAUDE_PLUGIN_DATA}"),
            format!(
                "/install/p|{}",
                crate::extension::plugin_data_dir("p").display()
            )
        );
        assert!(
            !crate::extension::plugin_data_dir("p")
                .starts_with(crate::extension::default_plugins_dir().join("p")),
            "the data dir must not sit inside the install dir"
        );
    }

    #[test]
    fn test_parse_mcp_json_remote_sse_transport() {
        let content = r#"{
            "mcpServers": {
                "events": {
                    "type": "sse",
                    "url": "https://events.example.com/sse"
                }
            }
        }"#;

        let result = parse(content, Path::new("/p/x"), "ev").unwrap();
        let config = result.get("plugin:ev/events").unwrap();
        assert_eq!(config.transport, McpTransportType::Sse);
        assert_eq!(
            config.url.as_deref(),
            Some("https://events.example.com/sse")
        );
    }

    #[test]
    fn test_parse_mcp_json_default_transport_is_stdio() {
        // Bare entry without `type` must still parse as stdio (backward-compat).
        let content = r#"{
            "mcpServers": {
                "legacy": { "command": "node", "args": ["server.js"] }
            }
        }"#;
        let result = parse(content, Path::new("/p/x"), "legacy").unwrap();
        let config = result.get("plugin:legacy/legacy").unwrap();
        assert_eq!(config.transport, McpTransportType::Stdio);
        assert_eq!(config.command.as_deref(), Some("node"));
    }

    #[test]
    fn test_parse_mcp_json_stdio_without_command_errors() {
        let content = r#"{
            "mcpServers": {
                "broken": { "args": ["x"] }
            }
        }"#;
        let err = parse(content, Path::new("/p/x"), "broken").unwrap_err();
        assert!(
            err.contains("missing 'command'"),
            "stdio without command must be a hard error: {err}"
        );
    }

    #[test]
    fn test_parse_mcp_json_remote_without_url_errors() {
        let content = r#"{
            "mcpServers": {
                "broken": { "type": "http", "headers": {} }
            }
        }"#;
        let err = parse(content, Path::new("/p/x"), "broken").unwrap_err();
        assert!(
            err.contains("missing 'url'"),
            "remote without url must be a hard error: {err}"
        );
    }

    #[test]
    fn test_parse_mcp_json_unknown_transport_errors() {
        let content = r#"{
            "mcpServers": {
                "broken": { "type": "telnet" }
            }
        }"#;
        let err = parse(content, Path::new("/p/x"), "broken").unwrap_err();
        assert!(
            err.contains("unknown MCP transport type 'telnet'"),
            "unknown transport must surface a clear error: {err}"
        );
    }

    /// `"remote"` is the one wrong spelling with a pedigree: this module's own
    /// doc comment, its stdio error hint, and three of its tests all promised
    /// it while the parser never accepted it, so it shipped as four statements
    /// of a fact only one of which was true. It stays rejected — but the
    /// rejection has to name the legal set, because the people who reach this
    /// error are the ones who read the old docs.
    #[test]
    fn the_remote_spelling_is_rejected_and_the_error_names_the_legal_set() {
        let content = r#"{
            "mcpServers": {
                "srv": { "type": "remote", "url": "https://mcp.example.com/api" }
            }
        }"#;
        let err = parse(content, Path::new("/p/x"), "remote-plugin").unwrap_err();
        assert!(
            err.contains("unknown MCP transport type 'remote'"),
            "'remote' must be rejected, not silently coerced: {err}"
        );
        for legal in ["stdio", "http", "sse"] {
            assert!(
                err.contains(legal),
                "error must point at '{legal}' so the author can fix it: {err}"
            );
        }
    }

    // ── Which program a stdio server runs ─────────────────────────────────

    /// A plugin root with `bin/srv` inside and `outside` next to it.
    fn command_fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plug");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin/srv"), "").unwrap();
        std::fs::write(tmp.path().join("outside"), "").unwrap();
        (tmp, root)
    }

    #[test]
    fn bare_and_absolute_commands_are_kept_as_written() {
        let (_tmp, root) = command_fixture();
        assert_eq!(command_of("node", &root).unwrap(), "node");
        // Absolute anywhere — outside the root, even nonexistent. "Absolute"
        // is platform-relative (`Path::is_absolute`): a rooted `/x` is NOT
        // absolute on Windows and would be root-prefixed, then refused.
        #[cfg(unix)]
        let (abs_interp, abs_missing) = ("/usr/bin/python3", "/nonexistent/interp");
        #[cfg(windows)]
        let (abs_interp, abs_missing) = (r"C:\qa-abs\python3.exe", r"C:\nonexistent\interp");
        assert_eq!(command_of(abs_interp, &root).unwrap(), abs_interp);
        assert_eq!(command_of(abs_missing, &root).unwrap(), abs_missing);
    }

    #[test]
    fn relative_and_rooted_commands_resolve_to_the_file_inside_the_root() {
        let (_tmp, root) = command_fixture();
        let inside = std::fs::canonicalize(root.join("bin/srv"))
            .unwrap()
            .display()
            .to_string();
        assert_eq!(command_of("./bin/srv", &root).unwrap(), inside);
        assert_eq!(command_of("bin/srv", &root).unwrap(), inside);
        assert_eq!(
            command_of("${CLAUDE_PLUGIN_ROOT}/bin/srv", &root).unwrap(),
            inside
        );
        assert_eq!(
            command_of("${ALEPH_PLUGIN_ROOT}/bin/../bin/srv", &root).unwrap(),
            inside
        );
    }

    #[test]
    fn a_command_that_leaves_the_root_is_refused_and_names_the_server() {
        let (_tmp, root) = command_fixture();
        for written in [
            "../outside",
            "${CLAUDE_PLUGIN_ROOT}/../outside",
            "${ALEPH_PLUGIN_ROOT}/../outside",
        ] {
            let err = command_of(written, &root).unwrap_err();
            assert!(
                err.contains("MCP server 'srv' refused") && err.contains("outside the plugin root"),
                "{written}: {err}"
            );
        }
    }

    /// Canonicalization follows symlinks: a link inside the root that points
    /// out is outside.
    #[cfg(unix)]
    #[test]
    fn a_symlink_inside_the_root_that_points_out_is_refused() {
        let (tmp, root) = command_fixture();
        std::os::unix::fs::symlink(tmp.path().join("outside"), root.join("link")).unwrap();
        let err = command_of("./link", &root).unwrap_err();
        assert!(err.contains("outside the plugin root"), "{err}");
    }

    /// Fail closed: a contained command that does not resolve is refused,
    /// and `${PLUGIN_ROOT}` (not a variable here) is just a relative path.
    #[test]
    fn a_contained_command_that_does_not_resolve_is_refused() {
        let (_tmp, root) = command_fixture();
        for written in ["./missing", "${PLUGIN_ROOT}/../outside", ""] {
            assert!(
                command_of(written, &root).is_err(),
                "{written:?} must be refused"
            );
        }
    }

    /// A directory inside the root is not a program: refused at parse, not
    /// counted and then failed at spawn.
    #[test]
    fn a_directory_inside_the_root_is_not_a_command() {
        let (_tmp, root) = command_fixture();
        for written in [".", "./bin", "${CLAUDE_PLUGIN_ROOT}/bin"] {
            let err = command_of(written, &root).unwrap_err();
            assert!(err.contains("not a file"), "{written:?}: {err}");
        }
    }

    /// The mount creates the data directory only for a server that names it.
    #[test]
    fn the_data_directory_is_provisioned_only_for_a_server_that_names_it() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let (_tmp, root) = command_fixture();
        let plain = parse_declared_servers(
            r#"{"mcpServers":{"s":{"command":"node"}}}"#,
            &root,
            "p415-plain",
        )
        .unwrap();
        provision_data_dir("p415-plain", &root, &plain);
        assert!(!crate::extension::plugin_data_dir("p415-plain").exists());

        let named = parse_declared_servers(
            r#"{"mcpServers":{"s":{"command":"node","env":{"DB":"${ALEPH_PLUGIN_DATA}/db"}}}}"#,
            &root,
            "p415-named",
        )
        .unwrap();
        provision_data_dir("p415-named", &root, &named);
        assert!(crate::extension::plugin_data_dir("p415-named").is_dir());
    }

    /// Parsing runs on every discovery pass, for rows nobody enabled; it
    /// provisions nothing. The data directory is the spawn's to create.
    #[test]
    fn parsing_never_creates_the_data_directory() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let (_tmp, root) = command_fixture();
        let data = crate::extension::plugin_data_dir("p415-data");
        parse(
            r#"{"mcpServers":{"s":{"command":"node","args":["${CLAUDE_PLUGIN_DATA}/db"]}}}"#,
            &root,
            "p415-data",
        )
        .unwrap();
        assert!(!data.exists(), "{} was created by a parse", data.display());
    }

    /// `${CLAUDE_PLUGIN_DATA}` is absolute and not a root variable: the
    /// plugin's own data directory, kept as written.
    #[test]
    fn a_data_directory_command_is_an_absolute_command() {
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let (_tmp, root) = command_fixture();
        let expected = format!(
            "{}/venv/bin/python",
            crate::extension::plugin_data_dir("p").display()
        );
        assert_eq!(
            command_of("${CLAUDE_PLUGIN_DATA}/venv/bin/python", &root).unwrap(),
            expected
        );
    }

    #[test]
    fn the_operator_env_sits_under_the_authors_env_on_stdio_only() {
        let servers = parse_declared_servers(
            r#"{"mcpServers":{
                "s":{"command":"node","env":{"CLAUDE_PLUGIN_OPTION_KEY":"author"}},
                "r":{"type":"http","url":"https://x"}
            }}"#,
            Path::new("/p/x"),
            "p",
        )
        .unwrap();
        let keyed = with_operator_env(servers, &serde_json::json!({"key": "operator", "other": 1}));
        let stdio = &keyed["plugin:p/s"];
        assert_eq!(
            stdio
                .env
                .get("CLAUDE_PLUGIN_OPTION_KEY")
                .map(String::as_str),
            Some("author"),
            "the author's explicit value wins"
        );
        assert_eq!(
            stdio
                .env
                .get("CLAUDE_PLUGIN_OPTION_OTHER")
                .map(String::as_str),
            Some("1")
        );
        assert!(stdio.env.contains_key("ALEPH_PLUGIN_CONFIG"));
        assert!(keyed["plugin:p/r"].env.is_empty());
    }
}
