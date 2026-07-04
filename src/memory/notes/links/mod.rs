//! Link subsystem policy layer — pure functions, zero storage coupling (P4).
//! Resolution strategy chain (`resolve`) + unlinked-mention scanner
//! (`mentions`, Task 15). SQL plumbing stays in `store/sqlite/notes/`;
//! lifecycle triggers are wired at `indexer` / `note_manage` / gateway.

pub mod mentions;
pub mod resolve;

pub use resolve::{
    normalize_link_key, resolve, LinkResolveContext, LinkStatus, ResolveStrategy, ResolvedLink,
};
