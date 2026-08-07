//! Shared test fixtures for `agent_manage` unit tests.
//!
//! Every test module previously re-implemented `test_workspace_mgr`,
//! `test_session_store`, and `test_instance` with byte-identical bodies —
//! when one fixture gained a field (e.g. `archive_after_days`), only one
//! module saw the change and the rest drifted into "passes locally, fails
//! in CI" territory. This module owns the single fixture set; each test
//! module pulls what it needs.
//!
//! Marked `#[cfg(test)]` so the production binary doesn't carry the
//! tempfile / SQLite dance.

use crate::gateway::agent_env::{AgentEnvStore, AgentEnvStoreConfig};
use crate::gateway::agent_instance::{AgentInstance, AgentInstanceConfig};
use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
use crate::gateway::session_store::SessionStore;
use crate::sync_primitives::Arc;
use tempfile::TempDir;

/// Build a fresh `AgentEnvStore` backed by a throwaway SQLite file.
///
/// Returned store is `Arc`-wrapped (matches production injection sites);
/// the underlying `TempDir` is held by the caller to keep the file alive
/// for the duration of the test.
#[must_use]
pub fn workspace_mgr() -> (Arc<AgentEnvStore>, TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = AgentEnvStoreConfig {
        db_path: temp.path().join("test.db"),
        default_profile: "default".to_string(),
        archive_after_days: 0,
    };
    let store = Arc::new(AgentEnvStore::new(config).expect("agent env store"));
    (store, temp)
}

/// Build a fresh session store backed by a throwaway SQLite file.
#[must_use]
pub fn session_store() -> (Arc<dyn SessionStore>, TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let cfg = SessionManagerConfig {
        db_path: temp.path().join("sessions.db"),
        ..Default::default()
    };
    let sm = Arc::new(SessionManager::new(cfg).expect("session manager"));
    (sm, temp)
}

/// Build a fully-instantiated `AgentInstance` rooted in a throwaway
/// directory pair (state + workspace).
#[must_use]
pub fn instance(agent_id: &str) -> (AgentInstance, Arc<dyn SessionStore>, TempDir) {
    let temp = tempfile::tempdir().expect("tempdir");
    let (sm, _sm_temp) = session_store();
    let config = AgentInstanceConfig {
        agent_id: agent_id.to_string(),
        workspace: temp.path().join("workspace"),
        agent_dir: temp.path().join("state"),
        model: "claude-sonnet-4-5".to_string(),
        ..Default::default()
    };
    let instance = AgentInstance::new(config, sm.clone()).expect("instance");
    (instance, sm, temp)
}

/// Build a fully-instantiated `AgentInstance` with both backing tempdirs
/// owned by the caller (returned as a tuple alongside the instance).
///
/// Two-tempdir flavor because some tests want to drop the session store
/// before the workspace (e.g. shutdown ordering tests).
#[allow(dead_code)]
#[must_use]
pub fn instance_with_sm(
    agent_id: &str,
) -> (AgentInstance, Arc<dyn SessionStore>, TempDir, TempDir) {
    let (instance, sm, instance_temp) = instance(agent_id);
    let (_sm, sm_temp) = session_store();
    (instance, sm, instance_temp, sm_temp)
}
