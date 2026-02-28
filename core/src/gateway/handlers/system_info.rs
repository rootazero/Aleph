//! System Info Handler
//!
//! Returns real system metrics: CPU, memory, disk, uptime, platform.

use serde_json::json;
use sysinfo::{Disks, System};

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

/// Handle system.info requests
///
/// Returns a JSON object with real system metrics.
pub async fn handle(request: JsonRpcRequest) -> JsonRpcResponse {
    // Spawn blocking because sysinfo does synchronous I/O
    let result = tokio::task::spawn_blocking(|| {
        let mut sys = System::new();

        // CPU requires two refreshes with a gap for accurate reading
        sys.refresh_cpu_all();
        std::thread::sleep(std::time::Duration::from_millis(200));
        sys.refresh_cpu_all();
        sys.refresh_memory();

        let cpu_usage = sys.global_cpu_usage();
        let cpu_count = sys.cpus().len();
        let memory_used = sys.used_memory();
        let memory_total = sys.total_memory();

        // Sum all disk usage
        let disks = Disks::new_with_refreshed_list();
        let mut disk_total: u64 = 0;
        let mut disk_used: u64 = 0;
        for disk in disks.list() {
            disk_total += disk.total_space();
            disk_used += disk.total_space() - disk.available_space();
        }

        let uptime = System::uptime();

        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            "uptime_secs": uptime,
            "cpu_usage_percent": cpu_usage,
            "cpu_count": cpu_count,
            "memory_used_bytes": memory_used,
            "memory_total_bytes": memory_total,
            "disk_used_bytes": disk_used,
            "disk_total_bytes": disk_total,
        })
    })
    .await;

    match result {
        Ok(info) => JsonRpcResponse::success(request.id, info),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to collect system info: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_system_info_response() {
        let request = JsonRpcRequest::with_id("system.info", None, json!(1));
        let response = handle(request).await;

        assert!(response.is_success());

        let result = response.result.unwrap();
        assert!(result["version"].is_string());
        assert!(result["platform"].is_string());
        assert!(result["uptime_secs"].is_u64());
        assert!(result["cpu_count"].is_u64());
        assert!(result["memory_total_bytes"].is_u64());
        assert!(result["memory_used_bytes"].is_u64());
        assert!(result["disk_total_bytes"].is_u64());
        assert!(result["disk_used_bytes"].is_u64());
        assert!(result["cpu_usage_percent"].is_number());
    }
}
