/// Example demonstrating file logging with daily rotation
///
/// This example shows how to use file-based logging with automatic
/// PII scrubbing and daily log rotation.
///
/// Run with: cargo run --example file_logging_demo
use alephcore::init_logging;
use alephcore::logging::get_log_directory;
use tracing::{debug, error, info, warn};

fn main() {
    // Initialize logging (console + file with PII scrubbing)
    init_logging();

    println!("=== File Logging Demo ===\n");

    // Show where logs are being written
    match get_log_directory() {
        Ok(log_dir) => {
            println!("Logs are being written to: {}\n", log_dir.display());
        }
        Err(e) => {
            eprintln!("Failed to get log directory: {}", e);
        }
    }

    // Example 1: Different log levels
    println!("Example 1: Different log levels");
    debug!("This is a debug message");
    info!("This is an info message");
    warn!("This is a warning message");
    error!("This is an error message");
    println!();

    // Example 2: Structured logging
    println!("Example 2: Structured logging with fields");
    info!(
        user_id = "user123",
        action = "login",
        "User logged in successfully"
    );
    info!(
        provider = "openai",
        model = "gpt-4",
        latency_ms = 450,
        "AI request completed"
    );
    println!();

    // Example 3: PII in logs (will be scrubbed)
    println!("Example 3: PII in logs (automatically scrubbed)");
    info!(user_email = "john@example.com", "User registered");
    warn!(
        api_key = "sk-proj1234567890abcdefghijklmnopqrstuvwxyz",
        "API key detected in request"
    );
    error!(
        sensitive_data = "SSN: 123-45-6789, Phone: (555) 123-4567",
        "Sensitive data in error"
    );
    println!();

    // Example 4: Long message
    println!("Example 4: Long message");
    info!(
        "This is a very long log message that demonstrates how the logging system handles longer text. \
         It includes multiple sentences and shows that formatting is preserved. \
         The message will be written to both console and file with PII scrubbing applied."
    );
    println!();

    println!("=== Demo Complete ===");
    println!("\nCheck the log file at the path shown above to see:");
    println!("  1. All messages from this demo");
    println!("  2. Timestamps for each message");
    println!("  3. PII automatically replaced with placeholders");
    println!("  4. Daily rotation (files named aleph-YYYY-MM-DD.log)");
}
