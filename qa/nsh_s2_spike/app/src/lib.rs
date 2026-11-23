//! The app's Rust code: what `App.tsx` imports from `aleph-s2-spike/native`. Their
//! TypeScript declarations are generated from these types into
//! `.nsh/types/aleph-s2-spike/native.d.ts`.

use nsh_base::theme;
use nsh_core::prelude::*;

/// The crate's UI set (`nsh.json` `"rust"`).
#[native_module("aleph-s2-spike/native")]
fn native(registry: &mut Registry) {
    registry
        .register::<Badge>()
        .register::<Pulse>()
        .module::<Format>()
        .module::<Probe>()
        .class::<Counter>()
        .class::<Tracked>();
}

// A component: `<Badge label="…" onPress={…} />`. The function's doc comment
// is the component's in TS, the fields' its props'.
/// The props of [`Badge`].
#[derive(Debug, Clone, PartialEq, Props)]
pub struct BadgeProps {
    /// The text.
    pub label: Prop<SharedString>,
}

/// The events of a [`Badge`].
#[derive(Debug, Clone, PartialEq, Event)]
pub enum BadgeEvent {
    /// The badge was pressed.
    Press,
}

/// A pill with a label; pressing it reports `onPress`.
#[component]
pub fn Badge(props: BadgeProps, cx: Scope<BadgeEvent>) -> impl IntoElement {
    view()
        .px_4()
        .py_2()
        .rounded_full()
        .bg(move || theme().primary)
        .active(|s| s.bg(move || theme().primary.opacity(0.9)))
        .on_press(move || cx.emit(BadgeEvent::Press))
        .child(text(props.label).text_color(move || theme().primary_foreground))
}

/// Text helpers.
#[module]
pub struct Format;

// A module: `Format.times(3)`. The struct's doc comment is the module's in
// TS (`#[module]` on it and on this block).
#[module]
impl Format {
    /// "once", "twice", "3 times".
    pub fn times(count: u32) -> String {
        match count {
            1 => "once".to_owned(),
            2 => "twice".to_owned(),
            n => format!("{n} times"),
        }
    }
}

/// A count kept in Rust.
#[class]
pub struct Counter {
    count: u32,
}

// A class: `new Counter()`, `counter.increment()`, `counter.count`.
#[class]
impl Counter {
    /// A counter at 0.
    pub fn new() -> Self {
        Counter { count: 0 }
    }

    /// Adds one; the new count.
    pub fn increment(&mut self) -> u32 {
        self.count += 1;
        self.count
    }

    /// The count.
    #[getter]
    pub fn count(&self) -> u32 {
        self.count
    }
}

// ---- S2 runtime spike probes (throwaway) ----

use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

static TICKS: AtomicU64 = AtomicU64::new(0);
static LIVE_TRACKED: AtomicI64 = AtomicI64::new(0);
static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(event: String) {
    EVENTS.lock().unwrap_or_else(|e| e.into_inner()).push(event);
}

/// Reads for the embedding spike host (same process, plain Rust).
pub mod spike_probe {
    use super::*;

    /// Interval ticks JS has reported so far.
    pub fn ticks() -> u64 {
        TICKS.load(Ordering::SeqCst)
    }

    /// `Tracked` instances alive now.
    pub fn live_tracked() -> i64 {
        LIVE_TRACKED.load(Ordering::SeqCst)
    }

    /// Everything JS and the components reported, in order.
    pub fn events() -> Vec<String> {
        EVENTS.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// What JS reports to the host.
#[module]
pub struct Probe;

#[module]
impl Probe {
    /// One interval tick.
    pub fn tick() {
        TICKS.fetch_add(1, Ordering::SeqCst);
    }

    /// A named event.
    pub fn report(event: String) {
        record(event);
    }
}

/// An object whose life Rust counts.
#[class]
pub struct Tracked {
    _id: u32,
}

#[class]
impl Tracked {
    /// A new tracked object.
    pub fn new() -> Self {
        LIVE_TRACKED.fetch_add(1, Ordering::SeqCst);
        Tracked { _id: 0 }
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        LIVE_TRACKED.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The props of [`Pulse`].
#[derive(Debug, Clone, PartialEq, Props)]
pub struct PulseProps {
    /// What it shows and reports.
    pub label: Prop<SharedString>,
}

/// The events of a [`Pulse`].
#[derive(Debug, Clone, PartialEq, Event)]
pub enum PulseEvent {
    /// It fired once, by itself, after mounting.
    Fire,
}

/// Records its props, then reports `onFire` once by itself (no input needed).
#[component]
pub fn Pulse(props: PulseProps, cx: Scope<PulseEvent>) -> impl IntoElement {
    record(format!("pulse-props:{}", props.label.get_untracked()));
    let _fire = set_timeout(Duration::from_millis(200), move || cx.emit(PulseEvent::Fire));
    view().child(text(props.label))
}
