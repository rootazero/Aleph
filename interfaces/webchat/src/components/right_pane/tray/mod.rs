//! The root state and browser persistence for explicitly pinned files.
//!
//! Metadata is a small localStorage projection. The bytes live in one IndexedDB
//! object store and are written completely before the metadata projection is
//! published. This module is the only tray storage authority used by the
//! composer, artifact rows, and the right-pane slot.

mod storage;
pub(crate) mod view;
pub(crate) use storage::read_attachment;
use storage::{delete_bytes, fetch_into_idb, persist_bytes_base64, read_bytes};
pub(crate) use view::{TrayPinArtifactButton, TrayPinDraftButton, TraySurface};

use leptos::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::api::artifacts::ArtifactItem;
use crate::platform::wide::views::chat::state::PendingAttachment;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const TRAY_STORAGE_KEY: &str = "aleph.tray.v1";
pub(crate) const TRAY_MAX_ITEMS: usize = 50;
pub(crate) const TRAY_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub(crate) const TRAY_MAX_ITEM_BYTES: u64 = 50 * 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TrayItem {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub origin: String,
    pub source: TraySource,
    pub pinned_at: i64,
    pub last_accessed_at: i64,
    pub blob_ref: BlobRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TraySource {
    DraftPin,
    ArtifactPin,
    UserUpload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum BlobRef {
    IndexedDb { key: String, size: u64 },
}

impl BlobRef {
    fn key(&self) -> &str {
        match self {
            Self::IndexedDb { key, .. } => key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoldedTray {
    pub visible: Vec<TrayItem>,
    pub hidden_count: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct TrayState {
    pub items: RwSignal<Vec<TrayItem>>,
    pub error: RwSignal<Option<String>>,
}

impl TrayState {
    #[must_use]
    pub(crate) fn new() -> Self {
        let state = Self {
            items: RwSignal::new(Vec::new()),
            error: RwSignal::new(None),
        };
        match load_metadata() {
            Ok(rows) => state.items.set(rows),
            Err(error) => {
                state
                    .error
                    .set(Some(crate::components::admin_refusal::settings_load_error(
                        crate::i18n::use_i18n(),
                        &error,
                        str::to_string,
                    )))
            }
        }
        state
    }

    pub(crate) async fn pin_draft(&self, attachment: PendingAttachment) -> Result<(), String> {
        if attachment.size > TRAY_MAX_ITEM_BYTES {
            return self.fail(format!(
                "file exceeds the {} MiB tray limit",
                TRAY_MAX_ITEM_BYTES / (1024 * 1024)
            ));
        }
        let id = draft_id(&attachment);
        let key = new_blob_key();
        if self
            .items
            .try_with_untracked(|rows| rows.iter().any(|row| row.id == id))
            .unwrap_or(false)
        {
            return self.refresh_access(&id);
        }
        let size = persist_bytes_base64(&key, &attachment.data_base64).await?;
        self.commit(TrayItem {
            id,
            name: attachment.name,
            mime_type: attachment.mime_type,
            size: size as u64,
            origin: "inbound".into(),
            source: TraySource::DraftPin,
            pinned_at: now_millis(),
            last_accessed_at: now_millis(),
            blob_ref: BlobRef::IndexedDb {
                key,
                size: size as u64,
            },
        })
        .await
    }

    pub(crate) async fn pin_artifact(&self, item: ArtifactItem) -> Result<(), String> {
        if item.size > TRAY_MAX_ITEM_BYTES {
            return self.fail(format!(
                "file exceeds the {} MiB tray limit",
                TRAY_MAX_ITEM_BYTES / (1024 * 1024)
            ));
        }
        let id = artifact_id(&item);
        let key = new_blob_key();
        if self
            .items
            .try_with_untracked(|rows| rows.iter().any(|row| row.id == id))
            .unwrap_or(false)
        {
            return self.refresh_access(&id);
        }
        // The capability URL is only a source. The durable value is the bytes
        // fetched now, because the gateway may expire that URL later.
        let size = fetch_into_idb(&item.url, &key).await?;
        self.commit(TrayItem {
            id,
            name: item.filename,
            mime_type: item.mime_type,
            size: size as u64,
            origin: item.origin,
            source: TraySource::ArtifactPin,
            pinned_at: now_millis(),
            last_accessed_at: now_millis(),
            blob_ref: BlobRef::IndexedDb {
                key,
                size: size as u64,
            },
        })
        .await
    }

    pub(crate) async fn pin_file(&self, file: web_sys::File) -> Result<(), String> {
        if file.size() as u64 > TRAY_MAX_ITEM_BYTES {
            return self.fail("Tray item exceeds 50 MiB".into());
        }
        let attachment = PendingAttachment {
            name: file.name(),
            mime_type: if file.type_().is_empty() {
                "application/octet-stream".into()
            } else {
                file.type_()
            },
            size: file.size() as u64,
            data_base64: read_attachment(&file).await?,
        };
        self.pin_upload(attachment).await
    }

    async fn pin_upload(&self, attachment: PendingAttachment) -> Result<(), String> {
        if attachment.size > TRAY_MAX_ITEM_BYTES {
            return self.fail(format!(
                "file exceeds the {} MiB tray limit",
                TRAY_MAX_ITEM_BYTES / (1024 * 1024)
            ));
        }
        let id = format!("upload:{}", stable_attachment_key(&attachment));
        if self
            .items
            .try_with_untracked(|rows| rows.iter().any(|row| row.id == id))
            .unwrap_or(false)
        {
            return self.refresh_access(&id);
        }
        let key = new_blob_key();
        let size = persist_bytes_base64(&key, &attachment.data_base64).await?;
        self.commit(TrayItem {
            id,
            name: attachment.name,
            mime_type: attachment.mime_type,
            size: size as u64,
            origin: "inbound".into(),
            source: TraySource::UserUpload,
            pinned_at: now_millis(),
            last_accessed_at: now_millis(),
            blob_ref: BlobRef::IndexedDb {
                key,
                size: size as u64,
            },
        })
        .await
    }

    async fn commit(&self, item: TrayItem) -> Result<(), String> {
        // IDB has completed. Merge against the latest root synchronously, persist
        // metadata, then publish: no await separates those three operations.
        let outcome = self
            .items
            .try_with_untracked(|rows| prepare_publish(rows, &item, write_metadata));
        match outcome {
            Some(Ok(Some(rows))) => {
                let _ = self.items.try_set(rows);
                let _ = self.error.try_set(None);
                Ok(())
            }
            Some(Ok(None)) => {
                let result = self.refresh_access(&item.id);
                let _ = delete_bytes(item.blob_ref.key()).await;
                result
            }
            other => {
                let _ = delete_bytes(item.blob_ref.key()).await;
                self.fail(
                    other
                        .and_then(Result::err)
                        .unwrap_or_else(|| "Tray owner disposed".into()),
                )
            }
        }
    }

    pub(crate) async fn remove(&self, item: TrayItem) -> Result<(), String> {
        let Some(mut rows) = self.items.try_get_untracked() else {
            return self.fail("Tray owner disposed".into());
        };
        rows.retain(|row| row.id != item.id);
        // Remove the reference first, keeping old bytes usable if metadata fails.
        if let Err(error) = write_metadata(&rows) {
            return self.fail(error);
        }
        let _ = self.items.try_set(rows);
        match delete_bytes(item.blob_ref.key()).await {
            Ok(()) => {
                let _ = self.error.try_set(None);
                Ok(())
            }
            Err(error) => self.fail(error), // safe orphan, no visible broken reference
        }
    }

    pub(crate) async fn read_bytes(&self, item: TrayItem) -> Result<Vec<u8>, String> {
        let bytes = match read_bytes(item.blob_ref.key()).await {
            Ok(bytes) => bytes,
            Err(error) => {
                return self.fail(error).map(|_| Vec::new());
            }
        };
        self.refresh_access(&item.id)?;
        Ok(bytes)
    }

    fn refresh_access(&self, id: &str) -> Result<(), String> {
        let prepared = self
            .items
            .try_with_untracked(|rows| prepare_access(rows, id, now_millis(), write_metadata));
        match prepared {
            Some(Ok(Some(rows))) => {
                let _ = self.items.try_set(rows);
                let _ = self.error.try_set(None);
                Ok(())
            }
            Some(Ok(None)) => Ok(()), // Removal during a read must not resurrect the item.
            Some(Err(error)) => self.fail(error),
            None => self.fail("Tray owner disposed".into()),
        }
    }

    pub(crate) async fn download(&self, item: TrayItem) -> Result<(), String> {
        let bytes = self.read_bytes(item.clone()).await?;
        storage::download(&bytes, &item.name, &item.mime_type)
    }

    fn fail(&self, error: String) -> Result<(), String> {
        let _ = self.error.try_set(Some(error.clone()));
        Err(error)
    }
}

fn draft_id(attachment: &PendingAttachment) -> String {
    format!("draft:{}", stable_attachment_key(attachment))
}

fn artifact_id(item: &ArtifactItem) -> String {
    format!("artifact:{}", item.id)
}

fn new_blob_key() -> String {
    // A second tab has an independent counter; salt avoids overwriting its bytes.
    #[cfg(target_arch = "wasm32")]
    let salt = js_sys::Math::random().to_bits();
    #[cfg(not(target_arch = "wasm32"))]
    let salt = 0_u64;
    format!(
        "blob:{}:{salt:x}:{}",
        now_millis(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

/// Return a publishable snapshot only after metadata persistence succeeds.
fn prepare_publish(
    rows: &[TrayItem],
    item: &TrayItem,
    persist: impl FnOnce(&[TrayItem]) -> Result<(), String>,
) -> Result<Option<Vec<TrayItem>>, String> {
    if rows.iter().any(|row| row.id == item.id) {
        return Ok(None);
    }
    let mut next = rows.to_vec();
    next.insert(0, item.clone());
    persist(&next)?;
    Ok(Some(next))
}

/// Refresh an existing item without changing its bytes or resurrecting a removed row.
fn prepare_access(
    rows: &[TrayItem],
    id: &str,
    now: i64,
    persist: impl FnOnce(&[TrayItem]) -> Result<(), String>,
) -> Result<Option<Vec<TrayItem>>, String> {
    let mut next = rows.to_vec();
    let Some(row) = next.iter_mut().find(|row| row.id == id) else {
        return Ok(None);
    };
    row.last_accessed_at = row.last_accessed_at.max(now);
    persist(&next)?;
    Ok(Some(next))
}

#[must_use]
pub(crate) fn folded_items(items: &[TrayItem], now: i64) -> FoldedTray {
    let cutoff = now.saturating_sub(TRAY_RETENTION_MS);
    let mut visible: Vec<_> = items
        .iter()
        .filter(|item| item.last_accessed_at >= cutoff)
        .cloned()
        .collect();
    visible.sort_by_key(|item| std::cmp::Reverse(item.last_accessed_at));
    visible.truncate(TRAY_MAX_ITEMS);
    FoldedTray {
        hidden_count: items.len().saturating_sub(visible.len()),
        visible,
    }
}

#[must_use]
pub(crate) fn stable_attachment_key(attachment: &PendingAttachment) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    attachment.name.hash(&mut hasher);
    attachment.mime_type.hash(&mut hasher);
    attachment.size.hash(&mut hasher);
    attachment.data_base64.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub(crate) fn now_millis() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0
    }
}

fn write_metadata(items: &[TrayItem]) -> Result<(), String> {
    let json = serde_json::to_string(items).map_err(|error| error.to_string())?;
    #[cfg(target_arch = "wasm32")]
    {
        let storage = web_sys::window()
            .ok_or_else(|| "browser window unavailable".to_string())?
            .local_storage()
            .map_err(|error| format!("localStorage unavailable: {error:?}"))?
            .ok_or_else(|| "localStorage unavailable".to_string())?;
        storage
            .set_item(TRAY_STORAGE_KEY, &json)
            .map_err(|error| format!("localStorage write failed: {error:?}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = json;
        Ok(())
    }
}

fn load_metadata() -> Result<Vec<TrayItem>, String> {
    #[cfg(target_arch = "wasm32")]
    {
        let Some(storage) = web_sys::window()
            .ok_or_else(|| "browser window unavailable".to_string())?
            .local_storage()
            .map_err(|error| format!("localStorage unavailable: {error:?}"))?
        else {
            return Err("localStorage unavailable".into());
        };
        let Some(raw) = storage
            .get_item(TRAY_STORAGE_KEY)
            .map_err(|error| format!("localStorage read failed: {error:?}"))?
        else {
            return Ok(Vec::new());
        };
        serde_json::from_str(&raw).map_err(|error| format!("tray metadata is invalid: {error}"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, pinned_at: i64) -> TrayItem {
        TrayItem {
            id: id.into(),
            name: id.into(),
            mime_type: "text/plain".into(),
            size: 1,
            origin: "inbound".into(),
            source: TraySource::UserUpload,
            pinned_at,
            last_accessed_at: pinned_at,
            blob_ref: BlobRef::IndexedDb {
                key: id.into(),
                size: 1,
            },
        }
    }

    #[test]
    fn fold_keeps_recent_boundary_and_reports_old_rows_without_deleting_them() {
        let now = 10_000_000;
        let rows = vec![
            item("new", now),
            item("boundary", now - TRAY_RETENTION_MS),
            item("old", now - TRAY_RETENTION_MS - 1),
        ];
        let folded = folded_items(&rows, now);
        assert_eq!(
            folded
                .visible
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["new", "boundary"]
        );
        assert_eq!(folded.hidden_count, 1);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn fold_limits_the_default_view_without_truncating_the_source() {
        let rows: Vec<_> = (0..(TRAY_MAX_ITEMS + 2))
            .map(|i| item(&i.to_string(), 1_000_000))
            .collect();
        let folded = folded_items(&rows, 1_000_000);
        assert_eq!(folded.visible.len(), TRAY_MAX_ITEMS);
        assert_eq!(folded.hidden_count, 2);
        assert_eq!(rows.len(), TRAY_MAX_ITEMS + 2);
    }

    #[test]
    fn recently_accessed_old_file_returns_to_the_front_of_the_capacity_window() {
        let now = TRAY_RETENTION_MS + 100;
        let mut rows: Vec<_> = (0..TRAY_MAX_ITEMS)
            .map(|i| item(&format!("recent-{i}"), now - 1))
            .collect();
        rows.push(item("old-but-reused", 0));
        assert_eq!(folded_items(&rows, now).hidden_count, 1);
        let refreshed = prepare_access(&rows, "old-but-reused", now, |next| {
            assert_eq!(next.last().unwrap().last_accessed_at, now);
            Ok(())
        })
        .unwrap()
        .unwrap();
        let folded = folded_items(&refreshed, now);
        assert_eq!(folded.visible[0].id, "old-but-reused");
        assert_eq!(folded.visible[0].pinned_at, 0);
        assert_eq!(folded.visible.len(), TRAY_MAX_ITEMS);
        assert_eq!(folded.hidden_count, 1);
        assert_eq!(rows.last().unwrap().last_accessed_at, 0);
    }

    #[test]
    fn access_failure_preserves_root_and_access_does_not_resurrect_removed_rows() {
        let rows = vec![item("old", 1)];
        assert_eq!(
            prepare_access(&rows, "old", 20, |_| Err("quota exceeded".into())).unwrap_err(),
            "quota exceeded"
        );
        assert_eq!(rows[0].last_accessed_at, 1);
        assert!(
            prepare_access(&rows, "removed", 20, |_| panic!("no resurrection"))
                .unwrap()
                .is_none()
        );
        let refreshed = prepare_access(&rows, "old", 20, |_| Ok(()))
            .unwrap()
            .unwrap();
        let backwards = prepare_access(&refreshed, "old", 2, |_| Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(backwards[0].last_accessed_at, 20);
        assert_eq!(backwards[0].blob_ref, rows[0].blob_ref);
    }

    #[test]
    fn async_completions_merge_latest_state_and_deduplicate_ids() {
        let a = item("a", 1);
        let b = item("b", 2);
        let mut rows = vec![];
        rows = prepare_publish(&rows, &b, |_| Ok(())).unwrap().unwrap();
        rows = prepare_publish(&rows, &a, |_| Ok(())).unwrap().unwrap();
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(
            prepare_publish(&rows, &a, |_| panic!("duplicate must not persist"))
                .unwrap()
                .is_none()
        );
        let encoded = serde_json::to_string(&rows).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<TrayItem>>(&encoded).unwrap(),
            rows
        );
    }

    #[test]
    fn failed_metadata_publication_preserves_previous_root() {
        let old = vec![item("old", 1)];
        let result = prepare_publish(&old, &item("new", 2), |_| Err("quota exceeded".into()));
        assert_eq!(result.unwrap_err(), "quota exceeded");
        assert_eq!(old, vec![item("old", 1)]);
    }

    #[test]
    fn native_persistence_does_not_fake_success() {
        let owner = Owner::new();
        owner.with(|| {
            let tray = TrayState::new();
            let result = futures::executor::block_on(tray.pin_upload(PendingAttachment {
                name: "x".into(),
                mime_type: "text/plain".into(),
                size: 1,
                data_base64: "eA==".into(),
            }));
            assert!(result.unwrap_err().contains("unavailable"));
            assert!(tray.items.get_untracked().is_empty());
        });
    }

    #[test]
    fn metadata_serialization_contains_only_an_indexed_db_reference() {
        let row = item("x", 1);
        let encoded = serde_json::to_string(&row).unwrap();
        assert!(encoded.contains("IndexedDb"));
        assert!(!encoded.contains("base64"));
    }
}
