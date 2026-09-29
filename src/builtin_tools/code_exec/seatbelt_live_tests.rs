//! The programs `code_exec` spawns, run for real under the seatbelt profile the
//! tool asks for, with a `PATH` whose first candidate that profile cannot
//! execute.
//!
//! This is the Homebrew defect reduced to its shape: `/opt/homebrew/bin/bash`
//! is a symlink in a directory the profile grants nothing on, `sandbox-exec`
//! cannot read it at `execvp`, and the call exits 71 before printing a byte.
//! The witness directory reproduces that without needing Homebrew on the host,
//! and resolution goes through the same `*_in` functions the cached answers
//! use, so reverting the wiring goes red on any Mac.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{CodeExecArgs, Language};
use crate::sandbox::driver::{OsSandboxDriverTrait, OsSandboxProfile};
use crate::sandbox::platforms::macos::seatbelt::SeatbeltDriver;
use crate::sandbox::SandboxOutput;
use crate::utils::shell::{python3_in, resolve_in, ShellKind};

/// `sandbox-exec`'s own exit code when its `execvp` of the program fails.
const EX_OSERR: i32 = 71;

/// The Command Line Tools developer dir; its `usr/bin/python3` is what the
/// `/usr/bin/python3` stub execs when it is the active developer dir.
const CLT_DIR: &str = "/Library/Developer/CommandLineTools";

fn tool_args(language: Language) -> CodeExecArgs {
    CodeExecArgs {
        language,
        code: String::new(),
        working_dir: None,
        timeout_seconds: None,
        allow_network: false,
        allow_subprocess: false,
        extra_writable_paths: Vec::new(),
        justification: None,
    }
}

/// A directory the profile grants nothing on, holding `name` as a symlink to
/// `target` — a Homebrew-shaped entry that `PATH` puts first.
///
/// Under `$HOME`, because every tree the profile grants is a system tree, a
/// temp dir, or the workspace (itself a temp dir here). A `$HOME` that sits
/// under one of the temp trees would make the witness runnable; that is
/// refused by name instead of turning into a red that is not about the code.
fn witness(name: &str, target: &str) -> (tempfile::TempDir, PathBuf) {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    let home = std::fs::canonicalize(&home).expect("HOME exists");
    let temp_trees = [
        std::fs::canonicalize(std::env::temp_dir()).expect("temp dir"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/private/var/tmp"),
    ];
    assert!(
        !temp_trees.iter().any(|t| home.starts_with(t)),
        "HOME ({}) is under a tree the profile grants; the witness cannot be built here",
        home.display()
    );
    let dir = tempfile::Builder::new()
        .prefix(".aleph-seatbelt-witness-")
        .tempdir_in(&home)
        .expect("witness dir");
    let link = dir.path().join(name);
    std::os::unix::fs::symlink(target, &link).expect("symlink");
    (dir, link)
}

fn path_var(first: &Path) -> OsString {
    std::env::join_paths([first, Path::new("/usr/bin"), Path::new("/bin")]).expect("PATH")
}

/// The child environment `code_exec` builds — this process's values for
/// `default_pass_env()` — with the test's `PATH`. It matters: without `HOME`
/// the `/usr/bin/python3` stub misses xcrun's cache and tries to spawn
/// `xcodebuild`, which the Python profile's fork ban refuses.
fn tool_env(path: &OsString) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = super::default_pass_env()
        .into_iter()
        .filter_map(|name| std::env::var(&name).ok().map(|value| (name, value)))
        .collect();
    env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
    env
}

async fn run(
    program: &Path,
    argv: &[String],
    stdin: Option<&[u8]>,
    env: &HashMap<String, String>,
    cwd: &Path,
    profile: &OsSandboxProfile,
) -> SandboxOutput {
    SeatbeltDriver::new()
        .run(
            &program.to_string_lossy(),
            argv,
            env,
            stdin,
            cwd,
            profile,
            Duration::from_secs(20),
            4096,
        )
        .await
        .expect("sandbox-exec spawns")
}

fn stderr(out: &SandboxOutput) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[tokio::test]
async fn bash_tool_runs_when_path_puts_an_unrunnable_bash_first() {
    let driver = SeatbeltDriver::new();
    assert!(
        driver.is_supported(),
        "every macOS ships /usr/bin/sandbox-exec"
    );
    let (dir, fake_bash) = witness("bash", "/bin/bash");
    let path = path_var(dir.path());
    let env = tool_env(&path);

    let workspace = tempfile::tempdir().expect("workspace");
    let profile = driver
        .profile_for(
            &tool_args(Language::Shell).as_capabilities(),
            workspace.path(),
        )
        .expect("profile");
    let (argv, stdin) = ShellKind::Bash.invocation("echo one && echo two | cat");

    // Witness: first-on-PATH resolution would pick this link, and the profile
    // refuses it. If this stops being 71 the fixture proves nothing below.
    let refused = run(
        &fake_bash,
        &argv,
        stdin.as_deref(),
        &env,
        workspace.path(),
        &profile,
    )
    .await;
    assert_eq!(
        refused.exit_code,
        Some(EX_OSERR),
        "the witness bash must be unrunnable under the bash tool's profile; stderr: {}",
        stderr(&refused)
    );

    let shell = resolve_in(Some(path.as_os_str()));
    let out = run(
        &shell.program,
        &argv,
        stdin.as_deref(),
        &env,
        workspace.path(),
        &profile,
    )
    .await;
    assert_eq!(
        out.exit_code,
        Some(0),
        "resolved {} — stderr: {}",
        shell.program.display(),
        stderr(&out)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "one\ntwo\n");
}

/// `/usr/bin/python3` is an xcrun stub that execs from the active developer
/// dir. Run it both ways this host allows: as configured, and — when the
/// Command Line Tools are installed — with `DEVELOPER_DIR` pointed at them,
/// which is the CLT-only Mac the profile's CLT grant exists for.
#[tokio::test]
#[ignore = "macOS only; requires Xcode installed AND a seatbelt profile that grants writes to /var/folders/*/T/xcrun_db-* (cache for xcrun stub). Both gates are CI-host-dependent; run manually on a fully-configured Mac."]
async fn python_runs_when_path_puts_an_unrunnable_python3_first() {
    // No developer dir at all: the stub would offer the CLT installer (a GUI
    // dialog), so do not invoke it.
    let has_dev_dir = std::process::Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .output()
        .is_ok_and(|o| o.status.success());
    if !has_dev_dir {
        eprintln!("SKIP: `xcode-select -p` failed — no developer tools, /usr/bin/python3 is only an installer prompt");
        return;
    }

    let driver = SeatbeltDriver::new();
    let (dir, fake_python) = witness("python3", "/usr/bin/python3");
    let path = path_var(dir.path());
    let workspace = tempfile::tempdir().expect("workspace");
    let profile = driver
        .profile_for(
            &tool_args(Language::Python).as_capabilities(),
            workspace.path(),
        )
        .expect("profile");
    let argv = vec!["-c".to_string(), "print(1)".to_string()];
    let env = tool_env(&path);

    let refused = run(&fake_python, &argv, None, &env, workspace.path(), &profile).await;
    assert_eq!(
        refused.exit_code,
        Some(EX_OSERR),
        "the witness python3 must be unrunnable under code_exec's profile; stderr: {}",
        stderr(&refused)
    );

    let python = python3_in(Some(path.as_os_str()));
    let mut envs = vec![("as configured", env.clone())];
    if Path::new(CLT_DIR).join("usr/bin/python3").is_file() {
        let mut clt = env.clone();
        clt.insert("DEVELOPER_DIR".to_string(), CLT_DIR.to_string());
        envs.push(("DEVELOPER_DIR=Command Line Tools", clt));
    } else {
        eprintln!("SKIP (CLT half): {CLT_DIR} has no python3 on this host");
    }
    for (label, env) in envs {
        let out = run(
            &python.program,
            &argv,
            None,
            &env,
            workspace.path(),
            &profile,
        )
        .await;
        assert_eq!(
            out.exit_code,
            Some(0),
            "[{label}] resolved {} — stderr: {}",
            python.program.display(),
            stderr(&out)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "1",
            "[{label}]"
        );
    }
}
