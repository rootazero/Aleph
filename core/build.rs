// Build script for Aleph Core
//
// Automatically builds ControlPlane UI when control-plane feature is enabled

use std::process::Command;
use std::path::Path;

fn main() {
    #[cfg(feature = "control-plane")]
    {
        println!("cargo:rerun-if-changed=ui/control_plane/src");
        println!("cargo:rerun-if-changed=ui/control_plane/Cargo.toml");
        println!("cargo:rerun-if-changed=ui/control_plane/index.html");

        let control_plane_dir = Path::new("ui/control_plane");
        let dist_dir = control_plane_dir.join("dist");

        if !control_plane_dir.exists() {
            println!("cargo:warning=ControlPlane directory not found, skipping UI build");
            return;
        }

        // Skip build if dist directory already exists and contains files
        if dist_dir.exists() && dist_dir.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false) {
            println!("cargo:warning=ControlPlane UI already built (dist/ exists), skipping build");
            return;
        }

        println!("cargo:warning=Building ControlPlane UI...");

        let status = Command::new("trunk")
            .args(&["build", "--release"])
            .current_dir(control_plane_dir)
            .status()
            .expect("Failed to execute trunk. Make sure trunk is installed: cargo install trunk");

        if !status.success() {
            panic!("ControlPlane build failed. Check trunk output above.");
        }

        println!("cargo:warning=ControlPlane UI built successfully");
    }
}
