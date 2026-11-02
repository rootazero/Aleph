#![cfg(feature = "native-qa")]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

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

// Reuse the shell's exact root proof; other shell accessors are unused by the helper.
#[allow(dead_code)]
#[path = "../../../desktop/shell/src/qa_root.rs"]
mod qa_root;

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
    after: Option<u32>,
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
    c_root: PathBuf,
    bindings: HashMap<String, PairingBinding>,
}

#[derive(Clone)]
struct PairingBinding {
    user: String,
    baseline: HashSet<String>,
    actual: Option<String>,
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
    fixture: Arc<Mutex<Fixture>>,
    manager: Arc<ExtensionManager>,
    receipts: Arc<StdMutex<Vec<StoredReceipt>>>,
    rpc_observer: Arc<NativeQaObserver>,
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

struct NativeQaObserver {
    run: String,
    receipts: Arc<StdMutex<Vec<StoredReceipt>>>,
    manager: Arc<ExtensionManager>,
    project_a: String,
    project_b: String,
    project_c: String,
    device_labels: StdMutex<HashMap<String, String>>,
    pending: StdMutex<Vec<alephcore::gateway::native_qa::NativeQaRpcReceipt>>,
}

impl NativeQaObserver {
    fn append(&self, receipt: alephcore::gateway::native_qa::NativeQaRpcReceipt) {
        let Some(user) = receipt.user else {
            return;
        };
        let Some(actual_device) = receipt.device else {
            return;
        };
        let Some(device) = self
            .device_labels
            .lock()
            .expect("device label mutex poisoned")
            .get(&actual_device)
            .cloned()
        else {
            self.pending
                .lock()
                .expect("pending receipt mutex poisoned")
                .push(alephcore::gateway::native_qa::NativeQaRpcReceipt {
                    user: Some(user),
                    device: Some(actual_device),
                    ..receipt
                });
            return;
        };
        let project = match receipt.project_id.as_deref() {
            None => "global",
            Some(id) if id == self.project_a => "project-a",
            Some(id) if id == self.project_b => "project-b",
            Some(id) if id == self.project_c => "project-c",
            Some(_) => return,
        };
        let (operation, result, status) = match (receipt.operation, receipt.result) {
            (
                alephcore::gateway::native_qa::NativeQaRpcOperation::Connect,
                alephcore::gateway::native_qa::NativeQaRpcResult::Authorized,
            ) => ("connect", "authorized", "ok"),
            (
                alephcore::gateway::native_qa::NativeQaRpcOperation::Select,
                alephcore::gateway::native_qa::NativeQaRpcResult::Authorized,
            ) => ("select", "authorized", "ok"),
            (
                alephcore::gateway::native_qa::NativeQaRpcOperation::Select,
                alephcore::gateway::native_qa::NativeQaRpcResult::Refused,
            ) => ("select", "refused", "refused"),
            (
                alephcore::gateway::native_qa::NativeQaRpcOperation::Discovery,
                alephcore::gateway::native_qa::NativeQaRpcResult::SnapshotOk,
            ) => ("discovery", "snapshot_ok", "ok"),
            (
                alephcore::gateway::native_qa::NativeQaRpcOperation::Discovery,
                alephcore::gateway::native_qa::NativeQaRpcResult::SnapshotError,
            ) => ("discovery", "snapshot_error", "refused"),
            _ => return,
        };
        let mut receipts = self.receipts.lock().expect("receipt mutex poisoned");
        if receipts.len() >= 512 {
            return;
        }
        let seq = receipts.len() as u32 + 1;
        receipts.push(StoredReceipt {
            v: 1,
            r#type: "receipt".to_owned(),
            run: self.run.clone(),
            seq,
            user,
            device,
            operation: operation.to_owned(),
            project: project.to_owned(),
            revision: format!("{:016x}", self.manager.panel_ui_capability_revision()),
            result: result.to_owned(),
            status: status.to_owned(),
        });
    }

    fn confirm_device(&self, label: &str, actual: String) {
        self.device_labels
            .lock()
            .expect("device label mutex poisoned")
            .insert(actual, label.to_owned());
        let pending =
            std::mem::take(&mut *self.pending.lock().expect("pending receipt mutex poisoned"));
        pending.into_iter().for_each(|receipt| self.append(receipt));
    }
}

impl alephcore::gateway::native_qa::NativeQaRpcObserver for NativeQaObserver {
    fn record(&self, receipt: alephcore::gateway::native_qa::NativeQaRpcReceipt) {
        self.append(receipt);
    }
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
    qa_root::init_from_args([
        std::ffi::OsString::from("native-qa-helper"),
        std::ffi::OsString::from("--qa-root"),
        args.owned_root.clone().into_os_string(),
        std::ffi::OsString::from("--qa-nonce"),
        args.nonce.clone().into(),
    ])?;
    let aleph_home = args.owned_root.join(".aleph");
    std::fs::create_dir_all(&aleph_home)?;
    std::env::set_var("ALEPH_HOME", &aleph_home);
    anyhow::ensure!(
        args.panel_root.join("index.html").is_file(),
        "missing_panel"
    );
    let source = include_str!("../../../interfaces/webchat/tests/fixtures/community-live.mjs");
    write_panel(&aleph_home, "community-live", source.as_bytes())?;

    let projects = ProjectStore::shared();
    projects.create_schema()?;
    let a_root = args.owned_root.join("project-a");
    let b_root = args.owned_root.join("project-b");
    write_panel(
        &a_root.join(".aleph"),
        "project-a",
        source.replace("community-live", "project-a").as_bytes(),
    )?;
    write_panel(
        &b_root.join(".aleph"),
        "project-b",
        source.replace("community-live", "project-b").as_bytes(),
    )?;
    let project_a = projects.create("Native QA A", Some("alice"), Some(&a_root))?;
    let project_b = projects.create("Native QA B", Some("bob"), Some(&b_root))?;
    let c_root = args.owned_root.join("project-c");
    write_panel(
        &c_root.join(".aleph"),
        "project-c",
        source.replace("community-live", "project-c").as_bytes(),
    )?;
    let project_c = projects.create("C", Some("alice"), Some(&c_root))?;
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
    init_extension_manager(manager.clone())
        .map_err(|_| anyhow::anyhow!("extension_manager_already_initialized"))?;

    let security = Arc::new(SecurityStore::in_memory()?);
    security.create_user(
        "alice",
        "Alice",
        alephcore::gateway::security::store::UserRole::Member,
    )?;
    security.create_user(
        "bob",
        "Bob",
        alephcore::gateway::security::store::UserRole::Member,
    )?;
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
    let receipts = Arc::new(StdMutex::new(Vec::new()));
    let rpc_observer = Arc::new(NativeQaObserver {
        run: args.nonce.clone(),
        receipts: receipts.clone(),
        manager: manager.clone(),
        project_a: project_a.id.clone(),
        project_b: project_b.id.clone(),
        project_c: project_c.id.clone(),
        device_labels: StdMutex::new(HashMap::new()),
        pending: StdMutex::new(Vec::new()),
    });
    gateway.set_native_qa_observer(rpc_observer.clone());
    let fixture = Arc::new(Mutex::new(Fixture {
        origin: origin.clone(),
        tokens,
        projects,
        alice_project: project_a.id,
        c_root,
        bindings: HashMap::new(),
    }));
    let gateway_state = Arc::new(Mutex::new(GatewayState {
        run: args.nonce.clone(),
        asset_a_held: false,
        asset_a_notify: Arc::new(Notify::new()),
        fixture: fixture.clone(),
        manager,
        receipts,
        rpc_observer,
    }));

    let router = gateway
        .build_router()
        .layer(middleware::from_fn_with_state(
            gateway_state.clone(),
            delay_asset_response,
        ))
        .fallback_service(
            ServeDir::new(&args.panel_root)
                .fallback(ServeFile::new(args.panel_root.join("index.html"))),
        );
    let run = args.nonce;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        })
        .await
    });
    let control = control_loop(
        run.clone(),
        fixture,
        gateway_state,
        Some(server),
        Some(shutdown_tx),
    )
    .await;
    control
}

async fn control_loop(
    run: String,
    fixture: Arc<Mutex<Fixture>>,
    gateway_state: Arc<Mutex<GatewayState>>,
    mut server: Option<tokio::task::JoinHandle<Result<(), std::io::Error>>>,
    mut shutdown: Option<oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut output = tokio::io::BufWriter::new(tokio::io::stdout());
    write_json(
        &mut output,
        &Ready {
            v: 1,
            r#type: "ready",
            run: &run,
            origin: fixture.lock().await.origin.clone(),
        },
    )
    .await?;
    while let Some(line) = input.next_line().await? {
        let command: Command = match serde_json::from_str(&line) {
            Ok(command) => command,
            Err(_) => {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "bad_command",
                    },
                )
                .await?;
                continue;
            }
        };
        if command.v != 1 || command.r#type != "command" || command.run != run {
            write_json(
                &mut output,
                &ErrorMessage {
                    v: 1,
                    r#type: "error",
                    run: &run,
                    code: "bad_command",
                },
            )
            .await?;
            continue;
        }
        if command.command == "issue_pairing" {
            let user = command.user.as_deref().unwrap_or("");
            let device = command.device.as_deref().unwrap_or("");
            if !matches!(user, "alice" | "bob") || !matches!(device, "device-a" | "device-b") {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "bad_command",
                    },
                )
                .await?;
                continue;
            }
            let mut fixture = fixture.lock().await;
            if fixture.bindings.contains_key(device)
                || fixture
                    .bindings
                    .values()
                    .any(|binding| binding.user == user && binding.actual.is_none())
            {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "bad_state",
                    },
                )
                .await?;
                continue;
            }
            let baseline = fixture
                .tokens
                .list_panel_devices()?
                .into_iter()
                .map(|row| row.device_id)
                .collect();
            let ticket = fixture.tokens.create_bootstrap_ticket(None, Some(user))?;
            fixture.bindings.insert(
                device.to_owned(),
                PairingBinding {
                    user: user.to_owned(),
                    baseline,
                    actual: None,
                },
            );
            write_json(
                &mut output,
                &Pairing {
                    v: 1,
                    r#type: "pairing",
                    run: &run,
                    user,
                    device,
                    ticket,
                },
            )
            .await?;
            continue;
        }
        if command.command == "confirm_pairing" {
            let device = command.device.as_deref().unwrap_or("");
            let confirmed = {
                let mut fixture = fixture.lock().await;
                resolve_device(&mut fixture, device)
            };
            if let Ok(actual) = &confirmed {
                gateway_state
                    .lock()
                    .await
                    .rpc_observer
                    .confirm_device(device, actual.clone());
            }
            if confirmed.is_err() {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "bad_state",
                    },
                )
                .await?;
                continue;
            }
            write_json(
                &mut output,
                &Ack {
                    v: 1,
                    r#type: "ack",
                    run: &run,
                    id: command.id,
                    command: "confirm_pairing",
                    status: "ok",
                },
            )
            .await?;
            continue;
        }
        if command.command == "set_fixture" {
            let action = command.action.as_deref().unwrap_or("");
            let action_result = {
                let mut fixture_guard = fixture.lock().await;
                apply_fixture_action(&mut fixture_guard, action)
            };
            if action_result.is_err() {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "bad_state",
                    },
                )
                .await?;
                continue;
            }
            write_json(
                &mut output,
                &Ack {
                    v: 1,
                    r#type: "ack",
                    run: &run,
                    id: command.id,
                    command: "set_fixture",
                    status: "ok",
                },
            )
            .await?;
            continue;
        }
        if command.command == "hold_response" {
            if command.gate.as_deref() != Some("asset_a") {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "bad_command",
                    },
                )
                .await?;
                continue;
            }
            {
                let mut state = gateway_state.lock().await;
                state.asset_a_held = true;
                state.asset_a_notify = Arc::new(Notify::new());
            }
            write_json(
                &mut output,
                &Ack {
                    v: 1,
                    r#type: "ack",
                    run: &run,
                    id: command.id,
                    command: "hold_response",
                    status: "ok",
                },
            )
            .await?;
            continue;
        }
        if command.command == "release_response" {
            if command.gate.as_deref() != Some("asset_a") {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "bad_command",
                    },
                )
                .await?;
                continue;
            }
            let notify = {
                let mut state = gateway_state.lock().await;
                state.asset_a_held = false;
                state.asset_a_notify.clone()
            };
            notify.notify_one();
            write_json(
                &mut output,
                &Ack {
                    v: 1,
                    r#type: "ack",
                    run: &run,
                    id: command.id,
                    command: "release_response",
                    status: "ok",
                },
            )
            .await?;
            continue;
        }
        if command.command == "revoke_exact" {
            let device = command.device.as_deref().unwrap_or("");
            if !matches!(device, "device-a" | "device-b") {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "bad_command",
                    },
                )
                .await?;
                continue;
            }
            let mut fixture = fixture.lock().await;
            let actual = match resolve_device(&mut fixture, device) {
                Ok(actual) => actual,
                Err(error) if error.to_string() == "device_not_found" => {
                    write_json(
                        &mut output,
                        &ErrorMessage {
                            v: 1,
                            r#type: "error",
                            run: &run,
                            code: "already_revoked",
                        },
                    )
                    .await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let revoked = fixture.tokens.revoke_panel_device(&actual)?;
            if !revoked {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "already_revoked",
                    },
                )
                .await?;
                continue;
            }
            write_json(
                &mut output,
                &Ack {
                    v: 1,
                    r#type: "ack",
                    run: &run,
                    id: command.id,
                    command: "revoke_exact",
                    status: "ok",
                },
            )
            .await?;
            continue;
        }
        if command.command == "receipt_since" {
            let Some(after) = command.after else {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "bad_command",
                    },
                )
                .await?;
                continue;
            };
            let receipt = gateway_state
                .lock()
                .await
                .receipts
                .lock()
                .expect("receipt mutex poisoned")
                .iter()
                .find(|receipt| receipt.seq > after)
                .cloned();
            if let Some(receipt) = receipt {
                write_json(&mut output, &receipt).await?;
            } else {
                write_json(
                    &mut output,
                    &ErrorMessage {
                        v: 1,
                        r#type: "error",
                        run: &run,
                        code: "not_found",
                    },
                )
                .await?;
            }
            continue;
        }
        if command.command == "shutdown" {
            if let Some(shutdown) = shutdown.take() {
                let _ = shutdown.send(());
            }
            if let Some(server) = server.take() {
                server.await??;
            }
            write_json(
                &mut output,
                &Stopped {
                    v: 1,
                    r#type: "stopped",
                    run: &run,
                    listener_closed: true,
                    fixture_closed: true,
                },
            )
            .await?;
            continue;
        }
        write_json(
            &mut output,
            &ErrorMessage {
                v: 1,
                r#type: "error",
                run: &run,
                code: "bad_state",
            },
        )
        .await?;
    }
    if let Some(shutdown) = shutdown.take() {
        let _ = shutdown.send(());
    }
    if let Some(server) = server.take() {
        server.await??;
    }
    Ok(())
}

async fn delay_asset_response(
    State(gateway_state): State<Arc<Mutex<GatewayState>>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    if !path.starts_with("/tabs/") {
        return next.run(request).await;
    }
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let state = gateway_state.lock().await;
    let fixture = state.fixture.lock().await;
    let identity = token
        .and_then(|token| fixture.tokens.validate_device_token(token).ok().flatten())
        .and_then(|device| {
            // Observation reads only bindings established by the explicit pairing confirmation.
            fixture.bindings.iter().find_map(|(label, binding)| {
                (binding.actual.as_deref() == Some(device.device_id.as_str()))
                    .then(|| (binding.user.clone(), label.clone()))
            })
        });
    let revision = format!("{:016x}", state.manager.panel_ui_capability_revision());
    drop(fixture);
    drop(state);
    let response = next.run(request).await;
    if path.starts_with("/tabs/") {
        if let Some((user, device)) = identity {
            let state = gateway_state.lock().await;
            let mut receipts = state.receipts.lock().expect("receipt mutex poisoned");
            let result = match response.status().as_u16() {
                200 => Some(("asset_ok", "ok")),
                404 => Some(("asset_404", "refused")),
                _ => None, // A transport/server error cannot be upgraded to an authorization refusal.
            };
            if let Some((result, status)) = result.filter(|_| receipts.len() < 512) {
                let plugin = path
                    .strip_prefix("/tabs/")
                    .and_then(|path| path.split('/').next());
                let project = match plugin {
                    Some("project-a") => "project-a",
                    Some("project-b") => "project-b",
                    Some("project-c") => "project-c",
                    _ => "global",
                };
                let seq = receipts.len() as u32 + 1;
                receipts.push(StoredReceipt {
                    v: 1,
                    r#type: "receipt".to_owned(),
                    run: state.run.clone(),
                    seq,
                    user,
                    device,
                    operation: "asset".to_owned(),
                    project: project.to_owned(),
                    revision,
                    result: result.to_owned(),
                    status: status.to_owned(),
                });
            }
        }
    }
    // Hold the actual authorized A response, after the production route has run.
    // A held request would instead be re-authorized under B, missing the late-body case.
    let wait = if path.starts_with("/tabs/project-a/") {
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
    response
}

fn resolve_device(fixture: &mut Fixture, label: &str) -> anyhow::Result<String> {
    let binding = fixture
        .bindings
        .get(label)
        .ok_or_else(|| anyhow::anyhow!("unknown_device"))?;
    if let Some(actual) = &binding.actual {
        return Ok(actual.clone());
    }
    let candidates = fixture
        .tokens
        .list_panel_devices()?
        .into_iter()
        .filter(|device| {
            !binding.baseline.contains(&device.device_id)
                && device.user_id.as_deref() == Some(binding.user.as_str())
                && !fixture
                    .bindings
                    .values()
                    .any(|other| other.actual.as_deref() == Some(device.device_id.as_str()))
        })
        .map(|device| device.device_id)
        .collect::<Vec<_>>();
    anyhow::ensure!(!candidates.is_empty(), "device_not_found");
    anyhow::ensure!(candidates.len() == 1, "ambiguous_device");
    fixture
        .bindings
        .get_mut(label)
        .expect("binding exists")
        .actual = Some(candidates[0].clone());
    Ok(candidates[0].clone())
}

fn apply_fixture_action(fixture: &mut Fixture, action: &str) -> anyhow::Result<()> {
    match action {
        "unbind_workspace" => {
            fixture
                .projects
                .bind_workspace(&fixture.alice_project, None)?;
        }
        "rebind_workspace_c" => {
            fixture
                .projects
                .bind_workspace(&fixture.alice_project, Some(&fixture.c_root))?;
        }
        "revoke_membership" => {
            fixture
                .projects
                .remove_member(&fixture.alice_project, "alice")?;
        }
        "archive_a" => {
            fixture.projects.archive(&fixture.alice_project)?;
        }
        "revoke_device_a" => {
            let actual = resolve_device(fixture, "device-a")?;
            anyhow::ensure!(fixture.tokens.revoke_panel_device(&actual)?, "not_revoked");
        }
        // Selection belongs to the authenticated Panel, never a fixture-control no-op.
        _ => anyhow::bail!("unknown_action"),
    }
    Ok(())
}

async fn write_json<W, T>(output: &mut W, value: &T) -> anyhow::Result<()>
where
    W: AsyncWriteExt + Unpin,
    T: Serialize,
{
    output
        .write_all(serde_json::to_string(value)?.as_bytes())
        .await?;
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
        values.insert(
            key,
            iter.next()
                .ok_or_else(|| anyhow::anyhow!("missing_value"))?,
        );
    }
    Ok(Args {
        owned_root: PathBuf::from(
            values
                .remove("--owned-root")
                .ok_or_else(|| anyhow::anyhow!("missing_root"))?,
        ),
        nonce: values
            .remove("--nonce")
            .ok_or_else(|| anyhow::anyhow!("missing_nonce"))?
            .to_string_lossy()
            .to_string(),
        panel_root: PathBuf::from(
            values
                .remove("--panel-root")
                .ok_or_else(|| anyhow::anyhow!("missing_panel"))?,
        ),
    })
}

fn validate_nonce(nonce: &str) -> anyhow::Result<()> {
    if !(32..=128).contains(&nonce.len())
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
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
