//! [`CanvasStore`] — the whiteboard persistence facade.
//!
//! One canvas is one directory `<root>/<id>/` holding `doc.json` (and, from
//! Task 6 on, `assets/`). Every read-modify-write goes through the
//! [`DocLocks`]/`DocGuard` module boundary in `doc_io` — the store cannot
//! spell an unlocked write. Optimistic concurrency: `apply` carries the
//! caller's `base_revision` and rejects a stale one with the current value,
//! so a lost update is structurally impossible and a conflicted client
//! re-pulls and replays.

use std::path::PathBuf;
use std::sync::Arc;

use aleph_protocol::canvas::{CanvasDoc, CanvasOp, CanvasRow};
use tracing::warn;

use super::doc_io::DocLocks;
use super::validate;
use crate::gateway::event_bus::GatewayEventBus;

/// Canvas-layer error, three-way classified (not-found / caller-fixable /
/// internal, plus the revision conflict the protocol is built around).
///
/// Deliberately NO `From<String>`: the moment `?` can auto-convert, the next
/// caller error silently lands as `Internal` — the exact misclassification
/// the three-way split exists to prevent (§4.13c appendix A).
#[derive(Debug, thiserror::Error)]
pub enum CanvasError {
    /// The addressed thing does not exist; the payload names it
    /// ("canvas cv-…" / "asset … in canvas cv-…"). Never conflated with a
    /// parse failure — "failed to parse" and "does not exist" are two
    /// different answers.
    #[error("not found: {0}")]
    NotFound(String),
    /// Caller-fixable input problem (bad id charset, oversized batch, …).
    #[error("invalid canvas request: {0}")]
    Invalid(String),
    /// Stale `base_revision`. Carries the current one so the caller can
    /// re-pull and replay without a second round trip.
    #[error("revision conflict: canvas is at revision {current_revision}")]
    Conflict { current_revision: u64 },
    /// Our side failed: I/O, serialization, or a corrupt document on disk.
    #[error("canvas store error: {0}")]
    Internal(String),
}

/// File-backed canvas store with per-canvas write locks.
pub struct CanvasStore {
    /// `pub(super)` for the `assets` sibling module, which extends this type
    /// with the asset API and needs the same root and the same locks — a
    /// second lock table would split the critical section in two.
    pub(super) root: PathBuf,
    pub(super) locks: DocLocks,
    event_bus: Option<Arc<GatewayEventBus>>,
}

impl CanvasStore {
    /// A store rooted at `root` (production: `utils::paths::get_canvas_root()`).
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            locks: DocLocks::new(),
            event_bus: None,
        }
    }

    /// Attach the gateway event bus so committed applies announce themselves
    /// (`agent_env` precedent — emitting from the store, not the handlers, is
    /// what makes every future in-process mutator announce itself too).
    #[must_use]
    pub fn with_event_bus(mut self, bus: Arc<GatewayEventBus>) -> Self {
        self.event_bus = Some(bus);
        self
    }

    /// Create a new canvas and persist it. Creation goes through the same
    /// per-canvas lock as every other write — the creation path is a
    /// read-modify-write like any other (§5.23b).
    pub async fn create(
        &self,
        title: Option<String>,
        project_id: Option<String>,
        owner_user_id: Option<String>,
    ) -> Result<CanvasDoc, CanvasError> {
        let id = format!("cv-{}", uuid::Uuid::new_v4().simple());
        let now = now_ms();
        let doc = CanvasDoc {
            id: id.clone(),
            title: title.unwrap_or_else(|| "Untitled".to_string()),
            owner_user_id,
            project_id,
            revision: 1,
            shapes: Vec::new(),
            decks: Vec::new(),
            created_at_ms: now,
            updated_at_ms: now,
        };
        let mut guard = self.locks.lock(&id, self.doc_path(&id)).await?;
        guard.insert(doc);
        let committed = guard.commit().await?;
        Ok(committed.clone())
    }

    /// Read one document. A parse failure is `Internal`, never `NotFound`.
    pub async fn get(&self, id: &str) -> Result<CanvasDoc, CanvasError> {
        Self::checked_id(id)?;
        match super::doc_io::read(&self.doc_path(id)).await? {
            Some(doc) => Ok(doc),
            None => Err(CanvasError::NotFound(format!("canvas {id}"))),
        }
    }

    /// Enumerate the library. A broken row is skipped LOUDLY (named `warn!`)
    /// — never silently, and never fails the whole listing (§5.23b).
    pub async fn list(&self) -> Vec<CanvasRow> {
        let mut rows = Vec::new();
        let mut entries = match tokio::fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            // A root that does not exist yet is an empty library, not a fault.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return rows,
            Err(e) => {
                warn!(root = %self.root.display(), error = %e,
                    "canvas: cannot enumerate canvas root");
                return rows;
            }
        };
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(e) => {
                    warn!(root = %self.root.display(), error = %e,
                        "canvas: directory enumeration failed mid-way");
                    break;
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().is_dir() {
                continue; // stray files (.DS_Store and friends) are not rows
            }
            match super::doc_io::read(&entry.path().join("doc.json")).await {
                Ok(Some(doc)) => {
                    if doc.id != name {
                        warn!(canvas = %name, doc_id = %doc.id,
                            "canvas: doc.json id does not match its directory — skipping unaddressable document");
                        continue;
                    }
                    rows.push(CanvasRow {
                        id: doc.id,
                        title: doc.title,
                        revision: doc.revision,
                        shape_count: doc.shapes.len() as u64,
                        project_id: doc.project_id,
                        updated_at_ms: doc.updated_at_ms,
                    });
                }
                // A canvas dir with no doc.json (crash between mkdir and
                // rename) and a doc that will not parse both skip LOUDLY —
                // a silent skip is how a document vanishes from every
                // surface at once while its bytes sit intact on disk.
                Ok(None) => {
                    warn!(canvas = %name, "canvas: directory has no doc.json — skipping");
                }
                Err(e) => {
                    warn!(canvas = %name, error = %e, "canvas: unreadable doc.json — skipping");
                }
            }
        }
        // read_dir order is platform noise; the library listing is stable.
        rows.sort_by(|a, b| {
            b.updated_at_ms
                .cmp(&a.updated_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        rows
    }

    /// The only write entry point: validate, lock, check `base_revision`,
    /// apply in place, bump revision, commit, publish — the last two inside
    /// the same critical section. Returns the new revision.
    pub async fn apply(
        &self,
        id: &str,
        base_revision: u64,
        ops: Vec<CanvasOp>,
        actor: Option<String>,
    ) -> Result<u64, CanvasError> {
        Self::checked_id(id)?;
        validate::ops_shape(&ops)?;
        let mut guard = self.locks.lock(id, self.doc_path(id)).await?;
        let doc = guard
            .existing_mut()
            .ok_or_else(|| CanvasError::NotFound(format!("canvas {id}")))?;
        if doc.revision != base_revision {
            return Err(CanvasError::Conflict {
                current_revision: doc.revision,
            });
        }
        // In-place application; on failure the guard drops uncommitted and
        // the half-applied copy never reaches disk.
        validate::apply_ops(doc, &ops)?;
        doc.revision += 1;
        doc.updated_at_ms = now_ms();
        // `commit(&mut self)` keeps the per-canvas lock (see doc_io): the
        // publish below must happen inside the same critical section, or two
        // racing applies could publish in reverse revision order.
        let committed = guard.commit().await?;
        let new_revision = committed.revision;
        // The committed batch is what defines "referenced" from here on —
        // snapshot it while still inside the critical section, so the sweep
        // below can never race a later apply's references.
        let referenced: std::collections::HashSet<String> = committed
            .shapes
            .iter()
            .flat_map(|s| s.asset_ids())
            .map(str::to_string)
            .collect();
        self.emit_updated(committed, &ops, actor.as_deref());
        // Orphan sweep in passing (assets.rs): best-effort — a failed sweep
        // must never fail the apply that already committed and announced.
        if let Err(e) = self.sweep_assets_with(id, &referenced).await {
            warn!(canvas = %id, error = %e,
                "canvas: orphan asset sweep after apply failed");
        }
        drop(guard);
        Ok(new_revision)
    }

    /// Delete a canvas — document, assets, directory. Owner-only enforcement
    /// lives on the RPC face (Task 8), not here.
    pub async fn delete(&self, id: &str) -> Result<(), CanvasError> {
        Self::checked_id(id)?;
        let mut guard = self.locks.lock(id, self.doc_path(id)).await?;
        if guard.existing_mut().is_none() {
            return Err(CanvasError::NotFound(format!("canvas {id}")));
        }
        // Remove while still holding the per-canvas lock, so a racing apply
        // either committed before us or finds nothing to lock onto after.
        tokio::fs::remove_dir_all(self.root.join(id))
            .await
            .map_err(|e| CanvasError::Internal(format!("failed to delete canvas {id}: {e}")))?;
        drop(guard);
        Ok(())
    }

    /// Publish `canvas.updated` for a committed batch. Called INSIDE the
    /// per-canvas critical section (the guard is still alive), so event
    /// order can never diverge from commit order.
    fn emit_updated(&self, _doc: &CanvasDoc, _ops: &[CanvasOp], _actor: Option<&str>) {
        // wired in Task 9: builds `GatewayEventFrame::CanvasUpdated` (the
        // frame variant lands in the same change-set as this body — no
        // half-wired state) and publishes it on the bus.
        let Some(_bus) = &self.event_bus else { return };
    }

    pub(super) fn doc_path(&self, id: &str) -> PathBuf {
        self.root.join(id).join("doc.json")
    }

    /// Every path join sits behind this gate: the id charset has no
    /// separators and no dots, so `root.join(id)` cannot traverse.
    pub(super) fn checked_id(id: &str) -> Result<(), CanvasError> {
        if validate::is_valid_id(id) {
            Ok(())
        } else {
            Err(CanvasError::Invalid(format!(
                "invalid canvas id {id:?}: expected [A-Za-z0-9_-], 1..=64 chars"
            )))
        }
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{Deck, FracIndex, Shape, ShapeCommon, ShapeStyle, MAX_SHAPES};

    fn note(id: &str, text: &str) -> Shape {
        Shape::Note {
            common: ShapeCommon {
                id: id.to_string(),
                x: 0.0,
                y: 0.0,
                w: 200.0,
                h: 200.0,
                z: FracIndex::first(),
                parent_id: None,
            },
            style: ShapeStyle::default(),
            text: text.to_string(),
        }
    }

    fn upsert_note(id: &str) -> CanvasOp {
        CanvasOp::UpsertShape {
            shape: note(id, "hi"),
        }
    }

    fn note_text(shape: &Shape) -> &str {
        match shape {
            Shape::Note { text, .. } => text,
            other => panic!("expected a note, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn apply_with_stale_revision_returns_conflict_with_current() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let doc = store
            .create(Some("t".into()), None, Some("u1".into()))
            .await
            .unwrap();
        let op = upsert_note("n1");
        let r1 = store
            .apply(&doc.id, doc.revision, vec![op.clone()], None)
            .await
            .unwrap();
        let err = store
            .apply(&doc.id, doc.revision, vec![op], None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, CanvasError::Conflict { current_revision } if current_revision == r1),
            "stale base must carry the current revision: {err:?}"
        );
        // Guard lives past every assertion.
        drop(dir);
    }

    #[tokio::test]
    async fn concurrent_applies_serialize_one_wins_one_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(CanvasStore::new(dir.path().to_path_buf()));
        let doc = store.create(None, None, None).await.unwrap();
        let base = doc.revision;

        let spawn_apply = |shape_id: &'static str| {
            let store = Arc::clone(&store);
            let id = doc.id.clone();
            tokio::spawn(async move {
                store
                    .apply(&id, base, vec![upsert_note(shape_id)], None)
                    .await
            })
        };
        let ra = spawn_apply("a");
        let rb = spawn_apply("b");
        let (ra, rb) = (ra.await.unwrap(), rb.await.unwrap());

        let (winner, loser) = match (ra, rb) {
            (Ok(r), Err(e)) | (Err(e), Ok(r)) => (r, e),
            other => panic!("expected exactly one Ok and one Conflict, got {other:?}"),
        };
        assert!(
            matches!(loser, CanvasError::Conflict { current_revision } if current_revision == winner),
            "the loser must be told the winner's revision"
        );
        let on_disk = store.get(&doc.id).await.unwrap();
        assert_eq!(on_disk.revision, winner, "disk must carry the winner");
        assert_eq!(on_disk.shapes.len(), 1, "exactly one batch landed");
        drop(dir);
    }

    #[tokio::test]
    async fn delete_shape_op_removes_it_and_upsert_replaces_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let doc = store.create(None, None, None).await.unwrap();

        let r1 = store
            .apply(
                &doc.id,
                doc.revision,
                vec![
                    CanvasOp::UpsertShape {
                        shape: note("n1", "one"),
                    },
                    upsert_note("n2"),
                ],
                None,
            )
            .await
            .unwrap();

        // Upsert of an existing id replaces in place — same slot, no growth.
        let r2 = store
            .apply(
                &doc.id,
                r1,
                vec![CanvasOp::UpsertShape {
                    shape: note("n1", "two"),
                }],
                None,
            )
            .await
            .unwrap();
        let d = store.get(&doc.id).await.unwrap();
        assert_eq!(d.shapes.len(), 2);
        assert_eq!(d.shapes[0].id(), "n1", "replace-in-place keeps position");
        assert_eq!(note_text(&d.shapes[0]), "two");

        // Delete removes it; deleting a missing id is a no-op, not an error.
        let r3 = store
            .apply(
                &doc.id,
                r2,
                vec![
                    CanvasOp::DeleteShape { id: "n1".into() },
                    CanvasOp::DeleteShape {
                        id: "never-existed".into(),
                    },
                ],
                None,
            )
            .await
            .unwrap();
        let d = store.get(&doc.id).await.unwrap();
        assert_eq!(d.revision, r3);
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.shapes[0].id(), "n2");
        drop(dir);
    }

    #[tokio::test]
    async fn doc_meta_and_deck_ops_apply_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let doc = store.create(Some("old".into()), None, None).await.unwrap();

        let deck = Deck {
            id: "d1".into(),
            title: "deck".into(),
            frame_ids: vec!["f1".into()],
        };
        let r1 = store
            .apply(
                &doc.id,
                doc.revision,
                vec![
                    CanvasOp::SetDocMeta {
                        title: "new".into(),
                    },
                    CanvasOp::UpsertDeck { deck: deck.clone() },
                ],
                None,
            )
            .await
            .unwrap();
        let d = store.get(&doc.id).await.unwrap();
        assert_eq!(d.title, "new");
        assert_eq!(d.decks, vec![deck]);

        let _r2 = store
            .apply(
                &doc.id,
                r1,
                vec![CanvasOp::DeleteDeck { id: "d1".into() }],
                None,
            )
            .await
            .unwrap();
        assert!(store.get(&doc.id).await.unwrap().decks.is_empty());
        drop(dir);
    }

    #[tokio::test]
    async fn a_corrupt_doc_json_is_skipped_loudly_by_list_but_errors_on_get() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let good = store.create(Some("ok".into()), None, None).await.unwrap();

        // Hand-write a broken document next to the good one.
        let bad_dir = dir.path().join("cv-corrupt");
        std::fs::create_dir_all(&bad_dir).unwrap();
        std::fs::write(bad_dir.join("doc.json"), "{ this is not json").unwrap();

        let rows = store.list().await;
        assert_eq!(rows.len(), 1, "the broken row is skipped, not fatal");
        assert_eq!(rows[0].id, good.id);

        // "failed to parse" and "does not exist" are two different answers.
        let err = store.get("cv-corrupt").await.unwrap_err();
        assert!(
            matches!(err, CanvasError::Internal(_)),
            "a parse failure must never read as absence: {err:?}"
        );
        drop(dir);
    }

    #[tokio::test]
    async fn shape_count_over_cap_is_rejected_as_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let doc = store.create(None, None, None).await.unwrap();

        // Seed a document sitting exactly at the cap. Written directly (the
        // test owns the format) — driving 5000 shapes through 500-op batches
        // would test nothing extra and cost seconds.
        let full = CanvasDoc {
            shapes: (0..MAX_SHAPES)
                .map(|i| note(&format!("s{i}"), "x"))
                .collect(),
            ..doc.clone()
        };
        std::fs::write(
            dir.path().join(&doc.id).join("doc.json"),
            serde_json::to_string(&full).unwrap(),
        )
        .unwrap();

        // One shape past the cap: the WHOLE batch is rejected...
        let err = store
            .apply(&doc.id, doc.revision, vec![upsert_note("one_more")], None)
            .await
            .unwrap_err();
        assert!(
            matches!(err, CanvasError::Invalid(_)),
            "over-cap must be caller-fixable Invalid: {err:?}"
        );

        // ...and rejected means NOTHING landed: the same base revision still
        // applies, and a replace-in-place at the cap (no growth) passes.
        let r = store
            .apply(&doc.id, doc.revision, vec![upsert_note("s0")], None)
            .await
            .unwrap();
        assert_eq!(r, doc.revision + 1);
        assert_eq!(store.get(&doc.id).await.unwrap().shapes.len(), MAX_SHAPES);
        drop(dir);
    }

    #[tokio::test]
    async fn create_persists_owner_and_project_scope() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let doc = store
            .create(Some("t".into()), Some("p-1".into()), Some("u1".into()))
            .await
            .unwrap();
        assert!(doc.id.starts_with("cv-"), "id family: {}", doc.id);
        assert_eq!(doc.revision, 1);

        // A fresh store over the same root — what a restart reads back.
        let reopened = CanvasStore::new(dir.path().to_path_buf());
        let back = reopened.get(&doc.id).await.unwrap();
        assert_eq!(back.owner_user_id.as_deref(), Some("u1"));
        assert_eq!(back.project_id.as_deref(), Some("p-1"));
        assert_eq!(back, doc, "the whole document round-trips");
        drop(dir);
    }

    /// Pre-planted nail for Task 9 (name kept on purpose): once
    /// `emit_updated` publishes real frames, this test grows a typed
    /// subscription asserting the EVENT revision sequence is strictly
    /// increasing — which holds because the publish happens inside the
    /// per-canvas critical section (`DocGuard::commit(&mut self)` keeps the
    /// lock). Until then it pins the commit-order half: every contended
    /// apply lands exactly one distinct revision.
    #[tokio::test]
    async fn events_publish_in_revision_order_under_contention() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(CanvasStore::new(dir.path().to_path_buf()));
        let doc = store.create(None, None, None).await.unwrap();

        let mut handles = Vec::new();
        for i in 0..20u32 {
            let store = Arc::clone(&store);
            let id = doc.id.clone();
            handles.push(tokio::spawn(async move {
                loop {
                    let current = store.get(&id).await.unwrap();
                    match store
                        .apply(
                            &id,
                            current.revision,
                            vec![upsert_note(&format!("n{i}"))],
                            None,
                        )
                        .await
                    {
                        Ok(r) => return r,
                        Err(CanvasError::Conflict { .. }) => tokio::task::yield_now().await,
                        Err(e) => panic!("unexpected error under contention: {e:?}"),
                    }
                }
            }));
        }
        let mut revs = Vec::new();
        for h in handles {
            revs.push(h.await.unwrap());
        }
        revs.sort_unstable();
        let expected: Vec<u64> = (doc.revision + 1..=doc.revision + 20).collect();
        assert_eq!(
            revs, expected,
            "each contended apply must land exactly one distinct revision"
        );

        let final_doc = store.get(&doc.id).await.unwrap();
        assert_eq!(final_doc.revision, doc.revision + 20);
        assert_eq!(final_doc.shapes.len(), 20);
        drop(dir);
    }

    #[tokio::test]
    async fn a_path_traversal_id_is_rejected_before_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let long = "x".repeat(65);
        for bad in ["../escape", "a/b", "a\\b", "cv-1.json", "", long.as_str()] {
            let err = store.get(bad).await.unwrap_err();
            assert!(
                matches!(err, CanvasError::Invalid(_)),
                "id {bad:?} must be rejected as Invalid"
            );
        }
        assert!(matches!(
            store.delete("../escape").await.unwrap_err(),
            CanvasError::Invalid(_)
        ));
        assert!(matches!(
            store
                .apply("../escape", 1, vec![upsert_note("n")], None)
                .await
                .unwrap_err(),
            CanvasError::Invalid(_)
        ));
        drop(dir);
    }

    #[tokio::test]
    async fn a_missing_canvas_reads_not_found_on_get_apply_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        assert!(matches!(
            store.get("cv-nope").await.unwrap_err(),
            CanvasError::NotFound(_)
        ));
        assert!(matches!(
            store
                .apply("cv-nope", 1, vec![upsert_note("n")], None)
                .await
                .unwrap_err(),
            CanvasError::NotFound(_)
        ));
        assert!(matches!(
            store.delete("cv-nope").await.unwrap_err(),
            CanvasError::NotFound(_)
        ));
        drop(dir);
    }

    #[tokio::test]
    async fn an_invalid_op_batch_is_rejected_without_touching_the_document() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let doc = store.create(None, None, None).await.unwrap();

        // Empty batch: a client bug, not a no-op revision bump.
        assert!(matches!(
            store
                .apply(&doc.id, doc.revision, vec![], None)
                .await
                .unwrap_err(),
            CanvasError::Invalid(_)
        ));
        // Bad shape id inside an op.
        assert!(matches!(
            store
                .apply(&doc.id, doc.revision, vec![upsert_note("bad/../id")], None)
                .await
                .unwrap_err(),
            CanvasError::Invalid(_)
        ));
        // Nothing landed: same revision, no shapes.
        let unchanged = store.get(&doc.id).await.unwrap();
        assert_eq!(unchanged.revision, doc.revision);
        assert!(unchanged.shapes.is_empty());
        drop(dir);
    }

    #[tokio::test]
    async fn delete_removes_the_directory_and_every_surface_agrees() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let doc = store.create(Some("gone".into()), None, None).await.unwrap();
        store.delete(&doc.id).await.unwrap();

        assert!(!dir.path().join(&doc.id).exists(), "directory removed");
        assert!(matches!(
            store.get(&doc.id).await.unwrap_err(),
            CanvasError::NotFound(_)
        ));
        assert!(store.list().await.is_empty());
        drop(dir);
    }
}
