//! Throwaway S2 runtime spike host (macOS).
//!
//! Q1: an AppView running TSX embeds in a Rust-root NSH host and renders.
//! Q2: native components get real props and deliver events, both at the
//!     platform level (this host's `aleph/panel-surface`) and the UI-set
//!     level (the spike crate's `Pulse`, used from TSX).
//! Q4: after the surface unmounts, JS timers stop, JS-held Rust objects are
//!     freed, and the embedded AppView is dropped.
//!
//! Run: ALEPH_S2_BUNDLE=<abs path to bundle.js> cargo run

use std::rc::Rc;
use std::sync::Mutex;
use std::time::Duration;

use nsh_core::desktop::MainWindowOptions;
use nsh_core::element::desktop::quit;
use nsh_core::prelude::*;
use nsh_platform_macos::objc2::MainThreadMarker;
use nsh_platform_macos::objc2_app_kit::{NSApplication, NSView};
use nsh_platform_macos::objc2::rc::Retained;
use nsh_platform_macos::{
    AppOptions, AppSource, AppView, Bundle, ComponentContext, Components, NativeComponent, Root,
    run_app,
};
use serde_json::{Value as JsonValue, json};
use spike::spike_probe;

const SURFACE: &str = "aleph/panel-surface";

static HOST_EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static SAMPLES: Mutex<Vec<JsonValue>> = Mutex::new(Vec::new());

fn host_record(event: String) {
    HOST_EVENTS.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

fn thread_count() -> i64 {
    let pid = std::process::id().to_string();
    std::process::Command::new("ps")
        .args(["-M", "-p", &pid])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).lines().count() as i64 - 1)
        .unwrap_or(-1)
}

fn open_fds() -> i64 {
    std::fs::read_dir("/dev/fd").map(|dir| dir.count() as i64).unwrap_or(-1)
}

/// AppKit windows of the process (`None`: not sampled on the main thread).
fn window_count() -> Option<usize> {
    let mtm = MainThreadMarker::new()?;
    Some(NSApplication::sharedApplication(mtm).windows().count())
}

fn sample(phase: &str) {
    let entry = json!({
        "phase": phase,
        "ticks": spike_probe::ticks(),
        "live_tracked": spike_probe::live_tracked(),
        "threads": thread_count(),
        "fds": open_fds(),
        "windows": window_count(),
    });
    SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).push(entry);
}

/// The platform native component wrapping the embedded TSX surface.
struct PanelSurface {
    view: Rc<AppView>,
}

impl NativeComponent for PanelSurface {
    fn view(&self) -> Retained<NSView> {
        self.view.view()
    }

    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
        host_record(format!("surface-prop:{name}={}", value.cloned().unwrap_or(JsonValue::Null)));
    }
}

impl Drop for PanelSurface {
    fn drop(&mut self) {
        self.view.stop();
        host_record(format!("surface-dropped:strong={}", Rc::strong_count(&self.view)));
    }
}

fn register_components(components: &Components) {
    components.register(
        SURFACE,
        Rc::new(|context: ComponentContext| {
            let mtm = MainThreadMarker::new().expect("native components are made on the main thread");
            let view = AppView::new(mtm, AppOptions::default());
            let path = std::env::var("ALEPH_S2_BUNDLE").unwrap_or_default();
            match Bundle::read(std::path::Path::new(&path)) {
                Ok(bundle) => {
                    view.load(bundle.into());
                    host_record("surface-loaded".into());
                    context.emit("loaded", json!({ "bundle": path }));
                }
                Err(error) => host_record(format!("bundle-error:{error}")),
            }
            Box::new(PanelSurface { view }) as Box<dyn NativeComponent>
        }),
    );
}

#[derive(Clone, PartialEq, Props)]
struct HostProps {
    label: Prop<String>,
}

#[component]
fn Host(props: HostProps) -> impl IntoElement {
    let shown = signal(false);
    sample("baseline");
    // Each 4s cycle: mount at +0.5s, sample and unmount at +2.5s, sample
    // again at +4.0s (1.5s after the unmount, before the next mount).
    let cycles: u64 = std::env::var("ALEPH_S2_CYCLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    for cycle in 0..cycles {
        let start = cycle * 4_000;
        set_timeout(Duration::from_millis(start + 500), move || shown.set(true));
        set_timeout(Duration::from_millis(start + 2_500), move || {
            sample(&format!("cycle{cycle}:mounted"));
            shown.set(false);
        });
        set_timeout(Duration::from_millis(start + 4_000), move || {
            sample(&format!("cycle{cycle}:unmounted+1.5s"))
        });
    }
    let hold_ms: u64 = std::env::var("ALEPH_S2_HOLD_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000);
    let end = cycles * 4_000 + hold_ms;
    set_timeout(Duration::from_millis(end), || {
        sample("final");
        let report = json!({
            "samples": SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            "host_events": HOST_EVENTS.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            "app_events": spike_probe::events(),
        });
        println!("ALEPH_S2_SPIKE {report}");
        quit();
    });
    let label = props.label.get_untracked();
    view().flex_1().child(when(shown, move || {
        native(SURFACE)
            .flex_1()
            .prop("label", json!(label.clone()))
            .on_native_event(|event: &NativeEvent| {
                host_record(format!("surface-event:{}:{}", event.name, event.data))
            })
    }))
}

fn main() -> std::process::ExitCode {
    nsh_platform_macos::set_ui_sets(vec![nsh_base::registry(), spike::__nsh_native_module()]);
    nsh_platform_macos::set_components(register_components);
    let root = Root::new::<Host>(|| HostProps {
        label: "from-rust-root".to_string().into(),
    })
    .app_id("dev.aleph.s2-spike")
    .main_window(MainWindowOptions::default());
    run_app(
        AppSource::Rust(root),
        AppOptions {
            title: "Aleph S2 spike".to_string(),
            size: (900, 600),
            app_id: "dev.aleph.s2-spike".to_string(),
            ..AppOptions::default()
        },
    )
}
