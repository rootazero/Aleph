//! Native launch for the fixed JSONL consumer. Not an arbitrary command runner.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::{Child, Command};
use tokio::time::Instant;

use super::BubblewrapDriver;
use crate::sandbox::cgroup_v2::{CgroupV2Limits, CgroupV2Scope, StrictShellCgroup};
use crate::sandbox::driver::OsSandboxProfile;
use crate::sandbox::platforms::common::{reap_strict_shell_child, run_strict_shell_child};
use crate::sandbox::sandbox_init::{LinuxInitPolicy, SeccompNetworkMode};
use crate::sandbox::shell_input::{JSONL_ARGS, JSONL_PROGRAM, JSONL_RECORDS_PATH};
use crate::sandbox::shell_lifecycle::ShellLifecycle;
use crate::sandbox::{SandboxError, SandboxOutput};

const BWRAP: &str = "/usr/bin/bwrap";
const WORKSPACE: &str = "/workspace";
const INIT: &str = "/aleph-sandbox-init";
const STARTUP_LIMIT: Duration = Duration::from_secs(3);

impl BubblewrapDriver {
    fn fixed_jsonl_configured(&self) -> bool {
        self.options.no_new_privs
            && self.options.cgroup_enabled
            && self.options.include_platform_defaults
            && self.deny_read_globs.is_empty()
            && self
                .options
                .cpu_quota_percent
                .is_some_and(|value| value > 0)
            && self.options.max_pids.is_some_and(|value| value > 0)
    }

    pub(super) fn fixed_jsonl_preflight(&self) -> bool {
        self.fixed_jsonl_configured()
            && StrictShellCgroup::can_delegate()
            && check_immutable_system_executable(BWRAP).is_ok()
            && check_immutable_system_executable(JSONL_PROGRAM).is_ok()
            && self.open_jsonl_sandbox_init().is_ok()
    }

    pub(super) async fn run_jsonl_consumer(
        &self,
        workspace: &File,
        profile: &OsSandboxProfile,
        timeout: Duration,
        max_output_bytes: usize,
        lifecycle: &ShellLifecycle,
    ) -> Result<SandboxOutput, SandboxError> {
        let start = Instant::now();
        if !self.fixed_jsonl_configured() || profile.linux_init_policy.is_none() {
            return Err(SandboxError::UnsupportedPolicy {
                platform: "linux/bwrap",
                feature: "jsonl_inspect".into(),
                reason: "required sandbox controls are unavailable".into(),
            });
        }
        if !workspace.metadata().map_err(setup_error)?.is_dir() {
            return Err(setup_error("pinned workspace is not a directory"));
        }
        let workspace_fd = workspace.as_raw_fd();
        check_immutable_system_executable(BWRAP).map_err(setup_error)?;
        check_immutable_system_executable(JSONL_PROGRAM).map_err(setup_error)?;
        // Mount this process's executable even if an update swaps its path.
        let init_binary = self.open_jsonl_sandbox_init().map_err(setup_error)?;
        let init_fd = init_binary.as_raw_fd();
        let scope = StrictShellCgroup::create(CgroupV2Limits {
            memory_mb: profile.max_memory_mb,
            cpu_quota_percent: self.options.cpu_quota_percent,
            max_pids: self.options.max_pids,
        })
        .map_err(setup_error)?;

        let fifo_dir = tempfile::Builder::new()
            .prefix("aleph-jsonl-")
            .tempdir()
            .map_err(setup_error)?;
        let fifo = fifo_dir.path().join("records");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).map_err(setup_error)?;
        // SAFETY: The path is an owned, NUL-terminated string in a private directory.
        if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
            return Err(setup_error(io::Error::last_os_error()));
        }
        // O_PATH pins the mount source without contributing a FIFO reader.
        let fifo_inode = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_PATH | libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&fifo)
            .map_err(setup_error)?;
        if !fifo_inode
            .metadata()
            .map_err(setup_error)?
            .file_type()
            .is_fifo()
        {
            return Err(setup_error("pinned input is not a FIFO"));
        }
        let fifo_fd = fifo_inode.as_raw_fd();
        let writer_path = PathBuf::from(format!("/proc/self/fd/{fifo_fd}"));
        let mut cmd = Command::new(BWRAP);
        cmd.args([
            "--unshare-all",
            "--unshare-user",
            "--disable-userns",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--tmpfs",
            "/",
            "--dev",
            "/dev",
            "--size",
            "1048576",
            "--tmpfs",
            "/tmp",
        ]);
        let mut read_paths = vec![PathBuf::from(WORKSPACE), PathBuf::from(JSONL_RECORDS_PATH)];
        for root in ["/usr", "/lib", "/lib64", "/etc/ld.so.cache"] {
            if Path::new(root).exists() {
                cmd.args(["--ro-bind", root, root]);
                read_paths.push(PathBuf::from(root));
            }
        }
        let policy = LinuxInitPolicy {
            read_paths,
            write_paths: vec![PathBuf::from("/tmp")],
            require_landlock: true,
            seccomp_net: SeccompNetworkMode::UnixOnly,
        };
        let policy = serde_json::to_string(&policy).map_err(setup_error)?;
        cmd.arg("--ro-bind-fd")
            .arg(workspace_fd.to_string())
            .arg(WORKSPACE)
            .arg("--ro-bind-fd")
            .arg(fifo_fd.to_string())
            .arg(JSONL_RECORDS_PATH)
            .arg("--ro-bind-fd")
            .arg(init_fd.to_string())
            .arg(INIT)
            .args([
                "--remount-ro",
                "/",
                "--chdir",
                WORKSPACE,
                "--",
                INIT,
                "sandbox-init",
                "--policy",
            ])
            .arg(policy)
            .args(["--", JSONL_PROGRAM])
            .args(JSONL_ARGS)
            .env_clear()
            .env("HOME", "/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        self.apply_no_new_privs(&mut cmd);
        // The mandatory cgroup limits charged memory across the entire job.
        // RLIMIT_AS would also charge virtual mappings of sandbox-init, so the
        // same numerical limit can prevent it from even starting.
        let procs_path = scope.procs_path();
        // SAFETY: the descriptors stay open through spawn. The callback
        // uses only bounded stack memory and syscalls before exec.
        unsafe {
            cmd.pre_exec(move || {
                for fd in [workspace_fd, init_fd, fifo_fd] {
                    if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                }
                CgroupV2Scope::write_current_pid_to_path(&procs_path)
            })
        };

        if start.elapsed() >= timeout {
            return Err(setup_error("startup deadline elapsed"));
        }
        let mut child = lifecycle.spawn(|| cmd.spawn().map_err(setup_error))?;
        let ready = await_fifo_writer(
            &writer_path,
            &mut child,
            lifecycle,
            start + timeout.min(STARTUP_LIMIT),
        )
        .await
        .and_then(|writer| lifecycle.ready_with_input(writer));
        if let Err(error) = ready {
            reap_strict_shell_child(&mut child, scope, true).await?;
            lifecycle.reaped(lifecycle.cancellation_requested());
            return Err(setup_error(error));
        }
        // Neither the parent's O_PATH descriptor nor the host name has to
        // outlive the mounted reader and its verified writer.
        drop(fifo_inode);
        drop(fifo_dir);
        run_strict_shell_child(
            child,
            timeout.saturating_sub(start.elapsed()),
            max_output_bytes,
            lifecycle,
            scope,
        )
        .await
    }

    fn open_jsonl_sandbox_init(&self) -> io::Result<File> {
        #[cfg(feature = "test-helpers")]
        if let Some(path) = &self.test_sandbox_init {
            return File::open(path);
        }
        #[cfg(test)]
        if let Some(path) = std::env::var_os("ALEPH_TEST_FIXED_JSONL_SERVER") {
            return File::open(path);
        }
        File::open("/proc/self/exe")
    }
}

fn setup_error(error: impl std::fmt::Display) -> SandboxError {
    SandboxError::ExecutionFailed(format!("fixed JSONL startup refused: {error}"))
}

fn check_immutable_system_executable(path: &str) -> io::Result<()> {
    let canonical = fs::canonicalize(path)?;
    if !canonical.starts_with("/usr") {
        return Err(io::Error::other(
            "executable resolves outside the trusted runtime",
        ));
    }
    for entry in canonical.ancestors() {
        let meta = fs::metadata(entry)?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 || (entry == canonical && !meta.is_file()) {
            return Err(io::Error::other(
                "runtime executable or ancestor is not trusted",
            ));
        }
    }
    Ok(())
}

async fn await_fifo_writer(
    pinned_fifo_path: &Path,
    child: &mut Child,
    lifecycle: &ShellLifecycle,
    deadline: Instant,
) -> io::Result<File> {
    loop {
        if lifecycle.cancellation_requested() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "launch was cancelled",
            ));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "reader did not attach",
            ));
        }
        if let Some(status) = child.try_wait()? {
            return Err(io::Error::other(format!(
                "sandbox exited before reader attached ({status})"
            )));
        }
        // This procfs magic link refers to our still-open O_PATH descriptor,
        // not a destination that the workspace or consumer can substitute.
        match OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(pinned_fifo_path)
        {
            Ok(writer) => return Ok(writer),
            Err(error) if error.raw_os_error() == Some(libc::ENXIO) => {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::gateway::router::SessionKey;
    use crate::sandbox::build_sandbox;
    use crate::sandbox::context::{with_exec_workspace, LIVE_TAIL, SESSION_ID};
    use crate::sandbox::exec_approval::gate::ApprovalGate;
    use crate::sandbox::platforms::linux::bwrap::LinuxSandboxOptions;
    use crate::sandbox::shell_input::{
        JsonlFrame, ShellEffectDenied, ShellEffectResult, ShellInputAction, ShellPipeAttempt,
    };
    use crate::sandbox::shell_workspace::ShellWorkspace;
    use crate::sandbox::{LiveTail, SandboxCapabilities, SandboxCommand, SandboxConfig};

    #[tokio::test]
    #[ignore = "requires a delegated Linux supervisor subgroup and ALEPH_TEST_FIXED_JSONL_SERVER"]
    async fn fixed_jsonl_real_workspace_process() {
        assert!(std::env::var_os("ALEPH_TEST_FIXED_JSONL_SERVER").is_some());
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let witness = ShellWorkspace::pin(&workspace).unwrap();
        let config = SandboxConfig {
            enabled: true,
            workspace_root: root.path().join("sessions"),
            ..Default::default()
        };
        let sandbox = build_sandbox(
            &config,
            Arc::new(BubblewrapDriver::with_options(LinuxSandboxOptions {
                cpu_quota_percent: Some(50),
                max_pids: Some(16),
                ..Default::default()
            })),
            Arc::new(ApprovalGate::new(None)),
            Default::default(),
            &crate::config::types::ShellSecurityConfig::default(),
        );
        let sandbox = sandbox.as_workspace().unwrap();
        let supervisor = fs::read_to_string("/proc/self/cgroup").unwrap();
        let supervisor =
            crate::sandbox::cgroup_v2::parse_proc_self_cgroup_path(&supervisor).unwrap();
        let delegated_root = Path::new("/sys/fs/cgroup")
            .join(supervisor.strip_prefix("/").unwrap().parent().unwrap());

        for finish in ["eof", "stop", "timeout", "shutdown"] {
            let session = SessionKey::main("fixed-jsonl-proof");
            let launch = SandboxCommand {
                session_id: session.clone(),
                tool_name: "bash".into(),
                program: JSONL_PROGRAM.into(),
                args: JSONL_ARGS.into_iter().map(str::to_owned).collect(),
                env: Default::default(),
                stdin: None,
                cwd: Some(witness.path().to_path_buf()),
                capabilities: SandboxCapabilities {
                    max_memory_mb: Some(256),
                    ..Default::default()
                },
                timeout: Some(Duration::from_secs(10)),
            };
            let expired_control = ShellLifecycle::default();
            let expired = with_exec_workspace(
                Some(workspace.clone()),
                sandbox.execute_fixed_jsonl(
                    launch.clone(),
                    &witness,
                    &expired_control,
                    Instant::now(),
                ),
            )
            .await;
            assert!(
                matches!(expired, Err(SandboxError::ExecutionFailed(message)) if message == "fixed JSONL startup deadline elapsed")
            );
            assert!(expired_control.input_snapshot().is_err());

            let job = Arc::new(ShellLifecycle::default());
            let live = Arc::new(LiveTail::new());
            let mut run = Box::pin(SESSION_ID.scope(
                session,
                LIVE_TAIL.scope(
                    live.clone(),
                    with_exec_workspace(
                        Some(workspace.clone()),
                        sandbox.execute_fixed_jsonl(
                            launch,
                            &witness,
                            &job,
                            Instant::now()
                                + if finish == "timeout" {
                                    Duration::from_secs(3)
                                } else {
                                    Duration::from_secs(8)
                                },
                        ),
                    ),
                ),
            ));
            let deadline = Instant::now() + Duration::from_secs(5);
            tokio::select! {
                result = &mut run => panic!("fixed consumer ended before attaching: {result:?}"),
                ready = tokio::time::timeout_at(deadline, async {
                    while job.input_snapshot().is_err() {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }) => ready.expect("fixed consumer never attached"),
            }
            let group = fs::read_dir(&delegated_root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .as_bytes()
                        .starts_with(b"aleph-jsonl-")
                })
                .expect("the real consumer has a delegated job group");
            for (control, value) in [
                ("cpu.max", "50000 100000"),
                ("memory.max", "268435456"),
                ("memory.swap.max", "0"),
                ("pids.max", "16"),
            ] {
                assert_eq!(
                    fs::read_to_string(group.join(control)).unwrap().trim(),
                    value
                );
            }
            let processes = fs::read_to_string(group.join("cgroup.procs")).unwrap();
            let jq = processes
                .lines()
                .find_map(|pid| {
                    let process = Path::new("/proc").join(pid);
                    (fs::read_to_string(process.join("comm")).ok()?.trim() == "jq")
                        .then_some(process)
                })
                .expect("jq is a member before input delivery");
            assert!(
                processes.lines().count() > 1,
                "jq and its supervisor share the group"
            );
            let status = fs::read_to_string(jq.join("status")).unwrap();
            assert!(status
                .lines()
                .any(|line| line.starts_with("NoNewPrivs:") && line.ends_with('1')));
            assert!(status
                .lines()
                .any(|line| line.starts_with("CapEff:") && line.ends_with("0000000000000000")));
            assert!(status
                .lines()
                .any(|line| line.starts_with("Seccomp:") && line.ends_with('2')));
            let frame = JsonlFrame::from_record(&serde_json::json!([1, 2])).unwrap();
            let bytes = frame.bytes().len();
            let attempt = |prefix, action| {
                ShellPipeAttempt::new_for_test(job.clone(), prefix, deadline.into_std(), action)
            };
            assert_eq!(
                attempt(0, ShellInputAction::Write(frame)).consume(),
                Ok(ShellEffectResult::Written(bytes))
            );
            tokio::select! {
                result = &mut run => panic!("fixed consumer ended before EOF: {result:?}"),
                output = tokio::time::timeout_at(deadline, async {
                    loop {
                        let output = live.snapshot().stdout;
                        if output.ends_with(b"\n") {
                            break output;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                }) => assert_eq!(output.expect("no output before EOF"), b"{\"kind\":\"array\",\"count\":2}\n"),
            }
            let registry = crate::builtin_tools::process_registry::process_registry();
            let result = match finish {
                "shutdown" => {
                    let crate::builtin_tools::process_registry::RegisterOutcome::Registered(id) =
                        registry.reserve_shell("native shutdown", None, job.clone())
                    else {
                        panic!("native shutdown job must be in the production registry");
                    };
                    let (unconfirmed, ()) = tokio::join!(
                        crate::builtin_tools::bash_exec::shutdown_background_jobs(),
                        async {
                            let result = run.await;
                            assert!(matches!(result, Err(SandboxError::ExecutionFailed(_))));
                            let outcome = crate::builtin_tools::code_exec::sandbox_result_to_output(
                                result,
                                "bash".into(),
                                8,
                            );
                            assert!(!registry.complete(id, outcome));
                            assert!(matches!(
                                registry.poll(id, None),
                                crate::builtin_tools::process_registry::PollOutcome::Killed
                            ));
                        },
                    );
                    assert_eq!(unconfirmed, 0);
                    None
                }
                _ => {
                    match finish {
                        "stop" => {
                            job.request_cancel();
                            assert_eq!(
                                attempt(bytes, ShellInputAction::Close).consume(),
                                Err(ShellEffectDenied::Stale)
                            );
                        }
                        "eof" => assert_eq!(
                            attempt(bytes, ShellInputAction::Close).consume(),
                            Ok(ShellEffectResult::Closed)
                        ),
                        "timeout" => {}
                        _ => unreachable!(),
                    }
                    Some(
                        tokio::time::timeout(Duration::from_secs(6), run)
                            .await
                            .expect("native group did not settle"),
                    )
                }
            };
            assert!(
                job.settled(),
                "strict teardown was not acknowledged: {result:?}"
            );
            assert!(!group.exists(), "{finish} left a populated group");
            assert!(!jq.exists(), "{finish} left the consumer alive");
            match finish {
                "eof" => assert_eq!(result.unwrap().unwrap().exit_code, Some(0)),
                "timeout" => assert!(matches!(result, Some(Err(SandboxError::Timeout { .. })))),
                _ => assert!(job.stopped(), "{finish}: {result:?}"),
            }
        }
    }
}
