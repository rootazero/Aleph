//! The three contracts a picker partition function must satisfy — one shared
//! derivation across the three catalogue pages that use it today (chat,
//! generation, search). The embedding and rerank pages are expected to join
//! later; they do not yet.
//!
//! Each corresponds to a real, possible silent failure:
//!
//! * If an empty query filters, the catalogue page can **no longer tell you
//!   which providers Aleph supports** — you would have to know a vendor's
//!   name before you could discover we support it.
//! * If a configured row disappears from the offer, search cannot find it,
//!   and once deleted it can never be configured again.
//! * If a configured row is offered without its mark, it reads as "not set up
//!   yet", and an operator would overwrite existing credentials.

use crate::components::preset_picker::PickerRow;

fn ids(rows: &[PickerRow]) -> Vec<String> {
    rows.iter().map(|r| r.id.clone()).collect()
}

/// An empty query must return **every** offerable row, in the catalogue's own
/// order.
///
/// `expected_ids` is spelled out in full rather than just a count: equal
/// counts with a different order still means a bare Enter selects the wrong
/// row.
pub fn empty_query_offers_everything(
    offer: impl Fn(&str) -> Vec<PickerRow>,
    expected_ids: &[&str],
) {
    let rows = offer("");
    assert_eq!(
        ids(&rows),
        expected_ids,
        "an empty query must offer every row, in the catalogue's own order — \
         a catalogue that only appears after you type cannot tell you what exists"
    );
}

/// A configured row is still offered, and `configured` is true.
pub fn configured_rows_stay_offered_and_marked(
    offer: impl Fn(&str) -> Vec<PickerRow>,
    configured_id: &str,
) {
    let rows = offer("");
    let row = rows
        .iter()
        .find(|r| r.id == configured_id)
        .unwrap_or_else(|| {
            panic!(
                "configured row {configured_id} vanished from the picker — \
                 search can no longer find it and deleting it would be one-way"
            )
        });
    assert!(
        row.configured,
        "row {configured_id} is offered but unmarked, which reads as 'not set up yet'"
    );
}

/// After deletion the row returns to the picker, and `configured` becomes
/// false.
///
/// `after_delete` is the **post-deletion** offer closure. Most callers build
/// it from a configured list with the row's entry removed; the chat catalogue
/// instead derives `configured` from a field on the row itself, so its
/// closure builds a catalogue entry that was simply never configured. Either
/// way, this only checks that `configured` reads false.
pub fn deleted_row_returns_to_the_picker(after_delete: impl Fn(&str) -> Vec<PickerRow>, id: &str) {
    let rows = after_delete("");
    let row = rows.iter().find(|r| r.id == id).unwrap_or_else(|| {
        panic!("deleted row {id} is unreachable — it can never be set up again")
    });
    assert!(
        !row.configured,
        "row {id} was deleted but still marked configured"
    );
}
