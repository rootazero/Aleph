/// Example demonstrating PII scrubbing functionality
///
/// This example shows how to use the PII scrubbing utilities and logging layer.
///
/// Run with: cargo run --example pii_scrubbing_demo
use alephcore::logging::create_pii_scrubbing_layer;
use alephcore::utils::pii::scrub_pii;
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn main() {
    // Set up logging with PII scrubbing
    tracing_subscriber::registry()
        .with(create_pii_scrubbing_layer())
        .init();

    println!("=== PII Scrubbing Demo ===\n");

    // Example 1: Direct PII scrubbing
    println!("Example 1: Direct PII scrubbing");
    let sensitive_text = "My email is john@example.com and phone is 123-456-7890";
    let scrubbed = scrub_pii(sensitive_text);
    println!("Original: {}", sensitive_text);
    println!("Scrubbed: {}\n", scrubbed);

    // Example 2: API key scrubbing
    println!("Example 2: API key scrubbing");
    let api_text = "Using API key: sk-proj1234567890abcdefghijklmnopqrstuvwxyz";
    let scrubbed_api = scrub_pii(api_text);
    println!("Original: {}", api_text);
    println!("Scrubbed: {}\n", scrubbed_api);

    // Example 3: Logging with automatic PII scrubbing
    println!("Example 3: Logging with automatic PII scrubbing");
    println!("(Check the log output below - PII should be scrubbed)\n");

    info!("User input: Contact me at jane.doe@example.com");
    warn!("API key detected: sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890");
    error!(
        user_data = "SSN: 123-45-6789, Card: 1234-5678-9012-3456",
        "Sensitive data logged"
    );

    // Example 4: Multiple PII types
    println!("\nExample 4: Multiple PII types in one string");
    let complex_text =
        "Email: alice@test.org, Phone: (555) 123-4567, API: sk-test123456789012345678";
    let scrubbed_complex = scrub_pii(complex_text);
    println!("Original: {}", complex_text);
    println!("Scrubbed: {}\n", scrubbed_complex);

    // Example 5: No PII (should remain unchanged)
    println!("Example 5: Text without PII (should remain unchanged)");
    let clean_text = "This is a normal message with no sensitive information.";
    let scrubbed_clean = scrub_pii(clean_text);
    println!("Original: {}", clean_text);
    println!("Scrubbed: {}\n", scrubbed_clean);

    println!("=== Demo Complete ===");
}
