//! Post-run governance triggers + session-facing helpers.
//!
//! Three mechanical consumers of the topology (zero judgment, R7-clean):
//! - `notify_goal_settled` — at a goal's victory-claim moment, poke every
//!   cron watcher paired to it via a `watches` edge (debounced). Reviewing a
//!   win at the moment it is claimed is when cheap wins get caught; the
//!   watcher's periodic cadence still backstops.
//! - `governing_owner` — the `owns_reference` write-protection lookup used
//!   by the goal tool's objective ACL (§6.2 of the design: a governed loop
//!   editing its own reference is exactly what must be denied).
//! - `render_session_topology` — deterministic bytes for the
//!   `GraphTopologyLayer` prompt injection (no timestamps, no counters: the
//!   graph unchanged ⇒ bytes unchanged, so the prompt cache holds).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use once_cell::sync::OnceCell;
use tracing::{info, warn};

use crate::loop_graph::types::EdgeKind;
use crate::sync_primitives::Mutex;
use crate::tasks::cron::SharedCronService;

/// The graph is scoped to the default agent. Every reader here — watcher
/// pokes, the objective ACL, the prompt injection — plus the doctor lint and
/// the `loop_graph` tool now name the SAME constant, so there is one answer to
/// "which scope am I in". (There used to be four spellings, one of them a
/// model-facing arg that no reader honored.)
const DEFAULT_AGENT: &str = crate::routing::DEFAULT_AGENT_ID;

/// Minimum interval between pokes of the SAME watcher — structural rate
/// limit (not a judgment): a burst of goal settlements collapses into one
/// watcher run.
const WATCH_DEBOUNCE: Duration = Duration::from_secs(60);

static CRON_TRIGGER: OnceCell<SharedCronService> = OnceCell::new();
static DEBOUNCE: OnceCell<Mutex<HashMap<String, Instant>>> = OnceCell::new();

/// Install the cron handle that powers watcher pokes. Called once at boot
/// next to `loop_graph::init_global`; absent (None / never called) the
/// trigger degrades to a no-op and watchers rely on their own cadence.
pub fn init_cron_trigger(svc: Option<SharedCronService>) {
    if let Some(s) = svc {
        let _ = CRON_TRIGGER.set(s);
    }
}

fn debounce_pass(watcher_job_id: &str) -> bool {
    let map = DEBOUNCE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    match guard.get(watcher_job_id) {
        Some(last) if now.duration_since(*last) < WATCH_DEBOUNCE => false,
        _ => {
            guard.insert(watcher_job_id.to_string(), now);
            true
        }
    }
}

/// Forget the stamp [`debounce_pass`] just recorded — called when the poke it
/// admitted failed to run (cron "already running" / disabled), so the failure
/// does not consume the window: a settle landing while the watcher is mid-run
/// would otherwise be dropped AND suppress every retry for the next 60s.
/// Safe: the fresh stamp blocks concurrent passes for the whole window, so the
/// entry removed here is always the one this failed poke inserted.
fn debounce_rollback(watcher_job_id: &str) {
    let map = DEBOUNCE.get_or_init(|| Mutex::new(HashMap::new()));
    map.lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(watcher_job_id);
}

/// Can a watcher with this id be poked on a victory claim?
///
/// Only `cron:` watchers can: the poke is `CronService::run_job`, and nothing
/// else in the graph's vocabulary has a "run it now" handle. A watcher of any
/// other kind still satisfies `lint_naked_loops` and still renders in the
/// prompt as watching its target — it simply never gets the immediate review,
/// only whatever cadence it has of its own. `pair` always builds a `cron:`
/// watcher, so this is reachable only through a hand-wired `link`, which is
/// why that action says so at write time. One predicate, both readers.
#[must_use]
pub fn watcher_is_pokeable(watcher_id: &str) -> bool {
    watcher_id.starts_with("cron:")
}

/// Cron job ids of every `watches` watcher pointed at `node_id`. Pure lookup
/// (unit-testable); empty on store errors.
fn watcher_jobs_for(store: &crate::loop_graph::LoopGraphStore, node_id: &str) -> Vec<String> {
    store
        .list_edges(DEFAULT_AGENT)
        .map(|edges| {
            edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Watches && e.to_id == node_id)
                .filter_map(|e| {
                    e.from_id.strip_prefix("cron:").map(|s| s.to_string()).or_else(|| {
                        warn!(
                            watcher = %e.from_id,
                            "loop_graph: watcher cannot be poked (not a cron loop) — cadence only"
                        );
                        None
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Poke every cron watcher paired (via `watches`) to `node_id`. Best-effort
/// and bounded: no graph / no store / no cron handle / no watchers → no-op.
///
/// Returns whether the caller's one-shot settle claim was *earned*: `true`
/// when there was nothing to poke (no graph, no watchers — the claim costs
/// nothing and re-asking every turn would be pure noise) or when at least one
/// watcher was actually poked. `false` says "watchers exist and none of them
/// ran", which is the only case where holding the claim would retire the
/// victory review for good — the caller releases it so the next observation of
/// the same terminal row retries.
async fn notify_node_settled(node_id: &str) -> bool {
    let Some(store) = crate::loop_graph::global() else {
        return true;
    };
    let watcher_jobs = watcher_jobs_for(&store, node_id);
    if watcher_jobs.is_empty() {
        return true;
    }
    let Some(cron) = CRON_TRIGGER.get() else {
        info!(node = %node_id, "loop_graph: watchers paired but no cron trigger handle");
        return false;
    };
    // A watcher held off by the 60s debounce counts as poked: the run it was
    // debounced against is the review this settle wanted.
    let mut any_poked = false;
    for job_id in watcher_jobs {
        if !debounce_pass(&job_id) {
            any_poked = true;
            continue;
        }
        let service = cron.lock().await;
        match service.run_job(&job_id).await {
            Ok(()) => {
                any_poked = true;
                info!(node = %node_id, watcher = %job_id,
                    "loop_graph: victory claim — watcher cron poked");
            }
            Err(e) => {
                debounce_rollback(&job_id);
                warn!(node = %node_id, watcher = %job_id, error = %e,
                    "loop_graph: failed to poke watcher cron (debounce rolled back)");
            }
        }
    }
    any_poked
}

/// Goal victory-claim entry. Call sites (all guarded by the store's
/// settle-notify CAS): the goal continuation hook's gate-less terminal
/// complete and gate-pass commit moments, plus the goal tool's Passive
/// `Complete` arm (`builtin_tools/goal.rs` — Passive goals never reach the
/// continuation hook).
///
/// Returns `false` when watchers exist but none could be poked — the caller
/// holding a one-shot claim must give it back (`release_settle_notify`), or
/// this completion is never reviewed.
pub async fn notify_goal_settled(session: &str) -> bool {
    notify_node_settled(&format!("goal:{session}")).await
}

/// Team victory-claim entry — a disband is the team's "we're done" moment.
/// Call site: `team_disband` success path. No one-shot claim guards this one
/// (a disband happens once), so the return is informational.
pub async fn notify_team_settled(team_id: &str) -> bool {
    notify_node_settled(&format!("team:{team_id}")).await
}

/// The id of the loop owning `goal:<session>`'s reference via an
/// `owns_reference` edge, if any. Pure lookup for the objective ACL.
///
/// `Ok(None)` means "genuinely ungoverned" — including the legitimate case
/// where the loop-graph subsystem never booted. `Err` means the question could
/// not be answered, and callers **must not** read that as permission: this is
/// an ACL, and collapsing a locked/busy DB into `None` turned the §6.2 write
/// protection off for exactly the call that hit the error, with no log line
/// and no trace. That fail-OPEN shape has already cost this repo once on the
/// goal subsystem itself.
pub fn governing_owner(session: &str) -> crate::error::Result<Option<String>> {
    let Some(store) = crate::loop_graph::global() else {
        return Ok(None);
    };
    governing_owner_in(&store, session)
}

/// Store-taking form of [`governing_owner`] (unit-testable without the
/// process global).
fn governing_owner_in(
    store: &crate::loop_graph::LoopGraphStore,
    session: &str,
) -> crate::error::Result<Option<String>> {
    let node_id = format!("goal:{session}");
    // Raw-column read, not `list_edges`: that one is fail-soft and DROPS a row
    // it cannot decode, which for an ACL is indistinguishable from "no such
    // edge" — i.e. a grant. See `LoopGraphStore::owns_reference_sources`.
    Ok(store
        .owns_reference_sources(DEFAULT_AGENT, &node_id)?
        .into_iter()
        .next())
}

/// Deterministic topology context for a governed session's prompt. `None`
/// (no graph / session not a registered node) leaves the prompt
/// byte-identical. Content is drawn from graph rows only — no clocks, no
/// counters — so unchanged graph ⇒ unchanged bytes (cache-safe).
#[must_use]
pub fn render_session_topology(session: &str) -> Option<String> {
    let store = crate::loop_graph::global()?;
    render_session_topology_in(&store, session)
}

/// Store-taking form of [`render_session_topology`].
#[must_use]
fn render_session_topology_in(
    store: &crate::loop_graph::LoopGraphStore,
    session: &str,
) -> Option<String> {
    let node_id = format!("goal:{session}");
    let node = store.get_node(DEFAULT_AGENT, &node_id).ok()??;
    let edges = store.list_edges(DEFAULT_AGENT).ok()?;
    let nodes = store.list_nodes(DEFAULT_AGENT).ok()?;
    let label_of = |id: &str| -> String {
        nodes
            .iter()
            .find(|n| n.id == id)
            .map_or_else(|| id.to_string(), |n| format!("{} ({})", id, n.label))
    };

    let mut out = String::new();
    out.push_str(&format!(
        "本会话是循环治理图中的节点 {}（{}）。\n",
        node.id, node.label
    ));
    if let Some(c) = &node.cadence {
        out.push_str(&format!("声明节奏: {c}\n"));
    }
    for e in edges.iter().filter(|e| e.to_id == node_id) {
        match e.kind {
            EdgeKind::Watches => out.push_str(&format!(
                "看守你的环: {}——你的胜利宣称会被它从反指标视角复核，用便宜方式赢没有意义。\n",
                label_of(&e.from_id)
            )),
            EdgeKind::OwnsReference => out.push_str(&format!(
                "你的 objective 由 {} 治理：你对自己的参照只读。认为目标本身错了→写提案 note\
                 （note_manage，tag: reference-proposal），由治理环裁决。\n",
                label_of(&e.from_id)
            )),
            EdgeKind::Audits => out.push_str(&format!(
                "审计你的环: {}——它定期核验你的数字仍触到现实。\n",
                label_of(&e.from_id)
            )),
            _ => {}
        }
    }
    for e in edges
        .iter()
        .filter(|e| e.from_id == node_id && e.kind == EdgeKind::AnchoredBy)
    {
        out.push_str(&format!("你的锚点: {}\n", label_of(&e.to_id)));
    }
    let mut roots: Vec<_> = nodes
        .iter()
        .filter(|n| n.kind == crate::loop_graph::NodeKind::Root)
        .collect();
    roots.sort_by(|a, b| a.id.cmp(&b.id));
    for r in roots {
        if let Some(body) = &r.body {
            out.push_str(&format!(
                "根参照 {}（人供给——你可以引用、必须遵循、无权修改）: {body}\n",
                r.id
            ));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_graph::{GraphEdge, GraphNode, LoopGraphStore, NodeKind, Origin};
    use crate::sync_primitives::Arc;

    fn seeded_store() -> (tempfile::TempDir, Arc<LoopGraphStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(LoopGraphStore::open(&dir.path().join("g.db")).unwrap());
        store
            .upsert_node(&GraphNode::new(
                DEFAULT_AGENT,
                "goal:sess-1",
                NodeKind::LoopGoal,
                "被治理的目标",
                Origin::Llm,
            ))
            .unwrap();
        store
            .upsert_node(
                &GraphNode::new(
                    DEFAULT_AGENT,
                    "cron:steward",
                    NodeKind::LoopCron,
                    "月度参照复审",
                    Origin::Llm,
                )
                .with_cadence("monthly"),
            )
            .unwrap();
        store
            .upsert_node(
                &GraphNode::new(
                    DEFAULT_AGENT,
                    "root:aleph",
                    NodeKind::Root,
                    "什么算更好",
                    Origin::Human,
                )
                .with_body("用户真实工作被推进且不被打扰 > 任何代理指标"),
            )
            .unwrap();
        store
            .upsert_edge(&GraphEdge::new(
                DEFAULT_AGENT,
                "cron:steward",
                "goal:sess-1",
                EdgeKind::OwnsReference,
                Origin::Llm,
            ))
            .unwrap();
        (dir, store)
    }

    #[test]
    fn watcher_jobs_resolve_for_goal_and_team_nodes() {
        let (_dir, store) = seeded_store();
        store
            .upsert_node(&GraphNode::new(
                DEFAULT_AGENT,
                "team:release-crew",
                NodeKind::Team,
                "发版小队",
                Origin::Llm,
            ))
            .unwrap();
        store
            .upsert_node(
                &GraphNode::new(
                    DEFAULT_AGENT,
                    "cron:team-watch",
                    NodeKind::LoopCron,
                    "小队看守",
                    Origin::Llm,
                )
                .with_cadence("nightly"),
            )
            .unwrap();
        store
            .upsert_edge(&GraphEdge::new(
                DEFAULT_AGENT,
                "cron:team-watch",
                "team:release-crew",
                EdgeKind::Watches,
                Origin::Llm,
            ))
            .unwrap();

        assert_eq!(
            watcher_jobs_for(&store, "team:release-crew"),
            vec!["team-watch".to_string()],
            "watches edge on a team node must surface its cron watcher"
        );
        assert!(
            watcher_jobs_for(&store, "goal:sess-1").is_empty(),
            "owns_reference edge is not a watcher"
        );
        assert!(watcher_jobs_for(&store, "team:nonexistent").is_empty());
    }

    /// The "watched" the lint sees and the "watched" a victory claim can reach
    /// are not the same set — and the narrower one has no other way to be
    /// discovered, so `link` tells the model at write time.
    #[test]
    fn only_cron_watchers_can_be_poked() {
        assert!(watcher_is_pokeable("cron:daily-counter-metric"));
        for id in ["heartbeat:probe-1", "daemon:dreaming", "team:crew"] {
            assert!(
                !watcher_is_pokeable(id),
                "{id} has no run-it-now handle, so it only ever runs on its own cadence"
            );
        }
    }

    #[test]
    fn governing_owner_and_topology_render() {
        let (_dir, store) = seeded_store();

        assert_eq!(
            governing_owner_in(&store, "sess-1").unwrap().as_deref(),
            Some("cron:steward"),
            "owns_reference edge must surface the owner"
        );
        assert_eq!(
            governing_owner_in(&store, "sess-2").unwrap(),
            None,
            "no owns_reference edge ⇒ genuinely ungoverned, not an error"
        );

        let rendered =
            render_session_topology_in(&store, "sess-1").expect("governed session renders");
        assert!(rendered.contains("cron:steward"));
        assert!(rendered.contains("reference-proposal"));
        assert!(rendered.contains("根参照 root:aleph"));
        // Ungoverned session: byte-identical prompt (None).
        assert!(render_session_topology_in(&store, "sess-2").is_none());
        // Deterministic bytes: same graph ⇒ same render.
        assert_eq!(
            rendered,
            render_session_topology_in(&store, "sess-1").unwrap()
        );
    }

    #[test]
    fn debounce_collapses_bursts() {
        assert!(debounce_pass("job-x"));
        assert!(
            !debounce_pass("job-x"),
            "second poke within window must be dropped"
        );
        assert!(debounce_pass("job-y"), "distinct watcher unaffected");
    }

    #[test]
    fn failed_poke_rolls_back_its_debounce_stamp() {
        // A poke that never ran (run_job error) must not consume the window —
        // the next settle retries immediately instead of being suppressed.
        assert!(debounce_pass("job-rollback"));
        assert!(!debounce_pass("job-rollback"));
        debounce_rollback("job-rollback");
        assert!(
            debounce_pass("job-rollback"),
            "a failed poke must not consume the debounce window"
        );
        // Rolling back an unknown watcher is a no-op.
        debounce_rollback("job-never-passed");
    }
}
