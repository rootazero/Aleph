//! Whiteboard canvas UI state — shared by the wide library page / editor
//! (`views/canvas/`) and read-only by nothing else yet.
//!
//! Provided once in `app.rs::AppContent` (never by a phone screen — the
//! `context_ownership` guard's rule). All fields are `RwSignal`s and the
//! struct is `Copy`, matching `ChatState` / `MemoryState`: the library page,
//! the editor shell (Task 12+) and the realtime reconciler (Task 17) all
//! read the same signals instead of threading props.
//!
//! The document itself ([`CanvasDoc`]) is the *server's* shape — this module
//! deliberately owns no second document model. What lives here is the part
//! the server must not know: which canvas this client is looking at, its
//! camera, its active tool, and its in-flight error/conflict flags.

use aleph_protocol::canvas::{CanvasDoc, CanvasEnvelope, CanvasOp, CanvasRow, GeoForm};
use leptos::prelude::*;

/// The optimistic `canvas.apply` batch currently on the wire.
///
/// Published by the editor's send pump (`views/canvas/editor.rs::pump`) for
/// exactly one reader: the `canvas.updated` reconciler in
/// `views/canvas/mod.rs`, which must tell this batch's broadcast **echo**
/// (revision `base_revision + 1`, same ops) from a foreign batch that won
/// the race to the same revision. It is a projection of the editor's
/// `SendQueue::inflight` — republished here because the queue is a
/// `StoredValue` scoped inside the editor component, which the view shell
/// (where the frame handler lives) cannot reach. The pump is the only
/// writer besides [`CanvasState::close_canvas`].
///
/// The ops are stored verbatim rather than hashed: equality against the
/// frame's ops is exact (serde_json round-trips finite `f64` losslessly and
/// every other field is discrete), a batch is small, and a hash would only
/// add collision reasoning.
#[derive(Debug, Clone, PartialEq)]
pub struct InflightBatch {
    /// The `base_revision` the batch was sent against; its echo carries
    /// `base_revision + 1`.
    pub base_revision: u64,
    /// The batch ops, byte-for-byte what went on the wire.
    pub ops: Vec<CanvasOp>,
}

/// The editor's active tool.
///
/// Defined here (not in `views/canvas/`) because the toolbar writes it and
/// the pointer state machine (Task 13) reads it — two sibling subtrees.
/// `Geo` carries which geometric form the next drag creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CanvasTool {
    #[default]
    Select,
    Pan,
    Draw,
    Geo(GeoForm),
    Note,
    Text,
    Frame,
    Arrow,
}

/// Viewport camera: world-space offset plus zoom.
///
/// `x`/`y` are the world coordinates at the viewport's top-left corner;
/// `zoom` is screen-pixels-per-world-unit. The pure screen↔world math over
/// this struct arrives with the editor shell (Task 12, `views/canvas/`);
/// the struct lives here so the state can hold one before the math exists.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            zoom: 1.0,
        }
    }
}

/// All whiteboard UI state. `Copy` — hand it around freely.
#[derive(Clone, Copy)]
pub struct CanvasState {
    /// Currently open canvas id; `None` renders the library page.
    pub open_canvas: RwSignal<Option<String>>,
    /// Library listing (visible slice, server-filtered).
    pub rows: RwSignal<Vec<CanvasRow>>,
    /// Whether `canvas.list` has answered at least once this session.
    ///
    /// Two consumers need it and neither can derive it: an empty `rows` is
    /// both "you have no canvases" and "nobody has asked yet", and rendering
    /// the first sentence for the second state tells the user their library
    /// is empty before the question has been put. (The phone screen already
    /// carried a local `loaded` flag for exactly this; the wide surface did
    /// not, and said "No canvases yet" during every cold load.) Set by the
    /// loader in `views/canvas/mod.rs` — including on failure, where "we
    /// asked and it went wrong" is reported by `load_error`, not by
    /// pretending the question is still open.
    pub rows_loaded: RwSignal<bool>,
    /// The open document, as last fetched or reconciled.
    pub doc: RwSignal<Option<CanvasDoc>>,
    /// Capability base URL for `<image href>` — `{asset_base}/{asset_id}`.
    /// Minted per `canvas.get`; stale after its TTL, refreshed by refetch.
    pub asset_base: RwSignal<Option<String>>,
    /// This client's selection (shape ids). Pushed to the server debounced
    /// (Task 13) so the model can answer "what did the user select".
    pub selection: RwSignal<Vec<String>>,
    /// Active editor tool.
    pub tool: RwSignal<CanvasTool>,
    /// Viewport camera. Per-device UI preference — never synced (spec §6).
    pub camera: RwSignal<Camera>,
    /// True while an optimistic apply has been refused with a revision
    /// conflict and the replay (refetch + reapply, Task 13) is in flight.
    pub pending_conflict: RwSignal<bool>,
    /// The apply batch currently on the wire — the reconciler's echo
    /// detector (see [`InflightBatch`]). Written only by the editor's send
    /// pump and by [`Self::close_canvas`].
    pub inflight: RwSignal<Option<InflightBatch>>,
    /// Last load failure, already classified for display
    /// (`admin_refusal::settings_load_error` framing).
    pub load_error: RwSignal<Option<String>>,
}

impl CanvasState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            open_canvas: RwSignal::new(None),
            rows: RwSignal::new(Vec::new()),
            rows_loaded: RwSignal::new(false),
            doc: RwSignal::new(None),
            asset_base: RwSignal::new(None),
            selection: RwSignal::new(Vec::new()),
            tool: RwSignal::new(CanvasTool::default()),
            camera: RwSignal::new(Camera::default()),
            pending_conflict: RwSignal::new(false),
            inflight: RwSignal::new(None),
            load_error: RwSignal::new(None),
        }
    }

    /// Land a `canvas.get` envelope: document, live selection and the freshly
    /// minted asset base, in one place.
    ///
    /// One writer for three signals on purpose — a call site that set `doc`
    /// but forgot `asset_base` would render every image against an expired
    /// capability, and nothing would say so.
    pub fn adopt_envelope(&self, envelope: CanvasEnvelope) {
        self.selection.set(envelope.selection);
        self.asset_base.set(envelope.asset_base);
        self.doc.set(Some(envelope.canvas));
        self.pending_conflict.set(false);
    }

    /// Back to the library: drop the open document and everything scoped to
    /// it. `rows` survives — the library list is not per-document state.
    pub fn close_canvas(&self) {
        self.open_canvas.set(None);
        self.doc.set(None);
        self.asset_base.set(None);
        self.selection.set(Vec::new());
        self.pending_conflict.set(false);
        // A batch still on the wire belongs to the document being left; kept,
        // it could shadow-match a frame of the next canvas opened.
        self.inflight.set(None);
    }
}

impl Default for CanvasState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::CanvasDoc;

    fn doc(id: &str, revision: u64) -> CanvasDoc {
        CanvasDoc {
            id: id.to_string(),
            title: "t".to_string(),
            owner_user_id: None,
            project_id: None,
            revision,
            shapes: Vec::new(),
            decks: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    /// The envelope lands as one unit: doc, selection and asset base all
    /// move together, and a pending conflict is cleared — a fresh fetch IS
    /// the recovery the flag was waiting for.
    #[test]
    fn adopt_envelope_writes_doc_selection_and_asset_base_together() {
        let state = CanvasState::new();
        state.pending_conflict.set(true);
        state.adopt_envelope(CanvasEnvelope {
            canvas: doc("cv-1", 3),
            selection: vec!["s1".to_string()],
            asset_base: Some("/canvas-asset/cap/cv-1".to_string()),
        });
        assert_eq!(
            state.doc.get_untracked().expect("doc set").revision,
            3,
            "the document must land"
        );
        assert_eq!(state.selection.get_untracked(), vec!["s1".to_string()]);
        assert_eq!(
            state.asset_base.get_untracked().as_deref(),
            Some("/canvas-asset/cap/cv-1")
        );
        assert!(
            !state.pending_conflict.get_untracked(),
            "a successful fetch resolves the conflict flag"
        );
    }

    /// Closing drops everything scoped to the open document but keeps the
    /// library rows — the list is not per-document state.
    #[test]
    fn close_canvas_drops_document_scope_but_keeps_the_library() {
        let state = CanvasState::new();
        state.rows.set(vec![]);
        state.open_canvas.set(Some("cv-1".to_string()));
        state.adopt_envelope(CanvasEnvelope {
            canvas: doc("cv-1", 1),
            selection: vec!["s1".to_string()],
            asset_base: Some("/canvas-asset/cap/cv-1".to_string()),
        });
        state.inflight.set(Some(InflightBatch {
            base_revision: 1,
            ops: Vec::new(),
        }));
        state.close_canvas();
        assert!(state.open_canvas.get_untracked().is_none());
        assert!(state.doc.get_untracked().is_none());
        assert!(state.asset_base.get_untracked().is_none());
        assert!(state.selection.get_untracked().is_empty());
        assert!(
            state.inflight.get_untracked().is_none(),
            "an inflight record must not outlive the document it was sent for"
        );
    }
}
