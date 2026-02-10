//! AlephConnector trait 定义

use serde_json::Value;
use std::pin::Pin;
use futures::Stream;

/// 统一的 WebSocket 连接抽象
///
/// 这个 trait 提供了跨平台的 WebSocket 连接接口，
/// 支持 WASM（浏览器）和原生（Tokio）环境。
///
/// ## 注意
///
/// - 使用 `?Send` 是因为 WASM 环境不支持 `Send`
/// - 所有方法都是异步的，使用 `async_trait` 宏
pub trait AlephConnector {
    /// 连接到 Gateway
    ///
    /// # 参数
    ///
    /// - `url`: WebSocket URL（例如：`ws://127.0.0.1:18789`）
    ///
    /// # 错误
    ///
    /// 如果连接失败，返回 [`ConnectionError::ConnectionFailed`]
    fn connect(
        &mut self,
        url: &str,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + '_>>;

    /// 断开连接
    ///
    /// # 错误
    ///
    /// 如果断开失败，返回 [`ConnectionError`]
    fn disconnect(
        &mut self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + '_>>;

    /// 发送消息
    ///
    /// # 参数
    ///
    /// - `message`: JSON 消息
    ///
    /// # 错误
    ///
    /// 如果发送失败，返回 [`ConnectionError::SendFailed`]
    fn send(
        &mut self,
        message: Value,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), ConnectionError>> + '_>>;

    /// 接收消息流
    ///
    /// 返回一个 Stream，持续产生接收到的消息。
    ///
    /// # 注意
    ///
    /// 这个方法返回的 Stream 应该在连接断开时自动结束。
    fn receive(&mut self) -> Pin<Box<dyn Stream<Item = Result<Value, ConnectionError>> + '_>>;

    /// 检查连接状态
    ///
    /// # 返回
    ///
    /// - `true`: 已连接
    /// - `false`: 未连接
    fn is_connected(&self) -> bool;
}

/// 连接错误类型
#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    /// 连接失败
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    /// 发送失败
    #[error("Send failed: {0}")]
    SendFailed(String),

    /// 接收失败
    #[error("Receive failed: {0}")]
    ReceiveFailed(String),

    /// 未连接
    #[error("Not connected")]
    NotConnected,

    /// 序列化错误
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// 其他错误
    #[error("Other error: {0}")]
    Other(String),
}
