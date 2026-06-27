//! Phone Extensions screen (`/extensions`): Browse-first single-column store.
//! The desktop left-column `CategoryNav` is replaced by a horizontal chip bar
//! (`PhoneCategoryBar`); the responsive `BrowsePane` grid (already `grid-cols-1`
//! at phone width) and all three overlays (detail drawer / install flow /
//! installed panel, each `max-w-[94vw]` fixed) are reused verbatim. No
//! sub-routing — category + overlays are app-level `StoreState` signals, and
//! `StoreState` is provided at the app root (app.rs), so this screen holds no
//! state. The overlays sit INSIDE `PhoneShell` so its `z-[70]` stacking context
//! does not hide them (they are `z-[60]`/`z-50`). I/O-only (R4).

pub mod bar;

use leptos::prelude::*;

use crate::components::extensions::detail_drawer::ExtensionDetailDrawer;
use crate::components::extensions::install_flow::InstallFlow;
use crate::platform::phone::extensions::bar::PhoneCategoryBar;
use crate::platform::phone::shell::PhoneShell;
use crate::views::extensions::browse::BrowsePane;
use crate::views::extensions::installed::InstalledPanel;

#[component]
#[must_use]
pub fn PhoneExtensions() -> impl IntoView {
    view! {
        <PhoneShell title="Extensions">
            <div>
                <PhoneCategoryBar/>
                <BrowsePane/>
                <ExtensionDetailDrawer/>
                <InstallFlow/>
                <InstalledPanel/>
            </div>
        </PhoneShell>
    }
}
