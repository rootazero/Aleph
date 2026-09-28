//! The files that hold a human-only gate's state are beyond the model's file
//! tools — driven through `file_write` and `file_ops`, the way the registry
//! dispatches them (`call_json`), each tool built under an isolated
//! `$ALEPH_HOME` so its denylist names that home's files. `$HOME` is left
//! alone: every gate file hangs off `$ALEPH_HOME`, and a test that moved
//! `$HOME` would race the unguarded `~/.ssh` denylist tests of this module
//! (it did, the first time this ran beside them).

use crate::builtin_tools::file_ops::{FileOpsTool, FileWriteTool};
use crate::tools::AlephTool;
use crate::utils::paths::IsolatedAlephHome;
use serde_json::json;
use std::path::PathBuf;

/// Every gate-state file, from the store that owns it.
fn gate_files() -> Vec<(&'static str, PathBuf)> {
    vec![
        (
            "shell consent registry",
            crate::extension::hooks::ShellHookConsent::default_path(),
        ),
        (
            "exec-approval grants",
            crate::sandbox::exec_approval::grants::GrantStore::default_path(),
        ),
        (
            "config approval policy",
            crate::approval::ConfigApprovalPolicy::config_path(),
        ),
        (
            "plugin enable/trust state",
            crate::extension::plugin_state::PluginsConfig::default_path().unwrap(),
        ),
    ]
}

/// `file_write` refuses every gate-state file and writes nothing there, and
/// `file_ops delete` refuses each and leaves it in place; a sibling file in
/// the same home is written (the control).
#[tokio::test]
async fn the_models_file_tools_cannot_write_a_human_only_gate() {
    let _env = IsolatedAlephHome::new();
    let home = crate::utils::paths::get_config_dir().unwrap();
    let write = FileWriteTool::new();
    let ops = FileOpsTool::new();

    for (what, path) in gate_files() {
        assert!(path.starts_with(&home), "{what}: {}", path.display());
        let written = write
            .call_json(json!({ "file_path": path, "content": "{\"entries\":[]}" }))
            .await;
        assert!(
            written.is_err(),
            "{what}: file_write wrote {}",
            path.display()
        );
        assert!(!path.exists(), "{what}: {} was created", path.display());

        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "operator's").unwrap();
        let deleted = ops
            .call_json(json!({ "operation": "delete", "path": path }))
            .await;
        assert!(
            deleted.is_err(),
            "{what}: file_ops deleted {}",
            path.display()
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "operator's",
            "{what}"
        );
    }

    let sibling = home.join("notes.txt");
    write
        .call_json(json!({ "file_path": sibling, "content": "ok" }))
        .await
        .expect("a sibling file in the same home is writable");
    assert_eq!(std::fs::read_to_string(&sibling).unwrap(), "ok");
}

/// `name` with every other letter upper-cased (`sHeLl-…`).
#[cfg(any(target_os = "macos", windows))]
fn mixed_case(name: &str) -> String {
    name.chars()
        .enumerate()
        .map(|(i, c)| {
            if i % 2 == 1 {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

/// Review P4.17 R2-I-1. With a gate file ABSENT — a fresh install never
/// creates the consent registry, the always-grants or the approval policy
/// until a human does something — an upper-case or mixed-case spelling of
/// its name is, on a case-insensitive volume, the file its store then reads.
/// `file_write` refuses both spellings, and the gate file still does not
/// exist.
#[cfg(any(target_os = "macos", windows))]
#[tokio::test]
async fn a_case_variant_of_an_absent_gate_file_is_refused() {
    let _env = IsolatedAlephHome::new();
    let write = FileWriteTool::new();

    for (what, path) in gate_files() {
        assert!(!path.exists(), "{what}: {} exists", path.display());
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for variant in [name.to_uppercase(), mixed_case(&name)] {
            let alias = path.with_file_name(&variant);
            let written = write
                .call_json(json!({ "file_path": alias, "content": "{\"entries\":[]}" }))
                .await;
            assert!(written.is_err(), "{what}: file_write wrote {variant}");
            assert!(!path.exists(), "{what}: {variant} created the gate file");
        }
    }
}

/// The same hole in an absent credential leaf, the class this round's gate
/// files joined: a `~/.netrc` entry (here in a tempdir, so the real home is
/// never written) refuses `.NETRC` and `.NetRc` while nothing is there, and a
/// file whose name matches no entry in any case is written (the control).
#[cfg(any(target_os = "macos", windows))]
#[tokio::test]
async fn a_case_variant_of_an_absent_credential_leaf_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let netrc = home.path().join(".netrc");
    let write = FileWriteTool::with_denied_paths(vec![netrc.to_string_lossy().into_owned()]);

    for variant in [".NETRC", ".NetRc"] {
        let written = write
            .call_json(json!({ "file_path": home.path().join(variant), "content": "machine x" }))
            .await;
        assert!(written.is_err(), "file_write wrote {variant}");
        assert!(!netrc.exists(), "{variant} created .netrc");
    }

    let other = home.path().join("NOTES.txt");
    write
        .call_json(json!({ "file_path": other, "content": "ok" }))
        .await
        .expect("a name no entry covers is writable in any case");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "ok");
}
