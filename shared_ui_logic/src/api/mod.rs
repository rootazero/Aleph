//! # API 层（API Layer）
//!
//! 高级 RPC 方法封装，提供类型安全的 Gateway API 访问。
//!
//! ## 模块
//!
//! - [`memory`]: Memory API - 记忆系统管理
//! - [`config`]: Config API - 配置管理
//!
//! ## 使用示例
//!
//! ### Memory API
//!
//! ```rust,ignore
//! use aleph_ui_logic::api::MemoryApi;
//! use aleph_ui_logic::connection::create_connector;
//!
//! let connector = create_connector();
//! let memory = MemoryApi::new(connector);
//!
//! // Get statistics
//! let stats = memory.stats().await?;
//! println!("Total facts: {}", stats.count);
//!
//! // Search
//! let results = memory.search("rust", Some(10)).await?;
//! ```
//!
//! ### Config API
//!
//! ```rust,ignore
//! use aleph_ui_logic::api::ConfigApi;
//! use aleph_ui_logic::connection::create_connector;
//!
//! let connector = create_connector();
//! let config = ConfigApi::new(connector);
//!
//! // Get and update policies
//! let mut policies = config.policies_get().await?;
//! policies.allow_web_browsing = true;
//! config.policies_update(policies).await?;
//! ```

pub mod config;
pub mod memory;

// Re-export commonly used types
pub use config::{
    BehaviorConfig, CodeExecConfig, ConfigApi, FileOpsConfig, PoliciesConfig, SearchConfig,
    ShortcutsConfig,
};
pub use memory::{MemoryApi, MemorySearchItem, MemoryStats};
