//! Session Resume — save and restore conversation context across sessions.
pub mod reader;
pub mod snapshot;
pub mod writer;

pub use reader::SnapshotReader;
pub use snapshot::SessionSnapshot;
pub use writer::SnapshotWriter;

/// The memory partition a session snapshot belongs to, for the agent whose
/// base id is `agent_id`.
///
/// **This is the one derivation both the writer and the reader call.** Every
/// other source the assembler gathers is partitioned (notes go through
/// `project_scope::session_read_ids`, the profile floor through
/// `profile_floor_id`); the snapshot was the one that was not, so one user's
/// `/end-summary` was injected verbatim into another user's system prompt.
///
/// Derived through [`crate::memory::project_scope::session_write_id`] — the
/// same family every other memory seam partitions by — with the **legacy
/// project-directory axis deliberately switched off** (`project_scoped =
/// false`, no root). Two reasons, and neither is a shortcut:
///
/// 1. That family is org-tier and shared by design —
///    [`crate::gateway::visibility::partition_visible`] admits `proj-*` for
///    every caller. The isolation boundary a session summary must not cross is
///    the session-scope axis (`u-*` personal / `p-*` room), which
///    `session_write_id` resolves from the ambient scope alone.
/// 2. The writer runs at session end with no `MemoryConfig` in hand. Passing
///    the toggle through on the read side only would give the two sides
///    different answers for the same session, which is the failure this
///    function exists to make impossible.
#[must_use]
pub fn snapshot_partition(agent_id: &str) -> String {
    crate::memory::project_scope::session_write_id(agent_id, false, None)
}

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
