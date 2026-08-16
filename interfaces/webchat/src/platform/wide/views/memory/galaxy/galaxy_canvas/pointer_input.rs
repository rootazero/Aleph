//! Pointer input for the galaxy canvas: a pure gesture state machine
//! ([`PointerTracker`], unit-tested on the native target) plus the DOM plumbing
//! that feeds it ([`PointerCtx`], web-sys, untestable natively).
//!
//! Every coordinate here is CSS pixels, canvas-local — `Scene` owns the
//! conversion to the device-pixel backing store (see `galaxy_canvas.rs`).
//!
//! Rules:
//! - 1 pointer → orbit (pan instead when the host reports Shift / middle button)
//! - 2 pointers → pinch: the inter-pointer distance drives zoom, their midpoint
//!   drives pan. A multi-touch gesture never feeds the orbit path (two fingers
//!   used to compute each delta against the OTHER finger's last position, which
//!   spun the camera violently).
//! - a tap/click is only reported when the gesture stayed single-pointer and the
//!   pointer never travelled further than [`CLICK_THRESHOLD_PX`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use leptos::callback::Callback;
use leptos::prelude::*;
use wasm_bindgen::JsCast;

use super::super::gl::scene::Scene;
use super::perf_now;
use super::super::interaction::CanvasEvent;

/// Click threshold in CSS pixels. A pointer-up within this distance of
/// pointer-down counts as a click; larger = drag (no selection).
const CLICK_THRESHOLD_PX: f32 = 5.0;

/// Pixels of wheel-delta per pixel of pinch spread. `Scene::on_wheel` reads a
/// pixel-mode wheel delta, so a pinch must be expressed in the same unit; a
/// gain of 2 makes a 100 px spread ≈ one firm mouse-wheel notch.
const PINCH_ZOOM_GAIN: f32 = 2.0;

/// What a pointer-move means. Deltas are CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    /// The move changed nothing (unknown pointer, or a pinch sample with no
    /// previous sample to difference against).
    None,
    Orbit {
        dx: f32,
        dy: f32,
    },
    Pan {
        dx: f32,
        dy: f32,
    },
    Pinch {
        /// Pixel-mode wheel delta equivalent; positive = fingers spread = zoom in.
        zoom_delta: f32,
        /// Midpoint travel — pans the look-at centre.
        pan_dx: f32,
        pan_dy: f32,
        /// Midpoint of the two pointers: the zoom anchor.
        cursor: (f32, f32),
    },
}

/// What a pointer-up means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Release {
    /// The gesture was a tap/click (single pointer, under the drag threshold).
    pub click: bool,
    /// Where the pointer came up (CSS px, canvas-local).
    pub pos: (f32, f32),
}

/// Active pointers + the click-vs-drag and pinch bookkeeping over them.
#[derive(Debug, Default)]
pub struct PointerTracker {
    /// `(pointer_id, position)` for every pointer currently down, in the order
    /// they went down. Two entries is a pinch; more are ignored (first two win).
    pointers: Vec<(i32, (f32, f32))>,
    /// Where the gesture's FIRST pointer went down (click-vs-drag distance).
    down_pos: (f32, f32),
    /// Latched once a gesture ever had 2+ pointers: kills the click on release.
    multi: bool,
    /// `(inter-pointer distance, midpoint)` at the last pinch sample.
    pinch: Option<(f32, (f32, f32))>,
}

impl PointerTracker {
    /// A pointer went down. Re-entering an id that is already tracked replaces it.
    pub fn down(&mut self, id: i32, x: f32, y: f32) {
        self.pointers.retain(|(pid, _)| *pid != id);
        if self.pointers.is_empty() {
            self.down_pos = (x, y);
            self.multi = false;
        }
        self.pointers.push((id, (x, y)));
        if self.pointers.len() >= 2 {
            self.multi = true;
        }
        self.pinch = self.pinch_sample();
    }

    /// A pointer moved. `pan_modifier` is the host's Shift-key / middle-button
    /// read; it only applies to the single-pointer path.
    pub fn moved(&mut self, id: i32, x: f32, y: f32, pan_modifier: bool) -> Gesture {
        let Some(slot) = self.pointers.iter_mut().find(|(pid, _)| *pid == id) else {
            return Gesture::None; // hover, or a pointer we never saw go down
        };
        let (px, py) = slot.1;
        slot.1 = (x, y);

        if self.pointers.len() == 1 {
            let (dx, dy) = (x - px, y - py);
            return if pan_modifier {
                Gesture::Pan { dx, dy }
            } else {
                Gesture::Orbit { dx, dy }
            };
        }

        let Some((dist, mid)) = self.pinch_sample() else {
            return Gesture::None;
        };
        let Some((prev_dist, prev_mid)) = self.pinch.replace((dist, mid)) else {
            return Gesture::None; // first sample of this pinch: nothing to diff
        };
        Gesture::Pinch {
            // `zoom_delta` feeds `Scene::on_wheel`, whose contract is
            // "positive = dolly OUT". Spreading the fingers (dist grows) must
            // zoom IN, so the difference is taken previous-minus-current.
            zoom_delta: (prev_dist - dist) * PINCH_ZOOM_GAIN,
            pan_dx: mid.0 - prev_mid.0,
            pan_dy: mid.1 - prev_mid.1,
            cursor: mid,
        }
    }

    /// A pointer came up. Reports a click only for a single-pointer gesture that
    /// stayed within the drag threshold.
    pub fn up(&mut self, id: i32, x: f32, y: f32) -> Release {
        let was_tracked = self.pointers.iter().any(|(pid, _)| *pid == id);
        self.pointers.retain(|(pid, _)| *pid != id);
        let travelled = ((x - self.down_pos.0).powi(2) + (y - self.down_pos.1).powi(2)).sqrt();
        let click = was_tracked
            && self.pointers.is_empty()
            && !self.multi
            && travelled < CLICK_THRESHOLD_PX;
        self.pinch = self.pinch_sample();
        if self.pointers.is_empty() {
            self.multi = false;
        }
        Release { click, pos: (x, y) }
    }

    /// A pointer was cancelled (or the pointer left under capture loss): drop it,
    /// never a click.
    pub fn cancel(&mut self, id: i32) {
        self.pointers.retain(|(pid, _)| *pid != id);
        self.pinch = self.pinch_sample();
        if self.pointers.is_empty() {
            self.multi = false;
        }
    }

    /// True while any pointer is down — the host uses this to gate hover picking.
    pub fn is_active(&self) -> bool {
        !self.pointers.is_empty()
    }

    /// `(distance, midpoint)` of the first two active pointers.
    fn pinch_sample(&self) -> Option<(f32, (f32, f32))> {
        let (_, a) = *self.pointers.first()?;
        let (_, b) = *self.pointers.get(1)?;
        let dist = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        Some((dist, ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)))
    }
}

/// A wheel notch in line mode (Firefox's default) — one line ≈ 16 px.
const WHEEL_LINE_PX: f64 = 16.0;
/// A wheel notch in page mode — one page ≈ a viewport.
const WHEEL_PAGE_PX: f64 = 400.0;

/// The host state the DOM pointer handlers need. Handlers clone out of it, so
/// each closure captures only what it uses.
pub struct PointerCtx {
    pub scene: Rc<RefCell<Option<Scene>>>,
    pub tracker: Rc<RefCell<PointerTracker>>,
    /// Canvas bounding-rect origin in client px, cached to avoid a forced reflow
    /// on every pointermove. Refreshed on pointerdown and on resize.
    pub origin: Rc<Cell<(f32, f32)>>,
    pub last_hover: Rc<RefCell<Option<String>>>,
    pub on_event: Callback<CanvasEvent>,
}

impl PointerCtx {
    pub fn pointerdown(&self) -> impl FnMut(web_sys::PointerEvent) + 'static {
        let tracker = self.tracker.clone();
        let origin = self.origin.clone();
        move |ev: web_sys::PointerEvent| {
            // Capture so move/up fire even when the pointer leaves the canvas, and
            // snapshot the canvas origin for this gesture.
            if let Some(el) = target_element(&ev) {
                let rect = el.get_bounding_client_rect();
                origin.set((rect.left() as f32, rect.top() as f32));
                let _ = el.set_pointer_capture(ev.pointer_id());
            }
            let (x, y) = local_pos(&ev, origin.get());
            tracker.borrow_mut().down(ev.pointer_id(), x, y);
        }
    }

    pub fn pointermove(&self) -> impl FnMut(web_sys::PointerEvent) + 'static {
        let scene = self.scene.clone();
        let tracker = self.tracker.clone();
        let origin = self.origin.clone();
        let last_hover = self.last_hover.clone();
        let on_event = self.on_event;
        move |ev: web_sys::PointerEvent| {
            let local = local_pos(&ev, origin.get());

            if !tracker.borrow().is_active() {
                // Hover (nothing down): pick and emit HoverNode on transition.
                let hit = scene.borrow().as_ref().and_then(|s| s.pick(local));
                let mut lh = last_hover.borrow_mut();
                if *lh != hit {
                    *lh = hit.clone();
                    on_event.run(CanvasEvent::HoverNode(hit));
                }
                return;
            }

            // Pan on Shift-drag or middle-button drag (single pointer only); the
            // modifier is read live off the event, so no state threading.
            let pan_modifier = ev.shift_key() || (ev.buttons() & 4) != 0;
            let gesture =
                tracker
                    .borrow_mut()
                    .moved(ev.pointer_id(), local.0, local.1, pan_modifier);
            let t_ms = perf_now();
            let mut guard = scene.borrow_mut();
            let Some(s) = guard.as_mut() else {
                return;
            };
            match gesture {
                Gesture::Orbit { dx, dy } => s.on_drag(dx, dy, t_ms),
                Gesture::Pan { dx, dy } => s.on_pan(dx, dy, t_ms),
                Gesture::Pinch {
                    zoom_delta,
                    pan_dx,
                    pan_dy,
                    cursor,
                } => {
                    s.on_wheel(zoom_delta, cursor, t_ms);
                    s.on_pan(pan_dx, pan_dy, t_ms);
                }
                Gesture::None => {}
            }
        }
    }

    pub fn pointerup(&self) -> impl FnMut(web_sys::PointerEvent) + 'static {
        let scene = self.scene.clone();
        let tracker = self.tracker.clone();
        let origin = self.origin.clone();
        let on_event = self.on_event;
        move |ev: web_sys::PointerEvent| {
            if let Some(el) = target_element(&ev) {
                let _ = el.release_pointer_capture(ev.pointer_id());
            }
            let (x, y) = local_pos(&ev, origin.get());
            let release = tracker.borrow_mut().up(ev.pointer_id(), x, y);
            if !release.click {
                return;
            }
            let hit = scene.borrow().as_ref().and_then(|s| s.pick(release.pos));
            match hit {
                Some(id) => on_event.run(CanvasEvent::SelectNode(id)),
                None => on_event.run(CanvasEvent::DeselectNode),
            }
        }
    }

    pub fn pointercancel(&self) -> impl FnMut(web_sys::PointerEvent) + 'static {
        let tracker = self.tracker.clone();
        move |ev: web_sys::PointerEvent| {
            tracker.borrow_mut().cancel(ev.pointer_id());
        }
    }

    /// The pointer left the canvas: drop the hover, otherwise the label sticks on
    /// screen (and keeps re-projecting every frame) forever.
    pub fn pointerleave(&self) -> impl FnMut(web_sys::PointerEvent) + 'static {
        let last_hover = self.last_hover.clone();
        let on_event = self.on_event;
        move |_ev: web_sys::PointerEvent| {
            if last_hover.borrow().is_none() {
                return;
            }
            *last_hover.borrow_mut() = None;
            on_event.run(CanvasEvent::HoverNode(None));
        }
    }

    pub fn wheel(&self) -> impl FnMut(web_sys::WheelEvent) + 'static {
        let scene = self.scene.clone();
        let origin = self.origin.clone();
        move |ev: web_sys::WheelEvent| {
            ev.prevent_default();
            // Normalise delta_mode to PIXELS: Firefox defaults to LINE mode and
            // reports deltaY == 3 per notch, which is a ~0.15% zoom otherwise.
            let px = match ev.delta_mode() {
                1 => ev.delta_y() * WHEEL_LINE_PX,
                2 => ev.delta_y() * WHEEL_PAGE_PX,
                _ => ev.delta_y(),
            };
            // `Scene::on_wheel` reads a positive delta as "dolly OUT", and a
            // positive deltaY is a scroll *down* — which every map and canvas
            // app zooms out. So the delta passes through unnegated.
            let delta = px as f32;
            let (ox, oy) = origin.get();
            let cursor = (ev.client_x() as f32 - ox, ev.client_y() as f32 - oy);
            let t_ms = perf_now();
            if let Some(s) = scene.borrow_mut().as_mut() {
                s.on_wheel(delta, cursor, t_ms);
            }
        }
    }
}

fn target_element(ev: &web_sys::PointerEvent) -> Option<web_sys::Element> {
    ev.target()?.dyn_into::<web_sys::Element>().ok()
}

/// Client px → canvas-local CSS px.
fn local_pos(ev: &web_sys::PointerEvent, origin: (f32, f32)) -> (f32, f32) {
    (
        ev.client_x() as f32 - origin.0,
        ev.client_y() as f32 - origin.1,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pointer_move_orbits() {
        let mut t = PointerTracker::default();
        t.down(1, 100.0, 100.0);
        assert_eq!(
            t.moved(1, 110.0, 95.0, false),
            Gesture::Orbit { dx: 10.0, dy: -5.0 }
        );
    }

    #[test]
    fn pan_modifier_switches_single_pointer_to_pan() {
        let mut t = PointerTracker::default();
        t.down(1, 100.0, 100.0);
        assert_eq!(
            t.moved(1, 90.0, 100.0, true),
            Gesture::Pan { dx: -10.0, dy: 0.0 }
        );
    }

    #[test]
    fn move_of_untracked_pointer_is_none() {
        let mut t = PointerTracker::default();
        assert_eq!(t.moved(7, 10.0, 10.0, false), Gesture::None);
        assert!(!t.is_active());
    }

    #[test]
    fn two_pointers_never_orbit_and_spread_zooms_in() {
        let mut t = PointerTracker::default();
        t.down(1, 0.0, 0.0);
        t.down(2, 100.0, 0.0); // dist 100, mid (50, 0)

        // Second finger moves out to x=140: dist 140, mid (70, 0).
        match t.moved(2, 140.0, 0.0, false) {
            Gesture::Pinch {
                zoom_delta,
                pan_dx,
                pan_dy,
                cursor,
            } => {
                // `Scene::on_wheel` reads positive as "dolly OUT", so spreading
                // the fingers — which must zoom IN — has to emit a NEGATIVE delta.
                assert!(zoom_delta < 0.0, "spread must zoom in, got {zoom_delta}");
                assert!((zoom_delta + 40.0 * PINCH_ZOOM_GAIN).abs() < 1e-3);
                assert!((pan_dx - 20.0).abs() < 1e-3);
                assert!(pan_dy.abs() < 1e-6);
                assert!((cursor.0 - 70.0).abs() < 1e-3);
            }
            g => panic!("two pointers must pinch, got {g:?}"),
        }
    }

    #[test]
    fn pinch_pinch_together_zooms_out() {
        let mut t = PointerTracker::default();
        t.down(1, 0.0, 0.0);
        t.down(2, 100.0, 0.0);
        match t.moved(2, 60.0, 0.0, false) {
            // Fingers closing => dolly out => positive under on_wheel's contract.
            Gesture::Pinch { zoom_delta, .. } => assert!(zoom_delta > 0.0),
            g => panic!("expected pinch, got {g:?}"),
        }
    }

    #[test]
    fn short_tap_is_a_click() {
        let mut t = PointerTracker::default();
        t.down(1, 200.0, 150.0);
        t.moved(1, 202.0, 151.0, false);
        let r = t.up(1, 202.0, 151.0);
        assert!(r.click);
        assert_eq!(r.pos, (202.0, 151.0));
        assert!(!t.is_active());
    }

    #[test]
    fn drag_beyond_threshold_is_not_a_click() {
        let mut t = PointerTracker::default();
        t.down(1, 200.0, 150.0);
        t.moved(1, 260.0, 150.0, false);
        assert!(!t.up(1, 260.0, 150.0).click);
    }

    #[test]
    fn lifting_a_pinch_never_reports_a_click_and_resumes_orbit() {
        let mut t = PointerTracker::default();
        t.down(1, 100.0, 100.0);
        t.down(2, 101.0, 100.0); // second finger lands right next to the first
        assert!(!t.up(2, 101.0, 100.0).click);
        // One pointer left → single-pointer orbit again, delta against ITS own
        // last position (not the other finger's).
        assert_eq!(
            t.moved(1, 105.0, 100.0, false),
            Gesture::Orbit { dx: 5.0, dy: 0.0 }
        );
        // ...but the release of the gesture that was ever multi-touch is not a click.
        assert!(!t.up(1, 105.0, 100.0).click);
        assert!(!t.is_active());
    }

    #[test]
    fn cancel_clears_the_pointer() {
        let mut t = PointerTracker::default();
        t.down(1, 10.0, 10.0);
        t.cancel(1);
        assert!(!t.is_active());
        assert_eq!(t.moved(1, 20.0, 20.0, false), Gesture::None);
    }
}
