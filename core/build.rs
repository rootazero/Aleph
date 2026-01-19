// Build script for Aether Core
//
// - UniFFI scaffolding generation for macOS (default)
// - csbindgen P/Invoke generation for Windows (cabi feature)

fn main() {
    // UniFFI scaffolding for macOS
    #[cfg(feature = "uniffi")]
    {
        uniffi::generate_scaffolding("src/aether.udl").unwrap();
    }

    // csbindgen for Windows
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
