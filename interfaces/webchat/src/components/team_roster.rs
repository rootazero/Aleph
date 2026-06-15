//! Left roster rail for team chat: leader + members with live status dots.
//! Rendered only when `chat.team_id.is_some()` (conditional in view.rs).

use crate::views::chat::state::{ChatState, MemberStatus};
use crate::views::chat::team_events::agent_color;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn TeamRoster() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    view! {
        <div class="aleph-team-roster w-40 shrink-0 border-r border-border p-2 space-y-1 overflow-y-auto">
            {move || {
                chat.team_members
                    .get()
                    .into_iter()
                    .enumerate()
                    .map(|(i, m)| {
                        let color = agent_color(i);
                        let dot_color = match m.status {
                            MemberStatus::Working => "#e0a458",
                            MemberStatus::Done => "#4ec9b0",
                            MemberStatus::Error => "#d16969",
                            MemberStatus::Idle => "#6b7280",
                        };
                        view! {
                            <div
                                class="flex items-center gap-2 text-xs px-2 py-1 rounded"
                                style=format!("border-left: 3px solid {color}")
                            >
                                <span style=format!("color: {dot_color}")>"●"</span>
                                // `m.is_leader` (Copy bool) is read first, then
                                // `m.name` is moved into the view (no clone needed).
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
    }
}
