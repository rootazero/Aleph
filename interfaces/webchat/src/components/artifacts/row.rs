//! One artifact row, plus the filter chip.

use leptos::prelude::*;

use super::lightbox::LightboxTarget;
use super::{origin_badge_class, ArtifactFilter};
use crate::api::artifacts::ArtifactItem;

/// One filter chip. Extracted so the call sites stay one line each.
#[component]
pub(super) fn FilterChip<L>(
    filter: RwSignal<ArtifactFilter>,
    this: ArtifactFilter,
    label: L,
) -> impl IntoView
where
    L: Fn() -> String + Send + Sync + 'static,
{
    view! {
        <button
            class=move || {
                if filter.get() == this {
                    "px-2 py-0.5 rounded bg-primary text-white"
                } else {
                    "px-2 py-0.5 rounded text-text-secondary hover:text-text-primary"
                }
            }
            on:click=move |_| filter.set(this)
        >{move || label()}</button>
    }
}

/// One artifact row: thumbnail for images, name + size + origin badge for all.
///
/// The two kinds deliberately behave differently on click:
///
/// * **Images** open the in-pane [`super::lightbox::Lightbox`] — a glance
///   should not cost you the app.
/// * **Everything else** is a plain top-level link to the capability URL. In
///   the desktop shell that path is classified external (`external_link.rs`),
///   so the OS browser opens it instead of replacing the single webview.
#[component]
pub(super) fn ArtifactRow(
    item: ArtifactItem,
    lightbox: RwSignal<Option<LightboxTarget>>,
) -> impl IntoView {
    let is_image = item.is_image();
    let url = item.url.clone();
    let filename = item.filename.clone();
    let size = item.human_size();
    let origin = item.origin.clone();
    let badge = origin_badge_class(&origin);

    // Shared inner body so the image and file variants cannot drift apart.
    let body = {
        let filename = filename.clone();
        let thumb_src = url.clone();
        let thumb_alt = filename.clone();
        view! {
            {is_image.then(|| view! {
                <img
                    src=thumb_src
                    alt=thumb_alt
                    loading="lazy"
                    class="w-9 h-9 rounded object-cover bg-surface-muted shrink-0"
                />
            })}
            <span class="min-w-0 flex-1 text-left">
                <span class="block text-xs truncate group-hover:text-primary">{filename}</span>
                <span class="block text-[11px] text-text-tertiary">{size}</span>
            </span>
            <span class=format!("px-1.5 py-0.5 rounded text-[10px] shrink-0 {badge}")>{origin}</span>
        }
    };

    let row_class = "w-full flex items-center gap-2 py-1.5 border-b border-border/40 group";

    if is_image {
        let target = LightboxTarget {
            url: url.clone(),
            filename: filename.clone(),
        };
        view! {
            <button
                type="button"
                class=row_class
                on:click=move |_| lightbox.set(Some(target.clone()))
            >
                {body}
            </button>
        }
        .into_any()
    } else {
        view! {
            <a href=url target="_blank" rel="noreferrer" class=row_class>
                {body}
            </a>
        }
        .into_any()
    }
}
