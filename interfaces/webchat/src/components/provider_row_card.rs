//! Shared provider-row card for Settings provider lists.
//!
//! Five sites across `views/settings/{providers,embedding_providers,reranking_providers}/`
//! render structurally-identical "card buttons" in left-panel lists. Each card has:
//! - a coloured icon tile with the first character of the provider name, or an
//!   optional `icon_glyph` (e.g. a generation-category emoji) when supplied
//! - the provider name + an optional badge (e.g. "Default", "Verified", "Active")
//! - a subtitle line (description, model, or `model · dims`)
//! - a small verified dot on the icon corner (only some lists)
//! - selection / configured / unconfigured visual states driven by the parent signal
//!
//! Reactive slots accept `impl Fn() -> T + 'static + Send` closures so each site can
//! decide whether the predicate is static (`move || true`) or signal-driven
//! (`move || providers.get().iter().any(...)`).
//!
//! Also supports OAuth-style rows (e.g. the `SubscriptionLoginSection` row in
//! `providers/list.rs`) via the optional `large_icon` prop (`w-10 h-10` icon
//! tile) and the optional `trailing` slot (a right-pushed element such as a
//! chevron). Prefer these props over writing a new inline row.

use leptos::prelude::*;

/// Verified-dot variants rendered on the icon corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowDot {
    /// No dot rendered.
    None,
    /// Green success dot — provider is verified.
    Verified,
    /// Greyed dot — placeholder used by the Custom providers list when a row
    /// always shows SOME dot regardless of verified state.
    Inactive,
}

/// Classes for a row drawn as a standalone card, spaced from its siblings.
fn row_class_card(selected: bool, configured: bool) -> String {
    let base = "text-left p-3 rounded-lg border transition-all";
    if selected {
        format!("{base} bg-primary-subtle border-primary")
    } else if configured {
        format!("{base} bg-surface-raised border-border hover:border-primary/40")
    } else {
        format!("{base} bg-surface-sunken border-border hover:border-border-strong")
    }
}

/// Classes for a row drawn as a member of a bordered list.
///
/// Extracted so the one property the flush mode exists to guarantee — that it
/// contributes **no** border and **no** corner radius, leaving the container's
/// border as the only edge — is something a test can assert rather than
/// something a reader has to re-derive from a class soup.
fn row_class_flush(selected: bool) -> String {
    let base = "text-left p-3 transition-colors";
    if selected {
        format!("{base} bg-primary-subtle")
    } else {
        format!("{base} hover:bg-surface-raised")
    }
}

#[component]
pub fn ProviderRowCard(
    /// Display name (capitalisation is caller's responsibility).
    name: String,
    /// CSS color value for the icon-tile background.
    icon_color: String,
    /// Secondary descriptor line — description, model, or `model · dims`.
    subtitle: String,
    /// Selected predicate — drives the primary-subtle border + bg.
    is_selected: impl Fn() -> bool + 'static + Send,
    /// Configured predicate — when not selected, configured rows get bg-surface-raised,
    /// unconfigured rows get bg-surface-sunken.
    is_configured: impl Fn() -> bool + 'static + Send,
    /// Verified-indicator dot on the icon corner.
    dot: impl Fn() -> RowDot + 'static + Send,
    /// Right-side badge slot. Caller returns an `AnyView` (use `.into_any()`).
    /// Use `move || view! { <span></span> }.into_any()` to render no badge.
    badge: impl Fn() -> AnyView + 'static + Send,
    /// Click handler.
    on_click: impl Fn() + 'static + Send,
    /// Optional trailing element rendered after the name/subtitle block,
    /// pushed to the right (e.g. a chevron for OAuth subscription rows).
    #[prop(optional, into)]
    trailing: Option<ViewFn>,
    /// When true the icon tile uses `w-10 h-10` instead of `w-8 h-8`
    /// (used by OAuth subscription rows).
    #[prop(optional)]
    large_icon: bool,
    /// Optional glyph (e.g. an emoji) rendered in the icon tile instead of the
    /// first character. Used by generation presets whose icons convey the
    /// generation category (🖼️/🎬/🎤).
    ///
    /// `optional_no_strip`, not `optional`: plain `optional` strips the
    /// `Option` and hands the setter a bare `String`, so a caller holding an
    /// `Option` — a shared picker drawing rows from catalogues that do and do
    /// not have glyphs — cannot forward it. Omitting the prop still defaults
    /// to `None`.
    #[prop(optional_no_strip)]
    icon_glyph: Option<String>,
    /// Draw the row as a member of a bordered list rather than as a card of
    /// its own: no border, no corners, selection carried by background alone.
    ///
    /// The default (a card) is right in the left panel, where rows are spaced
    /// siblings on the page background. It is wrong inside the picker's
    /// popover: there the rows already sit inside a bordered, padded container,
    /// so each card drew a *third* border at a *different* width than the
    /// button above it and the sections below it — the eye reads a column whose
    /// edges keep moving. Flush rows plus one divider between them read as one
    /// list, which is what a catalogue is.
    ///
    /// `is_configured` is deliberately not consulted in this mode. In a card
    /// list it separates rows that are yours from rows that are on offer; in
    /// the picker every row is on offer and the ones you already have say so
    /// with a badge, so a second background tint would only make the list
    /// stripey.
    #[prop(optional)]
    flush: bool,
) -> impl IntoView {
    let first_char = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let icon_content = icon_glyph.unwrap_or(first_char);
    let name_for_view = name;
    view! {
        <button
            on:click=move |_| on_click()
            class=move || {
                if flush {
                    // `is_configured` is not read here on purpose — see the
                    // prop's doc. Reading it would also subscribe this class to
                    // a signal whose value the row does not draw.
                    row_class_flush(is_selected())
                } else {
                    row_class_card(is_selected(), is_configured())
                }
            }
        >
            <div class="flex items-center gap-3">
                <div class="relative shrink-0">
                    <div
                        class=format!(
                            "{} rounded-lg flex items-center justify-center text-white text-sm font-bold",
                            if large_icon { "w-10 h-10" } else { "w-8 h-8" }
                        )
                        style=format!("background-color: {}", icon_color)
                    >
                        {icon_content}
                    </div>
                    {move || match dot() {
                        RowDot::Verified => view! {
                            <span class="absolute -top-0.5 -right-0.5 w-2.5 h-2.5 rounded-full bg-success border-2 border-surface-raised" />
                        }.into_any(),
                        RowDot::Inactive => view! {
                            <span class="absolute -top-0.5 -right-0.5 w-2.5 h-2.5 rounded-full bg-text-tertiary/30 border-2 border-surface-raised" />
                        }.into_any(),
                        RowDot::None => view! { <span /> }.into_any(),
                    }}
                </div>
                <div class="min-w-0">
                    <div class="flex items-center gap-2">
                        <span class="font-medium text-text-primary text-sm truncate">
                            {name_for_view}
                        </span>
                        {move || badge()}
                    </div>
                    <div class="text-xs text-text-tertiary truncate">
                        {subtitle}
                    </div>
                </div>
                {trailing.map(|t| view! { <div class="ml-auto shrink-0">{t.run()}</div> })}
            </div>
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defect this mode exists for: inside the picker's bordered popover a
    /// card drew its own border, inset from the container around it and from
    /// the rows below it, so one column showed edges at three widths. A flush
    /// row must contribute no edge of its own.
    #[test]
    fn a_flush_row_draws_no_edge_of_its_own() {
        for selected in [true, false] {
            let cls = row_class_flush(selected);
            assert!(
                !cls.contains("border"),
                "flush row must not draw a border: {cls}"
            );
            assert!(
                !cls.contains("rounded"),
                "flush row must not round its corners: {cls}"
            );
        }
    }

    /// …and selection still has to be visible without one, or the keyboard
    /// walk would light nothing.
    #[test]
    fn a_flush_row_shows_selection_through_its_background() {
        assert!(row_class_flush(true).contains("bg-primary-subtle"));
        assert!(!row_class_flush(false).contains("bg-primary-subtle"));
    }

    /// The default is unchanged: the left-panel lists are spaced cards on the
    /// page background, where the border *is* the card.
    #[test]
    fn a_card_row_keeps_its_border_and_corners() {
        for (selected, configured) in [(true, true), (false, true), (false, false)] {
            let cls = row_class_card(selected, configured);
            assert!(cls.contains("border"), "{cls}");
            assert!(cls.contains("rounded-lg"), "{cls}");
        }
    }
}
