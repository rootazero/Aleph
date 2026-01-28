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
//! # Force start (useful for development)
//! cargo run --features gateway --bin aether-gateway -- --force
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
use clap::Parser;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[cfg(feature = "gateway")]
use aethecore::gateway::GatewayServer;

/// Aether Gateway - WebSocket control plane for AI agents
#[derive(Parser, Debug)]
#[command(name = "aether-gateway")]
#[command(version, about, long_about = None)]
struct Args {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

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
        println!();

        let config = GatewayConfig {
            max_connections: args.max_connections,
            require_auth: false,
            timeout_secs: 300,
        };

        let server = GatewayServer::with_config(addr, config);

        // Set up graceful shutdown
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            println!("\nShutting down gateway...");
            let _ = shutdown_tx.send(());
        });

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
