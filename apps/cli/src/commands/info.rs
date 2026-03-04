//! Server info command

use crate::client::AlephClient;
use crate::error::CliResult;
use crate::output;

/// Format bytes into a human-readable string (e.g., "4.2 GB")
fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1_073_741_824.0;
    const MB: f64 = 1_048_576.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else {
        format!("{:.0} MB", b / MB)
    }
}

/// Format seconds into a human-readable uptime string (e.g., "2d 3h 15m")
fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;

    if days > 0 {
        format!("{}d {}h {}m", days, hours, minutes)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else {
        format!("{}m", minutes)
    }
}

/// Run info command
pub async fn run(server_url: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url).await?;

    // Get health info
    let health: serde_json::Value = client.call("health", None::<()>).await?;

    // Get system info (graceful fallback)
    let system: serde_json::Value = client
        .call("system.info", None::<()>)
        .await
        .unwrap_or(serde_json::json!({}));

    // Get providers list (graceful fallback)
    let providers: serde_json::Value = client
        .call("providers.list", None::<()>)
        .await
        .unwrap_or(serde_json::json!({}));

    if json {
        let combined = serde_json::json!({
            "health": health,
            "system": system,
            "providers": providers,
        });
        output::print_json(&combined);
    } else {
        println!("Aleph Info");
        println!("\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\n");

        // Status section
        println!(
            "{:<14}{}",
            "Health:",
            health
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("connected")
        );

        if let Some(v) = system.get("version").and_then(|v| v.as_str()) {
            println!("{:<14}{}", "Version:", v);
        }

        if let Some(p) = system.get("platform").and_then(|v| v.as_str()) {
            println!("{:<14}{}", "Platform:", p);
        }

        // System section
        let has_system = system.get("cpu_usage_percent").is_some();
        if has_system {
            println!("\nSystem");
            println!("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");

            if let Some(cpu) = system.get("cpu_usage_percent").and_then(|v| v.as_f64()) {
                println!("{:<14}{:.1}%", "CPU:", cpu);
            }

            let mem_used = system.get("memory_used_bytes").and_then(|v| v.as_u64());
            let mem_total = system.get("memory_total_bytes").and_then(|v| v.as_u64());
            if let (Some(used), Some(total)) = (mem_used, mem_total) {
                println!("{:<14}{} / {}", "Memory:", format_bytes(used), format_bytes(total));
            }

            let disk_used = system.get("disk_used_bytes").and_then(|v| v.as_u64());
            let disk_total = system.get("disk_total_bytes").and_then(|v| v.as_u64());
            if let (Some(used), Some(total)) = (disk_used, disk_total) {
                println!("{:<14}{} / {}", "Disk:", format_bytes(used), format_bytes(total));
            }

            if let Some(uptime) = system.get("uptime_secs").and_then(|v| v.as_u64()) {
                println!("{:<14}{}", "Uptime:", format_uptime(uptime));
            }
        }

        // Providers section
        if let Some(list) = providers.get("providers").and_then(|v| v.as_array()) {
            if !list.is_empty() {
                println!("\nProviders");
                println!("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}");

                for provider in list {
                    if let Some(name) = provider.get("name").and_then(|v| v.as_str()) {
                        let enabled = provider
                            .get("enabled")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let status = if enabled { "+" } else { "-" };
                        println!("  {} {}", status, name);
                    }
                }
            }
        }
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0 MB");
        assert_eq!(format_bytes(512 * 1024 * 1024), "512 MB");
        assert_eq!(format_bytes(4_500_000_000), "4.2 GB");
        assert_eq!(format_bytes(16_000_000_000), "14.9 GB");
    }

    #[test]
    fn test_format_uptime() {
        assert_eq!(format_uptime(0), "0m");
        assert_eq!(format_uptime(300), "5m");
        assert_eq!(format_uptime(3700), "1h 1m");
        assert_eq!(format_uptime(90000), "1d 1h 0m");
        assert_eq!(format_uptime(183300), "2d 2h 55m");
    }
}
