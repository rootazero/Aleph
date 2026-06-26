//! One Vault note cell: title + "type · date" sub + colored type badge.

use leptos::prelude::*;

use crate::api::CompressedFact;
use crate::views::memory::data::{fact_facet, format_ts, MemoryFacet};

/// (badge label, badge CSS modifier) for a note's facet.
fn badge_for(category: &str) -> (&'static str, &'static str) {
    match fact_facet(category) {
        MemoryFacet::Facts => ("Fact", "badge-primary"),
        MemoryFacet::Feedback => ("Feedback", "badge-info"),
        MemoryFacet::Lessons => ("Lesson", "badge-warning"),
        // fact_facet never returns AllNotes/Raw, but keep the match total.
        _ => ("Note", "badge-info"),
    }
}

#[component]
#[must_use]
pub fn PhoneMemoryCell(fact: CompressedFact, on_open: Callback<CompressedFact>) -> impl IntoView {
    let (label, badge_cls) = badge_for(&fact.category);
    let title = fact.content.clone();
    let sub = format!("{} · {}", label, format_ts(fact.created_at));
    let fact_for_click = fact.clone();
    view! {
        <div class="cell" on:click=move |_| on_open.run(fact_for_click.clone())>
            <div class="cell-body">
                <div class="cell-title" style="font-weight:500;">{title}</div>
                <div class="cell-sub" style="margin-top:2px;">{sub}</div>
            </div>
            <span class=format!("badge {badge_cls}") style="flex:none;">{label}</span>
        </div>
    }
}
