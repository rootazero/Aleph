//! Cron job RPC handlers.
//!
//! Handlers for cron job operations: list, get, create, update, delete,
//! status, run, runs, toggle.
//!
//! `HandlerRegistry::new()` used to register `handle_xxx_stub` placeholders
//! that the boot path overrode at phase 2; the stub file was severed in
//! the 2026-08-17 audit (audit finding sw-gateway-batch1-F01). The real
//! handlers below are the only surface now.

mod real;

pub use real::{
    handle_create, handle_delete, handle_get, handle_list, handle_run, handle_runs, handle_status,
    handle_toggle, handle_update,
};
