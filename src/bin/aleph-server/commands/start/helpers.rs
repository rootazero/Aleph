//! Subsystem initializer helpers extracted from `start::start_server`.
//!
//! Each function handles one cohesive initialization concern. Visibility is
//! `pub(super)` so only the orchestrator in `start::mod` can call them.

use std::net::SocketAddr;
use std::sync::Arc;

use crate::cli::Args;
use crate::daemon::{expand_path, remove_pid_file};

use alephcore::gateway::session_store::SessionStore;
use alephcore::gateway::{GatewayConfig as FullGatewayConfig, SessionManager};

/// Compile the operator's `[[security.mask_patterns]]` into the process-wide
/// redaction set, before any run can produce output.
///
/// Sits next to `PiiEngine::init` because it is the same shape: config reaching
/// a process global exactly once, so that the seven `SecretMasker::new()` sites
/// downstream inherit it without any of them being aware.
///
/// A pattern that fails to compile is logged at `error`, not dropped quietly —
/// the symptom of a silently-rejected redaction pattern is a secret printed in
/// the clear, which looks identical to "the operator never configured one".
pub(super) fn install_mask_patterns(config: &alephcore::Config) {
    let security = &config.security;
    if security.mask_patterns.is_empty() {
        return;
    }
    let (installed, rejected) = alephcore::exec::masker::install_operator_patterns(
        security
            .mask_patterns
            .iter()
            .map(|p| (p.pattern.as_str(), p.replacement.as_str())),
    );
    for (pattern, err) in &rejected {
        tracing::error!(
            %pattern,
            error = %err,
            "security.mask_patterns: invalid regex — this credential shape will NOT be redacted"
        );
    }
    tracing::info!(
        installed,
        rejected = rejected.len(),
        "installed operator secret-mask patterns"
    );
}

/// Validate that the resolved bind address (config host/port after CLI
/// overrides) is available, or return an error if not. Callers pass the same
/// `final_bind`/`final_port` the server will bind, so the probe matches reality.
pub(super) async fn validate_bind_address(
    bind: &str,
    port: u16,
    force: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let ip: std::net::IpAddr = bind
        .parse()
        .map_err(|e| format!("Invalid bind address '{bind}': {e}"))?;
    let addr = SocketAddr::new(ip, port);
    if !force {
        if let Err(e) = tokio::net::TcpListener::bind(addr).await {
            return Err(format!(
                "Error: Cannot bind to {addr}: {e}\nHint: Use --force to attempt to start anyway, or choose a different port with --port"
            )
            .into());
        }
    }
    Ok(())
}

/// Format a bind address + port into a bracketed string safe for IPv6.
pub(super) fn format_socket_addr(bind: &str, port: u16) -> String {
    match bind.parse::<std::net::IpAddr>() {
        Ok(ip) => std::net::SocketAddr::new(ip, port).to_string(),
        Err(_) => format!("{bind}:{port}"),
    }
}

/// Distinctive, grep-friendly token that begins every boot marker line.
const BOOT_MARKER_TAG: &str = "ALEPH-BOOT";

/// Format the single-line boot marker. Pure so the grep contract is testable.
fn boot_marker_line(ts: &str, pid: u32, version: &str) -> String {
    format!("{BOOT_MARKER_TAG} ts={ts} pid={pid} version={version}")
}

/// Emit one grep-friendly, timestamped boot marker to the raw stdout stream.
///
/// This is a plain `println!`, not a tracing event, on purpose: the redirected
/// `server.log` (a `--log-file` daemon redirect or a foreground shell `>`) only
/// captures raw stdout/stderr. The tracing console layer is dropped when stdout
/// is not a TTY (see `initialize_tracing`), and the tracing *file* layer writes
/// to the separate, already-rotated `~/.aleph/logs/` stream — so neither leaves
/// a timestamped boundary in `server.log`. That stream appends across restarts,
/// which makes an old boot's lines easy to mistake for the current boot's. This
/// marker gives every boot an unambiguous start line in every run mode:
///   grep ALEPH-BOOT ~/.aleph/server.log | tail -1
/// is the current boot; everything after it is this run.
pub(super) fn print_boot_marker() {
    println!(
        "{}",
        boot_marker_line(
            &chrono::Utc::now().to_rfc3339(),
            std::process::id(),
            env!("ALEPH_VERSION"),
        )
    );
}

/// Print the startup banner and available method list to stdout.
pub(super) fn print_startup_banner(addr: SocketAddr, full_config: &FullGatewayConfig) {
    println!(
        "PII filtering engine initialized (enabled: {})",
        full_config.privacy.pii_filtering
    );
    println!("╔═══════════════════════════════════════════════╗");
    println!(
        "║         Aleph Gateway v{}           ║",
        env!("ALEPH_VERSION")
    );
    println!("╠═══════════════════════════════════════════════╣");
    let ws_scheme = if full_config.gateway.tls.enabled {
        "wss"
    } else {
        "ws"
    };
    println!("║  WebSocket: {ws_scheme}://{addr}          ║");
    println!("║  Protocol:  JSON-RPC 2.0                      ║");
    println!("╚═══════════════════════════════════════════════╝");
    println!();
    println!("Available methods:");
    println!("  - health    : Check server health status");
    println!("  - echo      : Echo back parameters (testing)");
    println!("  - version   : Get server version info");
    println!("  - agent.run : Execute agent request with streaming");
    println!();
    println!(
        "Agents: {:?}",
        full_config.agents.keys().collect::<Vec<_>>()
    );
    println!();
}

/// Initialize the tracing subscriber with file + console logging.
///
/// Uses `aleph_logging::init_component_logging` which provides:
/// - Console output with PII scrubbing
/// - File output to `~/.aleph/logs/aleph-server.log.YYYY-MM-DD`
/// - Daily rotation and 7-day retention
pub(super) fn initialize_tracing(args: &Args) {
    let filter = format!("aleph_server={0},alephcore={0}", args.log_level);
    if let Err(e) = aleph_logging::init_component_logging("server", 7, &filter) {
        eprintln!("Warning: Failed to initialize file logging: {e}. Falling back to console only.");
        use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(true)
                    .with_thread_ids(false)
                    .with_file(false)
                    .with_line_number(false),
            )
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&filter)),
            )
            .init();
    }
}

/// Load gateway configuration, apply CLI overrides, and return resolved values.
/// Returns (`full_config`, `final_bind`, `final_port`, `final_max_connections`).
pub(super) fn load_gateway_config(
    args: &Args,
) -> Result<(FullGatewayConfig, String, u16, usize), Box<dyn std::error::Error>> {
    let full_config = match &args.config {
        Some(config_path) => {
            let path = expand_path(&config_path.to_string_lossy());
            match FullGatewayConfig::load(&path) {
                Ok(cfg) => {
                    if !args.daemon {
                        println!("Loaded config from: {}", path.display());
                    }
                    cfg
                }
                Err(e) => {
                    return Err(
                        format!("Error loading config from {}: {}", path.display(), e).into(),
                    );
                }
            }
        }
        None => FullGatewayConfig::load_default()
            .map_err(|e| format!("Error loading default config: {e}"))?,
    };

    // CLI args override config file settings only when explicitly provided.
    let final_bind = args
        .bind
        .clone()
        .unwrap_or_else(|| full_config.gateway.host.clone());
    let final_port = args.port.unwrap_or(full_config.gateway.port);
    let final_max_connections = args
        .max_connections
        .unwrap_or(full_config.gateway.max_connections);

    Ok((full_config, final_bind, final_port, final_max_connections))
}

/// Initialize the session store backend based on configuration.
/// Selects by string: `"file"` enables the JSON/JSONL file backend and
/// `"sqlite"` selects `SQLite`; unknown values are rejected. The SHIPPED default
/// is `"file"` (`config::types::general::default_session_store_backend`), so the
/// file branch is the one a stock install takes.
/// Returns the trait object and an optional owned `SessionManager` for `SQLite`
/// so the caller can attach raw-memory writer and event bus before wrapping.
pub(super) async fn initialize_session_store(
    daemon: bool,
    backend: &str,
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    raw_memory_writer: Option<Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>>,
) -> Result<(Arc<dyn SessionStore>, Option<SessionManager>), Box<dyn std::error::Error>> {
    if backend == "file" {
        let config =
            alephcore::gateway::session_store::file_backend::FileSessionStoreConfig::default();
        match alephcore::gateway::session_store::file_backend::FileSessionStore::new(config) {
            Ok(mut store) => {
                store = store.with_event_bus(event_bus.clone());
                // Crash-window sweep: a delete_session archived on a previous
                // run may have crashed between the rename and the event
                // emission, leaving Panels showing a deleted conversation
                // until their next full refresh. Re-emit recent archives'
                // delete events now that the bus exists. Best-effort; never
                // blocks startup.
                store.sweep_archive_events().await;
                // Spec 1 G3-A: wire the session-end emit for the file
                // backend too (the SQLite path does this via
                // `with_raw_memory_writer` in start/mod.rs).
                if let Some(writer) = raw_memory_writer {
                    store = store.with_raw_memory_writer(writer);
                }
                // Heal session dirs whose on-disk names don't match this
                // platform's session_dir(key) form — e.g. a `~/.aleph` copied
                // from macOS (':' separators) onto Windows (':'→'_'), where the
                // transferred dir names no longer resolve so history lookups
                // miss. Idempotent; a no-op when names already match. Runs
                // before the legacy SQLite import (which always writes canonical
                // names). Best-effort: never blocks startup.
                let normalized =
                    alephcore::gateway::session_store::migration::normalize_session_dir_names(
                        store.config().base_dir.as_path(),
                    )
                    .await;
                if normalized > 0 && !daemon {
                    println!("Normalized {normalized} migrated session directory name(s)");
                }
                // Repair damage persisted by already-fixed writer bugs: legacy
                // millisecond `last_active_at` values (they sort above every
                // seconds-stamped session, pinning new conversations below the
                // fold) and torn `metadata.json` documents (skipped by every
                // listing, so the conversation vanishes). Idempotent,
                // best-effort, never blocks startup.
                let repaired =
                    alephcore::gateway::session_store::migration::repair_session_metadata(
                        store.config().base_dir.as_path(),
                    )
                    .await;
                if repaired.did_work() && !daemon {
                    println!(
                        "Repaired session metadata: {} timestamp(s) normalized, {} rebuilt from transcript, {} quarantined",
                        repaired.timestamps_normalized,
                        repaired.rebuilt_from_transcript,
                        repaired.quarantined
                    );
                }
                if alephcore::gateway::session_store::migration::migration_needed(
                    &store.config().base_dir,
                ) {
                    if !daemon {
                        println!("Migrating legacy SQLite sessions to file backend ...");
                    }
                    if let Err(e) =
                        alephcore::gateway::session_store::migration::export_legacy_messages(&store)
                            .await
                    {
                        eprintln!("Warning: Session migration failed: {e}");
                    }
                }
                if !daemon {
                    println!("Session store initialized (file backend)");
                }
                Ok((Arc::new(store), None))
            }
            Err(e) => Err(format!("Error: Failed to initialize file session store: {e}").into()),
        }
    } else if backend == "sqlite" {
        let sm = match SessionManager::with_defaults() {
            Ok(sm) => sm,
            Err(e) => {
                return Err(format!(
                    "Error: Failed to initialize SQLite session store: {e}. Sessions are required."
                )
                .into());
            }
        };
        if !daemon {
            println!("Session store initialized (SQLite backend)");
        }
        let dyn_store: Arc<dyn SessionStore> = Arc::new(sm.clone());
        Ok((dyn_store, Some(sm)))
    } else {
        Err(format!(
            "Unknown session store backend '{backend}'; expected 'file' or 'sqlite'."
        )
        .into())
    }
}

/// Build a `SessionService` backed by the same `SQLite` file that the
/// `SessionManager` uses.
///
/// Opens a **dedicated** connection to the sessions DB, runs the
/// `session_events` migration, and returns an `InProcessActorSessionService`
/// around a `SqliteEventStore`. Uses a separate connection (not the one owned
/// by `SessionManager`) because `SessionManager` and `SqliteEventStore` use
/// different `Mutex` types.
///
/// Returns `None` on any failure — then no event log exists for this run:
/// `decline_global_session_service` / `decline_global_session_event_store`
/// say so on the capability roster, every reader of the two handles takes its
/// own `None` arm (`SESSION_SERVICE_READERS` / `SESSION_EVENT_STORE_READERS`
/// name each one), and the `messages` projection has nothing to project from.
/// Nothing writes the transcript in the log's place.
pub(super) fn build_sqlite_session_service(
    db_path: &std::path::Path,
    observer: Option<Arc<dyn alephcore::session::observer::SessionEventObserver>>,
    raw_store: Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore>,
) -> Option<(
    Arc<dyn alephcore::session::service::SessionService>,
    Arc<dyn alephcore::session::store::SessionEventStore>,
)> {
    let conn = match alephcore::utils::sqlite_open::open_sqlite_safe(db_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                path = ?db_path,
                error = %e,
                "failed to open the session_events connection; no event log this run"
            );
            return None;
        }
    };
    if let Err(e) = alephcore::session::store::migrate_add_session_events(&conn) {
        tracing::warn!(
            error = %e,
            "session_events migration failed; no event log this run"
        );
        return None;
    }
    let store: Arc<dyn alephcore::session::store::SessionEventStore> =
        Arc::new(alephcore::session::store::SqliteEventStore::new(conn));
    let svc = alephcore::session::in_process::InProcessActorSessionService::new(store.clone())
        .with_raw_memory_writer(raw_store);
    let svc = match observer {
        Some(o) => svc.with_observer(o),
        None => svc,
    };
    let service: Arc<dyn alephcore::session::service::SessionService> = Arc::new(svc);
    Some((service, store))
}

/// Initialize the `ExtensionManager` for the plugin system.
pub(super) async fn initialize_extension_manager(
    daemon: bool,
    ownership_tree: Arc<alephcore::capability::ownership::OwnershipTree>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Migrate old single-dir layout and update official skills. Resolve via the
    // authoritative resolver (ALEPH_HOME / $HOME) so bundled content is
    // extracted into the SAME ~/.aleph as the rest of the daemon's state.
    let aleph_home = alephcore::utils::paths::get_config_dir()?;
    // Bundled extraction may perform a one-time network clone on first run;
    // run it off the async executor so a slow clone never stalls startup.
    let home_for_extract = aleph_home.clone();
    let _ = tokio::task::spawn_blocking(move || {
        alephcore::bundled::extract_bundled_content(&home_for_extract)
    })
    .await;

    match alephcore::extension::ExtensionManager::with_defaults_with_ownership_tree(ownership_tree)
        .await
    {
        Ok(extension_manager) => {
            // SkillSystem is now always initialized; load_all() will init it
            // with discovered skill directories automatically.

            let manager = Arc::new(extension_manager);
            if let Err(_existing) = alephcore::gateway::init_extension_manager(manager) {
                if !daemon {
                    println!("Extension manager already initialized");
                }
            } else if !daemon {
                println!("Extension manager initialized");
            }
        }
        Err(e) => {
            // Warn-and-continue is a decline that never got stamped: without
            // this the plugin/skill/hook surface is simply absent, and every
            // reader of the manager sees the same `None` a boot that died
            // earlier would leave.
            alephcore::gateway::decline_extension_manager(
                "`ExtensionManager::with_defaults_with_ownership_tree()` failed, so no plugin, skill \
                 or hook is loaded this boot — the accompanying \"Failed to \
                 initialize extension manager\" message names the cause. Check \
                 that the extension tree under `$ALEPH_HOME` (else `~/.aleph`) \
                 is readable.",
            );
            // NOT gated on `!daemon`: `tracing` routes to the log file in
            // daemon mode, which is the production path, and the decline above
            // promises the operator a message that names the cause. The
            // `eprintln!` below cannot be that message — it is the interactive
            // nicety, and in daemon mode it never runs, taking `{e}` with it.
            // Same reasoning, written out, at `start/mod.rs`'s orchestrator arm.
            tracing::warn!(
                error = %e,
                "Failed to initialize extension manager; plugins, skills and hooks unavailable"
            );
            if !daemon {
                eprintln!("Warning: Failed to initialize extension manager: {e}. Plugin tools will be unavailable.");
            }
        }
    }
    Ok(())
}

/// Preserve the existing browser-specific stop budget. Memory cleanup is
/// awaited separately and has no process-wide deadline that can skip joins.
pub(super) const ORDERLY_BROWSER_STOP_BUDGET: std::time::Duration =
    std::time::Duration::from_millis(2500);

/// Spawn Ctrl-C and SIGTERM handlers; return the oneshot receiver for `run_until_shutdown`.
///
/// Both signals share one graceful path: forensics → remove PID file → signal
/// `run_until_shutdown` via the oneshot, so the post-run teardown in
/// `start_server` (memory/health monitor shutdown, endpoint cleanup,
/// GatewayStop hooks, plugin stop) runs for supervised SIGTERM stops too, not
/// just interactive Ctrl-C. The owner awaits cleanup before the process returns.
pub(super) fn setup_graceful_shutdown(args: &Args) -> tokio::sync::oneshot::Receiver<()> {
    use alephcore::gateway::shutdown_forensics::snapshot_shutdown_context;

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let pid_file = args.pid_file.clone();
    let daemon_mode = args.daemon;
    tokio::spawn(async move {
        let ctrl_c = async {
            if let Err(e) = tokio::signal::ctrl_c().await {
                // Must NOT fall through to the shutdown path on registration
                // failure: sending on — or even dropping — `shutdown_tx` both
                // trigger graceful shutdown (`run_until_shutdown` treats a
                // closed channel as a signal). Park forever to keep the sender
                // alive; the server just loses Ctrl-C handling.
                tracing::warn!("Failed to listen for Ctrl-C: {e}; Ctrl-C shutdown disabled");
                std::future::pending::<()>().await;
            }
        };
        #[cfg(unix)]
        let sigterm = async {
            use tokio::signal::unix::{signal, SignalKind};
            match signal(SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(e) => {
                    tracing::warn!("Failed to listen for SIGTERM: {e}; SIGTERM shutdown disabled");
                    std::future::pending::<()>().await;
                }
            }
        };
        #[cfg(not(unix))]
        let sigterm = std::future::pending::<()>();

        tokio::select! {
            () = ctrl_c => {
                // Forensics: capture context before tearing anything down. Skip
                // `with_parent_command` here — the Ctrl-C path is the interactive
                // case where the operator already knows who sent it.
                let ctx = snapshot_shutdown_context("ctrl_c", None, false);
                tracing::warn!("{}", ctx.as_log_line());
                if !daemon_mode {
                    println!("\nShutting down gateway...");
                }
            }
            () = sigterm => {
                // Forensics: include parent command — SIGTERM usually comes
                // from launchd / systemd / supervisor scripts and the parent
                // PID is the diagnostic clue we want.
                let ctx = snapshot_shutdown_context("SIGTERM", Some(15), true);
                tracing::warn!("{}", ctx.as_log_line());
            }
        }

        remove_pid_file(&pid_file);
        if let Err(e) = shutdown_tx.send(()) {
            tracing::warn!("Failed to send shutdown signal: {:?}", e);
        }
    });

    shutdown_rx
}

/// Build an `HttpProvider` from a provider name and config.
///
/// Mirrors the logic of `providers::create_provider` but returns the concrete
/// `HttpProvider` type (needed for `stream_raw()` in the passthrough path).
pub(super) fn build_http_provider(
    name: &str,
    config: &alephcore::ProviderConfig,
) -> Result<alephcore::providers::http_provider::HttpProvider, Box<dyn std::error::Error>> {
    use alephcore::providers::http_provider::HttpProvider;
    use alephcore::providers::presets;

    let mut cfg = config.clone();
    let name_lower = name.to_lowercase();

    // Apply preset
    if let Some(preset) = presets::get_preset(&name_lower) {
        if cfg.base_url.is_none()
            || cfg
                .base_url
                .as_ref()
                .is_some_and(std::string::String::is_empty)
        {
            cfg.base_url = Some(preset.base_url.to_string());
        }
        if cfg.protocol.is_none() {
            cfg.protocol = Some(preset.protocol.to_string());
        }
    }

    let protocol_name = cfg.protocol();
    let registry = alephcore::providers::protocols::ProtocolRegistry::global();
    if registry.list_protocols().is_empty() {
        registry.register_builtin();
    }
    let adapter = registry
        .get(&protocol_name)
        .ok_or_else(|| format!("Unknown protocol: '{protocol_name}'"))?;

    HttpProvider::new(name.to_string(), cfg, adapter).map_err(std::convert::Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_marker_is_greppable_and_carries_fields() {
        let line = boot_marker_line("2026-07-24T00:00:00+00:00", 4242, "26.7.24");
        // The grep contract: a stable leading tag so `grep ALEPH-BOOT | tail -1`
        // always lands on a boot boundary, plus the three fields ops need.
        assert!(line.starts_with(BOOT_MARKER_TAG));
        assert!(line.contains("ts=2026-07-24T00:00:00+00:00"));
        assert!(line.contains("pid=4242"));
        assert!(line.contains("version=26.7.24"));
    }

    /// The single orderly funnel retains every existing child reaper.
    #[test]
    fn orderly_teardown_runs_every_reaper() {
        let src = include_str!("mod.rs").replace('\r', "");
        let prefix = alephcore::utils::source_scan::production_prefix(&src);
        assert!(
            prefix.len() < src.len(),
            "production boundary must exclude tests"
        );
        let production = alephcore::utils::source_scan::code_text(&prefix);
        for reaper in ["kill_all_running_background", "shutdown_browsers_global"] {
            assert!(
                production.contains(&format!("{reaper}(")),
                "the common teardown must still call {reaper}()"
            );
        }
        assert!(production.contains("startup_resources.install_pty(manager)"));
        assert!(production.contains("startup_resources.finish(result).await"));
        let cleanup_at = production
            .find("startup_resources.cleanup().await")
            .expect("the orderly path must await owned cleanup");
        let scope_at = production
            .find("builtin_registration_scope.take()")
            .expect("the registry scope must remain owned until cleanup");
        assert!(cleanup_at < scope_at);

        let owner_src = include_str!("builder/startup_resources.rs").replace('\r', "");
        let owner_prefix = alephcore::utils::source_scan::production_prefix(&owner_src);
        assert!(owner_prefix.len() < owner_src.len());
        let owner = alephcore::utils::source_scan::code_text(&owner_prefix);
        assert!(owner.contains("self.pty = Some(manager)"));
        assert!(owner.contains("self.cleanup().await"));
        assert!(owner.contains("if let Some(manager) = self.pty"));
        let stop_at = owner
            .find("manager.deactivate().await")
            .expect("the installed PTY manager must be stopped and joined");
        for teardown in [
            "events.close()",
            "bridge.clone().join().await",
            "host.clone().close_and_await().await",
        ] {
            let teardown_at = owner.find(teardown).expect("owned teardown must remain");
            assert!(stop_at < teardown_at, "PTY stop must precede {teardown}");
        }

        let pty_src = include_str!("../../../../gateway/pty/manager.rs").replace('\r', "");
        let pty_prefix = alephcore::utils::source_scan::production_prefix(&pty_src);
        assert!(pty_prefix.len() < pty_src.len());
        let pty = alephcore::utils::source_scan::code_text(&pty_prefix);
        for stop_edge in [
            "let cleanup = self.close_all_checked()",
            "let generation = self.begin_stop()?",
            "poll_fn(|cx| self.poll_stop(generation, cx)).await",
            "Pin::new(&mut owned.handle).poll(cx)",
        ] {
            assert!(pty.contains(stop_edge), "PTY stop must retain {stop_edge}");
        }
    }
}
