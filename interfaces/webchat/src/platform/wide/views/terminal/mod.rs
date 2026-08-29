//! Embedded terminal view.
//!
//! The VT emulator is on the server (see `src/gateway/pty/screen/`), so this
//! view is a renderer: it subscribes to `pty.screen`, paints a grid, and
//! sends keystrokes. Unmounting is lossless — the screen survives on the
//! server and `pty.attach` restores it — which is why the subscription is
//! ephemeral and there is no park/reveal machinery here.
//!
//! This module is the wide/desktop entry point only (Task 14). There is no
//! phone screen yet — `app.rs`'s `MainContent` renders nothing for
//! `PanelMode::Terminal` on a phone form factor, the same treatment
//! `PanelMode::Projects` already gets there.

use leptos::prelude::*;

pub mod session;

#[component]
pub fn TerminalView() -> impl IntoView {
    view! {
        <div class="flex flex-1 min-w-0 min-h-0 flex-col" data-terminal-view="">
            <div class="flex-1 min-h-0 grid place-items-center text-text-secondary">
                "Terminal"
            </div>
        </div>
    }
}
