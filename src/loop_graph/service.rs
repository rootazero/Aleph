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
use crate::tasks::cron::SharedCronService;

/// v1 scopes the graph to the default agent; multi-agent scoping is an
/// explicit open question in the design (§11.9) awaiting a real consumer.
const DEFAULT_AGENT: &str = "main";

/// Minimum interval between pokes of the SAME watcher — structural rate
/// limit (not a judgment): a burst of goal settlements collapses into one
/// watcher run.
const WATCH_DEBOUNCE: Duration = Duration::from_secs(60);

static CRON_TRIGGER: OnceCell<SharedCronService> = OnceCell::new();
static DEBOUNCE: OnceCell<std::sync::Mutex<HashMap<String, Instant>>> = OnceCell::new();

/// Install the cron handle that powers watcher pokes. Called once at boot
/// next to `loop_graph::init_global`; absent (None / never called) the
/// trigger degrades to a no-op and watchers rely on their own cadence.
pub fn init_cron_trigger(svc: Option<SharedCronService>) {
    if let Some(s) = svc {
        let _ = CRON_TRIGGER.set(s);
    }
}

fn debounce_pass(watcher_job_id: &str) -> bool {
    let map = DEBOUNCE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
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

/// Poke every cron watcher paired (via `watches`) to `goal:<session>`.
/// Call sites: the goal continuation hook's victory-claim moments
/// (gate-less terminal complete, and gate-pass commit). Best-effort and
/// bounded: no graph / no store / no cron handle / no watchers → no-op.
pub async fn notify_goal_settled(session: &str) {
    let Some(store) = crate::loop_graph::global() else {
        return;
    };
    let node_id = format!("goal:{session}");
    let Ok(edges) = store.list_edges(DEFAULT_AGENT) else {
        return;
    };
    let watcher_jobs: Vec<String> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Watches && e.to_id == node_id)
        .filter_map(|e| e.from_id.strip_prefix("cron:").map(str::to_string))
        .collect();
    if watcher_jobs.is_empty() {
        return;
    }
    let Some(cron) = CRON_TRIGGER.get() else {
        info!(session = %session, "loop_graph: watchers paired but no cron trigger handle");
        return;
    };
    for job_id in watcher_jobs {
        if !debounce_pass(&job_id) {
            continue;
        }
        let service = cron.lock().await;
        match service.run_job(&job_id).await {
            Ok(()) => {
                info!(session = %session, watcher = %job_id,
                    "loop_graph: victory claim — watcher cron poked");
            }
            Err(e) => {
                warn!(session = %session, watcher = %job_id, error = %e,
                    "loop_graph: failed to poke watcher cron");
            }
        }
    }
}

/// The id of the loop owning `goal:<session>`'s reference via an
/// `owns_reference` edge, if any. Pure lookup for the objective ACL.
#[must_use]
pub fn governing_owner(session: &str) -> Option<String> {
    let store = crate::loop_graph::global()?;
    governing_owner_in(&store, session)
}

/// Store-taking form of [`governing_owner`] (unit-testable without the
/// process global).
#[must_use]
pub fn governing_owner_in(
    store: &crate::loop_graph::LoopGraphStore,
    session: &str,
) -> Option<String> {
    let node_id = format!("goal:{session}");
    store
        .list_edges(DEFAULT_AGENT)
        .ok()?
        .into_iter()
        .find(|e| e.kind == EdgeKind::OwnsReference && e.to_id == node_id)
        .map(|e| e.from_id)
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
pub fn render_session_topology_in(
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
    fn governing_owner_and_topology_render() {
        let (_dir, store) = seeded_store();

        assert_eq!(
            governing_owner_in(&store, "sess-1").as_deref(),
            Some("cron:steward"),
            "owns_reference edge must surface the owner"
        );
        assert_eq!(governing_owner_in(&store, "sess-2"), None);

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
}
