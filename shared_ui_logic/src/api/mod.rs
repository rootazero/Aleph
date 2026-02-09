//! # API 层（API Layer）
//!
//! 高级 RPC 方法封装，提供类型安全的 Gateway API 访问。
//!
//! ## 模块
//!
//! - [`memory`]: Memory API - 记忆系统管理
//! - [`config`]: Config API - 配置管理
//! - [`plugins`]: Plugins API - 插件管理
//! - [`providers`]: Providers API - AI 提供商管理
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
//!
//! ### Plugins API
//!
//! ```rust,ignore
//! use aleph_ui_logic::api::PluginsApi;
//! use aleph_ui_logic::connection::create_connector;
//!
//! let connector = create_connector();
//! let plugins = PluginsApi::new(connector);
//!
//! // List all plugins
//! let list = plugins.list().await?;
//! for plugin in list {
//!     println!("{}: {} ({})", plugin.name, plugin.version, plugin.enabled);
//! }
//! ```
//!
//! ### Providers API
//!
//! ```rust,ignore
//! use aleph_ui_logic::api::ProvidersApi;
//! use aleph_ui_logic::connection::create_connector;
//!
//! let connector = create_connector();
//! let providers = ProvidersApi::new(connector);
//!
//! // List all providers
//! let list = providers.list().await?;
//! for provider in list {
//!     println!("{}: {} (default: {})", provider.name, provider.model, provider.is_default);
//! }
//! ```

pub mod config;
pub mod memory;
pub mod plugins;
pub mod providers;

// Re-export commonly used types
pub use config::{
    BehaviorConfig, CodeExecConfig, ConfigApi, FileOpsConfig, PoliciesConfig, SearchConfig,
    ShortcutsConfig,
};
pub use memory::{MemoryApi, MemorySearchItem, MemoryStats};
pub use plugins::{PluginInfo, PluginsApi};
pub use providers::{ProviderConfig, ProviderInfo, ProvidersApi, TestResult};
