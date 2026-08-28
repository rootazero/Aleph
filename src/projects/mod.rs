//! Project rooms.
//!
//! A `Project` is a room: a name, an owner, a roster, and optionally a bound
//! workspace directory. Membership IS the authorization (spec §6.1) — there
//! are no per-resource grants in v1 — and the room's scope id
//! (`project:<id>`) is what sessions and memory partitions hang off.
//!
//! **One table, two views.** The desktop Panel's "recent working directory"
//! picker is this same catalogue filtered to rows with a `workspace_path`,
//! ordered by `last_used_at`; the project sidebar is the same table filtered
//! by roster. They are deliberately not two entities — "project" meaning two
//! different things in one codebase is the confusion this promotion exists to
//! remove.
//!
//! Persistence: `~/.aleph/data/projects.db` (SQLite). The pre-P2
//! `~/.aleph/projects.json` catalogue is adopted once at boot and renamed
//! aside; see [`ProjectStore::migrate_from_json`].
//!
//! [`roster`] is a read-optimised projection of the membership table, NOT a
//! second source of truth — read its module doc before touching it.

pub mod attribution_backfill;
pub mod authz;
pub mod binding;
pub mod events;
pub mod roster;
mod run_context;
mod store;

pub use attribution_backfill::{backfill_legacy_room_attribution, BackfillReport};
pub use binding::{peer_kind_str, ChannelBinding};
pub use run_context::{current as current_project_root, with_project_root};
pub use store::{Project, ProjectError, ProjectStatus, ProjectStore};
