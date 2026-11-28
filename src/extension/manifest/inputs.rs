//! Files observed during one synchronous package parse.
//!
//! The reader records inputs before use, including absent paths and directories
//! whose entries are enumerated. The existing component parsers also run outside
//! package ingestion, so recording is scoped to the synchronous parse on this
//! thread and ends before control returns to the caller.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{self, ReadDir};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use super::manifest_cache::FileStamp;

#[derive(Debug, PartialEq, Eq)]
struct PathStamp {
    entry: Result<FileStamp, ErrorKind>,
    target: Option<Result<FileStamp, ErrorKind>>,
}

impl PathStamp {
    fn read(path: &Path) -> Self {
        match fs::symlink_metadata(path) {
            Ok(meta) => Self {
                entry: Ok(FileStamp::from_metadata(&meta)),
                target: meta.is_symlink().then(|| {
                    fs::metadata(path)
                        .map(|meta| FileStamp::from_metadata(&meta))
                        .map_err(|error| error.kind())
                }),
            },
            Err(error) => Self {
                entry: Err(error.kind()),
                target: None,
            },
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct SourceObservations {
    paths: HashMap<PathBuf, PathStamp>,
    canonical: HashMap<PathBuf, Result<PathBuf, ErrorKind>>,
}

impl SourceObservations {
    pub(super) fn verify(&self) -> anyhow::Result<()> {
        for (path, before) in &self.paths {
            anyhow::ensure!(
                PathStamp::read(path) == *before,
                "Plugin input changed while reading {}",
                path.display()
            );
        }
        for (path, before) in &self.canonical {
            anyhow::ensure!(
                fs::canonicalize(path).map_err(|error| error.kind()) == *before,
                "Plugin path changed while reading {}",
                path.display()
            );
        }
        Ok(())
    }
}

thread_local! {
    static OBSERVATIONS: RefCell<Option<SourceObservations>> = const { RefCell::new(None) };
}

pub(super) fn capture<T>(read: impl FnOnce() -> T) -> (T, SourceObservations) {
    let outer = OBSERVATIONS.with(|cell| cell.replace(Some(SourceObservations::default())));
    struct Restore(Option<SourceObservations>);
    impl Drop for Restore {
        fn drop(&mut self) {
            OBSERVATIONS.with(|cell| cell.replace(self.0.take()));
        }
    }
    let guard = Restore(outer);
    let result = read();
    let observed = OBSERVATIONS.with(|cell| cell.take().expect("package observation is active"));
    drop(guard);
    (result, observed)
}

fn observe(path: &Path) {
    OBSERVATIONS.with_borrow_mut(|active| {
        if let Some(active) = active.as_mut() {
            if !active.paths.contains_key(path) {
                active.paths.insert(path.to_owned(), PathStamp::read(path));
            }
        }
    });
}

pub(crate) fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let path = path.as_ref();
    observe(path);
    let result = fs::canonicalize(path);
    OBSERVATIONS.with_borrow_mut(|active| {
        if let Some(active) = active.as_mut() {
            if !active.canonical.contains_key(path) {
                active.canonical.insert(
                    path.to_owned(),
                    result.as_ref().cloned().map_err(|error| error.kind()),
                );
            }
        }
    });
    result
}

pub(crate) fn is_file(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    observe(path);
    path.is_file()
}

pub(super) fn is_dir(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    observe(path);
    path.is_dir()
}

pub(super) fn exists(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref();
    observe(path);
    path.exists()
}

pub(super) fn read_to_string(path: impl AsRef<Path>) -> io::Result<String> {
    let path = path.as_ref();
    observe(path);
    fs::read_to_string(path)
}

pub(super) fn read_dir(path: impl AsRef<Path>) -> io::Result<ReadDir> {
    let path = path.as_ref();
    observe(path);
    fs::read_dir(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_changes_to_an_observed_file_before_parse_finishes() {
        let root = tempfile::tempdir().unwrap();
        let manifest = root.path().join("plugin.json");
        fs::write(&manifest, "before").unwrap();

        let ((first, second), observed) = capture(|| {
            let first = read_to_string(&manifest).unwrap();
            fs::write(&manifest, "after replacement").unwrap();
            let second = read_to_string(&manifest).unwrap();
            (first, second)
        });

        assert_eq!(first, "before");
        assert_eq!(second, "after replacement");
        assert!(observed.verify().is_err());
    }
}
