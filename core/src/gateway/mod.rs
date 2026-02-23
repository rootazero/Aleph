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
//! let addr: SocketAddr = "127.0.0.1:18789".parse().unwrap();
//! let server = GatewayServer::new(addr);
//! server.run().await?;
//! ```

// Social Connectivity: Bridge & Link data models (always compiled)
pub mod bridge;
pub mod link;
pub mod transport;

#[cfg(feature = "gateway")]
pub mod protocol;
#[cfg(feature = "gateway")]
pub mod server;
#[cfg(feature = "gateway")]
pub mod event_bus;
#[cfg(feature = "gateway")]
pub mod event_emitter;
#[cfg(feature = "gateway")]
pub mod tool_display;
#[cfg(feature = "gateway")]
pub mod stream_buffer;
#[cfg(feature = "gateway")]
pub mod message_dedup;
#[cfg(feature = "gateway")]
pub mod router;
#[cfg(feature = "gateway")]
pub mod security;
#[cfg(feature = "gateway")]
pub mod handlers;
#[cfg(feature = "gateway")]
pub mod mdns_broadcaster;

// ControlPlane: Embedded web UI
#[cfg(feature = "control-plane")]
pub mod control_plane;

// Phase 4: Multi-Agent & Dispatcher
#[cfg(feature = "gateway")]
pub mod agent_instance;
#[cfg(feature = "gateway")]
pub mod config;
#[cfg(feature = "gateway")]
pub mod session_manager;
#[cfg(feature = "gateway")]
pub mod execution_engine;
#[cfg(feature = "gateway")]
pub mod loop_callback_adapter;
#[cfg(feature = "gateway")]
pub mod provider_factory;
#[cfg(feature = "gateway")]
pub mod session_storage;
#[cfg(feature = "gateway")]
pub mod channel;
#[cfg(feature = "gateway")]
pub mod channel_registry;
#[cfg(feature = "gateway")]
pub mod interfaces;
#[cfg(feature = "gateway")]
pub mod device_store;
#[cfg(feature = "gateway")]
pub mod hot_reload;
#[cfg(feature = "gateway")]
pub mod http_server;
#[cfg(feature = "gateway")]
pub mod inbound_context;
#[cfg(feature = "gateway")]
pub mod pairing_store;
#[cfg(feature = "gateway")]
pub mod reply_emitter;
#[cfg(feature = "gateway")]
pub mod routing_config;
#[cfg(feature = "gateway")]
pub mod inbound_router;
#[cfg(feature = "gateway")]
pub mod execution_adapter;
#[cfg(feature = "gateway")]
pub mod a2a_policy;
#[cfg(feature = "gateway")]
pub mod context;
#[cfg(feature = "gateway")]
pub mod webhooks;
#[cfg(feature = "gateway")]
pub mod run_event_bus;
#[cfg(feature = "gateway")]
pub mod workspace;
#[cfg(feature = "gateway")]
pub use server::GatewayServer;
#[cfg(feature = "gateway")]
pub use protocol::{JsonRpcRequest, JsonRpcResponse, JsonRpcError};
#[cfg(feature = "gateway")]
pub use event_bus::GatewayEventBus;
#[cfg(feature = "gateway")]
pub use event_emitter::{EventEmitter, StreamEvent, GatewayEventEmitter, NoOpEventEmitter, DynEventEmitter};
#[cfg(feature = "gateway")]
pub use tool_display::{ToolDisplay, get_tool_display, format_tool_meta, format_tool_summary, group_paths};
#[cfg(feature = "gateway")]
pub use stream_buffer::StreamBuffer;
#[cfg(feature = "gateway")]
pub use message_dedup::{normalize_text, is_text_duplicate, SentMessageTracker, SentRecord};
#[cfg(feature = "gateway")]
pub use router::AgentRouter;
#[cfg(feature = "gateway")]
pub use mdns_broadcaster::MdnsBroadcaster;

// Phase 4 exports
#[cfg(feature = "gateway")]
pub use agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry, AgentState};
#[cfg(feature = "gateway")]
pub use config::GatewayConfig;
#[cfg(feature = "gateway")]
pub use session_manager::{SessionManager, SessionManagerConfig};
#[cfg(feature = "gateway")]
pub use execution_engine::{ExecutionEngine, ExecutionEngineConfig, RunRequest, RunStatus, SimpleExecutionEngine};
#[cfg(feature = "gateway")]
pub use loop_callback_adapter::{EventEmittingCallback, ResponseChunkEmitter, UserQuestion};
#[cfg(feature = "gateway")]
pub use provider_factory::{
    create_provider_registry_from_env, create_claude_provider_from_env,
    create_openai_provider_from_env, can_create_provider_from_env,
    available_provider_from_env, ProviderFactoryError
};
#[cfg(feature = "gateway")]
pub use session_storage::{SessionStorage, LoadedSession, SessionMeta};
#[cfg(feature = "gateway")]
pub use channel::{
    Channel, ChannelFactory, ChannelConfig, ChannelInfo, ChannelCapabilities,
    ChannelId, ConversationId, UserId, MessageId,
    InboundMessage, OutboundMessage, SendResult, Attachment,
    ChannelStatus, ChannelError, ChannelResult,
};
#[cfg(feature = "gateway")]
pub use channel_registry::{ChannelRegistry, ChannelStatusSummary};
#[cfg(feature = "gateway")]
pub use device_store::{DeviceStore, ApprovedDevice};
#[cfg(feature = "gateway")]
pub use handlers::auth::{AuthContext, handle_connect, handle_pairing_approve, handle_pairing_reject, handle_pairing_list, handle_devices_list, handle_devices_revoke, create_hello_notification};
#[cfg(feature = "gateway")]
pub use handlers::events::{SubscriptionManager, handle_subscribe, handle_unsubscribe, handle_list as handle_events_list};
#[cfg(feature = "gateway")]
pub use handlers::plugins::{init_extension_manager, is_extension_manager_initialized};
#[cfg(feature = "gateway")]
pub use event_bus::{TopicEvent, TopicFilter, topic_matches};
#[cfg(feature = "gateway")]
pub use hot_reload::{ConfigWatcher, ConfigWatcherConfig, ConfigEvent, ConfigWatcherError};
#[cfg(feature = "gateway")]
pub use inbound_context::{InboundContext, ReplyRoute};
#[cfg(feature = "gateway")]
pub use pairing_store::{PairingStore, PairingRequest, PairingError, SqlitePairingStore};
#[cfg(feature = "gateway")]
pub use reply_emitter::{ReplyEmitter, ReplyEmitterConfig};
#[cfg(feature = "gateway")]
pub use routing_config::{RoutingConfig, DmScope};
#[cfg(feature = "gateway")]
pub use inbound_router::{InboundMessageRouter, RoutingError, ChannelConfig as RouterChannelConfig, DmPolicy, GroupPolicy};
#[cfg(feature = "gateway")]
pub use execution_adapter::ExecutionAdapter;
#[cfg(feature = "gateway")]
pub use a2a_policy::AgentToAgentPolicy;
#[cfg(feature = "gateway")]
pub use context::GatewayContext;
#[cfg(feature = "gateway")]
pub use webhooks::{
    WebhooksConfig, WebhookEndpointConfig, SignatureFormat,
    WebhookHandlerState, WebhookProcessor, WebhookRequest, WebhookError,
    create_router as create_webhook_router,
};
#[cfg(feature = "gateway")]
pub use run_event_bus::{
    RunEvent, RunStatus as RunEventStatus, RunEndResult,
    WaitError, QueueError, ActiveRunHandle, wait_for_run_end,
};
#[cfg(feature = "gateway")]
pub use workspace::{
    Workspace, WorkspaceManager, WorkspaceManagerConfig, WorkspaceError,
    CacheState, UserActiveWorkspace,
};
