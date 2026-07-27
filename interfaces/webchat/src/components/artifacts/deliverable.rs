//! The deliverables strip — what the agent published as finished work.
//!
//! Everything else in this pane is *material*: the image you dropped in, the
//! chart a tool drew, the transcript you exported. A deliverable is the thing
//! the conversation was for, so it does not queue up behind them in the same
//! newest-first list. It sits above the fold with an Open button, and a newly
//! arrived one opens itself.
//!
//! Auto-open is not a heuristic about "the run finished". The core only marks a
//! document as a deliverable when the agent called `artifact_publish`, so the
//! decision that something is worth interrupting for was made by the model, and
//! this only carries it out.

use leptos::prelude::*;
use std::collections::HashSet;

use crate::api::artifacts::ArtifactItem;

/// Split a listing into (deliverables, everything else), preserving the
/// server's newest-first order within each.
#[must_use]
pub fn partition_deliverables(items: &[ArtifactItem]) -> (Vec<ArtifactItem>, Vec<ArtifactItem>) {
    items
        .iter()
        .cloned()
        .partition(ArtifactItem::is_deliverable)
}

/// The newest deliverable this pane has not shown the user yet.
///
/// `items` arrives newest-first from the server and is not re-sorted here, so
/// the first match is the newest. Returns `None` when every deliverable is
/// already accounted for — which is the normal case on a plain refresh, and the
/// reason a re-read triggered by an unrelated artifact does not re-open
/// anything.
#[must_use]
pub fn first_unseen_deliverable(
    items: &[ArtifactItem],
    seen: &HashSet<String>,
) -> Option<ArtifactItem> {
    items
        .iter()
        .find(|i| i.is_deliverable() && !seen.contains(&i.id))
        .cloned()
}

/// Every deliverable id in a listing, for seeding the seen-set.
#[must_use]
pub fn deliverable_ids(items: &[ArtifactItem]) -> Vec<String> {
    items
        .iter()
        .filter(|i| i.is_deliverable())
        .map(|i| i.id.clone())
        .collect()
}

/// One deliverable, rendered as a card rather than a list row.
///
/// A plain anchor, not a scripted click: in the desktop shell the `/artifact/`
/// path is classified external, so the OS browser opens it instead of replacing
/// the shell's single webview.
#[component]
pub(super) fn DeliverableCard<L>(item: ArtifactItem, open_label: L) -> impl IntoView
where
    L: Fn() -> String + Send + Sync + 'static,
{
    let size = item.human_size();
    view! {
        <a
            href=item.url
            target="_blank"
            rel="noreferrer"
            class="flex items-center gap-2 px-2.5 py-2 mb-1.5 rounded-lg border border-primary/30
                   bg-primary/5 hover:bg-primary/10 group"
        >
            <span class="text-primary text-sm shrink-0" aria-hidden="true">"◆"</span>
            <span class="min-w-0 flex-1">
                <span class="block text-xs font-medium truncate group-hover:text-primary">
                    {item.filename}
                </span>
                <span class="block text-[11px] text-text-tertiary">{size}</span>
            </span>
            <span class="px-2 py-0.5 rounded text-[11px] bg-primary text-white shrink-0">
                {move || open_label()}
            </span>
        </a>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, origin: &str) -> ArtifactItem {
        ArtifactItem {
            id: id.into(),
            filename: format!("{id}.html"),
            mime_type: "text/html".into(),
            size: 1,
            origin: origin.into(),
            run_id: None,
            created_at: 0,
            url: format!("/artifact/c/{id}/f"),
        }
    }

    #[test]
    fn deliverables_are_split_out_without_reordering_either_side() {
        let rows = vec![
            item("d2", "deliverable"),
            item("img", "outbound"),
            item("d1", "deliverable"),
            item("up", "inbound"),
        ];
        let (deliverables, rest) = partition_deliverables(&rows);
        // Server order (newest first) survives inside each group.
        assert_eq!(
            deliverables
                .iter()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            vec!["d2", "d1"]
        );
        assert_eq!(
            rest.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["img", "up"]
        );
    }

    #[test]
    fn an_export_is_not_a_deliverable() {
        // The distinction this whole feature exists for: a transcript export is
        // the conversation, not the work product, and must not be pinned or
        // auto-opened.
        let rows = vec![item("e", "export"), item("d", "deliverable")];
        let (deliverables, rest) = partition_deliverables(&rows);
        assert_eq!(deliverables.len(), 1);
        assert_eq!(deliverables[0].id, "d");
        assert_eq!(rest[0].id, "e");
    }

    #[test]
    fn the_newest_unseen_deliverable_is_the_one_offered() {
        let rows = vec![
            item("newest", "deliverable"),
            item("older", "deliverable"),
            item("img", "outbound"),
        ];
        let mut seen = HashSet::new();
        assert_eq!(
            first_unseen_deliverable(&rows, &seen).map(|i| i.id),
            Some("newest".to_string())
        );

        seen.insert("newest".to_string());
        assert_eq!(
            first_unseen_deliverable(&rows, &seen).map(|i| i.id),
            Some("older".to_string()),
            "an already-shown newest must not mask an unshown older one"
        );

        seen.insert("older".to_string());
        assert_eq!(first_unseen_deliverable(&rows, &seen), None);
    }

    #[test]
    fn a_refresh_caused_by_an_unrelated_artifact_offers_nothing() {
        // Every ping re-reads the whole list; without the seen-set that would
        // re-open the same document on every screenshot the agent takes.
        let seen: HashSet<String> = ["d".to_string()].into_iter().collect();
        let rows = vec![item("d", "deliverable"), item("shot", "outbound")];
        assert!(first_unseen_deliverable(&rows, &seen).is_none());
    }

    #[test]
    fn only_deliverable_ids_seed_the_seen_set() {
        let rows = vec![
            item("d", "deliverable"),
            item("e", "export"),
            item("o", "outbound"),
        ];
        assert_eq!(deliverable_ids(&rows), vec!["d".to_string()]);
    }
}
