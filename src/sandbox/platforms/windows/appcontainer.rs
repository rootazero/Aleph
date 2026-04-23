use std::ffi::c_void;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::Foundation::HLOCAL;
use windows_sys::Win32::Security::CreateAppContainerProfile;
use windows_sys::Win32::Security::DeleteAppContainerProfile;
use windows_sys::Win32::Security::DeriveAppContainerSidFromAppContainerName;
use windows_sys::Win32::Security::GetLengthSid;
use windows_sys::Win32::Security::SID;
use windows_sys::Win32::Security::SECURITY_CAPABILITIES;
use windows_sys::Win32::System::Threading::PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES;
use windows_sys::Win32::System::Threading::UpdateProcThreadAttribute;

/// AppContainer capability for sandboxed processes.
#[derive(Debug, Clone)]
pub enum AppContainerCapability {
    /// Basic internet client access (outbound HTTP/HTTPS).
    InternetClient,
    /// Internet client + server (inbound + outbound).
    InternetClientServer,
    /// Private network access (LAN/VPN).
    PrivateNetworkClientServer,
    /// Custom capability string.
    Custom(String),
}

impl AppContainerCapability {
    /// Convert to Windows capability SID string.
    fn to_sid_string(&self) -> String {
        match self {
            AppContainerCapability::InternetClient => {
                "S-1-15-3-1".to_string()
            }
            AppContainerCapability::InternetClientServer => {
                "S-1-15-3-2".to_string()
            }
            AppContainerCapability::PrivateNetworkClientServer => {
                "S-1-15-3-3".to_string()
            }
            AppContainerCapability::Custom(s) => s.clone(),
        }
    }
}

/// Windows AppContainer sandbox isolation.
///
/// AppContainer provides stronger isolation than restricted tokens:
/// - Process runs with AppContainer SID
/// - File access limited to container-specific directories
/// - Network access controlled by capabilities
/// - Registry access isolated
///
/// Requires Windows 10+.
pub struct AppContainer {
    name: String,
    sid: Vec<u8>,
    capabilities: Vec<AppContainerCapability>,
}

impl AppContainer {
    /// Create a new AppContainer with the given name and capabilities.
    ///
    /// # Safety
    /// Caller must ensure the name is unique to avoid conflicts.
    pub unsafe fn new(
        name: &str,
        capabilities: Vec<AppContainerCapability>,
    ) -> Result<Self, String> {
        let name_wide: Vec<u16> = name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // Derive AppContainer SID from name
        let mut sid_ptr: *mut SID = std::ptr::null_mut();
        let result = DeriveAppContainerSidFromAppContainerName(
            name_wide.as_ptr(),
            &mut sid_ptr,
        );

        if result != ERROR_SUCCESS {
            return Err(format!(
                "DeriveAppContainerSidFromAppContainerName failed: {result}"
            ));
        }

        let sid_len = GetLengthSid(sid_ptr as *const c_void) as usize;
        let mut sid = vec![0u8; sid_len];
        std::ptr::copy_nonoverlapping(
            sid_ptr as *const u8,
            sid.as_mut_ptr(),
            sid_len,
        );

        LocalFree(sid_ptr as *mut c_void as HLOCAL);

        Ok(Self {
            name: name.to_string(),
            sid,
            capabilities,
        })
    }

    /// Create the AppContainer profile on the system.
    ///
    /// This registers the AppContainer with the system so processes
    /// can be launched inside it.
    ///
    /// # Safety
    /// Requires administrative privileges on some Windows versions.
    pub unsafe fn create_profile(&self,
    ) -> Result<(), String> {
        let name_wide: Vec<u16> = self
            .name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let display_name = self.name.clone();
        let display_name_wide: Vec<u16> = display_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let mut sid_ptr: *mut SID = std::ptr::null_mut();
        let result = CreateAppContainerProfile(
            name_wide.as_ptr(),
            display_name_wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
            &mut sid_ptr,
        );

        if result != ERROR_SUCCESS {
            return Err(format!(
                "CreateAppContainerProfile failed: {result}"
            ));
        }

        if !sid_ptr.is_null() {
            LocalFree(sid_ptr as *mut c_void as HLOCAL);
        }

        Ok(())
    }

    /// Delete the AppContainer profile from the system.
    ///
    /// # Safety
    /// Caller must ensure no processes are running in this container.
    pub unsafe fn delete_profile(&self,
    ) -> Result<(), String> {
        let name_wide: Vec<u16> = self
            .name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let result = DeleteAppContainerProfile(name_wide.as_ptr());

        if result != ERROR_SUCCESS {
            return Err(format!(
                "DeleteAppContainerProfile failed: {result}"
            ));
        }

        Ok(())
    }

    /// Get the AppContainer SID.
    pub fn sid(&self) -> &[u8] {
        &self.sid
    }

    /// Get the AppContainer name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the capabilities.
    pub fn capabilities(&self) -> &[AppContainerCapability] {
        &self.capabilities
    }

    /// Build SECURITY_CAPABILITIES structure for process creation.
    ///
    /// # Safety
    /// The returned structure contains pointers to internal data.
    /// It must not outlive the AppContainer.
    pub unsafe fn security_capabilities(
0026self) -> Result<SECURITY_CAPABILITIES, String> {
        // Convert capabilities to SID strings
        let capability_sids: Vec<Vec<u8>> = self
            .capabilities
            .iter()
            .map(|cap| {
                let sid_str = cap.to_sid_string();
                // Parse SID string to bytes
                // This is a simplified version; real implementation
                // would use ConvertStringSidToSidW
                sid_str.into_bytes()
            })
            .collect();

        // For now, return empty capabilities
        // Full implementation would allocate and populate
        // SID_AND_ATTRIBUTES array
        Ok(SECURITY_CAPABILITIES {
            AppContainerSid: self.sid.as_ptr() as *mut c_void,
            Capabilities: std::ptr::null_mut(),
            CapabilityCount: 0,
            Reserved: 0,
        })
    }
}

impl Drop for AppContainer {
    fn drop(&mut self) {
        // Note: We don't delete the profile here because
        // it may be reused across multiple process launches.
        // Profile cleanup should be done explicitly.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appcontainer_capability_sid_strings() {
        assert_eq!(
            AppContainerCapability::InternetClient.to_sid_string(),
            "S-1-15-3-1"
        );
        assert_eq!(
            AppContainerCapability::InternetClientServer.to_sid_string(),
            "S-1-15-3-2"
        );
        assert_eq!(
            AppContainerCapability::PrivateNetworkClientServer.to_sid_string(),
            "S-1-15-3-3"
        );
        assert_eq!(
            AppContainerCapability::Custom("test".to_string()).to_sid_string(),
            "test"
        );
    }

    // Note: AppContainer creation tests require Windows 10+
    // and are platform-specific. They are marked as ignored
    // and should be run on Windows only.
    #[test]
    #[ignore = "Windows-only test"]
    fn appcontainer_creation() {
        let container = unsafe {
            AppContainer::new(
                "Aleph.Test.Container",
                vec![AppContainerCapability::InternetClient],
            )
            .unwrap()
        };

        assert!(!container.sid().is_empty());
        assert_eq!(container.name(), "Aleph.Test.Container");
        assert_eq!(container.capabilities().len(), 1);
    }
}
