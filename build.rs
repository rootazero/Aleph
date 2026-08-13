// Build script for Aleph Core
//
// When control-plane feature is enabled:
// - Watches dist/ so rust-embed re-embeds when WASM assets change
// - Falls back to trunk build if dist/ is missing (for `cargo run` without justfile)

fn main() {
    // Read version from VERSION file (single source of truth)
    // This overrides CARGO_PKG_VERSION so all code uses the same version
    println!("cargo:rerun-if-changed=VERSION");
    // Re-embed bundled skills/plugins (include_dir!) when their submodule
    // checkouts change — without this, edits land only after a clean build.
    println!("cargo:rerun-if-changed=skills");
    println!("cargo:rerun-if-changed=plugins");
    // The Panel UI (interfaces/webchat/dist) is embedded UNCONDITIONALLY via
    // rust_embed (src/gateway/control_plane/assets.rs compiles regardless of the
    // `control-plane` feature). rust_embed emits no rerun-if-changed for its
    // folder, so without this an incremental `cargo build` after `just wasm`
    // reuses the cached embed and serves a STALE panel. This MUST stay outside
    // the `control-plane` cfg block below: production server builds (`just
    // build`) don't enable that feature, so a gated trigger never fires and the
    // panel never re-embeds.
    println!("cargo:rerun-if-changed=interfaces/webchat/dist");
    // `#[derive(RustEmbed)]` is a hard compile-time error when its `folder`
    // does not exist, and `dist/` holds only generated artifacts — so a fresh
    // clone (or anyone who `rm -rf`s it) cannot build alephcore at all. That
    // requirement used to be carried by a tracked `dist/.gitkeep`, i.e. by a
    // file whose only purpose lived in a .gitignore comment; it was read as
    // vestigial and deleted, and main stopped compiling. Build scripts run
    // before the crate they belong to, so creating the directory here makes
    // the requirement self-satisfying instead of a convention someone has to
    // know about.
    let _ = std::fs::create_dir_all("interfaces/webchat/dist");
    if let Ok(entries) = std::fs::read_dir("interfaces/webchat/dist") {
        for entry in entries.flatten() {
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }
    if let Ok(version) = std::fs::read_to_string("VERSION") {
        let version = version.trim();
        println!("cargo:rustc-env=ALEPH_VERSION={version}");
    } else {
        // Fallback to Cargo.toml version
        println!(
            "cargo:rustc-env=ALEPH_VERSION={}",
            env!("CARGO_PKG_VERSION")
        );
    }

    // macOS: embed an Info.plist into the `aleph-server` binary so the detached
    // daemon gets a STABLE, grantable Local Network Privacy identity. The
    // desktop shell launches it with `--daemon` (double-fork + setsid), which
    // severs the shell's TCC "responsible process" chain — the daemon then
    // becomes its own Local Network subject. Without a CFBundleIdentifier it is
    // ad-hoc/linker-signed under a per-build-random id that never appears in
    // System Settings → Privacy & Security → Local Network, so its requests to
    // LAN hosts (self-hosted SearXNG / Firecrawl, …) are silently blocked.
    // Embedded at link time so the codesign step (Tauri/packaging) covers — and
    // adopts the identifier from — this section. See src/bin/aleph-server/Info.plist.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let manifest_dir =
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
        let plist = format!("{manifest_dir}/src/bin/aleph-server/Info.plist");
        println!("cargo:rerun-if-changed=src/bin/aleph-server/Info.plist");
        println!(
            "cargo:rustc-link-arg-bin=aleph-server=-Wl,-sectcreate,__TEXT,__info_plist,{plist}"
        );
    }

    #[cfg(feature = "control-plane")]
    {
        use std::path::Path;
        use std::process::Command;

        let control_plane_dir = Path::new("interfaces/webchat");
        let dist_dir = control_plane_dir.join("dist");

        // (dist/ rerun-if-changed is emitted unconditionally above — the embed
        // is not gated by `control-plane`, so its trigger must not be either.)

        // Watch source for fallback trunk build trigger
        println!("cargo:rerun-if-changed=interfaces/webchat/src");
        println!("cargo:rerun-if-changed=interfaces/webchat/Cargo.toml");
        println!("cargo:rerun-if-changed=interfaces/webchat/index.html");

        if !control_plane_dir.exists() {
            println!("cargo:warning=Panel directory not found, skipping UI build");
            return;
        }

        // If dist/ already has files (built by `just wasm`), skip trunk
        if dist_dir.exists()
            && dist_dir
                .read_dir()
                .map(|mut d| d.next().is_some())
                .unwrap_or(false)
        {
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
                println!(
                    "cargo:warning=Failed to execute trunk: {}. Server will run without UI.",
                    e
                );
                println!("cargo:warning=Run `just wasm` first, or install trunk.");
            }
        }
    }
}
