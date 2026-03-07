// Build script for Aleph Core
//
// When control-plane feature is enabled:
// - Watches dist/ so rust-embed re-embeds when WASM assets change
// - Falls back to trunk build if dist/ is missing (for `cargo run` without justfile)

fn main() {
    #[cfg(feature = "control-plane")]
    {
        use std::path::Path;
        use std::process::Command;

        let control_plane_dir = Path::new("../apps/panel");
        let dist_dir = control_plane_dir.join("dist");

        // Watch dist/ files so cargo recompiles when assets change (rust-embed)
        println!("cargo:rerun-if-changed=../apps/panel/dist");
        if dist_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&dist_dir) {
                for entry in entries.flatten() {
                    println!("cargo:rerun-if-changed={}", entry.path().display());
                }
            }
        }

        // Watch source for fallback trunk build trigger
        println!("cargo:rerun-if-changed=../apps/panel/src");
        println!("cargo:rerun-if-changed=../apps/panel/Cargo.toml");
        println!("cargo:rerun-if-changed=../apps/panel/index.html");

        if !control_plane_dir.exists() {
            println!("cargo:warning=Panel directory not found, skipping UI build");
            return;
        }

        // If dist/ already has files (built by `just wasm`), skip trunk
        if dist_dir.exists() && dist_dir.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false) {
            println!("cargo:warning=Panel UI assets found in dist/, embedding into binary");
            return;
        }

        // Fallback: try trunk build for `cargo run --features control-plane` without justfile
        println!("cargo:warning=Building Panel UI via trunk...");

        match Command::new("trunk")
            .args(&["build", "--release"])
            .current_dir(control_plane_dir)
            .status()
        {
            Ok(status) if status.success() => {
                println!("cargo:warning=Panel UI built successfully");
            }
            Ok(_) => {
                println!("cargo:warning=Panel build failed. Server will run without UI.");
                println!("cargo:warning=Run `just wasm` first, or fix trunk issues.");
            }
            Err(e) => {
                println!("cargo:warning=Failed to execute trunk: {}. Server will run without UI.", e);
                println!("cargo:warning=Run `just wasm` first, or install trunk.");
            }
        }
    }
}
