// Windows anonymous Job Object ActiveProcessLimit probe. std-only + raw kernel32 FFI.
// NOT sandbox-init, NOT AppContainer. Modes: (none)=parent | wrapper | service | child.
#![allow(non_snake_case, clippy::upper_case_acronyms)]
use std::ffi::c_void;
use std::fs;
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FileTime {
    lo: u32,
    hi: u32,
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
    fn GetProcessTimes(p: HANDLE, c: *mut FileTime, e: *mut FileTime, k: *mut FileTime, u: *mut FileTime) -> i32;
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

/// One bounded spawn attempt of a trivial `child` process (immediate exit). Returns a literal observation.
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
                    Ok(Some(st)) => {
                        return format!("SPAWNED pid={pid} exit_code={:#x}", st.code().unwrap_or(-1) as u32)
                    }
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

fn mode_service(root: &Path, variant: &str) {
    let exe = std::env::current_exe().unwrap();
    put(root, "service-pid", &std::process::id().to_string());
    put(root, "svc-attempt1", &attempt_spawn(&exe));
    if variant == "L2B" && wait_file(root, "go-after", 20).is_some() {
        put(root, "svc-attempt2", &attempt_spawn(&exe));
    }
    // stay alive (bounded) so the parent can query/terminate it via its owned handle
    let end = Instant::now() + Duration::from_secs(25);
    while Instant::now() < end && !root.join("stop").exists() {
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn mode_wrapper(root: &Path, sc: &str) {
    let exe = std::env::current_exe().unwrap();
    put(root, "wrapper-ready", &std::process::id().to_string());
    let mut line = [0u8; 3];
    let _ = std::io::stdin().read_exact(&mut line); // blocks until parent writes GO (after job assignment)
    if &line != b"GO\n" {
        put(root, "wrapper-result", "NO-GO");
        return;
    }
    if sc == "L1" {
        put(root, "wrapper-result", &format!("target: {}", attempt_spawn(&exe)));
        return;
    }
    match Command::new(&exe)
        .args(["service", &root.to_string_lossy(), sc])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Err(e) => {
            put(root, "wrapper-result", &format!("service-spawn: REFUSED {}", io_desc(&e)));
        }
        Ok(mut svc) => {
            put(root, "wrapper-result", &format!("service-spawn: OK pid={}", svc.id()));
            if sc == "L2A" {
                let end = Instant::now() + Duration::from_secs(25);
                while Instant::now() < end {
                    if let Ok(Some(_)) = svc.try_wait() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                if let Ok(None) = svc.try_wait() {
                    let _ = svc.kill();
                }
                put(root, "wrapper-done", "service-exited-or-killed");
            } else {
                // L2B: wrapper exits as soon as service finished its first attempt (service keeps running)
                let _ = wait_file(root, "svc-attempt1", 15);
                put(root, "wrapper-done", "wrapper-exiting-early");
            }
        }
    }
}

struct Sc {
    job: HANDLE,
    wrapper: Option<Child>,
    svc: HANDLE,
}
impl Drop for Sc {
    fn drop(&mut self) {
        unsafe {
            if !self.svc.is_null() {
                CloseHandle(self.svc);
            }
            TerminateJobObject(self.job, 0xDEAD);
            if let Some(w) = self.wrapper.as_mut() {
                let _ = w.wait();
            }
            CloseHandle(self.job); // KILL_ON_JOB_CLOSE as backstop
        }
    }
}
fn acct(job: HANDLE) -> Accounting {
    let mut a = Accounting::default();
    let mut ret = 0u32;
    let ok = unsafe {
        QueryInformationJobObject(
            job,
            JOB_CLASS_BASIC_ACCOUNTING,
            &mut a as *mut _ as *mut c_void,
            std::mem::size_of::<Accounting>() as u32,
            &mut ret,
        )
    };
    assert!(ok != 0, "QueryInformationJobObject(accounting) {}", oserr());
    a
}

/// Enumerate job members as "pid:image" (kernel-observed membership, via JobObjectBasicProcessIdList).
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
fn ft(f: FileTime) -> u64 {
    ((f.hi as u64) << 32) | f.lo as u64
}

struct Report {
    fails: u32,
}
impl Report {
    fn say(&mut self, s: impl Into<String>) {
        println!("{}", s.into());
    }
    fn check(&mut self, label: &str, ok: bool, detail: &str) {
        if !ok {
            self.fails += 1;
        }
        self.say(format!("  [{}] {label} :: {detail}", if ok { "PASS" } else { "FAIL" }));
    }
}

fn start(r: &mut Report, exe: &Path, base: &Path, name: &str, limit: u32) -> (Sc, PathBuf) {
    let root = base.join(name);
    fs::create_dir_all(&root).unwrap();
    r.say(format!("--- scenario {name}: ActiveProcessLimit={limit} + KILL_ON_JOB_CLOSE, root={}", root.display()));
    let job = unsafe { CreateJobObjectW(std::ptr::null_mut(), std::ptr::null()) };
    assert!(!job.is_null(), "CreateJobObjectW {}", oserr());
    let mut ext = ExtLimit::default();
    ext.Basic.LimitFlags = JOB_LIMIT_ACTIVE_PROCESS | JOB_LIMIT_KILL_ON_CLOSE;
    ext.Basic.ActiveProcessLimit = limit;
    let ok = unsafe {
        SetInformationJobObject(job, JOB_CLASS_EXT_LIMIT, &ext as *const _ as *const c_void, std::mem::size_of::<ExtLimit>() as u32)
    };
    assert!(ok != 0, "SetInformationJobObject {}", oserr());
    let mut back = ExtLimit::default();
    let mut ret = 0u32;
    assert!(
        unsafe {
            QueryInformationJobObject(job, JOB_CLASS_EXT_LIMIT, &mut back as *mut _ as *mut c_void, std::mem::size_of::<ExtLimit>() as u32, &mut ret)
        } != 0
    );
    r.check(
        "limit read-back",
        back.Basic.ActiveProcessLimit == limit && back.Basic.LimitFlags & JOB_LIMIT_KILL_ON_CLOSE != 0,
        &format!("ActiveProcessLimit={} flags={:#x}", back.Basic.ActiveProcessLimit, back.Basic.LimitFlags),
    );
    let mut sc = Sc { job, wrapper: None, svc: std::ptr::null_mut() };
    let w = Command::new(exe)
        .args(["wrapper", &root.to_string_lossy(), name])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("spawn wrapper");
    sc.wrapper = Some(w);
    let ready = wait_file(&root, "wrapper-ready", 10);
    r.check("wrapper blocked pre-assignment (ready, no GO yet)", ready.is_some(), &format!("wrapper pid={:?}", ready));
    let wh = sc.wrapper.as_ref().unwrap().as_raw_handle() as HANDLE;
    r.check("wrapper NOT in job before assign", !in_job(wh, job), "IsProcessInJob=false");
    r.check("wrapper not yet spawned anything", !root.join("wrapper-result").exists(), "no wrapper-result before GO");
    let ok = unsafe { AssignProcessToJobObject(job, wh) };
    r.check("AssignProcessToJobObject(wrapper)", ok != 0, &if ok != 0 { "ok".into() } else { oserr() });
    let a = acct(job);
    r.check("wrapper in job after assign", in_job(wh, job), &format!("Active={} Total={}", a.ActiveProcesses, a.TotalProcesses));
    let me = unsafe { GetCurrentProcess() };
    r.check("parent (probe) NOT in job", !in_job(me, job), "IsProcessInJob(self)=false");
    sc.wrapper.as_mut().unwrap().stdin.as_mut().unwrap().write_all(b"GO\n").expect("GO");
    (sc, root)
}

fn open_service(r: &mut Report, sc: &mut Sc, root: &Path) -> bool {
    let Some(pid_s) = wait_file(root, "service-pid", 10) else {
        r.check("service started", false, "no service-pid");
        return false;
    };
    let pid: u32 = pid_s.trim().parse().unwrap();
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED | SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
    r.check(
        "OpenProcess(service pid: QUERY_LIMITED|SYNCHRONIZE|TERMINATE) by parent",
        !h.is_null(),
        &if h.is_null() { oserr() } else { format!("pid={pid} handle ok (NORMAL, non-AppContainer service only)") },
    );
    if h.is_null() {
        return false;
    }
    sc.svc = h;
    r.check("service handle IsProcessInJob", in_job(h, sc.job), "member of parent-owned job");
    let (mut c, mut e, mut k, mut u) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
    let ok = unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) };
    r.check("GetProcessTimes creation time", ok != 0 && ft(c) > 0, &format!("creation_filetime={}", ft(c)));
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(h, &mut code) };
    r.check("GetExitCodeProcess while alive == STILL_ACTIVE", code == STILL_ACTIVE, &format!("{code}"));
    let w = unsafe { WaitForSingleObject(h, 0) };
    r.check("WaitForSingleObject(0) while alive == WAIT_TIMEOUT", w == 0x102, &format!("{w:#x}"));
    true
}
fn kill_service(r: &mut Report, sc: &mut Sc) {
    let ok = unsafe { TerminateProcess(sc.svc, 0x4A4A) };
    r.check("TerminateProcess(service owner handle)", ok != 0, &if ok != 0 { "ok".into() } else { oserr() });
    let w = unsafe { WaitForSingleObject(sc.svc, 5000) };
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(sc.svc, &mut code) };
    r.check("service wait observable + exit code", w == 0 && code == 0x4A4A, &format!("wait={w:#x} exit={code:#x}"));
}

fn parent() -> i32 {
    let exe = std::env::current_exe().unwrap();
    let tmp = std::env::temp_dir();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let base = tmp.join(format!("aleph-jobprobe-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    let mut r = Report { fails: 0 };
    r.say(format!("probe exe={} pid={} base={}", exe.display(), std::process::id(), base.display()));
    r.say("SCOPE: anonymous Job Object limit semantics with std surrogate wrapper/service; NOT sandbox-init, NOT AppContainer, NOT production proof.");

    // ---- L1: limit 1, wrapper consumes the only slot
    {
        let (sc, root) = start(&mut r, &exe, &base, "L1", 1);
        let res = wait_file(&root, "wrapper-result", 15);
        r.say(format!("  wrapper observed: {:?}", res));
        r.check("L1 target spawn refused", res.as_deref().is_some_and(|s| s.contains("REFUSED")), res.as_deref().unwrap_or("timeout"));
        let a = acct(sc.job);
        r.say(format!("  job accounting after: Total={} Active={} TotalTerminated={}", a.TotalProcesses, a.ActiveProcesses, a.TotalTerminatedProcesses));
        r.check("L1 no extra process ever entered job", a.TotalProcesses == 1, &format!("TotalProcesses={} members_now={:?}", a.TotalProcesses, members(sc.job)));
    }

    // ---- L2A: limit 2, wrapper stays alive: wrapper+service ok, service's own child refused
    {
        let (mut sc, root) = start(&mut r, &exe, &base, "L2A", 2);
        let wr = wait_file(&root, "wrapper-result", 15);
        r.check("L2A wrapper spawned service (slot 2)", wr.as_deref().is_some_and(|s| s.contains("OK")), wr.as_deref().unwrap_or("timeout"));
        if open_service(&mut r, &mut sc, &root) {
            let a1 = wait_file(&root, "svc-attempt1", 15);
            r.check("L2A service's further child refused (wrapper alive)", a1.as_deref().is_some_and(|s| s.contains("REFUSED")), a1.as_deref().unwrap_or("timeout"));
            let a = acct(sc.job);
            let m = members(sc.job);
            r.say(format!("  job members: {:?} (Active={} Total={})", m, a.ActiveProcesses, a.TotalProcesses));
            r.check("L2A job has exactly 2 non-conhost members (wrapper+service)", non_conhost(&m) == 2, &format!("{:?}", m));
            kill_service(&mut r, &mut sc);
            let d = wait_file(&root, "wrapper-done", 10);
            r.check("L2A wrapper observed service kill and finished", d.is_some(), d.as_deref().unwrap_or("timeout"));
            std::thread::sleep(Duration::from_millis(300));
            let a = acct(sc.job);
            r.say(format!("  job accounting after kill: Total={} Active={} TotalTerminated={}", a.TotalProcesses, a.ActiveProcesses, a.TotalTerminatedProcesses));
        }
    }

    // ---- L2B: limit 2, wrapper exits early while parent keeps job + service alive
    {
        let (mut sc, root) = start(&mut r, &exe, &base, "L2B", 2);
        let wr = wait_file(&root, "wrapper-result", 15);
        r.check("L2B wrapper spawned service (slot 2)", wr.as_deref().is_some_and(|s| s.contains("OK")), wr.as_deref().unwrap_or("timeout"));
        if open_service(&mut r, &mut sc, &root) {
            let a1 = wait_file(&root, "svc-attempt1", 15);
            r.check("L2B attempt BEFORE wrapper exit refused", a1.as_deref().is_some_and(|s| s.contains("REFUSED")), a1.as_deref().unwrap_or("timeout"));
            let wh = sc.wrapper.as_ref().unwrap().as_raw_handle() as HANDLE;
            let w = unsafe { WaitForSingleObject(wh, 10000) };
            r.check("wrapper exited (parent still owns job)", w == 0, &format!("wait={w:#x}"));
            let a = acct(sc.job);
            let m = members(sc.job);
            r.say(format!("  job members after wrapper exit: {:?} (Active={} Total={})", m, a.ActiveProcesses, a.TotalProcesses));
            r.check("job has exactly 1 non-conhost member (service) after wrapper exit", non_conhost(&m) == 1, &format!("{:?}", m));
            put(&root, "go-after", "1");
            let a2 = wait_file(&root, "svc-attempt2", 15);
            r.check(
                "L2B attempt AFTER wrapper exit SUCCEEDS (slot freed => L2 is not a no-fork guarantee)",
                a2.as_deref().is_some_and(|s| s.contains("SPAWNED")),
                a2.as_deref().unwrap_or("timeout"),
            );
            let a = acct(sc.job);
            r.say(format!("  job accounting: Total={} Active={} TotalTerminated={} members={:?}", a.TotalProcesses, a.ActiveProcesses, a.TotalTerminatedProcesses, members(sc.job)));
            kill_service(&mut r, &mut sc);
        }
    }

    // ---- owned cleanup (only the dir we created under temp_dir with our own prefix)
    let ok_prefix = base.parent() == Some(tmp.as_path())
        && base.file_name().unwrap().to_string_lossy().starts_with("aleph-jobprobe-");
    if ok_prefix {
        let _ = fs::remove_dir_all(&base);
    }
    r.say(format!("cleanup: fixture root removed={} exists_now={}", ok_prefix, base.exists()));
    r.say(format!("RESULT: {} (fails={})", if r.fails == 0 { "ALL PASS" } else { "FAILURES" }, r.fails));
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
        Some("service") => mode_service(Path::new(&a[2]), &a[3]),
        Some("wrapper") => mode_wrapper(Path::new(&a[2]), &a[3]),
        _ => std::process::exit(parent()),
    }
}
