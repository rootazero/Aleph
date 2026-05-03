//! Device management command handlers
//!
//! Spec C policy:
//! - `devices list`: **NoLock** — read-only against `devices.db`. WAL
//!   mode (set by `open_sqlite_safe`, Task 7) makes concurrent reads
//!   alongside the running server safe.
//! - `devices revoke`: **LockOnly** — writes `devices.db` under
//!   `~/.aleph/data/`. No matching `/v1/admin/devices/*` IPC route was
//!   built (deferred to a future spec), so the CLI refuses cleanly
//!   when the server is running and tells the operator to stop the
//!   server first.

/// Handle devices list command
pub fn handle_devices_list() -> Result<(), Box<dyn std::error::Error>> {
    alephcore::cli::policy::run_no_lock(|| Ok::<(), anyhow::Error>(()))?;

    use alephcore::gateway::device_store::DeviceStore;

    let store_path = alephcore::utils::paths::get_devices_db_path()
        .map_err(|e| format!("Failed to get device store path: {}", e))?;
    if !store_path.exists() {
        println!("No approved devices");
        return Ok(());
    }

    let device_store = DeviceStore::open(&store_path)?;
    let devices = device_store.list_devices();

    if devices.is_empty() {
        println!("No approved devices");
    } else {
        println!("Approved devices:");
        println!(
            "{:<36} {:<20} {:<10} {:<20}",
            "DEVICE ID", "NAME", "TYPE", "APPROVED AT"
        );
        println!("{}", "-".repeat(90));
        for device in devices {
            let device_type = device.device_type.unwrap_or_else(|| "-".to_string());
            let approved_at = device.approved_at.get(..19).unwrap_or(&device.approved_at);
            println!(
                "{:<36} {:<20} {:<10} {:<20}",
                device.device_id, device.device_name, device_type, approved_at
            );
        }
    }
    Ok(())
}

/// Handle devices revoke command
pub fn handle_devices_revoke(device_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::cli::policy::{with_policy, CommandPolicy};

    let data_dir = alephcore::utils::paths::get_data_dir()
        .map_err(|e| format!("Failed to resolve data dir: {}", e))?;

    with_policy::<_, ()>(
        CommandPolicy::LockOnly,
        &data_dir,
        |_lock| revoke_locked(device_id),
        serde_json::Value::Null,
    )
    .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })
}

fn revoke_locked(device_id: &str) -> anyhow::Result<()> {
    use alephcore::gateway::device_store::DeviceStore;

    let store_path = alephcore::utils::paths::get_devices_db_path()
        .map_err(|e| anyhow::anyhow!("Failed to get device store path: {}", e))?;
    if !store_path.exists() {
        anyhow::bail!("No device store found");
    }

    let device_store = DeviceStore::open(&store_path)
        .map_err(|e| anyhow::anyhow!("Failed to open device store: {}", e))?;

    if device_store
        .revoke_device(device_id)
        .map_err(|e| anyhow::anyhow!("Failed to revoke device: {}", e))?
    {
        println!("Device revoked: {}", device_id);
        Ok(())
    } else {
        anyhow::bail!("Device not found: {}", device_id);
    }
}
