//! Single-agent artifacts surface — the body of the workspace pane when the
//! conversation is not a team chat.
//!
//! This pane replaced a contextual *inspector* that routed clicks in the chat
//! column to a tool-detail / cost / reasoning / plan view. The tool surface in
//! particular rendered through the very same `tool_card::render_body` the chat
//! column already used, so the right column was showing a second copy of what
//! was on screen a few hundred pixels to its left. What it never showed — what
//! nothing in the Panel showed — was the image you dragged in or the file the
//! agent just wrote. That is now this pane's whole job.
//!
//! Four sources land in one column, and the first is not like the others:
//!
//! * **deliverables** — what the agent published as *finished work*
//!   ([`deliverable`]). Pinned above the fold, and a newly arrived one opens
//!   itself in a browser tab. Everything below is the material that produced
//!   it.
//! * **inbound** — what you gave the agent,
//! * **outbound** — what it produced along the way,
//! * **workspace files** — the project tree, folded in at the foot
//!   ([`files::WorkspaceFiles`]) rather than living as a second browser.
//!
//! Freshness comes from the gateway's content-free `session.artifact` ping: the
//! frame carries only the session key, so every arrival triggers a re-read of
//! the authoritative list rather than a local patch. That is deliberate — the
//! `agent_trace` stream is intentionally lossy, so a live frame may never be
//! the source of truth for what exists. Pings for *other* sessions are filtered
//! out ([`ping_is_for_session`]) so a background run cannot make the visible
//! pane flicker.
//!
//! Pure logic (filtering, badge derivation) is separated from the view so it is
//! host-testable via `cargo test -p aleph-panel --lib`.

pub mod deliverable;
mod files;
mod lightbox;
mod row;

use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashSet;

use crate::api::artifacts::{ping_is_for_session, ArtifactItem, ArtifactsApi, ARTIFACT_TOPIC};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::chat::state::ChatState;
use deliverable::{
    deliverable_ids, first_unseen_deliverable, partition_deliverables, DeliverableCard,
};
use files::WorkspaceFiles;
use lightbox::{Lightbox, LightboxTarget};
use row::{ArtifactRow, FilterChip};

/// Which rows the pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactFilter {
    All,
    Images,
    Files,
}

impl ArtifactFilter {
    /// Does `item` belong in this filter's view?
    #[must_use]
    pub fn matches(self, item: &ArtifactItem) -> bool {
        match self {
            Self::All => true,
            Self::Images => item.is_image(),
            Self::Files => !item.is_image(),
        }
    }
}

/// Apply a filter to a list, preserving the server's newest-first order.
#[must_use]
pub fn filtered(items: &[ArtifactItem], filter: ArtifactFilter) -> Vec<ArtifactItem> {
    items
        .iter()
        .filter(|i| filter.matches(i))
        .cloned()
        .collect()
}

/// Tailwind classes for an origin badge. Inbound (what the user gave the agent)
/// and outbound (what the agent produced) read differently at a glance.
///
/// Deliverables deliberately have no arm here: they never reach a material row
/// — [`partition_deliverables`] lifts them into their own section, where the
/// card carries its own styling.
#[must_use]
pub fn origin_badge_class(origin: &str) -> &'static str {
    use aleph_protocol::artifact as wire;
    match origin {
        wire::ORIGIN_INBOUND => "bg-surface-muted text-text-secondary",
        wire::ORIGIN_EXPORT => "bg-primary/15 text-primary",
        // "outbound" and anything a newer core invents.
        _ => "bg-success/15 text-success",
    }
}

/// The artifacts pane body. Renders nothing useful until the session has a key
/// (a brand-new conversation has produced nothing yet, by definition).
#[component]
#[must_use]
pub fn ArtifactsSurface() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();

    let items = RwSignal::new(Vec::<ArtifactItem>::new());
    let filter = RwSignal::new(ArtifactFilter::All);
    let exporting = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let lightbox = RwSignal::new(Option::<LightboxTarget>::None);
    // Deliverables this pane has already offered the user. Seeded from the
    // first listing of a conversation so that *opening* an old conversation
    // never re-opens its report — only a document that arrives while you are
    // watching is new.
    let seen = RwSignal::new(HashSet::<String>::new());
    let seeded = RwSignal::new(false);
    // Set when an auto-open was refused (a pop-up blocker, or no window at
    // all). The banner it drives turns the retry into a real user gesture,
    // which browsers always allow — so a blocked open degrades to one click
    // rather than to silence.
    let blocked = RwSignal::new(Option::<ArtifactItem>::None);

    // Single settle path for a fresh listing: decide what is new BEFORE the
    // seen-set absorbs it, then publish.
    let settle = move |rows: Vec<ArtifactItem>| {
        let arrival = seen.with_untracked(|s| first_unseen_deliverable(&rows, s));
        let first_listing = !seeded.get_untracked();
        seen.update(|s| s.extend(deliverable_ids(&rows)));
        seeded.set(true);
        items.set(rows);
        error.set(None);

        // Nothing auto-opens on the first listing: everything there predates
        // the user's attention, and popping a tab for a week-old report the
        // moment a conversation is opened would be the opposite of helpful.
        if let (false, Some(doc)) = (first_listing, arrival) {
            if !open_in_new_tab(&doc.url) {
                blocked.set(Some(doc));
            }
        }
    };

    // Single re-read path. `get_untracked` on the key so calling this from an
    // event handler cannot register a reactive dependency outside an effect.
    let refetch = move || {
        let Some(key) = chat.session_key.get_untracked() else {
            items.set(Vec::new());
            return;
        };
        spawn_local(async move {
            match ArtifactsApi::list(&dash, &key).await {
                Ok(rows) => settle(rows),
                // A session that has never stored anything is an empty list,
                // not an error — so anything that lands here is worth showing.
                Err(e) => error.set(Some(e)),
            }
        });
    };

    // Re-read when the conversation changes. Writes `items`/`error` only, and
    // neither is in the tracked set, so it cannot self-retrigger. Also drops a
    // stale lightbox and the seen-set: both point at the previous session.
    Effect::new(move |_| {
        let _ = chat.session_key.get();
        lightbox.set(None);
        blocked.set(None);
        seen.update(HashSet::clear);
        seeded.set(false);
        refetch();
    });

    // Ask the gateway to push us the invalidation ping.
    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let dash2 = dash;
        spawn_local(async move {
            let _ = dash2.subscribe_topic(ARTIFACT_TOPIC).await;
        });
    });

    let sub_id = dash.subscribe_events(move |evt| {
        if evt.topic != ARTIFACT_TOPIC {
            return;
        }
        let Some(key) = chat.session_key.get_untracked() else {
            return;
        };
        if ping_is_for_session(&evt.data, &key) {
            refetch();
        }
    });
    on_cleanup(move || dash.unsubscribe_events(sub_id));

    let export = move |_| {
        let Some(key) = chat.session_key.get_untracked() else {
            return;
        };
        if exporting.get_untracked() {
            return;
        }
        exporting.set(true);
        spawn_local(async move {
            match ArtifactsApi::export_html(&dash, &key).await {
                Ok(res) => {
                    error.set(None);
                    // The export is itself an artifact of this session, so the
                    // core's own ping will refresh the list; re-read anyway so
                    // the new row is there even if the frame was dropped (the
                    // event stream is explicitly best-effort).
                    refetch();
                    // The await above already cost us the user gesture, so a
                    // strict pop-up blocker can refuse this too; the export row
                    // is in the list either way.
                    let _ = open_in_new_tab(&res.url);
                }
                Err(e) => error.set(Some(e)),
            }
            exporting.set(false);
        });
    };

    let has_session = move || chat.session_key.get().is_some();

    view! {
        // `relative` anchors the lightbox overlay to the pane, so an image
        // preview covers this column and leaves the chat streaming beside it.
        // `aleph-pane-top` (not `aleph-content-top`): the header below carries
        // a real button, and on web the smaller content inset would park it on
        // the NotificationCenter bell.
        <div class="relative flex-1 min-h-0 flex flex-col aleph-pane-top">
            <div class="flex items-center gap-2 px-4 py-2 border-b border-border shrink-0">
                <span class="text-xs font-semibold">{t!(i18n, common.artifacts_title)}</span>
                <span class="text-[11px] text-text-tertiary">
                    {move || {
                        let n = items.get().len();
                        if n == 0 { String::new() } else { n.to_string() }
                    }}
                </span>
                <span class="flex-1" />
                <button
                    class="px-2 py-1 rounded text-[11px] text-text-secondary hover:text-text-primary
                           disabled:opacity-40 disabled:cursor-not-allowed"
                    disabled=move || exporting.get() || !has_session()
                    on:click=export
                >{t!(i18n, common.artifacts_export)}</button>
            </div>

            <div class="flex gap-1 px-4 py-2 shrink-0 text-[11px]">
                <FilterChip
                    filter=filter
                    this=ArtifactFilter::All
                    label=move || t_string!(i18n, common.artifacts_filter_all).to_string()
                />
                <FilterChip
                    filter=filter
                    this=ArtifactFilter::Images
                    label=move || t_string!(i18n, common.artifacts_filter_images).to_string()
                />
                <FilterChip
                    filter=filter
                    this=ArtifactFilter::Files
                    label=move || t_string!(i18n, common.artifacts_filter_files).to_string()
                />
            </div>

            {move || error.get().map(|e| view! {
                <div class="mx-4 mb-2 px-2 py-1 rounded bg-error/10 text-error text-[11px] break-words">
                    {e}
                </div>
            })}

            // Auto-open was refused; this click is the gesture that is not.
            {move || blocked.get().map(|doc| {
                let url = doc.url.clone();
                view! {
                    <a
                        href=url
                        target="_blank"
                        rel="noreferrer"
                        class="mx-4 mb-2 px-2 py-1.5 rounded bg-primary/10 border border-primary/30
                               text-[11px] text-primary flex items-center gap-2"
                        on:click=move |_| blocked.set(None)
                    >
                        <span class="flex-1 min-w-0 truncate">
                            {t!(i18n, common.artifacts_ready)}
                        </span>
                        <span class="shrink-0 underline">{t!(i18n, common.artifacts_open)}</span>
                    </a>
                }
            })}

            <div class="flex-1 min-h-0 overflow-y-auto px-4 pb-3">
                // Deliverables sit above the filters: they are the answer, not
                // one more row to sift for. The filter chips below govern the
                // supporting material only.
                {move || {
                    let (deliverables, _) = partition_deliverables(&items.get());
                    (!deliverables.is_empty()).then(|| view! {
                        <div class="pb-2 mb-1 border-b border-border/40">
                            <div class="text-[11px] font-semibold text-text-secondary pb-1.5">
                                {t!(i18n, common.artifacts_deliverables)}
                            </div>
                            {deliverables.into_iter().map(|item| view! {
                                <DeliverableCard
                                    item=item
                                    open_label=move || {
                                        t_string!(i18n, common.artifacts_open).to_string()
                                    }
                                />
                            }).collect::<Vec<_>>()}
                        </div>
                    })
                }}
                {move || {
                    let all = items.get();
                    let (_, material) = partition_deliverables(&all);
                    let rows = filtered(&material, filter.get());
                    if rows.is_empty() {
                        // "Nothing produced yet" is only true when the session
                        // really has nothing. A published report with no
                        // supporting files must not be captioned as emptiness.
                        if all.is_empty() {
                            view! {
                                <div class="text-xs text-text-tertiary py-2">
                                    {t!(i18n, common.artifacts_empty)}
                                </div>
                            }.into_any()
                        } else {
                            ().into_any()
                        }
                    } else {
                        rows.into_iter()
                            .map(|item| view! { <ArtifactRow item=item lightbox=lightbox /> })
                            .collect::<Vec<_>>()
                            .into_any()
                    }
                }}
            </div>

            <WorkspaceFiles />
            <Lightbox target=lightbox />
        </div>
    }
}

/// Open `url` in a new tab. Returns whether it actually opened.
///
/// A browser refuses `window.open` outside a user gesture, and returns `null`
/// rather than throwing — so the caller can tell the difference between "the
/// document is on screen" and "the user needs to click", and does.
#[cfg(target_arch = "wasm32")]
fn open_in_new_tab(url: &str) -> bool {
    web_sys::window().is_some_and(|w| {
        matches!(
            w.open_with_url_and_target(url, "_blank"),
            Ok(Some(_))
        )
    })
}

/// Host unit tests have no window, which is the same answer a blocked pop-up
/// gives: nothing opened.
#[cfg(not(target_arch = "wasm32"))]
const fn open_in_new_tab(_url: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(mime: &str, origin: &str) -> ArtifactItem {
        ArtifactItem {
            id: "id".into(),
            filename: "f".into(),
            mime_type: mime.into(),
            size: 1,
            origin: origin.into(),
            run_id: None,
            created_at: 0,
            url: "/artifact/c/id/f".into(),
        }
    }

    #[test]
    fn the_images_filter_keeps_only_images() {
        let rows = vec![
            item("image/png", "outbound"),
            item("application/pdf", "inbound"),
            item("image/svg+xml", "outbound"),
        ];
        assert_eq!(filtered(&rows, ArtifactFilter::Images).len(), 2);
        assert_eq!(filtered(&rows, ArtifactFilter::Files).len(), 1);
        assert_eq!(filtered(&rows, ArtifactFilter::All).len(), 3);
    }

    #[test]
    fn filtering_preserves_the_servers_newest_first_order() {
        // The pane must never re-sort: `artifacts.list` is already newest-first
        // and re-sorting client-side would drift from the store's own order.
        let mut a = item("image/png", "outbound");
        a.id = "newest".into();
        let mut b = item("image/png", "outbound");
        b.id = "older".into();
        let rows = vec![a, b];
        let out = filtered(&rows, ArtifactFilter::Images);
        assert_eq!(out[0].id, "newest");
        assert_eq!(out[1].id, "older");
    }

    #[test]
    fn an_unknown_origin_still_gets_a_badge() {
        // Forward compatibility: a newer core may add an origin the Panel has
        // never heard of; it must render, not blank out.
        assert!(!origin_badge_class("something_new").is_empty());
        assert_ne!(
            origin_badge_class("inbound"),
            origin_badge_class("outbound")
        );
    }
}
