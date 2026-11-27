#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn main() -> std::process::ExitCode {
    aleph_nsh_shell::run_command_line()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn main() -> std::process::ExitCode {
    eprintln!("The NSH host is unsupported on this target");
    std::process::ExitCode::FAILURE
}
