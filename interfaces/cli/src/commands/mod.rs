//! Command handlers.
//!
//! Each submodule owns one top-level CLI subcommand (e.g. `cron_cmd` →
//! `aleph cron`). `cli_args` collects every Clap `Subcommand` enum so
//! `main.rs` stays focused on parsing and dispatch.

pub mod ask;
pub mod audit_cmd;
pub mod calls;
pub mod channels_cmd;
pub mod chat;
pub mod chat_cmd;
pub mod cli_args;
pub mod completion;
pub mod config_cmd;
pub mod connect;
pub mod cron_cmd;
pub mod daemon;
pub mod doctor;
pub mod gateway_cmd;
pub mod health;
pub mod heartbeat_cmd;
pub mod hooks_cmd;
pub mod identity_cmd;
pub mod info;
pub mod logs_cmd;
pub mod mcp_cmd;
pub mod memory_cmd;
pub mod open_cmd;
pub mod plugin_cmd;
pub mod plugins_cmd;
pub mod providers_cmd;
pub mod projects_cmd;
pub mod proxy_cmd;
pub mod run_follow;
pub mod sandbox_cmd;
pub mod secret_cmd;
pub mod services_cmd;
pub mod session;
pub mod skills_cmd;
pub mod spend_cmd;
pub mod tools;
pub mod trace_cmd;
pub mod users_cmd;
pub mod watch;
pub mod webhook_cmd;
pub mod workspace_cmd;
