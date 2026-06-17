//! Keep a keyboard-selected list row inside its scroll viewport.
//!
//! Shared by the slash-command palette and the @-mention palette: both are
//! `max-h`-capped scroll containers navigated by ↑/↓, so the highlighted row
//! must follow the selection when it leaves the visible window.

use wasm_bindgen::JsCast;

/// Scroll `container` just enough to reveal the row tagged `data-pidx="{idx}"`.
///
/// No-op when the row is already fully visible — equivalent to
/// `scrollIntoView({block: "nearest"})`, computed by hand so it needs no
/// `ScrollIntoViewOptions` web-sys feature. `offset_top` is relative to the
/// container because every palette container is `position: relative` (via
/// `.glass`), making it the rows' `offsetParent`.
pub(crate) fn reveal_selected_row(container: &web_sys::HtmlElement, idx: usize) {
    let Ok(Some(row)) = container.query_selector(&format!("[data-pidx=\"{idx}\"]")) else {
        return;
    };
    let Some(row) = row.dyn_ref::<web_sys::HtmlElement>() else {
        return;
    };
    let top = row.offset_top();
    let bottom = top + row.offset_height();
    let view_top = container.scroll_top();
    let view_bottom = view_top + container.client_height();
    if top < view_top {
        container.set_scroll_top(top);
    } else if bottom > view_bottom {
        container.set_scroll_top(bottom - container.client_height());
    }
}
