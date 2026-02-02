//! Device management command handlers

use crate::commands::pairing::get_device_store_path;

/// Handle devices list command
#[cfg(feature = "gateway")]
pub fn handle_devices_list() -> Result<(), Box<dyn std::error::Error>> {
    use aethecore::gateway::device_store::DeviceStore;

    let store_path = get_device_store_path();
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
            let approved_at = &device.approved_at[..19]; // Truncate to "2026-01-28T12:00:00"
            println!(
                "{:<36} {:<20} {:<10} {:<20}",
                device.device_id, device.device_name, device_type, approved_at
            );
        }
    }
    Ok(())
}

/// Handle devices revoke command
#[cfg(feature = "gateway")]
pub fn handle_devices_revoke(device_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    use aethecore::gateway::device_store::DeviceStore;

    let store_path = get_device_store_path();
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
