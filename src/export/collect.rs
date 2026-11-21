//! Reading artifact bytes for a document, within the inline budget.
//!
//! Both documents need the same thing — turn records into [`ExportArtifact`]s
//! without letting a session full of 50 MB blobs decide how much memory the
//! render costs. Doing that once matters more than it looks: the cheap way to
//! get it wrong is to read every blob and *then* check the ceiling, which
//! bounds the document but not the process.

use tracing::warn;

use super::{
    base64_encoded_len, ExportArtifact, MAX_INLINE_ARTIFACT_BYTES, MAX_INLINE_TOTAL_BYTES,
};
use crate::artifacts::{ArtifactRecord, ArtifactStore};

/// Why an artifact is listed in the document but carries no bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// The artifact does not fit the inline budget: either it is over the
    /// per-artifact ceiling, or the document's total ceiling is already spent.
    OverBudget,
    /// The bytes could not be read back from the store.
    Unreadable,
}

impl DropReason {
    /// The reason in the words a reader or a model needs — "over the inline
    /// budget" or "unreadable", not "failed".
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            DropReason::OverBudget => "over the inline budget",
            DropReason::Unreadable => "unreadable",
        }
    }
}

impl std::fmt::Display for DropReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An artifact that is listed in the document but carries no bytes.
///
/// The row still appears in the document; what is missing is the payload. The
/// reason travels with the artifact so a caller can name *which* attachment
/// did not make it and *why* — a figure the budget refused and a figure the
/// store lost are different failures, and "it failed" says neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedArtifact {
    /// The store id of the artifact.
    pub id: String,
    /// The file name the document shows.
    pub filename: String,
    /// Why it carries no bytes.
    pub reason: DropReason,
}

/// Read the bytes for each record, obeying the inline budget.
///
/// Over-budget artifacts are still returned — they just carry `bytes: None`,
/// so the renderer prints a name-and-size row instead of a data URL. Skipping
/// the *read* (rather than reading and discarding) is why a session holding
/// 200 × 50 MB blobs cannot be turned into an out-of-memory render. A record
/// whose bytes cannot be read degrades the same way.
///
/// Ordering is the caller's: this preserves the slice it is given.
///
/// This is the artifacts-only view of [`collect_artifacts_with_drops`]; a
/// caller that needs to say *which* attachment was left out uses that one.
pub async fn collect_artifacts(
    store: &ArtifactStore,
    session_key: &str,
    records: &[ArtifactRecord],
) -> Vec<ExportArtifact> {
    collect_artifacts_with_drops(store, session_key, records).await.0
}

/// Read the bytes for each record, obeying the inline budget, and report the
/// artifacts that carry no bytes.
///
/// The artifacts come back in the order given. The drops say which of them
/// were left without bytes and why. Without that second half, a document that
/// silently lost an attachment looks exactly like one that never had it: the
/// model publishes it, tells the user every figure is in, and the figure is
/// not.
pub async fn collect_artifacts_with_drops(
    store: &ArtifactStore,
    session_key: &str,
    records: &[ArtifactRecord],
) -> (Vec<ExportArtifact>, Vec<DroppedArtifact>) {
    let mut inlined_total: u64 = 0;
    let mut out = Vec::with_capacity(records.len());
    let mut dropped: Vec<DroppedArtifact> = Vec::new();

    for record in records {
        // The budget is counted in the bytes the document will hold, so a
        // payload is measured after base64 inflation — the unit the artifact
        // store measures the finished file in.
        let encoded = base64_encoded_len(record.size);
        let fits = encoded <= MAX_INLINE_ARTIFACT_BYTES
            && inlined_total.saturating_add(encoded) <= MAX_INLINE_TOTAL_BYTES;

        let bytes = if fits {
            match store.read(session_key, &record.id).await {
                Ok((_, bytes)) => {
                    inlined_total = inlined_total.saturating_add(encoded);
                    Some(bytes)
                }
                Err(e) => {
                    warn!(
                        artifact_id = %record.id,
                        error = %e,
                        "artifact bytes unreadable; including it as a listed-only row"
                    );
                    dropped.push(DroppedArtifact {
                        id: record.id.clone(),
                        filename: record.filename.clone(),
                        reason: DropReason::Unreadable,
                    });
                    None
                }
            }
        } else {
            warn!(
                artifact_id = %record.id,
                filename = %record.filename,
                "artifact is over the inline budget; including it as a listed-only row"
            );
            dropped.push(DroppedArtifact {
                id: record.id.clone(),
                filename: record.filename.clone(),
                reason: DropReason::OverBudget,
            });
            None
        };

        out.push(ExportArtifact {
            filename: record.filename.clone(),
            mime_type: record.mime_type.clone(),
            bytes,
            size: record.size,
        });
    }

    (out, dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::ArtifactOrigin;
    use tempfile::TempDir;

    const SESSION: &str = "agent:main:main";

    #[tokio::test]
    async fn oversized_artifacts_are_listed_but_not_inlined() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(tmp.path().to_path_buf());

        let small = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "small.txt",
                "text/plain",
                b"tiny",
            )
            .await
            .expect("put small");
        // Declaring a size past the per-artifact ceiling is enough: the reader
        // consults the record, so no multi-MB fixture has to be written.
        let mut huge = small.clone();
        huge.filename = "huge.bin".to_string();
        huge.size = MAX_INLINE_ARTIFACT_BYTES + 1;

        let collected = collect_artifacts(&store, SESSION, &[huge, small]).await;

        assert_eq!(collected.len(), 2, "over-budget artifacts stay listed");
        assert!(collected[0].bytes.is_none(), "over-budget is not inlined");
        assert_eq!(collected[0].size, MAX_INLINE_ARTIFACT_BYTES + 1);
        assert_eq!(collected[1].bytes.as_deref(), Some(&b"tiny"[..]));
    }

    /// The row surviving is not the same as the caller knowing: without the
    /// drop, a document that quietly lost an attachment is indistinguishable
    /// from one that never had it.
    #[tokio::test]
    async fn a_dropped_artifact_is_reported_with_its_name_and_reason() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(tmp.path().to_path_buf());

        let small = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "small.txt",
                "text/plain",
                b"tiny",
            )
            .await
            .expect("put small");
        let mut huge = small.clone();
        huge.filename = "huge.bin".to_string();
        huge.size = MAX_INLINE_ARTIFACT_BYTES + 1;
        let mut ghost = small.clone();
        ghost.filename = "ghost.txt".to_string();
        ghost.id = uuid::Uuid::new_v4().to_string();

        let (collected, dropped) =
            collect_artifacts_with_drops(&store, SESSION, &[huge.clone(), ghost.clone(), small])
                .await;

        assert_eq!(collected.len(), 3, "every artifact stays listed");
        assert_eq!(dropped.len(), 2, "both lost artifacts are reported");
        assert_eq!(dropped[0].filename, "huge.bin");
        assert_eq!(dropped[0].id, huge.id);
        assert_eq!(dropped[0].reason, DropReason::OverBudget);
        assert_eq!(dropped[0].reason.as_str(), "over the inline budget");
        assert_eq!(dropped[1].filename, "ghost.txt");
        assert_eq!(dropped[1].id, ghost.id);
        assert_eq!(dropped[1].reason, DropReason::Unreadable);
        assert_eq!(dropped[1].reason.as_str(), "unreadable");
    }

    #[tokio::test]
    async fn an_artifact_that_made_it_in_is_not_reported_as_dropped() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(tmp.path().to_path_buf());

        let record = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "small.txt",
                "text/plain",
                b"tiny",
            )
            .await
            .expect("put");

        let (collected, dropped) =
            collect_artifacts_with_drops(&store, SESSION, &[record]).await;

        assert_eq!(collected.len(), 1);
        assert!(collected[0].bytes.is_some());
        assert!(dropped.is_empty(), "{:?}", dropped);
    }

    #[tokio::test]
    async fn an_unreadable_artifact_degrades_to_a_listed_row() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(tmp.path().to_path_buf());

        let mut ghost = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "ghost.txt",
                "text/plain",
                b"gone",
            )
            .await
            .expect("put");
        // An id that no blob backs — the read fails, the row survives.
        ghost.id = uuid::Uuid::new_v4().to_string();

        let collected = collect_artifacts(&store, SESSION, &[ghost]).await;
        assert_eq!(collected.len(), 1);
        assert!(collected[0].bytes.is_none());
        assert_eq!(collected[0].filename, "ghost.txt");
    }

    #[tokio::test]
    async fn the_running_total_stops_inlining_before_the_read() {
        let tmp = TempDir::new().expect("tempdir");
        let store = ArtifactStore::new(tmp.path().to_path_buf());

        let real = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Outbound,
                "a.bin",
                "application/octet-stream",
                b"x",
            )
            .await
            .expect("put");

        // Records that each pass the PER-ARTIFACT cap but together exhaust the
        // whole-document ceiling. Declared sizes do the work — the reader
        // consults the record, so no multi-MB fixture has to be written.
        //
        // The budget counts encoded bytes, so a record's declared size is the
        // payload whose base64 length is compared against the caps: three
        // payload bytes encode to four, so a payload of three quarters of the
        // per-artifact cap encodes to exactly that cap.
        let per_doc = (MAX_INLINE_TOTAL_BYTES / MAX_INLINE_ARTIFACT_BYTES) as usize;
        let payload = MAX_INLINE_ARTIFACT_BYTES / 4 * 3;
        let mut records: Vec<_> = (0..=per_doc)
            .map(|_| {
                let mut r = real.clone();
                r.size = payload;
                r
            })
            .collect();
        records.last_mut().expect("non-empty").filename = "one-too-many.bin".to_string();

        let collected = collect_artifacts(&store, SESSION, &records).await;
        assert!(
            collected[per_doc - 1].bytes.is_some(),
            "everything up to the ceiling should be inlined"
        );
        assert!(
            collected[per_doc].bytes.is_none(),
            "the record past the document ceiling must be skipped, not read"
        );
    }
}
