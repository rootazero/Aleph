//! Cron job RPC handlers.
//!
//! Handlers for cron job operations: list, get, create, update, delete,
//! status, run, runs, toggle.
//!
//! Each method has two variants:
//! - `handle_xxx_stub`: stateless stubs returning fake/empty data (used in HandlerRegistry::new())
//! - `handle_xxx`: real handlers that delegate to `CronService` via `SharedCronService`
//!
//! External callers keep using `cron::handle_xxx` paths unchanged — the
//! split into `real.rs` / `stubs.rs` is private; only the re-exports below
//! are public.

mod real;
mod stubs;

pub use real::{
    handle_create, handle_delete, handle_get, handle_list, handle_run, handle_runs, handle_status,
    handle_toggle, handle_update,
};
pub use stubs::{
    handle_create_stub, handle_delete_stub, handle_get_stub, handle_list_stub, handle_run_stub,
    handle_runs_stub, handle_status_stub, handle_toggle_stub, handle_update_stub,
};
