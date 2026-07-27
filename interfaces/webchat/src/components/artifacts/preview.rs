//! Full-pane artifact viewer — images and text, in place.
//!
//! # Why a row opens here instead of in a new tab
//!
//! In the desktop shell a new tab is not a tab at all: `external_link.rs` hands
//! `/artifact/` URLs to the OS browser, so glancing at a thumbnail or a
//! three-line log would cost you the app. Anything Aleph can render itself
//! stays here; anything it cannot — a PDF, an archive, a rendered document —
//! still leaves, because those want a real viewer.
//!
//! # Why text is fetched rather than linked
//!
//! This started as an image-only lightbox, and everything that was not an image
//! was a download link. That answered "where did my screenshot go" and left
//! "what is in the file the agent just wrote" unanswered — the pane listed a
//! name, a size and a badge for a `.md` it could perfectly well have shown. The
//! bytes now come back over `artifacts.read_text`, the same JSON-RPC shape the
//! workspace-file preview a few pixels below already uses.
//!
//! Markdown is *rendered* (through the Panel's own sanitizing renderer, the one
//! chat bubbles use); everything else is printed as source in a `<pre>`. Both
//! paths put model- and user-controlled bytes on screen as **text**, never as
//! markup this module assembled.
//!
//! The overlay covers the pane, not the window: it is `absolute inset-0` inside
//! the pane's `relative` root, so the chat column keeps streaming underneath.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::artifacts::{ArtifactItem, ArtifactsApi, TextPreview};
use crate::components::markdown::MarkdownRenderer;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n};
use crate::views::chat::state::ChatState;

/// What the viewer is showing. `None` means closed.
///
/// The two variants are not a style choice: an image is already at a URL the
/// browser can paint, while text has to be asked for. Keeping them apart in the
/// type is what stops a fetch from being attempted for a PNG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PreviewTarget {
    /// Paint the bytes at `url` directly.
    Image { url: String, filename: String },
    /// Read the bytes over RPC and show them as text.
    Text {
        id: String,
        url: String,
        filename: String,
        /// Render as Markdown rather than printing the source.
        markdown: bool,
    },
}

impl PreviewTarget {
    /// The viewer this row opens, or `None` when the row is neither an image
    /// nor readable text — a PDF, an archive, a published document. Those keep
    /// their plain link out to a real viewer.
    #[must_use]
    pub fn for_item(item: &ArtifactItem) -> Option<Self> {
        if item.is_image() {
            return Some(Self::Image {
                url: item.url.clone(),
                filename: item.filename.clone(),
            });
        }
        if item.is_text() {
            return Some(Self::Text {
                id: item.id.clone(),
                url: item.url.clone(),
                filename: item.filename.clone(),
                markdown: item.is_markdown(),
            });
        }
        None
    }

    /// Name shown in the caption bar.
    fn filename(&self) -> &str {
        match self {
            Self::Image { filename, .. } | Self::Text { filename, .. } => filename,
        }
    }

    /// Capability URL, for the "open outside" affordance.
    fn url(&self) -> &str {
        match self {
            Self::Image { url, .. } | Self::Text { url, .. } => url,
        }
    }
}

/// Artifact overlay. Renders nothing when `target` is `None`.
///
/// Closes on backdrop click, on the ✕ button, and on Escape. The Escape
/// listener is registered once for the component's lifetime and only acts when
/// the viewer is actually open, so it coexists with the other Escape handlers
/// in the app (the palette's and the sidebar's) the same way those coexist with
/// each other.
#[component]
pub(super) fn Preview(target: RwSignal<Option<PreviewTarget>>) -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let dash = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Fetched body for a `Text` target. `None` while loading; the error signal
    // carries a failure so a broken read says so instead of showing an empty
    // page that reads as "the file is empty".
    let text = RwSignal::new(Option::<TextPreview>::None);
    let text_error = RwSignal::new(Option::<String>::None);

    {
        use leptos::ev::keydown;
        window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
            if ev.key() == "Escape" && target.get().is_some() {
                target.set(None);
            }
        });
    }

    // Fetch when the target becomes (or changes to) text. Clears first so the
    // previous file's body can never be shown under the new file's name — a
    // stale-render bug that is invisible in review and obvious in use. Writes
    // only `text`/`text_error`, neither of which it reads, so it cannot
    // self-retrigger.
    Effect::new(move |_| {
        let current = target.get();
        text.set(None);
        text_error.set(None);
        let Some(PreviewTarget::Text { id, .. }) = current else {
            return;
        };
        let Some(key) = chat.session_key.get_untracked() else {
            return;
        };
        spawn_local(async move {
            match ArtifactsApi::read_text(&dash, &key, &id).await {
                Ok(preview) => text.set(Some(preview)),
                Err(e) => text_error.set(Some(e)),
            }
        });
    });

    move || {
        target.get().map(|t| {
            let caption = t.filename().to_string();
            let alt = caption.clone();
            let href = t.url().to_string();

            let body = match &t {
                PreviewTarget::Image { url, .. } => view! {
                    <div class="flex-1 min-h-0 flex items-center justify-center p-3">
                        <img
                            src=url.clone()
                            alt=alt
                            class="max-w-full max-h-full object-contain rounded"
                        />
                    </div>
                }
                .into_any(),
                PreviewTarget::Text { markdown, .. } => {
                    let markdown = *markdown;
                    view! {
                        <div class="flex-1 min-h-0 overflow-auto p-3">
                            {move || match (text.get(), text_error.get()) {
                                (_, Some(e)) => view! {
                                    <div class="px-2 py-1 rounded bg-error/10 text-error
                                                text-[11px] break-words">{e}</div>
                                }
                                .into_any(),
                                (Some(preview), None) => {
                                    let source = preview.content.clone();
                                    view! {
                                        {markdown.then(|| view! {
                                            <MarkdownRenderer content=preview.content.clone() />
                                        })}
                                        {(!markdown).then(|| view! {
                                            // A text node, not markup: whatever the
                                            // file holds is shown, never parsed.
                                            <pre class="text-xs whitespace-pre-wrap break-words
                                                        font-mono text-text-secondary">
                                                {source}
                                            </pre>
                                        })}
                                        // A preview that silently stops is a
                                        // preview that lies about the file.
                                        {preview.truncated.then(|| view! {
                                            <div class="mt-2 pt-2 border-t border-border/40
                                                        text-[11px] text-text-tertiary">
                                                {t!(i18n, common.artifacts_preview_truncated)}
                                            </div>
                                        })}
                                    }
                                    .into_any()
                                }
                                (None, None) => view! {
                                    <div class="text-xs text-text-tertiary italic">
                                        {t!(i18n, common.artifacts_preview_loading)}
                                    </div>
                                }
                                .into_any(),
                            }}
                        </div>
                    }
                    .into_any()
                }
            };

            view! {
                <div
                    class="absolute inset-0 z-30 flex flex-col bg-surface-base/95 backdrop-blur-sm"
                    on:click=move |_| target.set(None)
                >
                    // `aleph-pane-top`: the overlay covers the pane from its very
                    // top edge, so without the inset this row — ✕ included — lands
                    // under the app chrome on web and the bell swallows the click.
                    <div class="aleph-pane-top flex items-center gap-2 px-3 pb-2 border-b border-border shrink-0">
                        <span class="min-w-0 flex-1 truncate text-xs">{caption}</span>
                        // Stop propagation so following the link does not also
                        // run the backdrop's close handler.
                        <a
                            href=href
                            target="_blank"
                            rel="noreferrer"
                            class="px-1.5 py-0.5 rounded text-[11px] text-text-secondary
                                   hover:text-text-primary shrink-0"
                            on:click=|ev: web_sys::MouseEvent| ev.stop_propagation()
                        >"↗"</a>
                        <button
                            class="px-1.5 py-0.5 rounded text-[11px] text-text-secondary
                                   hover:text-text-primary shrink-0"
                            on:click=move |ev: web_sys::MouseEvent| {
                                ev.stop_propagation();
                                target.set(None);
                            }
                        >"✕"</button>
                    </div>
                    // Scrolling and selecting inside the body must not be read as
                    // "clicked the backdrop" — a dismissal on every text selection
                    // is what makes a reading surface unusable.
                    <div
                        class="flex-1 min-h-0 flex flex-col"
                        on:click=|ev: web_sys::MouseEvent| ev.stop_propagation()
                    >
                        {body}
                    </div>
                </div>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(mime: &str) -> ArtifactItem {
        ArtifactItem {
            id: "id-1".into(),
            filename: "f".into(),
            mime_type: mime.into(),
            size: 1,
            origin: "outbound".into(),
            run_id: None,
            created_at: 0,
            url: "/artifact/c/id-1/f".into(),
        }
    }

    #[test]
    fn an_image_row_opens_the_image_view() {
        assert!(matches!(
            PreviewTarget::for_item(&item("image/png")),
            Some(PreviewTarget::Image { .. })
        ));
    }

    #[test]
    fn a_text_row_opens_the_text_view() {
        let target = PreviewTarget::for_item(&item("text/plain")).expect("previewable");
        match target {
            PreviewTarget::Text { id, markdown, .. } => {
                assert_eq!(id, "id-1", "the fetch needs the artifact id, not the url");
                assert!(!markdown);
            }
            PreviewTarget::Image { .. } => panic!("a text row opened the image view"),
        }
    }

    #[test]
    fn a_markdown_row_asks_for_the_renderer() {
        let target = PreviewTarget::for_item(&item("text/markdown")).expect("previewable");
        assert!(matches!(target, PreviewTarget::Text { markdown: true, .. }));
    }

    /// The rows that must keep leaving for a real viewer. A published
    /// deliverable is `text/html` — showing its *source* in a `<pre>` would be
    /// the least useful possible answer to a click on a finished report, which
    /// is why the deliverable card is a plain link and never reaches here.
    #[test]
    fn binaries_have_no_in_pane_view() {
        for mime in [
            "application/pdf",
            "application/zip",
            "application/octet-stream",
            "video/mp4",
        ] {
            assert!(
                PreviewTarget::for_item(&item(mime)).is_none(),
                "{mime} should leave the pane"
            );
        }
    }

    /// SVG is served as a picture everywhere else in the product; a click on the
    /// thumbnail asks for the picture, not its markup.
    #[test]
    fn svg_opens_as_an_image_not_as_source() {
        assert!(matches!(
            PreviewTarget::for_item(&item("image/svg+xml")),
            Some(PreviewTarget::Image { .. })
        ));
    }
}
