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
//! Three sources land in one column:
//!
//! * **inbound** — what you gave the agent,
//! * **outbound** — what it produced,
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

mod files;
mod lightbox;
mod row;

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::artifacts::{ping_is_for_session, ArtifactItem, ArtifactsApi, ARTIFACT_TOPIC};
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::views::chat::state::ChatState;
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
#[must_use]
pub fn origin_badge_class(origin: &str) -> &'static str {
    match origin {
        "inbound" => "bg-surface-muted text-text-secondary",
        "export" => "bg-primary/15 text-primary",
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

    // Single re-read path. `get_untracked` on the key so calling this from an
    // event handler cannot register a reactive dependency outside an effect.
    let refetch = move || {
        let Some(key) = chat.session_key.get_untracked() else {
            items.set(Vec::new());
            return;
        };
        spawn_local(async move {
            match ArtifactsApi::list(&dash, &key).await {
                Ok(rows) => {
                    items.set(rows);
                    error.set(None);
                }
                // A session that has never stored anything is an empty list,
                // not an error — so anything that lands here is worth showing.
                Err(e) => error.set(Some(e)),
            }
        });
    };

    // Re-read when the conversation changes. Writes `items`/`error` only, and
    // neither is in the tracked set, so it cannot self-retrigger. Also drops a
    // stale lightbox: the image it points at belongs to the previous session.
    Effect::new(move |_| {
        let _ = chat.session_key.get();
        lightbox.set(None);
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
                    open_in_new_tab(&res.url);
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
        <div class="relative flex-1 min-h-0 flex flex-col aleph-content-top">
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

            <div class="flex-1 min-h-0 overflow-y-auto px-4 pb-3">
                {move || {
                    let rows = filtered(&items.get(), filter.get());
                    if rows.is_empty() {
                        view! {
                            <div class="text-xs text-text-tertiary py-2">
                                {t!(i18n, common.artifacts_empty)}
                            </div>
                        }.into_any()
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

/// Open `url` in a new tab. No-op off `wasm32` (host unit tests).
#[cfg(target_arch = "wasm32")]
fn open_in_new_tab(url: &str) {
    if let Some(w) = web_sys::window() {
        let _ = w.open_with_url_and_target(url, "_blank");
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn open_in_new_tab(_url: &str) {}

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
