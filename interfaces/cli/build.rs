// Build script for the `aleph` CLI binary.
//
// `aleph-cli` deliberately does NOT depend on `alephcore` (it is a reference
// client), so it cannot inherit `ALEPH_VERSION` from the core build script.
// Re-derive it here from the workspace `VERSION` file — the single source of
// truth — so the CLI reports the same version as the daemon instead of the
// (manually kept-in-sync) `CARGO_PKG_VERSION`.
fn main() {
    // Crate manifest dir is `interfaces/cli`; the VERSION file lives at the
    // workspace root, two levels up.
    println!("cargo:rerun-if-changed=../../VERSION");
    if let Ok(version) = std::fs::read_to_string("../../VERSION") {
        println!("cargo:rustc-env=ALEPH_VERSION={}", version.trim());
    } else {
        // Fallback to the crate's Cargo version when the VERSION file is not
        // reachable (e.g. crate built in isolation outside the workspace).
        println!(
            "cargo:rustc-env=ALEPH_VERSION={}",
            env!("CARGO_PKG_VERSION")
        );
    }
}
