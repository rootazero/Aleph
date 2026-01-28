//! Aether Gateway - WebSocket Control Plane
//!
//! A standalone WebSocket server that provides a JSON-RPC 2.0 interface
//! for controlling Aether agents and receiving events.
//!
//! # Usage
//!
//! ```bash
//! # Run with default settings (127.0.0.1:18789)
//! cargo run --features gateway --bin aether-gateway
//!
//! # Specify custom bind address and port
//! cargo run --features gateway --bin aether-gateway -- --bind 0.0.0.0 --port 9000
//!
//! # Load configuration from file
//! cargo run --features gateway --bin aether-gateway -- --config ~/.aether/gateway.toml
//!
//! # Run as daemon (background process)
//! cargo run --features gateway --bin aether-gateway -- --daemon
//!
//! # Stop a running daemon
//! cargo run --features gateway --bin aether-gateway -- stop
//!
//! # Check gateway status
//! cargo run --features gateway --bin aether-gateway -- status
//! ```
//!
//! # Testing
//!
//! Use `websocat` or any WebSocket client to connect:
//!
//! ```bash
//! # Health check
//! echo '{"jsonrpc":"2.0","method":"health","id":1}' | websocat ws://127.0.0.1:18789
//!
//! # Echo test
//! echo '{"jsonrpc":"2.0","method":"echo","params":{"hello":"world"},"id":2}' | websocat ws://127.0.0.1:18789
//!
//! # Version info
//! echo '{"jsonrpc":"2.0","method":"version","id":3}' | websocat ws://127.0.0.1:18789
//! ```

use std::net::SocketAddr;
use std::path::PathBuf;
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(feature = "gateway")]
use aethecore::gateway::GatewayServer;
#[cfg(feature = "gateway")]
use aethecore::gateway::event_bus::GatewayEventBus;
#[cfg(feature = "gateway")]
use aethecore::gateway::router::AgentRouter;
#[cfg(feature = "gateway")]
use aethecore::gateway::handlers::agent::{AgentRunManager, handle_run};
#[cfg(feature = "gateway")]
use std::sync::Arc;

/// Default PID file location
const DEFAULT_PID_FILE: &str = "~/.aether/gateway.pid";
/// Default log file location for daemon mode
const DEFAULT_LOG_FILE: &str = "~/.aether/gateway.log";

/// Aether Gateway - WebSocket control plane for AI agents
#[derive(Parser, Debug)]
#[command(name = "aether-gateway")]
#[command(version, about, long_about = None)]
struct Args {
    /// Subcommand (start, stop, status)
    #[command(subcommand)]
    command: Option<Command>,

    /// Configuration file path (TOML)
    #[arg(long, short = 'c')]
    config: Option<PathBuf>,

    /// Run as daemon (background process)
    #[arg(long, short = 'd')]
    daemon: bool,

    /// PID file path (for daemon mode)
    #[arg(long, default_value = DEFAULT_PID_FILE)]
    pid_file: String,

    /// Log file path (for daemon mode)
    #[arg(long)]
    log_file: Option<PathBuf>,

    /// Bind address
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,

    /// Port number
    #[arg(long, default_value = "18789")]
    port: u16,

    /// Force start even if port appears to be in use
    #[arg(long)]
    force: bool,

    /// Log level (error, warn, info, debug, trace)
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Maximum number of concurrent connections
    #[arg(long, default_value = "1000")]
    max_connections: usize,
}

/// Gateway subcommands
#[derive(Subcommand, Debug)]
enum Command {
    /// Start the gateway (default)
    Start,
    /// Stop a running daemon
    Stop,
    /// Check gateway status
    Status,
}

/// Expand ~ to home directory
fn expand_path(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

/// Check if a process with given PID is running
#[cfg(unix)]
fn is_process_running(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(not(unix))]
fn is_process_running(_pid: i32) -> bool {
    false
}

/// Read PID from file
fn read_pid_file(pid_file: &str) -> Option<i32> {
    let path = expand_path(pid_file);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Write PID to file
fn write_pid_file(pid_file: &str) -> std::io::Result<()> {
    let path = expand_path(pid_file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, format!("{}", std::process::id()))
}

/// Remove PID file
fn remove_pid_file(pid_file: &str) {
    let path = expand_path(pid_file);
    let _ = std::fs::remove_file(&path);
}

/// Handle stop command
fn handle_stop(pid_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(pid) = read_pid_file(pid_file) {
        if is_process_running(pid) {
            #[cfg(unix)]
            {
                println!("Sending SIGTERM to gateway process (PID {})", pid);
                unsafe { libc::kill(pid, libc::SIGTERM) };

                // Wait for process to exit (max 5 seconds)
                for _ in 0..50 {
                    if !is_process_running(pid) {
                        println!("Gateway stopped successfully");
                        remove_pid_file(pid_file);
                        return Ok(());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }

                println!("Gateway did not stop gracefully, sending SIGKILL");
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }

            #[cfg(not(unix))]
            {
                eprintln!("Daemon mode is only supported on Unix systems");
                return Err("Unsupported platform".into());
            }
        } else {
            println!("Gateway is not running (stale PID file)");
            remove_pid_file(pid_file);
        }
    } else {
        println!("No gateway daemon is running (no PID file found)");
    }
    Ok(())
}

/// Handle status command
fn handle_status(pid_file: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(pid) = read_pid_file(pid_file) {
        if is_process_running(pid) {
            println!("Gateway is running (PID {})", pid);
        } else {
            println!("Gateway is not running (stale PID file for PID {})", pid);
        }
    } else {
        println!("Gateway is not running (no PID file)");
    }
    Ok(())
}

/// Daemonize the current process (Unix only)
#[cfg(unix)]
fn daemonize(pid_file: &str, log_file: Option<&PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    use std::fs::OpenOptions;

    // Check if already running
    if let Some(pid) = read_pid_file(pid_file) {
        if is_process_running(pid) {
            return Err(format!("Gateway already running (PID {})", pid).into());
        }
    }

    // Fork the process
    match unsafe { libc::fork() } {
        -1 => return Err("Fork failed".into()),
        0 => {
            // Child process - continue
        }
        _ => {
            // Parent process - exit
            std::process::exit(0);
        }
    }

    // Create new session
    if unsafe { libc::setsid() } == -1 {
        return Err("setsid failed".into());
    }

    // Fork again to prevent terminal reattachment
    match unsafe { libc::fork() } {
        -1 => return Err("Second fork failed".into()),
        0 => {
            // Child continues
        }
        _ => {
            std::process::exit(0);
        }
    }

    // Redirect stdout/stderr to log file if specified
    if let Some(log_path) = log_file {
        let log_path = expand_path(&log_path.to_string_lossy());
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        use std::os::unix::io::AsRawFd;
        let fd = log_file.as_raw_fd();

        unsafe {
            libc::dup2(fd, libc::STDOUT_FILENO);
            libc::dup2(fd, libc::STDERR_FILENO);
        }
    } else {
        // Redirect to /dev/null by default
        use std::os::unix::io::AsRawFd;
        let dev_null = std::fs::File::open("/dev/null")?;
        let fd = dev_null.as_raw_fd();

        unsafe {
            libc::dup2(fd, libc::STDOUT_FILENO);
            libc::dup2(fd, libc::STDERR_FILENO);
        }
    }

    // Write PID file
    write_pid_file(pid_file)?;

    Ok(())
}

#[cfg(not(unix))]
fn daemonize(_pid_file: &str, _log_file: Option<&PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    Err("Daemon mode is only supported on Unix systems".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Handle subcommands
    match args.command {
        Some(Command::Stop) => {
            return handle_stop(&args.pid_file);
        }
        Some(Command::Status) => {
            return handle_status(&args.pid_file);
        }
        Some(Command::Start) | None => {
            // Continue with start logic
        }
    }

    // Handle daemon mode
    if args.daemon {
        let log_file = args.log_file.clone().or_else(|| {
            Some(PathBuf::from(DEFAULT_LOG_FILE))
        });
        daemonize(&args.pid_file, log_file.as_ref())?;
    }

    // Initialize tracing
    let filter = format!("aether_gateway={},aethecore::gateway={}", args.log_level, args.log_level);
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_file(false)
            .with_line_number(false))
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&filter)))
        .init();

    let addr: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .map_err(|e| format!("Invalid address: {}", e))?;

    // Check if port is available (unless --force is specified)
    if !args.force {
        if let Err(e) = std::net::TcpListener::bind(addr) {
            eprintln!("Error: Cannot bind to {}: {}", addr, e);
            eprintln!("Hint: Use --force to attempt to start anyway, or choose a different port with --port");
            std::process::exit(1);
        }
    }

    #[cfg(feature = "gateway")]
    {
        use aethecore::gateway::server::GatewayConfig;

        if !args.daemon {
            println!("╔═══════════════════════════════════════════════╗");
            println!("║         Aether Gateway v{}           ║", env!("CARGO_PKG_VERSION"));
            println!("╠═══════════════════════════════════════════════╣");
            println!("║  WebSocket: ws://{}          ║", addr);
            println!("║  Protocol:  JSON-RPC 2.0                      ║");
            println!("╚═══════════════════════════════════════════════╝");
            println!();
            println!("Available methods:");
            println!("  - health    : Check server health status");
            println!("  - echo      : Echo back parameters (testing)");
            println!("  - version   : Get server version info");
            println!("  - agent.run : Execute agent request with streaming");
            println!();
        }

        let config = GatewayConfig {
            max_connections: args.max_connections,
            require_auth: false,
            timeout_secs: 300,
        };

        let mut server = GatewayServer::with_config(addr, config);

        // Set up agent.run handler with dependencies
        let event_bus = server.event_bus().clone();
        let router = Arc::new(AgentRouter::new());
        let run_manager = Arc::new(AgentRunManager::new(router.clone(), event_bus));

        // Register agent.run handler
        let run_manager_clone = run_manager.clone();
        server.handlers_mut().register("agent.run", move |req| {
            let manager = run_manager_clone.clone();
            async move { handle_run(req, manager).await }
        });

        // Set up graceful shutdown
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let pid_file = args.pid_file.clone();

        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            if !args.daemon {
                println!("\nShutting down gateway...");
            }
            remove_pid_file(&pid_file);
            let _ = shutdown_tx.send(());
        });

        // Also handle SIGTERM for daemon mode
        #[cfg(unix)]
        {
            let pid_file_term = args.pid_file.clone();
            tokio::spawn(async move {
                use tokio::signal::unix::{signal, SignalKind};
                if let Ok(mut stream) = signal(SignalKind::terminate()) {
                    stream.recv().await;
                    remove_pid_file(&pid_file_term);
                    std::process::exit(0);
                }
            });
        }

        server.run_until_shutdown(shutdown_rx).await?;
    }

    #[cfg(not(feature = "gateway"))]
    {
        eprintln!("Error: Gateway feature is not enabled.");
        eprintln!("Rebuild with: cargo build --features gateway");
        std::process::exit(1);
    }

    Ok(())
}
