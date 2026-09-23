//! `al` — the CLI's primary name. The same program as `aleph`; everything
//! lives in the library so the two binaries cannot differ.

fn main() -> std::process::ExitCode {
    aleph_cli::main()
}
