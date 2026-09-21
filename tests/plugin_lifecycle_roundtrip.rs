//! Plugin lifecycle round trip against a REAL MCP server process.
//!
//! enable → disable → enable of an MCP-kind plugin that also declares a
//! `[memory]` section and a `commands/*.md`: the transient server starts /
//! is removed / starts again (observed on the manager's event bus), the
//! memory extension appears / disappears / reappears, the slash entry too.
//!
//! This is the observation `lifecycle.rs::tests::g2_*` cannot make: the five
//! other disposers are proven in-process there, but the `mcp_server` step
//! only becomes visible when a real child is spawned, handshaken, killed and
//! spawned again. `ServerRemoved` for a transient server is emitted ONLY
//! after the actor found a running client and `child.kill().await` returned
//! (`mcp/manager/actor.rs::remove_transient_server`), so waiting for it is
//! waiting for the effect, not for the call.
//!
//! `harness = false` and self-re-exec: with `ALEPH_QA_MCP_MOCK=1` this same
//! binary runs a minimal stdio MCP server (legacy handshake: `server/discover`
//! → `-32601`, `initialize` → `2025-03-26`, `tools/list` → one tool). No
//! python, no network, no second crate — the mock is the four `match` arms in
//! `run_mock_server`.
//!
//! `ALEPH_HOME` is pointed at a fresh tempdir in `main`, BEFORE the tokio
//! runtime exists: `ProjectStore::shared()` resolves it process-globally from
//! inside `load_all`, and `set_var` after worker threads are up races every
//! `getenv` on them. The tempdir is removed on every exit path — there is no
//! early `process::exit` between its creation and `TempDir::close`.
//!
//! Exit code is the verdict: 0 = every claim held; 1 = at least one did not
//! (each is printed as `[PASS]` / `[FAIL]`). Set `RUST_LOG=debug` to see the
//! manager's own log (handshake errors land there).

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use alephcore::discovery::DiscoveryConfig;
use alephcore::extension::{ExtensionConfig, ExtensionManager};
use alephcore::mcp::manager::{McpManagerActor, McpManagerEvent};
use alephcore::memory::extensions::{MemoryExtension, MemoryExtensionRegistry};
use alephcore::tool_metadata::ToolCatalog;
use serde_json::json;
use tokio::sync::broadcast;

const PLUGIN_ID: &str = "qa-mcp-mock";
const PLUGIN_NAME: &str = "QA MCP Mock";
/// `extension/mcp_config.rs`: `plugin:<plugin_id>/<server_name>`.
const SERVER_ID: &str = "plugin:qa-mcp-mock/mock";
/// `extension/registry/types.rs::namespaced_component_key`: `<plugin_id>:<name>`.
const SLASH: &str = "qa-mcp-mock:ping";
const WAIT: Duration = Duration::from_secs(30);

fn main() {
    if std::env::var_os("ALEPH_QA_MCP_MOCK").is_some() {
        run_mock_server();
        return;
    }
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();

    // Created and exported before the runtime spawns a single worker thread
    // (see the file header). Everything Aleph resolves off `ALEPH_HOME` —
    // the plugin parent, `data/projects.db`, `plugins.toml` — lands here.
    let home = tempfile::Builder::new()
        .prefix("aleph-plugin-roundtrip-")
        .tempdir()
        .expect("tempdir for isolated ALEPH_HOME");
    std::env::set_var("ALEPH_HOME", home.path());

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mut failures = rt.block_on(drive(home.path()));

    // Tear the runtime down BEFORE removing the tree: the actor task owns the
    // mock child (`kill_on_drop`) and the manager's state lives under `home`.
    rt.shutdown_timeout(Duration::from_secs(10));
    failures += remove_home(home);

    if failures == 0 {
        println!("VERDICT: PASS");
    } else {
        println!("VERDICT: FAIL ({failures} claim(s))");
        std::process::exit(1);
    }
}

/// Remove the scratch home and report it as a claim of its own: a test that
/// leaves `$TMPDIR` litter is a finding in this repo.
///
/// The one thing still holding a file under `home` at this point is the
/// process-global `ProjectStore` (`projects/store.rs::shared`, a `OnceLock`
/// that never drops) with `data/projects.db`. On unix an open descriptor does
/// not block unlink, so the removal must succeed and a failure counts. On
/// Windows SQLite opens without `FILE_SHARE_DELETE`, so that one file cannot
/// be deleted by any means available to this process — the same limitation
/// `utils::scratch::keep_until_exit` documents for its non-unix arm. It is
/// printed, not counted, so the test does not report a structural residue as
/// a lifecycle defect.
fn remove_home(home: tempfile::TempDir) -> usize {
    let path = home.path().to_path_buf();
    match home.close() {
        Ok(()) => {
            println!("  [PASS] scratch ALEPH_HOME removed — {}", path.display());
            0
        }
        Err(e) if cfg!(unix) => {
            println!(
                "  [FAIL] scratch ALEPH_HOME removed — {}: {e}",
                path.display()
            );
            1
        }
        Err(e) => {
            println!(
                "  [LEAK] scratch ALEPH_HOME could not be fully removed on this platform \
                 (process-global sqlite handle) — {}: {e}",
                path.display()
            );
            0
        }
    }
}

// ── the mock server ───────────────────────────────────────────────────────

fn run_mock_server() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        // Notifications (no id) get no answer.
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let response = match method {
            "initialize" => json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "qa-mock", "version": "0" }
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0", "id": id,
                "result": { "tools": [{
                    "name": "qa_echo",
                    "description": "echoes its input",
                    "inputSchema": { "type": "object", "properties": {} }
                }] }
            }),
            "ping" => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
            other => json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("method not found: {other}") }
            }),
        };
        if writeln!(out, "{response}").is_err() {
            break;
        }
        let _ = out.flush();
    }
}

// ── the drive ─────────────────────────────────────────────────────────────

struct Ledger(usize);
impl Ledger {
    fn check(&mut self, claim: &str, ok: bool, detail: impl std::fmt::Display) {
        println!(
            "  [{}] {claim} — {detail}",
            if ok { "PASS" } else { "FAIL" }
        );
        if !ok {
            self.0 += 1;
        }
    }
}

/// `$ALEPH_HOME/plugins/installed/<id>`: `installed/` has no manifest, so
/// `discovery/scanner.rs::scan_plugin_parent` reads it one level deeper
/// (the monorepo rule) and finds the plugin root beneath it.
fn plant_plugin(aleph_home: &Path) {
    let dir = aleph_home.join("plugins").join("installed").join(PLUGIN_ID);
    std::fs::create_dir_all(dir.join("commands")).unwrap();
    std::fs::write(
        dir.join("aleph.plugin.toml"),
        format!(
            "[plugin]\nid = \"{PLUGIN_ID}\"\nname = \"{PLUGIN_NAME}\"\nkind = \"mcp\"\n\n\
             [memory]\nhooks = [\"on_retrieve\"]\npriority = 50\n"
        ),
    )
    .unwrap();
    let me = std::env::current_exe().unwrap();
    std::fs::write(
        dir.join(".mcp.json"),
        serde_json::to_string_pretty(&json!({
            "mcpServers": {
                "mock": {
                    "command": me.to_string_lossy(),
                    "args": [],
                    "env": { "ALEPH_QA_MCP_MOCK": "1" }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("commands/ping.md"),
        "---\ndescription: ping\n---\nPong\n",
    )
    .unwrap();
}

async fn wait_for(
    events: &mut broadcast::Receiver<McpManagerEvent>,
    pred: impl Fn(&McpManagerEvent) -> bool,
) -> bool {
    tokio::time::timeout(WAIT, async {
        loop {
            match events.recv().await {
                Ok(e) if pred(&e) => break true,
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break false,
            }
        }
    })
    .await
    .unwrap_or(false)
}

async fn has_slash(catalog: &ToolCatalog) -> bool {
    catalog.list_all().await.iter().any(|t| t.name == SLASH)
}

fn started(e: &McpManagerEvent) -> bool {
    matches!(e, McpManagerEvent::ServerStarted { server_id, .. } if server_id == SERVER_ID)
}
fn removed(e: &McpManagerEvent) -> bool {
    matches!(e, McpManagerEvent::ServerRemoved { server_id, .. } if server_id == SERVER_ID)
}

async fn drive(aleph_home: &Path) -> usize {
    let mut led = Ledger(0);
    plant_plugin(aleph_home);

    // `ExtensionConfig::extra_plugin_parents` is `cfg(test)` — invisible from
    // `tests/`, so this literal is complete (`..Default::default()` would be
    // `clippy::needless_update`).
    let manager = ExtensionManager::new(ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: aleph_home.to_path_buf(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
        },
        plugins_config_path: Some(aleph_home.join("plugins.toml")),
    })
    .await
    .unwrap();

    let (actor, handle) = McpManagerActor::new(Some(aleph_home.join("mcp.json")))
        .await
        .unwrap();
    tokio::spawn(actor.run());
    let mut events = handle.subscribe(); // BEFORE the first mount
    manager.set_mcp_handle(handle);
    let memory = Arc::new(MemoryExtensionRegistry::new());
    manager.set_memory_registry(memory.clone());
    let catalog = Arc::new(ToolCatalog::new());
    manager.set_tool_catalog(catalog.clone());

    let memory_names = || -> Vec<String> {
        memory
            .mcp_bindings_snapshot()
            .iter()
            .map(|e| e.name().to_string())
            .collect()
    };

    // ── boot: load_all mounts it ──
    let summary = manager.load_all().await.unwrap();
    led.check(
        "load_all mounted the plugin",
        summary.plugins_loaded == 1,
        format!(
            "plugins_loaded={} errors={:?}",
            summary.plugins_loaded, summary.errors
        ),
    );
    let row = manager.get_plugin_record(PLUGIN_ID).await;
    led.check(
        "row is Loaded",
        row.as_ref().is_some_and(|r| r.status.is_active()),
        format!("{:?}", row.map(|r| r.status)),
    );
    led.check(
        "transient server started (real child, real handshake)",
        wait_for(&mut events, started).await,
        SERVER_ID,
    );
    led.check(
        "memory extension registered",
        memory_names() == vec![PLUGIN_NAME.to_string()],
        format!("{:?}", memory_names()),
    );
    led.check("slash entry registered", has_slash(&catalog).await, SLASH);

    // ── disable: everything goes ──
    led.check(
        "disable reports a change",
        manager.set_plugin_enabled(PLUGIN_ID, false).await,
        "",
    );
    led.check(
        "transient server removed",
        wait_for(&mut events, removed).await,
        SERVER_ID,
    );
    led.check(
        "memory extension gone",
        memory_names().is_empty(),
        format!("{:?}", memory_names()),
    );
    led.check("slash entry gone", !has_slash(&catalog).await, SLASH);
    let row = manager.get_plugin_record(PLUGIN_ID).await;
    led.check(
        "row is Disabled and listable",
        row.as_ref().is_some_and(|r| !r.status.is_active()),
        format!("{:?}", row.map(|r| r.status)),
    );

    // ── enable again: everything comes back (the asymmetry this round fixes) ──
    led.check(
        "enable reports a change",
        manager.set_plugin_enabled(PLUGIN_ID, true).await,
        "",
    );
    led.check(
        "transient server started AGAIN",
        wait_for(&mut events, started).await,
        SERVER_ID,
    );
    led.check(
        "memory extension back",
        memory_names() == vec![PLUGIN_NAME.to_string()],
        format!("{:?}", memory_names()),
    );
    led.check("slash entry back", has_slash(&catalog).await, SLASH);

    // ── leave the fixture clean so the child does not outlive the test ──
    led.check(
        "final disable reports a change",
        manager.set_plugin_enabled(PLUGIN_ID, false).await,
        "",
    );
    led.check(
        "transient server removed at the end",
        wait_for(&mut events, removed).await,
        SERVER_ID,
    );
    led.0
}
