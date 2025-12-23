// Build script for UniFFI binding generation
fn main() {
    uniffi::generate_scaffolding("src/aether.udl").unwrap();
}
