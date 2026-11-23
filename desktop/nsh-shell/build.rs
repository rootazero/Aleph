//! With the `tsx-panel` feature the shell compiles in the Panel's bundle;
//! fail early, with the command that builds it, when it is missing.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_TSX_PANEL").is_none() {
        return;
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let bundle = Path::new(&manifest).join("../../interfaces/panel/out-bundle");
    for file in ["bundle.js", "capabilities.json"] {
        let path = bundle.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        if !path.is_file() {
            panic!(
                "the tsx-panel feature embeds {}; build it first with `just panel-tsx-bundle` \
                 (nsh build interfaces/panel -o interfaces/panel/out-bundle)",
                path.display()
            );
        }
    }
}
