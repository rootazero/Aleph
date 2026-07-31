//! Aleph CLI — reference implementation of the Aleph protocol client.
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                        aleph-cli                             │
//! │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────────┐ │
//! │  │  CLI    │  │ Client  │  │Commands │  │   aleph-tui     │ │
//! │  │ (clap)  │→ │(WS+RPC) │→ │ Handler │→ │   (library)     │ │
//! │  └─────────┘  └─────────┘  └─────────┘  └─────────────────┘ │
//! └───────────────────────────────┬─────────────────────────────┘
//!                                 │ WebSocket (JSON-RPC 2.0)
//!                                 ↓
//!                         Aleph Gateway Server
//! ```
//!
//! The CLI is intentionally a thin I/O layer (R4): every subcommand is a
//! straight argv → JSON-RPC translation followed by response rendering. No
//! business logic lives here; that all sits behind the gateway.

mod commands;
pub(crate) mod output;

use clap::Parser;
use tracing::info;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use aleph_client::{CliConfig, CliResult};

use commands::cli_args::{
    CallsAction, ChannelsAction, ChatControlAction, Commands, ConfigAction, CronAction,
    DaemonAction, GatewayAction, HeartbeatAction, HooksAction, IdentityAction, LogsAction,
    MarketplaceAction, McpAction, MemoryAction, PluginAction, ProvidersAction, ProxyAction,
    SandboxAction, SecretAction, ServicesAction, SessionAction, SkillsAction, ToolsAction,
    TraceAction, WebhookAction, WorkspaceAction,
};

/// Aleph CLI - Personal AI Assistant Client
#[derive(Parser)]
#[command(name = "aleph")]
#[command(author, version = env!("ALEPH_VERSION"), about, long_about = None)]
pub(crate) struct Cli {
    /// Gateway server URL
    #[arg(short, long, default_value = aleph_client::DEFAULT_GATEWAY_URL)]
    server: String,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<String>,

    /// Output in JSON format (applies to all subcommands)
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[tokio::main]
async fn main() -> CliResult<()> {
    let cli = Cli::parse();

    let default_filter = if cli.verbose { "debug" } else { "info" };
    if let Err(e) = aleph_logging::init_component_logging("cli", 7, default_filter) {
        eprintln!("Failed to init file logging: {e}");
        tracing_subscriber::registry()
            .with(fmt::layer())
            .with(if cli.verbose {
                EnvFilter::new("debug")
            } else {
                EnvFilter::new("info")
            })
            .init();
    }

    let config = CliConfig::load(cli.config.as_deref())?;
    let server_url = cli.server.clone();
    let json = cli.json;

    info!("Aleph CLI v{}", env!("ALEPH_VERSION"));

    match cli.command {
        Some(cmd) => dispatch(cmd, &server_url, &config, json, cli.verbose).await?,
        None => commands::chat::run(&server_url, None, None, &config, cli.verbose).await?,
    }

    Ok(())
}

/// Dispatch the parsed top-level subcommand to its handler module.
///
/// Each arm is intentionally short: convert clap variants into the
/// JSON-RPC-shaped argv each module expects and forward. No branching policy
/// lives here.
async fn dispatch(
    cmd: Commands,
    server_url: &str,
    config: &CliConfig,
    json: bool,
    verbose: bool,
) -> CliResult<()> {
    match cmd {
        Commands::Health => commands::health::run(server_url, config, json).await,
        Commands::Info => commands::info::run(server_url, config, json).await,
        Commands::Tools { category, action } => {
            dispatch_tools(server_url, config, category, action, json).await
        }
        Commands::Calls { action } => dispatch_calls(server_url, config, action, json).await,
        Commands::Connect { device } => {
            commands::connect::run(server_url, &device, config, json).await
        }
        Commands::Ask {
            message,
            session,
            last,
            output_last_message,
        } => {
            // Headless parity with `echo x | aleph ask`: when stdin is piped,
            // fold it into the effective prompt (appended as context when an
            // explicit message is also given). Pure I/O — the merged string is
            // forwarded to the same `agent.run` path (R4).
            let piped = commands::ask::read_piped_stdin();
            let effective = commands::ask::merge_prompt(message.as_deref(), piped.as_deref())
                .ok_or_else(|| {
                    aleph_client::CliError::Other(
                        "no message provided — pass a message argument or pipe text on stdin"
                            .to_string(),
                    )
                })?;
            commands::ask::run(
                server_url,
                &effective,
                session.as_deref(),
                last,
                json,
                output_last_message.as_deref(),
                config,
            )
            .await
        }
        Commands::Chat { session, agent } => {
            commands::chat::run(
                server_url,
                agent.as_deref(),
                session.as_deref(),
                config,
                verbose,
            )
            .await
        }
        Commands::Session { action } => dispatch_session(server_url, action, config, json).await,
        Commands::Config { action } => dispatch_config(server_url, action, config, json).await,
        Commands::Cron { action } => dispatch_cron(server_url, action, config, json).await,
        Commands::Heartbeat { action } => {
            dispatch_heartbeat(server_url, config, action, json).await
        }
        Commands::Channels { action } => dispatch_channels(server_url, action, config, json).await,
        Commands::Daemon { action } => dispatch_daemon(action, json),
        Commands::Providers { action } => {
            dispatch_providers(server_url, config, action, json).await
        }
        Commands::Memory { action } => dispatch_memory(server_url, config, action, json).await,
        Commands::Plugin { action } => dispatch_plugin(server_url, config, action, json).await,
        Commands::Services { action } => dispatch_services(server_url, config, action, json).await,
        Commands::Skills { action } => dispatch_skills(server_url, config, action, json).await,
        Commands::Workspace { action } => {
            dispatch_workspace(server_url, config, action, json).await
        }
        Commands::Gateway { action } => dispatch_gateway(server_url, config, action, json).await,
        Commands::Logs { action } => dispatch_logs(server_url, config, action, json).await,
        Commands::Trace { action } => dispatch_trace(server_url, config, action, json).await,
        Commands::Identity { action } => dispatch_identity(server_url, config, action, json).await,
        Commands::Mcp { action } => dispatch_mcp(server_url, config, action, json).await,
        Commands::Sandbox { action } => dispatch_sandbox(action),
        Commands::ChatControl { action } => {
            dispatch_chat_control(server_url, action, config, json).await
        }
        Commands::Doctor { fix } => commands::doctor::run(server_url, json, fix, config).await,
        Commands::Completion { shell } => {
            commands::completion::run(shell);
            Ok(())
        }
        Commands::Secret { action } => dispatch_secret(server_url, config, action, json).await,
        Commands::Hooks { action } => dispatch_hooks(server_url, config, action, json).await,
        Commands::Webhook { action } => dispatch_webhook(action, json).await,
        Commands::Proxy { action } => dispatch_proxy(action, json).await,
        Commands::Open => commands::open_cmd::run(server_url, json).await,
        Commands::Watch { session } => {
            commands::watch::run(server_url, session.as_deref(), config, json).await
        }
    }
}

async fn dispatch_webhook(action: WebhookAction, json: bool) -> CliResult<()> {
    use commands::webhook_cmd;
    match action {
        WebhookAction::List => webhook_cmd::list(json).await,
        WebhookAction::Add => webhook_cmd::add(json).await,
        WebhookAction::Remove => webhook_cmd::remove(json).await,
    }
}

async fn dispatch_proxy(action: ProxyAction, json: bool) -> CliResult<()> {
    use commands::proxy_cmd;
    match action {
        ProxyAction::Show => proxy_cmd::show(json).await,
        ProxyAction::Set => proxy_cmd::set(json).await,
        ProxyAction::Clear => proxy_cmd::clear(json).await,
    }
}

async fn dispatch_secret(
    server_url: &str,
    config: &CliConfig,
    action: SecretAction,
    json: bool,
) -> CliResult<()> {
    use commands::secret_cmd;
    match action {
        SecretAction::List => secret_cmd::list(server_url, config, json).await,
        SecretAction::Set { name, value } => {
            secret_cmd::set(server_url, config, &name, value.as_deref(), json).await
        }
        SecretAction::Delete { name } => secret_cmd::delete(server_url, config, &name, json).await,
        SecretAction::Verify { name } => secret_cmd::verify(server_url, config, &name, json).await,
        SecretAction::Providers => secret_cmd::providers(server_url, config, json).await,
    }
}

async fn dispatch_hooks(
    server_url: &str,
    config: &CliConfig,
    action: HooksAction,
    json: bool,
) -> CliResult<()> {
    use commands::hooks_cmd;
    match action {
        HooksAction::List => hooks_cmd::list(server_url, config, json).await,
        HooksAction::Add {
            event,
            command,
            prompt,
            agent,
            url,
            matcher,
            timeout_secs,
        } => {
            hooks_cmd::add(
                server_url,
                config,
                &event,
                command.as_deref(),
                prompt.as_deref(),
                agent.as_deref(),
                url.as_deref(),
                matcher.as_deref(),
                timeout_secs,
                json,
            )
            .await
        }
        HooksAction::Remove {
            event,
            command,
            index,
        } => hooks_cmd::remove(server_url, config, &event, command.as_deref(), index, json).await,
        HooksAction::Reload => hooks_cmd::reload(server_url, config, json).await,
        HooksAction::Events => hooks_cmd::events(server_url, config, json).await,
    }
}

async fn dispatch_tools(
    server_url: &str,
    config: &CliConfig,
    category: Option<String>,
    action: Option<ToolsAction>,
    json: bool,
) -> CliResult<()> {
    match action {
        // `aleph tools` (no subcommand) — preserve legacy list behaviour, honouring
        // the global `--category` shortcut.
        None => commands::tools::run(server_url, config, category.as_deref(), json).await,
        Some(ToolsAction::List { category: sub }) => {
            // Sub-flag wins; falls back to the parent --category if absent.
            let effective = sub.or(category);
            commands::tools::run(server_url, config, effective.as_deref(), json).await
        }
        Some(ToolsAction::Describe { name }) => {
            commands::tools::describe(server_url, config, &name, json).await
        }
        Some(ToolsAction::Invoke { name, args, agent }) => {
            commands::tools::invoke(
                server_url,
                config,
                &name,
                args.as_deref(),
                agent.as_deref(),
                json,
            )
            .await
        }
    }
}

async fn dispatch_calls(
    server_url: &str,
    config: &CliConfig,
    action: CallsAction,
    json: bool,
) -> CliResult<()> {
    match action {
        CallsAction::List => commands::calls::list(server_url, config, json).await,
        CallsAction::Cancel { call_id } => {
            commands::calls::cancel(server_url, config, &call_id, json).await
        }
    }
}

async fn dispatch_session(
    server_url: &str,
    action: SessionAction,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    use commands::session;
    match action {
        SessionAction::List => session::list(server_url, config, json).await,
        SessionAction::New { name } => {
            session::create(server_url, name.as_deref(), config, json).await
        }
        SessionAction::Delete { key } => session::delete(server_url, &key, config, json).await,
        SessionAction::Usage { key } => session::usage(server_url, &key, config, json).await,
        SessionAction::Compact { key, instructions } => {
            session::compact(server_url, &key, instructions.as_deref(), config, json).await
        }
        SessionAction::Truncate { key, keep } => {
            session::truncate(server_url, &key, keep, config, json).await
        }
        SessionAction::Export {
            key,
            output,
            format,
        } => session::export(server_url, &key, output.as_deref(), format, config, json).await,
    }
}

async fn dispatch_config(
    server_url: &str,
    action: ConfigAction,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    use commands::config_cmd;
    match action {
        ConfigAction::File => {
            config_cmd::file();
            Ok(())
        }
        ConfigAction::Get { section } => {
            config_cmd::get(server_url, section.as_deref(), config, json).await
        }
        ConfigAction::Set { path, value } => {
            config_cmd::set(server_url, &path, &value, config, json).await
        }
        ConfigAction::Validate => config_cmd::validate(server_url, config, json).await,
        ConfigAction::Reload => config_cmd::reload(server_url, config, json).await,
        ConfigAction::Schema { output } => {
            config_cmd::schema(server_url, output.as_deref(), config, json).await
        }
        ConfigAction::Edit => config_cmd::edit(),
    }
}

async fn dispatch_cron(
    server_url: &str,
    action: CronAction,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    use commands::cron_cmd;
    match action {
        CronAction::List => cron_cmd::list(server_url, config, json).await,
        CronAction::Status => cron_cmd::status(server_url, config, json).await,
        CronAction::Get { job_id } => cron_cmd::get(server_url, &job_id, config, json).await,
        CronAction::Run { job_id } => cron_cmd::run(server_url, &job_id, config, json).await,
        CronAction::Runs { job_id, limit } => {
            cron_cmd::runs(server_url, &job_id, limit, config, json).await
        }
        CronAction::Create {
            name,
            schedule,
            schedule_kind,
            agent,
            prompt,
            timezone,
            tags,
        } => {
            cron_cmd::create(
                server_url,
                &name,
                schedule.as_deref(),
                schedule_kind.as_deref(),
                &agent,
                &prompt,
                timezone.as_deref(),
                tags.as_deref(),
                config,
                json,
            )
            .await
        }
        CronAction::Update {
            job_id,
            name,
            agent,
            prompt,
            schedule,
            schedule_kind,
            timezone,
            tags,
            enable,
            disable,
            timeout_secs,
            clear_timeout,
        } => {
            cron_cmd::update(
                server_url,
                &job_id,
                name.as_deref(),
                agent.as_deref(),
                prompt.as_deref(),
                schedule.as_deref(),
                schedule_kind.as_deref(),
                timezone.as_deref(),
                tags.as_deref(),
                enable,
                disable,
                timeout_secs,
                clear_timeout,
                config,
                json,
            )
            .await
        }
        CronAction::Delete { job_id } => cron_cmd::delete(server_url, &job_id, config, json).await,
        CronAction::Toggle {
            job_id,
            enable,
            disable,
        } => cron_cmd::toggle(server_url, &job_id, enable, disable, config, json).await,
    }
}

async fn dispatch_heartbeat(
    server_url: &str,
    config: &CliConfig,
    action: HeartbeatAction,
    json: bool,
) -> CliResult<()> {
    use commands::heartbeat_cmd;
    match action {
        HeartbeatAction::List => heartbeat_cmd::list(server_url, config, json).await,
        HeartbeatAction::Get { task_id } => {
            heartbeat_cmd::get(server_url, config, &task_id, json).await
        }
        HeartbeatAction::Create {
            name,
            interval,
            tool,
            agent,
            params,
            trigger,
            disabled,
        } => {
            heartbeat_cmd::create(
                server_url,
                config,
                &name,
                &interval,
                &tool,
                &agent,
                params.as_deref(),
                trigger.as_deref(),
                disabled,
                json,
            )
            .await
        }
        HeartbeatAction::Update {
            task_id,
            name,
            agent,
            interval,
            enable,
            disable,
        } => {
            heartbeat_cmd::update(
                server_url,
                config,
                &task_id,
                name.as_deref(),
                agent.as_deref(),
                interval.as_deref(),
                enable,
                disable,
                json,
            )
            .await
        }
        HeartbeatAction::Delete { task_id } => {
            heartbeat_cmd::delete(server_url, config, &task_id, json).await
        }
        HeartbeatAction::Toggle {
            task_id,
            enable,
            disable,
        } => heartbeat_cmd::toggle(server_url, config, &task_id, enable, disable, json).await,
        HeartbeatAction::Wake { task_id, reason } => {
            heartbeat_cmd::wake(server_url, config, &task_id, reason.as_deref(), json).await
        }
        HeartbeatAction::Runs { task_id, limit } => {
            heartbeat_cmd::runs(server_url, config, &task_id, limit, json).await
        }
    }
}

async fn dispatch_channels(
    server_url: &str,
    action: ChannelsAction,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    use commands::channels_cmd;
    match action {
        ChannelsAction::List => channels_cmd::list(server_url, config, json).await,
        ChannelsAction::Status { name } => {
            channels_cmd::status(server_url, name.as_deref(), config, json).await
        }
    }
}

fn dispatch_daemon(action: DaemonAction, json: bool) -> CliResult<()> {
    use commands::daemon;
    match action {
        DaemonAction::Status => daemon::status(json),
        DaemonAction::Start {
            port,
            daemon: bg,
            config,
        } => daemon::start(port, bg, config.as_deref(), json),
        DaemonAction::Stop => daemon::stop(json),
        DaemonAction::Restart {
            port,
            daemon: bg,
            config,
        } => daemon::restart(port, bg, config.as_deref(), json),
        DaemonAction::Logs { lines, level } => daemon::logs(lines, level.as_deref(), json),
    }
}

async fn dispatch_providers(
    server_url: &str,
    config: &CliConfig,
    action: ProvidersAction,
    json: bool,
) -> CliResult<()> {
    use commands::providers_cmd;
    match action {
        ProvidersAction::List => providers_cmd::list(server_url, config, json).await,
        ProvidersAction::Get { name } => providers_cmd::get(server_url, config, &name, json).await,
        ProvidersAction::Add {
            name,
            r#type,
            api_key,
            base_url,
        } => {
            providers_cmd::add(
                server_url,
                config,
                &name,
                &r#type,
                &api_key,
                base_url.as_deref(),
                json,
            )
            .await
        }
        ProvidersAction::Test { name } => {
            providers_cmd::test(server_url, config, &name, json).await
        }
        ProvidersAction::SetDefault { name } => {
            providers_cmd::set_default(server_url, config, &name, json).await
        }
        ProvidersAction::Remove { name } => {
            providers_cmd::remove(server_url, config, &name, json).await
        }
    }
}

async fn dispatch_memory(
    server_url: &str,
    config: &CliConfig,
    action: MemoryAction,
    json: bool,
) -> CliResult<()> {
    use commands::memory_cmd;
    match action {
        MemoryAction::Search { query, limit } => {
            memory_cmd::search(server_url, config, &query, limit, json).await
        }
        MemoryAction::Stats => memory_cmd::stats(server_url, config, json).await,
        MemoryAction::Clear { facts_only } => {
            memory_cmd::clear(server_url, config, facts_only, json).await
        }
        MemoryAction::Compress => memory_cmd::compress(server_url, config, json).await,
        MemoryAction::Delete { id } => memory_cmd::delete(server_url, config, &id, json).await,
    }
}

async fn dispatch_plugin(
    server_url: &str,
    config: &CliConfig,
    action: PluginAction,
    json: bool,
) -> CliResult<()> {
    use aleph_client::{AlephClient, CliError};
    use commands::{plugin_cmd, plugins_cmd};

    match action {
        // Lifecycle (server-connected)
        PluginAction::List => plugins_cmd::list(server_url, config, json).await,
        PluginAction::Install { source, scope } => {
            // Local-file / GitHub sources need client-side I/O (read the zip,
            // fetch the release) and stay here. Everything else is forwarded
            // raw to the daemon, which owns the marketplace-vs-git-url
            // classification (R4: the shell no longer decides).
            if source.starts_with("github:") || source.ends_with(".zip") {
                plugins_cmd::install(server_url, config, &source, json).await
            } else {
                let (client, _events) = AlephClient::connect(server_url, config).await?;
                let result: serde_json::Value = client
                    .call(
                        "plugin.install",
                        Some(serde_json::json!({ "source": source, "scope": scope })),
                    )
                    .await?;
                if json {
                    crate::output::print_json(&result);
                } else {
                    println!("{result}");
                }
                client.close().await?;
                Ok(())
            }
        }
        PluginAction::Uninstall { name, .. } => {
            plugins_cmd::uninstall(server_url, config, &name, json).await
        }
        PluginAction::Enable { name } => plugins_cmd::enable(server_url, config, &name, json).await,
        PluginAction::Disable { name } => {
            plugins_cmd::disable(server_url, config, &name, json).await
        }
        PluginAction::Call {
            plugin,
            tool,
            params,
        } => plugins_cmd::call(server_url, config, &plugin, &tool, params.as_deref(), json).await,
        PluginAction::Update { name } => plugins_cmd::update(server_url, config, &name, json).await,
        PluginAction::Reload { name } => plugins_cmd::reload(server_url, config, &name, json).await,
        PluginAction::Info { name } => plugins_cmd::info(server_url, config, &name, json).await,
        // Dev tools (local, no server)
        PluginAction::Init {
            name,
            template,
            dir,
        } => {
            let tmpl: plugin_cmd::PluginTemplate =
                template.parse().map_err(|e: String| CliError::Other(e))?;
            let dir_path = dir.map(std::path::PathBuf::from);
            plugin_cmd::init(&name, tmpl, dir_path.as_deref())
        }
        PluginAction::Validate { path } => plugin_cmd::validate(std::path::Path::new(&path), json),
        PluginAction::Pack { path, output } => {
            let out = output.map(std::path::PathBuf::from);
            plugin_cmd::pack(std::path::Path::new(&path), out.as_deref())
        }
        PluginAction::Doctor => plugin_cmd::doctor(json),
        PluginAction::Sync => commands::skills_cmd::sync(server_url, config, "plugins", json).await,
        // Marketplace
        PluginAction::Marketplace { action } => {
            dispatch_marketplace(server_url, config, action, json).await
        }
    }
}

async fn dispatch_marketplace(
    server_url: &str,
    config: &CliConfig,
    action: MarketplaceAction,
    json: bool,
) -> CliResult<()> {
    use aleph_client::AlephClient;
    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let (method, params) = match action {
        MarketplaceAction::Add { source } => (
            "plugin.marketplace.add",
            serde_json::json!({ "source": source }),
        ),
        MarketplaceAction::List => ("plugin.marketplace.list", serde_json::json!({})),
        MarketplaceAction::Update { name } => (
            "plugin.marketplace.update",
            match name {
                Some(n) => serde_json::json!({ "name": n }),
                None => serde_json::json!({}),
            },
        ),
        MarketplaceAction::Remove { name } => (
            "plugin.marketplace.remove",
            serde_json::json!({ "name": name }),
        ),
    };
    let result: serde_json::Value = client.call(method, Some(params)).await?;
    if json {
        crate::output::print_json(&result);
    } else if matches!(method, "plugin.marketplace.list") {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{result}");
    }
    client.close().await?;
    Ok(())
}

async fn dispatch_services(
    server_url: &str,
    config: &CliConfig,
    action: ServicesAction,
    json: bool,
) -> CliResult<()> {
    use commands::services_cmd;
    match action {
        ServicesAction::List { plugin, state } => {
            services_cmd::list(
                server_url,
                config,
                plugin.as_deref(),
                state.as_deref(),
                json,
            )
            .await
        }
        ServicesAction::Status {
            plugin_id,
            service_id,
        } => services_cmd::status(server_url, config, &plugin_id, &service_id, json).await,
        ServicesAction::Start {
            plugin_id,
            service_id,
        } => services_cmd::start(server_url, config, &plugin_id, &service_id, json).await,
        ServicesAction::Stop {
            plugin_id,
            service_id,
        } => services_cmd::stop(server_url, config, &plugin_id, &service_id, json).await,
    }
}

async fn dispatch_skills(
    server_url: &str,
    config: &CliConfig,
    action: SkillsAction,
    json: bool,
) -> CliResult<()> {
    use commands::skills_cmd;
    match action {
        SkillsAction::List => skills_cmd::list(server_url, config, json).await,
        SkillsAction::Install { source } => {
            skills_cmd::install(server_url, config, &source, json).await
        }
        SkillsAction::Delete { name } => skills_cmd::delete(server_url, config, &name, json).await,
        SkillsAction::Sync => skills_cmd::sync(server_url, config, "skills", json).await,
    }
}

async fn dispatch_workspace(
    server_url: &str,
    config: &CliConfig,
    action: WorkspaceAction,
    json: bool,
) -> CliResult<()> {
    use commands::workspace_cmd;
    match action {
        WorkspaceAction::List => workspace_cmd::list(server_url, config, json).await,
        WorkspaceAction::Create { name, description } => {
            workspace_cmd::create(server_url, config, &name, description.as_deref(), json).await
        }
        WorkspaceAction::Archive { name } => {
            workspace_cmd::archive(server_url, config, &name, json).await
        }
    }
}

async fn dispatch_gateway(
    server_url: &str,
    config: &CliConfig,
    action: GatewayAction,
    json: bool,
) -> CliResult<()> {
    use commands::gateway_cmd;
    match action {
        GatewayAction::Call { method, params } => {
            gateway_cmd::call(server_url, config, &method, params.as_deref(), json).await
        }
    }
}

async fn dispatch_logs(
    server_url: &str,
    config: &CliConfig,
    action: LogsAction,
    json: bool,
) -> CliResult<()> {
    use commands::logs_cmd;
    match action {
        LogsAction::Level => logs_cmd::level(server_url, config, json).await,
        LogsAction::SetLevel { level } => {
            logs_cmd::set_level(server_url, config, &level, json).await
        }
        LogsAction::Dir => logs_cmd::dir(server_url, config, json).await,
    }
}

async fn dispatch_trace(
    server_url: &str,
    config: &CliConfig,
    action: TraceAction,
    json: bool,
) -> CliResult<()> {
    use commands::trace_cmd;
    match action {
        TraceAction::List { limit } => trace_cmd::list(server_url, config, limit, json).await,
        TraceAction::Show { task_id } => trace_cmd::show(server_url, config, &task_id, json).await,
    }
}

async fn dispatch_identity(
    server_url: &str,
    config: &CliConfig,
    action: IdentityAction,
    json: bool,
) -> CliResult<()> {
    use commands::identity_cmd;
    match action {
        IdentityAction::Get { agent } => {
            identity_cmd::get(server_url, config, agent.as_deref(), json).await
        }
        IdentityAction::Set {
            content,
            file,
            file_name,
            agent,
        } => {
            identity_cmd::set(
                server_url,
                config,
                content.as_deref(),
                file.as_deref(),
                file_name.as_deref(),
                agent.as_deref(),
                json,
            )
            .await
        }
        IdentityAction::Clear { file_name, agent } => {
            identity_cmd::clear(
                server_url,
                config,
                file_name.as_deref(),
                agent.as_deref(),
                json,
            )
            .await
        }
        IdentityAction::List { agent } => {
            identity_cmd::list(server_url, config, agent.as_deref(), json).await
        }
    }
}

async fn dispatch_mcp(
    server_url: &str,
    config: &CliConfig,
    action: McpAction,
    json: bool,
) -> CliResult<()> {
    use commands::mcp_cmd;
    match action {
        McpAction::Pending => mcp_cmd::pending(server_url, config, json).await,
        McpAction::Approve { request_id, reason } => {
            mcp_cmd::approve(server_url, config, &request_id, reason.as_deref(), json).await
        }
        McpAction::Reject { request_id, reason } => {
            mcp_cmd::reject(server_url, config, &request_id, reason.as_deref(), json).await
        }
        McpAction::Cancel { request_id } => {
            mcp_cmd::cancel(server_url, config, &request_id, json).await
        }
    }
}

fn dispatch_sandbox(action: SandboxAction) -> CliResult<()> {
    use commands::sandbox_cmd;
    match action {
        SandboxAction::Info => sandbox_cmd::info(),
        SandboxAction::Run {
            network,
            fs_write,
            fs_read,
            log_denials,
            command,
        } => sandbox_cmd::run(
            network.as_deref(),
            &fs_write,
            &fs_read,
            log_denials,
            &command,
        ),
    }
}

async fn dispatch_chat_control(
    server_url: &str,
    action: ChatControlAction,
    config: &CliConfig,
    json: bool,
) -> CliResult<()> {
    use commands::chat_cmd;
    match action {
        ChatControlAction::Send {
            message,
            session,
            stream,
            thinking,
        } => {
            chat_cmd::send(
                server_url,
                &message,
                session.as_deref(),
                stream,
                thinking.as_deref(),
                config,
                json,
            )
            .await
        }
        ChatControlAction::Abort { run_id } => {
            chat_cmd::abort(server_url, &run_id, config, json).await
        }
        ChatControlAction::History { session_key, limit } => {
            chat_cmd::history(server_url, &session_key, limit, config, json).await
        }
        ChatControlAction::Clear {
            session_key,
            keep_system,
        } => chat_cmd::clear(server_url, &session_key, keep_system, config, json).await,
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn parse_trace_list_command() {
        assert!(Cli::try_parse_from(["aleph", "trace", "list"]).is_ok());
    }

    #[test]
    fn parses_open_subcommand() {
        assert!(Cli::try_parse_from(["aleph", "open"]).is_ok());
    }

    #[test]
    fn parse_trace_show_command() {
        assert!(Cli::try_parse_from(["aleph", "trace", "show", "task-1"]).is_ok());
    }

    // === Backward-compatibility guards: every legacy command path must still parse. ===

    #[test]
    fn parse_tools_bare_remains_supported() {
        // `aleph tools` (no subcommand) preserves the original list-with-filter shape.
        assert!(Cli::try_parse_from(["aleph", "tools"]).is_ok());
    }

    #[test]
    fn parse_tools_legacy_category_flag_still_parses() {
        // Existing scripts using `aleph tools -c foo` must keep working.
        assert!(Cli::try_parse_from(["aleph", "tools", "-c", "memory"]).is_ok());
    }

    #[test]
    fn parse_tools_describe_subcommand() {
        assert!(Cli::try_parse_from(["aleph", "tools", "describe", "memory_search"]).is_ok());
    }

    #[test]
    fn parse_tools_invoke_with_args() {
        assert!(Cli::try_parse_from([
            "aleph",
            "tools",
            "invoke",
            "memory_search",
            "--args",
            r#"{"query":"x"}"#
        ])
        .is_ok());
    }

    #[test]
    fn parse_cron_create_minimum() {
        assert!(Cli::try_parse_from([
            "aleph",
            "cron",
            "create",
            "daily-job",
            "--schedule",
            "0 0 * * *"
        ])
        .is_ok());
    }

    #[test]
    fn parse_cron_update_with_enable() {
        assert!(Cli::try_parse_from([
            "aleph", "cron", "update", "id-1", "--name", "renamed", "--enable"
        ])
        .is_ok());
    }

    #[test]
    fn parse_cron_update_enable_and_disable_conflict() {
        let result =
            Cli::try_parse_from(["aleph", "cron", "update", "id-1", "--enable", "--disable"]);
        assert!(
            result.is_err(),
            "clap's conflicts_with should reject --enable --disable"
        );
    }

    #[test]
    fn parse_session_truncate() {
        assert!(Cli::try_parse_from(["aleph", "session", "truncate", "abc", "-n", "10"]).is_ok());
    }

    #[test]
    fn parse_heartbeat_create_basic() {
        assert!(Cli::try_parse_from([
            "aleph",
            "heartbeat",
            "create",
            "check-gmail",
            "-i",
            "5m",
            "-t",
            "memory_search"
        ])
        .is_ok());
    }

    #[test]
    fn parse_heartbeat_wake() {
        assert!(Cli::try_parse_from([
            "aleph",
            "heartbeat",
            "wake",
            "check-gmail",
            "-r",
            "manual probe"
        ])
        .is_ok());
    }

    #[test]
    fn parse_plugins_legacy_alias_removed() {
        // Confirm the deprecated `aleph plugins` variant no longer exists as a
        // top-level subcommand — only `aleph plugin` is recognised.
        let result = Cli::try_parse_from(["aleph", "plugins", "list"]);
        assert!(
            result.is_err(),
            "legacy `aleph plugins` should now be rejected; use `aleph plugin`"
        );
    }

    #[test]
    fn lan_trust_removed_auth_surfaces() {
        // LAN-trust revert (T6): the gateway has no auth, so the client's
        // auth/pairing/devices subcommands are gone. Confirm they no longer
        // parse as top-level commands.
        assert!(Cli::try_parse_from(["aleph", "pairing", "list"]).is_err());
        assert!(Cli::try_parse_from(["aleph", "devices", "list"]).is_err());
        assert!(Cli::try_parse_from(["aleph", "auth", "show-token"]).is_err());
        assert!(Cli::try_parse_from(["aleph", "guests", "list"]).is_err());
    }

    #[test]
    fn parse_secret_subcommands() {
        assert!(Cli::try_parse_from(["aleph", "secret", "list"]).is_ok());
        assert!(Cli::try_parse_from(["aleph", "secret", "providers"]).is_ok());
        // Set with inline value, and without (where the handler will prompt).
        assert!(Cli::try_parse_from(["aleph", "secret", "set", "OPENAI_API_KEY"]).is_ok());
        assert!(Cli::try_parse_from([
            "aleph",
            "secret",
            "set",
            "OPENAI_API_KEY",
            "--value",
            "sk-x"
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["aleph", "secret", "delete", "OPENAI_API_KEY"]).is_ok());
        assert!(Cli::try_parse_from(["aleph", "secret", "verify", "OPENAI_API_KEY"]).is_ok());
        // delete/verify require a name
        assert!(Cli::try_parse_from(["aleph", "secret", "delete"]).is_err());
        assert!(Cli::try_parse_from(["aleph", "secret", "verify"]).is_err());
    }

    #[test]
    fn parse_hooks_subcommands() {
        assert!(Cli::try_parse_from(["aleph", "hooks", "list"]).is_ok());
        assert!(Cli::try_parse_from(["aleph", "hooks", "events"]).is_ok());
        assert!(Cli::try_parse_from(["aleph", "hooks", "reload"]).is_ok());
        assert!(Cli::try_parse_from([
            "aleph",
            "hooks",
            "add",
            "before_tool_call",
            "--command",
            "echo hi"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "aleph",
            "hooks",
            "remove",
            "before_tool_call",
            "--index",
            "0"
        ])
        .is_ok());
        // command/prompt/agent/url conflict — clap should refuse two at once
        assert!(Cli::try_parse_from([
            "aleph",
            "hooks",
            "add",
            "before_tool_call",
            "--command",
            "x",
            "--prompt",
            "y"
        ])
        .is_err());
        // remove must supply --command or --index
        assert!(Cli::try_parse_from(["aleph", "hooks", "remove", "before_tool_call"]).is_ok());
        // parses; handler rejects
    }

    #[test]
    fn parse_webhook_and_proxy_preview_surfaces() {
        // Webhook
        assert!(Cli::try_parse_from(["aleph", "webhook", "list"]).is_ok());
        assert!(Cli::try_parse_from(["aleph", "webhook", "add"]).is_ok());
        assert!(Cli::try_parse_from(["aleph", "webhook", "remove"]).is_ok());
        // Proxy
        assert!(Cli::try_parse_from(["aleph", "proxy", "show"]).is_ok());
        assert!(Cli::try_parse_from(["aleph", "proxy", "set"]).is_ok());
        assert!(Cli::try_parse_from(["aleph", "proxy", "clear"]).is_ok());
    }
}
