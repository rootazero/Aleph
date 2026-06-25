//! Host-testable pure logic for the team task strip/drawer: status → label/color
//! mapping over the raw snake_case `CoordTaskStatus` wire strings, "most salient"
//! task selection, and the overflow count. No Leptos signals / DOM here.

use crate::api::teams::CoordTaskDto;

/// Chinese label for a raw `CoordTaskStatus` wire string (snake_case, all 10
/// variants from src/agents/swarm/tasks/mod.rs). Unknown / future variants echo
/// verbatim so the strip/drawer always render something (never panics).
#[must_use]
pub fn task_status_label(status: &str) -> String {
    match status {
        "waiting_review" => "待审阅",
        "in_progress" => "进行中",
        "pending" => "待处理",
        "blocked" => "阻塞",
        "completed" => "已完成",
        "failed" => "失败",
        "cancelled" => "已取消",
        "skipped" => "已跳过",
        "paused" => "已暂停",
        "unsatisfiable" => "不可满足",
        other => return other.to_string(),
    }
    .to_string()
}

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
    fn label_maps_all_ten_variants_and_echoes_unknown() {
        assert_eq!(task_status_label("waiting_review"), "待审阅");
        assert_eq!(task_status_label("in_progress"), "进行中");
        assert_eq!(task_status_label("pending"), "待处理");
        assert_eq!(task_status_label("blocked"), "阻塞");
        assert_eq!(task_status_label("completed"), "已完成");
        assert_eq!(task_status_label("failed"), "失败");
        assert_eq!(task_status_label("cancelled"), "已取消");
        assert_eq!(task_status_label("skipped"), "已跳过");
        assert_eq!(task_status_label("paused"), "已暂停");
        assert_eq!(task_status_label("unsatisfiable"), "不可满足");
        // Unknown / future variants echo verbatim (never panics, forward-compatible).
        assert_eq!(
            task_status_label("weird_future_state"),
            "weird_future_state"
        );
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
