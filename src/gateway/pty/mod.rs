//! Embedded PTY terminal subsystem.
//!
//! Gives the gateway an interactive pseudo-terminal capability — the Rust
//! mapping of hermes-agent's `pty_bridge.py` embedded-terminal stream. Output
//! is multiplexed over the *existing* loopback `/ws` JSON-RPC transport via the
//! `pty.screen` topic instead of a second WebSocket/ephemeral port, so the
//! desktop shell's single fixed-port discovery + bootstrap-cookie auth model is
//! preserved (R6 one core, many channels).
//!
//! Layers:
//! - [`session`]: a single PTY (`portable-pty` master/child) + reader thread.
//! - [`manager`]: the process-global bounded session registry + event-bus sink.
//!
//! Handlers live in `gateway::handlers::pty` (`pty.spawn/input/resize/close/list/attach`).
//! Operator-only, on both the RPC and event faces — see the module doc on
//! `gateway::handlers::pty` for why both faces matter and how the sentence
//! that used to stand here (claiming the surface was open to all connections)
//! survived one face going admin-only while the other didn't.

pub mod jail;
pub mod manager;
pub mod screen;
pub mod session;

pub use manager::{
    attach_event_bus, manager, owner_admits, PtyManager, SessionInfo, SessionOwner, SpawnResult,
};
pub use session::{PtySession, SpawnOptions};

/// The workspace roots a PTY may be spawned under, read fresh on every
/// spawn — a boot-time snapshot would let a workspace registered after
/// start-up stay unusable until restart.
///
/// The root is `workspace_root_for(&defaults)`, NOT `default_workspace_root()`:
/// the latter answers "where does this live when nothing is configured",
/// which is a different question and is wrong on every install that sets
/// `[agents.defaults] workspace_root`.
///
/// The directory is created if missing. `agent_resolver::resolve_one`
/// already provisions `<root>/<agent_id>` via `create_dir_all` for every
/// configured agent, which creates this same root as a side effect the
/// first time any agent is resolved — this makes that guarantee explicit
/// here too, so a fresh install's default-cwd `pty.spawn` does not depend on
/// some other subsystem having run first. A failed create is not fatal: it
/// just leaves the root unresolvable, which [`jail::resolve_spawn_cwd`]
/// already treats as "not registered" and refuses — never as "allow
/// anywhere".
#[must_use]
pub fn workspace_roots(defaults: &crate::config::types::AgentDefaults) -> Vec<std::path::PathBuf> {
    let root = crate::config::agent_resolver::workspace_root_for(defaults);
    let _ = std::fs::create_dir_all(&root);
    vec![root]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::event_bus::GatewayEventBus;
    use crate::sync_primitives::Arc;

    /// `workspace_roots` must resolve the root the rest of the daemon
    /// actually uses, which means honouring a configured
    /// `[agents.defaults] workspace_root` rather than answering with the
    /// built-in default.
    ///
    /// Written because the two are a documented trap in this repo and this
    /// function had no test of its own: `default_workspace_root()` answers
    /// "where does this live when nothing is configured", while
    /// `workspace_root_for(defaults)` answers "where does it live". Reaching
    /// for the first is silent on an unconfigured machine — including every
    /// developer's and CI's — and wrong on exactly the installs that set the
    /// key. The signatures are the tell: one takes no argument, so it cannot
    /// be reading configuration at all.
    ///
    /// The assertion is also the reason this test needs no environment
    /// isolation, which is the second half of why it is worth having: with
    /// `workspace_root` set, `workspace_root_for` returns it without
    /// consulting `ALEPH_HOME` at all (`agent_resolver/mod.rs:487`). Every
    /// path here is one the test owns.
    #[test]
    fn workspace_roots_honours_a_configured_root_rather_than_the_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let configured = tmp.path().join("chosen");

        let defaults = crate::config::types::AgentDefaults {
            workspace_root: Some(configured.clone()),
            ..Default::default()
        };

        let roots = workspace_roots(&defaults);

        assert_eq!(
            roots.len(),
            1,
            "one root, so `canonical_roots[0]` is deterministic"
        );
        assert_eq!(
            roots[0], configured,
            "a configured workspace_root must win over default_workspace_root()"
        );
        assert!(
            configured.is_dir(),
            "the root must exist afterwards -- the jail refuses a root it cannot canonicalise, \
             so a resolver that only computes the path hands the jail an empty allowed set"
        );
    }

    /// The wire from the public `attach_event_bus` entry point to a real bus
    /// subscriber, exercised end-to-end rather than by checking that
    /// `start_flush_loop` was *called* — `build_router()`'s own tests already
    /// call `attach_event_bus` with zero assertions on the result, so a
    /// was-it-called guard would be trivially satisfied whether or not the
    /// loop inside it actually runs. Only an assertion that a frame arrived
    /// can fail when that inner call is cut.
    ///
    /// Polling shape matches `session::tests::a_child_write_reaches_the_server_held_screen`
    /// deliberately, not a fixed sleep-then-assert: bounded retries against a
    /// real 16ms cadence, so a slow CI runner doesn't turn into a flake.
    #[tokio::test(flavor = "multi_thread")]
    #[serial_test::parallel(pty_global_manager)]
    async fn a_write_reaches_a_real_subscriber_over_the_pty_screen_topic() {
        let bus = Arc::new(GatewayEventBus::new());
        let mut rx = bus.subscribe();

        // The exact call site `GatewayServer::build_router` makes — not a
        // shortcut into `PtyManager`'s internals.
        attach_event_bus(bus);

        let opts = SpawnOptions {
            rows: 10,
            cols: 40,
            ..Default::default()
        };
        let sid = manager().spawn(&opts).expect("spawn").session_id;
        let input: &[u8] = if cfg!(windows) {
            b"echo ALEPH_FLUSH_WIRE_OK\r\n"
        } else {
            b"printf 'ALEPH_FLUSH_WIRE_OK'\n"
        };
        manager().write(&sid, input).expect("write");

        let mut found: Option<(u16, u16)> = None;
        for _ in 0..100 {
            loop {
                match rx.try_recv() {
                    Ok(raw) => {
                        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                            continue;
                        };
                        if v.get("topic").and_then(|t| t.as_str())
                            != Some(aleph_protocol::pty::PTY_SCREEN_TOPIC)
                        {
                            continue;
                        }
                        let Some(data) = v.get("data") else { continue };
                        let Ok(frame) = serde_json::from_value::<aleph_protocol::pty::PtyScreenFrame>(
                            data.clone(),
                        ) else {
                            continue;
                        };
                        if frame.session_id == sid
                            && frame.patch.rows.iter().any(|r| {
                                r.runs
                                    .iter()
                                    .any(|run| run.text.contains("ALEPH_FLUSH_WIRE_OK"))
                            })
                        {
                            found = Some((frame.rows, frame.cols));
                        }
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            if found.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        manager().close(&sid).ok();
        // Geometry on the wire. The client-side unit tests prove `apply`
        // does the right thing WITH a frame's dimensions; only this one
        // proves the server puts them there. An implementation that read
        // dims outside the screen lock, or transposed them, passes every
        // other test in this change.
        assert_eq!(
            found,
            Some((10, 40)),
            "the frame must carry the geometry it was spawned with, rows first"
        );
    }
}
