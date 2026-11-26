//! The Gateway the app connects to, kept across launches in
//! `<home>/.aleph/.desktop-shell-target`. A missing or unreadable file means
//! the bundled local server. Callers pass the home directory, so tests point
//! these functions at a temporary directory and never at the user's home.

use aleph_client::ConnectionTarget;
use std::io::Write;
use std::path::{Path, PathBuf};

fn marker(home: &Path) -> PathBuf {
    home.join(".aleph").join(".desktop-shell-target")
}

/// The saved target; `Local` when nothing usable is saved.
pub(crate) fn load_target(home: &Path) -> ConnectionTarget {
    match std::fs::read_to_string(marker(home)) {
        Ok(saved) => ConnectionTarget::parse(&saved).unwrap_or(ConnectionTarget::Local),
        Err(_) => ConnectionTarget::Local,
    }
}

/// Saves `target`. `Local` is the default, so saving it removes the file;
/// a remote target replaces the file through a rename, so a reader sees the
/// old contents or the new ones, never a partial write.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the connect form (plan Task 7) is the first caller")
)]
pub(crate) fn save_target(home: &Path, target: &ConnectionTarget) -> std::io::Result<()> {
    let marker = marker(home);
    if matches!(target, ConnectionTarget::Local) {
        return match std::fs::remove_file(&marker) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            outcome => outcome,
        };
    }
    let directory = home.join(".aleph");
    std::fs::create_dir_all(&directory)?;
    let staging = directory.join(format!(".desktop-shell-target.{}.tmp", std::process::id()));
    let written = std::fs::File::create(&staging).and_then(|mut file| {
        file.write_all(target.to_persisted().as_bytes())?;
        file.sync_all()
    });
    let outcome = written.and_then(|()| std::fs::rename(&staging, &marker));
    if outcome.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::{load_target, save_target};
    use aleph_client::ConnectionTarget;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A fresh directory under the OS temp dir standing in for the user's
    /// home; the only thing it ever deletes is itself.
    struct TestHome(PathBuf);

    impl TestHome {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aleph-nsh-target-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn marker(&self) -> PathBuf {
            self.0.join(".aleph").join(".desktop-shell-target")
        }

        fn write_marker(&self, contents: &str) {
            std::fs::create_dir_all(self.0.join(".aleph")).unwrap();
            std::fs::write(self.marker(), contents).unwrap();
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_file_is_local() {
        let home = TestHome::new();
        assert!(load_target(home.path()) == ConnectionTarget::Local);
    }

    #[test]
    fn remote_round_trips() {
        let home = TestHome::new();
        let remote = ConnectionTarget::parse("https://gw.example.com").unwrap();
        save_target(home.path(), &remote).unwrap();
        assert!(load_target(home.path()) == remote);
    }

    #[test]
    fn saving_local_removes_file() {
        let home = TestHome::new();
        let remote = ConnectionTarget::parse("https://gw.example.com").unwrap();
        save_target(home.path(), &remote).unwrap();
        assert!(home.marker().exists());
        save_target(home.path(), &ConnectionTarget::Local).unwrap();
        assert!(!home.marker().exists());
        assert!(load_target(home.path()) == ConnectionTarget::Local);
    }

    #[test]
    fn saving_local_without_a_file_succeeds() {
        let home = TestHome::new();
        save_target(home.path(), &ConnectionTarget::Local).unwrap();
        assert!(!home.marker().exists());
    }

    #[test]
    fn unreadable_contents_are_local() {
        for contents in ["", "\u{0}garbage", "ftp://x", "   \n"] {
            let home = TestHome::new();
            home.write_marker(contents);
            assert!(
                load_target(home.path()) == ConnectionTarget::Local,
                "{contents:?} should read as Local"
            );
        }
    }
}
