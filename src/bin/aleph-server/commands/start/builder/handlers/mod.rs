//! Handler registration helpers for the gateway server.
//!
//! All `register_*` and `start_*` / `setup_*` functions are collected here so
//! that `start.rs` only contains subsystem initializers and the top-level
//! `start_server()` orchestrator.

use std::path::PathBuf;
use std::sync::Arc;

use alephcore::gateway::handlers::auth as auth_handlers;
use alephcore::gateway::handlers::channel as channel_handlers;
use alephcore::gateway::handlers::config as config_handlers;
use alephcore::gateway::handlers::discord_panel as discord_panel_handlers;
use alephcore::gateway::handlers::group_chat as group_chat_handlers;
use alephcore::gateway::handlers::group_chat::SharedOrchestrator;
use alephcore::gateway::handlers::identity as identity_handlers;
use alephcore::gateway::handlers::identity::SharedIdentityCtx;
use alephcore::gateway::handlers::memory as memory_handlers;
use alephcore::gateway::handlers::oauth as oauth_handlers;
use alephcore::gateway::handlers::session as session_handlers;
use alephcore::gateway::handlers::workspace as workspace_handlers;
use alephcore::gateway::GatewayServer;
use alephcore::gateway::{
    AgentEnvStore, ChannelRegistry, ConfigEvent, ConfigWatcher, ConfigWatcherConfig,
};
use alephcore::group_chat::GroupChatExecutor;
use alephcore::memory::store::MemoryBackend;

use crate::cli::Args;
use crate::server_init::serve_webchat;

/// Register a JSON-RPC handler with shared context via Arc.
///
/// Eliminates the repeated clone-into-closure boilerplate.
/// Supports 0, 1, or 2 context arguments.
macro_rules! register_handler {
    // No context args (stateless handler)
    ($server:expr, $method:expr, $handler:path) => {{
        $server
            .handlers_mut()
            .register($method, |req| async move { $handler(req).await });
    }};
    // 1 context arg
    ($server:expr, $method:expr, $handler:path, $ctx1:expr) => {{
        let ctx1 = ::std::sync::Arc::clone(&$ctx1);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = ::std::sync::Arc::clone(&ctx1);
            async move { $handler(req, ctx1).await }
        });
    }};
    // 2 context args
    ($server:expr, $method:expr, $handler:path, $ctx1:expr, $ctx2:expr) => {{
        let ctx1 = ::std::sync::Arc::clone(&$ctx1);
        let ctx2 = ::std::sync::Arc::clone(&$ctx2);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = ::std::sync::Arc::clone(&ctx1);
            let ctx2 = ::std::sync::Arc::clone(&ctx2);
            async move { $handler(req, ctx1, ctx2).await }
        });
    }};
    // 3 context args
    ($server:expr, $method:expr, $handler:path, $ctx1:expr, $ctx2:expr, $ctx3:expr) => {{
        let ctx1 = ::std::sync::Arc::clone(&$ctx1);
        let ctx2 = ::std::sync::Arc::clone(&$ctx2);
        let ctx3 = ::std::sync::Arc::clone(&$ctx3);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = ::std::sync::Arc::clone(&ctx1);
            let ctx2 = ::std::sync::Arc::clone(&ctx2);
            let ctx3 = ::std::sync::Arc::clone(&ctx3);
            async move { $handler(req, ctx1, ctx2, ctx3).await }
        });
    }};
    // 4 context args
    ($server:expr, $method:expr, $handler:path, $ctx1:expr, $ctx2:expr, $ctx3:expr, $ctx4:expr) => {{
        let ctx1 = ::std::sync::Arc::clone(&$ctx1);
        let ctx2 = ::std::sync::Arc::clone(&$ctx2);
        let ctx3 = ::std::sync::Arc::clone(&$ctx3);
        let ctx4 = ::std::sync::Arc::clone(&$ctx4);
        $server.handlers_mut().register($method, move |req| {
            let ctx1 = ::std::sync::Arc::clone(&ctx1);
            let ctx2 = ::std::sync::Arc::clone(&ctx2);
            let ctx3 = ::std::sync::Arc::clone(&ctx3);
            let ctx4 = ::std::sync::Arc::clone(&ctx4);
            async move { $handler(req, ctx1, ctx2, ctx3, ctx4).await }
        });
    }};
}

mod agents;
mod canvas;
mod config;
mod core;
mod extensions;
mod mcp;
mod memory;
mod session;
mod settings;
mod system;

pub(in crate::commands::start) use agents::*;
pub(in crate::commands::start) use canvas::*;
pub(in crate::commands::start) use config::*;
pub(in crate::commands::start) use core::*;
pub(in crate::commands::start) use extensions::*;
pub(in crate::commands::start) use mcp::*;
pub(in crate::commands::start) use memory::*;
pub(in crate::commands::start) use session::*;
pub(in crate::commands::start) use settings::*;
pub(in crate::commands::start) use system::*;
