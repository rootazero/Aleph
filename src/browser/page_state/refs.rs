//! Ref identity and generations (spec §4.3).
//!
//! A ref is a promise: `e12` means the same element it meant last snapshot.
//! Two rules keep it honest — the same key always mints the same number within
//! a document, and a number is **never reissued** after a navigation. The
//! second is what stops a stale `e12` from silently addressing whatever now
//! happens to carry that backend node id.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// `"e{n}"` — the token the model sees and hands back.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RefId(pub String);

/// Whether `candidate` has the shape this table MINTS (`e` + decimal digits,
/// [`RefTable::mint`]).
///
/// Not "is it in the table" — that is `resolve`'s job and its answer is
/// staleness. This separates a THIRD case the two used to share a spelling: a
/// string that was never a ref of ours at all. The playwright-cli driver
/// accepts a CSS selector as a `ref_id`, so a `browser_exec` script written
/// against that driver arrives here with `#go`, and answering "stale, re-run
/// browser_snapshot" sends the model to fetch a ref no snapshot will ever mint
/// — it re-snapshots, fails again, and spends its budget in a loop
/// (判据 §8: "I do not recognise this" must not be reported as "this expired").
#[must_use]
pub fn is_minted_shape(candidate: &str) -> bool {
    let Some(digits) = candidate.strip_prefix('e') else {
        return false;
    };
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

impl std::fmt::Display for RefId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a ref actually points at.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RefKey {
    pub frame_id: String,
    pub loader_id: String,
    pub backend_node_id: u64,
}

/// The document half of a [`RefKey`], carried on every state node so the
/// driver knows which session to resolve against.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameKey {
    pub frame_id: String,
    pub loader_id: String,
}

/// Why a ref no longer resolves. Three facts, not two: the model's next move
/// differs for each.
///
/// **Defined by Task 8 in `crate::browser::error`, not here** — that is where
/// `BrowserError::StaleRef { ref_id, reason }` needs it, and Task 8 lands
/// before this module exists. Re-exported rather than redeclared: two enums
/// with the same three variants is the same fact written twice, and the one
/// that drifts is always the copy nobody is converting through (判据 §1).
pub use crate::browser::error::StaleReason;

/// Which renderer a ref's document was in when it was last seen — or that it
/// was not seen at all.
///
/// Three states, not two, because "the last capture did not describe this
/// document" is a different fact from "it was in the page's renderer", and
/// reading the first as the second is how a stale ref becomes a click on
/// whatever now carries its `backendNodeId` (判据 §8: an unknown may only say
/// "I do not know").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameVerdict {
    /// The page's own renderer. Its ids resolve against the page's session.
    PageRenderer,
    /// A renderer of its own. Its ids are meaningless to the page's session.
    OtherRenderer,
    /// The last capture did not contain this document.
    ///
    /// Stale by construction: refs are only minted from a capture, so a ref
    /// whose document the latest capture did not see is one whose frame has
    /// navigated or gone. Deliberately NOT split into "navigated" and "gone" —
    /// the two are distinguishable with another lookup, and the model's move is
    /// the same either way, so the distinction would be a label with no
    /// consumer.
    Unseen,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefEntry {
    pub key: RefKey,
    pub minted_generation: u64,
}

/// Per-tab ref state. Lives in `EngineHandle`'s `TabEntry`.
pub struct RefTable {
    /// The main frame's loader id — which document these refs belong to.
    document: Option<String>,
    /// Every DOCUMENT the last capture saw — keyed by `(frame_id, loader_id)`,
    /// valued by whether it was in a renderer of its own.
    ///
    /// ⚠️ Keyed by the document, not by the frame, and that is the fix for a
    /// staleness hole rather than a tidy-up. A ref outlives its capture: a
    /// subframe navigation clears nothing (`reset_for_document` only fires when
    /// the MAIN loader changes), so a ref minted while frame `F` was
    /// out-of-process still resolves after `F` navigates same-site and becomes
    /// in-process. Keyed by frame alone, `F` would then look safe and a
    /// renderer-2 `backendNodeId` would be resolved against the page's session
    /// — the wrong-element click that reports success. The ref carries the
    /// loader it was minted against (`RefKey::loader_id`), so the pair is the
    /// discriminator and it was already in hand.
    ///
    /// ⚠️ It also replaced a `main_frame_id`, which was a WIDER predicate than
    /// the hazard: a `backendNodeId` is scoped to a RENDERER, not a frame, so a
    /// same-origin `<iframe>`, a `srcdoc` or an `about:blank` — separate
    /// `frameId`s inside the page's own renderer — resolve perfectly well
    /// against the page's session, and refusing them took away something that
    /// worked (判据 §5).
    ///
    /// Empty before the first capture, which [`FrameVerdict::Unseen`] reads as
    /// "I do not know", never as permission.
    captured_documents: std::collections::HashMap<FrameKey, bool>,
    /// Next number to hand out. **Monotonic for the life of the tab**, across
    /// navigations: that is what makes "never reissued" true.
    next: u64,
    /// Every number handed out before the current document began. `resolve`
    /// answers `Navigated` for these instead of `Unknown`.
    retired_below: u64,
    by_key: HashMap<RefKey, RefId>,
    by_id: HashMap<RefId, RefEntry>,
}

impl RefTable {
    #[must_use]
    pub fn new() -> Self {
        Self {
            document: None,
            captured_documents: std::collections::HashMap::new(),
            next: 1,
            retired_below: 1,
            by_key: HashMap::new(),
            by_id: HashMap::new(),
        }
    }

    /// The id for `key`, minting one the first time it is seen in this
    /// document. Idempotent, and the second call does NOT update
    /// `minted_generation`: the recorded generation is the ref's age, which is
    /// what a caller wants to know.
    pub fn mint(&mut self, key: &RefKey, generation: u64) -> RefId {
        if let Some(existing) = self.by_key.get(key) {
            return existing.clone();
        }
        let id = RefId(format!("e{}", self.next));
        self.next += 1;
        self.by_key.insert(key.clone(), id.clone());
        self.by_id.insert(
            id.clone(),
            RefEntry {
                key: key.clone(),
                minted_generation: generation,
            },
        );
        id
    }

    /// What `r` points at, or why it does not.
    ///
    /// # Which variants this function can actually return
    ///
    /// `Navigated` and `Unknown` — **never `NodeGone`**, and the signature
    /// cannot say so. This table only knows what it minted and what it retired;
    /// "the document is the same and the node has left it" is a fact only the
    /// driver learns, from `DOM.resolveNode` failing on a ref this function
    /// just resolved happily (Task 12/14). A caller writing a `match` with a
    /// `NodeGone` arm against THIS function has written an arm nothing can
    /// reach (判据 §2); a caller matching on a `BrowserError::StaleRef` coming
    /// back from the driver needs all three.
    pub fn resolve(&self, r: &RefId) -> Result<RefEntry, StaleReason> {
        if let Some(entry) = self.by_id.get(r) {
            return Ok(entry.clone());
        }
        match parse_ref_number(&r.0) {
            Some(n) if n < self.retired_below => Err(StaleReason::Navigated),
            _ => Err(StaleReason::Unknown),
        }
    }

    /// Point the table at `main_loader_id`, clearing it if that is a different
    /// document. `PageState::build` calls this on every capture, so the
    /// same-document case must be free.
    ///
    /// Answers whether the document actually CHANGED. The bool exists for the
    /// event pump, which has to report whether folding an event in altered any
    /// state, and the alternative was for the pump to compare
    /// [`Self::document`] itself — a second copy of the rule "the same loader
    /// is not a new document", free to drift from this one (判据 §1). Callers
    /// that only want the effect may ignore it.
    pub fn reset_for_document(&mut self, main_loader_id: &str) -> bool {
        if self.document.as_deref() == Some(main_loader_id) {
            return false;
        }
        self.by_key.clear();
        self.by_id.clear();
        self.retired_below = self.next;
        self.document = Some(main_loader_id.to_string());
        true
    }

    #[must_use]
    pub fn document(&self) -> Option<&str> {
        self.document.as_deref()
    }

    /// Record every document this capture saw and which renderer it was in.
    /// Called by `PageState::build` on every capture, from
    /// [`super::RawFrame::separate_renderer`].
    ///
    /// **Replaced wholesale, not merged.** A frame that stopped being
    /// out-of-process must stop being refused, and a map that only ever grew
    /// would keep refusing it — a stale entry here is a capability that never
    /// comes back (判据 §5). Wholesale replacement is also what turns a
    /// navigated subframe's old document into an [`FrameVerdict::Unseen`]
    /// rather than leaving it looking current.
    pub fn set_captured_documents<I: IntoIterator<Item = (FrameKey, bool)>>(&mut self, docs: I) {
        self.captured_documents = docs.into_iter().collect();
    }

    /// Which renderer this ref's document was in, as of the last capture.
    ///
    /// The question a driver must ask before resolving a `backendNodeId`
    /// against the page's session: a renderer is one node space, so ids from
    /// any document the page's own capture produced are meaningful there, and
    /// ids from another renderer collide silently — measured, the page session
    /// answers such an id with a DIFFERENT node and the geometry call then
    /// succeeds on that one, so "the call worked" can never stand in for a
    /// node-space check.
    #[must_use]
    pub fn renderer_of(&self, key: &RefKey) -> FrameVerdict {
        let doc = FrameKey {
            frame_id: key.frame_id.clone(),
            loader_id: key.loader_id.clone(),
        };
        match self.captured_documents.get(&doc) {
            Some(false) => FrameVerdict::PageRenderer,
            Some(true) => FrameVerdict::OtherRenderer,
            None => FrameVerdict::Unseen,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// `#[derive(Default)]` would give `next: 0` and `retired_below: 0` — the
/// first ref would be `e0`, and after a navigation `resolve("e0")` would answer
/// `Unknown` instead of `Navigated`. Every construction site uses `new()`
/// today, which is exactly why the trap is worth closing now: nothing would go
/// red. Same shape as the `BrowserSystemConfig` derive Task 9 pins.
impl Default for RefTable {
    fn default() -> Self {
        Self::new()
    }
}

/// `"e12"` → `Some(12)`. Anything else → `None`, which `resolve` reads as
/// "never minted here" rather than as any kind of staleness.
fn parse_ref_number(s: &str) -> Option<u64> {
    s.strip_prefix('e')?.parse().ok()
}

#[cfg(test)]
mod tests {
    /// The third case, which used to share a spelling with "stale".
    ///
    /// Both directions, and the negatives are the ones that matter: the other
    /// driver takes a CSS selector as a `ref_id`, so `#go` and `.btn` really do
    /// arrive here, and calling them stale sends the model to re-snapshot for
    /// something no snapshot can mint.
    #[test]
    fn a_minted_ref_is_told_apart_from_a_string_that_was_never_one() {
        for minted in ["e0", "e1", "e12", "e99999"] {
            assert!(
                super::is_minted_shape(minted),
                "{minted} is exactly what mint() produces"
            );
        }
        for foreign in [
            "#go",  // a CSS id selector — the measured case
            ".btn", // a class selector
            "e",    // the prefix with no ordinal
            "",     // nothing at all
            "E1",   // the wrong case
            "e1x",  // trailing junk
            "1",    // the ordinal without the prefix
            "button[ref=e1]",
        ] {
            assert!(
                !super::is_minted_shape(foreign),
                "{foreign:?} is not a ref this table mints"
            );
        }
        // Derived, not assumed: whatever `mint` produces must be recognised by
        // the recogniser, or the two can drift apart silently (判据 §1).
        let mut table = super::RefTable::new();
        let id = table.mint(&key("F1", "L1", 7), 1);
        assert!(
            super::is_minted_shape(&id.0),
            "mint() produced {id:?}, which the recogniser rejects"
        );
    }

    use super::*;

    fn key(frame: &str, loader: &str, node: u64) -> RefKey {
        RefKey {
            frame_id: frame.to_string(),
            loader_id: loader.to_string(),
            backend_node_id: node,
        }
    }

    /// The same element keeps its number across snapshots of the same
    /// document — spec §4.3, and the whole point: the model must not have to
    /// relearn the numbering after every capture.
    #[test]
    fn the_same_key_mints_the_same_id_forever_within_one_document() {
        let mut t = RefTable::new();
        t.reset_for_document("L-1");
        let a = t.mint(&key("F", "L-1", 42), 1);
        let b = t.mint(&key("F", "L-1", 43), 1);
        assert_eq!(a, RefId("e1".into()));
        assert_eq!(b, RefId("e2".into()));
        // A later generation of the SAME document re-mints the same ids.
        assert_eq!(t.mint(&key("F", "L-1", 42), 9), RefId("e1".into()));
        assert_eq!(t.mint(&key("F", "L-1", 43), 9), RefId("e2".into()));
        assert_eq!(t.len(), 2);
        // The recorded generation is the FIRST one — the ref's age, which is
        // what `resolve` reports back to the caller.
        assert_eq!(t.resolve(&a).expect("live ref").minted_generation, 1);
    }

    /// A navigation empties the table, and the ids it handed out before come
    /// back as `Navigated` — not as "unknown", and above all not as a live ref
    /// pointing at whatever now carries that backend node id.
    #[test]
    fn a_new_document_retires_every_ref_and_never_reissues_its_number() {
        let mut t = RefTable::new();
        t.reset_for_document("L-1");
        let old = t.mint(&key("F", "L-1", 42), 1);
        assert!(t.resolve(&old).is_ok());

        t.reset_for_document("L-2");
        assert_eq!(t.len(), 0);
        assert_eq!(t.document(), Some("L-2"));
        assert_eq!(t.resolve(&old), Err(StaleReason::Navigated));

        // The number is never reused, so a stale ref can never silently
        // address a DIFFERENT element of the new page.
        let fresh = t.mint(&key("F", "L-2", 42), 2);
        assert_ne!(fresh, old);
        assert_eq!(fresh, RefId("e2".into()));
    }

    /// A ref that was never minted is `Unknown`, not `Navigated`. Two facts,
    /// two next moves: one says "re-snapshot", the other says "you made that up".
    #[test]
    fn a_ref_that_never_existed_is_unknown_not_navigated() {
        let mut t = RefTable::new();
        t.reset_for_document("L-1");
        t.mint(&key("F", "L-1", 1), 1);
        assert_eq!(t.resolve(&RefId("e99".into())), Err(StaleReason::Unknown));
        assert_eq!(
            t.resolve(&RefId("banana".into())),
            Err(StaleReason::Unknown)
        );
    }

    /// Re-asserting the SAME document must not clear anything. `build` calls
    /// this on every capture, so a reset that fired unconditionally would
    /// renumber the page on every snapshot.
    #[test]
    fn resetting_to_the_same_document_is_a_no_op() {
        let mut t = RefTable::new();
        t.reset_for_document("L-1");
        let a = t.mint(&key("F", "L-1", 7), 1);
        t.reset_for_document("L-1");
        assert_eq!(t.len(), 1);
        assert!(t.resolve(&a).is_ok());
    }
}
