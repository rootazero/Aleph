//! CLI argument enums for the `aleph` binary.
//!
//! Extracted out of `main.rs` so the entry point stays focused on parsing,
//! logging and dispatch. Every subcommand enum lives here; `main.rs` only
//! wires them to handler functions.

use clap::{Subcommand, ValueEnum};

/// Top-level subcommands for the `aleph` client CLI.
///
/// Each variant maps to one handler module under `commands/`. Variants do not
/// carry logic — they are pure I/O envelopes (R4: interface-only).
#[derive(Subcommand)]
pub enum Commands {
    /// Start interactive chat session
    Chat {
        /// Session key (creates new if not specified)
        #[arg(short, long)]
        session: Option<String>,

        /// Agent name to bind this session to
        #[arg(short, long)]
        agent: Option<String>,

        /// Reopen the most recently active session (by `last_active_at`)
        ///
        /// The interactive twin of `aleph ask --last`, and the same resolver
        /// (`aleph_client::resolve_last_session`). Opt-in rather than the
        /// default, matching codex `resume` and pi `--continue`: a bare
        /// `aleph chat` must not silently append to yesterday's thread.
        #[arg(short = 'c', long = "continue", conflicts_with = "session")]
        continue_last: bool,
    },

    /// Send a single message and get response
    ///
    /// The message may be omitted when text is piped on stdin
    /// (`echo "prompt" | aleph ask`). When both are given, the piped text is
    /// appended below the message as context (`git diff | aleph ask "review"`).
    Ask {
        /// The message to send (optional when piping text on stdin)
        message: Option<String>,

        /// Session key
        #[arg(short, long, conflicts_with = "last")]
        session: Option<String>,

        /// Resume the most recently active session (by `last_active_at`)
        #[arg(long, conflicts_with = "session")]
        last: bool,

        /// Write the final agent message to FILE (codex-parity `-o`)
        #[arg(short = 'o', long = "output-last-message", value_name = "FILE")]
        output_last_message: Option<String>,
    },

    /// Check server health
    Health,

    /// List, describe, or directly invoke registered tools
    Tools {
        /// Filter the implicit list by category (legacy shortcut for `tools list -c`)
        #[arg(short, long)]
        category: Option<String>,

        #[command(subcommand)]
        action: Option<ToolsAction>,
    },

    /// Inspect or cancel in-flight tool calls (per-tool `AbortSignal` RPC)
    Calls {
        #[command(subcommand)]
        action: CallsAction,
    },

    /// Manage sessions
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Show server information
    Info,

    /// Open a connection and report the role the server grants. A loopback
    /// server auto-authorizes as operator; a remote one walls the connection
    /// until it is shown a credential, which this CLI cannot yet present.
    Connect {
        /// Device name for this client
        #[arg(short, long, default_value = "aleph-cli")]
        device: String,
    },

    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Manage cron jobs
    Cron {
        #[command(subcommand)]
        action: CronAction,
    },

    /// Manage heartbeat tasks (recurring agent probes with conditional wake)
    Heartbeat {
        #[command(subcommand)]
        action: HeartbeatAction,
    },

    /// Manage channels
    Channels {
        #[command(subcommand)]
        action: ChannelsAction,
    },

    /// Manage Gateway daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },

    /// Call any Gateway RPC method directly
    Gateway {
        #[command(subcommand)]
        action: GatewayAction,
    },

    /// AI provider management
    Providers {
        #[command(subcommand)]
        action: ProvidersAction,
    },

    /// Memory management
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },

    /// Plugin management — lifecycle, dev tools, and marketplace
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },

    /// Background service management
    Services {
        #[command(subcommand)]
        action: ServicesAction,
    },

    /// Skill management (list, install, remove)
    Skills {
        #[command(subcommand)]
        action: SkillsAction,
    },

    /// Workspace management
    Workspace {
        #[command(subcommand)]
        action: WorkspaceAction,
    },

    /// User management — add people to this core and manage their roles
    Users {
        #[command(subcommand)]
        action: UsersAction,
    },

    /// Log management
    Logs {
        #[command(subcommand)]
        action: LogsAction,
    },

    /// Structured trace replay inspection
    Trace {
        #[command(subcommand)]
        action: TraceAction,
    },

    /// Identity/soul management
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
    },

    /// Chat control (send, abort, history, clear)
    #[command(name = "chat-control")]
    ChatControl {
        #[command(subcommand)]
        action: ChatControlAction,
    },

    /// MCP tool approval workflow
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },

    /// Run a command under Aleph's sandbox (forwards to aleph-server sandbox-debug)
    Sandbox {
        #[command(subcommand)]
        action: SandboxAction,
    },

    /// Diagnose installation, config, gateway, providers, MCP and sandbox.
    ///
    /// When checks fail and stdout is a TTY, `doctor` offers to launch an
    /// AI-assisted repair (press `f`): the failing checks are forwarded to
    /// the daemon agent, which fixes what it can via its own tools. Pass
    /// `--fix` to launch that repair without the interactive prompt.
    Doctor {
        /// Skip the interactive `[f]` prompt and launch AI-assisted repair
        /// directly whenever any check fails.
        #[arg(long)]
        fix: bool,
    },

    /// Generate shell completion script
    Completion {
        /// Shell type (bash, zsh, fish, elvish, powershell)
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    /// Manage named vault secrets (API keys, tokens, etc.)
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },

    /// Manage user-level hooks in ~/.aleph/hooks.json
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },

    /// (Preview) Manage inbound webhook subscriptions
    Webhook {
        #[command(subcommand)]
        action: WebhookAction,
    },

    /// (Preview) Manage outbound HTTP/HTTPS proxy configuration
    Proxy {
        #[command(subcommand)]
        action: ProxyAction,
    },

    /// Open the Aleph Panel in the system browser. The static Panel assets are
    /// served unauthenticated (the login wall is on the WebSocket, not the HTTP
    /// root), so this just derives the Panel URL from the configured endpoint
    /// and launches the browser — a remote Panel will ask for a credential
    /// itself once it connects.
    Open,

    /// Watch live agent activity across all sessions as a timestamped feed
    /// (run accepted, tool calls, provider retries, run receipts). Ctrl-C
    /// stops and prints an observation summary.
    Watch {
        /// Only show runs belonging to this session key
        #[arg(short, long)]
        session: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ToolsAction {
    /// List registered tools (optionally filtered by category)
    List {
        /// Filter by category
        #[arg(short, long)]
        category: Option<String>,
    },
    /// Show a single tool's description and schema metadata
    Describe {
        /// Tool name (key) as shown by `aleph tools list`
        name: String,
    },
    /// Directly invoke a tool, bypassing the LLM loop. JSON arguments are
    /// forwarded to `tools.invoke` (deterministic execution path).
    Invoke {
        /// Registered tool name (e.g. "`memory_search`")
        name: String,
        /// JSON arguments object (e.g. '{"query": "foo"}'). Defaults to `{}`.
        #[arg(short = 'a', long = "args", value_name = "JSON")]
        args: Option<String>,
        /// Override the resolving `agent_id` (defaults to "main" server-side)
        #[arg(long)]
        agent: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Print config file path
    File,
    /// Get configuration (optionally by section: gateway, agents, channels, etc.)
    Get {
        /// Config section name (e.g., gateway, agents, channels)
        section: Option<String>,
    },
    /// Set a configuration value
    Set {
        /// Dot-separated config path (e.g., gateway.port)
        path: String,
        /// Value to set (JSON or plain string)
        value: String,
    },
    /// Validate current configuration
    Validate,
    /// Reload configuration on the server
    Reload,
    /// Export configuration schema
    Schema {
        /// Output file path (prints to stdout if not specified)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Open config file in $EDITOR
    Edit,
}

#[derive(Subcommand)]
pub enum CronAction {
    /// List all cron jobs
    List,
    /// Show cron scheduler status
    Status,
    /// Inspect a single job's definition and state
    Get {
        /// Job ID
        job_id: String,
    },
    /// Trigger a cron job manually (sets `next_run_at_ms` to now)
    Run {
        /// Job ID to trigger
        job_id: String,
    },
    /// Show execution history for a job (SQLite-backed)
    Runs {
        /// Job ID
        job_id: String,
        /// Max records to return
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Create a new cron job
    Create {
        /// Human-readable job name
        name: String,
        /// Cron expression (e.g. '*/5 * * * *'). Use --schedule-kind for At/Interval.
        #[arg(short = 's', long, conflicts_with = "schedule_kind")]
        schedule: Option<String>,
        /// Raw `ScheduleKind` JSON (advanced; conflicts with --schedule)
        #[arg(long = "schedule-kind", value_name = "JSON")]
        schedule_kind: Option<String>,
        /// Agent ID to run as (defaults to "main")
        #[arg(short, long, default_value = "main")]
        agent: String,
        /// Prompt the agent should run on each tick
        #[arg(short, long, default_value = "")]
        prompt: String,
        /// IANA timezone (e.g. "`America/New_York`")
        #[arg(long)]
        timezone: Option<String>,
        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,
    },
    /// Update an existing cron job (partial — only provided flags are sent)
    Update {
        /// Job ID
        job_id: String,
        /// New name
        #[arg(long)]
        name: Option<String>,
        /// New agent ID
        #[arg(long)]
        agent: Option<String>,
        /// New prompt
        #[arg(long)]
        prompt: Option<String>,
        /// New cron expression (mutually exclusive with --schedule-kind)
        #[arg(long, conflicts_with = "schedule_kind")]
        schedule: Option<String>,
        /// New `ScheduleKind` JSON (advanced)
        #[arg(long = "schedule-kind", value_name = "JSON")]
        schedule_kind: Option<String>,
        /// New timezone
        #[arg(long)]
        timezone: Option<String>,
        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,
        /// Enable the job (mutually exclusive with --disable)
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        /// Disable the job
        #[arg(long)]
        disable: bool,
        /// Override the per-job execution timeout (seconds). Positive sets the
        /// override; conflicts with --clear-timeout. Omit both to leave as-is.
        #[arg(long, value_name = "SECS", conflicts_with = "clear_timeout")]
        timeout_secs: Option<u64>,
        /// Clear the per-job execution timeout override (revert to default).
        #[arg(long)]
        clear_timeout: bool,
    },
    /// Delete a cron job permanently
    Delete {
        /// Job ID
        job_id: String,
    },
    /// Toggle enabled state (or force enable/disable)
    Toggle {
        /// Job ID
        job_id: String,
        /// Force enable
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        /// Force disable
        #[arg(long)]
        disable: bool,
    },
}

#[derive(Subcommand)]
pub enum HeartbeatAction {
    /// List heartbeat tasks
    List,
    /// Inspect a single heartbeat task
    Get {
        /// Task ID
        task_id: String,
    },
    /// Create a heartbeat task. The probe tool is invoked each interval; when
    /// `--trigger always` or the configured trigger fires, the agent wakes.
    Create {
        /// Task name
        name: String,
        /// Probe interval (e.g. "5m", "30s", "1h", or raw ms)
        #[arg(short, long)]
        interval: String,
        /// Probe tool name (registered in the `BuiltinToolRegistry`)
        #[arg(short = 't', long)]
        tool: String,
        /// Agent ID to wake (defaults to "main")
        #[arg(short, long, default_value = "main")]
        agent: String,
        /// JSON object passed as probe `tool_params`
        #[arg(short = 'p', long = "params", value_name = "JSON")]
        params: Option<String>,
        /// Raw `TriggerCondition` JSON (defaults to {"Always":{}} server-side)
        #[arg(long = "trigger", value_name = "JSON")]
        trigger: Option<String>,
        /// Start disabled (default is enabled)
        #[arg(long)]
        disabled: bool,
    },
    /// Update an existing heartbeat task (partial)
    Update {
        /// Task ID
        task_id: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        agent: Option<String>,
        /// New interval (e.g. "10m")
        #[arg(short, long)]
        interval: Option<String>,
        /// Enable the task (mutually exclusive with --disable)
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        /// Disable the task
        #[arg(long)]
        disable: bool,
    },
    /// Delete a heartbeat task
    Delete {
        /// Task ID
        task_id: String,
    },
    /// Toggle enabled state (or force enable/disable)
    Toggle {
        /// Task ID
        task_id: String,
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
    },
    /// Manually trigger a heartbeat task (`UserAction` priority)
    Wake {
        /// Task ID
        task_id: String,
        /// Free-form reason recorded in the wake request
        #[arg(short, long)]
        reason: Option<String>,
    },
    /// Show execution history for a task
    Runs {
        /// Task ID
        task_id: String,
        /// Max records to return
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
}

#[derive(Subcommand)]
pub enum ChannelsAction {
    /// List all channels
    List,
    /// Show channel status
    Status {
        /// Channel name (shows all if not specified)
        name: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum CallsAction {
    /// List every currently-in-flight tool call (`tools.in_flight`)
    List,
    /// Cancel a specific in-flight tool call by `tool_call_id`
    Cancel {
        /// LLM-issued `tool_call_id` (see `aleph calls list`)
        call_id: String,
    },
}

#[derive(Subcommand)]
pub enum GatewayAction {
    /// Call an RPC method
    Call {
        /// RPC method name (e.g., "health", "providers.list")
        method: String,
        /// JSON params (optional, e.g., '{"section": "general"}')
        params: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Show Gateway server status
    Status,
    /// Start Gateway server
    Start {
        /// Port number for the server
        #[arg(short, long)]
        port: Option<u16>,
        /// Run as a background daemon
        #[arg(short, long)]
        daemon: bool,
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,
    },
    /// Stop Gateway server
    Stop,
    /// Restart Gateway server
    Restart {
        /// Port number for the server
        #[arg(short, long)]
        port: Option<u16>,
        /// Run as a background daemon
        #[arg(short, long)]
        daemon: bool,
        /// Path to configuration file
        #[arg(short, long)]
        config: Option<String>,
    },
    /// View Gateway logs
    Logs {
        /// Number of lines to show
        #[arg(short = 'n', long, default_value = "50")]
        lines: usize,
        /// Filter by log level (e.g., warn, error)
        #[arg(short, long)]
        level: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ProvidersAction {
    /// List AI providers, optionally narrowed to a search query
    List {
        /// Substring to match against provider names — omit to list everything
        ///
        /// Filtered client-side through the shared ranker, so the row a bare
        /// query lands on is the same one the Panel and the TUI open.
        query: Option<String>,
    },
    /// Get provider details
    Get { name: String },
    /// Replace a provider's model ladder
    ///
    /// The other half of `providers models --refresh`: that reports what a
    /// vendor serves, this adopts it. Order is semantic — the first `--model`
    /// is what a turn naming no model gets, the rest are the failover rungs.
    SetModels {
        name: String,
        /// Model id to offer; repeat to declare failover rungs
        #[arg(long = "model", value_name = "ID")]
        models: Vec<String>,
    },
    /// Add a new provider
    Add {
        name: String,
        /// Provider type (e.g., openai, anthropic, ollama)
        #[arg(long)]
        r#type: String,
        /// API key
        #[arg(long)]
        api_key: String,
        /// Base URL (optional)
        #[arg(long)]
        base_url: Option<String>,
        /// Model id to offer; repeat to declare failover rungs (the first is
        /// the one a turn naming no model gets)
        ///
        /// Required: `providers.create` rejects an empty ladder outright, so
        /// there is no "let the server pick" to fall back on. Long-only, like
        /// every other flag on this subcommand.
        #[arg(long = "model", value_name = "ID")]
        models: Vec<String>,
    },
    /// Test provider connectivity
    Test { name: String },
    /// List the models a provider offers, or refresh them from the vendor
    Models {
        /// Provider name — omit to cover every provider
        name: Option<String>,
        /// Ask each provider's /models endpoint instead of reading the
        /// catalogue, and report how it answered
        #[arg(long)]
        refresh: bool,
    },
    /// Set as default provider
    SetDefault { name: String },
    /// Remove a provider
    Remove { name: String },
}

#[derive(Subcommand)]
pub enum MemoryAction {
    /// Search memory
    Search {
        /// Search query
        query: String,
        /// Maximum results to return
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Show memory statistics
    Stats,
    /// Clear memory
    Clear {
        /// Only clear facts, keep other memories
        #[arg(long)]
        facts_only: bool,
    },
    /// Compress and optimize memory
    Compress,
    /// Delete a specific memory entry
    Delete {
        /// Memory entry ID
        id: String,
    },
}

/// Unified plugin subcommands: lifecycle, dev tools, and marketplace.
///
/// Note: the legacy hidden `Plugins` variant was removed during the CLI parity
/// pass — `aleph plugin` is the single entrypoint for both lifecycle and dev.
#[derive(Subcommand)]
pub enum PluginAction {
    // === Lifecycle (server-connected) ===
    /// List installed plugins
    List,
    /// Install a plugin from source (URL, path, or zip)
    Install {
        source: String,
        /// Installation scope: user or system
        #[arg(long, default_value = "user")]
        scope: String,
    },
    /// Uninstall a plugin
    Uninstall {
        name: String,
        /// Scope override
        #[arg(long)]
        scope: Option<String>,
        /// Keep plugin data directory
        #[arg(long)]
        keep_data: bool,
    },
    /// Enable a disabled plugin
    Enable { name: String },
    /// Disable a plugin
    Disable { name: String },
    /// Update an installed plugin to its latest marketplace version
    Update {
        /// Plugin name to update
        name: String,
    },
    /// Hot-reload a single installed plugin by name
    Reload {
        /// Plugin name (ID) to reload
        name: String,
    },
    /// Show detailed info about a specific plugin
    Info {
        /// Plugin name or ID
        name: String,
    },
    /// Call a plugin tool
    Call {
        /// Plugin name
        plugin: String,
        /// Tool name
        tool: String,
        /// JSON params (optional)
        params: Option<String>,
    },
    // === Dev tools (local, no server needed) ===
    /// Scaffold a new plugin project
    Init {
        /// Plugin name
        name: String,
        /// Plugin type: nodejs, wasm, or static
        #[arg(short = 't', long = "type", default_value = "static")]
        template: String,
        /// Target directory (defaults to ./<name>)
        #[arg(short, long)]
        dir: Option<String>,
    },
    /// Validate a plugin directory
    Validate {
        /// Plugin directory (defaults to current directory)
        #[arg(default_value = ".")]
        path: String,
    },
    /// Package a plugin for distribution
    Pack {
        /// Plugin directory (defaults to current directory)
        #[arg(default_value = ".")]
        path: String,
        /// Output file path
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Run plugin diagnostics
    Doctor,
    /// Refresh official plugins from the external repo (git clone/pull)
    Sync,
    // === Marketplace ===
    /// Plugin marketplace management
    Marketplace {
        #[command(subcommand)]
        action: MarketplaceAction,
    },
}

#[derive(Subcommand)]
pub enum MarketplaceAction {
    /// Add a marketplace source
    Add { source: String },
    /// List configured marketplace sources
    List,
    /// Update marketplace index (all sources or a specific one)
    Update { name: Option<String> },
    /// Remove a marketplace source
    Remove { name: String },
}

#[derive(Subcommand)]
pub enum ServicesAction {
    /// List background services
    List {
        /// Filter by plugin ID
        #[arg(long)]
        plugin: Option<String>,
        /// Filter by state (running, stopped, error)
        #[arg(long)]
        state: Option<String>,
    },
    /// Get service status
    Status {
        /// Plugin ID
        plugin_id: String,
        /// Service ID
        service_id: String,
    },
    /// Start a service
    Start {
        /// Plugin ID
        plugin_id: String,
        /// Service ID
        service_id: String,
    },
    /// Stop a service
    Stop {
        /// Plugin ID
        plugin_id: String,
        /// Service ID
        service_id: String,
    },
}

#[derive(Subcommand)]
pub enum SkillsAction {
    /// List all installed skills
    List,
    /// Install a skill from a git URL, local path, or .zip
    Install { source: String },
    /// Remove an installed skill by name
    Delete { name: String },
    /// Refresh official skills from the external repo (git clone/pull)
    Sync,
}

#[derive(Subcommand)]
pub enum SessionAction {
    /// List all sessions
    List,
    /// Create a new session
    New {
        /// Session name
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Delete a session
    Delete {
        /// Session key to delete
        key: String,
    },
    /// Show session usage statistics
    Usage {
        /// Session key
        key: String,
    },
    /// Compact a session: summarize its older turns and stop replaying them
    Compact {
        /// Session key
        key: String,
        /// Optional focus for the summary, e.g. "keep the migration details"
        #[arg(long)]
        instructions: Option<String>,
    },
    /// Truncate the session message log to the most recent N messages
    /// (session.truncate; preserves session metadata).
    Truncate {
        /// Session key
        key: String,
        /// Number of most recent messages to keep
        #[arg(short = 'n', long, default_value = "20")]
        keep: usize,
    },
    /// Export a session's full transcript to a file or stdout.
    ///
    /// Reads the complete message log via `chat.history` (no limit) and
    /// renders it as Markdown (default) or JSON. Pure I/O — no server-side
    /// state is mutated.
    Export {
        /// Session key to export
        key: String,
        /// Write the transcript to FILE instead of stdout
        #[arg(short = 'o', long = "output", value_name = "FILE")]
        output: Option<String>,
        /// Output format (markdown or json). The global `--json` flag implies
        /// json when this is omitted.
        #[arg(long, value_enum)]
        format: Option<SessionExportFormat>,
    },
}

/// Transcript export rendering format for `session export`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum SessionExportFormat {
    /// Human-readable Markdown transcript
    Markdown,
    /// Machine-readable JSON (`chat.history` envelope)
    Json,
}

#[derive(Subcommand)]
pub enum WorkspaceAction {
    /// List all workspaces
    List {
        /// Also show archived workspaces, with a Status column
        ///
        /// Archiving is a soft delete, so without this the result of
        /// `workspace archive` cannot be seen from any client.
        #[arg(long)]
        include_archived: bool,
    },
    /// Show one workspace in detail
    ///
    /// Archived workspaces are shown too — this is addressed by exact ID, and
    /// the output says so on its Status line. `update` refuses them.
    Get {
        /// Workspace ID (the ID column of `workspace list`)
        id: String,
    },
    /// Create a new workspace
    Create {
        /// Workspace ID — the key `archive` addresses it by (URL-safe slug, e.g. "crypto")
        id: String,
        /// Display name (defaults to the ID)
        #[arg(long)]
        name: Option<String>,
        /// Optional description
        #[arg(long)]
        description: Option<String>,
        /// Optional emoji or icon identifier
        #[arg(long)]
        icon: Option<String>,
    },
    /// Change a workspace's name, description or icon
    ///
    /// Every field is a patch: the ones you omit are left alone, not cleared.
    /// At least one is required — an update that changes nothing would print
    /// "updated" and mean it, which is worse than an error.
    ///
    /// This is the only way to fix a display name after `create`, and archiving
    /// is not an escape hatch: it is a soft delete, the ID stays taken, and an
    /// archived workspace is read-only. `unarchive` first if you need to edit
    /// one.
    Update {
        /// Workspace ID (the ID column of `workspace list`)
        id: String,
        /// New display name
        #[arg(long)]
        name: Option<String>,
        /// New description
        #[arg(long)]
        description: Option<String>,
        /// New emoji or icon identifier
        #[arg(long)]
        icon: Option<String>,
    },
    /// Archive a workspace
    ///
    /// A soft delete, and reversible — see `unarchive`. The ID stays taken
    /// either way: `create` will refuse it while it is archived, and say so.
    Archive {
        /// Workspace ID to archive (the ID column of `workspace list`)
        id: String,
    },
    /// Restore an archived workspace
    ///
    /// The inverse of `archive`. Find the ID with `workspace list
    /// --include-archived`; the restored workspace is printed in full, and it
    /// is writable again.
    ///
    /// This does not create anything: the ID was never released, and the
    /// workspace's memory and notes stayed on disk under it the whole time.
    Unarchive {
        /// Workspace ID to restore (the ID column of `workspace list --include-archived`)
        id: String,
    },
}

#[derive(Subcommand)]
pub enum UsersAction {
    /// List every principal this core knows
    List,
    /// Create a principal (the server generates their id)
    Create {
        /// Display name — what the roster picker and message bylines show
        display_name: String,
        /// `member` (default) or `admin`
        #[arg(long)]
        role: Option<String>,
    },
    /// Rename a principal, change their role, or deactivate them
    Update {
        /// The `u-…` id from `aleph users list`
        user_id: String,
        /// New display name
        #[arg(long = "name")]
        display_name: Option<String>,
        /// `member` or `admin` — applied to live connections without a reconnect
        #[arg(long)]
        role: Option<String>,
        /// `active` or `deactivated` — deactivating revokes every device they hold
        #[arg(long)]
        status: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum LogsAction {
    /// Get current log level
    Level,
    /// Set log level (trace, debug, info, warn, error)
    SetLevel {
        /// Log level to set
        level: String,
    },
    /// Show log directory path
    Dir,
}

#[derive(Subcommand)]
pub enum TraceAction {
    /// List recent persisted trace replays
    List {
        /// Maximum number of tasks to show
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
    },
    /// Show a structured replay for a persisted task
    Show {
        /// Replay task ID
        task_id: String,
    },
}

#[derive(Subcommand)]
pub enum IdentityAction {
    /// Show the live identity (SOUL.md) and identity-file status
    Get {
        /// Target agent id (defaults to the daemon's default agent)
        #[arg(long)]
        agent: Option<String>,
    },
    /// Write an identity file (SOUL.md by default) from inline content or a
    /// file. Takes effect on the next turn.
    Set {
        /// Markdown content to write (omit when using --file)
        content: Option<String>,
        /// Read content from this file instead of the inline argument
        #[arg(long, value_name = "PATH")]
        file: Option<String>,
        /// Which identity file to write (default: SOUL.md)
        #[arg(long = "file-name", value_name = "NAME")]
        file_name: Option<String>,
        /// Target agent id (defaults to the daemon's default agent)
        #[arg(long)]
        agent: Option<String>,
    },
    /// Remove a custom identity file (SOUL.md by default), reverting to default
    Clear {
        /// Which identity file to clear (default: SOUL.md)
        #[arg(long = "file-name", value_name = "NAME")]
        file_name: Option<String>,
        /// Target agent id (defaults to the daemon's default agent)
        #[arg(long)]
        agent: Option<String>,
    },
    /// List identity files (exists / size / path)
    List {
        /// Target agent id (defaults to the daemon's default agent)
        #[arg(long)]
        agent: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum SandboxAction {
    /// Print the active sandbox profile summary and exit
    Info,
    /// Run a command under the active sandbox profile
    Run {
        /// Network policy: `none`, `all`, or omit for the default
        #[arg(long, value_name = "POLICY")]
        network: Option<String>,
        /// Extra paths to grant write access to (repeatable)
        #[arg(long = "fs-write", value_name = "PATH")]
        fs_write: Vec<String>,
        /// Extra paths to grant read access to (repeatable)
        #[arg(long = "fs-read", value_name = "PATH")]
        fs_read: Vec<String>,
        /// On macOS, stream sandbox denials from `log stream` while the command runs
        #[arg(long)]
        log_denials: bool,
        /// Command and arguments to execute. Use `--` to separate from flags.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum McpAction {
    /// List pending tool approval requests
    Pending,
    /// Approve a tool execution request
    Approve {
        /// Request ID
        request_id: String,
        /// Reason for approval
        #[arg(long)]
        reason: Option<String>,
    },
    /// Reject a tool execution request
    Reject {
        /// Request ID
        request_id: String,
        /// Reason for rejection
        #[arg(long)]
        reason: Option<String>,
    },
    /// Cancel a pending approval
    Cancel {
        /// Request ID
        request_id: String,
    },
}

#[derive(Subcommand)]
pub enum ChatControlAction {
    /// Send a message (non-interactive)
    Send {
        /// The message to send
        message: String,
        /// Session key
        #[arg(short, long)]
        session: Option<String>,
        /// Follow the run live (tool activity + streamed response) instead
        /// of returning after dispatch
        #[arg(long)]
        stream: bool,
        /// Thinking level (none, concise, verbose)
        #[arg(long)]
        thinking: Option<String>,
    },
    /// Abort a running chat
    Abort {
        /// Run ID to abort
        run_id: String,
        /// Also abandon everything this session still has queued behind the
        /// run. Without it the cancel frees the session slot and the gateway's
        /// wait lane starts firing the backlog, one full agent turn at a time.
        #[arg(long)]
        session_key: Option<String>,
    },
    /// Show chat history
    History {
        /// Session key
        session_key: String,
        /// Maximum messages to return
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Clear chat history
    Clear {
        /// Session key
        session_key: String,
        /// Keep system messages
        #[arg(long)]
        keep_system: bool,
    },
}

#[derive(Subcommand)]
pub enum WebhookAction {
    /// List configured webhook endpoints (currently TOML-driven)
    List,
}

#[derive(Subcommand)]
pub enum ProxyAction {
    /// Show effective proxy configuration (backend not yet implemented)
    Show,
}

#[derive(Subcommand)]
pub enum HooksAction {
    /// Show the current contents of ~/.aleph/hooks.json
    List,
    /// Add a hook entry
    Add {
        /// Event name (`snake_case` or `PascalCase`: `before_tool_call`, `PreToolUse`, ...)
        event: String,
        /// Shell command to run
        #[arg(long, conflicts_with_all = ["prompt", "agent", "url"])]
        command: Option<String>,
        /// Inline prompt to inject
        #[arg(long, conflicts_with_all = ["command", "agent", "url"])]
        prompt: Option<String>,
        /// Named subagent to invoke
        #[arg(long, conflicts_with_all = ["command", "prompt", "url"])]
        agent: Option<String>,
        /// HTTP endpoint to POST the event payload to
        #[arg(long, conflicts_with_all = ["command", "prompt", "agent"])]
        url: Option<String>,
        /// Regex matched against `tool_name` for tool-related events
        #[arg(long)]
        matcher: Option<String>,
        /// Per-hook timeout in seconds (applies to command/http actions)
        #[arg(long)]
        timeout_secs: Option<u64>,
    },
    /// Remove hook entries by substring or index
    Remove {
        /// Event name
        event: String,
        /// Substring filter — drops every group whose action contains this string
        #[arg(long, conflicts_with = "index")]
        command: Option<String>,
        /// Zero-based group index to drop
        #[arg(long, conflicts_with = "command")]
        index: Option<usize>,
    },
    /// Force the extension manager to re-read hooks from disk
    Reload,
    /// List valid hook event names
    Events,
}

#[derive(Subcommand)]
pub enum SecretAction {
    /// List secret names (values are never returned)
    List,
    /// Store a secret. If --value is omitted, reads from a TTY prompt
    /// (or stdin in --json mode).
    Set {
        /// Secret name (alphanumerics + `_-.:`)
        name: String,
        /// Inline value (prefer the interactive prompt to avoid shell history)
        #[arg(long)]
        value: Option<String>,
    },
    /// Delete a secret by name
    Delete {
        /// Secret name
        name: String,
    },
    /// Confirm a secret exists and report its size in bytes
    Verify {
        /// Secret name
        name: String,
    },
    /// List configured secret provider backends
    Providers,
}
