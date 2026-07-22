//! Snapshot capture and restore orchestration.

use std::collections::{HashMap, VecDeque};

use super::{CreateSnapshotOutput, RestoreDiff, SqliteSnapshotStore, TeamSnapshotPayload};
use crate::agents::swarm::tasks::{
    CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskStore, NewCoordTask,
};
use crate::error::{AlephError, Result};
use crate::teams::types::{NewTeam, NewTeamMember, TeamMember};
use crate::teams::TeamStore;

/// Topologically sort `tasks` so every dep that also appears in `tasks` comes
/// before the dependent. Off-snapshot deps are ignored. Also returns the
/// count of in-payload dependency edges — these are the edges that the
/// restore will re-attach (or drop, if all live counterparts are already
/// covered by `tasks_to_skip_active`).
///
/// Errors on cycle — captured snapshots are always acyclic, but a corrupted
/// blob could be malformed. Defensive bail.
///
/// Returns indices into `tasks` (rather than references) so callers can
/// borrow `tasks` mutably-or-not freely.
pub(super) fn topo_sort_and_count_edges(tasks: &[CoordTask]) -> Result<(Vec<usize>, usize)> {
    let n = tasks.len();
    let mut id_to_idx: HashMap<String, usize> = HashMap::with_capacity(n);
    for (i, t) in tasks.iter().enumerate() {
        if id_to_idx.insert(t.id.clone(), i).is_some() {
            return Err(AlephError::ConfigError {
                message: format!("snapshot task graph has duplicate id: '{}'", t.id),
                suggestion: None,
            });
        }
    }
    let mut in_degree = vec![0usize; n];
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut edge_count = 0usize;
    for (i, t) in tasks.iter().enumerate() {
        for dep in &t.dependencies {
            if let Some(&j) = id_to_idx.get(dep.as_str()) {
                in_degree[i] += 1;
                children[j].push(i);
                edge_count += 1;
            }
        }
    }
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i);
        for &c in &children[i] {
            in_degree[c] -= 1;
            if in_degree[c] == 0 {
                queue.push_back(c);
            }
        }
    }
    if order.len() != n {
        return Err(AlephError::ConfigError {
            message: "snapshot task graph contains a cycle".into(),
            suggestion: None,
        });
    }
    Ok((order, edge_count))
}

/// Capture a snapshot of the team's current state. Reads from both stores
/// (teams.db for config+members, coord-tasks.db for the DAG).
pub async fn capture_snapshot(
    snapshot_store: &SqliteSnapshotStore,
    team_store: &dyn TeamStore,
    coord_store: &dyn CoordTaskStore,
    team_id: &str,
    tag: &str,
    note: &str,
) -> Result<CreateSnapshotOutput> {
    let team = team_store
        .get_team(team_id)
        .await?
        .ok_or_else(|| AlephError::NotFound(format!("team `{team_id}` not found")))?;
    let members = team_store.get_members(team_id).await?;
    let tasks = coord_store
        .list_tasks(CoordTaskFilter {
            team_id: Some(team_id.to_string()),
            ..Default::default()
        })
        .await?;
    let task_count = tasks.len();
    let member_count = members.len();

    let payload = TeamSnapshotPayload {
        team,
        members,
        tasks,
        note: note.to_string(),
    };
    let (id, created_at, size) = snapshot_store.insert(team_id, tag, &payload).await?;

    Ok(CreateSnapshotOutput {
        snapshot_id: id,
        team_id: team_id.to_string(),
        tag: tag.to_string(),
        created_at,
        task_count,
        member_count,
        size_bytes: size,
    })
}

/// Restore a snapshot.
///
/// `dry_run = true` (the default at the tool layer) computes a diff without
/// touching live state. `dry_run = false` performs:
/// - Members: add missing, remove extras (leader never removed).
/// - Tasks: create missing by subject+owner; skip `InProgress` live tasks
///   (don't yank work mid-flight).
/// - Team config and `created_at` are NOT overwritten — they are immutable
///   audit anchors.
///
/// Status of restored tasks is `Pending` (with `managed_by` carried over).
/// `result`, `started_at`, `completed_at` from the snapshot are NOT
/// re-applied — the restored task starts fresh in the live DAG.
pub async fn restore_snapshot(
    snapshot_store: &SqliteSnapshotStore,
    team_store: &dyn TeamStore,
    coord_store: &dyn CoordTaskStore,
    snapshot_id: &str,
    dry_run: bool,
) -> Result<RestoreDiff> {
    let (meta, payload) = snapshot_store
        .get(snapshot_id)
        .await?
        .ok_or_else(|| AlephError::NotFound(format!("snapshot `{snapshot_id}` not found")))?;
    let team_id = &meta.team_id;

    // --- Live state --------------------------------------------------------
    let live_team = team_store.get_team(team_id).await?;
    let live_members = if live_team.is_some() {
        team_store.get_members(team_id).await?
    } else {
        Vec::new()
    };
    // Like members: when the team itself is gone, tasks still rowed under the
    // stale team_id are orphans, not live state — matching against them would
    // classify every snapshot task as "already live" and skip recreation.
    let live_tasks = if live_team.is_some() {
        coord_store
            .list_tasks(CoordTaskFilter {
                team_id: Some(team_id.clone()),
                ..Default::default()
            })
            .await?
    } else {
        Vec::new()
    };
    let current_task_count = live_tasks.len();

    // --- Member diff -------------------------------------------------------
    let live_member_ids: HashMap<String, ()> = live_members
        .iter()
        .map(|m| (m.agent_id.clone(), ()))
        .collect();
    let snap_member_ids: HashMap<String, &TeamMember> = payload
        .members
        .iter()
        .map(|m| (m.agent_id.clone(), m))
        .collect();

    let members_to_add: Vec<String> = payload
        .members
        .iter()
        .filter(|m| !live_member_ids.contains_key(&m.agent_id))
        .map(|m| m.agent_id.clone())
        .collect();

    let leader_id = payload.team.leader_id.clone();
    let members_to_remove: Vec<String> = live_members
        .iter()
        .filter(|m| !snap_member_ids.contains_key(&m.agent_id))
        .filter(|m| m.agent_id != leader_id) // never remove leader
        .map(|m| m.agent_id.clone())
        .collect();

    // --- Task diff ---------------------------------------------------------
    // Live tasks keyed by (subject, owner) — that's the "logical identity"
    // we restore against, since snapshot ids won't match.
    let live_key = |t: &CoordTask| (t.subject.clone(), t.owner.clone());
    let live_by_key: HashMap<(String, Option<String>), &CoordTask> =
        live_tasks.iter().map(|t| (live_key(t), t)).collect();

    let mut tasks_to_add: Vec<String> = Vec::new();
    let mut tasks_to_update: Vec<String> = Vec::new();
    let mut tasks_to_skip_active: Vec<String> = Vec::new();

    for snap in &payload.tasks {
        let k = live_key(snap);
        match live_by_key.get(&k) {
            Some(live) if live.status == CoordTaskStatus::InProgress => {
                tasks_to_skip_active.push(snap.subject.clone());
            }
            Some(_) => {
                tasks_to_update.push(snap.subject.clone());
            }
            None => {
                tasks_to_add.push(snap.subject.clone());
            }
        }
    }

    // --- Dependency graph -------------------------------------------------
    // Topo-sort the snapshot tasks so dependencies are created before
    // dependents. Off-snapshot deps are ignored (their counterparts may have
    // been deleted in the meantime). `edges_to_restore` is the count of
    // in-payload edges — what the restore will re-attach.
    let (topo_order, edges_to_restore) = topo_sort_and_count_edges(&payload.tasks)?;

    let diff = RestoreDiff {
        dry_run,
        team_id: team_id.clone(),
        snapshot_id: snapshot_id.to_string(),
        current_task_count,
        snapshot_task_count: payload.tasks.len(),
        tasks_to_add: tasks_to_add.clone(),
        tasks_to_update: tasks_to_update.clone(),
        tasks_to_skip_active: tasks_to_skip_active.clone(),
        members_to_add: members_to_add.clone(),
        members_to_remove: members_to_remove.clone(),
        edges_restored: edges_to_restore,
    };

    if dry_run {
        return Ok(diff);
    }

    // --- Apply mutations ---------------------------------------------------
    // 1. Team config: recreate if missing. `create_team` mints a FRESH id
    //    (`NewTeam` carries none), so every write below must target the
    //    recreated id — writing against the snapshot's stale `meta.team_id`
    //    made the first `add_member` fail NotFound, aborting the whole restore
    //    (the exact scenario this branch exists for) and leaking one orphan
    //    shell team per retry.
    let effective_team_id = if live_team.is_none() {
        let recreated = team_store
            .create_team(NewTeam {
                name: payload.team.name.clone(),
                description: payload.team.description.clone(),
                leader_id: payload.team.leader_id.clone(),
            })
            .await?;
        // `NewTeam` carries no protocol field and `create_team` always sets it
        // to None, so the leader-authored operating protocol — captured in
        // `payload.team.protocol` and injected verbatim into every member's
        // launch context by the handoff builder — would be silently dropped on
        // a restore-after-delete (a partial-fidelity resurrection). Re-apply it
        // best-effort; a failed protocol write must not abort a restore that has
        // already recreated the team (mirrors the remove_member warn-&-continue).
        if payload.team.protocol.is_some() {
            if let Err(e) = team_store
                .set_protocol(&recreated.id, payload.team.protocol.clone())
                .await
            {
                tracing::warn!(
                    team_id = %recreated.id,
                    error = %e,
                    "snapshot restore: failed to re-apply captured team protocol"
                );
            }
        }
        recreated.id
    } else {
        team_id.clone()
    };

    // 2. Members. Preserve `kind` and ACP routing fields so external CLI
    // members survive snapshot/restore round-trips.
    for agent_id in &members_to_add {
        let snap_m = snap_member_ids.get(agent_id).ok_or_else(|| {
            AlephError::ConfigError {
                message: format!(
                    "snapshot restore: member {agent_id} in members_to_add but not in snap_member_ids"
                ),
                suggestion: None,
            }
        })?;
        team_store
            .add_member(NewTeamMember {
                team_id: effective_team_id.clone(),
                agent_id: agent_id.clone(),
                role: snap_m.role.clone(),
                kind: snap_m.kind.clone(),
                acp_harness_id: snap_m.acp_harness_id.clone(),
                acp_cwd: snap_m.acp_cwd.clone(),
                acp_session_name: snap_m.acp_session_name.clone(),
            })
            .await?;
    }
    for agent_id in &members_to_remove {
        // remove_member errors if the agent is the leader — we already
        // filtered that out, but be defensive: log & continue.
        if let Err(e) = team_store.remove_member(&effective_team_id, agent_id).await {
            tracing::warn!(
                team_id = %effective_team_id,
                agent_id = %agent_id,
                error = %e,
                "snapshot restore: remove_member failed; skipping"
            );
        }
    }

    // 3. Tasks: create the additions in dependency order, remapping snapshot
    //    `blocked_by` ids to whichever live id ended up backing each snapshot
    //    task — either a freshly-created one or an already-live counterpart
    //    matched by (subject, owner). Updates and active-skips are recorded
    //    in the diff but their config is NOT mutated — restoring would
    //    clobber live work.
    //
    //    Edges whose snapshot dep target is missing from the payload (e.g.
    //    the dep was deleted before snapshot or pruned) are silently
    //    dropped. The diff's `edges_restored` counts only the edges that the
    //    restore will (or did) actually re-attach.
    let mut id_map: HashMap<String, String> = HashMap::new();
    for snap in &payload.tasks {
        let k = live_key(snap);
        if let Some(live) = live_by_key.get(&k) {
            id_map.insert(snap.id.clone(), live.id.clone());
        }
    }

    for idx in topo_order {
        let snap = &payload.tasks[idx];
        if id_map.contains_key(&snap.id) {
            continue; // already live; we don't mutate live tasks
        }
        let mut blocked_by: Vec<String> = Vec::with_capacity(snap.dependencies.len());
        for dep in &snap.dependencies {
            if let Some(live_id) = id_map.get(dep.as_str()) {
                blocked_by.push(live_id.clone());
            }
            // else: dep target wasn't in payload — drop the edge silently.
        }
        let created = coord_store
            .create_task(NewCoordTask {
                team_id: Some(effective_team_id.clone()),
                subject: snap.subject.clone(),
                description: snap.description.clone(),
                owner: snap.owner.clone(),
                priority: snap.priority,
                blocked_by,
                metadata: snap.metadata.clone(),
            })
            .await?;
        id_map.insert(snap.id.clone(), created.id);
    }

    // Point the caller at the team the restore actually landed on (differs
    // from the snapshot's team_id only in the recreate-after-delete case).
    let mut diff = diff;
    diff.team_id = effective_team_id;
    Ok(diff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::swarm::tasks::Priority;

    fn task(id: &str, deps: &[&str]) -> CoordTask {
        CoordTask {
            id: id.to_string(),
            team_id: Some("team-1".into()),
            subject: id.into(),
            description: String::new(),
            status: CoordTaskStatus::Pending,
            owner: None,
            priority: Priority::Normal,
            result: None,
            metadata: serde_json::json!({}),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            created_at: 0,
            started_at: None,
            completed_at: None,
            locked_by: None,
            locked_at: None,
        }
    }

    #[test]
    fn topo_sort_rejects_duplicate_task_ids() {
        let tasks = vec![task("dup", &[]), task("dup", &[])];
        let err = topo_sort_and_count_edges(&tasks).unwrap_err();
        assert!(format!("{err:?}").contains("duplicate"));
    }

    #[test]
    fn topo_sort_accepts_unique_ids_with_cycle() {
        let tasks = vec![task("a", &["b"]), task("b", &["a"])];
        let err = topo_sort_and_count_edges(&tasks).unwrap_err();
        assert!(format!("{err:?}").contains("cycle"));
    }

    #[test]
    fn topo_sort_accepts_unique_acyclic_graph() {
        let tasks = vec![task("a", &[]), task("b", &["a"]), task("c", &["a", "b"])];
        let (order, edges) = topo_sort_and_count_edges(&tasks).unwrap();
        assert_eq!(order.len(), 3);
        assert_eq!(edges, 3);
    }
}
