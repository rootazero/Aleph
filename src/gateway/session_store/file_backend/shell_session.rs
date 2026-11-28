//! File-store witness for the paired Shell input effect.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Arc;

use crate::gateway::router::SessionKey;
use crate::gateway::session_store::error::SessionStoreError;
use crate::gateway::session_store::types::SessionMetadata;

use super::{meta::MetaGuard, FileSessionStore};

/// Process-local identity of a file-store session. Holding the old directory
/// open prevents inode reuse, even if its key is immediately recreated.
#[doc(hidden)]
#[derive(Clone)]
pub struct ShellSessionIdentity {
    key: SessionKey,
    slot: Arc<tokio::sync::Mutex<()>>,
    _directory: Arc<File>,
    device: u64,
    inode: u64,
}

/// Holds the existing file-store mutation boundary through input commit.
#[doc(hidden)]
pub struct ShellSessionGuard {
    guard: MetaGuard,
    identity: ShellSessionIdentity,
}

impl ShellSessionGuard {
    pub(crate) fn identity(&self) -> &ShellSessionIdentity {
        &self.identity
    }

    pub(crate) fn metadata(&self) -> Option<&SessionMetadata> {
        self.guard.existing()
    }
}

impl FileSessionStore {
    pub(super) async fn shell_session_guard(
        &self,
        key: &SessionKey,
        expected: Option<&ShellSessionIdentity>,
    ) -> Result<Option<ShellSessionGuard>, SessionStoreError> {
        // Background/derived sessions also have directory reapers. They are
        // outside the initial paired Main-session profile.
        if !matches!(key, SessionKey::Main { .. })
            || expected.is_some_and(|identity| &identity.key != key)
        {
            return Ok(None);
        }
        let label = key.to_key_string();
        let Some(guard) = self
            .metadata_locks
            .try_lock_existing(&label, self.metadata_path(&label))
            .await?
        else {
            return Ok(None);
        };
        if guard
            .existing()
            .is_none_or(|metadata| metadata.key != label)
            || expected.is_some_and(|identity| !Arc::ptr_eq(&identity.slot, guard.mutex()))
        {
            return Ok(None);
        }

        let Some((directory, device, inode)) =
            open_session_directory(self.session_dir(&label)).await?
        else {
            return Ok(None);
        };
        if expected.is_some_and(|identity| identity.device != device || identity.inode != inode) {
            return Ok(None);
        }
        let identity = ShellSessionIdentity {
            key: key.clone(),
            slot: guard.mutex().clone(),
            _directory: Arc::new(directory),
            device,
            inode,
        };
        Ok(Some(ShellSessionGuard { guard, identity }))
    }
}

async fn open_session_directory(
    path: PathBuf,
) -> Result<Option<(File, u64, u64)>, SessionStoreError> {
    tokio::task::spawn_blocking(move || {
        let directory = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => file,
            Err(_) => return None,
        };
        let metadata = directory.metadata().ok()?;
        Some((directory, metadata.dev(), metadata.ino()))
    })
    .await
    .map_err(|error| SessionStoreError::DatabaseError(format!("Session identity task: {error}")))
}
