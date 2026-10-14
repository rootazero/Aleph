# Windows managed admin session — handoff checklist (Task #53 / #48)

Status: **PLAN ONLY. Nothing here was executed.** No Cargo, elevation, WFP, AppContainer, process, keyring, registry or browser action was taken while writing this. Sources: `src/sandbox/platforms/windows/{managed_launch,driver,elevation,job}.rs`, `windows-managed-{driver-stage,blockers-run,job-probe,job-tighten-probe}.md`, `docs/reference/{PROCESS_MANAGEMENT,REDLINES}.md`.

## 1. What production still lacks (admin rights do NOT finish #53)
`managed_launch.rs` is a contract + fake-API tests only. `HostFacts::current_build` hard-codes `production_launcher=false`, `wrapper_reports_service_identity=false`, `CURRENT_CAPABILITIES` all false; `managed_spawn_preflight` always returns `UnsupportedPolicy`. Missing code (none exists):
1. A real `impl ManagedLaunchApi` (CreateProcessAsUserW `CREATE_SUSPENDED`, `GetProcessTimes`, `AssignProcessToJobObject`/`IsProcessInJob`, `ResumeThread`, bounded terminate-and-confirm-empty).
2. Per-run WFP module: no `Win32_NetworkManagement_WindowsFilteringPlatform` / `Win32_System_Rpc` windows-sys features, no `Fwpm*` FFI, no random per-run SID/provider/sublayer GUIDs, no readback comparator.
3. `sandbox-init-windows` handoff (or parent-side creation): wrapper PID is not the service PID.
4. Leftover-filter recovery story (persistent filters survive a crash).
Admin only lets an experiment *measure* arbitration (item 2); it writes none of 1-4. Flip `CURRENT_CAPABILITIES` only after evidence below exists AND the code is compiled/tested.

## 2. Pass criteria the session must prove (each needs logged evidence)
| # | Claim | Evidence required |
|---|---|---|
| P1 | Exact per-run SID confinement, no global loopback exemption | Filters keyed to a fresh random package/user SID only; `CheckNetIsolation LoopbackExempt -s` (or equivalent) identical before/after; unconfined host process and a second SID unaffected |
| P2 | Suspended service identity | Handle-derived PID + creation FILETIME recorded before resume; `IsProcessInJob==true` before `ResumeThread`; PID/FILETIME re-read after teardown start matches |
| P3 | Default-deny other hosts | Connect to LAN IP, public IP, other loopback port, UDP, `::1` other port all denied |
| P4 | Exact loopback port permitted | CONNECT and LISTEN/ACCEPT on allowed port succeed on 127.0.0.1 and ::1 |
| P5 | Child/fork refused | Service's CreateProcess fails (os error 1816 with ActiveProcessLimit, or equivalent) while wrapper-slot caveat from `windows-managed-job-probe.md` §5.2 is settled; `conhost.exe` filtered from counts |
| P6 | Teardown order | Job `ActiveProcesses==0` confirmed BEFORE `FwpmFilterDeleteByKey0`; then sublayer, provider, profile deleted; re-enumeration shows zero leftovers; kill-mid-run recovery works |
| P7 | Arbitration | Whether WFP permit beats AppContainer loopback block; if not, whether a per-SID loopback exemption is needed (product/installer decision, not a silent gateway action) |

## 3. Session safety rules
- Disposable VM snapshot only; never the dev host, never the real `.aleph`, no HOME/USERPROFILE use, no user-profile enumeration.
- Operator elevates manually; tools never launch UAC or `RunAs`.
- Everything created carries a unique random provider GUID and is deleted in a `finally`/emergency script; take `netsh wfp show state` snapshot only inside the VM, to a VM-local path, before and after.
- Abort on first unexpected leftover; do not retry blindly (3-attempt cap, see AGENTS.md).

## 4. Order of operations in the session
1. `powershell -NoProfile -File qa/browser_managed/windows/preflight.ps1` (read-only; must exit 0).
2. Baseline: WFP state + loopback-exempt list snapshots.
3. Run fixtures per §5 (create AppContainer profile -> suspended `net_fixture` -> record identity -> job assign -> WFP transaction -> readback -> resume -> matrix P3/P4/P5 -> terminate -> confirm empty -> remove filters -> re-enumerate).
4. Kill-mid-run test (P6 recovery), then emergency cleanup.
5. After snapshot diff must equal baseline. Save logs under `docs/reports/verify-20261007/`.

## 5. Prerequisite executables/fixtures that MUST be written first (no command is given for them because they do not exist)
- `qa/browser_managed/windows/wfp_probe.exe` — opt-in, refuses unless elevated and given an explicit `--i-am-in-a-disposable-vm` flag; implements random GUIDs, transaction add of provider/sublayer/filters for `ALE_AUTH_CONNECT/LISTEN/RECV_ACCEPT` V4+V6, `FwpmFilterGetByKey0` readback+compare, delete-by-key teardown, emergency `--cleanup <provider-guid>`. Needs the two windows-sys features above, built outside the production crate or behind a non-default flag (R3, R1 exception only covers `src/sandbox/*`).
- `qa/browser_managed/windows/net_fixture.exe` — small TCP/UDP client+listener that accepts target list and prints per-target result; can fork once to test P5.
- A harness (script or the wfp_probe itself) for suspended-create + FILETIME + job membership + ordered teardown (reusing `.analysis/browser-live-runtime/windows-job-probe/job_probe.rs` ideas; `.analysis` is untracked scratch).
- Optional `qa/browser_managed/windows/collect_snapshots.ps1` for before/after state diffs (to be written; not provided).

## 6. Preflight script scope
`qa/browser_managed/windows/preflight.ps1` is read-only: own-token admin/integrity check, BFE state, OS, hypervisor/free RAM, leftover-process names, fixture presence. It does not elevate, enumerate profiles/registry/keyring, or touch `.aleph`. It was only syntax-parsed, not run. Passing it is necessary, not sufficient.

## 7. Not done
No production edits, no tests, no Cargo, no fixtures, no WFP code, no run of the preflight, no commit.
