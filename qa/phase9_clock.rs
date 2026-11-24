//! Shared public macOS monotonic clock and collector rendezvous for debug QA.
use std::sync::OnceLock;

static ORIGIN: OnceLock<u64> = OnceLock::new();

pub fn now_ns() -> u64 {
    #[cfg(target_os = "macos")]
    {
        #[repr(C)]
        struct Timespec {
            seconds: std::ffi::c_long,
            nanoseconds: std::ffi::c_long,
        }
        unsafe extern "C" {
            fn clock_gettime(clock: i32, value: *mut Timespec) -> i32;
        }
        let mut value = Timespec {
            seconds: 0,
            nanoseconds: 0,
        };
        // CLOCK_MONOTONIC = 6 in the public macOS time.h SDK.
        assert_eq!(unsafe { clock_gettime(6, &mut value) }, 0);
        value.seconds as u64 * 1_000_000_000 + value.nanoseconds as u64
    }
    #[cfg(not(target_os = "macos"))]
    {
        panic!("Phase9 shared clock requires macOS");
    }
}

pub fn start() {
    if let Ok(gate) = std::env::var("ALEPH_PANEL_QA_START_GATE") {
        let nonce = std::env::var("ALEPH_PANEL_QA_GATE_NONCE").expect("QA gate nonce");
        let pid_path = std::env::var("ALEPH_PANEL_QA_PID").expect("QA PID capture");
        let temporary = format!("{pid_path}.pending");
        std::fs::write(
            &temporary,
            serde_json::json!({"pid": std::process::id(), "nonce": nonce}).to_string(),
        )
        .expect("QA PID rendezvous");
        std::fs::rename(temporary, pid_path).expect("QA PID atomic publication");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::fs::read_to_string(&gate).ok().as_deref() != Some(nonce.as_str()) {
            assert!(
                std::time::Instant::now() < deadline,
                "QA sampler start gate timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    let origin = *ORIGIN.get_or_init(now_ns);
    if let Ok(path) = std::env::var("ALEPH_PANEL_QA_CLOCK") {
        std::fs::write(path, origin.to_string()).expect("QA native clock capture");
    }
}

pub fn annotate(event: &mut serde_json::Value) {
    let origin = *ORIGIN.get_or_init(now_ns);
    let now = now_ns();
    event["native_clock_origin_ns"] = origin.into();
    event["native_clock_ns"] = now.into();
    event["native_at_ms"] = serde_json::json!((now - origin) as f64 / 1_000_000.0);
}
