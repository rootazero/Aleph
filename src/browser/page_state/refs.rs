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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefEntry {
    pub key: RefKey,
    pub minted_generation: u64,
}

/// Per-tab ref state. Lives in `EngineHandle`'s `TabEntry`.
pub struct RefTable {
    /// The main frame's loader id — which document these refs belong to.
    document: Option<String>,
    /// The main frame's **id**, recorded so a ref can be told apart from a ref
    /// in some OTHER frame of the same page.
    ///
    /// Separate from [`Self::document`] because the two change on different
    /// events: a navigation gives the main frame a new LOADER and keeps its
    /// frame id, which is why `reset_for_document` does not take this and
    /// `PageState::build` sets it instead — build has `frames[0]` in hand,
    /// which is the main frame by definition.
    main_frame_id: Option<String>,
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
            main_frame_id: None,
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
    pub fn reset_for_document(&mut self, main_loader_id: &str) {
        if self.document.as_deref() == Some(main_loader_id) {
            return;
        }
        self.by_key.clear();
        self.by_id.clear();
        self.retired_below = self.next;
        self.document = Some(main_loader_id.to_string());
    }

    #[must_use]
    pub fn document(&self) -> Option<&str> {
        self.document.as_deref()
    }

    /// Record which frame is the main one. Called by `PageState::build` on
    /// every capture, from `raw.frames[0]`.
    pub fn set_main_frame(&mut self, frame_id: &str) {
        if self.main_frame_id.as_deref() != Some(frame_id) {
            self.main_frame_id = Some(frame_id.to_string());
        }
    }

    /// The main frame's id, or `None` before any capture has happened.
    ///
    /// A ref whose [`RefKey::frame_id`] differs from this is a ref in another
    /// document of the same page — an `<iframe>`. For a same-renderer page that
    /// is only bookkeeping; for an out-of-process frame the id lives in a
    /// DIFFERENT node space, where resolving it against this page's session is
    /// meaningless (判据: an id outside its space carries an implicit namespace,
    /// which is no namespace at all).
    #[must_use]
    pub fn main_frame_id(&self) -> Option<&str> {
        self.main_frame_id.as_deref()
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
