use crate::error::Result;
use std::path::PathBuf;

/// Helper for generating sandbox profiles
pub struct ProfileGenerator;

impl ProfileGenerator {
    /// Create temporary workspace directory
    pub fn create_temp_workspace() -> Result<PathBuf> {
        let temp_dir = tempfile::Builder::new()
            .prefix("aleph-sandbox-")
            .tempdir()?;
        let path = temp_dir.path().to_path_buf();
        // Keep the directory by converting to path (prevents auto-cleanup)
        let _ = temp_dir.into_path();
        Ok(path)
    }

    /// Write profile content to temporary file
    pub fn write_temp_profile(content: &str, extension: &str) -> Result<PathBuf> {
        use std::io::Write;
        let mut temp_file = tempfile::Builder::new()
            .prefix("aleph-profile-")
            .suffix(extension)
            .tempfile()?;
        temp_file.write_all(content.as_bytes())?;
        let temp_path = temp_file.into_temp_path();
        temp_path
            .keep()
            .map_err(|e| crate::error::AlephError::IoError(format!("Failed to persist temp file: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_temp_workspace() {
        let workspace = ProfileGenerator::create_temp_workspace().unwrap();
        assert!(workspace.exists());
        assert!(workspace.is_dir());
        std::fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn test_write_temp_profile() {
        let content = "(version 1)\n(deny default)\n";
        let profile_path = ProfileGenerator::write_temp_profile(content, ".sb").unwrap();
        assert!(profile_path.exists());
        let read_content = std::fs::read_to_string(&profile_path).unwrap();
        assert_eq!(read_content, content);
        std::fs::remove_file(profile_path).ok();
    }
}
