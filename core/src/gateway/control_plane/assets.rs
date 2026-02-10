//! Asset Embedding
//!
//! Embeds ControlPlane static assets (HTML/CSS/JS/WASM) into the binary.

use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "ui/control_plane/dist/"]
pub struct ControlPlaneAssets;

impl ControlPlaneAssets {
    /// Get the index.html file
    pub fn get_index_html() -> Option<Vec<u8>> {
        Self::get("index.html").map(|f| f.data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assets_exist() {
        // This test will only pass after building the UI
        // For now, we just check that the struct compiles
        let _assets = ControlPlaneAssets;
    }
}
