//! Outgoing wikilink list, shared by the memory console's two note drawers.
//!
//! The console has two of them — the Vault table's `drawer.rs` and the
//! galaxy's `node_detail_panel.rs` — and they had drifted into complementary
//! halves of one feature: the galaxy rendered outgoing links with their
//! resolver tier and dangling/tombstone status, the Vault rendered the
//! evidence chain. Each was missing exactly what the other had, and
//! `graph.node_detail` had been sending `outgoing` to both all along.
//!
//! The link semantics (what a status means, which edges are navigable, how
//! confidence and resolver tier are shown) live here once, so the two drawers
//! cannot answer that question differently again.

use leptos::prelude::*;

use crate::i18n::{t, t_string, use_i18n};
use crate::memory_graph::adapter::OutgoingLinkDto;

/// A link whose target resolved to a live note. Only these navigate; the
/// other two states name a note that is not there to open.
const STATUS_ACTIVE: &str = "active";
/// The target has never resolved to a note.
const STATUS_DANGLING: &str = "dangling";
/// The target resolved to a note that has since been deleted.
const STATUS_TOMBSTONE: &str = "tombstone";

/// Outgoing links section. Renders nothing at all when the note has none —
/// an empty "Links out" heading would read as "this note links nowhere on
/// purpose", which is not a claim the data supports.
#[component]
pub fn OutgoingLinks(
    links: Signal<Vec<OutgoingLinkDto>>,
    /// Invoked with the target note id when a live link is clicked. Each
    /// drawer navigates its own way (the galaxy selects a node, the Vault
    /// loads another note into itself), which is exactly the part that is
    /// NOT shared.
    on_navigate: Callback<String>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <Show when=move || !links.get().is_empty()>
            <div class="mt-4">
                <div class="text-[10px] uppercase tracking-widest text-text-tertiary mb-1">
                    {t!(i18n, memory.detail_outgoing)}
                </div>
                <ul class="space-y-1">
                    {move || links.get().into_iter().map(|link| {
                        let navigable = link.status == STATUS_ACTIVE;
                        let target = link.to.clone();
                        let display = link.label.clone().unwrap_or_else(|| link.raw.clone());
                        let relation = link
                            .relation
                            .clone()
                            .map(|r| format!(" · {r}"))
                            .unwrap_or_default();
                        let meta = match link.status.as_str() {
                            STATUS_DANGLING => {
                                format!(" · {}", t_string!(i18n, memory.link_dangling))
                            }
                            STATUS_TOMBSTONE => {
                                format!(" · {}", t_string!(i18n, memory.link_deleted))
                            }
                            // Which resolver tier made this edge
                            // (exact_path / exact_filename / alias /
                            // normalized) plus how confident it was.
                            _ => match link.resolved_by.as_deref() {
                                Some(tier) => {
                                    format!(" · {:.0}% · {tier}", link.confidence * 100.0)
                                }
                                None => format!(" · {:.0}%", link.confidence * 100.0),
                            },
                        };
                        let style = match link.status.as_str() {
                            STATUS_DANGLING => "font-size:11px;color:var(--text-meta);font-style:italic;padding:3px 6px",
                            STATUS_TOMBSTONE => "font-size:11px;color:var(--text-meta);text-decoration:line-through;padding:3px 6px",
                            _ => "font-size:11px;color:var(--cat-reference);padding:3px 6px;border-radius:4px;background:rgba(96,165,250,0.08);cursor:pointer",
                        };
                        view! {
                            <li
                                style=style
                                on:click=move |_| {
                                    if navigable {
                                        on_navigate.run(target.clone());
                                    }
                                }
                            >
                                {display}{relation}{meta}
                            </li>
                        }
                    }).collect_view()}
                </ul>
            </div>
        </Show>
    }
}
