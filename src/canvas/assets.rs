//! Content-addressed asset storage for one canvas —
//! `<root>/<id>/assets/<sha256>.<ext>`.
//!
//! The address IS the content hash: the filename carries no caller input
//! (the extension comes from the mime allowlist table, never from a client
//! filename), so there is no traversal surface and re-uploading identical
//! bytes deduplicates for free. The reverse also holds — [`parse_asset_id`]
//! accepts exactly what [`CanvasStore::put_asset`] can mint, and everything
//! else (stray files, traversal shapes, unknown extensions) is rejected or
//! left untouched.
//!
//! Orphan reclamation: an asset lands BEFORE the op that references it
//! (`asset.put` → `canvas.apply`), so "unreferenced" alone is not "garbage".
//! Reaping only removes unreferenced files whose mtime is older than
//! [`ORPHAN_GRACE`] — the window that keeps the put→apply race from eating a
//! just-uploaded asset — and a dedupe hit in `put_asset` re-touches the file
//! so a re-referenced old asset re-arms the same window. The actual sweep
//! runs in-apply (still inside the per-canvas critical section); the unit
//! tests below exercise that path through real `apply` calls.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use aleph_protocol::canvas::{MAX_ASSET_BYTES, MAX_HTML_ASSET_BYTES};
use sha2::{Digest, Sha256};
use tracing::warn;

use super::store::{CanvasError, CanvasStore};

/// How long an unreferenced asset survives before the sweep may reap it.
///
/// The grace window exists for the put→apply protocol gap: the Panel (or the
/// model) uploads bytes first and commits the referencing op second, and a
/// sweep running between the two must not delete the upload. One hour is
/// orders of magnitude beyond that gap while still bounding how long deleted
/// content lingers on disk.
pub(super) const ORPHAN_GRACE: Duration = Duration::from_secs(60 * 60);

/// The asset mime allowlist and its extension mapping — ONE table, both
/// directions ([`ext_for_mime`] on upload, [`mime_for_ext`] on read), so the
/// "supported set" cannot fork into two drifting copies (§0).
const ASSET_MIME_TABLE: &[(&str, &str)] = &[
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
    ("image/svg+xml", "svg"),
    ("text/html", "html"),
];

/// Canonical mime (and its extension) for a caller-supplied mime string.
/// Matching is case-insensitive and whitespace-tolerant — boundary
/// normalization, not laxity: the output is always the table's own spelling.
fn ext_for_mime(mime: &str) -> Option<(&'static str, &'static str)> {
    let normalized = mime.trim().to_ascii_lowercase();
    ASSET_MIME_TABLE
        .iter()
        .find(|(m, _)| *m == normalized)
        .copied()
}

fn mime_for_ext(ext: &str) -> Option<&'static str> {
    ASSET_MIME_TABLE
        .iter()
        .find(|(_, e)| *e == ext)
        .map(|(m, _)| *m)
}

/// Parse `<sha256-hex>.<ext>` into the asset's mime type.
///
/// Strict on purpose: exactly 64 lowercase hex digits, one dot, an extension
/// from the allowlist table. This is both the traversal gate (no separators,
/// no dots beyond the one split, nothing our writer could not have minted)
/// and the mime resolver for [`CanvasStore::read_asset`].
/// `pub(super)` so `validate.rs` can reject garbage `asset_id`s at upsert
/// time, before they are committed to doc.json.
pub(super) fn parse_asset_id(asset_id: &str) -> Option<&'static str> {
    let (digest, ext) = asset_id.split_once('.')?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return None;
    }
    mime_for_ext(ext)
}

impl CanvasStore {
    /// Store `bytes` as a content-addressed asset of canvas `id`; returns the
    /// asset id (`<sha256>.<ext>`). Identical bytes deduplicate to the same
    /// id. Runs under the canvas's write lock so it cannot race a concurrent
    /// `delete` into resurrecting the canvas directory.
    pub async fn put_asset(
        &self,
        id: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<String, CanvasError> {
        Self::checked_id(id)?;
        let Some((canonical_mime, ext)) = ext_for_mime(mime) else {
            return Err(CanvasError::Invalid(format!(
                "unsupported asset mime type {mime:?}: expected one of {}",
                ASSET_MIME_TABLE
                    .iter()
                    .map(|(m, _)| *m)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        if bytes.is_empty() {
            return Err(CanvasError::Invalid(
                "asset carries no bytes — an empty upload is a client bug".to_string(),
            ));
        }
        // The cap is keyed by what the bytes ARE, not one blanket number: an
        // html asset feeds a sandboxed iframe and gets the tighter budget.
        let cap = if canonical_mime == "text/html" {
            MAX_HTML_ASSET_BYTES
        } else {
            MAX_ASSET_BYTES
        };
        if bytes.len() > cap {
            return Err(CanvasError::Invalid(format!(
                "{} bytes exceeds the {cap}-byte cap for {canonical_mime}",
                bytes.len()
            )));
        }

        let digest = format!("{:x}", Sha256::digest(bytes));
        let asset_id = format!("{digest}.{ext}");

        // Existence check + write in one critical section (the same
        // per-canvas lock every doc write takes).
        let mut guard = self.locks.lock(id, self.doc_path(id)).await?;
        if guard.existing_mut().is_none() {
            return Err(CanvasError::NotFound(format!("canvas {id}")));
        }
        let dir = self.assets_dir(id);
        let path = dir.join(&asset_id);
        // Existence check must distinguish "regular file present" from
        // "something else at this path" (a directory placed by an operator
        // or a symlink would otherwise be treated as a dedupe hit and the
        // supplied bytes would never be written — silent data loss).
        // `metadata().is_ok()` accepts non-file types; we gate on
        // `is_file()` so the dedupe branch only fires when an actual file
        // already sits at the canonical name.
        let dedupe_hit = match tokio::fs::metadata(&path).await {
            Ok(md) if md.is_file() => true,
            Ok(_) => {
                // A directory / symlink / fifo occupies the path. Refuse
                // to silently dedupe; fall through to write the bytes over
                // top (atomic_write_bytes will replace the existing entry
                // atomically — the bytes the caller supplied are stored).
                warn!(canvas = %id, asset = %asset_id,
                    "canvas: non-file entry at asset path; treating as miss");
                false
            }
            Err(_) => false,
        };
        if dedupe_hit {
            // Dedupe hit. Re-touch the mtime so the orphan sweep's grace
            // window re-arms: without this, put(old orphan) → sweep → apply
            // would land a dangling reference. Best-effort — a failed touch
            // degrades to the pre-existing race window, never to data loss
            // of referenced content (the sweep only reaps UNreferenced files).
            match std::fs::OpenOptions::new().write(true).open(&path) {
                Ok(f) => {
                    if let Err(e) = f.set_modified(SystemTime::now()) {
                        warn!(canvas = %id, asset = %asset_id, error = %e,
                            "canvas: could not renew asset grace window on dedupe");
                    }
                }
                Err(e) => warn!(canvas = %id, asset = %asset_id, error = %e,
                    "canvas: could not reopen asset to renew its grace window"),
            }
            drop(guard);
            return Ok(asset_id);
        }
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| CanvasError::Internal(format!("failed to create assets dir: {e}")))?;
        crate::utils::atomic_write::atomic_write_bytes(&path, bytes)
            .await
            .map_err(|e| CanvasError::Internal(format!("failed to write asset: {e}")))?;
        drop(guard);
        Ok(asset_id)
    }

    /// Read an asset back as `(mime_type, bytes)`.
    ///
    /// The asset id is validated to the exact shape [`Self::put_asset`] can
    /// mint BEFORE any path is built — a traversal-shaped id is `Invalid`
    /// without touching disk, and the mime comes from the same table that
    /// chose the extension on the way in.
    pub async fn read_asset(
        &self,
        id: &str,
        asset_id: &str,
    ) -> Result<(String, Vec<u8>), CanvasError> {
        Self::checked_id(id)?;
        let Some(mime) = parse_asset_id(asset_id) else {
            return Err(CanvasError::Invalid(format!(
                "invalid asset id {asset_id:?}: expected <sha256-hex>.<ext> with a whitelisted extension"
            )));
        };
        let path = self.assets_dir(id).join(asset_id);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok((mime.to_string(), bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Collapse to a single NotFound: the previous shape did a
                // second `tokio::fs::metadata(&canvas_dir)` to
                // distinguish "canvas missing" from "asset missing in a
                // present canvas", but that second syscall was
                // outside the per-canvas read lock so a concurrent
                // delete could land between the two checks and flip
                // the diagnostic. The caller already has the canvas
                // id (it was a parameter); telling it which one was
                // wrong is not worth a TOCTOU surface.
                Err(CanvasError::NotFound(format!(
                    "canvas {id} or asset {asset_id} not found"
                )))
            }
            Err(e) => Err(CanvasError::Internal(format!(
                "failed to read asset {asset_id}: {e}"
            ))),
        }
    }

    /// The sweep body, given an already-computed referenced set. Callers hold
    /// the canvas's write lock (`apply`'s in-passing sweep, which reuses the
    /// set from the batch it just committed while still inside the
    /// critical section).
    pub(super) async fn sweep_assets_with(
        &self,
        id: &str,
        referenced: &HashSet<String>,
    ) -> Result<usize, CanvasError> {
        let dir = self.assets_dir(id);
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            // No assets dir means nothing was ever uploaded — zero orphans.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => {
                return Err(CanvasError::Internal(format!(
                    "failed to enumerate assets of canvas {id}: {e}"
                )))
            }
        };
        let now = SystemTime::now();
        let mut removed = 0usize;
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(e) => {
                    warn!(canvas = %id, error = %e,
                        "canvas: asset enumeration failed mid-sweep");
                    break;
                }
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if parse_asset_id(&name).is_none() {
                // Not a name our writer could have minted — not ours to
                // delete. Loud, per §5.23b: a silent skip hides the stray.
                warn!(canvas = %id, file = %name,
                    "canvas: stray file in assets dir — leaving it alone");
                continue;
            }
            if referenced.contains(&name) {
                continue;
            }
            // Fail-soft skips must never become delete verdicts: an
            // unreadable mtime is "unknown", and unknown is not "old" (§0).
            let modified = match entry.metadata().await.and_then(|m| m.modified()) {
                Ok(modified) => modified,
                Err(e) => {
                    warn!(canvas = %id, asset = %name, error = %e,
                        "canvas: unreadable asset mtime — skipping, not reaping");
                    continue;
                }
            };
            match now.duration_since(modified) {
                Ok(age) if age >= ORPHAN_GRACE => {}
                // Younger than the grace window, or an mtime in the future
                // (clock skew): either way, not provably stale — keep it.
                _ => continue,
            }
            match tokio::fs::remove_file(entry.path()).await {
                Ok(()) => removed += 1,
                Err(e) => warn!(canvas = %id, asset = %name, error = %e,
                    "canvas: failed to remove orphan asset"),
            }
        }
        Ok(removed)
    }

    fn assets_dir(&self, id: &str) -> PathBuf {
        self.root.join(id).join("assets")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{
        AiFrameStatus, CanvasOp, FracIndex, Shape, ShapeCommon, ShapeStyle,
    };
    use filetime::FileTime;
    use std::path::Path;

    const PNG: &str = "image/png";

    fn common(id: &str) -> ShapeCommon {
        ShapeCommon {
            id: id.to_string(),
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 100.0,
            z: FracIndex::first(),
            parent_id: None,
        }
    }

    fn upsert_image(id: &str, asset_id: &str) -> CanvasOp {
        CanvasOp::UpsertShape {
            shape: Shape::Image {
                common: common(id),
                asset_id: asset_id.to_string(),
                natural_w: 1.0,
                natural_h: 1.0,
            },
        }
    }

    fn upsert_ai_frame(id: &str, reference_asset_ids: Vec<String>) -> CanvasOp {
        CanvasOp::UpsertShape {
            shape: Shape::AiImageFrame {
                common: common(id),
                prompt: "p".to_string(),
                reference_asset_ids,
                status: AiFrameStatus::Draft,
            },
        }
    }

    fn upsert_note(id: &str) -> CanvasOp {
        CanvasOp::UpsertShape {
            shape: Shape::Note {
                common: common(id),
                style: ShapeStyle::default(),
                text: "hi".to_string(),
            },
        }
    }

    /// Push a file's mtime past the orphan grace window.
    fn age_past_grace(path: &Path) {
        let old = SystemTime::now() - (ORPHAN_GRACE + Duration::from_secs(3600));
        filetime::set_file_mtime(path, FileTime::from_system_time(old)).unwrap();
    }

    async fn store_with_canvas() -> (tempfile::TempDir, CanvasStore, String) {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let doc = store.create(None, None, None).await.unwrap();
        (dir, store, doc.id)
    }

    #[tokio::test]
    async fn put_asset_is_content_addressed_and_deduplicates() {
        let (dir, store, id) = store_with_canvas().await;
        let a = store.put_asset(&id, PNG, b"same bytes").await.unwrap();
        let b = store.put_asset(&id, PNG, b"same bytes").await.unwrap();
        assert_eq!(a, b, "identical bytes must address identically");
        assert!(a.ends_with(".png"), "extension from the mime table: {a}");
        let files: Vec<_> = std::fs::read_dir(dir.path().join(&id).join("assets"))
            .unwrap()
            .collect();
        assert_eq!(files.len(), 1, "dedupe stores one file, not two");

        let c = store.put_asset(&id, PNG, b"other bytes").await.unwrap();
        assert_ne!(a, c, "different bytes must address differently");
        drop(dir);
    }

    #[tokio::test]
    async fn the_asset_byte_cap_is_keyed_by_mime() {
        let (dir, store, id) = store_with_canvas().await;
        // Over the html cap but under the general cap: the SAME bytes are
        // rejected as html and accepted as png — the cap keys off the mime
        // dimension, not one blanket number.
        let mid = vec![b'a'; MAX_HTML_ASSET_BYTES + 1];
        let err = store.put_asset(&id, "text/html", &mid).await.unwrap_err();
        assert!(
            matches!(err, CanvasError::Invalid(_)),
            "over-cap html must be caller-fixable: {err:?}"
        );
        store.put_asset(&id, PNG, &mid).await.unwrap();

        let huge = vec![b'a'; MAX_ASSET_BYTES + 1];
        let err = store.put_asset(&id, PNG, &huge).await.unwrap_err();
        assert!(matches!(err, CanvasError::Invalid(_)));
        drop(dir);
    }

    #[tokio::test]
    async fn an_unlisted_mime_type_is_rejected_and_matching_is_normalized() {
        let (dir, store, id) = store_with_canvas().await;
        for bad in ["application/pdf", "text/javascript", "image/x-icon", ""] {
            let err = store.put_asset(&id, bad, b"x").await.unwrap_err();
            assert!(
                matches!(err, CanvasError::Invalid(_)),
                "mime {bad:?} must be rejected as Invalid"
            );
        }
        // Case/whitespace are boundary noise, not a different mime.
        let a = store.put_asset(&id, " IMAGE/PNG ", b"x").await.unwrap();
        assert!(a.ends_with(".png"));
        drop(dir);
    }

    #[tokio::test]
    async fn an_empty_asset_is_rejected_as_a_client_bug() {
        let (dir, store, id) = store_with_canvas().await;
        let err = store.put_asset(&id, PNG, b"").await.unwrap_err();
        assert!(matches!(err, CanvasError::Invalid(_)));
        drop(dir);
    }

    #[tokio::test]
    async fn put_asset_on_a_missing_canvas_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let store = CanvasStore::new(dir.path().to_path_buf());
        let err = store.put_asset("cv-nope", PNG, b"x").await.unwrap_err();
        assert!(matches!(err, CanvasError::NotFound(_)));
        assert!(
            !dir.path().join("cv-nope").exists(),
            "a refused put must not conjure the canvas directory"
        );
        drop(dir);
    }

    #[tokio::test]
    async fn read_asset_round_trips_mime_and_bytes() {
        let (dir, store, id) = store_with_canvas().await;
        let payload = [0u8, 159, 146, 150, 255]; // non-UTF-8 on purpose
        let asset_id = store.put_asset(&id, PNG, &payload).await.unwrap();
        let (mime, bytes) = store.read_asset(&id, &asset_id).await.unwrap();
        assert_eq!(mime, PNG);
        assert_eq!(bytes, payload);
        drop(dir);
    }

    #[tokio::test]
    async fn a_traversal_shaped_asset_id_is_rejected_as_invalid() {
        let (dir, store, id) = store_with_canvas().await;
        let hex = "a".repeat(64);
        for bad in [
            "../../etc/passwd".to_string(),
            "..".to_string(),
            format!("{hex}/../x.png"),
            format!("..\\{hex}.png"),
            format!("{hex}.exe"),        // unlisted extension
            format!("{hex}.png.html"),   // double extension
            hex.to_uppercase() + ".png", // we only mint lowercase
            "abc.png".to_string(),       // digest too short
            hex.clone(),                 // no extension at all
            String::new(),
        ] {
            let err = store.read_asset(&id, &bad).await.unwrap_err();
            assert!(
                matches!(err, CanvasError::Invalid(_)),
                "asset id {bad:?} must be Invalid (shape rejection, not a disk miss): {err:?}"
            );
        }
        drop(dir);
    }

    #[tokio::test]
    async fn a_missing_asset_reads_not_found_not_invalid() {
        let (dir, store, id) = store_with_canvas().await;
        let never = format!("{}.png", "b".repeat(64));
        let err = store.read_asset(&id, &never).await.unwrap_err();
        assert!(
            matches!(err, CanvasError::NotFound(_)),
            "a well-formed id that hits nothing is NotFound: {err:?}"
        );
        drop(dir);
    }
}
