//! WebSocket Gateway for Moltbot-style architecture
//!
//! Provides a centralized control plane for all agent interactions.
//! The Gateway acts as a WebSocket server that clients connect to for
//! sending commands and receiving events using JSON-RPC 2.0 protocol.
//!
//! # Features
//!
//! - **JSON-RPC 2.0**: Standard request/response protocol
//! - **Event Broadcasting**: Push events to all connected clients
//! - **Bearer Token Auth**: Secure connection authentication
//! - **Device Pairing**: QR code / PIN-based pairing flow
//!
//! # Example
//!
//! ```rust,ignore
//! use alephcore::gateway::GatewayServer;
//! use std::net::SocketAddr;
//!
//! let addr: SocketAddr = "127.0.0.1:18790".parse().unwrap();
//! let server = GatewayServer::new(addr);
//! server.run().await?;
//! ```

// Social Connectivity: Bridge & Link data models (always compiled)
pub mod bridge;
pub mod link;
pub mod transport;

pub mod formatter;
pub mod protocol;
pub mod server;
pub mod event_bus;
pub mod event_emitter;
pub mod tool_display;
pub mod stream_buffer;
pub mod message_dedup;
pub mod router;
pub mod security;
pub mod handlers;
pub mod mdns_broadcaster;

// ControlPlane: Embedded web UI
pub mod control_plane;

// Phase 4: Multi-Agent & Dispatcher
pub mod agent_instance;
pub mod config;
pub mod session_manager;
pub mod execution_engine;
pub mod loop_callback_adapter;
pub mod provider_factory;
pub mod session_storage;
pub mod channel;
pub mod channel_registry;
pub mod interfaces;
pub mod device_store;
pub mod presence;
pub mod bind_mode;
pub mod hot_reload;
pub mod http_server;
pub mod inbound_context;
pub mod pairing_store;
pub mod reply_emitter;
pub mod routing_config;
pub mod inbound_router;
pub mod execution_adapter;
pub mod a2a_policy;
pub mod context;
pub mod lane;
pub mod webhook_receiver;
pub mod webhooks;
pub mod run_event_bus;
pub mod workspace;
pub mod state_version;
pub mod hello_snapshot;
pub mod event_scope;
pub mod rate_limiter;
pub mod challenge;
pub mod tailscale;
pub mod openai_api;
pub use server::GatewayServer;
pub use protocol::{JsonRpcRequest, JsonRpcResponse, JsonRpcError};
pub use event_bus::GatewayEventBus;
pub use event_emitter::{EventEmitter, StreamEvent, GatewayEventEmitter, NoOpEventEmitter, DynEventEmitter};
pub use tool_display::{ToolDisplay, get_tool_display, format_tool_meta, format_tool_summary, group_paths};
pub use stream_buffer::StreamBuffer;
pub use message_dedup::{normalize_text, is_text_duplicate, SentMessageTracker, SentRecord};
pub use router::AgentRouter;
pub use mdns_broadcaster::MdnsBroadcaster;

// Phase 4 exports
pub use agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry, AgentState};
pub use config::GatewayConfig;
pub use session_manager::{SessionManager, SessionManagerConfig};
pub use execution_engine::{ExecutionEngine, ExecutionEngineConfig, RunRequest, RunStatus, SimpleExecutionEngine};
pub use loop_callback_adapter::{EventEmittingCallback, ResponseChunkEmitter, UserQuestion};
pub use provider_factory::{
    create_provider_registry_from_env, create_claude_provider_from_env,
    create_openai_provider_from_env, can_create_provider_from_env,
    available_provider_from_env, ProviderFactoryError
};
pub use session_storage::{SessionStorage, LoadedSession, SessionMeta};
pub use channel::{
    Channel, ChannelFactory, ChannelConfig, ChannelInfo, ChannelCapabilities,
    ChannelId, ConversationId, UserId, MessageId,
    InboundMessage, OutboundMessage, SendResult, Attachment,
    ChannelStatus, ChannelError, ChannelResult,
};
pub use channel_registry::{ChannelRegistry, ChannelStatusSummary};
pub use device_store::{DeviceStore, ApprovedDevice};
pub use presence::{PresenceTracker, PresenceEntry};
pub use state_version::{StateVersionTracker, StateVersion};
pub use hello_snapshot::{HelloSnapshot, ConnectionLimits};
pub use handlers::auth::{AuthContext, handle_connect, handle_pairing_approve, handle_pairing_reject, handle_pairing_list, handle_devices_list, handle_devices_revoke, create_hello_notification};
pub use handlers::events::{SubscriptionManager, handle_subscribe, handle_unsubscribe, handle_list as handle_events_list};
pub use handlers::plugins::{init_extension_manager, is_extension_manager_initialized};
pub use event_bus::{TopicEvent, TopicFilter, topic_matches};
pub use bind_mode::BindMode;
pub use hot_reload::{ConfigWatcher, ConfigWatcherConfig, ConfigEvent, ConfigWatcherError, ReloadMode};
pub use inbound_context::{InboundContext, ReplyRoute};
pub use pairing_store::{PairingStore, PairingRequest, PairingError, SqlitePairingStore};
pub use reply_emitter::{ReplyEmitter, ReplyEmitterConfig};
pub use routing_config::{RoutingConfig, DmScope};
pub use inbound_router::{InboundMessageRouter, RoutingError, ChannelConfig as RouterChannelConfig, DmPolicy, GroupPolicy};
pub use execution_adapter::ExecutionAdapter;
pub use a2a_policy::AgentToAgentPolicy;
pub use context::GatewayContext;
pub use webhook_receiver::{WebhookHandler, WebhookReceiver};
pub use webhooks::{
    WebhooksConfig, WebhookEndpointConfig, SignatureFormat,
    WebhookHandlerState, WebhookProcessor, WebhookRequest, WebhookError,
    create_router as create_webhook_router,
};
pub use run_event_bus::{
    RunEvent, RunStatus as RunEventStatus, RunEndResult,
    WaitError, QueueError, ActiveRunHandle, wait_for_run_end,
};
pub use workspace::{
    Workspace, WorkspaceManager, WorkspaceManagerConfig, WorkspaceError,
    CacheState, UserActiveWorkspace, ActiveWorkspace,
};

// Property-based tests
#[cfg(test)]
mod proptest_protocol;
#[cfg(test)]
mod proptest_channel;

#[cfg(all(test, feature = "loom"))]
mod loom_concurrency;
