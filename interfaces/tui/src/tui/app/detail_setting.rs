//! Device-local transcript detail preference, not conversation state.

use std::path::{Path, PathBuf};

use shared_ui_logic::transcript::DetailLevel;

fn detail_file(root: &Path) -> PathBuf {
    root.join("tui-detail")
}

pub fn load_at(root: &Path, env: Option<&str>) -> DetailLevel {
    env.and_then(DetailLevel::parse)
        .or_else(|| {
            std::fs::read_to_string(detail_file(root))
                .ok()
                .as_deref()
                .and_then(DetailLevel::parse)
        })
        .unwrap_or_default()
}

pub fn remember_at(root: &Path, level: DetailLevel) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let path = detail_file(root);
    std::fs::write(&path, level.as_str())?;
    Ok(path)
}

/// Never read a developer's device preference in a test binary.
#[cfg(not(test))]
pub fn boot() -> DetailLevel {
    aleph_protocol::paths::aleph_home().map_or_else(DetailLevel::default, |root| {
        load_at(&root, std::env::var("ALEPH_TUI_DETAIL").ok().as_deref())
    })
}

#[cfg(test)]
pub fn boot() -> DetailLevel {
    DetailLevel::default()
}

pub fn remember(level: DetailLevel) -> std::io::Result<PathBuf> {
    let root = aleph_protocol::paths::aleph_home().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "no home directory for tui-detail",
        )
    })?;
    remember_at(&root, level)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_root_uses_environment_without_writing_to_home() {
        // An impossible file beneath an injected root: tests must not create
        // or remove even a temporary directory without an owned TempDir.
        let root = Path::new("/nonexistent-aleph-tui-detail-test-root");
        assert_eq!(load_at(root, Some("full")), DetailLevel::Full);
        assert_eq!(load_at(root, Some("brief")), DetailLevel::Brief);
    }
}
