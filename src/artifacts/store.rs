//! Filesystem I/O for the artifact store.
//!
//! See the [module docs](super) for the on-disk layout and the isolation
//! guarantees the session-key encoding provides.

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, SystemTime};

use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::warn;
use uuid::Uuid;

use super::{encode_session_key, ArtifactError, ArtifactOrigin, ArtifactRecord};
use crate::media::cache::MAX_FILE_SIZE;
use crate::utils::filename::sanitize_filename;

/// Largest blob the store accepts. References the same constant the
/// media cache enforces, so an attachment that made it through the
/// media cache is never rejected here.
pub const MAX_ARTIFACT_BYTES: u64 = MAX_FILE_SIZE;

/// Retained artifacts per session. A `put` past this evicts the oldest, so a
/// long-running session cannot grow the data directory without bound.
pub const MAX_ARTIFACTS_PER_SESSION: usize = 200;

/// MIME type used when the caller's is empty or implausible.
const FALLBACK_MIME: &str = "application/octet-stream";

/// Longest MIME type kept, in bytes.
const MAX_MIME_BYTES: usize = 255;

/// Largest sidecar the store will read.
///
/// A record is a handful of short strings, so 64 KiB is already far past a
/// legitimate sidecar while still refusing a file that was swapped in behind
/// the store's back before it is allocated.
const MAX_SIDECAR_BYTES: u64 = 64 * 1024;

/// Age past which an unreferenced blob or leftover temp file is certainly not
/// part of an in-flight write.
///
/// A blob is published moments before its sidecar, so this only has to cover a
/// slow disk plus process scheduling. An hour is comfortably beyond that and
/// still reclaims crash debris on the next `put`.
const ORPHAN_GRACE: Duration = Duration::from_secs(60 * 60);

/// Table size at which dead publication-lock slots are pruned.
const PUBLICATION_LOCK_PRUNE_AT: usize = 128;

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

        // Publication plus eviction is one critical section per session:
        // without it two concurrent puts each take a directory snapshot that
        // contains the other's record and can evict it, so a `put` that
        // returned `Ok` would name an artifact another `put` had deleted.
        let _publishing = publication_lock(&dir).lock_owned().await;

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

        let blob = dir.join(format!("{}.bin", record.id));
        let sidecar = dir.join(format!("{}.json", record.id));
        // A failed publication undoes whatever this call already wrote: a blob
        // without its sidecar is invisible to `list`, so it would sit outside
        // the session cap forever and no eviction would ever revisit it.
        if let Err(e) = publish(&blob, &sidecar, &record, bytes).await {
            remove_quietly(&blob).await;
            remove_quietly(&sidecar).await;
            return Err(e);
        }

        // The record this call just published is never a victim of this
        // call's eviction: `put` reports success, so the artifact it names
        // has to stay readable.
        evict_overflow(&dir, Some(record.id.as_str())).await;
        reconcile_orphans(&dir).await;

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
    /// [`ArtifactError::NotFound`] when the session has no such artifact,
    /// [`ArtifactError::TooLarge`] when the sidecar or blob on disk exceeds
    /// what the store is willing to read, [`ArtifactError::Io`] when the blob
    /// does not match the size its sidecar declares.
    pub async fn read(
        &self,
        session_key: &str,
        id: &str,
    ) -> Result<(ArtifactRecord, Vec<u8>), ArtifactError> {
        validate_id(id)?;
        let dir = self.session_dir(session_key);

        let sidecar = read_bounded(&dir.join(format!("{id}.json")), id, MAX_SIDECAR_BYTES).await?;
        let record: ArtifactRecord = serde_json::from_slice(&sidecar)?;
        // Read at most one byte past the size the sidecar claims, so a blob
        // replaced by a much larger file is rejected instead of allocated in
        // full. The claim is clamped to the store's own limit: a corrupt
        // sidecar must not be able to raise the ceiling.
        let limit = record.size.min(MAX_ARTIFACT_BYTES);
        let bytes = read_bounded(&dir.join(format!("{id}.bin")), id, limit).await?;

        // The sidecar advertises the blob's size. Bytes that disagree with it
        // are a half-written remnant, and serving them would make every
        // consumer's size arithmetic wrong (Content-Length, export budgets),
        // so the mismatch is reported instead of shipped.
        if bytes.len() as u64 != record.size {
            return Err(ArtifactError::Io(std::io::Error::new(
                ErrorKind::UnexpectedEof,
                format!(
                    "artifact {id} is truncated: sidecar claims {} bytes, blob has {}",
                    record.size,
                    bytes.len()
                ),
            )));
        }

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

/// Per-directory publication locks.
///
/// Store instances share nothing but their root path, so the table is
/// process-global. The daemon is a singleton, which makes a process-local lock
/// a complete answer rather than a partial one.
#[derive(Default)]
struct PublicationLocks {
    slots: std::sync::Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>,
}

impl PublicationLocks {
    /// Resolve the lock for `dir`, creating it when no live holder exists.
    ///
    /// Upgrade-or-insert happens under the table's own mutex, so two tasks
    /// racing on one directory always end up with the same `Arc`.
    fn slot(&self, dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(live) = slots.get(dir).and_then(Weak::upgrade) {
            return live;
        }
        if slots.len() >= PUBLICATION_LOCK_PRUNE_AT {
            slots.retain(|_, weak| weak.strong_count() > 0);
        }
        let fresh = Arc::new(tokio::sync::Mutex::new(()));
        slots.insert(dir.to_path_buf(), Arc::downgrade(&fresh));
        fresh
    }
}

/// The lock serializing publication and eviction of one session directory.
fn publication_lock(dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<PublicationLocks> = OnceLock::new();
    LOCKS.get_or_init(PublicationLocks::default).slot(dir)
}

/// Write the blob, then its sidecar.
///
/// Bytes first, sidecar second: a crash between the two leaves an orphan `.bin`
/// that `list` never sees (it reads only `.json`), rather than a published
/// record pointing at bytes that are not there.
async fn publish(
    blob: &Path,
    sidecar_path: &Path,
    record: &ArtifactRecord,
    bytes: &[u8],
) -> Result<(), ArtifactError> {
    write_atomic(blob, bytes).await?;
    let sidecar = serde_json::to_vec(record)?;
    write_atomic(sidecar_path, &sidecar).await
}

/// Read at most `limit` bytes, mapping "missing" onto [`ArtifactError::NotFound`]
/// and a longer file onto [`ArtifactError::TooLarge`].
///
async fn read_bounded(path: &Path, id: &str, limit: u64) -> Result<Vec<u8>, ArtifactError> {
    let file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(ArtifactError::NotFound(id.to_string()))
        }
        Err(e) => return Err(e.into()),
    };

    // Fail before allocating anything in the ordinary oversized case; the
    // bounded read below is what holds if the file grows after this check.
    match file.metadata().await {
        Ok(metadata) if metadata.len() > limit => {
            return Err(ArtifactError::TooLarge {
                size: metadata.len(),
                max: limit,
            });
        }
        Ok(_) => {}
        Err(e) => return Err(e.into()),
    }

    let mut bytes = Vec::new();
    let mut limited = file.take(limit.saturating_add(1));
    limited.read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > limit {
        return Err(ArtifactError::TooLarge {
            size: bytes.len() as u64,
            max: limit,
        });
    }
    Ok(bytes)
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
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        let raw = match read_bounded(&path, name, MAX_SIDECAR_BYTES).await {
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
/// `keep` names the record the caller has just published. It is never a
/// victim: a `put` that reports success must leave a readable artifact
/// behind. It still counts towards the cap, so the session stays within
/// [`MAX_ARTIFACTS_PER_SESSION`] records.
///
/// Best-effort: the blob is already durably written when this runs, so a
/// failure here is logged, never surfaced as a failed `put`.
async fn evict_overflow(dir: &Path, keep: Option<&str>) {
    let mut records = match read_records(dir).await {
        Ok(records) => records,
        Err(e) => {
            warn!(path = %dir.display(), error = %e, "artifact eviction skipped: cannot read session");
            return;
        }
    };
    let total = records.len();
    if total <= MAX_ARTIFACTS_PER_SESSION {
        return;
    }
    // The freshly published record is not a candidate: evicting it here would
    // leave `put` reporting success for an artifact that is already gone.
    records.retain(|record| keep != Some(record.id.as_str()));

    // Cheapest-to-lose first, then oldest first, with a stable tiebreak so
    // concurrent puts in the same millisecond still evict deterministically.
    records.sort_by(|a, b| {
        eviction_rank(a.origin)
            .cmp(&eviction_rank(b.origin))
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });

    // The kept record is part of `total`, so the overflow is counted against
    // the whole session: the cap holds even though one record is untouchable.
    let overflow = total - MAX_ARTIFACTS_PER_SESSION;
    for record in records.into_iter().take(overflow) {
        let json = dir.join(format!("{}.json", record.id));
        let bin = dir.join(format!("{}.bin", record.id));
        // Sidecar first, and the blob only once the sidecar is gone: a
        // surviving sidecar keeps the artifact listed, and a listed record
        // whose bytes are gone is a broken artifact. Failures are logged but not
        // propagated — the blob is already durably written, and surfacing
        // them as a failed `put` would undo work that succeeded.
        match fs::remove_file(&json).await {
            Ok(()) => {}
            // Already gone: the record is not listed, so its blob is a
            // leftover and may go too.
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                warn!(path = %json.display(), error = %e, "artifact eviction: could not remove sidecar; keeping its blob");
                continue;
            }
        }
        if let Err(e) = fs::remove_file(&bin).await {
            warn!(path = %bin.display(), error = %e, "artifact eviction: could not remove blob");
        }
    }
}

/// Delete blobs and temp files that no published record references.
///
/// A crash between the blob write and the sidecar write, or an eviction whose
/// blob deletion failed, leaves files under the session directory that
/// `read_records` never sees. They do not count towards
/// [`MAX_ARTIFACTS_PER_SESSION`], so no later eviction would ever revisit them
/// and repeated damage could fill the disk the cap exists to protect.
///
/// Only files older than [`ORPHAN_GRACE`] are removed: the publication lock is
/// process-local, so a second process's in-flight `.bin` — written moments
/// before its sidecar — must not be mistaken for debris.
async fn reconcile_orphans(dir: &Path) {
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(e) => {
            warn!(path = %dir.display(), error = %e, "artifact reconciliation skipped: cannot read session");
            return;
        }
    };

    let mut referenced = HashSet::new();
    let mut candidates = Vec::new();
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(e) => {
                warn!(path = %dir.display(), error = %e, "artifact reconciliation stopped early");
                break;
            }
        };
        let path = entry.path();
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("json") => {
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    referenced.insert(stem.to_string());
                }
            }
            Some("bin" | "tmp") => candidates.push(path),
            _ => {}
        }
    }

    for path in candidates {
        if path.extension().and_then(|ext| ext.to_str()) == Some("bin")
            && path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| referenced.contains(stem))
        {
            continue;
        }
        if !is_stale(&path).await {
            continue;
        }
        remove_quietly(&path).await;
    }
}

/// True when `path` has not been modified for longer than [`ORPHAN_GRACE`].
/// An unreadable or uninterpretable mtime keeps the file.
async fn is_stale(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path).await else {
        return false;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age > ORPHAN_GRACE)
}

/// Write via a sibling temp file plus rename, so readers only ever observe a
/// complete file, and fsync both the bytes and the directory entry before the
/// write is acknowledged: a `put` that returned `Ok` has to survive a crash.
async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);

    if let Err(e) = write_synced(&tmp, bytes).await {
        remove_quietly(&tmp).await;
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, path).await {
        remove_quietly(&tmp).await;
        return Err(e);
    }
    sync_parent_dir(path).await;
    Ok(())
}

/// Write `bytes` to `path` and fsync them before returning.
async fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut file = fs::File::create(path).await?;
    file.write_all(bytes).await?;
    file.sync_all().await
}

/// Sync the directory whose entry the rename just changed, so the new name
/// survives a crash.
///
/// Best-effort, and deliberately not an error path: the entry is already
/// renamed into place, so there is nothing to roll back, and not every
/// platform lets a directory be opened and synced this way (Windows needs
/// `FILE_FLAG_BACKUP_SEMANTICS`, some filesystems reject `fsync` on a
/// directory). The bytes themselves are already synced before the rename.
#[cfg(unix)]
async fn sync_parent_dir(path: &Path) {
    let Some(dir) = path.parent() else {
        return;
    };
    match fs::File::open(dir).await {
        Ok(handle) => {
            if let Err(e) = handle.sync_all().await {
                warn!(path = %dir.display(), error = %e, "artifact write: could not sync directory");
            }
        }
        Err(e) => {
            warn!(path = %dir.display(), error = %e, "artifact write: could not open directory to sync");
        }
    }
}

/// Windows cannot open a directory as a file without
/// `FILE_FLAG_BACKUP_SEMANTICS`, and its filesystem journals the rename.
#[cfg(not(unix))]
async fn sync_parent_dir(_path: &Path) {}

/// Remove a file, treating "already gone" as success. Failures are logged and
/// never surfaced: every caller is cleaning up after an attempt that already
/// has a disposition.
async fn remove_quietly(path: &Path) {
    if let Err(e) = fs::remove_file(path).await {
        if e.kind() != ErrorKind::NotFound {
            warn!(path = %path.display(), error = %e, "artifact cleanup: could not remove file");
        }
    }
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
    use crate::utils::filename::FALLBACK_FILENAME;
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

    /// A `put` that reports success must leave a readable artifact behind:
    /// the record it just published is never the victim of its own
    /// eviction.
    #[tokio::test]
    async fn put_never_evicts_the_record_it_just_wrote() {
        let (store, _guard) = test_store();

        // A session full of deliverables: the new scratch artifact is the
        // only rank-0 record, so it would be the first eviction victim.
        for _ in 0..MAX_ARTIFACTS_PER_SESSION {
            store
                .put(
                    SESSION,
                    None,
                    ArtifactOrigin::Deliverable,
                    "report.html",
                    "text/html",
                    b"x",
                )
                .await
                .unwrap();
        }

        let fresh = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Outbound,
                "scratch.png",
                "image/png",
                b"x",
            )
            .await
            .unwrap();

        assert!(
            store
                .list(SESSION)
                .await
                .unwrap()
                .iter()
                .any(|record| record.id == fresh.id),
            "the record this put published was evicted by its own call"
        );
        assert!(store.read(SESSION, &fresh.id).await.is_ok());
    }

    /// The sidecar is the store's promise about the bytes; a blob that
    /// contradicts it must not be served as if it were whole.
    #[tokio::test]
    async fn read_rejects_a_blob_that_disagrees_with_its_sidecar() {
        let (store, guard) = test_store();
        let record = store
            .put(
                SESSION,
                None,
                ArtifactOrigin::Outbound,
                "a.txt",
                "text/plain",
                b"0123456789",
            )
            .await
            .unwrap();

        let dir = guard.path().join(encode_session_key(SESSION));
        std::fs::write(dir.join(format!("{}.bin", record.id)), b"012").unwrap();

        assert!(matches!(
            store.read(SESSION, &record.id).await.unwrap_err(),
            ArtifactError::Io(_)
        ));
    }
}
