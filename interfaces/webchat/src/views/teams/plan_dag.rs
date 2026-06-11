//! PlanDagView — read-only DAG visualization of a team's CoordTask
//! dependencies. Sibling sub-tab of Kanban / Workers / Overview, shares
//! `TeamsTabState::selected_team_id`.
//!
//! MVP scope: layered SVG layout (BFS by `dependencies` depth), node
//! click opens the same `TaskDetailDrawer` Kanban uses. No drag, no
//! zoom, no inline edit — those land later if the layout proves out.

use std::collections::HashMap;

use leptos::prelude::*;
use leptos::task::spawn_local;

use super::components::task_drawer::TaskDetailDrawer;
use super::TeamsTabState;
use crate::api::teams::{CoordTaskDto, TaskFilter, TeamsApi};
use crate::context::DashboardState;

/// Width of a task node card.
const NODE_W: f32 = 200.0;
/// Height of a task node card.
const NODE_H: f32 = 56.0;
/// Horizontal spacing between sibling nodes in the same layer.
const COL_GAP: f32 = 36.0;
/// Vertical spacing between layers.
const LAYER_GAP: f32 = 64.0;
/// Outer padding around the entire DAG.
const PAD: f32 = 32.0;

#[component]
#[must_use]
pub fn PlanDagView() -> impl IntoView {
    let dash = expect_context::<DashboardState>();
    let state = expect_context::<TeamsTabState>();

    let tasks: RwSignal<Vec<CoordTaskDto>> = RwSignal::new(Vec::new());
    let drawer: RwSignal<Option<CoordTaskDto>> = RwSignal::new(None);

    // Fetch tasks for the currently-selected team. Mirrors kanban.rs:27.
    let refresh = move || {
        let Some(team_id) = state.selected_team_id.get_untracked() else {
            tasks.set(Vec::new());
            return;
        };
        spawn_local(async move {
            if let Ok(list) = TeamsApi::list_tasks(&dash, &team_id, TaskFilter::default()).await {
                tasks.set(list);
            }
        });
    };

    Effect::new(move |_| {
        let _ = state.selected_team_id.get();
        refresh();
    });

    // Live refresh on team.{id}.task.* events (kanban-parity).
    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let dash2 = dash;
        spawn_local(async move {
            let _ = dash2.subscribe_topic("team.*.task.*").await;
        });
    });
    let sub_id = dash.subscribe_events(move |evt| {
        let Some(active) = state.selected_team_id.get_untracked() else {
            return;
        };
        let topic = evt.topic.as_str();
        if topic.starts_with("team.") && topic.contains(".task.") {
            let parts: Vec<&str> = topic.splitn(4, '.').collect();
            if parts.len() >= 3 && parts[1] == active {
                refresh();
            }
        }
    });
    on_cleanup(move || dash.unsubscribe_events(sub_id));

    // On drawer mutation (status change, comment add), the kanban path
    // refreshes via topic events anyway; trigger a manual refresh too so
    // there's no perceived lag.
    let on_changed = Callback::new(move |_| refresh());

    view! {
        <div class="flex-1 flex flex-col h-full overflow-hidden aleph-content-top">
            <div class="flex-1 overflow-auto bg-surface">
                {move || {
                    let list = tasks.get();
                    if list.is_empty() {
                        view! {
                            <div class="flex items-center justify-center h-full text-text-tertiary">
                                <div class="text-sm">"No tasks yet for this team."</div>
                            </div>
                        }.into_any()
                    } else {
                        render_dag(list, drawer).into_any()
                    }
                }}
            </div>
            <TaskDetailDrawer open_for=drawer on_changed=on_changed />
        </div>
    }
}

/// Compute one BFS-style depth per task. Iterative pass converges in
/// O(V) on a DAG; an unresolved cycle would diverge — we guard with a
/// per-pass change counter and bail after N == tasks.len() passes,
/// which is the maximum acyclic depth.
fn compute_depths(tasks: &[CoordTaskDto]) -> HashMap<String, usize> {
    let mut depths: HashMap<String, usize> = HashMap::new();
    let n = tasks.len();
    for _ in 0..=n {
        let mut changed = false;
        for t in tasks {
            let new_depth = if t.dependencies.is_empty() {
                0
            } else {
                let max_dep = t
                    .dependencies
                    .iter()
                    .filter_map(|d| depths.get(d).copied())
                    .max();
                match max_dep {
                    Some(d) => d + 1,
                    None => 0, // deps not yet (or never) resolved — treat as root for now
                }
            };
            if depths.get(&t.id).copied() != Some(new_depth) {
                depths.insert(t.id.clone(), new_depth);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    depths
}

/// Map task status to a Tailwind hex colour. Matches kanban's column accents.
fn status_fill(status: &str) -> &'static str {
    match status {
        "pending" => "#9ca3af",       // gray-400
        "blocked" => "#f97316",       // orange-500
        "in_progress" => "#3b82f6",   // blue-500
        "completed" => "#10b981",     // emerald-500
        "failed" => "#ef4444",        // red-500
        "unsatisfiable" => "#ef4444", // red-500 — a dependency terminally failed
        "cancelled" => "#6b7280",     // gray-500
        _ => "#9ca3af",
    }
}

fn render_dag(tasks: Vec<CoordTaskDto>, drawer: RwSignal<Option<CoordTaskDto>>) -> impl IntoView {
    let depths = compute_depths(&tasks);

    // Group tasks by depth, preserving created-at order within each layer.
    let mut layers: Vec<Vec<CoordTaskDto>> = Vec::new();
    let mut sorted = tasks.clone();
    sorted.sort_by_key(|t| (depths.get(&t.id).copied().unwrap_or(0), t.created_at));
    for t in sorted {
        let d = depths.get(&t.id).copied().unwrap_or(0);
        while layers.len() <= d {
            layers.push(Vec::new());
        }
        layers[d].push(t);
    }

    // Assign positions: each layer is a horizontal row; within a layer,
    // nodes stack left-to-right.
    let mut positions: HashMap<String, (f32, f32)> = HashMap::new();
    let mut max_w: f32 = 0.0;
    for (layer_idx, layer) in layers.iter().enumerate() {
        let row_w = layer.len() as f32 * NODE_W + (layer.len().saturating_sub(1)) as f32 * COL_GAP;
        max_w = max_w.max(row_w);
        for (col_idx, task) in layer.iter().enumerate() {
            let x = PAD + col_idx as f32 * (NODE_W + COL_GAP);
            let y = PAD + layer_idx as f32 * (NODE_H + LAYER_GAP);
            positions.insert(task.id.clone(), (x, y));
        }
    }
    let total_w = max_w + 2.0 * PAD;
    let total_h = layers.len() as f32 * NODE_H
        + (layers.len().saturating_sub(1)) as f32 * LAYER_GAP
        + 2.0 * PAD;

    // Pre-compute edge segments (dep_id -> task) so they render under nodes.
    let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();
    for t in &tasks {
        let Some(&(tx, ty)) = positions.get(&t.id) else {
            continue;
        };
        for dep_id in &t.dependencies {
            if let Some(&(dx, dy)) = positions.get(dep_id) {
                // From bottom-centre of dep to top-centre of task.
                let x1 = dx + NODE_W / 2.0;
                let y1 = dy + NODE_H;
                let x2 = tx + NODE_W / 2.0;
                let y2 = ty;
                edges.push((x1, y1, x2, y2));
            }
        }
    }

    view! {
        <svg
            xmlns="http://www.w3.org/2000/svg"
            width=total_w
            height=total_h
            viewBox=format!("0 0 {} {}", total_w, total_h)
            class="block"
        >
            // Edges first — they sit under nodes.
            <g class="edges" stroke="#94a3b8" stroke-width="1.5" fill="none">
                {edges.into_iter().map(|(x1, y1, x2, y2)| {
                    // Smooth cubic Bezier between layers: control points
                    // pull down/up to give a gentle S-curve.
                    let mid_y = (y1 + y2) / 2.0;
                    let path = format!("M {x1} {y1} C {x1} {mid_y}, {x2} {mid_y}, {x2} {y2}");
                    view! {
                        <path d=path />
                    }
                }).collect_view()}
            </g>
            // Nodes on top.
            <g class="nodes">
                {tasks.into_iter().map(|t| {
                    let (x, y) = positions.get(&t.id).copied().unwrap_or((0.0, 0.0));
                    let fill = status_fill(&t.status);
                    let task_for_click = t.clone();
                    let subject = t.subject.clone();
                    let status = t.status.clone();

                    view! {
                        <g
                            class="cursor-pointer"
                            on:click=move |_| drawer.set(Some(task_for_click.clone()))
                        >
                            <rect
                                x=x
                                y=y
                                width=NODE_W
                                height=NODE_H
                                rx="8"
                                ry="8"
                                fill="#ffffff"
                                stroke=fill
                                stroke-width="2"
                            />
                            // Status pill on the left edge.
                            <rect
                                x=x
                                y=y
                                width="6"
                                height=NODE_H
                                fill=fill
                            />
                            <text
                                x=x + 16.0
                                y=y + 22.0
                                font-size="13"
                                font-weight="600"
                                fill="#0f172a"
                            >
                                // SVG <text> doesn't word-wrap; truncate to ~22 chars to
                                // avoid overflow past the right edge of the card.
                                {if subject.chars().count() > 22 {
                                    let mut s: String = subject.chars().take(22).collect();
                                    s.push('…');
                                    s
                                } else {
                                    subject
                                }}
                            </text>
                            <text
                                x=x + 16.0
                                y=y + 42.0
                                font-size="11"
                                fill="#64748b"
                            >
                                {status}
                            </text>
                        </g>
                    }
                }).collect_view()}
            </g>
        </svg>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, deps: &[&str]) -> CoordTaskDto {
        CoordTaskDto {
            id: id.into(),
            team_id: None,
            subject: id.into(),
            description: String::new(),
            status: "pending".into(),
            owner: None,
            priority: "normal".into(),
            result: None,
            dependencies: deps.iter().map(|s| (*s).into()).collect(),
            created_at: 0,
            started_at: None,
            completed_at: None,
        }
    }

    #[test]
    fn empty_input_returns_empty() {
        let depths = compute_depths(&[]);
        assert!(depths.is_empty());
    }

    #[test]
    fn single_root_task_has_depth_zero() {
        let tasks = vec![task("a", &[])];
        let depths = compute_depths(&tasks);
        assert_eq!(depths.get("a"), Some(&0));
    }

    #[test]
    fn linear_chain_increments_depth() {
        // a → b → c
        let tasks = vec![task("a", &[]), task("b", &["a"]), task("c", &["b"])];
        let depths = compute_depths(&tasks);
        assert_eq!(depths.get("a"), Some(&0));
        assert_eq!(depths.get("b"), Some(&1));
        assert_eq!(depths.get("c"), Some(&2));
    }

    #[test]
    fn diamond_takes_max_dep_depth() {
        // a → {b, c} → d
        let tasks = vec![
            task("a", &[]),
            task("b", &["a"]),
            task("c", &["a"]),
            task("d", &["b", "c"]),
        ];
        let depths = compute_depths(&tasks);
        assert_eq!(depths.get("a"), Some(&0));
        assert_eq!(depths.get("b"), Some(&1));
        assert_eq!(depths.get("c"), Some(&1));
        assert_eq!(depths.get("d"), Some(&2));
    }

    #[test]
    fn unresolved_dep_treated_as_root() {
        // b's dep "ghost" not in the task list — render as root rather than drop.
        let tasks = vec![task("b", &["ghost"])];
        let depths = compute_depths(&tasks);
        assert_eq!(depths.get("b"), Some(&0));
    }

    #[test]
    fn order_independent_for_acyclic_input() {
        // Same DAG, declared in reverse order — depths must match the
        // forward-order test above (the iterative pass must converge
        // regardless of source ordering).
        let tasks = vec![task("c", &["b"]), task("b", &["a"]), task("a", &[])];
        let depths = compute_depths(&tasks);
        assert_eq!(depths.get("a"), Some(&0));
        assert_eq!(depths.get("b"), Some(&1));
        assert_eq!(depths.get("c"), Some(&2));
    }

    #[test]
    fn cycle_terminates_with_bounded_depths() {
        // a ↔ b: an unresolvable cycle. compute_depths must still
        // terminate (the n+1 outer-loop cap is the guard) and produce
        // finite depths for every node.
        let tasks = vec![task("a", &["b"]), task("b", &["a"])];
        let depths = compute_depths(&tasks);
        assert!(depths.contains_key("a"));
        assert!(depths.contains_key("b"));
        // n+1 passes × +1 increment per pass ⇒ ceiling well under 16.
        assert!(depths.values().all(|&d| d < 16));
    }
}
