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
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

/// Collapsed cluster shows at most this many discs; the rest fold into "+N".
/// Also the expand/collapse cutoff: a team with ≤ this many members renders
/// the expanded pill bar, more collapses to the avatar cluster. 5 is chosen
/// to fit the default 1180px window (with the name cap below bounding each
/// pill); see the matching container-query threshold in `tailwind.css`.
const CLUSTER_CAP: usize = 5;

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

/// Responsive roster bar for team chat. At wide widths (≥560px) renders an
/// expanded horizontal pill bar — one labeled capsule per member with status
/// dot, Chinese label, and a "leader" chip for the leader. At narrow widths (or
/// when the team exceeds `CLUSTER_CAP`) it collapses to an avatar cluster
/// button that opens a popover. A `ResizeObserver` publishes the bar's
/// rendered height to `--aleph-team-roster-h` so the message list can pad its
/// top to clear the floating bar.
#[component]
#[must_use]
pub fn TeamParticipants() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let open = RwSignal::new(false);
    let root_ref = NodeRef::<leptos::html::Div>::new();

    // Holds the active ResizeObserver so the effect and cleanup can disconnect it.
    let observer_store: StoredValue<Option<web_sys::ResizeObserver>> = StoredValue::new(None);

    // Publish the bar's rendered height to `--aleph-team-roster-h` so the
    // message list can pad its top and not hide the first bubbles behind the
    // floating bar. Mirrors the composer's `--composer-clearance` observer.
    Effect::new(move |_| {
        let Some(el) = root_ref.get() else { return };
        // Disconnect any observer created by a prior run of this effect.
        observer_store.update_value(|slot| {
            if let Some(prev) = slot.take() {
                prev.disconnect();
            }
        });
        let cb: Closure<dyn FnMut(js_sys::Array)> = Closure::new(move |entries: js_sys::Array| {
            if let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                let target: web_sys::Element = entry.target();
                let h = target.get_bounding_client_rect().height();
                if let Some(root) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.document_element())
                    .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
                {
                    let _ = root
                        .style()
                        .set_property("--aleph-team-roster-h", &format!("{h}px"));
                }
            }
        });
        if let Ok(observer) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
            observer.observe(&el);
            observer_store.set_value(Some(observer));
        }
        cb.forget();
    });
    // Reset the var when leaving team mode so single chat keeps its normal pad.
    on_cleanup(move || {
        observer_store.update_value(|slot| {
            if let Some(obs) = slot.take() {
                obs.disconnect();
            }
        });
        if let Some(root) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.document_element())
            .and_then(|e| e.dyn_into::<web_sys::HtmlElement>().ok())
        {
            let _ = root.style().set_property("--aleph-team-roster-h", "0px");
        }
    });

    view! {
        <div
            node_ref=root_ref
            class="aleph-roster-wrap"
            class:aleph-roster-crowded=move || collapse_for_count(chat.team_members.get().len())
        >
            // Expanded pill bar — one labeled capsule per member. Hidden by the
            // container query when narrow; not rendered at all when crowded.
            <Show when=move || !collapse_for_count(chat.team_members.get().len())>
                <div class="aleph-roster-expanded items-center gap-1.5 overflow-x-auto">
                    {move || {
                        chat.team_members
                            .get()
                            .into_iter()
                            .map(|m| {
                                let color = agent_color_for_id(&m.agent_id);
                                let dot = status_color(m.status);
                                let label = member_status_label(m.status);
                                let glyph = member_glyph(&m);
                                view! {
                                    <span class="aleph-roster-pill">
                                        <span
                                            class="w-6 h-6 rounded-full flex items-center \
                                                   justify-center text-[10px] font-bold \
                                                   text-white shrink-0"
                                            style=format!("background-color: {color};")
                                        >
                                            {glyph}
                                        </span>
                                        // Cap the name width and ellipsize the
                                        // overflow so one long name can't balloon
                                        // the pill. 68px shows the leader "Main
                                        // Agent" (~66px at this font) in full — the
                                        // must-fit reference; 5 Chinese chars (~60px)
                                        // also fit, while a 6th char or a long latin
                                        // name truncates with an ellipsis. Bounds
                                        // each pill so up to CLUSTER_CAP(5) fit the
                                        // default 1180px window.
                                        <span class="text-xs font-semibold max-w-[68px] truncate">{m.name}</span>
                                        {m.is_leader.then(|| view! {
                                            <span class="text-[10px] px-1 rounded \
                                                         bg-primary/15 text-primary">"队长"</span>
                                        })}
                                        <span class="text-[10px]" style=format!("color: {dot};")>"●"</span>
                                        <span class="text-[10px] opacity-60">{label}</span>
                                    </span>
                                }
                            })
                            .collect::<Vec<_>>()
                    }}
                </div>
            </Show>

            // Collapsed cluster — always rendered (sole view when crowded; the
            // container query shows it when narrow). Click expands the popover.
            <div class="aleph-roster-collapsed relative">
                <button
                    type="button"
                    // The roster wrapper is `pointer-events-none` so the drag
                    // band + top chrome stay reachable through it; this button
                    // opts back in (clickable) and out of the drag region
                    // (`aleph-no-drag`) so pressing it does not drag the window.
                    class="aleph-no-drag pointer-events-auto flex items-center gap-1 \
                           rounded-full px-1.5 py-1 bg-surface-raised/70 backdrop-blur \
                           border border-border/60 hover:bg-surface-raised/90 transition-colors"
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

                // Expanded popover — backdrop catcher + roster card (per-member
                // status dot + Chinese label + leader marker).
                <Show when=move || open.get()>
                    <div class="fixed inset-0 z-10 pointer-events-auto" on:click=move |_| open.set(false)></div>
                    <div class="absolute left-0 top-full mt-1 z-20 min-w-[180px] \
                                pointer-events-auto rounded-lg border border-border \
                                bg-surface-raised/95 backdrop-blur shadow-lg p-1.5 space-y-0.5">
                        {move || {
                            chat.team_members
                                .get()
                                .into_iter()
                                .map(|m| {
                                    let color = agent_color_for_id(&m.agent_id);
                                    let dot = status_color(m.status);
                                    let label = member_status_label(m.status);
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
                                                <span class="text-[10px] px-1 rounded \
                                                             bg-primary/15 text-primary">"队长"</span>
                                            })}
                                            <span class="truncate">{m.name}</span>
                                            <span class="text-[10px] opacity-60 ml-auto">{label}</span>
                                        </div>
                                    }
                                })
                                .collect::<Vec<_>>()
                        }}
                    </div>
                </Show>
            </div>
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
        assert_eq!(cluster_overflow(5), None); // == CLUSTER_CAP, no overflow
    }

    #[test]
    fn cluster_overflow_counts_excess_above_cap() {
        assert_eq!(cluster_overflow(6), Some(1)); // 6 = first value to overflow the cap
        assert_eq!(cluster_overflow(8), Some(3));
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
        assert!(!collapse_for_count(5)); // == CLUSTER_CAP, still expandable
        assert!(collapse_for_count(6)); // first count that forces collapse
        assert!(collapse_for_count(9));
    }
}
