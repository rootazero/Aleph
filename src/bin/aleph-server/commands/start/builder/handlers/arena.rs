use super::*;

pub(in crate::commands::start) fn register_arena_handlers(
    server: &mut GatewayServer,
    manager: &Arc<alephcore::sync_primitives::RwLock<alephcore::arena::ArenaManager>>,
) {
    use alephcore::gateway::handlers::arena;

    register_handler!(server, "arena.create", arena::handle_create, manager);
    register_handler!(server, "arena.query", arena::handle_query, manager);
    register_handler!(server, "arena.settle", arena::handle_settle, manager);
}
