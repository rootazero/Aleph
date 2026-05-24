//! Project workspace registry.
//!
//! Maintains a small JSON-backed catalogue of user-chosen project
//! folders so the desktop Panel can offer a "进入项目工作 → 新建空白项目 /
//! 使用现有文件夹" picker without re-asking on every send.
//!
//! Each `Project` simply pairs a user-friendly `name` with the absolute
//! filesystem path that becomes `RunRequest.workspace_override` for any
//! chat run launched from it. The store is intentionally tiny — it does
//! not own the directory contents, it only remembers which directories
//! the user has opted into as Aleph projects, and when each was last
//! used.
//!
//! Persistence: `~/.aleph/projects.json`, atomic writes, fs2 advisory
//! lock on the sidecar `.lock` so concurrent CLI/Panel writes are safe.

mod store;

pub use store::{
    default_projects_path, project_id_for_path, Project, ProjectError, ProjectStore,
    RECENT_PROJECTS_CAP,
};
