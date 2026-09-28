//! `aleph` — the CLI's long name. The same program as `al`; everything lives
//! in the library so the two binaries cannot differ.

fn main() -> std::process::ExitCode {
    aleph_cli::main()
}
