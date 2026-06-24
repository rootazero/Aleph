//! Team chat participants affordance: a collapsed avatar-cluster button in the
//! chat surface top-left that expands into a popover listing leader + members
//! with live status. Replaces the always-on left roster rail (removed from
//! `view.rs`) so the conversation occupies the full width, like single chat.
//!
//! Reads the already-populated `chat.team_members` and reuses
//! `agent_identity::{agent_color_for_id, monogram}` for color/glyph. The pure
//! helpers (`member_glyph`, `cluster_overflow`, `status_color`) are host-tested.

use crate::views::chat::agent_identity::{agent_color_for_id, monogram};
use crate::views::chat::state::{ChatState, MemberStatus, TeamMemberView};
use leptos::prelude::*;

/// Collapsed cluster shows at most this many discs; the rest fold into "+N".
const CLUSTER_CAP: usize = 4;

/// Muted grey shared by the idle status dot and the overflow "+N" disc.
const MUTED_GREY: &str = "#6b7280";

/// Avatar glyph for a member: the emoji when present and non-empty, else a
/// name monogram (first char uppercased; "?" when the name is empty).
#[must_use]
pub fn member_glyph(m: &TeamMemberView) -> String {
    m.emoji
        .clone()
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| monogram(&m.name))
}

/// How many members overflow the collapsed cluster. `Some(n - CLUSTER_CAP)`
/// when there are more than `CLUSTER_CAP`, else `None`.
#[must_use]
pub fn cluster_overflow(n: usize) -> Option<usize> {
    n.checked_sub(CLUSTER_CAP).filter(|&extra| extra > 0)
}

/// Status-dot color, mirroring the (removed) roster rail mapping.
#[must_use]
pub fn status_color(s: MemberStatus) -> &'static str {
    match s {
        MemberStatus::Working => "#e0a458",
        MemberStatus::Done => "#4ec9b0",
        MemberStatus::Error => "#d16969",
        MemberStatus::Idle => MUTED_GREY,
    }
}

/// Chinese status word shown beside a member's name in the expanded roster bar.
/// Reuses the 4 existing `MemberStatus` variants (no "审阅中" state — spec §2).
#[must_use]
pub fn member_status_label(s: MemberStatus) -> &'static str {
    match s {
        MemberStatus::Working => "工作中",
        MemberStatus::Idle => "空闲",
        MemberStatus::Done => "完成",
        MemberStatus::Error => "错误",
    }
}

/// Count-driven collapse: more than `CLUSTER_CAP` members always render as the
/// avatar cluster (narrow-width collapse is handled separately by a CSS
/// container query). Mirrors the cluster's own `CLUSTER_CAP` cutoff.
#[must_use]
pub fn collapse_for_count(n: usize) -> bool {
    n > CLUSTER_CAP
}

/// Top-left participants affordance for team chat. Collapsed: overlapping
/// avatar discs + chevron. Expanded: a popover card (leader + members with
/// status dots), dismissed by clicking the transparent backdrop.
#[component]
#[must_use]
pub fn TeamParticipants() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let open = RwSignal::new(false);

    view! {
        <div class="relative">
            // Collapsed cluster button — overlapping discs + chevron.
            <button
                type="button"
                class="flex items-center gap-1 rounded-full px-1.5 py-1 \
                       bg-surface-raised/70 backdrop-blur border border-border/60 \
                       hover:bg-surface-raised/90 transition-colors"
                on:click=move |_| open.update(|o| *o = !*o)
            >
                <div class="flex items-center">
                    {move || {
                        let members = chat.team_members.get();
                        let mut discs = members
                            .iter()
                            .take(CLUSTER_CAP)
                            .enumerate()
                            .map(|(i, m)| {
                                let color = agent_color_for_id(&m.agent_id);
                                let glyph = member_glyph(m);
                                let margin = if i == 0 { "" } else { "-ml-2" };
                                view! {
                                    <span
                                        class=format!(
                                            "{margin} w-6 h-6 rounded-full flex items-center \
                                             justify-center text-[10px] font-bold text-white \
                                             ring-2 ring-surface-sunken"
                                        )
                                        style=format!("background-color: {color};")
                                    >
                                        {glyph}
                                    </span>
                                }
                                .into_any()
                            })
                            .collect::<Vec<_>>();
                        if let Some(extra) = cluster_overflow(members.len()) {
                            discs.push(
                                view! {
                                    <span
                                        class="-ml-2 w-6 h-6 rounded-full flex items-center \
                                               justify-center text-[10px] font-bold text-white \
                                               ring-2 ring-surface-sunken"
                                        style=format!("background-color: {MUTED_GREY};")
                                    >
                                        {format!("+{extra}")}
                                    </span>
                                }
                                .into_any(),
                            );
                        }
                        discs
                    }}
                </div>
                <span class="text-[10px] opacity-60 ml-0.5">"▾"</span>
            </button>

            // Expanded popover — backdrop catcher (click-outside closes) + card.
            <Show when=move || open.get()>
                <div
                    class="fixed inset-0 z-10"
                    on:click=move |_| open.set(false)
                ></div>
                <div class="absolute left-0 top-full mt-1 z-20 min-w-[180px] \
                            rounded-lg border border-border bg-surface-raised/95 \
                            backdrop-blur shadow-lg p-1.5 space-y-0.5">
                    {move || {
                        chat.team_members
                            .get()
                            .into_iter()
                            .map(|m| {
                                let color = agent_color_for_id(&m.agent_id);
                                let dot = status_color(m.status);
                                let glyph = member_glyph(&m);
                                view! {
                                    <div class="flex items-center gap-2 text-xs px-1.5 py-1 rounded">
                                        <span style=format!("color: {dot};")>"●"</span>
                                        <span
                                            class="w-6 h-6 rounded-full flex items-center \
                                                   justify-center text-[10px] font-bold \
                                                   text-white shrink-0"
                                            style=format!("background-color: {color};")
                                        >
                                            {glyph}
                                        </span>
                                        {m.is_leader.then(|| view! {
                                            <span class="text-[10px] opacity-60">"leader"</span>
                                        })}
                                        <span class="truncate">{m.name}</span>
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                    }}
                </div>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The component's `is_leader` label + popover rendering live inside `view!`
    // and are UI-only (exercised by manual E2E, not host tests). The pure
    // helpers below are the host-testable logic core.

    fn member(name: &str, emoji: Option<&str>, status: MemberStatus) -> TeamMemberView {
        TeamMemberView {
            agent_id: format!("id_{name}"),
            name: name.to_string(),
            emoji: emoji.map(String::from),
            role: "member".to_string(),
            is_leader: false,
            status,
        }
    }

    #[test]
    fn cluster_overflow_none_at_or_below_cap() {
        assert_eq!(cluster_overflow(0), None);
        assert_eq!(cluster_overflow(4), None);
    }

    #[test]
    fn cluster_overflow_counts_excess_above_cap() {
        assert_eq!(cluster_overflow(5), Some(1)); // 5 = first value to overflow the cap
        assert_eq!(cluster_overflow(7), Some(3));
    }

    #[test]
    fn member_glyph_prefers_emoji() {
        let m = member("Alice", Some("🛡️"), MemberStatus::Idle);
        assert_eq!(member_glyph(&m), "🛡️");
    }

    #[test]
    fn member_glyph_falls_back_to_name_monogram() {
        let m = member("alice", None, MemberStatus::Idle);
        assert_eq!(member_glyph(&m), "A");
    }

    #[test]
    fn member_glyph_empty_emoji_uses_monogram() {
        let m = member("bob", Some(""), MemberStatus::Idle);
        assert_eq!(member_glyph(&m), "B");
    }

    #[test]
    fn member_glyph_empty_name_no_emoji_is_question_mark() {
        let m = member("", None, MemberStatus::Idle);
        assert_eq!(member_glyph(&m), "?");
    }

    #[test]
    fn status_color_maps_all_variants() {
        assert_eq!(status_color(MemberStatus::Working), "#e0a458");
        assert_eq!(status_color(MemberStatus::Done), "#4ec9b0");
        assert_eq!(status_color(MemberStatus::Error), "#d16969");
        assert_eq!(status_color(MemberStatus::Idle), "#6b7280");
    }

    #[test]
    fn member_status_label_maps_all_variants() {
        assert_eq!(member_status_label(MemberStatus::Working), "工作中");
        assert_eq!(member_status_label(MemberStatus::Idle), "空闲");
        assert_eq!(member_status_label(MemberStatus::Done), "完成");
        assert_eq!(member_status_label(MemberStatus::Error), "错误");
    }

    #[test]
    fn collapse_only_above_cluster_cap() {
        assert!(!collapse_for_count(0));
        assert!(!collapse_for_count(4)); // == CLUSTER_CAP, still expandable
        assert!(collapse_for_count(5)); // first count that forces collapse
        assert!(collapse_for_count(9));
    }
}
