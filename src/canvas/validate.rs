//! Batch-shape and op-application validation for `canvas.apply`.
//!
//! [`ops_shape`] is the pre-lock gate — pure shape questions (batch size, id
//! charset, ink point caps) answerable without the document. [`apply_ops`]
//! mutates the guarded document in place and enforces the post-state cap
//! (`MAX_SHAPES`); on any error the caller drops its guard without
//! committing, so a rejected batch never half-lands on disk.

use aleph_protocol::canvas::{
    check_title, CanvasDoc, CanvasOp, Shape, MAX_OPS_PER_APPLY, MAX_SHAPES,
};

use super::store::CanvasError;

/// Upper bound on `[x, y, pressure]` points in one Ink stroke. Store-local,
/// not wire contract: at the Panel's pointer-sample rate this is minutes of
/// continuous drawing in a single stroke, and the bound is what keeps one op
/// from smuggling an unbounded payload past `MAX_OPS_PER_APPLY` (the
/// dimension of that cap is ops, not bytes — CWE-400 lesson).
pub(super) const MAX_INK_POINTS: usize = 10_000;

/// Canvas-internal ids (canvases, shapes, decks, parents, arrow bindings)
/// are `[A-Za-z0-9_-]{1,64}`.
///
/// This charset is also what makes joining an id into a filesystem path safe
/// — no separators, no dots, no traversal — so every store method gates its
/// `id` argument through this before any `root.join(id)`.
pub(super) fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Pre-lock validation of an op batch: count cap, id charset, point caps.
pub(super) fn ops_shape(ops: &[CanvasOp]) -> Result<(), CanvasError> {
    if ops.is_empty() {
        return Err(CanvasError::Invalid(
            "apply carries no ops — an empty batch is a client bug, not a no-op".to_string(),
        ));
    }
    if ops.len() > MAX_OPS_PER_APPLY {
        return Err(CanvasError::Invalid(format!(
            "{} ops in one apply exceeds the {MAX_OPS_PER_APPLY}-op cap",
            ops.len()
        )));
    }
    for op in ops {
        match op {
            CanvasOp::UpsertShape { shape } => shape_is_well_formed(shape)?,
            CanvasOp::DeleteShape { id } | CanvasOp::DeleteDeck { id } => require_id(id)?,
            // The title is the one stored dimension a human reads, and it
            // used to be the one with no cap at all. `check_title` is the
            // shared gate — the same function `canvas.create` and the Panel's
            // rename input call, so all three refuse the same strings.
            CanvasOp::SetDocMeta { title } => {
                check_title(title).map_err(|why| CanvasError::Invalid(why.to_string()))?;
            }
            CanvasOp::UpsertDeck { deck } => {
                require_id(&deck.id)?;
                for frame_id in &deck.frame_ids {
                    require_id(frame_id)?;
                }
            }
        }
    }
    Ok(())
}

/// Apply a validated batch to the guarded document, in place.
///
/// Upsert is replace-in-place by id (or append); delete of a missing id is a
/// no-op — a second client may legitimately have deleted it first, and
/// failing the whole batch for that would punish the concurrent editor this
/// protocol exists to serve. The post-state shape cap rejects the WHOLE
/// batch; the caller must then drop its guard uncommitted.
pub(super) fn apply_ops(doc: &mut CanvasDoc, ops: &[CanvasOp]) -> Result<(), CanvasError> {
    // Compute the post-state shape count BEFORE mutating so a cap
    // violation can be rejected without leaving `doc` in a half-
    // applied state. The previous shape checked the cap after the
    // in-place mutations and returned Err, but the caller (which
    // discards its lock on Err) could not roll the half-applied
    // state back, so a rejected batch would persist partial edits
    // through the guard's drop.
    //
    // The count is taken by simulating the batch on a side set of ids.
    // The older shape folded over `ops` and re-checked membership against
    // `doc.shapes` for every step — but `doc.shapes` is the ORIGINAL
    // doc, never the accumulating state, so the fold mis-counts when
    // the batch targets the same id twice: a duplicate `UpsertShape`
    // of a NEW id is counted as two additions (off by +1), and a
    // duplicate `DeleteShape` of an existing id is counted as two
    // removals (off by -1). A doc sitting at `MAX_SHAPES - 1` with a
    // batch `[UpsertShape s_new, UpsertShape s_new]` was rejected as
    // `MAX_SHAPES + 1` when the actual outcome is exactly `MAX_SHAPES`.
    let mut simulated: std::collections::HashSet<&str> =
        doc.shapes.iter().map(|s| s.id()).collect();
    for op in ops {
        match op {
            CanvasOp::UpsertShape { shape } => {
                simulated.insert(shape.id());
            }
            CanvasOp::DeleteShape { id } => {
                simulated.remove(id.as_str());
            }
            _ => {}
        }
    }
    let post_shape_count = simulated.len();
    if post_shape_count > MAX_SHAPES {
        return Err(CanvasError::Invalid(format!(
            "{post_shape_count} shapes would exceed the {MAX_SHAPES}-shape document cap"
        )));
    }
    for op in ops {
        match op {
            CanvasOp::UpsertShape { shape } => {
                match doc.shapes.iter_mut().find(|s| s.id() == shape.id()) {
                    Some(slot) => *slot = shape.clone(),
                    None => doc.shapes.push(shape.clone()),
                }
            }
            CanvasOp::DeleteShape { id } => doc.shapes.retain(|s| s.id() != id),
            CanvasOp::SetDocMeta { title } => doc.title.clone_from(title),
            CanvasOp::UpsertDeck { deck } => match doc.decks.iter_mut().find(|d| d.id == deck.id) {
                Some(slot) => *slot = deck.clone(),
                None => doc.decks.push(deck.clone()),
            },
            CanvasOp::DeleteDeck { id } => doc.decks.retain(|d| &d.id != id),
        }
    }
    Ok(())
}

fn shape_is_well_formed(shape: &Shape) -> Result<(), CanvasError> {
    let common = shape.common();
    require_id(&common.id)?;
    if let Some(parent) = &common.parent_id {
        require_id(parent)?;
    }
    match shape {
        Shape::Ink { points, .. } if points.len() > MAX_INK_POINTS => {
            return Err(CanvasError::Invalid(format!(
                "{} ink points exceeds the {MAX_INK_POINTS}-point stroke cap",
                points.len()
            )));
        }
        Shape::Arrow { start, end, .. } => {
            for bind in [&start.bind, &end.bind].into_iter().flatten() {
                require_id(bind)?;
            }
        }
        _ => {}
    }
    // Reject non-canonical `asset_id` shapes up front — a model that emits
    // `asset_id = "../escape.png"` (or any other string that does not
    // match the canonical `<sha256-hex>.<ext>` shape the store mints) would
    // otherwise commit the garbage verbatim. `read_asset` rejects the same
    // ids at read time, but rejecting at upsert prevents the orphan sweep
    // from getting confused by dangling references in the first place.
    for asset_id in shape.asset_ids() {
        if super::assets::parse_asset_id(asset_id).is_none() {
            return Err(CanvasError::Invalid(format!(
                "invalid asset_id {asset_id:?}: expected <sha256-hex>.<ext> with a whitelisted extension"
            )));
        }
    }
    Ok(())
}

fn require_id(id: &str) -> Result<(), CanvasError> {
    if is_valid_id(id) {
        Ok(())
    } else {
        Err(CanvasError::Invalid(format!(
            "invalid canvas id {id:?}: expected [A-Za-z0-9_-], 1..=64 chars"
        )))
    }
}
