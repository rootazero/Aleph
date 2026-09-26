//! The `bash` tool's shell, run for real under the seatbelt profile the tool
//! asks for, with a `PATH` whose first `bash` that profile cannot execute.
//!
//! This is the Homebrew defect reduced to its shape: `/opt/homebrew/bin/bash`
//! is a symlink in a directory the profile grants nothing on, `sandbox-exec`
//! cannot read it at `execvp`, and the call exits 71 before printing a byte.
//! The fake directory reproduces that without needing Homebrew on the host.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use super::{CodeExecArgs, Language};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::platforms::macos::seatbelt::SeatbeltDriver;
use crate::utils::shell::{locate_runnable, ShellKind};

/// `sandbox-exec`'s own exit code when its `execvp` of the program fails.
const EX_OSERR: i32 = 71;

fn bash_tool_args() -> CodeExecArgs {
    CodeExecArgs {
        language: Language::Shell,
        code: String::new(),
        working_dir: None,
        timeout_seconds: None,
        allow_network: false,
        allow_subprocess: false,
        extra_writable_paths: Vec::new(),
        justification: None,
    }
}

async fn run_shell(
    driver: &SeatbeltDriver,
    program: &Path,
    script: &str,
    path_var: &std::ffi::OsStr,
    cwd: &Path,
    profile: &OsSandboxProfile,
) -> crate::sandbox::SandboxOutput {
    let (argv, stdin) = ShellKind::Bash.invocation(script);
    let env = HashMap::from([("PATH".to_string(), path_var.to_string_lossy().into_owned())]);
    driver
        .run(
            &program.to_string_lossy(),
            &argv,
            &env,
            stdin.as_deref(),
            cwd,
            profile,
            Duration::from_secs(20),
            4096,
        )
        .await
        .expect("sandbox-exec spawns")
}

#[tokio::test]
async fn bash_tool_runs_when_path_puts_an_unrunnable_bash_first() {
    let driver = SeatbeltDriver::new();
    assert!(
        driver.is_supported(),
        "every macOS ships /usr/bin/sandbox-exec"
    );

    // Outside every grant the profile makes (system trees, tmp dirs, the
    // workspace) — the manifest dir, not the temp dir, which is granted.
    let fake_brew = tempfile::Builder::new()
        .prefix(".seatbelt-fake-brew-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .expect("fake brew dir");
    let fake_bash = fake_brew.path().join("bash");
    std::os::unix::fs::symlink("/bin/bash", &fake_bash).expect("symlink");
    let path_var =
        std::env::join_paths([fake_brew.path(), Path::new("/usr/bin"), Path::new("/bin")])
            .expect("PATH");

    let workspace = tempfile::tempdir().expect("workspace");
    let profile = driver
        .profile_for(&bash_tool_args().as_capabilities(), workspace.path())
        .expect("profile");
    let script = "echo one && echo two | cat";

    // Witness: the fixture reproduces the defect. First-on-PATH resolution
    // would pick this link, and the profile refuses it. If this stops being
    // 71, the fixture no longer proves anything below.
    let refused = run_shell(
        &driver,
        &fake_bash,
        script,
        &path_var,
        workspace.path(),
        &profile,
    )
    .await;
    assert_eq!(
        refused.exit_code,
        Some(EX_OSERR),
        "the fake brew bash must be unrunnable under the bash tool's profile; stderr: {}",
        String::from_utf8_lossy(&refused.stderr)
    );

    let program = locate_runnable("bash", Some(path_var.as_os_str())).expect("a bash exists");
    let out = run_shell(
        &driver,
        &program,
        script,
        &path_var,
        workspace.path(),
        &profile,
    )
    .await;
    assert_eq!(
        out.exit_code,
        Some(0),
        "resolved {} — stderr: {}",
        program.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "one\ntwo\n");
}
