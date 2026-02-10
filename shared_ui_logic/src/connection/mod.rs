//! # 连接层（Connection Layer）
//!
//! 提供统一的 WebSocket 连接抽象，支持 WASM 和原生环境。
//!
//! ## 核心组件
//!
//! - [`AlephConnector`]: 统一的连接器 trait
//! - [`DefaultConnector`]: 自动选择的平台默认实现
//! - [`ReconnectStrategy`]: 重连策略（指数退避）
//!
//! ## 平台支持
//!
//! - **WASM**: 使用 `web_sys::WebSocket`
//! - **原生**: 使用 `tokio_tungstenite`
//!
//! ## 使用示例
//!
//! ```rust,ignore
//! use aleph_ui_logic::connection::{create_connector, AlephConnector};
//!
//! async fn connect() {
//!     let mut connector = create_connector();
//!     connector.connect("ws://127.0.0.1:18789").await.unwrap();
//!
//!     // 发送消息
//!     connector.send(json!({"type": "req"})).await.unwrap();
//! }
//! ```

pub mod connector;
pub mod reconnect;

// 平台特定实现
#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
mod wasm;
#[cfg(all(target_arch = "wasm32", feature = "wasm"))]
pub use wasm::WasmConnector as DefaultConnector;

#[cfg(all(not(target_arch = "wasm32"), feature = "native"))]
mod native;
#[cfg(all(not(target_arch = "wasm32"), feature = "native"))]
pub use native::NativeConnector as DefaultConnector;

// 重新导出
pub use connector::{AlephConnector, ConnectionError};
pub use reconnect::ReconnectStrategy;

/// 创建默认连接器（自动选择平台）
#[cfg(any(
    all(target_arch = "wasm32", feature = "wasm"),
    all(not(target_arch = "wasm32"), feature = "native")
))]
pub fn create_connector() -> DefaultConnector {
    DefaultConnector::new()
}
