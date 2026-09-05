//! Debug Handlers
//!
//! Debug and testing endpoints for architecture validation.
//!
//! These handlers are intended for development and testing purposes only.
//! They should be disabled in production deployments.

//! Debug Handlers
//!
//! Debug and testing endpoints for architecture validation.
//!
//! These handlers are intended for development and testing purposes only.
//! They should be disabled in production deployments.
//!
//! Note (severed-wire audit 2026-09-04, sw-gateway-1-2): the
//! `parse_tool_call_params` helper + the `DebugToolCallParams`/`DebugToolCallResult`
//! request/response structs had zero callers anywhere in src/, tests/,
//! interfaces/, or desktop/. The doc comment on `parse_tool_call_params`
//! said "JsonRpcResponse is 152+ bytes but boxing it would complicate all
//! handler call sites" — i.e. the helper was designed to be called by a
//! `handle_debug_tool_call` RPC that never landed. Until that RPC is
//! designed (and registered), the parse helper + the two DTOs are dormant
//! scaffolding. The `pub mod debug` declaration is preserved so the
//! planned RPC has a home; once it lands it should re-introduce a
//! parameter parser alongside.
