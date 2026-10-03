//! A Claude Code MCP plugin, parse → mount, through the real daemon path
//! (P4.15 + ledger defect D-1).
//!
//! A plugin's MCP servers have two faces: the COUNT on its `plugins.list` row
//! (`PluginRecord::mcp_server_count`, from the adapter's capabilities) and the
//! SPAWN (`lifecycle.rs::mount_parsed`, step `mcp_server`). They used to come
//! from two readers with two policies — the count dropped an absolute command
//! outside the plugin root while the spawn ran it, and the spawn never read an
//! inline `mcpServers` object the count had counted. Each test here asserts
//! BOTH faces for one command shape, so splitting the readers again turns at
//! least one of them red.
//!
//! The spawn face is observed where the spawn itself reports: every command
//! here names a program that cannot run, so the actor's start fails and the
//! row's detail carries `mcp:<server id>: … (<the command it tried>) …` —
//! the string `StdioTransport::spawn` handed to `Command::new`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::discovery::DiscoveryConfig;
use crate::extension::{ExtensionConfig, ExtensionManager, PluginInfo, PluginStatus};

/// Same shape as `lifecycle.rs::tests::isolated_manager`, plus a real MCP
/// manager actor so the `mcp_server` step runs instead of being skipped.
async fn attached_manager(dir: &Path) -> ExtensionManager {
    let manager = ExtensionManager::new(ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: dir.to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
            claude_home_override: None,
        },
        plugins_config_path: Some(dir.join("plugins.toml")),
        extra_plugin_parents: vec![crate::discovery::ProjectPluginParent {
            project_root: dir.to_path_buf(),
            dir: dir.join("plugins"),
        }],
    })
    .await
    .unwrap();
    let (actor, handle) = crate::mcp::manager::McpManagerActor::new(Some(dir.join("mcp.json")))
        .await
        .unwrap();
    tokio::spawn(actor.run());
    manager.set_mcp_handle(handle);
    manager
}

/// A Claude Code plugin with NO `aleph` block. `manifest_extra` is spliced
/// into `plugin.json` (e.g. an inline `mcpServers`); `mcp_json` is written to
/// `<root>/.mcp.json` when given. Returns the plugin root.
fn write_cc_plugin(dir: &Path, id: &str, manifest_extra: &str, mcp_json: Option<&str>) -> PathBuf {
    let root = dir.join("plugins").join(id);
    std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    std::fs::write(
        root.join(".claude-plugin/plugin.json"),
        format!(r#"{{"name":"{id}"{manifest_extra}}}"#),
    )
    .unwrap();
    if let Some(body) = mcp_json {
        std::fs::write(root.join(".mcp.json"), body).unwrap();
    }
    root
}

fn row<'a>(info: &'a [PluginInfo], id: &str) -> &'a PluginInfo {
    info.iter()
        .find(|p| p.name == id)
        .unwrap_or_else(|| panic!("no row for {id}: {info:?}"))
}

/// Wait (bounded) until the row stops being `Pending` — every start here
/// fails fast, so the watcher writes its verdict within milliseconds.
async fn settled_row(manager: &ExtensionManager, id: &str) -> PluginInfo {
    for _ in 0..200 {
        let rec = manager.get_plugin_record(id).await.expect("registered");
        if !matches!(rec.status, PluginStatus::Pending { .. }) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    row(&manager.get_plugin_info().await, id).clone()
}

/// The count face says `n`, and the spawn face tried exactly `commands`
/// (each as `mcp:plugin:<id>/<server>` with the resolved command string).
fn assert_counted_and_spawned(row: &PluginInfo, n: usize, spawned: &[(&str, &str)]) {
    assert_eq!(row.kind, "mcp", "no aleph block, servers declared: {row:?}");
    assert_eq!(row.mcp_servers_count, n, "count face: {row:?}");
    let detail = row.error.clone().unwrap_or_default();
    for (server, command) in spawned {
        let server_id = crate::extension::mcp_config::plugin_server_id(&row.name, server);
        assert!(
            detail.contains(&format!("mcp:{server_id}")),
            "spawn face never started {server_id}: {row:?}"
        );
        assert!(
            detail.contains(command),
            "spawn face started {server_id} with something other than {command}: {detail}"
        );
    }
}

#[tokio::test]
async fn a_bare_command_is_counted_and_spawned_from_path() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let tmp = tempfile::tempdir().unwrap();
    write_cc_plugin(
        tmp.path(),
        "p415-bare",
        "",
        Some(r#"{"mcpServers":{"srv":{"command":"qa-nonexistent-mcp-binary-9f3a"}}}"#),
    );
    let manager = attached_manager(tmp.path()).await;
    manager.load_all().await.unwrap();

    let row = settled_row(&manager, "p415-bare").await;
    assert_counted_and_spawned(&row, 1, &[("srv", "(qa-nonexistent-mcp-binary-9f3a)")]);
}

/// An absolute interpreter anywhere is the normal Claude Code shape (P2.11's
/// fixture uses `sys.executable`). The count face used to drop it while the
/// spawn face ran it: `mcp_servers_count: 0` beside a running server.
#[tokio::test]
async fn an_absolute_command_outside_the_root_is_counted_and_spawned() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let tmp = tempfile::tempdir().unwrap();
    // "Absolute" is platform-relative (`Path::is_absolute`): a rooted `/x`
    // is not absolute on Windows and would be root-prefixed, then refused.
    #[cfg(unix)]
    let abs = "/nonexistent-p415/qa-abs-bin";
    #[cfg(windows)]
    let abs = r"C:\nonexistent-p415\qa-abs-bin.exe";
    let body = serde_json::json!({"mcpServers":{"srv":{"command":abs,"args":["x"]}}});
    write_cc_plugin(tmp.path(), "p415-abs", "", Some(&body.to_string()));
    let manager = attached_manager(tmp.path()).await;
    manager.load_all().await.unwrap();

    let row = settled_row(&manager, "p415-abs").await;
    assert_counted_and_spawned(&row, 1, &[("srv", &format!("({abs})") as &str)]);
}

/// A relative command and a `${…_PLUGIN_ROOT}` command are resolved against
/// the plugin root — and the spawn runs THAT path, not `./server.js` against
/// the daemon's working directory (`McpManagerConfig` carries no cwd).
#[tokio::test]
async fn a_relative_or_rooted_command_inside_the_root_is_counted_and_spawned_from_the_root() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = write_cc_plugin(
        tmp.path(),
        "p415-rel",
        "",
        Some(
            r#"{"mcpServers":{
                "rel":{"command":"./server.js"},
                "rooted":{"command":"${ALEPH_PLUGIN_ROOT}/bin/../server.js"}
            }}"#,
        ),
    );
    // Present but not executable: the spawn fails with EACCES, naming the path.
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::write(root.join("server.js"), "not a program\n").unwrap();
    let resolved = std::fs::canonicalize(root.join("server.js")).unwrap();
    let resolved = format!("({})", resolved.display());

    let manager = attached_manager(tmp.path()).await;
    manager.load_all().await.unwrap();

    let row = settled_row(&manager, "p415-rel").await;
    assert_counted_and_spawned(
        &row,
        2,
        &[("rel", resolved.as_str()), ("rooted", resolved.as_str())],
    );
}

/// The inline `mcpServers` object in `plugin.json` (two of Anthropic's own
/// plugins): the count face counted it and the spawn face read only
/// `<root>/.mcp.json`, so it was never started.
#[tokio::test]
async fn an_inline_mcp_servers_object_is_counted_and_spawned() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let tmp = tempfile::tempdir().unwrap();
    write_cc_plugin(
        tmp.path(),
        "p415-inline",
        r#","mcpServers":{"srv":{"command":"qa-nonexistent-mcp-binary-9f3a"}}"#,
        None,
    );
    let manager = attached_manager(tmp.path()).await;
    manager.load_all().await.unwrap();

    let row = settled_row(&manager, "p415-inline").await;
    assert_counted_and_spawned(&row, 1, &[("srv", "(qa-nonexistent-mcp-binary-9f3a)")]);
}

/// `${CLAUDE_PLUGIN_ROOT}/../outside.js` names a file that exists — outside
/// the root. Refused on both faces: not counted, never started, and the row
/// says which server and why.
#[tokio::test]
async fn a_root_variable_that_climbs_out_is_refused_on_both_faces_and_the_row_says_why() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let tmp = tempfile::tempdir().unwrap();
    write_cc_plugin(
        tmp.path(),
        "p415-escape",
        "",
        Some(r#"{"mcpServers":{"sneaky":{"command":"${CLAUDE_PLUGIN_ROOT}/../outside.js"}}}"#),
    );
    std::fs::write(tmp.path().join("plugins/outside.js"), "#!/bin/sh\n").unwrap();

    let manager = attached_manager(tmp.path()).await;
    manager.load_all().await.unwrap();

    let row = settled_row(&manager, "p415-escape").await;
    assert_eq!(row.mcp_servers_count, 0, "count face: {row:?}");
    assert_eq!(row.status, "error", "{row:?}");
    let detail = row.error.clone().unwrap_or_default();
    assert!(
        detail.contains("'sneaky'") && detail.contains("outside the plugin root"),
        "the row must name the server and the reason: {detail}"
    );
    assert!(
        !detail.contains("mcp:plugin:"),
        "spawn face started a refused server: {detail}"
    );
    assert!(
        manager
            .scope_steps("p415-escape")
            .is_none_or(|steps| !steps.contains(&"mcp_server")),
        "spawn face mounted a refused server"
    );
}

/// The kind is derived from `plugin.json` AND the `.mcp.json` beside it, and
/// the manifest cache is keyed on `plugin.json`. A `.mcp.json` added after the
/// first load must still make the next load an MCP one — otherwise the row
/// counts the new server (the adapter parse is uncached) and nothing starts it.
#[tokio::test]
async fn a_dot_mcp_json_added_after_the_first_load_is_counted_and_spawned_on_reload() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let tmp = tempfile::tempdir().unwrap();
    let root = write_cc_plugin(tmp.path(), "p415-late", "", None);
    let manager = attached_manager(tmp.path()).await;
    manager.load_all().await.unwrap();
    let before = row(&manager.get_plugin_info().await, "p415-late").clone();
    assert_eq!(
        (before.kind.as_str(), before.mcp_servers_count),
        ("static", 0)
    );

    std::fs::write(
        root.join(".mcp.json"),
        r#"{"mcpServers":{"srv":{"command":"qa-nonexistent-mcp-binary-9f3a"}}}"#,
    )
    .unwrap();
    manager.reload().await.unwrap();

    let row = settled_row(&manager, "p415-late").await;
    assert_counted_and_spawned(&row, 1, &[("srv", "(qa-nonexistent-mcp-binary-9f3a)")]);
}

/// An explicit non-`mcp` runtime never starts servers, so the row counts none
/// of the servers it declares — and says why, instead of counting what will
/// never run.
#[tokio::test]
async fn an_explicit_non_mcp_runtime_counts_no_servers_and_says_why() {
    let _home = crate::utils::paths::IsolatedAlephHome::new();
    let tmp = tempfile::tempdir().unwrap();
    write_cc_plugin(
        tmp.path(),
        "p415-static",
        r#","aleph":{"runtime":"static"}"#,
        Some(r#"{"mcpServers":{"srv":{"command":"qa-nonexistent-mcp-binary-9f3a"}}}"#),
    );
    let manager = attached_manager(tmp.path()).await;
    manager.load_all().await.unwrap();

    let row = settled_row(&manager, "p415-static").await;
    assert_eq!(row.kind, "static", "{row:?}");
    assert_eq!(row.mcp_servers_count, 0, "count face: {row:?}");
    assert_eq!(row.status, "loaded", "{row:?}");
    let detail = row.error.clone().unwrap_or_default();
    assert!(
        detail.contains("1 MCP server") && detail.contains("`static`"),
        "the row must say why the declared server is not counted: {row:?}"
    );
    assert!(
        manager
            .scope_steps("p415-static")
            .is_some_and(|steps| !steps.contains(&"mcp_server")),
        "a non-mcp runtime started a server"
    );
}
