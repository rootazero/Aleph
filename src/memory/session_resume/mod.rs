//! Session Resume — save and restore conversation context across sessions.
pub mod reader;
pub mod snapshot;
pub mod writer;

pub use reader::SnapshotReader;
pub use snapshot::SessionSnapshot;
pub use writer::SnapshotWriter;

/// Sanitize a session id into a filesystem-safe directory name.
///
/// Session ids are gateway keys like `agent:main:main`. Besides the
/// path-escape characters (`/`, `\`, NUL, `..`), the Windows-reserved set
/// (`:` `<` `>` `"` `|` `?` `*`) must be replaced too — `:` alone would make
/// `create_dir_all` fail on every session end there. The writer uses this for
/// the snapshot directory name and the reader applies the SAME mapping to the
/// exclude comparison, so current-session exclusion matches the on-disk names.
#[must_use]
pub fn sanitize_session_id(id: &str) -> String {
    id.replace(['/', '\\', '\0', ':', '<', '>', '"', '|', '?', '*'], "_")
        .replace("..", "__")
}

#[cfg(test)]
mod tests {
    use super::sanitize_session_id;

    #[test]
    fn sanitize_replaces_windows_reserved_characters() {
        assert_eq!(sanitize_session_id("agent:main:main"), "agent_main_main");
        assert_eq!(sanitize_session_id(r#"a<b>c"d|e?f*g"#), "a_b_c_d_e_f_g");
    }

    #[test]
    fn sanitize_replaces_path_escapes() {
        assert_eq!(sanitize_session_id("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_session_id("..secret"), "__secret");
        assert_eq!(sanitize_session_id("a\0b"), "a_b");
    }

    #[test]
    fn sanitize_leaves_safe_ids_unchanged() {
        assert_eq!(sanitize_session_id("session-abc_01"), "session-abc_01");
    }
}
