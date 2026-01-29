// Build script for Aether Core
//
// UniFFI removed - using Gateway WebSocket architecture
// csbindgen retained for Windows (cabi feature, deprecated)

fn main() {
    // csbindgen for Windows (deprecated)
    #[cfg(feature = "cabi")]
    {
        csbindgen::Builder::default()
            .input_extern_file("src/ffi_cabi.rs")
            .csharp_dll_name("aethecore")
            .csharp_namespace("Aether.Interop")
            .csharp_class_name("NativeMethods")
            .csharp_class_accessibility("public")
            .csharp_use_function_pointer(true)
            .generate_csharp_file("../platforms/windows/Aether/Interop/NativeMethods.g.cs")
            .unwrap();
    }
}
