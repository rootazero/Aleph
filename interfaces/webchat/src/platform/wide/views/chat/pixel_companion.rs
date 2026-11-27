use super::state::{ChatPhase, ChatState};
use leptos::prelude::*;

// An original 11×10 sprite. Each palette entry is rendered with one shadow layer.
const SPRITE: [&str; 10] = [
    "    +++    ",
    " + +%%%+ + ",
    " ++%%%%%++ ",
    "+%%%%#%%%%+",
    "+%o%%#%%o%+",
    "+%%%%#%%%%+",
    " +%%ooo%%+ ",
    "  +%%%%%+  ",
    "  ++ + ++  ",
    "  ++   ++  ",
];

fn pixels(layer: char) -> String {
    SPRITE
        .iter()
        .enumerate()
        .flat_map(|(y, row)| {
            row.chars().enumerate().filter_map(move |(x, pixel)| {
                (pixel == layer).then(|| format!("{}px {}px 0 currentColor", x * 2, y * 2))
            })
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[component]
pub(super) fn PixelCompanion() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let visible = RwSignal::new(!document().hidden());
    let listener = window_event_listener_untyped("visibilitychange", move |_| {
        visible.set(!document().hidden());
    });
    on_cleanup(move || listener.remove());
    let status = Memo::new(
        move |previous: Option<&(Option<String>, bool, &'static str)>| {
            let session = chat.session_key.get();
            let phase = chat.phase.get();
            let busy = phase.is_busy();
            let pose = if busy {
                "working"
            } else if phase == ChatPhase::Idle
                && previous.is_some_and(|(key, was_busy, _)| key == &session && *was_busy)
            {
                "complete"
            } else {
                "idle"
            };
            (session, busy, pose)
        },
    );
    view! {
        <span class="panel-pixel-companion" aria-hidden="true" data-pose=move || status.with(|status| status.2)
            data-visible=move || if visible.get() { "true" } else { "false" }>
            <span class="panel-pixel-sprite">
                <span class="panel-pixel-outline" style:box-shadow=pixels('+')></span>
                <span class="panel-pixel-body" style:box-shadow=pixels('%')></span>
                <span class="panel-pixel-highlight" style:box-shadow=pixels('#')></span>
                <span class="panel-pixel-eyes" style:box-shadow=pixels('o')></span>
            </span>
        </span>
    }
}
