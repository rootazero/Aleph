// Build script for the `aleph-tui` crate.
//
// Same job, same reason, as `interfaces/cli/build.rs`: this crate deliberately
// does NOT depend on `alephcore`, so it cannot inherit `ALEPH_VERSION` from the
// core build script, and `CARGO_PKG_VERSION` is the workspace number that is
// *manually* kept in sync with the `VERSION` file (see the comment above
// `[workspace.package] version`). The header line names a version to the user;
// it reads the single source of truth rather than the copy that drifts.
fn main() {
    // Crate manifest dir is `interfaces/tui`; the VERSION file lives at the
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
