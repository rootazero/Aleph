use super::{
    channel_handlers, discord_panel_handlers, session_handlers, Arc, ChannelRegistry,
    GatewayServer, MemoryBackend,
};

// ─── register_session_handlers ───────────────────────────────────────────────

pub(in crate::commands::start) fn register_session_handlers(
    server: &mut GatewayServer,
    session_store: &Arc<dyn alephcore::gateway::session_store::SessionStore>,
    memory_db: &MemoryBackend,
    daemon: bool,
) {
    register_handler!(
        server,
        "sessions.list",
        session_handlers::handle_list_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.history",
        session_handlers::handle_history_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.reset",
        session_handlers::handle_reset_db,
        session_store
    );
    // sessions.delete is wired with the raw_memory writer so the SessionEnd
    // capture path fires before the transcript is dropped (G4 fix).
    {
        let store = Arc::clone(session_store);
        let writer: Arc<dyn alephcore::memory::store::raw_memory::RawMemoryStore> =
            Arc::clone(memory_db) as _;
        server.handlers_mut().register("sessions.delete", move |req| {
            let store = Arc::clone(&store);
            let writer = Arc::clone(&writer);
            async move {
                session_handlers::handle_delete_db_with_capture(req, store, writer).await
            }
        });
    }
    register_handler!(
        server,
        "session.create",
        session_handlers::handle_create_db,
        session_store
    );
    register_handler!(
        server,
        "session.usage",
        session_handlers::handle_usage_db,
        session_store
    );
    // Manual compaction still edits the session *event log* (what the prompt
    // is rebuilt from) through process-wide handles, not the `messages` read
    // projection — but the RPC now also needs the store for the P1
    // visibility gate (the response discloses the addressed session's real
    // conversation summary), so it's wired through `register_handler!` like
    // its siblings instead of the old store-free plain registration.
    register_handler!(
        server,
        "session.compact",
        session_handlers::handle_compact_db,
        session_store
    );
    register_handler!(
        server,
        "session.truncate",
        session_handlers::handle_truncate_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.new",
        session_handlers::handle_new_session_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.set_topic",
        session_handlers::handle_set_topic_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.set_project_root",
        session_handlers::handle_set_project_root_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.patch",
        session_handlers::handle_patch_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.preview",
        session_handlers::handle_preview_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.compaction.list",
        session_handlers::handle_list_checkpoints_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.compaction.restore",
        session_handlers::handle_restore_checkpoint_db,
        session_store
    );
    register_handler!(
        server,
        "sessions.compaction.branch",
        session_handlers::handle_branch_checkpoint_db,
        session_store
    );

    if !daemon {
        println!("Session methods:");
        println!("  - sessions.list      : List all sessions");
        println!("  - sessions.history   : Get session message history");
        println!("  - sessions.reset     : Clear session messages");
        println!("  - sessions.delete    : Delete a session");
        println!("  - sessions.new       : Close current session and start new one");
        println!("  - sessions.set_topic : Set session topic/title");
        println!("  - sessions.patch     : Patch session metadata");
        println!("  - sessions.preview   : Preview session with recent messages");
        println!("  - session.create     : Create a new session");
        println!("  - session.usage      : Get session token/message stats");
        println!("  - session.compact    : Compact session history");
        println!("  - session.truncate   : Truncate session history to first N messages");
        println!("  - sessions.compaction.list    : List compaction checkpoints");
        println!("  - sessions.compaction.restore : Restore session to checkpoint");
        println!("  - sessions.compaction.branch  : Branch new session from checkpoint");
        println!();
    }
}

// ─── register_artifact_handlers ──────────────────────────────────────────────

/// Wire the artifact metadata RPCs (`artifacts.list`, `session.export_html`).
///
/// The store is built here from its default root (`<data_dir>/artifacts`) —
/// an `ArtifactStore` owns nothing but that path, so producers that build their
/// own handle from the same root see the same artifacts.
///
/// If the data directory cannot be resolved, both methods stay unregistered
/// rather than being wired to a store that would fail every call: a
/// `method_not_found` is a diagnosable boot problem, a handler that always
/// errors is not.
pub(in crate::commands::start) fn register_artifact_handlers(
    server: &mut GatewayServer,
    session_store: &Arc<dyn alephcore::gateway::session_store::SessionStore>,
    daemon: bool,
) {
    use alephcore::artifacts::ArtifactStore;
    use alephcore::gateway::handlers::artifacts as artifact_handlers;

    let root = match ArtifactStore::default_root() {
        Ok(root) => root,
        Err(e) => {
            tracing::warn!(
                "Artifact store unavailable ({e}); artifacts.list and session.export_html are not registered"
            );
            return;
        }
    };
    let store = Arc::new(ArtifactStore::new(root));

    register_handler!(
        server,
        "artifacts.list",
        artifact_handlers::handle_list,
        store,
        session_store
    );
    register_handler!(
        server,
        "artifacts.read_text",
        artifact_handlers::handle_read_text,
        store,
        session_store
    );
    register_handler!(
        server,
        "session.export_html",
        artifact_handlers::handle_export_html,
        store,
        session_store
    );

    if !daemon {
        println!("Artifact methods:");
        println!("  - artifacts.list      : List a session's stored artifacts");
        println!("  - artifacts.read_text : Read a text artifact for in-pane preview");
        println!("  - session.export_html : Export a session as a self-contained HTML document");
        println!();
    }
}

// ─── register_channel_handlers ───────────────────────────────────────────────

pub(in crate::commands::start) fn register_channel_handlers(
    server: &mut GatewayServer,
    channel_registry: &Arc<ChannelRegistry>,
    app_config: &Arc<tokio::sync::RwLock<alephcore::Config>>,
    vault: &Arc<alephcore::gateway::security::SharedTokenManager>,
) {
    register_handler!(
        server,
        "channels.list",
        channel_handlers::handle_list,
        channel_registry,
        app_config
    );
    register_handler!(
        server,
        "channels.status",
        channel_handlers::handle_status,
        channel_registry
    );
    register_handler!(
        server,
        "channel.start",
        channel_handlers::handle_start,
        channel_registry,
        app_config,
        vault
    );
    register_handler!(
        server,
        "channel.stop",
        channel_handlers::handle_stop,
        channel_registry
    );
    register_handler!(
        server,
        "channel.pairing_data",
        channel_handlers::handle_pairing_data,
        channel_registry
    );
    register_handler!(
        server,
        "channel.send",
        channel_handlers::handle_send,
        channel_registry
    );
    register_handler!(
        server,
        "channel.create",
        channel_handlers::handle_create,
        channel_registry,
        app_config,
        vault
    );
    register_handler!(
        server,
        "channel.delete",
        channel_handlers::handle_delete,
        channel_registry,
        app_config
    );

    // ---- Discord Control Plane panel handlers ----
    register_handler!(
        server,
        "discord.validate_token",
        discord_panel_handlers::handle_validate_token
    );
    register_handler!(
        server,
        "discord.list_guilds",
        discord_panel_handlers::handle_list_guilds,
        channel_registry
    );
    register_handler!(
        server,
        "discord.list_channels",
        discord_panel_handlers::handle_list_channels,
        channel_registry
    );
    register_handler!(
        server,
        "discord.audit_permissions",
        discord_panel_handlers::handle_audit_permissions,
        channel_registry
    );
}

// ─── setup_config_watcher ────────────────────────────────────────────────────
