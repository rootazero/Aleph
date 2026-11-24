//! The TSX Panel (`interfaces/panel`) embedded in the Rust-root shell as one
//! native component (macOS): an NSH `AppView` running the Panel's bundle,
//! with its host functions served by `panel_host`.
//!
//! The bundle is compiled into the shell (`include_str!`), so it is as trusted
//! as the shell's own code; it loads as JS source, not bytecode. At most one
//! surface lives at a time: a second one stays empty.
//!
//! The surface also serves `aleph.fontScale` (the Panel's text size), since
//! it owns the `AppView`; the other host functions are `panel_host`'s.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use nsh_engine::HostChannel;
use nsh_platform_macos::objc2::rc::Retained;
use nsh_platform_macos::objc2::MainThreadMarker;
use nsh_platform_macos::objc2_app_kit::NSView;
use nsh_platform_macos::{
    AppOptions, AppView, Bundle, ComponentContext, Components, NativeComponent,
};
use serde_json::{json, Value};

use crate::panel_host;

/// The native component the shell's root mounts.
pub(crate) const SURFACE: &str = "aleph/panel-surface";

/// The Panel's id (`interfaces/panel/nsh.json`); its storage (the saved
/// layout) lives under it.
const PANEL_APP_ID: &str = "com.aleph.panel";

const BUNDLE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../interfaces/panel/out-bundle/bundle.js"
));
const CAPABILITIES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../interfaces/panel/out-bundle/capabilities.json"
));

static TARGET: OnceLock<panel_host::Target> = OnceLock::new();
static LIVE: AtomicBool = AtomicBool::new(false);

/// The host function that sets the Panel's text size.
const FONT_SCALE: &str = "aleph.fontScale";

thread_local! {
    /// The live surface's view, on the main thread, where `aleph.fontScale`
    /// reaches it (host functions run on the engine thread).
    static SURFACE_VIEW: RefCell<Weak<AppView>> = const { RefCell::new(Weak::new()) };
}

/// Installs the Panel's UI sets and the surface component, once, before the
/// shell's `run_app` (both registrations are process-wide).
pub(crate) fn register(target: panel_host::Target) {
    if TARGET.set(target).is_err() {
        return;
    }
    nsh_platform_macos::set_ui_sets(vec![
        nsh_base::registry(),
        aleph_panel_ui::__nsh_native_module(),
    ]);
    nsh_platform_macos::set_components(register_components);
}

/// Whether [`register`] ran: the shell then mounts the TSX Panel instead of
/// the classic one.
pub(crate) fn registered() -> bool {
    TARGET.get().is_some()
}

fn register_components(components: &Components) {
    components.register(SURFACE, Rc::new(make_surface));
}

fn make_surface(context: ComponentContext) -> Box<dyn NativeComponent> {
    let mtm = MainThreadMarker::new().expect("native components are made on the main thread");
    let Some(claim) = Claim::acquire() else {
        eprintln!("NSH TSX Panel refused a second surface");
        context.emit(
            "refused",
            json!({ "reason": "another Panel surface is live" }),
        );
        return Box::new(RefusedSurface {
            view: NSView::new(mtm),
        });
    };
    let target = TARGET
        .get()
        .cloned()
        .unwrap_or(panel_host::Target::Unavailable(
            "the Panel surface is not registered",
        ));
    let channel = HostChannel::default();
    channel.handle(FONT_SCALE, handle_font_scale);
    let host = match panel_host::Handle::start(channel.clone(), target) {
        Ok(host) => Some(host),
        Err(error) => {
            eprintln!("NSH TSX Panel host refused to start: {error}");
            None
        }
    };
    let view = AppView::new(
        mtm,
        AppOptions {
            app_id: PANEL_APP_ID.to_owned(),
            host: channel.clone(),
            ..AppOptions::default()
        },
    );
    SURFACE_VIEW.with(|surface| *surface.borrow_mut() = Rc::downgrade(&view));
    match Bundle::source(BUNDLE.to_owned()).with_capabilities_json(Some(CAPABILITIES)) {
        Ok(bundle) => view.load(bundle.with_app_id(Some(PANEL_APP_ID)).into()),
        Err(error) => eprintln!("NSH TSX Panel bundle refused: {error}"),
    }
    Box::new(PanelSurface {
        view,
        channel,
        _host: host,
        _claim: claim,
    })
}

/// `aleph.fontScale(scale)`: a positive factor of the system's text size, or
/// `null` to follow the system again. The answer does not wait for the main
/// thread; a surface that went away in between ignores the change.
fn handle_font_scale(call: nsh_engine::HostCall) {
    let scale = match parse_font_scale(call.data()) {
        Ok(scale) => scale,
        Err(message) => return call.reject(message),
    };
    dispatch2::DispatchQueue::main().exec_async(move || {
        let view = SURFACE_VIEW.with(|surface| surface.borrow().upgrade());
        if let Some(view) = view {
            view.set_font_scale(scale);
        }
    });
    call.resolve(Value::Null);
}

fn parse_font_scale(data: &Value) -> Result<Option<f32>, &'static str> {
    if data.is_null() {
        return Ok(None);
    }
    match data.as_f64() {
        Some(scale) if scale.is_finite() && scale > 0.0 => Ok(Some(scale as f32)),
        _ => Err("font scale must be a positive number"),
    }
}

/// Fields drop in order after `drop`: the app stops first, then its host
/// functions go, then the claim is released.
struct PanelSurface {
    view: Rc<AppView>,
    /// Kept to remove `aleph.fontScale` when the surface goes.
    channel: HostChannel,
    _host: Option<panel_host::Handle>,
    _claim: Claim,
}

impl NativeComponent for PanelSurface {
    fn view(&self) -> Retained<NSView> {
        self.view.view()
    }

    fn set_prop(&mut self, _name: &str, _value: Option<&Value>) {}
}

impl Drop for PanelSurface {
    fn drop(&mut self) {
        self.view.stop();
        self.channel.remove_handler(FONT_SCALE);
        SURFACE_VIEW.with(|surface| *surface.borrow_mut() = Weak::new());
    }
}

struct RefusedSurface {
    view: Retained<NSView>,
}

impl NativeComponent for RefusedSurface {
    fn view(&self) -> Retained<NSView> {
        self.view.clone()
    }

    fn set_prop(&mut self, _name: &str, _value: Option<&Value>) {}
}

/// The one live surface.
struct Claim;

impl Claim {
    fn acquire() -> Option<Claim> {
        LIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Claim)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        LIVE.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_surface_is_refused() {
        let first = Claim::acquire().expect("the first surface is admitted");
        assert!(Claim::acquire().is_none());
        drop(first);
        assert!(Claim::acquire().is_some());
    }

    #[test]
    fn font_scale_accepts_a_positive_factor_or_null() {
        assert_eq!(parse_font_scale(&json!(1.25)), Ok(Some(1.25)));
        assert_eq!(parse_font_scale(&Value::Null), Ok(None));
        for bad in [json!(0), json!(-1), json!("big"), json!(true)] {
            assert_eq!(
                parse_font_scale(&bad),
                Err("font scale must be a positive number")
            );
        }
    }
}
