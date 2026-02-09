//! # Aleph UI Logic
//!
//! Aleph 端侧 SDK - 统一的 Client 与 Server 交互逻辑
//!
//! ## 核心价值
//!
//! - **类型安全**：所有 RPC 调用在编译期验证，消除类型漂移
//! - **跨平台**：同时支持 WASM（浏览器）和原生（Tauri/CLI）
//! - **响应式**：基于 Leptos Signals，UI 自动更新
//! - **可观测**：内置 Agent 行为追踪和指标收集
//!
//! ## Feature Flags
//!
//! - `core`: 基础协议层（无 UI 依赖）
//! - `leptos`: UI 状态层（依赖 Leptos）
//! - `observability`: 可观测性增强（Command Center 专用）
//! - `wasm`: WASM 支持
//! - `native`: 原生支持
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use aleph_ui_logic::connection::create_connector;
//! use aleph_ui_logic::protocol::RpcClient;
//!
//! #[tokio::main]
//! async fn main() {
//!     // 创建连接器（自动选择平台）
//!     let connector = create_connector();
//!
//!     // 创建 RPC 客户端
//!     let client = RpcClient::new(connector);
//!
//!     // 类型安全的 RPC 调用
//!     let stats: MemoryStats = client
//!         .call("memory.stats", ())
//!         .await
//!         .unwrap();
//! }
//! ```

#![warn(missing_docs)]
#![warn(clippy::all)]

// 连接层（core feature）
#[cfg(feature = "core")]
pub mod connection;

// 协议层（core feature）
#[cfg(feature = "core")]
pub mod protocol;

// 状态层（leptos feature）
#[cfg(feature = "leptos")]
pub mod state;

// API层（leptos feature）
#[cfg(feature = "leptos")]
pub mod api;

// 可观测性层（observability feature）
#[cfg(feature = "observability")]
pub mod observability;

// 重新导出常用类型
#[cfg(feature = "core")]
pub use connection::{AlephConnector, ConnectionError};

#[cfg(feature = "core")]
pub use protocol::{RpcClient, RpcError};
