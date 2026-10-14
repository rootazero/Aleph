// Windows anonymous Job Object DYNAMIC ActiveProcessLimit tighten (2 -> 1) probe.
// std-only + raw kernel32 FFI. NOT sandbox-init, NOT AppContainer, NOT production spawn-suspended barrier.
// Modes: (none)=parent | wrapper | service | child.
// The controlled stdin barrier (service blocks on an inherited stdin pipe the PARENT owns) is a test device,
// NOT a model of malicious-child admission.
#![allow(non_snake_case, clippy::upper_case_acronyms)]
use std::ffi::c_void;
use std::fs;
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type HANDLE = *mut c_void;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const JOB_LIMIT_ACTIVE_PROCESS: u32 = 0x8;
const JOB_LIMIT_KILL_ON_CLOSE: u32 = 0x2000;
const JOB_CLASS_BASIC_ACCOUNTING: u32 = 1;
const JOB_CLASS_EXT_LIMIT: u32 = 9;
const JOB_CLASS_PID_LIST: u32 = 3;
const PROCESS_TERMINATE: u32 = 0x1;
const PROCESS_QUERY_LIMITED: u32 = 0x1000;
const SYNCHRONIZE: u32 = 0x0010_0000;
const STILL_ACTIVE: u32 = 259;
const WAIT_TIMEOUT: u32 = 0x102;
const OVERALL_DEADLINE_SECS: u64 = 150;
const TEMP_PREFIX: &str = "aleph-jobtighten-";

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct BasicLimit {
    PerProcessUserTimeLimit: i64,
    PerJobUserTimeLimit: i64,
    LimitFlags: u32,
    MinimumWorkingSetSize: usize,
    MaximumWorkingSetSize: usize,
    ActiveProcessLimit: u32,
    Affinity: usize,
    PriorityClass: u32,
    SchedulingClass: u32,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct IoCounters {
    a: u64,
    b: u64,
    c: u64,
    d: u64,
    e: u64,
    f: u64,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ExtLimit {
    Basic: BasicLimit,
    Io: IoCounters,
    ProcessMemoryLimit: usize,
    JobMemoryLimit: usize,
    PeakProcessMemoryUsed: usize,
    PeakJobMemoryUsed: usize,
}
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Accounting {
    TotalUserTime: i64,
    TotalKernelTime: i64,
    ThisPeriodTotalUserTime: i64,
    ThisPeriodTotalKernelTime: i64,
    TotalPageFaultCount: u32,
    TotalProcesses: u32,
    ActiveProcesses: u32,
    TotalTerminatedProcesses: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> HANDLE;
    fn SetInformationJobObject(job: HANDLE, class: u32, info: *const c_void, len: u32) -> i32;
    fn QueryInformationJobObject(job: HANDLE, class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
    fn AssignProcessToJobObject(job: HANDLE, proc_: HANDLE) -> i32;
    fn IsProcessInJob(proc_: HANDLE, job: HANDLE, result: *mut i32) -> i32;
    fn TerminateJobObject(job: HANDLE, code: u32) -> i32;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> HANDLE;
    fn GetExitCodeProcess(p: HANDLE, code: *mut u32) -> i32;
    fn WaitForSingleObject(h: HANDLE, ms: u32) -> u32;
    fn TerminateProcess(p: HANDLE, code: u32) -> i32;
    fn CloseHandle(h: HANDLE) -> i32;
    fn GetCurrentProcess() -> HANDLE;
    fn QueryFullProcessImageNameW(p: HANDLE, flags: u32, buf: *mut u16, len: *mut u32) -> i32;
}

fn oserr() -> String {
    let e = std::io::Error::last_os_error();
    format!("os_error={} ({})", e.raw_os_error().unwrap_or(-1), e)
}
fn io_desc(e: &std::io::Error) -> String {
    format!("os_error={} ({})", e.raw_os_error().unwrap_or(-1), e)
}

/// UTC wall clock "YYYY-MM-DDTHH:MM:SSZ" from SystemTime (civil-from-days).
fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

fn put(root: &Path, name: &str, body: &str) {
    let tmp = root.join(format!("{name}.tmp"));
    fs::write(&tmp, body).expect("write tmp");
    fs::rename(&tmp, root.join(name)).expect("rename");
}
fn wait_file(root: &Path, name: &str, secs: u64) -> Option<String> {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if let Ok(s) = fs::read_to_string(root.join(name)) {
            return Some(s);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// One bounded spawn attempt of a trivial `child` process (immediate exit). Literal observation.
fn attempt_spawn(exe: &Path) -> String {
    match Command::new(exe)
        .arg("child")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Err(e) => format!("REFUSED {}", io_desc(&e)),
        Ok(mut c) => {
            let pid = c.id();
            let end = Instant::now() + Duration::from_secs(3);
            loop {
                match c.try_wait() {
                    Ok(Some(st)) => return format!("SPAWNED pid={pid} exit_code={:#x}", st.code().unwrap_or(-1) as u32),
                    Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(30)),
                    _ => {
                        let _ = c.kill();
                        let _ = c.wait();
                        return format!("SPAWNED pid={pid} still-running-after-3s(killed)");
                    }
                }
            }
        }
    }
}

fn mode_child() {
    std::process::exit(0);
}

/// Read one 3-byte line ("S1\n"/"S2\n") from the inherited stdin pipe (parent holds the write end).
fn read_cmd() -> Option<[u8; 3]> {
    let mut b = [0u8; 3];
    std::io::stdin().read_exact(&mut b).ok().map(|_| b)
}

/// Service: announces pid, then blocks on controlled stdin (S1 -> attempt 1, S2 -> attempt 2).
fn mode_service(root: &Path) {
    let exe = std::env::current_exe().unwrap();
    put(root, "service-pid", &std::process::id().to_string());
    match read_cmd() {
        Some(b) if &b == b"S1\n" => put(root, "svc-attempt1", &attempt_spawn(&exe)),
        _ => {
            put(root, "svc-attempt1", "NO-S1 (stdin EOF or bad command)");
            return;
        }
    }
    match read_cmd() {
        Some(b) if &b == b"S2\n" => put(root, "svc-attempt2", &attempt_spawn(&exe)),
        _ => put(root, "svc-attempt2", "NO-S2 (stdin EOF or bad command)"),
    }
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end && !root.join("stop").exists() {
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Wrapper: waits GO (parent assigns job first), spawns service (inherits the parent-owned stdin pipe),
/// stays alive until the parent drops file `wrapper-exit` (bounded), then exits.
fn mode_wrapper(root: &Path) {
    let exe = std::env::current_exe().unwrap();
    put(root, "wrapper-ready", &std::process::id().to_string());
    let mut line = [0u8; 3];
    let _ = std::io::stdin().read_exact(&mut line);
    if &line != b"GO\n" {
        put(root, "wrapper-result", "NO-GO");
        return;
    }
    match Command::new(&exe)
        .args(["service", &root.to_string_lossy()])
        .stdin(Stdio::inherit())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Err(e) => put(root, "wrapper-result", &format!("service-spawn: REFUSED {}", io_desc(&e))),
        Ok(svc) => {
            put(root, "wrapper-result", &format!("service-spawn: OK pid={}", svc.id()));
            let _ = wait_file(root, "wrapper-exit", 90);
            put(root, "wrapper-done", "wrapper-exiting");
            // `svc` dropped without wait/kill: Child drop does not kill.
        }
    }
}

struct Sc {
    job: HANDLE,
    wrapper: Option<Child>,
    svc: HANDLE,
}
impl Sc {
    /// TerminateJobObject, then bounded wait for owned handles; close job last (KILL_ON_JOB_CLOSE backstop).
    fn terminate_and_wait(&mut self) {
        unsafe {
            TerminateJobObject(self.job, 0xDEAD);
            if !self.svc.is_null() {
                WaitForSingleObject(self.svc, 5000);
                CloseHandle(self.svc);
                self.svc = std::ptr::null_mut();
            }
        }
        if let Some(w) = self.wrapper.as_mut() {
            let wh = w.as_raw_handle() as HANDLE;
            unsafe { WaitForSingleObject(wh, 5000) };
            let _ = w.try_wait();
        }
    }
}
impl Drop for Sc {
    fn drop(&mut self) {
        self.terminate_and_wait();
        if !self.job.is_null() {
            unsafe { CloseHandle(self.job) };
            self.job = std::ptr::null_mut();
        }
    }
}

fn acct(job: HANDLE) -> Accounting {
    let mut a = Accounting::default();
    let mut ret = 0u32;
    let ok = unsafe {
        QueryInformationJobObject(job, JOB_CLASS_BASIC_ACCOUNTING, &mut a as *mut _ as *mut c_void, std::mem::size_of::<Accounting>() as u32, &mut ret)
    };
    assert!(ok != 0, "QueryInformationJobObject(accounting) {}", oserr());
    a
}
fn query_ext(job: HANDLE) -> Result<ExtLimit, String> {
    let mut back = ExtLimit::default();
    let mut ret = 0u32;
    let ok = unsafe {
        QueryInformationJobObject(job, JOB_CLASS_EXT_LIMIT, &mut back as *mut _ as *mut c_void, std::mem::size_of::<ExtLimit>() as u32, &mut ret)
    };
    if ok == 0 {
        Err(oserr())
    } else {
        Ok(back)
    }
}
fn set_ext(job: HANDLE, ext: &ExtLimit) -> Result<(), String> {
    let ok = unsafe { SetInformationJobObject(job, JOB_CLASS_EXT_LIMIT, ext as *const _ as *const c_void, std::mem::size_of::<ExtLimit>() as u32) };
    if ok == 0 {
        Err(oserr())
    } else {
        Ok(())
    }
}

/// Job members as "pid:image" via JobObjectBasicProcessIdList.
fn members(job: HANDLE) -> Vec<String> {
    let mut buf = [0usize; 34];
    let mut ret = 0u32;
    let ok = unsafe { QueryInformationJobObject(job, JOB_CLASS_PID_LIST, buf.as_mut_ptr() as *mut c_void, (buf.len() * 8) as u32, &mut ret) };
    if ok == 0 {
        return vec![oserr()];
    }
    let n = (buf[0] >> 32) as usize;
    let mut out = vec![];
    for i in 0..n.min(32) {
        let pid = buf[1 + i] as u32;
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED, 0, pid) };
        let mut name = String::from("?");
        if !h.is_null() {
            let mut w = [0u16; 520];
            let mut l = w.len() as u32;
            if unsafe { QueryFullProcessImageNameW(h, 0, w.as_mut_ptr(), &mut l) } != 0 {
                let full = String::from_utf16_lossy(&w[..l as usize]);
                name = full.rsplit(char::from(92u8)).next().unwrap_or("?").to_string();
            }
            unsafe { CloseHandle(h) };
        }
        out.push(format!("{pid}:{name}"));
    }
    out
}
fn non_conhost(m: &[String]) -> usize {
    m.iter().filter(|x| !x.to_ascii_lowercase().ends_with(":conhost.exe")).count()
}
fn in_job(p: HANDLE, job: HANDLE) -> bool {
    let mut r = 0i32;
    assert!(unsafe { IsProcessInJob(p, job, &mut r) } != 0, "IsProcessInJob {}", oserr());
    r != 0
}
/// "alive" | "exited(code)" for an owned handle.
fn liveness(h: HANDLE) -> String {
    let w = unsafe { WaitForSingleObject(h, 0) };
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(h, &mut code) };
    if w == WAIT_TIMEOUT && code == STILL_ACTIVE {
        "alive".into()
    } else {
        format!("NOT-ALIVE(wait={w:#x} exit_code={code:#x})")
    }
}
fn is_alive(h: HANDLE) -> bool {
    liveness(h) == "alive"
}

struct Report {
    fails: u32,
    mismatches: u32,
}
impl Report {
    fn say(&mut self, s: impl Into<String>) {
        println!("{}", s.into());
    }
    /// Mechanical precondition/assertion: counts as FAIL when false.
    fn check(&mut self, label: &str, ok: bool, detail: &str) {
        if !ok {
            self.fails += 1;
        }
        self.say(format!("  [{}] {label} :: {detail}", if ok { "PASS" } else { "FAIL" }));
    }
    /// Empirical observation: literal outcome; `expected` is only the main's hypothesis, a mismatch is reported, not hidden.
    fn observe(&mut self, label: &str, expected: &str, matches: bool, literal: &str) {
        if !matches {
            self.mismatches += 1;
        }
        self.say(format!("  [OBSERVED{}] {label} :: {literal}  (hypothesis: {expected})", if matches { "" } else { " !=HYPOTHESIS" }));
    }
}

fn parent() -> i32 {
    let exe = std::env::current_exe().unwrap();
    let tmp = std::env::temp_dir();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let base = tmp.join(format!("{TEMP_PREFIX}{}-{nanos}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    let root = base.join("T21");
    fs::create_dir_all(&root).unwrap();
    let mut r = Report { fails: 0, mismatches: 0 };
    r.say(format!("probe start_utc={} exe={} pid={} base={}", utc_now(), exe.display(), std::process::id(), base.display()));
    r.say("SCOPE: anonymous Job Object dynamic ActiveProcessLimit 2->1 with std surrogate wrapper+service (NORMAL, non-AppContainer); NOT sandbox-init, NOT AppContainer, NOT production spawn-suspended barrier; controlled stdin barrier != malicious child admission.");

    // Global watchdog: hard 150 s bound over the whole run. Handle passed as usize (kernel handles are process-wide).
    let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
    assert!(!job.is_null(), "CreateJobObjectW {}", oserr());
    {
        let jh = job as usize;
        let wd_base = base.clone();
        let wd_tmp = tmp.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(OVERALL_DEADLINE_SECS));
            eprintln!("WATCHDOG: overall {OVERALL_DEADLINE_SECS}s deadline hit; terminating job and exiting");
            println!("WATCHDOG: overall {OVERALL_DEADLINE_SECS}s deadline hit; RESULT: ABORTED");
            unsafe { TerminateJobObject(jh as HANDLE, 0xDEAD) };
            std::thread::sleep(Duration::from_millis(500));
            if wd_base.parent() == Some(wd_tmp.as_path())
                && wd_base.file_name().is_some_and(|n| n.to_string_lossy().starts_with(TEMP_PREFIX))
            {
                let _ = fs::remove_dir_all(&wd_base);
            }
            std::process::exit(3);
        });
    }

    let mut ext = ExtLimit::default();
    ext.Basic.LimitFlags = JOB_LIMIT_ACTIVE_PROCESS | JOB_LIMIT_KILL_ON_CLOSE;
    ext.Basic.ActiveProcessLimit = 2;
    let set0 = set_ext(job, &ext);
    r.check("initial SetInformationJobObject(limit=2, KILL_ON_JOB_CLOSE)", set0.is_ok(), &format!("{set0:?}"));
    let mut sc = Sc { job, wrapper: None, svc: std::ptr::null_mut() };
    if set0.is_err() {
        return finish(r, base, tmp, sc);
    }
    match query_ext(job) {
        Ok(b) => r.check(
            "initial limit read-back",
            b.Basic.ActiveProcessLimit == 2 && b.Basic.LimitFlags & JOB_LIMIT_KILL_ON_CLOSE != 0,
            &format!("ActiveProcessLimit={} flags={:#x}", b.Basic.ActiveProcessLimit, b.Basic.LimitFlags),
        ),
        Err(e) => r.check("initial limit read-back", false, &e),
    }

    // Wrapper: NO_WINDOW, stdin piped (parent keeps write end), assigned to job BEFORE GO.
    let w = Command::new(&exe)
        .args(["wrapper", &root.to_string_lossy()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("spawn wrapper");
    sc.wrapper = Some(w);
    let ready = wait_file(&root, "wrapper-ready", 10);
    r.check("wrapper blocked pre-assignment (ready, no GO yet)", ready.is_some(), &format!("wrapper pid={ready:?}"));
    let wh = sc.wrapper.as_ref().unwrap().as_raw_handle() as HANDLE;
    r.check("wrapper NOT in job before assign", !in_job(wh, job), "IsProcessInJob=false");
    r.check("wrapper not yet spawned anything", !root.join("wrapper-result").exists(), "no wrapper-result before GO");
    let ok = unsafe { AssignProcessToJobObject(job, wh) };
    r.check("AssignProcessToJobObject(wrapper)", ok != 0, &if ok != 0 { "ok".into() } else { oserr() });
    r.check("wrapper in job after assign", in_job(wh, job), &format!("Active={}", acct(job).ActiveProcesses));
    let me = unsafe { GetCurrentProcess() };
    r.check("parent (probe) NOT in job", !in_job(me, job), "IsProcessInJob(self)=false");
    sc.wrapper.as_mut().unwrap().stdin.as_mut().unwrap().write_all(b"GO\n").expect("GO");

    // Service started by wrapper in controlled stdin WAIT mode; confirm exact handle in job, both alive.
    let wr = wait_file(&root, "wrapper-result", 15);
    r.check("wrapper spawned service (slot 2)", wr.as_deref().is_some_and(|s| s.contains("OK")), wr.as_deref().unwrap_or("timeout"));
    let svc_pid: Option<u32> = wait_file(&root, "service-pid", 10).and_then(|s| s.trim().parse().ok());
    let Some(svc_pid) = svc_pid else {
        r.check("service started", false, "no service-pid");
        return finish(r, base, tmp, sc);
    };
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED | SYNCHRONIZE | PROCESS_TERMINATE, 0, svc_pid) };
    r.check("OpenProcess(service pid, QUERY_LIMITED|SYNCHRONIZE|TERMINATE)", !h.is_null(), &if h.is_null() { oserr() } else { format!("pid={svc_pid} handle ok") });
    if h.is_null() {
        return finish(r, base, tmp, sc);
    }
    sc.svc = h;
    let svc_in_job = in_job(h, job);
    r.check("exact service handle IsProcessInJob", svc_in_job, "member of parent-owned job");
    let m_pre = members(job);
    let a_pre = acct(job);
    r.say(format!("  pre-tighten: members={:?} controlled(non-conhost)={} Active={} Total={}", m_pre, non_conhost(&m_pre), a_pre.ActiveProcesses, a_pre.TotalProcesses));
    r.check("pre-tighten: wrapper alive", is_alive(wh), &liveness(wh));
    r.check("pre-tighten: service alive", is_alive(h), &liveness(h));
    r.check("pre-tighten: exactly 2 controlled members (wrapper+service)", non_conhost(&m_pre) == 2, &format!("{m_pre:?}"));
    r.check("service blocked in WAIT (no svc-attempt1 yet)", !root.join("svc-attempt1").exists(), "svc-attempt1 absent");

    // ---- Dynamic tighten 2 -> 1: Query then Set, preserving flags (KILL_ON_JOB_CLOSE) and all other fields.
    let before = query_ext(job);
    r.say(format!("  QueryInformationJobObject(ext) before tighten: {}", match &before {
        Ok(b) => format!("OK ActiveProcessLimit={} flags={:#x}", b.Basic.ActiveProcessLimit, b.Basic.LimitFlags),
        Err(e) => format!("ERR {e}"),
    }));
    let Ok(mut tight) = before else {
        r.check("query before tighten", false, "query failed; cannot preserve flags");
        return finish(r, base, tmp, sc);
    };
    tight.Basic.ActiveProcessLimit = 1;
    tight.Basic.LimitFlags |= JOB_LIMIT_ACTIVE_PROCESS | JOB_LIMIT_KILL_ON_CLOSE;
    let t0 = Instant::now();
    let set1 = set_ext(job, &tight);
    r.say(format!("  SetInformationJobObject(ActiveProcessLimit=1, flags={:#x}) -> {} ({} ms)", tight.Basic.LimitFlags, match &set1 { Ok(()) => "returned nonzero (success)".to_string(), Err(e) => format!("RETURNED 0, {e}") }, t0.elapsed().as_millis()));
    let rb = query_ext(job);
    r.say(format!("  read-back after tighten attempt: {}", match &rb {
        Ok(b) => format!("ActiveProcessLimit={} flags={:#x} (KILL_ON_CLOSE preserved={})", b.Basic.ActiveProcessLimit, b.Basic.LimitFlags, b.Basic.LimitFlags & JOB_LIMIT_KILL_ON_CLOSE != 0),
        Err(e) => format!("ERR {e}"),
    }));
    std::thread::sleep(Duration::from_millis(500));
    let wl = liveness(wh);
    let sl = liveness(h);
    let a_post = acct(job);
    let m_post = members(job);
    r.say(format!("  post-tighten: wrapper={wl} service={sl} Active={} Total={} TotalTerminated={} members={:?} controlled={}", a_post.ActiveProcesses, a_post.TotalProcesses, a_post.TotalTerminatedProcesses, m_post, non_conhost(&m_post)));
    r.observe("tighten Set 2->1 while 2 members live accepted", "success or refusal both possible; literal recorded", set1.is_ok(), &format!("{set1:?}"));

    if set1.is_ok() && wl == "alive" && sl == "alive" {
        // Release the held service for its FIRST child attempt (wrapper still live, active=2 > limit=1).
        sc.wrapper.as_mut().unwrap().stdin.as_mut().unwrap().write_all(b"S1\n").expect("S1");
        let a1 = wait_file(&root, "svc-attempt1", 15);
        let a1s = a1.as_deref().unwrap_or("timeout").to_string();
        r.observe("attempt1 (wrapper+service live, limit=1)", "quota refusal", a1s.contains("REFUSED"), &a1s);
        r.say(format!("  after attempt1: Active={} TotalProcesses={} members={:?}", acct(job).ActiveProcesses, acct(job).TotalProcesses, members(job)));

        // Wrapper exits while parent keeps job.
        put(&root, "wrapper-exit", "1");
        let w = unsafe { WaitForSingleObject(wh, 10000) };
        r.check("wrapper exited (parent still owns job)", w == 0, &format!("wait={w:#x}"));
        std::thread::sleep(Duration::from_millis(300));
        let m2 = members(job);
        let a2 = acct(job);
        r.say(format!("  after wrapper exit: members={m2:?} controlled={} Active={} Total={} service={}", non_conhost(&m2), a2.ActiveProcesses, a2.TotalProcesses, liveness(h)));
        r.check("service still alive after wrapper exit", is_alive(h), &liveness(h));
        r.check("exactly 1 controlled member (service) after wrapper exit", non_conhost(&m2) == 1, &format!("{m2:?}"));

        // Held service attempts second child with quota=1 and 1 controlled member (itself).
        sc.wrapper.as_mut().unwrap().stdin.as_mut().unwrap().write_all(b"S2\n").expect("S2");
        let a2r = wait_file(&root, "svc-attempt2", 15);
        let a2s = a2r.as_deref().unwrap_or("timeout").to_string();
        r.observe("attempt2 (wrapper gone, service only, limit=1)", "quota refusal", a2s.contains("REFUSED"), &a2s);
        let a3 = acct(job);
        r.say(format!("  final accounting: Total={} Active={} TotalTerminated={} members={:?}", a3.TotalProcesses, a3.ActiveProcesses, a3.TotalTerminatedProcesses, members(job)));
    } else {
        r.say("  SKIPPED attempts: tighten did not succeed with both processes surviving (literal outcome above; no fake pass).");
    }
    finish(r, base, tmp, sc)
}

fn finish(mut r: Report, base: PathBuf, tmp: PathBuf, mut sc: Sc) -> i32 {
    sc.terminate_and_wait();
    let a = acct(sc.job);
    r.say(format!("  post-TerminateJobObject: Active={} Total={} TotalTerminated={}", a.ActiveProcesses, a.TotalProcesses, a.TotalTerminatedProcesses));
    drop(sc); // closes job handle
    // Owned cleanup: containment assertion, then only the files this probe created.
    let contained = base.parent() == Some(tmp.as_path())
        && base.file_name().is_some_and(|n| n.to_string_lossy().starts_with(TEMP_PREFIX))
        && !fs::symlink_metadata(&base).map(|m| m.file_type().is_symlink()).unwrap_or(true);
    if contained {
        let _ = fs::remove_dir_all(&base);
    }
    r.say(format!("cleanup: contained={contained} fixture root removed={} exists_now={}", contained && !base.exists(), base.exists()));
    r.say(format!("end_utc={}", utc_now()));
    r.say(format!(
        "RESULT: {} (mechanical_fails={}, hypothesis_mismatches={})",
        if r.fails == 0 { "MECHANICS OK" } else { "MECHANICAL FAILURES" },
        r.fails,
        r.mismatches
    ));
    if r.fails == 0 {
        0
    } else {
        1
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("child") => mode_child(),
        Some("service") => mode_service(Path::new(&a[2])),
        Some("wrapper") => mode_wrapper(Path::new(&a[2])),
        _ => std::process::exit(parent()),
    }
}
