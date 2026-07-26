//! Host-testable pure logic for the team task strip/drawer: status → color
//! mapping over the raw snake_case `CoordTaskStatus` wire strings, "most salient"
//! task selection, and the overflow count. No Leptos signals / DOM here.
//!
//! Status **labels** deliberately do not live here: the kanban already owns a
//! localized `CoordTaskStatus` → text table
//! ([`crate::views::teams::components::board_columns::column_label`]), and the
//! strip/drawer call it. This module used to carry a second, hardcoded-Chinese
//! copy of the same 10 variants — two sources that could (and did) disagree,
//! and one of which no locale switch could reach.

use crate::api::teams::CoordTaskDto;

/// Status dot color for a task (CSS hex), reusing the member palette family.
#[must_use]
pub fn task_status_color(status: &str) -> &'static str {
    match status {
        "waiting_review" => "#c586c0",           // purple — needs attention
        "in_progress" => "#e0a458",              // amber — active
        "completed" | "skipped" => "#4ec9b0",    // teal — done
        "failed" | "unsatisfiable" => "#d16969", // red — bad terminal
        _ => "#6b7280",                          // grey — pending/blocked/paused/unknown
    }
}

/// Lower rank = more salient. WaitingReview > InProgress > other non-terminal >
/// terminal. (Spec §3.2.)
fn salience_rank(status: &str) -> u8 {
    match status {
        "waiting_review" => 0,
        "in_progress" => 1,
        "completed" | "failed" | "cancelled" | "skipped" | "unsatisfiable" => 3,
        _ => 2, // pending / blocked / paused / unknown — non-terminal
    }
}

/// Recency key from existing timestamps (no `updated_at` field exists).
fn recency_key(t: &CoordTaskDto) -> u64 {
    t.completed_at.or(t.started_at).unwrap_or(t.created_at)
}

/// The single most-attention-worthy task: lowest salience rank, then most
/// recent, then lowest id (deterministic). `None` for an empty list.
#[must_use]
pub fn most_salient_task(tasks: &[CoordTaskDto]) -> Option<&CoordTaskDto> {
    tasks.iter().min_by(|a, b| {
        salience_rank(&a.status)
            .cmp(&salience_rank(&b.status))
            .then(recency_key(b).cmp(&recency_key(a))) // newer first
            .then(a.id.cmp(&b.id))
    })
}

/// "+N" badge value = remaining tasks after the salient one. `None` when ≤1.
#[must_use]
pub fn extra_task_count(total: usize) -> Option<usize> {
    total.checked_sub(1).filter(|&n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(
        id: &str,
        status: &str,
        created: u64,
        started: Option<u64>,
        completed: Option<u64>,
    ) -> CoordTaskDto {
        CoordTaskDto {
            id: id.to_string(),
            team_id: Some("t1".to_string()),
            subject: format!("subj-{id}"),
            description: String::new(),
            status: status.to_string(),
            owner: None,
            priority: "normal".to_string(),
            result: None,
            dependencies: Vec::new(),
            created_at: created,
            started_at: started,
            completed_at: completed,
        }
    }

    #[test]
    fn color_covers_every_stored_status_and_unknowns() {
        // All 10 `CoordTaskStatus` variants plus a future one must resolve to a
        // dot color (never panic, never render an empty style attribute).
        for s in [
            "pending",
            "blocked",
            "in_progress",
            "waiting_review",
            "paused",
            "completed",
            "skipped",
            "failed",
            "cancelled",
            "unsatisfiable",
            "weird_future_state",
        ] {
            assert!(task_status_color(s).starts_with('#'), "no color for {s}");
        }
    }

    #[test]
    fn salient_prefers_waiting_review_over_in_progress() {
        let tasks = vec![
            task("a", "in_progress", 10, Some(11), None),
            task("b", "waiting_review", 5, Some(6), None),
        ];
        assert_eq!(most_salient_task(&tasks).unwrap().id, "b");
    }

    #[test]
    fn salient_breaks_ties_by_recency_then_id() {
        // Both in_progress; pick the most-recently-advanced (started_at), then id.
        let tasks = vec![
            task("a", "in_progress", 1, Some(20), None),
            task("b", "in_progress", 1, Some(50), None),
            task("c", "pending", 1, None, None),
        ];
        assert_eq!(most_salient_task(&tasks).unwrap().id, "b");
    }

    #[test]
    fn salient_none_for_empty() {
        assert!(most_salient_task(&[]).is_none());
    }

    #[test]
    fn extra_count_hides_at_zero_and_one() {
        assert_eq!(extra_task_count(0), None);
        assert_eq!(extra_task_count(1), None);
        assert_eq!(extra_task_count(2), Some(1));
        assert_eq!(extra_task_count(5), Some(4));
    }
}
