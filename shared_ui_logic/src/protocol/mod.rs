//! # 协议层（Protocol Layer）
//!
//! 提供类型安全的 RPC 客户端和流式数据处理。
//!
//! ## 核心组件
//!
//! - [`RpcClient`]: JSON-RPC 2.0 客户端
//! - [`StreamHandler`]: 流式事件处理器
//! - [`EventDispatcher`]: 事件分发系统
//!
//! ## 使用示例
//!
//! ### RPC 客户端
//!
//! ```rust,ignore
//! use aleph_ui_logic::connection::create_connector;
//! use aleph_ui_logic::protocol::RpcClient;
//!
//! let mut connector = create_connector();
//! connector.connect("ws://127.0.0.1:18789").await?;
//!
//! let client = RpcClient::new(connector);
//! let result: MemoryStats = client.call("memory.stats", ()).await?;
//! ```
//!
//! ### 流式事件处理
//!
//! ```rust,ignore
//! use aleph_ui_logic::protocol::StreamHandler;
//!
//! let (handler, tx) = StreamHandler::new();
//! let agent_events = handler.filter_by_type("agent.thinking").into_stream();
//!
//! while let Some(event) = agent_events.next().await {
//!     println!("Agent: {:?}", event);
//! }
//! ```
//!
//! ### 事件分发
//!
//! ```rust,ignore
//! use aleph_ui_logic::protocol::EventDispatcher;
//!
//! let dispatcher = EventDispatcher::new();
//!
//! dispatcher.subscribe("agent.thinking", |payload| {
//!     println!("Thinking: {:?}", payload);
//! }).await;
//!
//! dispatcher.dispatch("agent.thinking", json!({"content": "..."})).await;
//! ```

pub mod events;
pub mod rpc;
pub mod streaming;

// Re-export commonly used types
pub use events::EventDispatcher;
pub use rpc::{RpcClient, RpcError};
pub use streaming::{StreamBuffer, StreamEvent, StreamHandler};
