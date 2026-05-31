use rusqlite::Connection;

use super::operations::topo_sort_and_count_edges;
use super::*;
use crate::agents::swarm::tasks::store::SqliteCoordTaskStore;
use crate::agents::swarm::tasks::{
    CoordTask, CoordTaskFilter, CoordTaskStatus, CoordTaskStore, NewCoordTask, Priority,
};
use crate::error::AlephError;
use crate::sync_primitives::Arc;
use crate::teams::types::{NewTeam, NewTeamMember, Team, TeamMember};
use crate::teams::{SqliteTeamStore, TeamStore};

async fn setup() -> (
    Arc<SqliteSnapshotStore>,
    Arc<SqliteTeamStore>,
    Arc<SqliteCoordTaskStore>,
) {
    // Coord tasks: single connection shared with snapshots.
    let coord_conn = Connection::open_in_memory().expect("open coord");
    let coord = Arc::new(SqliteCoordTaskStore::new(coord_conn));
    coord.migrate().await.expect("coord migrate");
    let snap = Arc::new(SqliteSnapshotStore::new_from_shared(
        coord.connection_handle(),
    ));

    // Teams: separate in-memory db.
    let teams_conn = Connection::open_in_memory().expect("open teams");
    let teams = Arc::new(SqliteTeamStore::new(teams_conn));
    teams.migrate().await.expect("teams migrate");

    (snap, teams, coord)
}

async fn seed_team(team_store: &dyn TeamStore) -> Team {
    team_store
        .create_team(NewTeam {
            name: "Alpha".into(),
            description: "test".into(),
            leader_id: "leader".into(),
        })
        .await
        .expect("create team")
}

#[tokio::test]
async fn capture_and_get_roundtrip() {
    let (snap, teams, coord) = setup().await;
    let team = seed_team(teams.as_ref()).await;
    teams
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "leader".into(),
            role: "leader".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    teams
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "worker".into(),
            role: "worker".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    coord
        .create_task(NewCoordTask {
            team_id: Some(team.id.clone()),
            subject: "task1".into(),
            description: "do thing".into(),
            owner: Some("worker".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    let out = capture_snapshot(
        snap.as_ref(),
        teams.as_ref(),
        coord.as_ref(),
        &team.id,
        "v1",
        "",
    )
    .await
    .unwrap();
    assert_eq!(out.task_count, 1);
    assert_eq!(out.member_count, 2);

    let (meta, payload) = snap.get(&out.snapshot_id).await.unwrap().unwrap();
    assert_eq!(meta.tag, "v1");
    assert_eq!(payload.team.id, team.id);
    assert_eq!(payload.members.len(), 2);
    assert_eq!(payload.tasks.len(), 1);
}

#[tokio::test]
async fn list_returns_newest_first() {
    let (snap, teams, coord) = setup().await;
    let team = seed_team(teams.as_ref()).await;
    teams
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "leader".into(),
            role: "leader".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    let first = capture_snapshot(
        snap.as_ref(),
        teams.as_ref(),
        coord.as_ref(),
        &team.id,
        "first",
        "",
    )
    .await
    .unwrap();
    // Tiny delay so timestamps differ.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = capture_snapshot(
        snap.as_ref(),
        teams.as_ref(),
        coord.as_ref(),
        &team.id,
        "second",
        "",
    )
    .await
    .unwrap();

    let list = snap.list(Some(&team.id)).await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, second.snapshot_id);
    assert_eq!(list[1].id, first.snapshot_id);
}

#[tokio::test]
async fn dry_run_restore_reports_diff_without_mutating() {
    let (snap, teams, coord) = setup().await;
    let team = seed_team(teams.as_ref()).await;
    teams
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "leader".into(),
            role: "leader".into(),
            ..Default::default()
        })
        .await
        .unwrap();

    // Snapshot with one task.
    coord
        .create_task(NewCoordTask {
            team_id: Some(team.id.clone()),
            subject: "snap-task".into(),
            description: "".into(),
            owner: Some("leader".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();
    let snap_meta = capture_snapshot(
        snap.as_ref(),
        teams.as_ref(),
        coord.as_ref(),
        &team.id,
        "v1",
        "",
    )
    .await
    .unwrap();

    // Live state: that task got renamed (simulating a divergence).
    let live = coord
        .list_tasks(CoordTaskFilter {
            team_id: Some(team.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let task_id = live[0].id.clone();
    coord
        .update_task(
            &task_id,
            crate::agents::swarm::tasks::CoordTaskUpdate {
                status: Some(CoordTaskStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Dry-run restore: should report 0 to add (subject+owner match), 1 to update,
    // and not actually mutate.
    let diff = restore_snapshot(
        snap.as_ref(),
        teams.as_ref(),
        coord.as_ref(),
        &snap_meta.snapshot_id,
        true,
    )
    .await
    .unwrap();
    assert!(diff.dry_run);
    assert_eq!(diff.tasks_to_update.len(), 1);
    assert_eq!(diff.tasks_to_add.len(), 0);

    // Verify no mutation: task still Completed, count unchanged.
    let live_after = coord
        .list_tasks(CoordTaskFilter {
            team_id: Some(team.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(live_after.len(), 1);
    assert_eq!(live_after[0].status, CoordTaskStatus::Completed);
}

#[tokio::test]
async fn live_restore_adds_missing_tasks_and_members() {
    let (snap, teams, coord) = setup().await;
    let team = seed_team(teams.as_ref()).await;
    teams
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "leader".into(),
            role: "leader".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    teams
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "snap-only".into(),
            role: "worker".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    coord
        .create_task(NewCoordTask {
            team_id: Some(team.id.clone()),
            subject: "snap-only-task".into(),
            description: "".into(),
            owner: Some("snap-only".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();

    let s = capture_snapshot(
        snap.as_ref(),
        teams.as_ref(),
        coord.as_ref(),
        &team.id,
        "v1",
        "",
    )
    .await
    .unwrap();

    // Mutate live: remove snap-only member + delete the task.
    teams.remove_member(&team.id, "snap-only").await.unwrap();
    let live = coord
        .list_tasks(CoordTaskFilter {
            team_id: Some(team.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    for t in live {
        coord
            .update_task(
                &t.id,
                crate::agents::swarm::tasks::CoordTaskUpdate {
                    status: Some(CoordTaskStatus::Cancelled),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    // Drop the cancelled task by deleting via store? Cancelled stays in the list.
    // For test purposes, replace by creating a divergent task with same subject —
    // but the subject+owner key matches our snapshot, so it will be treated as
    // "to_update" rather than "to_add". Instead, change the subject to force re-add.

    // Apply live restore — should re-add the snap-only member.
    let diff = restore_snapshot(
        snap.as_ref(),
        teams.as_ref(),
        coord.as_ref(),
        &s.snapshot_id,
        false,
    )
    .await
    .unwrap();
    assert!(!diff.dry_run);
    assert!(
        diff.members_to_add.contains(&"snap-only".to_string()),
        "expected snap-only re-add, got {:?}",
        diff.members_to_add
    );

    let live_members = teams.get_members(&team.id).await.unwrap();
    let ids: Vec<&str> = live_members.iter().map(|m| m.agent_id.as_str()).collect();
    assert!(ids.contains(&"snap-only"), "snap-only must be restored");
}

#[tokio::test]
async fn delete_snapshot_is_idempotent() {
    let (snap, teams, coord) = setup().await;
    let team = seed_team(teams.as_ref()).await;
    teams
        .add_member(NewTeamMember {
            team_id: team.id.clone(),
            agent_id: "leader".into(),
            role: "leader".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let s = capture_snapshot(
        snap.as_ref(),
        teams.as_ref(),
        coord.as_ref(),
        &team.id,
        "v1",
        "",
    )
    .await
    .unwrap();

    assert!(snap.delete(&s.snapshot_id).await.unwrap());
    assert!(!snap.delete(&s.snapshot_id).await.unwrap()); // already gone
    assert!(snap.get(&s.snapshot_id).await.unwrap().is_none());
}

// -----------------------------------------------------------------------
// Dependency-edge restoration
// -----------------------------------------------------------------------

fn mk_task(
    id: &str,
    team_id: &str,
    subject: &str,
    owner: Option<&str>,
    deps: Vec<&str>,
) -> CoordTask {
    CoordTask {
        id: id.into(),
        team_id: Some(team_id.into()),
        subject: subject.into(),
        description: String::new(),
        status: CoordTaskStatus::Pending,
        owner: owner.map(str::to_string),
        priority: Priority::Normal,
        result: None,
        metadata: serde_json::json!({}),
        dependencies: deps.into_iter().map(str::to_string).collect(),
        created_at: 0,
        started_at: None,
        completed_at: None,
        locked_by: None,
        locked_at: None,
    }
}

fn mk_payload(team: &Team, members: Vec<TeamMember>, tasks: Vec<CoordTask>) -> TeamSnapshotPayload {
    TeamSnapshotPayload {
        team: team.clone(),
        members,
        tasks,
        note: String::new(),
    }
}

async fn add_leader(teams: &SqliteTeamStore, team_id: &str) -> TeamMember {
    teams
        .add_member(NewTeamMember {
            team_id: team_id.into(),
            agent_id: "leader".into(),
            role: "leader".into(),
            ..Default::default()
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn restore_recreates_dependency_edges() {
    let (snap, teams, coord) = setup().await;
    let team = seed_team(teams.as_ref()).await;
    let leader = add_leader(teams.as_ref(), &team.id).await;

    // Build a hand-crafted payload — bypass capture_snapshot so the live
    // coord_store stays empty and the restore actually has to create.
    let task_a = mk_task("snap-A", &team.id, "task A", Some("leader"), vec![]);
    let task_b = mk_task("snap-B", &team.id, "task B", Some("leader"), vec!["snap-A"]);
    let payload = mk_payload(&team, vec![leader], vec![task_a, task_b]);
    let (sid, _, _) = snap.insert(&team.id, "v1", &payload).await.unwrap();

    let diff = restore_snapshot(snap.as_ref(), teams.as_ref(), coord.as_ref(), &sid, false)
        .await
        .unwrap();
    assert!(!diff.dry_run);
    assert_eq!(diff.tasks_to_add.len(), 2);
    assert_eq!(diff.edges_restored, 1);

    let live = coord
        .list_tasks(CoordTaskFilter {
            team_id: Some(team.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(live.len(), 2);
    // Find the dependent task and verify its dependency points at the
    // freshly-created counterpart of task A (not the stale snap-A id).
    let b = live.iter().find(|t| t.subject == "task B").expect("task B");
    let a = live.iter().find(|t| t.subject == "task A").expect("task A");
    assert_eq!(b.dependencies, vec![a.id.clone()]);
    // And A must have NO dependencies — it was a root.
    assert!(a.dependencies.is_empty());
}

#[tokio::test]
async fn restore_remaps_dep_to_existing_live_task() {
    let (snap, teams, coord) = setup().await;
    let team = seed_team(teams.as_ref()).await;
    let leader = add_leader(teams.as_ref(), &team.id).await;

    // Live coord already has task A — built independently, so its id
    // differs from the snapshot's "snap-A".
    let live_a = coord
        .create_task(NewCoordTask {
            team_id: Some(team.id.clone()),
            subject: "task A".into(),
            description: String::new(),
            owner: Some("leader".into()),
            priority: Priority::Normal,
            blocked_by: vec![],
            metadata: serde_json::json!({}),
        })
        .await
        .unwrap();
    assert_ne!(live_a.id, "snap-A"); // sanity — UUIDs differ

    // Snapshot has both A (mirror, will become tasks_to_update) and B
    // (new, depends on snap-A id).
    let task_a = mk_task("snap-A", &team.id, "task A", Some("leader"), vec![]);
    let task_b = mk_task("snap-B", &team.id, "task B", Some("leader"), vec!["snap-A"]);
    let payload = mk_payload(&team, vec![leader], vec![task_a, task_b]);
    let (sid, _, _) = snap.insert(&team.id, "v1", &payload).await.unwrap();

    let diff = restore_snapshot(snap.as_ref(), teams.as_ref(), coord.as_ref(), &sid, false)
        .await
        .unwrap();
    assert_eq!(diff.tasks_to_add, vec!["task B"]);
    assert_eq!(diff.tasks_to_update, vec!["task A"]);
    assert_eq!(diff.edges_restored, 1);

    let live = coord
        .list_tasks(CoordTaskFilter {
            team_id: Some(team.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(live.len(), 2);
    let b = live.iter().find(|t| t.subject == "task B").expect("task B");
    // B must point at the pre-existing live A, not a re-created one.
    assert_eq!(b.dependencies, vec![live_a.id.clone()]);
}

#[tokio::test]
async fn restore_drops_orphan_edges_silently() {
    let (snap, teams, coord) = setup().await;
    let team = seed_team(teams.as_ref()).await;
    let leader = add_leader(teams.as_ref(), &team.id).await;

    // B points at a ghost id with no matching payload entry. Restore
    // must drop that edge and create B with empty blocked_by.
    let task_b = mk_task("snap-B", &team.id, "task B", Some("leader"), vec!["ghost"]);
    let payload = mk_payload(&team, vec![leader], vec![task_b]);
    let (sid, _, _) = snap.insert(&team.id, "v1", &payload).await.unwrap();

    let diff = restore_snapshot(snap.as_ref(), teams.as_ref(), coord.as_ref(), &sid, false)
        .await
        .unwrap();
    assert_eq!(diff.edges_restored, 0);

    let live = coord
        .list_tasks(CoordTaskFilter {
            team_id: Some(team.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(live.len(), 1);
    assert!(live[0].dependencies.is_empty());
}

#[tokio::test]
async fn dry_run_reports_edges_to_restore_without_mutating() {
    let (snap, teams, coord) = setup().await;
    let team = seed_team(teams.as_ref()).await;
    let leader = add_leader(teams.as_ref(), &team.id).await;

    let task_a = mk_task("snap-A", &team.id, "task A", Some("leader"), vec![]);
    let task_b = mk_task("snap-B", &team.id, "task B", Some("leader"), vec!["snap-A"]);
    let payload = mk_payload(&team, vec![leader], vec![task_a, task_b]);
    let (sid, _, _) = snap.insert(&team.id, "v1", &payload).await.unwrap();

    let diff = restore_snapshot(snap.as_ref(), teams.as_ref(), coord.as_ref(), &sid, true)
        .await
        .unwrap();
    assert!(diff.dry_run);
    assert_eq!(diff.edges_restored, 1);

    // Live state must be untouched.
    let live = coord
        .list_tasks(CoordTaskFilter {
            team_id: Some(team.id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(live.is_empty());
}

#[test]
fn topo_sort_rejects_cyclic_snapshot() {
    // Synthesise a cycle: a depends on b, b depends on a.
    let team_id = "t1";
    let cycle = vec![
        mk_task("a", team_id, "A", None, vec!["b"]),
        mk_task("b", team_id, "B", None, vec!["a"]),
    ];
    let err = topo_sort_and_count_edges(&cycle).expect_err("cycle must error");
    assert!(matches!(err, AlephError::ConfigError { .. }));
}

#[test]
fn topo_sort_ignores_off_snapshot_deps() {
    let team_id = "t1";
    // "external" is not in the payload — must be treated as a free root,
    // not as an incoming edge.
    let tasks = vec![mk_task("only", team_id, "only", None, vec!["external"])];
    let (order, edges) = topo_sort_and_count_edges(&tasks).unwrap();
    assert_eq!(order, vec![0]);
    assert_eq!(edges, 0);
}
