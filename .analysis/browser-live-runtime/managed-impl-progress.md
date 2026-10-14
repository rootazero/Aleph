# managed impl progress
- start: no prior edits
## Plan (decided)
- D: new variant capabilities::NetworkPolicy::LoopbackPorts{ports} (+ policy::NetworkPolicy::LoopbackPorts). Companion derives once (companion/process.rs managed_network) : loopback literal check; windows -> LoopbackPorts{[port]}, other OS -> AllowHosts{[host]} (macOS unchanged). Windows generate_profile emits network=loopback_ports:..; run() refuses it (one-shot); spawn_managed parses -> NetworkRequest. Other drivers reject LoopbackPorts.
- B: managed.rs trait ManagedProcess (take_stdio/service_pid/start_kill/wait) + TokioManagedProcess; ManagedChild{process,keepalive}.
- A+C: extract AppContainerProfile RAII from windows_init/imp/app_container.rs; windows managed_launch production api + driver spawn_managed.
- E: wfp module.
- D+B code edited (capabilities.rs, policy.rs, bwrap.rs, seatbelt.rs, sandbox_init.rs, dns.rs, summary.rs, windows/driver.rs, windows_init/policy.rs, managed.rs, workspace/mod.rs, companion/process.rs); not yet compiled
- D+B: cargo check lib ok; cargo test --lib -- sandbox:: builtin_tools::browser_tools::companion => 557 passed 0 failed
- A+C step1: app_container.rs refactored into RAII AppContainerProfile/ProcAttributeList (+grant_workspace, sid_string); re-exported via windows_init (build_command_line too). fixed driver run() reason string. not compiled
- A+C step2: wrote src/sandbox/platforms/windows/launcher.rs (WindowsLaunchApi<B: ConfinementBackend>, launch(), WindowsManagedProcess, tests w/ RecordingBackend); job.rs Sync; native::SuspendedProcess::exit_code. not registered/compiled yet. next: wfp.rs, mod.rs, driver spawn_managed, HostFacts
- A+C+E: wfp.rs written, Cargo features added, driver spawn_managed override + managed_launch_inputs + tests, HostFacts current_build launcher/identity true, blocker tests -> 3. compiling next
- STOPPED (migration) state:
  - D+B: done, verified earlier: cargo test --lib sandbox:: + companion => 557 passed / 0 failed.
  - A+C+E code written: app_container.rs (AppContainerProfile/ProcAttributeList extraction), windows_init/imp/mod.rs + windows_init/mod.rs re-exports, job.rs (Sync), managed_launch.rs (SuspendedProcess::exit_code, HostFacts current_build production_launcher/wrapper_reports_service_identity=true, blocker describe + tests -> 3 blockers), NEW launcher.rs, NEW wfp.rs (WfpBackend + #[ignore] admin_tests), windows/mod.rs (mod launcher; mod wfp;), Cargo.toml (+Win32_NetworkManagement_WindowsFilteringPlatform, +Win32_System_Rpc), driver.rs (spawn_managed override, managed_launch_inputs, MANAGED_CONFIRM_TIMEOUT, tests), workspace/mod.rs windows test assertion (!wrapper PID).
  - Last `cargo check -p alephcore --lib --tests --offline`: 0 errors, 2 dead-code warnings; both fixed afterwards (launcher.rs identity() now #[cfg(test)]; removed unused AppContainerProfile::sid()) — fix NOT yet re-checked.
  - No half-finished edits. No cargo process running from this agent.
  - NOT run yet: windows:: / managed / companion tests after A+C+E, clippy -D warnings, bin check, rustfmt --check, admin probe.
  - Admin command (elevated PowerShell, worktree): cargo test -p alephcore --lib --offline sandbox::platforms::windows::wfp::admin_tests -- --ignored --nocapture --test-threads=1

## Resume session (review + fixes, pre-cargo)
- FIX wfp.rs: ALE_AUTH_LISTEN_V4/V6 do not offer FWPM_CONDITION_IP_PROTOCOL (MS "Filtering conditions available at each filtering layer"); add_filter now omits it on listen layers (`layer_has_protocol_condition`), decode requires it exactly where it exists; decode rejects a duplicate ALE_PACKAGE_ID condition.
- FIX fail-open: managed_launch abort()/teardown() dropped `net` on unconfirmed-empty paths (dynamic WFP session close = filters removed). New trait method `ManagedLaunchApi::retain_confinement(Option<Net>)`; launcher impl forgets net + sets `retained`; launcher::launch forgets the api (profile/ACEs) when retained.
- FIX pipe(): both ends created non-inheritable, only child end flipped inheritable (parent end never inheritable).
- HostFacts::current_build production_launcher / wrapper_reports_service_identity restored to FALSE: the only proof is the real-AppContainer launcher tests, which call CreateAppContainerProfile (HKCU + %LOCALAPPDATA%/Packages = user-profile mutation, forbidden this session). Those 4 tests now #[ignore]. Blocker texts updated; preflight now 5 blockers (Some(false)).
- admin probe: added internetClient baselines; `block_filter_demonstrated` only when non-loopback reachable without WFP and blocked with it.
- `cargo check -p alephcore --lib --tests --offline` (JOBS=1, DEBUG=0): Finished in 5m19s, 0 warnings/0 errors (grep of ^warning/^error empty).
- STOP: lead reported memory gate BLOCK (available_bytes=3330764800 < 4294967296). No cargo runner of mine was active (tasklist: only aleph-server PID2700). Next blocked command: targeted `cargo test -p alephcore --lib --offline sandbox::platforms::windows:: -- --test-threads=1`. Recheck after stop showed 12138446848 ALLOW, but no further cargo started pending lead re-authorization.
- Re-audit (static, before tests): selected modules sandbox::platforms::windows::{driver,job,elevation,launcher,managed_launch,wfp}, sandbox::managed, builtin_tools::browser_tools::companion::* contain no home_dir/USERPROFILE/HOME/.aleph/keyring/SQLite/vault/remove_dir_all/set_var/fs writes. Real-process tests use host-token ping.exe/cmd (job.rs, managed_launch native_tests, managed.rs) only. launcher.rs non-ignored tests: environment_block (pure) + current_capabilities_refuse (planning refuses before create_job/profile). driver test: preflight refuses before spawn. AppContainer launcher tests (4) and wfp admin_tests are #[ignore] and NOT run.
- Verification (all CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=0 --offline, memory gate ALLOW before each, no overlap):
  - `cargo test -p alephcore --lib sandbox::platforms::windows:: -- --test-threads=1`: 41 passed, 0 failed, 5 ignored (4 launcher AppContainer tests + wfp admin probe). Re-run after rustfmt: 41/0/5.
  - `cargo test -p alephcore --lib sandbox::managed -- --test-threads=1`: 2 passed, 0 failed.
  - `cargo test -p alephcore --lib builtin_tools::browser_tools::companion -- --test-threads=1`: 36 passed, 0 failed.
  - `cargo check --bin aleph-server`: Finished 4m31s, 0 warnings.
  - `cargo clippy -p alephcore --lib --no-deps -- -D warnings`: Finished 4m45s, clean (re-run after rustfmt below).
  - `rustfmt --edition 2021` applied to wfp.rs, launcher.rs, managed_launch.rs, driver.rs (all diffs were in this workstream's code; HEAD driver.rs was fmt-clean); `--check` now exit 0.
- NOT run: AppContainer launcher tests, WFP admin probe, Native, E2E. HostFacts launcher/identity false; CURRENT_CAPABILITIES all false.
  - post-rustfmt clippy -D warnings: Finished 1m31s, clean. All my cargo runs finished; no runner active.
