//! # 协议层（Protocol Layer）
//!
//! 提供类型安全的 RPC 客户端和流式数据处理。
//!
//! ## 核心组件
//!
//! - [`RpcClient`]: JSON-RPC 2.0 客户端
//! - [`StreamHandler`]: 流式事件处理器
//! - [`EventDispatcher`]: 事件分发系统

// TODO: 实现 RPC 客户端
// TODO: 实现流式数据处理
// TODO: 实现事件分发系统

/// RPC 错误类型（占位符）
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    /// 连接错误
    #[error("Connection error")]
    Connection,
}

/// RPC 客户端（占位符）
pub struct RpcClient;
