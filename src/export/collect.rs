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

/// Read the bytes for each record, obeying the inline budget.
///
/// Over-budget artifacts are still returned — they just carry `bytes: None`,
/// so the renderer prints a name-and-size row instead of a data URL. Skipping
/// the *read* (rather than reading and discarding) is why a session holding
/// 200 × 50 MB blobs cannot be turned into an out-of-memory render. A record
/// whose bytes cannot be read degrades the same way.
///
/// Ordering is the caller's: this preserves the slice it is given.
pub async fn collect_artifacts(
    store: &ArtifactStore,
    session_key: &str,
    records: &[ArtifactRecord],
) -> Vec<ExportArtifact> {
    let mut inlined_total: u64 = 0;
    let mut out = Vec::with_capacity(records.len());

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
                    None
                }
            }
        } else {
            None
        };

        out.push(ExportArtifact {
            filename: record.filename.clone(),
            mime_type: record.mime_type.clone(),
            bytes,
            size: record.size,
        });
    }

    out
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
