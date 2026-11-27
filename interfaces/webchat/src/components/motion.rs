use leptos::leptos_dom::helpers::{
    request_animation_frame_with_handle, AnimationFrameRequestHandle,
};
use leptos::prelude::*;
use wasm_bindgen::{closure::Closure, JsCast};

const STIFFNESS: f64 = 220.0;
const DAMPING: f64 = 28.0;
const MAX_FRAME_SECONDS: f64 = 1.0 / 30.0;
const POSITION_EPSILON: f64 = 0.001;
const VELOCITY_EPSILON: f64 = 0.01;
const MOTION_QUERY: &str = "(prefers-reduced-motion: no-preference)";

#[derive(Clone, Copy, Debug)]
struct Spring {
    position: f64,
    velocity: f64,
    target: f64,
}

impl Spring {
    fn new(position: f64) -> Self {
        Self {
            position,
            velocity: 0.0,
            target: position,
        }
    }

    fn retarget(&mut self, target: f64) {
        if target.is_finite() {
            self.target = target;
        }
    }

    fn snap(&mut self) {
        self.position = self.target;
        self.velocity = 0.0;
    }

    fn is_settled(&self) -> bool {
        (self.target - self.position).abs() <= POSITION_EPSILON
            && self.velocity.abs() <= VELOCITY_EPSILON
    }

    fn advance(&mut self, seconds: f64) -> f64 {
        if !self.is_settled() && seconds.is_finite() {
            let dt = seconds.clamp(0.0, MAX_FRAME_SECONDS);
            let acceleration = STIFFNESS * (self.target - self.position) - DAMPING * self.velocity;
            self.velocity += acceleration * dt;
            self.position += self.velocity * dt;
        }
        if self.is_settled() {
            self.snap();
        }
        self.position
    }
}

pub(crate) fn can_animate() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    window.document().is_some_and(|document| !document.hidden())
        && window
            .match_media(MOTION_QUERY)
            .ok()
            .flatten()
            .is_some_and(|query| query.matches())
}

/// Both animation consumers settle immediately when the tab or the OS disables motion.
pub(crate) fn on_motion_unavailable(cancel: impl Fn() + Copy + 'static) {
    let visibility = window_event_listener_untyped("visibilitychange", move |_| {
        if !can_animate() {
            cancel();
        }
    });
    on_cleanup(move || visibility.remove());
    if let Some(query) =
        web_sys::window().and_then(|window| window.match_media(MOTION_QUERY).ok()?)
    {
        let callback = Closure::<dyn FnMut(web_sys::Event)>::new(move |_| {
            if !can_animate() {
                cancel();
            }
        });
        if query
            .add_event_listener_with_callback("change", callback.as_ref().unchecked_ref())
            .is_ok()
        {
            let listener = StoredValue::new_local((query, callback));
            on_cleanup(move || {
                let _ = listener.try_with_value(|(query, callback)| {
                    let _ = query.remove_event_listener_with_callback(
                        "change",
                        callback.as_ref().unchecked_ref(),
                    );
                });
            });
        }
    }
}

fn now() -> Option<f64> {
    Some(web_sys::window()?.performance()?.now())
}

#[derive(Clone, Copy)]
struct SpringDriver {
    spring: StoredValue<Spring>,
    value: RwSignal<f64>,
    frame: StoredValue<Option<AnimationFrameRequestHandle>>,
}

impl SpringDriver {
    fn cancel_frame(self) {
        if let Some(pending) = self.frame.try_update_value(Option::take).flatten() {
            pending.cancel();
        }
    }

    fn settle(self) {
        self.cancel_frame();
        if let Some(position) = self.spring.try_update_value(|spring| {
            spring.snap();
            spring.position
        }) {
            let _ = self.value.try_set(position);
        }
    }

    fn schedule(self, then: f64) {
        let next = request_animation_frame_with_handle(move || {
            let _ = self.frame.try_update_value(Option::take);
            if !can_animate() {
                self.settle();
                return;
            }
            let Some(now) = now() else {
                self.settle();
                return;
            };
            if let Some((position, settled)) = self.spring.try_update_value(|spring| {
                let position = spring.advance((now - then) / 1_000.0);
                (position, spring.is_settled())
            }) {
                let _ = self.value.try_set(position);
                if !settled {
                    self.schedule(now);
                }
            }
        });
        if let Ok(next) = next {
            let _ = self.frame.try_update_value(|slot| *slot = Some(next));
        } else {
            self.settle();
        }
    }
}

/// Drive a visual position with at most one pending frame. Retargets retain velocity.
pub(crate) fn use_spring(initial: f64) -> (ReadSignal<f64>, Callback<f64>) {
    let driver = SpringDriver {
        spring: StoredValue::new(Spring::new(initial)),
        value: RwSignal::new(initial),
        frame: StoredValue::new(None),
    };
    on_cleanup(move || driver.cancel_frame());
    on_motion_unavailable(move || driver.settle());
    let retarget = Callback::new(move |target: f64| {
        let Some(settled) = driver.spring.try_update_value(|spring| {
            spring.retarget(target);
            spring.is_settled()
        }) else {
            return;
        };
        if settled || !can_animate() {
            driver.settle();
        } else if driver.frame.with_value(Option::is_none) {
            if let Some(now) = now() {
                driver.schedule(now);
            } else {
                driver.settle();
            }
        }
    });
    (driver.value.read_only(), retarget)
}

#[cfg(test)]
mod tests {
    use super::Spring;

    #[test]
    fn spring_retarget_keeps_current_position() {
        let mut spring = Spring::new(0.0);
        spring.retarget(1.0);
        spring.advance(1.0 / 60.0);
        let (position, velocity) = (spring.position, spring.velocity);
        spring.retarget(-1.0);
        assert_eq!((spring.position, spring.velocity), (position, velocity));
    }

    #[test]
    fn spring_settles_after_large_frame_gap() {
        let mut spring = Spring::new(0.0);
        spring.retarget(1.0);
        assert!(spring.advance(120.0) < 0.5);
        for _ in 0..120 {
            spring.advance(1.0 / 60.0);
        }
        assert_eq!(spring.position, 1.0);
        assert!(spring.is_settled());
    }

    #[test]
    fn snapping_to_target_clears_motion() {
        let mut spring = Spring::new(0.0);
        spring.retarget(1.0);
        spring.advance(1.0 / 60.0);
        spring.retarget(0.4);
        spring.snap();
        assert_eq!(spring.position, 0.4);
        assert!(spring.is_settled());
    }
}
