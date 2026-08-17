//! Boot wiring for the whiteboard canvas RPC family (`canvas.*`).
//!
//! Overrides the phase-1 `SERVICE_UNAVAILABLE` placeholders that
//! `HandlerRegistry::new()` registers, binding all eight methods to the one
//! boot-built [`alephcore::canvas::CanvasStore`] — the same Arc the `canvas`
//! builtin tool receives (Task 10), so the tool face and the RPC face share
//! the instance that owns the event bus (`workspace_manage` doc precedent:
//! two instances would split the per-canvas critical sections in two).

use std::sync::Arc;

use alephcore::gateway::GatewayServer;

// ─── register_canvas_handlers ────────────────────────────────────────────────

/// Wire the whiteboard canvas surface (`<data_dir>/canvas`).
pub(in crate::commands::start) fn register_canvas_handlers(
    server: &mut GatewayServer,
    canvas_store: &Arc<alephcore::canvas::CanvasStore>,
    daemon: bool,
) {
    use alephcore::gateway::handlers::canvas as canvas_handlers;

    register_handler!(
        server,
        "canvas.create",
        canvas_handlers::handle_create,
        canvas_store
    );
    register_handler!(
        server,
        "canvas.list",
        canvas_handlers::handle_list,
        canvas_store
    );
    register_handler!(
        server,
        "canvas.get",
        canvas_handlers::handle_get,
        canvas_store
    );
    register_handler!(
        server,
        "canvas.apply",
        canvas_handlers::handle_apply,
        canvas_store
    );
    register_handler!(
        server,
        "canvas.delete",
        canvas_handlers::handle_delete,
        canvas_store
    );
    register_handler!(
        server,
        "canvas.asset.put",
        canvas_handlers::handle_asset_put,
        canvas_store
    );
    register_handler!(
        server,
        "canvas.asset.get",
        canvas_handlers::handle_asset_get,
        canvas_store
    );
    register_handler!(
        server,
        "canvas.selection.set",
        canvas_handlers::handle_selection_set,
        canvas_store
    );

    if !daemon {
        println!("Canvas methods:");
        println!("  - canvas.create        : Create a whiteboard canvas (owner-stamped)");
        println!("  - canvas.list          : List the caller's visible canvases");
        println!("  - canvas.get           : Fetch a canvas document + live selection");
        println!("  - canvas.apply         : Apply ops (optimistic revision check)");
        println!("  - canvas.delete        : Delete a canvas (owner-only)");
        println!("  - canvas.asset.put     : Store a content-addressed asset (base64)");
        println!("  - canvas.asset.get     : Read an asset back (base64)");
        println!("  - canvas.selection.set : Push the Panel's live selection");
        println!();
    }
}
