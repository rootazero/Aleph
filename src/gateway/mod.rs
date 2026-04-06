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

pub mod auth_middleware;
pub mod event_bus;
pub mod event_emitter;
pub mod events;
pub mod formatter;
pub mod handlers;
pub mod mdns_broadcaster;
pub mod message_dedup;
pub mod middleware;
pub mod protocol;
pub mod router;
pub mod security;
pub mod server;
pub mod session;
pub mod stream_buffer;
pub mod tool_display;

// ControlPlane: Embedded web UI
pub mod control_plane;

// Phase 4: Multi-Agent & Dispatcher
pub mod agent_instance;
pub mod agent_lifecycle;
pub mod config;
pub mod execution_engine;
pub mod session_manager;
// loop_callback_adapter removed (depended on old OTAF agent_loop types)
pub mod bind_mode;
pub mod channel;
pub mod channel_approval;
pub mod channel_chunking;
pub mod channel_policy;
pub mod channel_registry;
pub mod coalescer;
pub mod device_store;
pub mod hot_reload;
pub mod http_server;
pub mod inbound_context;
pub mod inbound_router;
pub mod interfaces;
pub mod pairing_store;
pub mod pipeline;
pub mod presence;
pub mod provider_factory;
pub mod reply_emitter;
pub mod routing_config;

pub mod agent_env;
pub mod challenge;
pub mod context;
pub mod event_scope;
pub mod execution_adapter;
pub mod hello_snapshot;
pub mod i18n;
pub mod idempotency;
pub mod inter_agent_policy;
pub mod lane;
pub mod media;
pub mod openai_api;
pub mod rate_limiter;
pub mod run_event_bus;
pub mod session_scheduler;
pub mod state_version;
pub mod streaming;
pub mod streaming_sink;
pub mod tailscale;
pub mod voice;
pub mod webhook_receiver;
pub mod webhooks;
pub mod workspace_loader;
pub use event_bus::GatewayEventBus;
pub use event_emitter::{
    DynEventEmitter, EventEmitter, GatewayEventEmitter, NoOpEventEmitter, OutputMode, StreamEvent,
};
pub use mdns_broadcaster::MdnsBroadcaster;
pub use message_dedup::{is_text_duplicate, normalize_text, SentMessageTracker, SentRecord};
pub use protocol::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};
pub use router::AgentRouter;
pub use server::GatewayServer;
pub use stream_buffer::StreamBuffer;
pub use tool_display::{
    format_tool_meta, format_tool_summary, get_tool_display, group_paths, ToolDisplay,
};

// Phase 4 exports
pub use agent_instance::{AgentInstance, AgentInstanceConfig, AgentRegistry, AgentState};
pub use config::GatewayConfig;
pub use execution_engine::{
    ExecutionEngine, ExecutionEngineConfig, RunRequest, RunStatus, SimpleExecutionEngine,
};
pub use session_manager::{SessionManager, SessionManagerConfig};
// EventEmittingCallback, ResponseChunkEmitter, UserQuestion removed (old OTAF types)
pub use bind_mode::BindMode;
pub use channel::{
    Attachment, Channel, ChannelCapabilities, ChannelConfig, ChannelError, ChannelFactory,
    ChannelHealth, ChannelId, ChannelInfo, ChannelResult, ChannelStatus, ConversationId,
    HealthStatus, InboundMessage, MessageId, OutboundMessage, SendResult, UserId,
};
pub use channel_registry::{ChannelHealthSummary, ChannelRegistry, ChannelStatusSummary};
pub use device_store::{ApprovedDevice, DeviceStore};
pub use event_bus::{topic_matches, TopicEvent, TopicFilter};
pub use events::GatewayEventFrame;
pub use execution_adapter::ExecutionAdapter;
pub use handlers::auth::{
    create_hello_notification, handle_connect, handle_devices_list, handle_devices_revoke,
    handle_pairing_approve, handle_pairing_list, handle_pairing_reject, AuthContext,
};
pub use handlers::events::{
    handle_list as handle_events_list, handle_subscribe, handle_unsubscribe, SubscriptionManager,
};
pub use handlers::plugins::{init_extension_manager, is_extension_manager_initialized};
pub use hello_snapshot::{ConnectionLimits, HelloSnapshot};
pub use hot_reload::{
    ConfigEvent, ConfigWatcher, ConfigWatcherConfig, ConfigWatcherError, ReloadMode,
};
pub use inbound_context::{InboundContext, ReplyRoute};
pub use inbound_router::{
    ChannelConfig as RouterChannelConfig, DmPolicy, GroupPolicy, InboundMessageRouter, RoutingError,
};
pub use pairing_store::{PairingError, PairingRequest, PairingStore, SqlitePairingStore};
pub use presence::{PresenceEntry, PresenceTracker};
pub use provider_factory::{
    available_provider_from_env, can_create_provider_from_env, create_claude_provider_from_env,
    create_openai_provider_from_env, create_provider_registry_from_env, ProviderFactoryError,
};
pub use reply_emitter::{ReplyEmitter, ReplyEmitterConfig};
pub use routing_config::{DmScope, RoutingConfig};
pub use state_version::{StateVersion, StateVersionTracker};

pub use agent_env::{
    ActiveAgentEnv, AgentEnv, AgentEnvContext, AgentEnvError, AgentEnvFilter, AgentEnvStore,
    AgentEnvStoreConfig, CacheState, DEFAULT_AGENT,
};
pub use context::GatewayContext;
pub use inter_agent_policy::AgentToAgentPolicy;
pub use run_event_bus::{
    wait_for_run_end, ActiveRunHandle, QueueError, RunEndResult, RunEvent,
    RunStatus as RunEventStatus, WaitError,
};
pub use webhook_receiver::{WebhookHandler, WebhookReceiver};
pub use webhooks::{
    create_router as create_webhook_router, SignatureFormat, WebhookEndpointConfig, WebhookError,
    WebhookHandlerState, WebhookProcessor, WebhookRequest, WebhooksConfig,
};

// Property-based tests
#[cfg(test)]
mod proptest_channel;
#[cfg(test)]
mod proptest_protocol;

#[cfg(test)]
mod auth_probe_tests;

#[cfg(all(test, feature = "loom"))]
mod loom_concurrency;
