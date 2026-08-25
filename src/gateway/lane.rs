//! Lane-based concurrency control for Gateway RPC methods.
//!
//! Prevents one category of RPC methods from starving others by partitioning
//! methods into lanes, each with its own semaphore-based concurrency limit.
//!
//! # Lanes
//!
//! - **Query**: Read-only queries (health, echo, config.get, etc.)
//! - **Execute**: Agent execution (agent.run, chat.send, etc.)
//! - **Mutate**: State mutations (config.patch, memory.store, etc.)
//! - **System**: System management (plugins.install, skills.remove, etc.)
//!
//! # Channel-class priority (Panel-first)
//!
//! Within the `Execute` and `Mutate` lanes, the manager keeps **two
//! independent semaphores**: a `desktop_*` pool reserved for the local
//! desktop Panel and a `shared_*` pool used by every other client (bots,
//! CLI, ...). A request tagged [`ChannelClass::Desktop`] tries the desktop
//! pool first and only falls back to the shared pool when the desktop pool
//! is empty. Requests tagged [`ChannelClass::Bot`]
//! can only ever land on the shared pool, so a flood of long-running bot
//! traffic cannot starve interactive Panel sessions.
//!
//! `Query` and `System` lanes keep a single shared pool — they are either
//! cheap (Query) or already rate-limited by other means (System).

use crate::sync_primitives::Arc;
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Traffic lane for categorizing RPC methods.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Lane {
    /// Read-only queries (health, echo, config.get, models.list, etc.)
    Query,
    /// Agent execution (agent.run, chat.send)
    Execute,
    /// State mutations (config.patch, memory.delete, session.compact, session.truncate, sessions.delete)
    Mutate,
    /// System management (plugins.install, plugins.uninstall, skills.install, skills.remove, logs.setLevel)
    System,
}

impl Lane {
    /// Map an RPC method name to its corresponding lane.
    ///
    /// Resolution order:
    /// 1. Explicit override map — looked up by full method name.
    /// 2. Suffix heuristic — last `.`-separated segment of the method name.
    /// 3. Default — `Lane::Mutate` (fail-safe: new side-effecting RPCs are
    ///    idempotency-protected by default).
    ///
    /// The previous implementation defaulted to `Lane::Query`, which let
    /// every uncovered side-effecting method silently bypass idempotency
    /// (Spec 2 / G1 fix).
    #[must_use]
    pub fn for_method(method: &str) -> Self {
        if let Some(lane) = Self::override_for(method) {
            return lane;
        }
        if let Some(dot) = method.rfind('.') {
            let suffix = &method[dot + 1..];
            match suffix {
                "get" | "list" | "search" | "status" | "describe" | "history" | "effective"
                | "catalog" | "neighbors" | "subscribe" | "unsubscribe" | "stats" | "browse" => {
                    return Self::Query
                }
                "install" | "uninstall" => return Self::System,
                "run" | "send" | "invoke" | "execute" => return Self::Execute,
                _ => {}
            }
        }
        Self::Mutate
    }

    /// Explicit overrides for methods whose name doesn't match the
    /// suffix heuristic or whose family puts them in a non-default lane.
    fn override_for(method: &str) -> Option<Self> {
        match method {
            // Read-only ops with no dot-suffix (or whose suffix doesn't
            // match the Query rule).
            "health" | "echo" | "version" | "system.info" | "request.state" => Some(Self::Query),
            // gateway.identity.get matches the .get suffix; listed
            // defensively in case it's ever renamed.
            "gateway.identity.get" => Some(Self::Query),
            // gateway.metrics.lanes / gateway.metrics.run_concurrency /
            // gateway.metrics.subagent_concurrency are read-only diagnostics
            // gauges. None of the three suffixes match the Query suffix
            // heuristic, so list them explicitly.
            "gateway.metrics.lanes"
            | "gateway.metrics.run_concurrency"
            | "gateway.metrics.subagent_concurrency" => Some(Self::Query),
            // gateway.credentials returns a read-only auth-surface snapshot.
            // No `.get`/`.list` suffix to trip the heuristic, so list explicitly.
            "gateway.credentials" => Some(Self::Query),
            // teams.usage is a read-only token aggregation over task_traces.
            // The `.usage` suffix isn't in the Query heuristic; keep it out of
            // Mutate so it isn't idempotency-guarded for nothing.
            "teams.usage" => Some(Self::Query),
            // projects.workspace.read is a pure read of one file under a room
            // bound directory. Its `.read` suffix matches no Query token, so
            // without this line it lands in Mutate and gets idempotency-guarded
            // for nothing. Its `.list` sibling passes on the suffix heuristic.
            "projects.workspace.read" => Some(Self::Query),
            // The rest of the group-chat reading surface. `teams.chat.history`
            // already passes on its `.history` suffix, but its siblings do not:
            // `.thread`, `.list_tasks` and `.list_templates` match no Query
            // token (the heuristic wants an exact `list`, not `list_tasks`), so
            // on a deployment with `require_idempotency_key = true` opening a
            // group chat left the task strip permanently empty and the durable
            // thread unloadable while the message history rendered fine.
            "teams.chat.thread" | "teams.list_tasks" | "teams.list_templates" => Some(Self::Query),
            // moa.listPresets returns a read-only `[moa]` config snapshot. The
            // `.listPresets` suffix doesn't match the Query heuristic's `.list`,
            // so classify it explicitly — keeps this hot read off the Mutate
            // lane and out of idempotency guarding.
            "moa.listPresets" => Some(Self::Query),
            // Both feed the artifacts pane's reading surface and neither writes
            // anything: `fs.read_file` backs the workspace-file preview,
            // `artifacts.read_text` backs the attachment preview beside it. The
            // `read_*` suffixes match no Query token, so without these two lines
            // a pure read lands in Mutate and an operator running with
            // `require_idempotency_key = true` gets `IDEMPOTENCY_KEY_REQUIRED`
            // for opening a text file.
            "fs.read_file" | "artifacts.read_text" => Some(Self::Query),
            // tools.in_flight is the read-only twin of tools.cancel_call: it
            // lists what is currently running and mutates nothing. `.in_flight`
            // matches no Query token, so the same deployment that made opening
            // a text file need an idempotency key made it impossible to ask
            // "what is running right now" — the one call you want when things
            // are stuck.
            "tools.in_flight" => Some(Self::Query),
            // agent.resume re-triggers an interrupted run — it starts agent
            // execution exactly like agent.run, it just supplies the input from
            // the session log instead of the request. The `.resume` suffix
            // matches no heuristic token, so without this line it defaults to
            // Mutate and a burst of resumes would compete on the generic
            // mutation lane instead of being throttled against the run
            // concurrency budget the Execute lane exists to hold.
            "agent.resume" => Some(Self::Execute),
            // skills.remove is package management, not a data delete →
            // System lane. memory.delete / sessions.delete / session.truncate
            // are data ops that fall through to default Mutate.
            "skills.remove" | "bundled.sync" => Some(Self::System),
            // logs.setLevel changes process-wide runtime config →
            // System lane (preserves pre-G1 hardcode behavior).
            "logs.setLevel" => Some(Self::System),
            _ => None,
        }
    }

    /// Whether this lane's methods should be idempotency-guarded.
    /// Query lane is read-only and doesn't need protection.
    #[must_use]
    pub const fn needs_idempotency(&self) -> bool {
        !matches!(self, Self::Query)
    }
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Query => write!(f, "Query"),
            Self::Execute => write!(f, "Execute"),
            Self::Mutate => write!(f, "Mutate"),
            Self::System => write!(f, "System"),
        }
    }
}

/// Origin class of a request, used for prioritising desktop Panel traffic
/// over bot/adapter traffic inside congested lanes.
///
/// Derived once per WebSocket connection from the peer address (and, in
/// the future, token issuer metadata). See [`crate::gateway::server`] for
/// the derivation policy.
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum ChannelClass {
    /// Local interactive Panel — typically the Tauri shell on loopback.
    /// May draw from the reserved desktop pool first, with a shared-pool
    /// fallback.
    Desktop,
    /// Anything that is not the local Panel — remote bots, chat adapters, a
    /// CLI reaching the gateway over the network. Only ever uses the shared
    /// pool.
    ///
    /// There is deliberately no separate CLI variant. One existed, documented
    /// as "kept so future policy can differentiate", and nothing ever produced
    /// it: the sole derivation site classifies purely on `client_ip`, so a
    /// local CLI on loopback is `Desktop` and a remote one is `Bot`. Its only
    /// effect was a test that read as coverage of a live policy path. If
    /// loopback-CLI ever needs to differ from Panel, adding the variant back is
    /// a two-line change at the derivation site — cheaper than carrying a
    /// permanently dead one.
    Bot,
}

impl ChannelClass {
    /// Whether requests of this class may draw from the reserved desktop
    /// pool before falling back to the shared pool.
    const fn uses_desktop_pool(self) -> bool {
        matches!(self, Self::Desktop)
    }
}

/// Configuration for lane concurrency limits.
///
/// Execute and Mutate lanes are split into a desktop-reserved pool plus a
/// shared pool — see the module-level docs for the priority policy.
/// `#[serde(default)]` is set on every field so old configs that don't
/// know about the channel-class fields keep loading.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LaneConfig {
    /// Maximum concurrent query requests (single pool, shared by all
    /// channel classes).
    pub query_concurrency: usize,
    /// Maximum concurrent system requests (single pool).
    pub system_concurrency: usize,

    /// Reserved Execute permits for desktop Panel. Local Panel requests
    /// try this pool first.
    pub desktop_execute_concurrency: usize,
    /// Shared Execute permits — used by bots, CLI, and as the Desktop
    /// fallback when the desktop pool is full.
    pub shared_execute_concurrency: usize,

    /// Reserved Mutate permits for desktop Panel.
    pub desktop_mutate_concurrency: usize,
    /// Shared Mutate permits.
    pub shared_mutate_concurrency: usize,

    /// Timeout, in seconds, for acquiring a lane permit on the shared
    /// pool. The desktop pool is non-blocking (try-then-fallback).
    pub acquire_timeout_secs: u64,
}

impl Default for LaneConfig {
    fn default() -> Self {
        Self {
            query_concurrency: 50,
            system_concurrency: 3,
            // Panel-first defaults: desktop reserves 4 Execute / 8 Mutate
            // permits, bots share 3 / 4. Total Execute capacity matches
            // the pre-split default (8) while guaranteeing desktop is
            // never blocked by bot saturation up to 4 concurrent loops.
            desktop_execute_concurrency: 4,
            shared_execute_concurrency: 3,
            desktop_mutate_concurrency: 8,
            shared_mutate_concurrency: 4,
            acquire_timeout_secs: 30,
        }
    }
}

/// Errors returned by lane operations.
#[derive(Debug)]
pub enum LaneError {
    /// The lane is congested and the permit could not be acquired within the timeout.
    Congested(Lane),
}

impl fmt::Display for LaneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Congested(lane) => {
                write!(f, "lane {lane} is congested, could not acquire permit")
            }
        }
    }
}

impl std::error::Error for LaneError {}

/// Semaphore pair for one lane.
///
/// `desktop` is `None` for lanes (Query / System) that aren't split per
/// channel class. `desktop_total` / `shared_total` snapshot the configured
/// capacity at construction so [`LaneManager::snapshot`] can report a stable
/// denominator alongside the live `available_permits()` reading from the
/// `Semaphore`.
struct LanePool {
    desktop: Option<Arc<Semaphore>>,
    shared: Arc<Semaphore>,
    desktop_total: Option<usize>,
    shared_total: usize,
}

/// Live occupancy of a single lane, suitable for diagnostics (`gateway.metrics.lanes`).
///
/// `*_available` is sampled from the underlying [`Semaphore`] at snapshot
/// time and may race with concurrent acquires/releases — it is a *gauge*,
/// not a transactional reading.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LaneOccupancy {
    /// Lane name (matches the [`Lane`] `Display` impl).
    pub lane: String,
    /// Total desktop-pool permits, `None` for lanes (Query / System) that
    /// don't split per channel class.
    pub desktop_total: Option<usize>,
    /// Currently free desktop-pool permits.
    pub desktop_available: Option<usize>,
    /// Total shared-pool permits.
    pub shared_total: usize,
    /// Currently free shared-pool permits.
    pub shared_available: usize,
}

/// Lane-based concurrency manager.
///
/// Each lane has its own semaphore pair. Acquiring a permit on one lane
/// does not affect availability on other lanes, preventing one category
/// of RPC methods from starving others; within Execute/Mutate lanes the
/// desktop Panel additionally gets a reserved pool that bots cannot
/// touch.
pub struct LaneManager {
    lanes: HashMap<Lane, LanePool>,
    timeout: Duration,
}

impl LaneManager {
    /// Create a new `LaneManager` from the given configuration.
    #[must_use]
    pub fn new(config: LaneConfig) -> Self {
        let mut lanes = HashMap::new();

        // Single-pool lanes.
        lanes.insert(
            Lane::Query,
            LanePool {
                desktop: None,
                shared: Arc::new(Semaphore::new(config.query_concurrency)),
                desktop_total: None,
                shared_total: config.query_concurrency,
            },
        );
        lanes.insert(
            Lane::System,
            LanePool {
                desktop: None,
                shared: Arc::new(Semaphore::new(config.system_concurrency)),
                desktop_total: None,
                shared_total: config.system_concurrency,
            },
        );

        // Channel-class-split lanes.
        lanes.insert(
            Lane::Execute,
            LanePool {
                desktop: Some(Arc::new(Semaphore::new(config.desktop_execute_concurrency))),
                shared: Arc::new(Semaphore::new(config.shared_execute_concurrency)),
                desktop_total: Some(config.desktop_execute_concurrency),
                shared_total: config.shared_execute_concurrency,
            },
        );
        lanes.insert(
            Lane::Mutate,
            LanePool {
                desktop: Some(Arc::new(Semaphore::new(config.desktop_mutate_concurrency))),
                shared: Arc::new(Semaphore::new(config.shared_mutate_concurrency)),
                desktop_total: Some(config.desktop_mutate_concurrency),
                shared_total: config.shared_mutate_concurrency,
            },
        );

        Self {
            lanes,
            timeout: Duration::from_secs(config.acquire_timeout_secs),
        }
    }

    /// Acquire a permit for the lane corresponding to the given RPC method.
    ///
    /// `channel_class` selects which semaphore pool the request may draw
    /// from. [`ChannelClass::Desktop`] tries the reserved desktop pool
    /// first and falls back to the shared pool with timeout. Other
    /// classes go straight to the shared pool.
    ///
    /// Returns an `OwnedSemaphorePermit` that releases the slot on drop,
    /// or `LaneError::Congested` if the shared pool is full for longer
    /// than `acquire_timeout_secs`.
    pub async fn acquire(
        &self,
        method: &str,
        channel_class: ChannelClass,
    ) -> Result<OwnedSemaphorePermit, LaneError> {
        let lane = Lane::for_method(method);
        let pool = match self.lanes.get(&lane) {
            Some(p) => p,
            None => return Err(LaneError::Congested(lane)),
        };

        // Desktop class tries the reserved pool first, non-blocking.
        // Bot never touches the desktop pool.
        if channel_class.uses_desktop_pool() {
            if let Some(desktop_sem) = pool.desktop.as_ref() {
                if let Ok(permit) = desktop_sem.clone().try_acquire_owned() {
                    return Ok(permit);
                }
            }
        }

        let shared = pool.shared.clone();
        match tokio::time::timeout(self.timeout, shared.acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_closed)) => {
                // Semaphore closed — treat as congested
                Err(LaneError::Congested(lane))
            }
            Err(_elapsed) => Err(LaneError::Congested(lane)),
        }
    }

    /// Snapshot the live occupancy of every lane in a fixed order
    /// (Query, Execute, Mutate, System) for diagnostics endpoints.
    ///
    /// Each entry's `*_available` field is sampled from the underlying
    /// [`Semaphore`] and races with concurrent acquires/releases — treat it
    /// as a gauge, not a transactional reading.
    #[must_use]
    pub fn snapshot(&self) -> Vec<LaneOccupancy> {
        // Fixed iteration order makes the output stable and predictable.
        // (HashMap iteration would otherwise reshuffle on each call.)
        [Lane::Query, Lane::Execute, Lane::Mutate, Lane::System]
            .iter()
            .filter_map(|lane| {
                let pool = self.lanes.get(lane)?;
                Some(LaneOccupancy {
                    lane: lane.to_string(),
                    desktop_total: pool.desktop_total,
                    desktop_available: pool.desktop.as_ref().map(|s| s.available_permits()),
                    shared_total: pool.shared_total,
                    shared_available: pool.shared.available_permits(),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_acquire_query_lane() {
        let manager = LaneManager::new(LaneConfig::default());
        let result = manager.acquire("health", ChannelClass::Bot).await;
        assert!(result.is_ok(), "should acquire query lane for 'health'");
    }

    #[tokio::test]
    async fn test_lane_for_method() {
        // Query lane (explicit overrides + suffix heuristic).
        assert_eq!(Lane::for_method("health"), Lane::Query);
        assert_eq!(Lane::for_method("echo"), Lane::Query);
        assert_eq!(Lane::for_method("config.get"), Lane::Query);
        assert_eq!(Lane::for_method("models.list"), Lane::Query);

        // Execute lane
        assert_eq!(Lane::for_method("agent.run"), Lane::Execute);
        assert_eq!(Lane::for_method("chat.send"), Lane::Execute);

        // teams.usage is a read-only aggregator → explicit Query override
        // (its `.usage` suffix isn't covered by the heuristic).
        assert_eq!(Lane::for_method("teams.usage"), Lane::Query);

        // Mutate lane
        assert_eq!(Lane::for_method("config.patch"), Lane::Mutate);
        assert_eq!(Lane::for_method("memory.delete"), Lane::Mutate);
        assert_eq!(Lane::for_method("session.compact"), Lane::Mutate);
        assert_eq!(Lane::for_method("session.truncate"), Lane::Mutate);
        assert_eq!(Lane::for_method("sessions.delete"), Lane::Mutate);

        // System lane
        assert_eq!(Lane::for_method("plugins.install"), Lane::System);
        assert_eq!(Lane::for_method("plugins.uninstall"), Lane::System);
        assert_eq!(Lane::for_method("skills.install"), Lane::System);
        assert_eq!(Lane::for_method("skills.remove"), Lane::System);
        assert_eq!(Lane::for_method("logs.setLevel"), Lane::System);
    }

    #[tokio::test]
    async fn test_execute_lane_saturation() {
        // Reproduce the pre-split single-pool saturation behavior by
        // sizing both desktop and shared to 1 each and exhausting both.
        let config = LaneConfig {
            desktop_execute_concurrency: 0,
            shared_execute_concurrency: 1,
            acquire_timeout_secs: 1,
            ..Default::default()
        };
        let manager = LaneManager::new(config);

        // Hold the only shared permit (via Bot, which can't escape to
        // the desktop pool).
        let _permit = manager
            .acquire("agent.run", ChannelClass::Bot)
            .await
            .expect("first acquire should succeed");

        // Second Bot acquire should time out — the shared pool is full
        // and Bot cannot touch the desktop pool (which has 0 permits
        // anyway).
        let result = manager.acquire("chat.send", ChannelClass::Bot).await;
        assert!(
            result.is_err(),
            "second acquire should fail (lane saturated)"
        );
        match result.unwrap_err() {
            LaneError::Congested(lane) => {
                assert_eq!(lane, Lane::Execute);
            }
        }
    }

    #[tokio::test]
    async fn snapshot_reports_per_lane_occupancy_in_fixed_order() {
        let manager = LaneManager::new(LaneConfig {
            query_concurrency: 5,
            system_concurrency: 2,
            desktop_execute_concurrency: 4,
            shared_execute_concurrency: 3,
            desktop_mutate_concurrency: 8,
            shared_mutate_concurrency: 4,
            acquire_timeout_secs: 30,
        });

        let snap = manager.snapshot();
        assert_eq!(snap.len(), 4, "expect one entry per lane");
        // Fixed iteration order so this isn't HashMap-shuffled.
        assert_eq!(snap[0].lane, "Query");
        assert_eq!(snap[1].lane, "Execute");
        assert_eq!(snap[2].lane, "Mutate");
        assert_eq!(snap[3].lane, "System");

        // Query / System are single-pool — no desktop split.
        assert!(snap[0].desktop_total.is_none() && snap[0].desktop_available.is_none());
        assert!(snap[3].desktop_total.is_none() && snap[3].desktop_available.is_none());
        assert_eq!(snap[0].shared_total, 5);
        assert_eq!(snap[0].shared_available, 5);

        // Execute / Mutate split per channel class.
        assert_eq!(snap[1].desktop_total, Some(4));
        assert_eq!(snap[1].desktop_available, Some(4));
        assert_eq!(snap[1].shared_total, 3);
        assert_eq!(snap[1].shared_available, 3);

        // Holding a Desktop Execute permit drops desktop_available by 1.
        let _permit = manager
            .acquire("agent.run", ChannelClass::Desktop)
            .await
            .expect("desktop acquire should succeed");
        let snap2 = manager.snapshot();
        let exec = snap2.iter().find(|s| s.lane == "Execute").unwrap();
        assert_eq!(
            exec.desktop_available,
            Some(3),
            "live desktop_available should reflect the held permit"
        );
        assert_eq!(
            exec.shared_available, 3,
            "shared pool must be untouched when Desktop drew from its own pool"
        );
    }

    #[test]
    fn test_needs_idempotency() {
        assert!(!Lane::Query.needs_idempotency());
        assert!(Lane::Execute.needs_idempotency());
        assert!(Lane::Mutate.needs_idempotency());
        assert!(Lane::System.needs_idempotency());
    }

    #[tokio::test]
    async fn test_different_lanes_independent() {
        let config = LaneConfig {
            desktop_execute_concurrency: 0,
            shared_execute_concurrency: 1,
            acquire_timeout_secs: 1,
            ..Default::default()
        };
        let manager = LaneManager::new(config);

        // Saturate execute lane.
        let _permit = manager
            .acquire("agent.run", ChannelClass::Bot)
            .await
            .expect("execute acquire should succeed");

        // Query lane should still work.
        let result = manager.acquire("health", ChannelClass::Bot).await;
        assert!(
            result.is_ok(),
            "query lane should be independent of execute lane"
        );
    }

    // -------------------------------------------------------------------------
    // Spec 2 / G1 — heuristic + override coverage
    // -------------------------------------------------------------------------

    #[test]
    fn new_rpc_defaults_to_mutate() {
        assert_eq!(Lane::for_method("tools.invoke"), Lane::Execute);
        assert_eq!(Lane::for_method("agents.create"), Lane::Mutate);
        assert_eq!(Lane::for_method("agents.delete"), Lane::Mutate);
        assert_eq!(Lane::for_method("cron.toggle"), Lane::Mutate);
        assert_eq!(Lane::for_method("heartbeat.wake"), Lane::Mutate);
        assert_eq!(Lane::for_method("memory.search"), Lane::Query);
        assert_eq!(Lane::for_method("graph.neighbors"), Lane::Query);
        assert_eq!(Lane::for_method("trace.list"), Lane::Query);
        assert_eq!(Lane::for_method("daemon.shutdown"), Lane::Mutate);
        assert_eq!(Lane::for_method("plugins.install"), Lane::System);
    }

    #[test]
    fn explicit_overrides_win_over_heuristic() {
        assert_eq!(Lane::for_method("health"), Lane::Query);
        assert_eq!(Lane::for_method("echo"), Lane::Query);
        assert_eq!(Lane::for_method("version"), Lane::Query);
        assert_eq!(Lane::for_method("system.info"), Lane::Query);
        assert_eq!(Lane::for_method("request.state"), Lane::Query);
    }

    /// The two reading RPCs behind the artifacts pane. Neither `read_file` nor
    /// `read_text` matches a Query suffix, so without their overrides a pure
    /// read is idempotency-guarded and fails outright under
    /// `require_idempotency_key`.
    #[test]
    fn the_panes_read_rpcs_are_query_lane() {
        assert_eq!(Lane::for_method("fs.read_file"), Lane::Query);
        assert_eq!(Lane::for_method("artifacts.read_text"), Lane::Query);
        assert_eq!(Lane::for_method("artifacts.list"), Lane::Query);
        assert!(!Lane::for_method("artifacts.read_text").needs_idempotency());
    }

    /// `tools.in_flight` only reads the in-flight registry; its mutating twin
    /// is `tools.cancel_call`, which correctly stays on Mutate.
    #[test]
    fn the_in_flight_diagnostic_is_query_lane() {
        assert_eq!(Lane::for_method("tools.in_flight"), Lane::Query);
        assert!(!Lane::for_method("tools.in_flight").needs_idempotency());
        assert_eq!(Lane::for_method("tools.cancel_call"), Lane::Mutate);
    }

    #[test]
    fn unknown_method_with_no_suffix_defaults_to_mutate() {
        assert_eq!(Lane::for_method("totally_unknown_rpc"), Lane::Mutate);
    }

    #[test]
    fn legacy_method_mappings_preserved() {
        assert_eq!(Lane::for_method("agent.run"), Lane::Execute);
        assert_eq!(Lane::for_method("chat.send"), Lane::Execute);
        assert_eq!(Lane::for_method("config.patch"), Lane::Mutate);
        assert_eq!(Lane::for_method("memory.delete"), Lane::Mutate);
        assert_eq!(Lane::for_method("sessions.delete"), Lane::Mutate);
        assert_eq!(Lane::for_method("skills.remove"), Lane::System);
        assert_eq!(Lane::for_method("plugins.uninstall"), Lane::System);
        assert_eq!(Lane::for_method("skills.install"), Lane::System);
        assert_eq!(Lane::for_method("logs.setLevel"), Lane::System);
    }

    // -------------------------------------------------------------------------
    // Channel-class dual-pool — Panel-first priority
    // -------------------------------------------------------------------------

    /// (Goal case a) Bots saturate the shared Execute pool; Desktop must
    /// still acquire immediately via the reserved desktop pool.
    #[tokio::test]
    async fn bot_saturation_does_not_block_desktop_execute() {
        let config = LaneConfig {
            desktop_execute_concurrency: 4,
            shared_execute_concurrency: 3,
            acquire_timeout_secs: 1,
            ..Default::default()
        };
        let manager = LaneManager::new(config);

        // Three Bot requests fill the shared pool.
        let _b1 = manager
            .acquire("agent.run", ChannelClass::Bot)
            .await
            .expect("bot 1 should acquire shared");
        let _b2 = manager
            .acquire("agent.run", ChannelClass::Bot)
            .await
            .expect("bot 2 should acquire shared");
        let _b3 = manager
            .acquire("agent.run", ChannelClass::Bot)
            .await
            .expect("bot 3 should acquire shared");

        // Desktop must still acquire well within the timeout — via the
        // reserved desktop pool.
        let desktop_result = tokio::time::timeout(
            Duration::from_millis(100),
            manager.acquire("chat.send", ChannelClass::Desktop),
        )
        .await
        .expect("desktop must not wait when shared is saturated");
        assert!(
            desktop_result.is_ok(),
            "desktop should acquire via desktop_sem"
        );
    }

    /// (Goal case b) Desktop saturates *both* its reserved pool and the
    /// shared pool. A further Desktop request must *queue* on the shared
    /// pool rather than fail immediately — and complete once a shared
    /// permit is freed.
    #[tokio::test]
    async fn desktop_overflow_queues_on_shared_pool() {
        use tokio::sync::Notify;

        let config = LaneConfig {
            desktop_execute_concurrency: 1,
            shared_execute_concurrency: 1,
            acquire_timeout_secs: 5,
            ..Default::default()
        };
        let manager = Arc::new(LaneManager::new(config));

        // Fill the desktop pool.
        let _d1 = manager
            .acquire("agent.run", ChannelClass::Desktop)
            .await
            .expect("first desktop should acquire desktop_sem");
        // Fill the shared pool via a Bot, so when we later release it
        // we know exactly which pool freed up.
        let bot_permit = manager
            .acquire("agent.run", ChannelClass::Bot)
            .await
            .expect("bot should acquire shared");

        // A second Desktop acquire must now queue on shared.
        let mgr = manager.clone();
        let started = Arc::new(Notify::new());
        let started_clone = started.clone();
        let handle = tokio::spawn(async move {
            started_clone.notify_one();
            mgr.acquire("agent.run", ChannelClass::Desktop).await
        });

        started.notified().await;
        // Give the spawned task a moment to attempt acquire and end up
        // queued on the shared semaphore.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !handle.is_finished(),
            "Desktop overflow must queue on shared, not fail immediately"
        );

        // Release the bot's shared permit. The queued Desktop request
        // should pick it up — proving it actually fell back to shared.
        drop(bot_permit);

        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("queued Desktop acquire should complete once shared frees")
            .expect("spawned task must not panic");
        assert!(
            result.is_ok(),
            "Desktop should acquire via shared after Bot releases"
        );
    }

    /// (Goal case c) Bots must never reach into the reserved desktop
    /// pool — even when the desktop pool has free permits and the shared
    /// pool is saturated.
    #[tokio::test]
    async fn bot_cannot_borrow_from_desktop_pool() {
        let config = LaneConfig {
            desktop_execute_concurrency: 10,
            shared_execute_concurrency: 1,
            acquire_timeout_secs: 1,
            ..Default::default()
        };
        let manager = LaneManager::new(config);

        // Bot fills the shared pool.
        let _bot = manager
            .acquire("agent.run", ChannelClass::Bot)
            .await
            .expect("bot should acquire shared");

        // Second Bot must time out — Bot is forbidden from touching
        // the 10 free desktop permits.
        let result = manager.acquire("chat.send", ChannelClass::Bot).await;
        match result {
            Err(LaneError::Congested(Lane::Execute)) => {}
            other => panic!(
                "Bot should be Congested when shared pool saturates, got {:?}",
                other
            ),
        }
    }

    /// (Goal case d) When both pools are unavailable, acquire still
    /// returns `LaneError::Congested` after the configured timeout.
    #[tokio::test]
    async fn acquire_timeout_returns_congested_when_both_pools_full() {
        let config = LaneConfig {
            desktop_execute_concurrency: 0,
            shared_execute_concurrency: 1,
            acquire_timeout_secs: 1,
            ..Default::default()
        };
        let manager = LaneManager::new(config);

        let _hold = manager
            .acquire("agent.run", ChannelClass::Bot)
            .await
            .expect("first bot should acquire shared");

        // Desktop class: desktop pool has 0 permits, shared is full →
        // must time out with Congested.
        let result = manager.acquire("chat.send", ChannelClass::Desktop).await;
        match result {
            Err(LaneError::Congested(Lane::Execute)) => {}
            other => panic!("expected Congested(Execute), got {:?}", other),
        }
    }

    /// Query lane has no desktop split — any channel class shares the
    /// same pool. Verify Desktop still congests when the Query pool is
    /// saturated by Bots (i.e. there is no hidden desktop pool quietly
    /// rescuing it).
    #[tokio::test]
    async fn query_lane_is_shared_across_channel_classes() {
        let config = LaneConfig {
            query_concurrency: 1,
            acquire_timeout_secs: 1,
            ..Default::default()
        };
        let manager = LaneManager::new(config);

        let _bot = manager
            .acquire("health", ChannelClass::Bot)
            .await
            .expect("bot should acquire query");
        let result = manager.acquire("config.get", ChannelClass::Desktop).await;
        match result {
            Err(LaneError::Congested(Lane::Query)) => {}
            other => panic!(
                "Query is single-pool; Desktop must Congest when Bot saturates, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn lane_config_round_trips_through_toml() {
        // Backwards compat: missing `lane` config keys must fall back to
        // defaults rather than break TOML loading.
        let cfg: LaneConfig = toml::from_str("").expect("empty TOML should yield defaults");
        let defaults = LaneConfig::default();
        assert_eq!(cfg.query_concurrency, defaults.query_concurrency);
        assert_eq!(
            cfg.desktop_execute_concurrency,
            defaults.desktop_execute_concurrency
        );
        assert_eq!(
            cfg.shared_execute_concurrency,
            defaults.shared_execute_concurrency
        );
        assert_eq!(
            cfg.desktop_mutate_concurrency,
            defaults.desktop_mutate_concurrency
        );
        assert_eq!(
            cfg.shared_mutate_concurrency,
            defaults.shared_mutate_concurrency
        );

        // Partial override only touches the named field.
        let partial: LaneConfig = toml::from_str(
            r#"
desktop_execute_concurrency = 16
shared_execute_concurrency = 2
"#,
        )
        .expect("partial TOML should deserialize");
        assert_eq!(partial.desktop_execute_concurrency, 16);
        assert_eq!(partial.shared_execute_concurrency, 2);
        assert_eq!(
            partial.desktop_mutate_concurrency,
            defaults.desktop_mutate_concurrency
        );
    }
}
