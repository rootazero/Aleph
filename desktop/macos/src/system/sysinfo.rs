//! System info via NSProcessInfo.

use aleph_desktop::system_types::SystemInfo;
use aleph_desktop::Result;
use objc2_foundation::NSProcessInfo;

/// Get system information using NSProcessInfo.
pub fn system_info() -> Result<SystemInfo> {
    let info = NSProcessInfo::processInfo();

    let version = info.operatingSystemVersion();
    let os_version = format!(
        "{}.{}.{}",
        version.majorVersion, version.minorVersion, version.patchVersion
    );

    let hostname = info.hostName().to_string();

    // NSProcessInfo does not expose userName; use env vars
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".into());

    let arch = std::env::consts::ARCH.to_string();

    Ok(SystemInfo {
        os_name: "macOS".to_string(),
        os_version,
        hostname,
        arch,
        username,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_info() {
        let info = system_info().unwrap();
        assert_eq!(info.os_name, "macOS");
        assert!(!info.os_version.is_empty());
        assert!(!info.hostname.is_empty());
        assert!(!info.username.is_empty());
        assert!(!info.arch.is_empty());
    }
}
