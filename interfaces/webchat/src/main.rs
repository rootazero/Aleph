use aleph_panel::app::*;
use aleph_panel::panic_overlay;
use leptos::prelude::*;

fn main() {
    panic_overlay::install();
    mount_to_body(App);
}
