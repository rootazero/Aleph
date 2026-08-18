//! The mechanics every keyboard-walkable list in this crate shares: where the
//! highlight goes on ↑/↓, how the lit row is brought into view, and whether a
//! scroll well still has rows below the fold.
//!
//! # Why these four live together and not in one of the pickers
//!
//! Four surfaces walk a list with the arrow keys — the settings preset
//! disclosure ([`super::preset_picker`]), the ⌘K palette
//! ([`super::command_palette`]), the chat model popover
//! ([`super::model_picker`]) and the phone providers screen. Written per
//! surface, "where does ↓ leave the highlight" gets four answers and nobody
//! compares them; the palette is the proof. It bumped an unbounded counter on
//! ↓ and clamped only at the point of *use*, so after pressing ↓ past the end
//! the highlight sat on the last row while the counter kept climbing — ↑ then
//! did nothing visible until it had been pressed as many times as ↓ had been.
//! Nothing was ever mis-fired, so no test and no bug report could have found
//! it; it simply read as a dead key.
//!
//! [`step_highlight`] is therefore the only thing in this crate that moves a
//! highlight, and it clamps **on write**. A caller may still clamp on read (a
//! list can shrink between the keypress and the render), but because both
//! clamps are this one function they cannot disagree.
//!
//! # The scroll well
//!
//! [`publish_more_below`] answers "does this list continue below the fold",
//! which is the only thing the bottom fade is allowed to depend on. The fade is
//! conditional rather than permanent because an affordance that says "there is
//! more below" while sitting at the end of a list is worse than none: a reader
//! who catches it lying once stops reading it.

use leptos::html::ElementType;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// Move a highlight by `delta`, clamped to `[0, len - 1]`.
///
/// Returns 0 for an empty list: there is no row to light, and every caller
/// checks emptiness before indexing anyway. Passing `delta == 0` is the
/// read-side clamp — "pull this index back into a list that may have shrunk".
#[must_use]
pub(crate) fn step_highlight(len: usize, cur: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    cur.saturating_add_signed(delta).min(len - 1)
}

/// DOM id of row `index` in the list named `list`.
///
/// The `list` namespace is not defensive padding: the settings disclosure and
/// the chat model popover are mounted by different routes today, but "no two of
/// these are ever on screen together" is a fact about routing that neither file
/// can check, and a stale `getElementById` hit scrolls the *wrong* list with no
/// error. One `&'static str` per call site makes the separation structural.
#[must_use]
pub(crate) fn row_dom_id(list: &str, index: usize) -> String {
    format!("aleph-{list}-row-{index}")
}

/// Bring row `index` of `list` into view. Best-effort: a missing element simply
/// means the list re-rendered underneath us, which the next keypress fixes.
///
/// `block: nearest` scrolls the row's own scroll well and stops. The
/// argument-less overload aligns to an edge and walks **every** scrollable
/// ancestor, so each ArrowDown would also jerk the panel the list sits inside.
pub(crate) fn scroll_row_into_view(list: &str, index: usize) {
    let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(&row_dom_id(list, index)))
    else {
        return;
    };
    let opts = web_sys::ScrollIntoViewOptions::new();
    opts.set_block(web_sys::ScrollLogicalPosition::Nearest);
    el.scroll_into_view_with_scroll_into_view_options(&opts);
}

/// Slack below the viewport at which a scroll well is treated as having more
/// rows. One pixel, not zero: a fractional device-pixel layout leaves
/// `scrollHeight - scrollTop - clientHeight` sitting at 0.5 when the well is
/// scrolled fully to the bottom, and a bare `> 0` would then claim there is
/// more to see forever.
const MORE_BELOW_SLACK_PX: i32 = 1;

/// Whether a scroll well still has content below its visible bottom edge.
///
/// The predicate behind the bottom fade, kept separate from the fade because
/// the interesting property is that it is *conditional*. A permanent bottom
/// mask is one CSS line and is what most of the web ships, but it dims the
/// final row of a list that has already ended — an affordance that says "there
/// is more below" when there is not is worse than none, because the reader
/// learns to ignore it.
///
/// **Private on purpose.** Calling it from outside means having read the
/// geometry yourself, and doing that from the deferred callback where the
/// measurement belongs is the panic [`publish_more_below`] exists to prevent.
/// Module privacy makes "go through the safe wrapper" a compile error rather
/// than a convention someone has to have been told about.
#[must_use]
fn has_more_below(scroll_top: i32, client_height: i32, scroll_height: i32) -> bool {
    scroll_height - scroll_top - client_height > MORE_BELOW_SLACK_PX
}

/// Measure `list` and publish whether it still has rows below its bottom edge.
///
/// The only way this crate answers that question, and deliberately
/// **deferred-safe**: every caller runs it from `request_animation_frame`,
/// because a Leptos effect is queued off the render pass and a synchronous
/// `scrollHeight` there would describe the list as it was *before* the change
/// that scheduled the measurement. One frame is long enough for the component
/// to have unmounted — a route change, a closed drawer — and
/// `NodeRef::get_untracked` **unwraps**, so a plain read would panic on a
/// disposed ref and take the page to the recovery overlay.
///
/// That is the hazard [`crate::disposed_reads`] guards inside `spawn_local`, in
/// a block shape its scanner cannot see: the callback is a named closure
/// defined elsewhere, so no textual rule reaches it. Making the measurement
/// itself the only spelling is the structural answer — `try_…().flatten()` is
/// behaviourally identical while the ref is live, and the write side is already
/// a silent no-op on a disposed signal.
///
/// A missing element publishes `false`: a closed disclosure must not be left
/// claiming there is more to see.
pub(crate) fn publish_more_below<E>(list: NodeRef<E>, flag: RwSignal<bool>)
where
    E: ElementType + 'static,
    E::Output: JsCast + Clone + AsRef<web_sys::Element> + 'static,
{
    let Some(el) = list.try_get_untracked().flatten() else {
        flag.set(false);
        return;
    };
    let el: &web_sys::Element = el.as_ref();
    flag.set(has_more_below(
        el.scroll_top(),
        el.client_height(),
        el.scroll_height(),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_past_the_end_stays_on_the_last_row() {
        assert_eq!(step_highlight(3, 2, 1), 2);
    }

    #[test]
    fn stepping_before_the_first_row_stays_on_the_first() {
        assert_eq!(step_highlight(3, 0, -1), 0);
    }

    #[test]
    fn stepping_walks_one_row_at_a_time() {
        assert_eq!(step_highlight(5, 1, 1), 2);
        assert_eq!(step_highlight(5, 3, -1), 2);
    }

    #[test]
    fn stepping_an_empty_list_is_zero() {
        assert_eq!(step_highlight(0, 4, 1), 0);
        assert_eq!(step_highlight(0, 0, -1), 0);
    }

    #[test]
    fn a_shrinking_list_pulls_the_highlight_back_in_range() {
        // The query narrowed the list under a highlight that was past its new
        // end; a delta of 0 is the read-side clamp Enter performs.
        assert_eq!(step_highlight(2, 9, 0), 1);
    }

    /// The palette's original defect, stated as a property: pressing ↓ past the
    /// end and then ↑ once must move the highlight. With an unbounded counter
    /// it does not — it takes as many ↑ as there were surplus ↓.
    #[test]
    fn arrow_up_moves_immediately_after_over_pressing_arrow_down() {
        let len = 4;
        let mut cur = 0;
        for _ in 0..20 {
            cur = step_highlight(len, cur, 1);
        }
        assert_eq!(cur, len - 1);
        assert_eq!(step_highlight(len, cur, -1), len - 2);
    }

    #[test]
    fn row_ids_are_namespaced_per_list() {
        assert_eq!(row_dom_id("picker", 3), "aleph-picker-row-3");
        assert_ne!(row_dom_id("picker", 3), row_dom_id("palette", 3));
    }

    #[test]
    fn a_well_scrolled_to_its_bottom_has_nothing_below() {
        // 400px of content in a 300px well, scrolled the full 100px.
        assert!(!has_more_below(100, 300, 400));
    }

    #[test]
    fn a_well_at_its_top_with_overflow_has_more_below() {
        assert!(has_more_below(0, 300, 400));
    }

    #[test]
    fn a_well_shorter_than_its_box_has_nothing_below() {
        // Six rows in a max-h-96 well: scrollHeight == clientHeight.
        assert!(!has_more_below(0, 384, 384));
    }

    #[test]
    fn a_sub_pixel_remainder_is_not_more_content() {
        // Fractional layout leaves a 1px residue at the true bottom.
        assert!(!has_more_below(99, 300, 400));
    }
}
