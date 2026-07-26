//! Filesystem I/O for the artifact store.
//!
//! See the [module docs](super) for the on-disk layout and the isolation
//! guarantees the session-key encoding provides.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use tokio::fs;
use tracing::warn;
use uuid::Uuid;

use super::{encode_session_key, ArtifactError, ArtifactOrigin, ArtifactRecord};

/// Largest blob the store accepts, mirroring `media::cache::MAX_FILE_SIZE`
/// (50 MB) so an attachment that made it through the media cache is never
/// rejected here.
pub const MAX_ARTIFACT_BYTES: u64 = 50 * 1024 * 1024;

/// Retained artifacts per session. A `put` past this evicts the oldest, so a
/// long-running session cannot grow the data directory without bound.
pub const MAX_ARTIFACTS_PER_SESSION: usize = 200;

/// Longest display filename kept, in characters. Keeps the value comfortably
/// under filesystem and `Content-Disposition` limits downstream.
const MAX_FILENAME_CHARS: usize = 200;

/// Filename used when the caller's name sanitizes down to nothing.
const FALLBACK_FILENAME: &str = "unnamed";

/// MIME type used when the caller's is empty or implausible.
const FALLBACK_MIME: &str = "application/octet-stream";

/// Longest MIME type kept, in bytes.
const MAX_MIME_BYTES: usize = 255;

/// Durable, id-addressed blob storage rooted at one directory.
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    /// Build a store rooted at `root`. The directory is created lazily on the
    /// first [`put`](Self::put).
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The conventional root, `<aleph_data_dir>/artifacts`.
    ///
    /// Deliberately outside `std::env::temp_dir()`: inbound media currently
    /// lands in the temp dir and is `remove_dir_all`'d at the end of every run,
    /// which is exactly what an artifact must survive.
    pub fn default_root() -> Result<PathBuf, ArtifactError> {
        crate::utils::paths::get_data_dir()
            .map(|dir| dir.join("artifacts"))
            .map_err(|e| ArtifactError::Root(e.to_string()))
    }

    /// The process-wide store at [`default_root`](Self::default_root), for
    /// callers with no injected one — tools and the dispatch chokepoint.
    ///
    /// A store owns nothing but its root path, so this instance and one the
    /// gateway builds from the same root are interchangeable; there is no
    /// shared mutable state to keep coherent. Resolved once: a data directory
    /// that cannot be resolved at first use will not resolve later either, and
    /// re-warning on every tool call would be noise.
    ///
    /// `None` means the data directory is unavailable — the caller has no store
    /// and must degrade, never fail the work it was doing.
    #[must_use]
    pub fn shared() -> Option<&'static Self> {
        static SHARED: std::sync::OnceLock<Option<ArtifactStore>> = std::sync::OnceLock::new();
        SHARED
            .get_or_init(|| match Self::default_root() {
                Ok(root) => Some(Self::new(root)),
                Err(e) => {
                    warn!(error = %e, "artifact store unavailable: cannot resolve the artifact root");
                    None
                }
            })
            .as_ref()
    }

    /// Root directory this store writes under.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Store `bytes` under `session_key` and return the record describing them.
    ///
    /// `origin` is the caller's declaration, never inferred. `filename` and
    /// `mime_type` are normalized before they are recorded.
    ///
    /// # Errors
    /// [`ArtifactError::TooLarge`] past [`MAX_ARTIFACT_BYTES`], or an I/O error
    /// if the blob cannot be written.
    pub async fn put(
        &self,
        session_key: &str,
        run_id: Option<&str>,
        origin: ArtifactOrigin,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<ArtifactRecord, ArtifactError> {
        let size = bytes.len() as u64;
        if size > MAX_ARTIFACT_BYTES {
            return Err(ArtifactError::TooLarge {
                size,
                max: MAX_ARTIFACT_BYTES,
            });
        }

        let dir = self.session_dir(session_key);
        fs::create_dir_all(&dir).await?;

        let record = ArtifactRecord {
            id: Uuid::new_v4().hyphenated().to_string(),
            session_key: session_key.to_string(),
            run_id: run_id.map(str::to_string),
            origin,
            filename: sanitize_filename(filename),
            mime_type: normalize_mime(mime_type),
            size,
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        // Bytes first, sidecar second. A crash between the two leaves an orphan
        // `.bin` that `list` never sees (it reads only `.json`), rather than a
        // published record pointing at bytes that are not there.
        write_atomic(&dir.join(format!("{}.bin", record.id)), bytes).await?;
        let sidecar = serde_json::to_vec(&record)?;
        write_atomic(&dir.join(format!("{}.json", record.id)), &sidecar).await?;

        evict_overflow(&dir).await;

        Ok(record)
    }

    /// Every artifact of a session, newest first.
    ///
    /// An unknown session yields an empty list. A single unreadable sidecar is
    /// skipped with a warning rather than blinding the whole listing.
    ///
    /// # Errors
    /// I/O errors other than "the session has no directory yet".
    pub async fn list(&self, session_key: &str) -> Result<Vec<ArtifactRecord>, ArtifactError> {
        let mut records = read_records(&self.session_dir(session_key)).await?;
        records.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(records)
    }

    /// Read one artifact's record and bytes.
    ///
    /// `id` must be a bare lowercase hyphenated UUID; anything else is rejected
    /// before it can touch the filesystem. The artifact is looked up only under
    /// `session_key`'s own directory, so a caller scoped to one session cannot
    /// reach another's bytes.
    ///
    /// # Errors
    /// [`ArtifactError::InvalidId`] for a malformed id,
    /// [`ArtifactError::NotFound`] when the session has no such artifact.
    pub async fn read(
        &self,
        session_key: &str,
        id: &str,
    ) -> Result<(ArtifactRecord, Vec<u8>), ArtifactError> {
        validate_id(id)?;
        let dir = self.session_dir(session_key);

        let sidecar = read_or_not_found(&dir.join(format!("{id}.json")), id).await?;
        let record: ArtifactRecord = serde_json::from_slice(&sidecar)?;
        let bytes = read_or_not_found(&dir.join(format!("{id}.bin")), id).await?;

        Ok((record, bytes))
    }

    /// Delete every artifact of a session. Purging an unknown session succeeds.
    ///
    /// # Errors
    /// I/O errors other than "the session has no directory".
    pub async fn purge_session(&self, session_key: &str) -> Result<(), ArtifactError> {
        match fs::remove_dir_all(self.session_dir(session_key)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn session_dir(&self, session_key: &str) -> PathBuf {
        self.root.join(encode_session_key(session_key))
    }
}

/// Read a file, mapping "missing" onto [`ArtifactError::NotFound`].
async fn read_or_not_found(path: &Path, id: &str) -> Result<Vec<u8>, ArtifactError> {
    match fs::read(path).await {
        Ok(bytes) => Ok(bytes),
        Err(e) if e.kind() == ErrorKind::NotFound => Err(ArtifactError::NotFound(id.to_string())),
        Err(e) => Err(e.into()),
    }
}

/// Parse every sidecar in `dir`, in whatever order the filesystem hands them
/// back. A missing directory is an empty listing, not an error.
async fn read_records(dir: &Path) -> Result<Vec<ArtifactRecord>, ArtifactError> {
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let mut records = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let raw = match fs::read(&path).await {
            Ok(raw) => raw,
            Err(e) => {
                warn!(path = %path.display(), error = %e, "skipping unreadable artifact sidecar");
                continue;
            }
        };
        match serde_json::from_slice::<ArtifactRecord>(&raw) {
            Ok(record) => records.push(record),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "skipping corrupt artifact sidecar");
            }
        }
    }
    Ok(records)
}

/// Count the sidecars in `dir` without reading any of them.
async fn count_sidecars(dir: &Path) -> Result<usize, std::io::Error> {
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut count = 0;
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            count += 1;
        }
    }
    Ok(count)
}

/// Eviction order key: lower is discarded first.
///
/// Deliverables rank last because they are the *point* of the session — the
/// report the user asked for, as opposed to the screenshots and intermediate
/// downloads that produced it. Evicting purely by age would let a run that
/// generates two hundred scratch images silently delete the one artifact the
/// session existed to produce.
const fn eviction_rank(origin: ArtifactOrigin) -> u8 {
    match origin {
        ArtifactOrigin::Inbound | ArtifactOrigin::Outbound | ArtifactOrigin::Export => 0,
        ArtifactOrigin::Deliverable => 1,
    }
}

/// Drop the least valuable artifacts until the session is back within
/// [`MAX_ARTIFACTS_PER_SESSION`] — oldest first within a rank, and
/// deliverables only once nothing else is left (see [`eviction_rank`]).
///
/// Best-effort: the blob is already durably written when this runs, so a
/// failure here is logged, never surfaced as a failed `put`.
async fn evict_overflow(dir: &Path) {
    match count_sidecars(dir).await {
        Ok(count) if count <= MAX_ARTIFACTS_PER_SESSION => return,
        Ok(_) => {}
        Err(e) => {
            warn!(path = %dir.display(), error = %e, "artifact eviction skipped: cannot list session");
            return;
        }
    }

    let mut records = match read_records(dir).await {
        Ok(records) => records,
        Err(e) => {
            warn!(path = %dir.display(), error = %e, "artifact eviction skipped: cannot read session");
            return;
        }
    };
    if records.len() <= MAX_ARTIFACTS_PER_SESSION {
        return;
    }

    // Cheapest-to-lose first, then oldest first, with a stable tiebreak so
    // concurrent puts in the same millisecond still evict deterministically.
    records.sort_by(|a, b| {
        eviction_rank(a.origin)
            .cmp(&eviction_rank(b.origin))
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    let overflow = records.len() - MAX_ARTIFACTS_PER_SESSION;
    for record in records.into_iter().take(overflow) {
        // Sidecar first: a partial delete leaves an invisible orphan blob, not
        // a listed record whose bytes are gone.
        let _ = fs::remove_file(dir.join(format!("{}.json", record.id))).await;
        let _ = fs::remove_file(dir.join(format!("{}.bin", record.id))).await;
    }
}

/// Write via a sibling temp file plus rename, so readers only ever observe a
/// complete file.
async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);

    fs::write(&tmp, bytes).await?;
    if let Err(e) = fs::rename(&tmp, path).await {
        let _ = fs::remove_file(&tmp).await;
        return Err(e);
    }
    Ok(())
}

/// Reject anything that is not a bare lowercase hyphenated UUID.
///
/// This is the guard that lets `read` interpolate the id straight into a path:
/// `/`, `\`, `.` and `..` cannot survive it, and neither can the braced or URN
/// UUID spellings (which carry `{` and `:`).
fn validate_id(id: &str) -> Result<(), ArtifactError> {
    let canonical = id.len() == 36
        && Uuid::parse_str(id).is_ok_and(|parsed| parsed.hyphenated().to_string() == id);
    if canonical {
        Ok(())
    } else {
        Err(ArtifactError::InvalidId(id.to_string()))
    }
}

/// Reduce a caller-supplied name to a plain display filename: no directory
/// components, no characters that are illegal on Windows, no control bytes,
/// bounded length.
fn sanitize_filename(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    let cleaned: String = base
        .chars()
        .filter(|c| {
            !c.is_control() && !matches!(c, '/' | '\\' | ':' | '"' | '<' | '>' | '|' | '?' | '*')
        })
        .take(MAX_FILENAME_CHARS)
        .collect();

    // Trailing dots and spaces are silently dropped by Windows; strip them here
    // so the recorded name matches what any filesystem would keep. This also
    // turns a residual ".." into the fallback.
    let trimmed = cleaned.trim().trim_end_matches('.').trim_end();
    if trimmed.is_empty() {
        FALLBACK_FILENAME.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Normalize a caller-supplied MIME type.
///
/// Control characters are stripped because this value is later echoed into a
/// `Content-Type` response header; a bare `\r\n` there would be header
/// injection. An empty or implausibly long value falls back to
/// `application/octet-stream`.
fn normalize_mime(mime: &str) -> String {
    let cleaned: String = mime.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() || cleaned.len() > MAX_MIME_BYTES {
        FALLBACK_MIME.to_string()
    } else {
        cleaned.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SESSION: &str = "agent:main:main";

    fn test_store() -> (ArtifactStore, TempDir) {
        let dir = TempDir::new().unwrap();
        (ArtifactStore::new(dir.path().to_path_buf()), dir)
    }

    #[tokio::test]
    async fn put_then_read_roundtrips() {
        let (store, _guard) = test_store();
        let record = store
            .put(
                SESSION,
                Some("run-1"),
                ArtifactOrigin::Outbound,
                "chart.png",
                "image/png",
                b"\x89PNG payload",
            )
            .await
            .unwrap();

        assert_eq!(record.filename, "chart.png");
        assert_eq!(record.mime_type, "image/png");
        assert_eq!(record.origin, ArtifactOrigin::Outbound);
        assert_eq!(record.run_id.as_deref(), Some("run-1"));
        assert_eq!(record.session_key, SESSION);
        assert_eq!(record.size, 12);

        let (read_back, bytes) = store.read(SESSION, &record.id).await.unwrap();
        assert_eq!(read_back, record);
        assert_eq!(bytes, b"\x89PNG payload");
    }

    #[tokio::test]
    async fn list_returns_newest_first() {
        let (store, _guard) = test_store();
        let mut ids = Vec::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            let record = store
                .put(
                    SESSION,
                    None,
                    ArtifactOrigin::Inbound,
                    name,
                    "text/plain",
                    b"x",
                )
                .await
                .unwrap();
            ids.push(record.id);
            // created_at has millisecond resolution; separate the writes so the
            // ordering under test is the timestamp, not the tiebreak.
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let listed: Vec<String> = store
            .list(SESSION)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        ids.reverse();
        assert_eq!(listed, ids);
    }

    #[tokio::test]
    async fn list_of_unknown_session_is_empty() {
        let (store, _guard) = test_store();
        assert!(store
            .list("agent:main:never-used")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn put_rejects_oversize_payload() {
        let (store, _guard) = test_store();
        let bytes = vec![0_u8; (MAX_ARTIFACT_BYTES + 1) as usize];
        let err = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "huge.bin",
                "application/octet-stream",
                &bytes,
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ArtifactError::TooLarge { size, max }
            if size == MAX_ARTIFACT_BYTES + 1 && max == MAX_ARTIFACT_BYTES));
        // Nothing was published.
        assert!(store.list(SESSION).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn put_evicts_oldest_beyond_cap() {
        let (store, _guard) = test_store();

        let oldest = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "0.txt",
                "text/plain",
                b"0",
            )
            .await
            .unwrap();
        // Make the first artifact strictly the oldest by millisecond clock, so
        // "which one gets evicted" is deterministic.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        for i in 1..=MAX_ARTIFACTS_PER_SESSION {
            store
                .put(
                    SESSION,
                    None,
                    ArtifactOrigin::Inbound,
                    &format!("{i}.txt"),
                    "text/plain",
                    b"x",
                )
                .await
                .unwrap();
        }

        let listed = store.list(SESSION).await.unwrap();
        assert_eq!(listed.len(), MAX_ARTIFACTS_PER_SESSION);
        assert!(
            !listed.iter().any(|r| r.id == oldest.id),
            "the oldest artifact should have been evicted"
        );
        assert!(matches!(
            store.read(SESSION, &oldest.id).await.unwrap_err(),
            ArtifactError::NotFound(_)
        ));
    }

    /// The oldest artifact is normally the first to go — but not when it is the
    /// deliverable. A run that generates a flood of scratch images must not be
    /// able to evict the report the session existed to produce.
    #[tokio::test]
    async fn eviction_sacrifices_everything_before_the_deliverable() {
        let (store, _guard) = test_store();

        let deliverable = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Deliverable,
                "report.html",
                "text/html",
                b"<p>the point of all this</p>",
            )
            .await
            .unwrap();
        // Strictly the oldest by millisecond clock: under a pure age order this
        // is exactly the record that would be dropped.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        for i in 0..=MAX_ARTIFACTS_PER_SESSION {
            store
                .put(
                    SESSION,
                    None,
                    ArtifactOrigin::Outbound,
                    &format!("scratch-{i}.png"),
                    "image/png",
                    b"x",
                )
                .await
                .unwrap();
        }

        let listed = store.list(SESSION).await.unwrap();
        assert_eq!(listed.len(), MAX_ARTIFACTS_PER_SESSION);
        assert!(
            listed.iter().any(|r| r.id == deliverable.id),
            "the deliverable was evicted despite being the last thing worth losing"
        );
    }

    #[tokio::test]
    async fn purge_session_removes_all_artifacts() {
        let (store, _guard) = test_store();
        let record = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Export,
                "s.html",
                "text/html",
                b"<p>hi</p>",
            )
            .await
            .unwrap();

        store.purge_session(SESSION).await.unwrap();

        assert!(store.list(SESSION).await.unwrap().is_empty());
        assert!(matches!(
            store.read(SESSION, &record.id).await.unwrap_err(),
            ArtifactError::NotFound(_)
        ));
        // Purging twice is not an error.
        store.purge_session(SESSION).await.unwrap();
    }

    #[tokio::test]
    async fn read_rejects_ids_that_are_not_bare_uuids() {
        let (store, _guard) = test_store();
        for bad in [
            "../../etc/passwd",
            "..",
            ".",
            "a/b",
            "a\\b",
            "not-a-uuid",
            "",
            // Braced and URN spellings carry '{' and ':' — parseable as UUIDs,
            // still rejected.
            "{6ba7b810-9dad-11d1-80b4-00c04fd430c8}",
            "urn:uuid:6ba7b810-9dad-11d1-80b4-00c04fd430c8",
            // Unhyphenated and uppercase are not the canonical form.
            "6ba7b8109dad11d180b400c04fd430c8",
            "6BA7B810-9DAD-11D1-80B4-00C04FD430C8",
        ] {
            assert!(
                matches!(
                    store.read(SESSION, bad).await,
                    Err(ArtifactError::InvalidId(_))
                ),
                "id {bad:?} should have been rejected"
            );
        }
    }

    #[tokio::test]
    async fn read_of_wellformed_but_unknown_id_is_not_found() {
        let (store, _guard) = test_store();
        let id = Uuid::new_v4().hyphenated().to_string();
        assert!(matches!(
            store.read(SESSION, &id).await.unwrap_err(),
            ArtifactError::NotFound(_)
        ));
    }

    #[tokio::test]
    async fn session_key_with_colon_never_reaches_the_path() {
        let (store, guard) = test_store();
        store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "a.txt",
                "text/plain",
                b"x",
            )
            .await
            .unwrap();

        let mut dirs = std::fs::read_dir(guard.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        dirs.sort();

        assert_eq!(dirs, vec!["agent%3Amain%3Amain".to_string()]);
        assert!(
            !dirs[0].contains(':'),
            "':' is illegal in a Windows path component"
        );
        // The raw key still addresses it.
        assert_eq!(store.list(SESSION).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn artifacts_are_isolated_per_session() {
        let (store, _guard) = test_store();
        let a = store
            .put(
                "agent:main:a",
                None,
                ArtifactOrigin::Inbound,
                "a.txt",
                "text/plain",
                b"a",
            )
            .await
            .unwrap();

        assert!(store.list("agent:main:b").await.unwrap().is_empty());
        assert!(matches!(
            store.read("agent:main:b", &a.id).await.unwrap_err(),
            ArtifactError::NotFound(_)
        ));
    }

    #[tokio::test]
    async fn put_sanitizes_filename_and_mime() {
        let (store, _guard) = test_store();
        let record = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "../../etc/passwd",
                "",
                b"x",
            )
            .await
            .unwrap();
        assert_eq!(record.filename, "passwd");
        assert_eq!(record.mime_type, FALLBACK_MIME);

        let record = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "..",
                "text/plain\r\nX-Injected: 1",
                b"x",
            )
            .await
            .unwrap();
        assert_eq!(record.filename, FALLBACK_FILENAME);
        assert!(!record.mime_type.contains('\r') && !record.mime_type.contains('\n'));
    }

    #[tokio::test]
    async fn corrupt_sidecar_is_skipped_not_fatal() {
        let (store, guard) = test_store();
        store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Inbound,
                "good.txt",
                "text/plain",
                b"x",
            )
            .await
            .unwrap();

        let dir = guard.path().join(encode_session_key(SESSION));
        std::fs::write(dir.join("garbage.json"), b"{not json").unwrap();

        let listed = store.list(SESSION).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].filename, "good.txt");
    }

    #[test]
    fn sanitize_filename_is_utf8_safe_when_truncating() {
        let name = "é".repeat(MAX_FILENAME_CHARS + 50);
        assert_eq!(sanitize_filename(&name).chars().count(), MAX_FILENAME_CHARS);
    }
}
