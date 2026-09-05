//! CLI argument definitions for Aleph Gateway
//!
//! This module contains all Clap-based command line argument parsing structures.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Default PID file location
pub const DEFAULT_PID_FILE: &str = "~/.aleph/gateway.pid";
/// Default log file location for daemon mode
pub const DEFAULT_LOG_FILE: &str = "~/.aleph/logs/gateway.log";

/// Aleph Gateway - WebSocket control plane for AI agents
#[derive(Parser, Debug)]
#[command(name = "aleph")]
#[command(version = env!("ALEPH_VERSION"), about, long_about = None)]
pub struct Args {
    /// Subcommand (start, stop, status)
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Configuration file path (TOML)
    #[arg(long, short = 'c')]
    pub config: Option<PathBuf>,

    /// Run as daemon (background process)
    #[arg(long, short = 'd')]
    pub daemon: bool,

    /// PID file path (for daemon mode)
    #[arg(long, default_value = DEFAULT_PID_FILE)]
    pub pid_file: String,

    /// Log file path (for daemon mode)
    #[arg(long)]
    pub log_file: Option<PathBuf>,

    /// Bind address
    #[arg(long)]
    pub bind: Option<String>,

    /// Port number
    #[arg(long)]
    pub port: Option<u16>,

    /// Force start even if port appears to be in use
    #[arg(long)]
    pub force: bool,

    /// Log level (error, warn, info, debug, trace)
    #[arg(long, default_value = "info")]
    pub log_level: String,

    /// Maximum number of concurrent connections
    #[arg(long)]
    pub max_connections: Option<usize>,

    /// `WebChat` UI directory (serves static files)
    #[arg(long)]
    pub webchat_dir: Option<PathBuf>,

    /// `WebChat` HTTP port (default: same as WebSocket port)
    #[arg(long)]
    pub webchat_port: Option<u16>,
}

/// Gateway subcommands
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start the gateway (default)
    Start,
    /// Stop a running daemon
    Stop,
    /// Check gateway status
    Status {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Self-diagnose runtime health and optionally apply mechanical repairs.
    ///
    /// Inspects the data directory, instance lock, config file, and
    /// shell-hook consent registry. Read-only by default; `--fix` applies
    /// deterministic repairs (recreate data dir, clear a stale lock).
    Doctor {
        /// Apply mechanical repairs for repairable findings.
        #[arg(long)]
        fix: bool,
        /// Emit a machine-readable JSON envelope instead of human output.
        #[arg(long)]
        json: bool,
        /// Run only these check ids (comma-separated, e.g. core/data-dir,core/vault).
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Skip these check ids (comma-separated). Ignored when --only is given.
        #[arg(long, value_delimiter = ',')]
        skip: Vec<String>,
    },
    /// Inspect the assembled system-prompt size, broken down per layer.
    ///
    /// Offline diagnostic (no network, no daemon). Assembles the prompt the way
    /// the main loop does — `ResolvedContext` for the chosen paradigm, resolved
    /// provider-behavior name, iteration cap — so the report matches what the
    /// model actually receives. Per-session content (identity files, curated
    /// memory, skills, MCP instructions, tool schemas) is still excluded: this
    /// measures the fixed scaffold, which is the budget-tuning target. Silent
    /// layers are listed by name so "why isn't my layer here?" is answerable.
    /// hermes `prompt-size` analogue.
    PromptSize {
        /// Assembly path: basic (sub-agent inline) | cached (main loop).
        #[arg(long, default_value = "cached")]
        path: String,
        /// Prompt mode: full | compact | minimal.
        #[arg(long, default_value = "full")]
        mode: String,
        /// Interaction paradigm driving the `ResolvedContext`:
        /// background | cli | messaging | webrich | embedded.
        #[arg(long, default_value = "background")]
        paradigm: String,
        /// Assemble with a bare `PromptConfig` and no `ResolvedContext` — the
        /// pre-2026-07-26 reporting shape. Useful only for comparing against
        /// old numbers; it under-reports the live prompt.
        #[arg(long)]
        bare: bool,
        /// Emit a machine-readable JSON envelope instead of human output.
        #[arg(long)]
        json: bool,
    },
    /// Re-trigger the interrupted run in a session.
    ///
    /// The boot scan resumes interrupted runs once, when the daemon starts. A
    /// run interrupted while the daemon kept running — or one the boot scan
    /// skipped after a transient store error — has no second trigger; this is
    /// it. Requires a running server (resuming means re-entering the harness,
    /// which only the server can do), and works even when
    /// `[resume] enabled = false`: that switch governs the automatic scan, not
    /// an explicit request.
    Resume {
        /// Session key to resume, e.g. `gui:chat` or `telegram:12345`.
        session_key: String,
        /// Emit a machine-readable JSON envelope instead of human output.
        #[arg(long)]
        json: bool,
    },
    /// Manage plugins
    Plugins {
        #[command(subcommand)]
        action: PluginsAction,
    },
    /// Gateway RPC tools
    Gateway {
        #[command(subcommand)]
        action: GatewayAction,
    },
    /// Manage start-on-boot for the standalone server (launchd / systemd / Task Scheduler)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
    /// Manage shell-hook consent (review/approve plugin shell hooks)
    Hooks {
        #[command(subcommand)]
        action: HooksAction,
    },
    /// Manage encrypted secrets
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },
    /// Read and verify the per-agent signed operation ledger (read-only;
    /// key lifecycle is the `agent_identity` tool)
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
    },
    /// Plugin management (CC-compatible)
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
    /// Bootstrap runtime dependencies (fnm, node, uv, playwright-cli, chromium, venv)
    BootstrapRuntime(BootstrapRuntimeArgs),
    /// Mint a one-time pairing ticket + LAN URL for a remote Panel (headless
    /// equivalent of Settings → Security → "Pair new device"). Prefer this over
    /// `bootstrap-token`: the ticket expires, is single-use, and is exchanged
    /// for a per-device token you can revoke on its own.
    Pair {
        /// Ticket lifetime in seconds (default 300, clamped to 60..=86400).
        #[arg(long, value_name = "SECONDS", conflicts_with_all = ["list", "revoke"])]
        ttl: Option<u64>,
        /// Bind the ticket to a principal from `aleph users list` (`u-…`).
        ///
        /// Omit it to pair your own device: an unbound ticket resolves to the
        /// owner. Pass it to invite someone else — that is what makes the
        /// redeeming device a distinct subject for every isolation predicate.
        #[arg(long = "user", value_name = "USER_ID", conflicts_with_all = ["list", "revoke"])]
        user: Option<String>,
        /// List the outstanding pairing tickets instead of minting one.
        ///
        /// Prints each ticket's non-secret id, expiry and binding — never the
        /// ticket code, which is the credential itself.
        #[arg(long, conflicts_with = "revoke")]
        list: bool,
        /// Burn one outstanding ticket by the id `--list` prints.
        ///
        /// An outstanding ticket is a live device credential: `token rotate`
        /// only reaches devices that already paired, and deactivating a user
        /// cannot touch an UNBOUND ticket because it belongs to nobody.
        #[arg(long = "revoke", value_name = "TICKET_ID")]
        revoke: Option<String>,
    },
    /// Print the shared Gateway token — the recovery / manual-entry credential.
    ///
    /// It never expires, it is also the secret vault's master key, and any
    /// device you paste it into keeps it. Never put it in a URL or QR code; use
    /// `aleph-server pair` to authorize a device.
    BootstrapToken,
    /// Check for a newer Aleph release and update the standalone server.
    ///
    /// Queries GitHub for the latest published release and compares it with
    /// the running version. Without `--check`, re-runs the official installer
    /// (install.sh / install.ps1) to download and install the correct binary
    /// for this platform. The desktop app self-updates separately (Tauri).
    Update {
        /// Only report whether an update is available; do not install.
        #[arg(long)]
        check: bool,
    },
    /// SP-2 internal: apply landlock + seccomp then exec target. Invoked
    /// by `BubblewrapDriver` inside the bwrap namespace; not for users.
    #[command(hide = true)]
    SandboxInit {
        /// Remaining argv passed through to `sandbox_init::run_init`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Smoke-test the active sandbox profile by running a command under it.
    /// Useful for diagnosing why a tool call failed under sandbox.
    ///
    /// Example: `aleph sandbox-debug --network none -- ls /tmp`
    SandboxDebug {
        /// Network policy: `none`, `all`, or omit for the default.
        #[arg(long, value_name = "POLICY")]
        network: Option<String>,
        /// Extra paths to grant write access to (repeatable).
        #[arg(long = "fs-write", value_name = "PATH")]
        fs_write: Vec<PathBuf>,
        /// Extra paths to grant read access to (repeatable).
        #[arg(long = "fs-read", value_name = "PATH")]
        fs_read: Vec<PathBuf>,
        /// Print active sandbox summary and exit (no command run).
        #[arg(long)]
        show_summary: bool,
        /// On macOS, stream sandbox denials from `log stream` for the
        /// duration of the command. Ignored on Linux/Windows.
        #[arg(long)]
        log_denials: bool,
        /// Command and arguments to execute under the sandbox. Use `--`
        /// to separate from sandbox-debug flags.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Run as a cluster node: dial out to a center, serve reverse-RPC tool.call,
    /// run bash in a LOCAL sandbox. Pure execution arm (no DB/LLM).
    Node {
        /// Center WebSocket base URL, e.g. <ws://127.0.0.1:18790>
        #[arg(long, value_name = "URL")]
        center: String,
        /// Human-readable node name shown in `environments.list`.
        #[arg(long, value_name = "NAME", default_value = "aleph-node")]
        name: String,
        /// Operator label for tag-based fan-out (repeatable), e.g.
        /// `--tag gpu --tag region=us`. Stored verbatim; shown in
        /// `environments.list` and used by `node_invoke_many`.
        #[arg(long = "tag", value_name = "TAG")]
        tags: Vec<String>,
    },
    /// SP-3a internal: apply restricted token + Low IL then spawn target
    /// via `CreateProcessAsUserW`. Invoked by `WindowsSandboxDriver`; not
    /// for users.
    #[command(hide = true)]
    SandboxInitWindows {
        /// Remaining argv passed through to `windows_init::run_init`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Plugins subcommands
#[derive(Subcommand, Debug)]
pub enum PluginsAction {
    /// List installed plugins
    List,
    /// Install a plugin from Git URL
    Install {
        /// Git URL of the plugin repository
        url: String,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin name
        name: String,
    },
    /// Enable a plugin
    Enable {
        /// Plugin name
        name: String,
    },
    /// Disable a plugin
    Disable {
        /// Plugin name
        name: String,
    },
}

/// Gateway subcommands
#[derive(Subcommand, Debug)]
pub enum GatewayAction {
    /// Call an RPC method on the Gateway
    Call {
        /// RPC method name (e.g., "health", "config.get")
        method: String,

        /// JSON parameters
        #[arg(long, short = 'p')]
        params: Option<String>,

        /// Gateway WebSocket URL
        #[arg(long, default_value = "ws://127.0.0.1:18790/ws")]
        url: String,

        /// Timeout in milliseconds
        #[arg(long, default_value = "30000")]
        timeout: u64,
    },
}

/// Service (start-on-boot) subcommands
#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Write the platform service descriptor, enable it, and start now
    Install,
    /// Stop and remove the service descriptor
    Uninstall,
    /// Arm the service to start on boot/login (no descriptor changes)
    Enable,
    /// Stop the service from starting on boot/login
    Disable,
    /// Report installed / enabled / running state
    Status,
}

/// Shell-hook consent subcommands.
///
/// Shell-command hooks shipped by plugins execute arbitrary code. The server
/// gates them behind the consent allowlist; these subcommands review and
/// manage that allowlist.
#[derive(Subcommand, Debug)]
pub enum HooksAction {
    /// List shell-command hooks and their consent status
    List,
    /// Run a hook's command to verify it, then optionally approve it
    Test {
        /// Hook fingerprint (a unique prefix is accepted)
        fingerprint: String,
    },
    /// Revoke consent for a hook (use `all` to revoke every approval)
    Revoke {
        /// Hook fingerprint (a unique prefix is accepted), or `all`
        fingerprint: String,
    },
    /// Diagnose the shell-hook consent registry
    Doctor,
}

/// Secret subcommands
#[derive(Subcommand, Debug)]
pub enum SecretAction {
    /// Initialize secret vault (uses token as master key)
    Init,
    /// Set a secret value (prompts when --value is omitted)
    Set {
        /// Secret name
        name: String,

        /// Secret value (avoid shell history by omitting and using prompt)
        #[arg(long)]
        value: Option<String>,
    },
    /// List stored secret names (never values)
    List,
    /// Delete a secret
    Delete {
        /// Secret name
        name: String,
    },
    /// Verify a secret exists
    Verify {
        /// Secret name
        name: String,
    },
    /// List configured secret providers and their status
    Providers,
}

/// Agent-identity subcommands.
///
/// Read-only by design: minting / rotating / revoking a key mutates state the
/// running daemon caches, so those live in the `agent_identity` tool. Reading
/// is what has to work when the daemon is stopped — or not trusted.
#[derive(Subcommand, Debug)]
pub enum IdentityAction {
    /// List agent identities: key fingerprint, chain length, revocation state
    List,
    /// Print recent ledger records, newest first
    Ledger {
        /// Only this agent (default: every agent)
        #[arg(long)]
        agent: Option<String>,
        /// Max records to print
        #[arg(long, default_value_t = 40)]
        limit: i64,
    },
    /// Recompute a chain and check every signature. Exits non-zero on a fault.
    ///
    /// With `--input` this needs no database, no vault and no Aleph
    /// installation at all — it checks an exported document on its own terms,
    /// which is the only form of the check an auditor who does not trust the
    /// host can run.
    Verify {
        /// Only this agent (default: verify every agent). Ignored with --input.
        #[arg(long)]
        agent: Option<String>,
        /// Verify an exported chain file instead of the local database.
        #[arg(long, value_name = "FILE")]
        input: Option<String>,
        /// Root fingerprint taken off-box, for use with --input. Repeatable.
        /// Without one, a clean verdict proves internal consistency only —
        /// whoever produced the document also chose the keys inside it.
        #[arg(long, value_name = "FINGERPRINT")]
        pin: Vec<String>,
        /// The head of the PREVIOUS export of this chain, as `<seq>:<hash>`
        /// (the `expect_head` value that export reported). The document must
        /// extend it. This is the only check that can catch a truncated tail:
        /// the anchor travels inside the file, so whoever produced the file
        /// edits it as freely as the rows.
        #[arg(long, value_name = "SEQ:HASH")]
        expect_head: Option<String>,
    },
    /// Write one agent's whole chain, its public keys and its anchor to a
    /// self-contained JSON document. Contains no private key and no tool
    /// arguments. Signed by the agent's active key when it has one.
    Export {
        /// Agent to export
        #[arg(long)]
        agent: String,
        /// Write here instead of stdout
        #[arg(long, value_name = "FILE")]
        out: Option<String>,
    },
    /// Check a file against its `.aleph-sig.json` signature envelope. Exits
    /// non-zero unless the verdict is `valid`.
    ///
    /// Read-only, and it needs only the public keys in security.db — never
    /// the vault: a revoked or retired key still verifies, a deleted one
    /// reports `unknown_signer`.
    VerifyArtifact {
        /// The artifact to check
        path: String,
        /// Read the envelope from here instead of `<path>.aleph-sig.json`
        #[arg(long, value_name = "FILE")]
        sig: Option<String>,
        /// Assert who must have signed
        #[arg(long)]
        agent: Option<String>,
    },
}

/// Plugin subcommands (unified, CC-compatible)
#[derive(Subcommand, Debug)]
pub enum PluginAction {
    /// List installed plugins
    List,
    /// Install a plugin from marketplace or URL
    Install {
        /// Plugin name or source URL
        source: String,
        /// Installation scope (user, project, local)
        #[arg(long, default_value = "user")]
        scope: String,
    },
    /// Update an installed plugin to the latest marketplace version
    Update {
        /// Plugin name (update all installed plugins if omitted)
        name: Option<String>,
        /// Re-install even if the version is unchanged
        #[arg(long)]
        force: bool,
        /// Installation scope (user, project, local)
        #[arg(long, default_value = "user")]
        scope: String,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin name
        name: String,
    },
    /// Enable a plugin
    Enable {
        /// Plugin name
        name: String,
    },
    /// Disable a plugin
    Disable {
        /// Plugin name
        name: String,
    },
    /// Marketplace management
    Marketplace {
        #[command(subcommand)]
        action: MarketplaceAction,
    },
}

/// `bootstrap-runtime` subcommand flags.
#[derive(clap::Args, Debug, Clone, Default)]
pub struct BootstrapRuntimeArgs {
    /// Install only the given capability (repeatable). Default set: uv, playwright-cli.
    #[arg(long)]
    pub only: Vec<String>,

    /// Skip the given capability (repeatable).
    #[arg(long)]
    pub skip: Vec<String>,

    /// Reinstall even if the ledger says Ready.
    #[arg(long)]
    pub force: bool,

    /// Exit 0 regardless of failures (install.sh / install.ps1 use this).
    #[arg(long)]
    pub best_effort: bool,

    /// Emit NDJSON progress events to stderr instead of pretty output.
    #[arg(long)]
    pub json: bool,

    /// Suppress per-step output; only errors.
    #[arg(long)]
    pub quiet: bool,
}

/// Marketplace subcommands
#[derive(Subcommand, Debug)]
pub enum MarketplaceAction {
    /// Add a marketplace source (GitHub owner/repo or local path)
    Add {
        /// Source: "owner/repo" for GitHub, path for local
        source: String,
    },
    /// List registered marketplaces
    List,
    /// Update marketplace cache
    Update {
        /// Marketplace name (update all if omitted)
        name: Option<String>,
    },
    /// Remove a marketplace
    Remove {
        /// Marketplace name
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_parses_plugins_list() {
        let args = Args::try_parse_from(["aleph", "plugins", "list"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                assert!(matches!(action, PluginsAction::List));
            }
            _ => panic!("Expected Plugins command with List action"),
        }
    }

    #[test]
    fn test_cli_parses_plugins_install() {
        let args = Args::try_parse_from([
            "aleph",
            "plugins",
            "install",
            "https://github.com/example/plugin.git",
        ]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                if let PluginsAction::Install { url } = action {
                    assert_eq!(url, "https://github.com/example/plugin.git");
                } else {
                    panic!("Expected PluginsAction::Install");
                }
            }
            _ => panic!("Expected Plugins command with Install action"),
        }
    }

    #[test]
    fn test_cli_parses_plugins_uninstall() {
        let args = Args::try_parse_from(["aleph", "plugins", "uninstall", "my-plugin"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                if let PluginsAction::Uninstall { name } = action {
                    assert_eq!(name, "my-plugin");
                } else {
                    panic!("Expected PluginsAction::Uninstall");
                }
            }
            _ => panic!("Expected Plugins command with Uninstall action"),
        }
    }

    #[test]
    fn test_cli_parses_plugins_enable() {
        let args = Args::try_parse_from(["aleph", "plugins", "enable", "my-plugin"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                if let PluginsAction::Enable { name } = action {
                    assert_eq!(name, "my-plugin");
                } else {
                    panic!("Expected PluginsAction::Enable");
                }
            }
            _ => panic!("Expected Plugins command with Enable action"),
        }
    }

    #[test]
    fn test_cli_parses_plugins_disable() {
        let args = Args::try_parse_from(["aleph", "plugins", "disable", "my-plugin"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Plugins { action }) => {
                if let PluginsAction::Disable { name } = action {
                    assert_eq!(name, "my-plugin");
                } else {
                    panic!("Expected PluginsAction::Disable");
                }
            }
            _ => panic!("Expected Plugins command with Disable action"),
        }
    }

    #[test]
    fn test_cli_parses_hooks_list() {
        let args = Args::try_parse_from(["aleph", "hooks", "list"]).unwrap();
        match args.command {
            Some(Command::Hooks { action }) => assert!(matches!(action, HooksAction::List)),
            _ => panic!("Expected Hooks command with List action"),
        }
    }

    #[test]
    fn test_cli_parses_hooks_test() {
        let args = Args::try_parse_from(["aleph", "hooks", "test", "a1b2c3d4"]).unwrap();
        match args.command {
            Some(Command::Hooks { action }) => {
                if let HooksAction::Test { fingerprint } = action {
                    assert_eq!(fingerprint, "a1b2c3d4");
                } else {
                    panic!("Expected HooksAction::Test");
                }
            }
            _ => panic!("Expected Hooks command with Test action"),
        }
    }

    #[test]
    fn test_cli_parses_hooks_revoke_all() {
        let args = Args::try_parse_from(["aleph", "hooks", "revoke", "all"]).unwrap();
        match args.command {
            Some(Command::Hooks { action }) => {
                if let HooksAction::Revoke { fingerprint } = action {
                    assert_eq!(fingerprint, "all");
                } else {
                    panic!("Expected HooksAction::Revoke");
                }
            }
            _ => panic!("Expected Hooks command with Revoke action"),
        }
    }

    #[test]
    fn test_cli_parses_hooks_doctor() {
        let args = Args::try_parse_from(["aleph", "hooks", "doctor"]).unwrap();
        match args.command {
            Some(Command::Hooks { action }) => assert!(matches!(action, HooksAction::Doctor)),
            _ => panic!("Expected Hooks command with Doctor action"),
        }
    }

    #[test]
    fn test_cli_parses_secret_set() {
        let args = Args::try_parse_from([
            "aleph",
            "secret",
            "set",
            "openai.main",
            "--value",
            "sk-ant-test",
        ]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Secret { action }) => {
                if let SecretAction::Set { name, value } = action {
                    assert_eq!(name, "openai.main");
                    assert_eq!(value.as_deref(), Some("sk-ant-test"));
                } else {
                    panic!("Expected SecretAction::Set");
                }
            }
            _ => panic!("Expected Secret command with Set action"),
        }
    }

    #[test]
    fn test_cli_parses_secret_verify() {
        let args = Args::try_parse_from(["aleph", "secret", "verify", "wallet.main"]);
        assert!(args.is_ok());
        let args = args.unwrap();
        match args.command {
            Some(Command::Secret { action }) => {
                if let SecretAction::Verify { name } = action {
                    assert_eq!(name, "wallet.main");
                } else {
                    panic!("Expected SecretAction::Verify");
                }
            }
            _ => panic!("Expected Secret command with Verify action"),
        }
    }

    #[test]
    fn test_cli_parses_node_subcommand() {
        let args = Args::try_parse_from([
            "aleph-server",
            "node",
            "--center",
            "ws://127.0.0.1:18790",
            "--name",
            "edge-1",
        ])
        .unwrap();
        match args.command {
            Some(Command::Node { center, name, .. }) => {
                assert_eq!(center, "ws://127.0.0.1:18790");
                assert_eq!(name, "edge-1");
            }
            _ => panic!("Expected Node command"),
        }
    }

    #[test]
    fn test_cli_node_name_defaults() {
        let args =
            Args::try_parse_from(["aleph-server", "node", "--center", "ws://127.0.0.1:18790"])
                .unwrap();
        match args.command {
            Some(Command::Node { name, .. }) => assert_eq!(name, "aleph-node"),
            _ => panic!("Expected Node command"),
        }
    }

    #[test]
    fn node_command_collects_repeated_tags() {
        let cli = Args::try_parse_from([
            "aleph-server",
            "node",
            "--center",
            "ws://127.0.0.1:18790",
            "--name",
            "gpu-1",
            "--tag",
            "gpu",
            "--tag",
            "region=us",
        ])
        .expect("parses");
        match cli.command {
            Some(Command::Node { tags, .. }) => {
                assert_eq!(tags, vec!["gpu".to_string(), "region=us".to_string()]);
            }
            _ => panic!("Expected Node command"),
        }
    }

    #[test]
    fn node_command_defaults_to_no_tags() {
        let cli =
            Args::try_parse_from(["aleph-server", "node", "--center", "ws://c"]).expect("parses");
        match cli.command {
            Some(Command::Node { tags, .. }) => assert!(tags.is_empty()),
            _ => panic!("Expected Node command"),
        }
    }

    #[test]
    fn test_cli_parses_update_check_flag() {
        let args = Args::try_parse_from(["aleph-server", "update", "--check"]).unwrap();
        match args.command {
            Some(Command::Update { check }) => assert!(check),
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_cli_update_defaults_to_install() {
        let args = Args::try_parse_from(["aleph-server", "update"]).unwrap();
        match args.command {
            Some(Command::Update { check }) => assert!(!check),
            _ => panic!("Expected Update command"),
        }
    }

    #[test]
    fn test_cli_parses_bootstrap_runtime_default() {
        let args = Args::try_parse_from(["aleph", "bootstrap-runtime"]).unwrap();
        match args.command {
            Some(Command::BootstrapRuntime(a)) => {
                assert!(a.only.is_empty());
                assert!(a.skip.is_empty());
                assert!(!a.force);
                assert!(!a.best_effort);
                assert!(!a.json);
                assert!(!a.quiet);
            }
            _ => panic!("Expected BootstrapRuntime variant"),
        }
    }

    #[test]
    fn test_cli_parses_bootstrap_runtime_all_flags() {
        let args = Args::try_parse_from([
            "aleph",
            "bootstrap-runtime",
            "--only",
            "uv",
            "--only",
            "playwright-cli",
            "--skip",
            "cargo",
            "--force",
            "--best-effort",
            "--json",
            "--quiet",
        ])
        .unwrap();
        match args.command {
            Some(Command::BootstrapRuntime(a)) => {
                assert_eq!(a.only, vec!["uv", "playwright-cli"]);
                assert_eq!(a.skip, vec!["cargo"]);
                assert!(a.force);
                assert!(a.best_effort);
                assert!(a.json);
                assert!(a.quiet);
            }
            _ => panic!("Expected BootstrapRuntime variant"),
        }
    }
}
