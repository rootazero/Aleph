//! Pairing command handlers

use std::path::PathBuf;

/// Handle pairing list command
#[cfg(feature = "gateway")]
pub async fn handle_pairing_list() -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::gateway::security::{PairingManager, PairingRequest, SecurityStore};
    use std::sync::Arc;

    let store_path = alephcore::utils::paths::get_security_db_path()
        .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_security.db"));
    let store = Arc::new(SecurityStore::open(&store_path)?);
    let manager = PairingManager::new(store);
    let pending = manager.list_pending()?;

    if pending.is_empty() {
        println!("No pending pairing requests");
    } else {
        println!("Pending pairing requests:");
        println!("{:<10} {:<8} {:<30} {:<10}", "TYPE", "CODE", "NAME/CHANNEL", "EXPIRES IN");
        println!("{}", "-".repeat(60));
        for req in pending {
            let remaining = req.remaining_secs();
            match &req {
                PairingRequest::Device { code, device_name, .. } => {
                    println!("{:<10} {:<8} {:<30} {}s", "device", code, device_name, remaining);
                }
                PairingRequest::Channel { code, channel, sender_id, .. } => {
                    println!("{:<10} {:<8} {:<30} {}s", "channel", code, format!("{}:{}", channel, sender_id), remaining);
                }
            }
        }
    }
    Ok(())
}

/// Handle pairing approve command
#[cfg(feature = "gateway")]
pub async fn handle_pairing_approve(code: &str) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::gateway::security::{DeviceRole, DeviceType, PairingManager, PairingRequest, SecurityStore, TokenManager};
    use alephcore::gateway::device_store::{DeviceStore, ApprovedDevice};
    use std::sync::Arc;

    // Get device store and security store paths
    let store_path = alephcore::utils::paths::get_devices_db_path()
        .map_err(|e| format!("Failed to get device store path: {}", e))?;
    let security_store_path = alephcore::utils::paths::get_security_db_path()
        .map_err(|e| format!("Failed to get security store path: {}", e))?;
    let security_store = Arc::new(SecurityStore::open(&security_store_path)?);

    let pairing_manager = PairingManager::new(security_store.clone());

    // Confirm pairing - this returns the full pairing request
    let pairing_request = match pairing_manager.confirm_pairing(code) {
        Ok(req) => req,
        Err(e) => {
            eprintln!("Error: Invalid or expired pairing code: {} ({})", code, e);
            std::process::exit(1);
        }
    };

    // Extract info based on pairing type
    let (device_name, device_type): (String, Option<String>) = match &pairing_request {
        PairingRequest::Device { device_name, device_type, .. } => {
            (device_name.clone(), device_type.map(|t: DeviceType| t.as_str().to_string()))
        }
        PairingRequest::Channel { channel, sender_id, .. } => {
            // Channel pairing - approve the sender
            security_store.approve_sender(channel, sender_id)?;
            println!("Channel sender approved successfully!");
            println!("  Channel:   {}", channel);
            println!("  Sender ID: {}", sender_id);
            return Ok(());
        }
    };

    // Create device store and approve device
    let device_store = DeviceStore::open(&store_path)?;

    let device_id = uuid::Uuid::new_v4().to_string();
    let device = ApprovedDevice::new(
        device_id.clone(),
        device_name.clone(),
        device_type,
    );

    device_store.approve_device(&device)?;

    // Register device in security store for token generation
    security_store.upsert_device(&alephcore::gateway::security::store::DeviceUpsertData {
        device_id: &device_id,
        device_name: &device_name,
        device_type: None,
        public_key: &[0u8; 32], // placeholder public key
        fingerprint: &device_id[..16], // use device_id prefix as fingerprint
        role: "operator",
        scopes: &["*".to_string()],
    })?;

    // Generate token
    let token_manager = TokenManager::new(security_store);
    let signed_token = token_manager
        .issue_token(&device_id, DeviceRole::Operator, vec!["*".to_string()])
        .map_err(|e| format!("Failed to issue token: {}", e))?;

    let token = format!("{}:{}", signed_token.token, signed_token.signature);

    println!("Device approved successfully!");
    println!("  Device ID:   {}", device_id);
    println!("  Device Name: {}", device_name);
    println!("  Token:       {}", token);
    println!();
    println!("The device can now connect using this token.");

    Ok(())
}

/// Handle pairing reject command
#[cfg(feature = "gateway")]
pub async fn handle_pairing_reject(code: &str) -> Result<(), Box<dyn std::error::Error>> {
    use alephcore::gateway::security::{PairingManager, SecurityStore};
    use std::sync::Arc;

    let store_path = alephcore::utils::paths::get_security_db_path()
        .unwrap_or_else(|_| PathBuf::from("/tmp/aleph_security.db"));
    let store = Arc::new(SecurityStore::open(&store_path)?);
    let manager = PairingManager::new(store);

    match manager.cancel_pairing(code) {
        Ok(true) => println!("Pairing request rejected: {}", code),
        Ok(false) | Err(_) => {
            eprintln!("Error: Invalid or expired pairing code: {}", code);
            std::process::exit(1);
        }
    }
    Ok(())
}
