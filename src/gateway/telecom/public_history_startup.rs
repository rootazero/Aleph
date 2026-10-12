//! Startup-only provisioning of encrypted public telecom history.
//!
//! The operator supplies a dedicated local key file; neither login credentials
//! nor the owner vault are consulted. This helper never installs a global store,
//! generates a key, or resets an existing database. Config/key changes need a
//! restart. Disabled settings return before any filesystem access.

use std::path::Path;

use crate::config::types::PublicHistoryStartupConfig;
use crate::sync_primitives::Arc;

use super::public_history::{PublicHistoryError, PublicHistoryStore};

/// Startup failures are non-fatal to ordinary public single-turn execution.
/// Display messages are static/redacted; production must not log source errors.
#[derive(Debug, thiserror::Error)]
pub enum PublicHistoryStartupError {
    #[error("invalid public history startup configuration")]
    InvalidConfig(#[source] PublicHistoryError),
    #[error("dedicated public history key file is not configured")]
    MissingKey,
    #[error("dedicated public history key file could not be opened or read")]
    KeyIo(#[source] std::io::Error),
    #[error("dedicated public history key file must be a regular file owned by this process with mode 0600")]
    UnsafeKeyFile,
    #[error("dedicated public history key must contain exactly 64 ASCII hex characters and at most one trailing newline")]
    MalformedKey,
    #[error("secure public history key-file loading is unsupported on this platform")]
    Unsupported,
    #[error("private public history directory could not be prepared")]
    DirectoryIo(#[source] std::io::Error),
    #[error("encrypted public history database could not be opened")]
    Store(#[source] PublicHistoryError),
}

/// Open `<data_dir>/telecom-public-history/public-history.db` using only the
/// configured dedicated key. No globals/environment variables are read or set.
///
/// `enabled = false` or `ttl_secs = 0` bypasses key, directory, and DB I/O.
/// Missing/unusable keys return an error before creating the history directory;
/// the startup caller warns and leaves public single-turn behavior usable.
pub fn open_configured_history(
    config: &PublicHistoryStartupConfig,
    data_dir: &Path,
) -> Result<Option<Arc<PublicHistoryStore>>, PublicHistoryStartupError> {
    if !config.enabled || config.ttl_secs == 0 {
        return Ok(None);
    }

    #[cfg(not(unix))]
    {
        let _ = data_dir;
        Err(PublicHistoryStartupError::Unsupported)
    }

    #[cfg(unix)]
    {
        let limits = config
            .store_config()
            .map_err(PublicHistoryStartupError::InvalidConfig)?;
        let key_file = config
            .key_file
            .as_deref()
            .ok_or(PublicHistoryStartupError::MissingKey)?;
        let crypto = open_secure_key_file(key_file)?;
        let private_dir =
            crate::utils::paths::ensure_private_dir(data_dir.join("telecom-public-history"))
                .map_err(PublicHistoryStartupError::DirectoryIo)?;
        let store =
            PublicHistoryStore::open(&private_dir.join("public-history.db"), crypto, limits)
                .map_err(PublicHistoryStartupError::Store)?;
        Ok(Some(Arc::new(store)))
    }
}

/// Open once and validate/read that FD, not a pathname checked before opening.
#[cfg(unix)]
fn open_secure_key_file(
    path: &Path,
) -> Result<crate::secrets::crypto::SecretsCrypto, PublicHistoryStartupError> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    use crate::secrets::crypto::SecretsCrypto;
    use zeroize::Zeroizing;

    // O_NONBLOCK avoids waiting indefinitely on a named pipe before fstat can
    // reject it; regular files are unaffected. O_NOFOLLOW refuses a final symlink.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(PublicHistoryStartupError::KeyIo)?;
    let metadata = file.metadata().map_err(PublicHistoryStartupError::KeyIo)?;
    // SAFETY: geteuid has no preconditions and does not access memory.
    let euid = unsafe { libc::geteuid() };
    if !secure_key_metadata(&metadata, euid) {
        return Err(PublicHistoryStartupError::UnsafeKeyFile);
    }
    if !(64..=65).contains(&metadata.len()) {
        return Err(PublicHistoryStartupError::MalformedKey);
    }
    // One extra byte detects concurrent growth beyond the valid 65-byte format.
    // The read never exceeds 66 bytes, regardless of metadata or file size.
    let mut bytes = Zeroizing::new(Vec::with_capacity(66));
    file.take(66)
        .read_to_end(&mut bytes)
        .map_err(PublicHistoryStartupError::KeyIo)?;
    let encoded = match bytes.as_slice() {
        key if key.len() == 64 => key,
        key if key.len() == 65 && key[64] == b'\n' => &key[..64],
        _ => return Err(PublicHistoryStartupError::MalformedKey),
    };
    if !encoded.iter().all(u8::is_ascii_hexdigit) {
        return Err(PublicHistoryStartupError::MalformedKey);
    }
    // SecretsCrypto accepts a String. Canonical hex represents the 32 key bytes
    // identically for upper/lowercase operator encodings; newline is not a key.
    let key = std::str::from_utf8(encoded)
        .map_err(|_| PublicHistoryStartupError::MalformedKey)?
        .to_ascii_lowercase();
    // The raw input is wiped on every exit; the moved key is held in SecretString.
    Ok(SecretsCrypto::new(key))
}

#[cfg(unix)]
fn secure_key_metadata(metadata: &std::fs::Metadata, euid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.is_file() && metadata.uid() == euid && metadata.mode() & 0o7777 == 0o600
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_and_zero_ttl_startup_do_not_touch_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("does-not-exist");
        for config in [
            PublicHistoryStartupConfig {
                key_file: Some(dir.path().join("missing.key")),
                ..Default::default()
            },
            PublicHistoryStartupConfig {
                enabled: true,
                ttl_secs: 0,
                key_file: Some(dir.path().join("missing.key")),
                ..Default::default()
            },
        ] {
            assert!(open_configured_history(&config, &data_dir)
                .unwrap()
                .is_none());
            assert!(!data_dir.exists());
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }

    #[cfg(not(unix))]
    #[test]
    fn enabled_history_is_unsupported_without_unix_fd_checks() {
        let dir = tempfile::tempdir().unwrap();
        let config = PublicHistoryStartupConfig {
            enabled: true,
            key_file: Some(dir.path().join("missing.key")),
            ..Default::default()
        };
        assert!(matches!(
            open_configured_history(&config, dir.path()),
            Err(PublicHistoryStartupError::Unsupported)
        ));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

        use crate::gateway::telecom::public_context::test_support::sample_public_context;
        use crate::gateway::telecom::public_history::{
            installed, PublicHistorySession, PublicHistoryTurn,
        };
        use chrono::{Duration, TimeZone, Utc};

        // Fixed test-only material, never a production key or provisioning default.
        const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

        fn write_key(path: &Path, bytes: &[u8]) {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .unwrap();
            file.write_all(bytes).unwrap();
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }

        fn config(key_file: &Path) -> PublicHistoryStartupConfig {
            PublicHistoryStartupConfig {
                enabled: true,
                key_file: Some(key_file.to_path_buf()),
                ..Default::default()
            }
        }

        #[test]
        fn secure_startup_reopens_two_turn_ciphertext_without_plaintext_or_globals() {
            let before = installed().map(Arc::as_ptr);
            let dir = tempfile::tempdir().unwrap();
            let key_file = dir.path().join("dedicated.key");
            write_key(&key_file, KEY.as_bytes());
            let config = config(&key_file);
            let session = PublicHistorySession::from_provenance(
                sample_public_context("startup-binding-canary-43").provenance(),
            );
            let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
            let first = PublicHistoryTurn {
                user: "startup-user-canary-lilac-31".into(),
                assistant: "startup-answer-canary-amber-59".into(),
            };
            let second = PublicHistoryTurn {
                user: "startup-user-canary-violet-62".into(),
                assistant: "startup-answer-canary-teal-47".into(),
            };
            let store = open_configured_history(&config, dir.path())
                .unwrap()
                .unwrap();
            store.append(&session, &first, now).unwrap();
            drop(store);
            // Format differences that encode the same key must not rotate it.
            write_key(
                &key_file,
                format!("{}\n", KEY.to_ascii_uppercase()).as_bytes(),
            );
            let store = open_configured_history(&config, dir.path())
                .unwrap()
                .unwrap();
            assert_eq!(store.load(&session, now).unwrap(), vec![first.clone()]);
            store
                .append(&session, &second, now + Duration::seconds(1))
                .unwrap();
            drop(store);
            let store = open_configured_history(&config, dir.path())
                .unwrap()
                .unwrap();
            assert_eq!(
                store.load(&session, now + Duration::seconds(2)).unwrap(),
                vec![first.clone(), second.clone()]
            );
            let history_dir = dir.path().join("telecom-public-history");
            assert_eq!(
                std::fs::metadata(&history_dir).unwrap().mode() & 0o7777,
                0o700
            );
            // Inspect DB + any WAL/SHM sidecars while the reopened store is live.
            for entry in std::fs::read_dir(&history_dir).unwrap() {
                let bytes = std::fs::read(entry.unwrap().path()).unwrap();
                for needle in [
                    first.user.as_str(),
                    first.assistant.as_str(),
                    second.user.as_str(),
                    second.assistant.as_str(),
                    "startup-binding-canary-43",
                    "+15550000001",
                    KEY,
                ] {
                    assert!(!bytes.windows(needle.len()).any(|w| w == needle.as_bytes()));
                }
            }
            assert_eq!(installed().map(Arc::as_ptr), before);
        }

        #[test]
        fn missing_or_unconfigured_key_never_creates_history_directory() {
            let dir = tempfile::tempdir().unwrap();
            let missing = dir.path().join("missing.key");
            assert!(matches!(
                open_configured_history(&config(&missing), dir.path()),
                Err(PublicHistoryStartupError::KeyIo(_))
            ));
            let absent = PublicHistoryStartupConfig {
                enabled: true,
                ..Default::default()
            };
            assert!(matches!(
                open_configured_history(&absent, dir.path()),
                Err(PublicHistoryStartupError::MissingKey)
            ));
            assert!(!missing.exists(), "startup must not generate a key");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }

        #[test]
        fn malformed_or_oversized_key_is_rejected_before_database_io() {
            let dir = tempfile::tempdir().unwrap();
            let key_file = dir.path().join("dedicated.key");
            for bytes in [
                Vec::new(),
                KEY.as_bytes()[..63].to_vec(),
                "g".repeat(64).into_bytes(),
                vec![0xff; 64],
                format!("{KEY} ").into_bytes(),
                format!("{KEY}\r\n").into_bytes(),
                format!("{KEY}\n\n").into_bytes(),
                "a".repeat(1024 * 1024).into_bytes(),
            ] {
                write_key(&key_file, &bytes);
                assert!(matches!(
                    open_configured_history(&config(&key_file), dir.path()),
                    Err(PublicHistoryStartupError::MalformedKey)
                ));
                assert!(!dir.path().join("telecom-public-history").exists());
            }
        }

        #[test]
        fn key_file_requires_exact_0600_and_regular_nonsymlink_file() {
            let dir = tempfile::tempdir().unwrap();
            let key_file = dir.path().join("dedicated.key");
            write_key(&key_file, KEY.as_bytes());
            for mode in [0o400, 0o640, 0o604, 0o644, 0o660, 0o600 | 0o4000] {
                std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(mode)).unwrap();
                assert!(matches!(
                    open_configured_history(&config(&key_file), dir.path()),
                    Err(PublicHistoryStartupError::UnsafeKeyFile)
                ));
            }
            std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600)).unwrap();
            let symlink = dir.path().join("key-link");
            std::os::unix::fs::symlink(&key_file, &symlink).unwrap();
            assert!(matches!(
                open_configured_history(&config(&symlink), dir.path()),
                Err(PublicHistoryStartupError::KeyIo(_))
            ));
            let directory = dir.path().join("key-directory");
            std::fs::create_dir(&directory).unwrap();
            assert!(matches!(
                open_configured_history(&config(&directory), dir.path()),
                Err(PublicHistoryStartupError::UnsafeKeyFile)
            ));
            assert!(!dir.path().join("telecom-public-history").exists());
        }

        #[test]
        fn fd_metadata_rejects_foreign_owner_without_privileged_chown() {
            let dir = tempfile::tempdir().unwrap();
            let key_file = dir.path().join("dedicated.key");
            write_key(&key_file, KEY.as_bytes());
            let file = std::fs::File::open(&key_file).unwrap();
            let metadata = file.metadata().unwrap();
            assert!(secure_key_metadata(&metadata, metadata.uid()));
            assert!(!secure_key_metadata(
                &metadata,
                metadata.uid().wrapping_add(1)
            ));
        }

        #[test]
        fn unsafe_history_directory_is_refused_without_adopting_or_repairing_it() {
            let dir = tempfile::tempdir().unwrap();
            let key_file = dir.path().join("dedicated.key");
            write_key(&key_file, KEY.as_bytes());
            let history_dir = dir.path().join("telecom-public-history");
            std::fs::create_dir(&history_dir).unwrap();
            std::fs::set_permissions(&history_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(matches!(
                open_configured_history(&config(&key_file), dir.path()),
                Err(PublicHistoryStartupError::DirectoryIo(_))
            ));
            assert_eq!(
                std::fs::metadata(&history_dir).unwrap().mode() & 0o7777,
                0o755
            );
            std::fs::remove_dir(&history_dir).unwrap();
            std::os::unix::fs::symlink(dir.path(), &history_dir).unwrap();
            assert!(matches!(
                open_configured_history(&config(&key_file), dir.path()),
                Err(PublicHistoryStartupError::DirectoryIo(_))
            ));
            assert!(!dir.path().join("public-history.db").exists());
        }

        #[test]
        fn invalid_startup_limits_or_relative_path_fail_before_io() {
            let dir = tempfile::tempdir().unwrap();
            let key_file = dir.path().join("missing.key");
            let mut settings = config(&key_file);
            settings.max_turns = 33;
            assert!(matches!(
                open_configured_history(&settings, dir.path()),
                Err(PublicHistoryStartupError::InvalidConfig(_))
            ));
            settings.max_turns = 32;
            settings.key_file = Some("relative.key".into());
            assert!(matches!(
                open_configured_history(&settings, dir.path()),
                Err(PublicHistoryStartupError::InvalidConfig(_))
            ));
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }

        #[test]
        fn lowered_startup_limits_reach_real_store() {
            let dir = tempfile::tempdir().unwrap();
            let key_file = dir.path().join("dedicated.key");
            write_key(&key_file, KEY.as_bytes());
            let mut settings = config(&key_file);
            settings.ttl_secs = 3;
            settings.max_turns = 1;
            settings.max_session_bytes = 16;
            settings.max_total_bytes = 8192;
            let session = PublicHistorySession::from_provenance(
                sample_public_context("limits-binding").provenance(),
            );
            let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
            let first = PublicHistoryTurn {
                user: "first".into(),
                assistant: "one".into(),
            };
            let second = PublicHistoryTurn {
                user: "second".into(),
                assistant: "two".into(),
            };
            let store = open_configured_history(&settings, dir.path())
                .unwrap()
                .unwrap();
            store.append(&session, &first, now).unwrap();
            store
                .append(&session, &second, now + Duration::seconds(1))
                .unwrap();
            assert_eq!(
                store.load(&session, now + Duration::seconds(2)).unwrap(),
                vec![second.clone()]
            );
            let oversized = PublicHistoryTurn {
                user: "x".repeat(17),
                assistant: String::new(),
            };
            assert!(matches!(
                store.append(&session, &oversized, now + Duration::seconds(2)),
                Err(PublicHistoryError::TurnTooLarge)
            ));
            assert!(store
                .load(&session, now + Duration::seconds(4))
                .unwrap()
                .is_empty());
            drop(store);
            settings.max_total_bytes = 1;
            let store = open_configured_history(&settings, dir.path())
                .unwrap()
                .unwrap();
            assert!(matches!(
                store.append(&session, &first, now),
                Err(PublicHistoryError::Capacity)
            ));
            assert!(store.load(&session, now).unwrap().is_empty());
        }

        #[test]
        fn wrong_key_startup_never_resets_database() {
            let dir = tempfile::tempdir().unwrap();
            let key_file = dir.path().join("dedicated.key");
            write_key(&key_file, KEY.as_bytes());
            let settings = config(&key_file);
            let session = PublicHistorySession::from_provenance(
                sample_public_context("wrong-key-binding").provenance(),
            );
            let now = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
            let turn = PublicHistoryTurn {
                user: "wrong-key-canary-user".into(),
                assistant: "wrong-key-canary-answer".into(),
            };
            let store = open_configured_history(&settings, dir.path())
                .unwrap()
                .unwrap();
            store.append(&session, &turn, now).unwrap();
            drop(store);
            write_key(&key_file, "b".repeat(64).as_bytes());
            let wrong = open_configured_history(&settings, dir.path())
                .unwrap()
                .unwrap();
            assert!(matches!(
                wrong.load(&session, now),
                Err(PublicHistoryError::Corrupt)
            ));
            drop(wrong);
            write_key(&key_file, KEY.as_bytes());
            let restored = open_configured_history(&settings, dir.path())
                .unwrap()
                .unwrap();
            assert_eq!(restored.load(&session, now).unwrap(), vec![turn]);
        }
    }
}
