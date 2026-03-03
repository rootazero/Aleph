//! Device management command handlers

/// Handle devices list command
pub fn handle_devices_list() -> Result<(), Box<dyn std::error::Error>> {
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
        println!("{:<36} {:<20} {:<10} {:<20}", "DEVICE ID", "NAME", "TYPE", "APPROVED AT");
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
    use alephcore::gateway::device_store::DeviceStore;

    let store_path = alephcore::utils::paths::get_devices_db_path()
        .map_err(|e| format!("Failed to get device store path: {}", e))?;
    if !store_path.exists() {
        eprintln!("Error: No device store found");
        std::process::exit(1);
    }

    let device_store = DeviceStore::open(&store_path)?;

    if device_store.revoke_device(device_id)? {
        println!("Device revoked: {}", device_id);
    } else {
        eprintln!("Error: Device not found: {}", device_id);
        std::process::exit(1);
    }
    Ok(())
}
