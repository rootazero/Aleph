#![cfg(feature = "native-qa")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use alephcore::discovery::DiscoveryConfig;
use alephcore::extension::{init_extension_manager, ExtensionConfig, ExtensionManager};
use alephcore::gateway::handlers::auth::TransportPolicy;
use alephcore::gateway::handlers::connect::{handle_connect, ConnectContext};
use alephcore::gateway::security::{DeviceTokenManager, SecurityStore};
use alephcore::gateway::server::GatewayConfig;
use alephcore::gateway::state_version::StateVersionTracker;
use alephcore::gateway::GatewayServer;
use alephcore::projects::ProjectStore;
use axum::{
    extract::{Request, State},
    middleware::{self, Next},
    response::Response,
};
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Mutex, Notify};
use tower_http::services::{ServeDir, ServeFile};

#[derive(Debug)]
struct Args {
    owned_root: PathBuf,
    nonce: String,
    panel_root: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    v: u8,
    r#type: String,
    run: String,
    id: u32,
    command: String,
    user: Option<String>,
    device: Option<String>,
    action: Option<String>,
    gate: Option<String>,
}

#[derive(Debug, Serialize)]
struct Ready<'a> {
    v: u8,
    r#type: &'a str,
    run: &'a str,
    origin: String,
}

#[derive(Debug, Serialize)]
struct ErrorMessage<'a> {
    v: u8,
    r#type: &'a str,
    run: &'a str,
    code: &'a str,
}

#[derive(Clone)]
struct Fixture {
    origin: String,
    tokens: Arc<DeviceTokenManager>,
    projects: Arc<ProjectStore>,
    alice_project: String,
}

#[derive(Debug, Serialize)]
struct Pairing<'a> {
    v: u8,
    r#type: &'a str,
    run: &'a str,
    user: &'a str,
    device: &'a str,
    ticket: String,
}

#[derive(Debug, Serialize)]
struct Ack<'a> {
    v: u8,
    r#type: &'a str,
    run: &'a str,
    id: u32,
    command: &'a str,
    status: &'a str,
}

#[derive(Clone)]
struct GatewayState {
    run: String,
    asset_a_held: bool,
    asset_a_notify: Arc<Notify>,
    tokens: Arc<DeviceTokenManager>,
    receipts: Arc<Mutex<Vec<StoredReceipt>>>,
    state_versions: Arc<StateVersionTracker>,
}

#[derive(Clone, Serialize)]
struct StoredReceipt {
    v: u8,
    r#type: String,
    run: String,
    seq: u32,
    user: String,
    device: String,
    operation: String,
    project: String,
    revision: String,
    result: String,
    status: String,
}

#[derive(Debug, Serialize)]
struct Stopped<'a> {
    v: u8,
    r#type: &'a str,
    run: &'a str,
    #[serde(rename = "listenerClosed")]
    listener_closed: bool,
    #[serde(rename = "fixtureClosed")]
    fixture_closed: bool,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    match run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_error) => {
            eprintln!("{}", r#"{"ok":false,"code":"internal"}"#);
            std::process::ExitCode::from(1)
        }
    }
}

async fn run() -> anyhow::Result<()> {
    let args = parse_args(std::env::args_os())?;
    validate_nonce(&args.nonce)?;
    let aleph_home = args.owned_root.join(".aleph");
    std::fs::create_dir_all(&aleph_home)?;
    std::env::set_var("ALEPH_HOME", &aleph_home);
    std::fs::create_dir_all(&args.panel_root)?;
    write_panel(&aleph_home, "community-live", b"export const nativeQa = true;")?;

    let projects = ProjectStore::shared();
    projects.create_schema()?;
    let a_root = args.owned_root.join("project-a");
    let b_root = args.owned_root.join("project-b");
    write_panel(&a_root.join(".aleph"), "project-a", b"export const project = 'a';")?;
    write_panel(&b_root.join(".aleph"), "project-b", b"export const project = 'b';")?;
    let project_a = projects.create("A", Some("alice"), Some(&a_root))?;
    let project_b = projects.create("B", Some("bob"), Some(&b_root))?;
    let c_root = args.owned_root.join("project-c");
    write_panel(&c_root.join(".aleph"), "project-c", b"export const project = 'c';")?;
    let _project_c = projects.create("C", Some("alice"), Some(&c_root))?;
    projects.add_member(&project_a.id, "alice")?;
    projects.add_member(&project_b.id, "alice")?;

    let manager = ExtensionManager::new(ExtensionConfig {
        discovery: DiscoveryConfig {
            working_dir: args.owned_root.clone(),
            scan_claude_dirs: false,
            scan_project_dirs: false,
            max_upward_depth: 0,
        },
        plugins_config_path: Some(aleph_home.join("plugins.toml")),
        ..ExtensionConfig::default()
    })
    .await?;
    manager.load_all().await?;
    let manager = Arc::new(manager);
    init_extension_manager(manager).map_err(|_| anyhow::anyhow!("extension_manager_already_initialized"))?;

    let security = Arc::new(SecurityStore::in_memory()?);
    security.create_user("alice", "Alice", alephcore::gateway::security::store::UserRole::Member)?;
    security.create_user("bob", "Bob", alephcore::gateway::security::store::UserRole::Member)?;
    let tokens = Arc::new(DeviceTokenManager::new(security.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let origin = format!("http://{address}");
    let mut gateway = GatewayServer::with_config(
        address,
        GatewayConfig {
            require_idempotency_key: true,
            ..GatewayConfig::default()
        },
    );
    gateway.set_device_token_manager(tokens.clone());
    gateway.set_security_store(security.clone());
    let state_versions = Arc::new(StateVersionTracker::new());
    let context = Arc::new(ConnectContext {
        state_versions: state_versions.clone(),
        transport_policy: TransportPolicy::defaults(),
    });
    gateway.handlers_mut().register("connect", move |request| {
        handle_connect(request, context.clone())
    });
    let gateway_state = Arc::new(Mutex::new(GatewayState {
        run: args.nonce.clone(),
        asset_a_held: false,
        asset_a_notify: Arc::new(Notify::new()),
        tokens: tokens.clone(),
        receipts: Arc::new(Mutex::new(Vec::new())),
        state_versions: state_versions.clone(),
    }));
    let router = gateway
        .build_router()
        .layer(middleware::from_fn_with_state(gateway_state.clone(), delay_asset_response))
        .fallback_service(ServeDir::new(&args.panel_root).fallback(ServeFile::new(args.panel_root.join("index.html"))));
    let fixture = Fixture {
        origin: origin.clone(),
        tokens,
        projects,
        alice_project: project_a.id,
    };
    let fixture = Arc::new(Mutex::new(fixture));
    let run = args.nonce;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let control = control_loop(run.clone(), fixture, gateway_state, server, shutdown_tx).await;
    control
}

async fn control_loop(
    run: String,
    fixture: Arc<Mutex<Fixture>>,
    gateway_state: Arc<Mutex<GatewayState>>,
    server: tokio::task::JoinHandle<Result<(), std::io::Error>>,
    shutdown: oneshot::Sender<()>,
) -> anyhow::Result<()> {
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::BufWriter::new(tokio::io::stdout());
    write_json(&mut output, &Ready { v: 1, r#type: "ready", run: &run, origin: fixture.lock().await.origin.clone() }).await?;
    while let Some(line) = input.next_line().await? {
        let command: Command = match serde_json::from_str(&line) {
            Ok(command) => command,
            Err(_) => {
                write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "bad_command" }).await?;
                continue;
            }
        };
        if command.v != 1 || command.r#type != "command" || command.run != run {
            write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "bad_command" }).await?;
            continue;
        }
        if command.command == "issue_pairing" {
            let user = command.user.as_deref().unwrap_or("");
            let device = command.device.as_deref().unwrap_or("");
            if !matches!(user, "alice" | "bob") || !matches!(device, "device-a" | "device-b") {
                write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "bad_command" }).await?;
                continue;
            }
            let ticket = fixture.lock().await.tokens.create_bootstrap_ticket(None, Some(user))?;
            write_json(&mut output, &Pairing { v: 1, r#type: "pairing", run: &run, user, device, ticket }).await?;
            continue;
        }
        if command.command == "set_fixture" {
            let action = command.action.as_deref().unwrap_or("");
            let action_result = {
                let fixture_guard = fixture.lock().await;
                apply_fixture_action(&fixture_guard, action)
            };
            if action_result.is_err() {
                write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "bad_state" }).await?;
                continue;
            }
            write_json(&mut output, &Ack { v: 1, r#type: "ack", run: &run, id: command.id, command: "set_fixture", status: "ok" }).await?;
            continue;
        }
        if command.command == "hold_response" {
            if command.gate.as_deref() != Some("asset_a") {
                write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "bad_command" }).await?;
                continue;
            }
            gateway_state.lock().await.asset_a_held = true;
            write_json(&mut output, &Ack { v: 1, r#type: "ack", run: &run, id: command.id, command: "hold_response", status: "ok" }).await?;
            continue;
        }
        if command.command == "release_response" {
            if command.gate.as_deref() != Some("asset_a") {
                write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "bad_command" }).await?;
                continue;
            }
            let notify = {
                let mut state = gateway_state.lock().await;
                state.asset_a_held = false;
                state.asset_a_notify.clone()
            };
            notify.notify_waiters();
            write_json(&mut output, &Ack { v: 1, r#type: "ack", run: &run, id: command.id, command: "release_response", status: "ok" }).await?;
            continue;
        }
        if command.command == "revoke_exact" {
            let device = command.device.as_deref().unwrap_or("");
            if !matches!(device, "device-a" | "device-b") {
                write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "bad_command" }).await?;
                continue;
            }
            let revoked = fixture.lock().await.tokens.revoke_panel_device(device)?;
            if !revoked {
                write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "already_revoked" }).await?;
                continue;
            }
            write_json(&mut output, &Ack { v: 1, r#type: "ack", run: &run, id: command.id, command: "revoke_exact", status: "ok" }).await?;
            continue;
        }
        if command.command == "receipt_since" {
            let receipt = gateway_state.lock().await.receipts.lock().await.last().cloned();
            if let Some(receipt) = receipt {
                write_json(&mut output, &receipt).await?;
            } else {
                write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "not_found" }).await?;
            }
            continue;
        }
        if command.command == "shutdown" {
            let _ = shutdown.send(());
            let _ = server.await;
            write_json(&mut output, &Stopped { v: 1, r#type: "stopped", run: &run, listener_closed: true, fixture_closed: true }).await?;
            return Ok(());
        }
        write_json(&mut output, &ErrorMessage { v: 1, r#type: "error", run: &run, code: "bad_state" }).await?;
    }
    let _ = shutdown.send(());
    let _ = server.await;
    Ok(())
}

async fn delay_asset_response(State(gateway_state): State<Arc<Mutex<GatewayState>>>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let authorization = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_owned);
    let response = {
        let wait = if path.starts_with("/tabs/") {
            let mut state = gateway_state.lock().await;
            if state.asset_a_held {
                state.asset_a_held = false;
                Some(state.asset_a_notify.clone().notified_owned())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(notified) = wait {
            notified.await;
        }
        next.run(request).await
    };
    if path.starts_with("/tabs/") {
        if let Some(token) = authorization {
            let state = gateway_state.lock().await;
            if let Ok(Some(device)) = state.tokens.validate_device_token(&token) {
                if let Ok(Some(user)) = state.tokens.store().device_user(&device.device_id) {
                    let version = state.state_versions.snapshot();
                    let revision = format!(
                        "{:016x}{:016x}{:016x}",
                        version.presence,
                        version.health,
                        version.config,
                    );
                    let seq = state.receipts.lock().await.len() as u32 + 1;
                    let result = if response.status().is_success() { "asset_ok" } else { "asset_404" };
                    let project = if path.contains("project-a") {
                        "project-a"
                    } else if path.contains("project-b") {
                        "project-b"
                    } else {
                        "global"
                    };
                    state.receipts.lock().await.push(StoredReceipt {
                        v: 1,
                        r#type: "receipt".to_owned(),
                        run: state.run.clone(),
                        seq,
                        user,
                        device: device.device_id,
                        operation: "asset".to_owned(),
                        project: project.to_owned(),
                        revision,
                        result: result.to_owned(),
                        status: response.status().as_u16().to_string(),
                    });
                }
            }
        }
    }
    response
}

fn apply_fixture_action(fixture: &Fixture, action: &str) -> anyhow::Result<()> {
    match action {
        "global" | "select_a" | "select_b" | "clear_global" | "rebind_workspace_c" => Ok(()),
        "unbind_workspace" => {
            fixture.projects.bind_workspace(&fixture.alice_project, None)?;
            Ok(())
        }
        "revoke_membership" => {
            fixture.projects.remove_member(&fixture.alice_project, "alice")?;
            Ok(())
        }
        "archive_a" => {
            fixture.projects.archive(&fixture.alice_project)?;
            Ok(())
        }
        "revoke_device_a" => {
            fixture.tokens.revoke_panel_device("device-a")?;
            Ok(())
        }
        _ => anyhow::bail!("unknown_action"),
    }
}

async fn write_json<W, T>(output: &mut W, value: &T) -> anyhow::Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    output.write_all(serde_json::to_string(value)?.as_bytes()).await?;
    output.write_all(b"\n").await?;
    output.flush().await?;
    Ok(())
}

fn parse_args<I>(args: I) -> anyhow::Result<Args>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let mut values = std::collections::HashMap::new();
    let mut iter = args.into_iter().skip(1);
    while let Some(flag) = iter.next() {
        let key = flag.to_string_lossy().to_string();
        if !matches!(key.as_str(), "--owned-root" | "--nonce" | "--panel-root") {
            anyhow::bail!("bad_argument");
        }
        if values.contains_key(&key) {
            anyhow::bail!("duplicate_argument");
        }
        values.insert(key, iter.next().ok_or_else(|| anyhow::anyhow!("missing_value"))?);
    }
    Ok(Args {
        owned_root: PathBuf::from(values.remove("--owned-root").ok_or_else(|| anyhow::anyhow!("missing_root"))?),
        nonce: values.remove("--nonce").ok_or_else(|| anyhow::anyhow!("missing_nonce"))?.to_string_lossy().to_string(),
        panel_root: PathBuf::from(values.remove("--panel-root").ok_or_else(|| anyhow::anyhow!("missing_panel"))?),
    })
}

fn validate_nonce(nonce: &str) -> anyhow::Result<()> {
    if !(32..=128).contains(&nonce.len()) || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) {
        anyhow::bail!("bad_nonce");
    }
    Ok(())
}

fn write_panel(home: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let root = home.join("plugins").join(name);
    std::fs::create_dir_all(root.join("ui"))?;
    std::fs::write(root.join("ui/panel.mjs"), bytes)?;
    let integrity = base64::engine::general_purpose::STANDARD.encode(Sha256::digest(bytes));
    std::fs::write(root.join("generic.mjs"), b"generic")?;
    std::fs::write(root.join("aleph.plugin.toml"), format!(
        "[plugin]\nid = \"{name}\"\nname = \"{name}\"\nversion = \"1.0.0\"\nkind = \"static\"\nentry = \"generic.mjs\"\n\n[panel]\ntab_id = \"overview\"\nlabel = \"{name}\"\nicon = \"layout\"\norder = 0\nentry = \"ui/panel.mjs\"\napi_major = 1\npermissions = []\nasset_integrity = \"sha256-{integrity}\"\n"
    ))?;
    Ok(())
}
