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
    // Absolute anywhere — outside the root, even nonexistent.
    assert_eq!(
        command_of("/usr/bin/python3", &root).unwrap(),
        "/usr/bin/python3"
    );
    assert_eq!(
        command_of("/nonexistent/interp", &root).unwrap(),
        "/nonexistent/interp"
    );
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
