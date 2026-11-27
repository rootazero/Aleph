use std::cell::{Cell, RefCell};
use std::rc::Rc;

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

const STIFFNESS: f64 = 220.0;
const DAMPING: f64 = 28.0;
const MAX_FRAME_SECONDS: f64 = 1.0 / 30.0;
const POSITION_EPSILON: f64 = 0.001;
const VELOCITY_EPSILON: f64 = 0.01;

/// Bounded, frame-rate independent damped spring. `step` clamps long frames so
/// returning from a suspended tab cannot produce a large integration jump.
#[derive(Clone, Copy, Debug)]
pub struct Spring {
    position: f64,
    velocity: f64,
    target: f64,
}

impl Spring {
    #[must_use]
    pub fn new(position: f64) -> Self {
        Self {
            position,
            velocity: 0.0,
            target: position,
        }
    }

    pub fn set_target(&mut self, target: f64) {
        if target.is_finite() {
            self.target = target;
        }
    }

    #[must_use]
    pub fn position(&self) -> f64 {
        self.position
    }

    #[must_use]
    pub fn is_settled(&self) -> bool {
        (self.target - self.position).abs() <= POSITION_EPSILON
            && self.velocity.abs() <= VELOCITY_EPSILON
    }

    /// Advance by elapsed seconds and return whether another frame is needed.
    pub fn step(&mut self, seconds: f64) -> bool {
        if self.is_settled() {
            self.position = self.target;
            self.velocity = 0.0;
            return false;
        }
        let dt = if seconds.is_finite() {
            seconds.clamp(0.0, MAX_FRAME_SECONDS)
        } else {
            0.0
        };
        let acceleration = STIFFNESS * (self.target - self.position) - DAMPING * self.velocity;
        self.velocity = (self.velocity + acceleration * dt).clamp(-10_000.0, 10_000.0);
        self.position = (self.position + self.velocity * dt).clamp(-1_000_000.0, 1_000_000.0);
        if self.is_settled() {
            self.position = self.target;
            self.velocity = 0.0;
            false
        } else {
            true
        }
    }
}

/// Reactive spring value driven by one cancellable browser frame loop.
/// The returned callback changes the target; reduced motion snaps immediately.
#[must_use]
pub fn use_spring(initial: f64) -> (ReadSignal<f64>, Callback<f64>) {
    let value = RwSignal::new(initial);
    let spring = Rc::new(RefCell::new(Spring::new(initial)));
    let alive = Rc::new(Cell::new(true));
    let raf_id = Rc::new(Cell::new(0_i32));
    let previous_time = Rc::new(Cell::new(None::<f64>));
    let frame: Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>> = Rc::new(RefCell::new(None));
    let frame_again = frame.clone();
    let spring_frame = spring.clone();
    let alive_frame = alive.clone();
    let raf_frame = raf_id.clone();
    let previous_frame = previous_time.clone();
    let value_frame = value;

    *frame.borrow_mut() = Some(Closure::wrap(Box::new(move |time: f64| {
        if !alive_frame.get() {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let hidden = window.document().is_some_and(|doc| doc.hidden());
        if hidden {
            previous_frame.set(None);
            return;
        }
        let reduced = window
            .match_media("(prefers-reduced-motion: reduce)")
            .ok()
            .flatten()
            .is_some_and(|query| query.matches());
        if reduced {
            let target = spring_frame.borrow().target;
            let mut state = spring_frame.borrow_mut();
            state.position = target;
            state.velocity = 0.0;
            value_frame.set(target);
            previous_frame.set(None);
            raf_frame.set(0);
            return;
        }
        let elapsed = previous_frame
            .replace(Some(time))
            .map(|prior| ((time - prior) / 1000.0).max(0.0))
            .unwrap_or(0.0);
        let mut state = spring_frame.borrow_mut();
        let keep_running = state.step(elapsed);
        value_frame.set(state.position());
        drop(state);
        if keep_running {
            if let Some(callback) = frame_again.borrow().as_ref() {
                raf_frame.set(
                    window
                        .request_animation_frame(callback.as_ref().unchecked_ref())
                        .unwrap_or(0),
                );
            }
        } else {
            previous_frame.set(None);
            raf_frame.set(0);
        }
    }) as Box<dyn FnMut(f64)>));

    let request = {
        let frame = frame.clone();
        let spring = spring.clone();
        let alive = alive.clone();
        let raf_id = raf_id.clone();
        let previous_time = previous_time.clone();
        Callback::new(move |target: f64| {
            if !target.is_finite() || !alive.get() {
                return;
            }
            spring.borrow_mut().set_target(target);
            let Some(window) = web_sys::window() else {
                return;
            };
            let reduced = window
                .match_media("(prefers-reduced-motion: reduce)")
                .ok()
                .flatten()
                .is_some_and(|query| query.matches());
            if reduced {
                let mut state = spring.borrow_mut();
                state.position = target;
                state.velocity = 0.0;
                value.set(target);
                previous_time.set(None);
                return;
            }
            if window.document().is_some_and(|doc| doc.hidden()) {
                return;
            }
            if raf_id.get() == 0 {
                previous_time.set(None);
                if let Some(callback) = frame.borrow().as_ref() {
                    raf_id.set(
                        window
                            .request_animation_frame(callback.as_ref().unchecked_ref())
                            .unwrap_or(0),
                    );
                }
            }
        })
    };

    let visibility_frame = frame.clone();
    let visibility_spring = spring.clone();
    let visibility_alive = alive.clone();
    let visibility_raf = raf_id.clone();
    let visibility_previous = previous_time.clone();
    let visibility = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
        let Some(window) = web_sys::window() else {
            return;
        };
        if !visibility_alive.get() {
            return;
        }
        if window.document().is_some_and(|doc| doc.hidden()) {
            if visibility_raf.get() != 0 {
                let _ = window.cancel_animation_frame(visibility_raf.get());
                visibility_raf.set(0);
            }
            visibility_previous.set(None);
            return;
        }
        if visibility_raf.get() != 0 {
            return;
        }
        visibility_previous.set(None);
        let reduced = window
            .match_media("(prefers-reduced-motion: reduce)")
            .ok()
            .flatten()
            .is_some_and(|query| query.matches());
        if reduced {
            let mut state = visibility_spring.borrow_mut();
            state.position = state.target;
            state.velocity = 0.0;
            value.set(state.target);
            return;
        }
        if !visibility_spring.borrow().is_settled() {
            if let Some(callback) = visibility_frame.borrow().as_ref() {
                visibility_raf.set(
                    window
                        .request_animation_frame(callback.as_ref().unchecked_ref())
                        .unwrap_or(0),
                );
            }
        }
    });
    let document = web_sys::window().and_then(|window| window.document());
    if let Some(document) = &document {
        let _ = document.add_event_listener_with_callback(
            "visibilitychange",
            visibility.as_ref().unchecked_ref(),
        );
    }
    on_cleanup(move || {
        alive.set(false);
        if let Some(window) = web_sys::window() {
            if raf_id.get() != 0 {
                let _ = window.cancel_animation_frame(raf_id.get());
            }
        }
        if let Some(document) = document {
            let _ = document.remove_event_listener_with_callback(
                "visibilitychange",
                visibility.as_ref().unchecked_ref(),
            );
        }
        frame.borrow_mut().take();
        drop(visibility);
    });
    (value.read_only(), request)
}

#[cfg(test)]
mod tests {
    use super::Spring;

    #[test]
    fn spring_settles_at_target_and_clamps_long_frames() {
        let mut spring = Spring::new(0.0);
        spring.set_target(1.0);
        let mut frames = 0;
        while spring.step(10.0) && frames < 300 {
            frames += 1;
        }
        assert!(frames < 300);
        assert_eq!(spring.position(), 1.0);
    }

    #[test]
    fn non_finite_targets_are_ignored() {
        let mut spring = Spring::new(2.0);
        spring.set_target(f64::NAN);
        assert_eq!(spring.position(), 2.0);
        assert!(!spring.step(0.0));
    }
}
