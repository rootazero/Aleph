//! `SQLite` store initialization for `coord_task` + snapshot + team + teams-evolution.
//!
//! Extracted from `agent_init.rs` to keep the boot-time orchestrator smaller.
//! Each helper opens its own connection (via `open_sqlite_safe`), runs
//! migrations, and returns `Option<Arc<…>>`. Failure is logged but never
//! panics — the dependent tools are simply disabled.

use alephcore::sync_primitives::Arc;

/// Open `coord.db`, create the `SqliteCoordTaskStore`, run migrations, and
/// derive the sibling `SqliteSnapshotStore` from the same connection
/// (avoids the `SQLite` "database is locked" hazard that two independent
/// connections would hit).
#[allow(clippy::type_complexity)]
pub(super) async fn init_coord_and_snapshot(
    event_bus: Arc<alephcore::gateway::event_bus::GatewayEventBus>,
    daemon: bool,
) -> (
    Option<Arc<dyn alephcore::agents::swarm::tasks::CoordTaskStore>>,
    Option<Arc<alephcore::teams::SqliteSnapshotStore>>,
) {
    use alephcore::agents::swarm::tasks::store::SqliteCoordTaskStore;
    use alephcore::teams::SqliteSnapshotStore;
    use alephcore::utils::paths::get_data_dir;

    let dir = match get_data_dir() {
        Ok(d) => d,
        Err(e) => {
            if !daemon {
                eprintln!(
                    "Warning: Failed to resolve data directory: {e}. Task coordination tools disabled."
                );
            }
            tracing::warn!(error = %e, "Failed to resolve data directory; task coordination tools disabled");
            return (None, None);
        }
    };

    let db_path = dir.join("coord.db");
    let conn = match alephcore::utils::sqlite_open::open_sqlite_safe(&db_path) {
        Ok(c) => c,
        Err(e) => {
            if !daemon {
                eprintln!(
                    "Warning: Failed to open coord.db: {e}. Task coordination tools disabled."
                );
            }
            return (None, None);
        }
    };

    let concrete = Arc::new(SqliteCoordTaskStore::new(conn).with_event_bus(event_bus));
    match concrete.migrate().await {
        Ok(()) => {
            if !daemon {
                println!("  Coord task store initialized (SQLite)");
            }
            let snap = Arc::new(SqliteSnapshotStore::new_from_shared(
                concrete.connection_handle(),
            ));
            let trait_obj =
                Arc::clone(&concrete) as Arc<dyn alephcore::agents::swarm::tasks::CoordTaskStore>;
            (Some(trait_obj), Some(snap))
        }
        Err(e) => {
            if !daemon {
                eprintln!(
                    "Warning: Coord task store migration failed: {e}. Task coordination tools disabled."
                );
            }
            (None, None)
        }
    }
}

/// Open `teams.db`, create the `SqliteTeamStore`, run migrations.
pub(super) async fn init_team_store(daemon: bool) -> Option<Arc<dyn alephcore::teams::TeamStore>> {
    use alephcore::teams::SqliteTeamStore;
    use alephcore::utils::paths::get_data_dir;

    let dir = match get_data_dir() {
        Ok(d) => d,
        Err(e) => {
            if !daemon {
                eprintln!(
                    "Warning: Failed to resolve data directory: {e}. Team management tools disabled."
                );
            }
            tracing::warn!(error = %e, "Failed to resolve data directory; team management tools disabled");
            return None;
        }
    };

    let db_path = dir.join("teams.db");
    let conn = match alephcore::utils::sqlite_open::open_sqlite_safe(&db_path) {
        Ok(c) => c,
        Err(e) => {
            if !daemon {
                eprintln!("Warning: Failed to open teams.db: {e}. Team management tools disabled.");
            }
            return None;
        }
    };
    let store = Arc::new(SqliteTeamStore::new(conn));
    match store.migrate().await {
        Ok(()) => {
            if !daemon {
                println!("  Team store initialized (SQLite)");
            }
            // Every consumer — the 37 `teams.*` RPCs, the `team_*` tools, the
            // dispatcher — receives the OWNERSHIP-SCOPED handle. This is the
            // only place the raw store is visible; publishing it unwrapped is
            // how the P1 team isolation gets bypassed (see
            // `teams::scoped::ScopedTeamStore`).
            Some(alephcore::teams::ScopedTeamStore::wrap(store))
        }
        Err(e) => {
            if !daemon {
                eprintln!(
                    "Warning: Team store migration failed: {e}. Team management tools disabled."
                );
            }
            None
        }
    }
}

#[allow(clippy::type_complexity)]
type TeamsEvolution = (
    Option<Arc<dyn alephcore::teams::artifacts::ArtifactStore>>,
    Option<Arc<dyn alephcore::teams::events::EventLogStore>>,
    Option<Arc<dyn alephcore::teams::messages::MessageStore>>,
    Option<Arc<dyn alephcore::teams::sessions::SessionStore>>,
);

/// Open the four "teams evolution" stores against the same `teams.db`:
/// artifact, event log, message, session. Each is migrated independently;
/// failures only disable the affected store.
pub(super) async fn init_teams_evolution_stores(daemon: bool) -> TeamsEvolution {
    use alephcore::teams::artifacts::SqliteArtifactStore;
    use alephcore::teams::events::SqliteEventLogStore;
    use alephcore::teams::messages::SqliteMessageStore;
    use alephcore::teams::sessions::SqliteSessionStore;
    use alephcore::utils::paths::get_data_dir;

    let dir = match get_data_dir() {
        Ok(d) => d,
        Err(e) => {
            if !daemon {
                eprintln!(
                    "Warning: Failed to resolve data directory: {e}. Team sub-stores disabled."
                );
            }
            tracing::warn!(error = %e, "Failed to resolve data directory; team sub-stores disabled");
            return (None, None, None, None);
        }
    };

    let db_path = dir.join("teams.db");

    macro_rules! init_store {
        ($store_ty:ident, $trait_ty:ty, $label:expr, $db:expr) => {{
            match alephcore::utils::sqlite_open::open_sqlite_safe(&$db) {
                Ok(conn) => {
                    let store = Arc::new($store_ty::new(conn));
                    match store.migrate().await {
                        Ok(()) => {
                            if !daemon {
                                println!("  {} initialized (SQLite)", $label);
                            }
                            Some(store as Arc<$trait_ty>)
                        }
                        Err(e) => {
                            if !daemon {
                                eprintln!(
                                    "Warning: {} migration failed: {}. Related tools disabled.",
                                    $label, e
                                );
                            }
                            None
                        }
                    }
                }
                Err(e) => {
                    if !daemon {
                        eprintln!(
                            "Warning: Failed to open teams.db for {}: {}. Related tools disabled.",
                            $label, e
                        );
                    }
                    None
                }
            }
        }};
    }

    // ArtifactStore needs a concrete return for downstream `session_coordinator`,
    // so we cannot use the trait-object macro path. Mirror the macro by hand.
    let artifact_store: Option<Arc<SqliteArtifactStore>> =
        match alephcore::utils::sqlite_open::open_sqlite_safe(&db_path) {
            Ok(conn) => {
                let store = Arc::new(SqliteArtifactStore::new(conn));
                match store.migrate().await {
                    Ok(()) => {
                        if !daemon {
                            println!("  Artifact store initialized (SQLite)");
                        }
                        Some(store)
                    }
                    Err(e) => {
                        if !daemon {
                            eprintln!(
                                "Warning: Artifact store migration failed: {e}. Related tools disabled."
                            );
                        }
                        None
                    }
                }
            }
            Err(e) => {
                if !daemon {
                    eprintln!(
                        "Warning: Failed to open teams.db for Artifact store: {e}. Related tools disabled."
                    );
                }
                None
            }
        };

    let event_log = init_store!(
        SqliteEventLogStore,
        dyn alephcore::teams::events::EventLogStore,
        "Event log store",
        db_path
    );
    let message = init_store!(
        SqliteMessageStore,
        dyn alephcore::teams::messages::MessageStore,
        "Message store",
        db_path
    );
    let session = init_store!(
        SqliteSessionStore,
        dyn alephcore::teams::sessions::SessionStore,
        "Session store",
        db_path
    );
    let artifact_trait: Option<Arc<dyn alephcore::teams::artifacts::ArtifactStore>> =
        artifact_store
            .as_ref()
            .map(|s| Arc::clone(s) as Arc<dyn alephcore::teams::artifacts::ArtifactStore>);
    (artifact_trait, event_log, message, session)
}

#[allow(clippy::type_complexity)]
type TeamComponents = (
    Option<Arc<alephcore::teams::messages::MessageRouter>>,
    Option<Arc<alephcore::teams::messages::Inbox>>,
    Option<Arc<alephcore::teams::sessions::SessionCoordinator>>,
);

/// Build higher-level team components (router, inbox, coordinator) from the
/// four lower-level stores. Anything `None` upstream gates the matching
/// higher-level component to `None`.
pub(super) fn build_team_components(
    message_store: &Option<Arc<dyn alephcore::teams::messages::MessageStore>>,
    event_store: &Option<Arc<dyn alephcore::teams::events::EventLogStore>>,
    team_session_store: &Option<Arc<dyn alephcore::teams::sessions::SessionStore>>,
    artifact_store: &Option<Arc<dyn alephcore::teams::artifacts::ArtifactStore>>,
    team_store: &Option<Arc<dyn alephcore::teams::TeamStore>>,
    escalation_rules: alephcore::teams::messages::EscalationRule,
) -> TeamComponents {
    let message_router: Option<Arc<alephcore::teams::messages::MessageRouter>> =
        match (message_store.as_ref(), event_store.as_ref()) {
            (Some(ms), Some(es)) => {
                use alephcore::teams::messages::MessageRouter;
                // The router is a global singleton while leaders are per-team:
                // escalation resolves each message's leader through the team
                // store at send time (a construction-time leader id can never
                // be right here). The escalation thresholds are mapped from the
                // optional [team_messages] TOML at the boot site (parallel to
                // the [team_dispatcher] / [team_broadcast] mappings).
                let mut router = MessageRouter::new(ms.clone(), es.clone(), escalation_rules, None);
                if let Some(ts) = team_store.as_ref() {
                    router = router.with_team_store(ts.clone());
                }
                Some(Arc::new(router))
            }
            _ => None,
        };

    let inbox: Option<Arc<alephcore::teams::messages::Inbox>> =
        match (message_store.as_ref(), event_store.as_ref()) {
            (Some(ms), Some(es)) => {
                use alephcore::teams::messages::Inbox;
                Some(Arc::new(Inbox::new(ms.clone(), es.clone())))
            }
            _ => None,
        };

    let session_coordinator: Option<Arc<alephcore::teams::sessions::SessionCoordinator>> = match (
        team_session_store.as_ref(),
        message_router.as_ref(),
        event_store.as_ref(),
        artifact_store.as_ref(),
    ) {
        (Some(ss), Some(mr), Some(es), Some(a_s)) => {
            use alephcore::teams::sessions::SessionCoordinator;
            Some(Arc::new(SessionCoordinator::new(
                ss.clone(),
                mr.clone(),
                es.clone(),
                a_s.clone(),
            )))
        }
        _ => None,
    };

    (message_router, inbox, session_coordinator)
}
