//! Fullscreen deck playback — the presentation overlay and the pure camera
//! fit math under it.
//!
//! A slide is a live [`Shape::Frame`] on the canvas (`Deck.frame_ids` — the
//! frames ARE the slides, playing copies nothing). The overlay renders the
//! *whole document* through the same `ShapeView` / `HtmlFrameOverlay` layers
//! as the editor, under a camera that fits the current frame's bbox to the
//! window ([`present_camera_for_frame`]), and clips everything outside the
//! frame away with a screen-space `clip-path` ([`clip_inset`]) — so a slide
//! shows exactly the canvas region its frame covers, letterboxed on the
//! unfitted axis.
//!
//! Navigation: →/↓/Space/PageDown and click advance, ←/↑/PageUp go back,
//! Esc (or the exit button) closes, the progress dots jump. While the
//! overlay is mounted the editor's own window key handler stands down (its
//! `presenting` guard) — otherwise an arrow key would nudge the selection
//! under the show.

use aleph_protocol::canvas::{Deck, Shape};
use leptos::prelude::*;

use super::interaction::Bbox;
use super::shape_view::{HtmlFrameOverlay, ShapeView};
use super::viewport;
use crate::i18n::{t, use_i18n};
use crate::state::canvas::{Camera, CanvasState};

/// Degenerate-input floor, world units / CSS px: a zero-size frame or a
/// zero-size window must yield a finite camera, not NaN (which CSS would
/// silently drop, rendering the whole stage untransformed).
const MIN_EXTENT: f64 = 1.0;

// ---------------------------------------------------------------------------
// Pure fit math — unit-tested, zero DOM.
// ---------------------------------------------------------------------------

/// The camera that fits `frame` inside `viewport` (CSS px), centered:
/// `zoom = min(vw/fw, vh/fh)` (contain, never crop), the frame's world
/// center on the viewport's center. Deliberately NOT clamped to the editor's
/// `MIN_ZOOM`/`MAX_ZOOM` — a small frame blown up to fullscreen is the
/// point of presenting, not a runaway gesture.
#[must_use]
pub(super) fn present_camera_for_frame(frame: Bbox, viewport: (f64, f64)) -> Camera {
    let sane = |v: f64| {
        if v.is_finite() && v > 0.0 {
            v
        } else {
            MIN_EXTENT
        }
    };
    let (vw, vh) = (sane(viewport.0), sane(viewport.1));
    let (fw, fh) = (sane(frame.w), sane(frame.h));
    let zoom = (vw / fw).min(vh / fh);
    let cx = frame.x + frame.w / 2.0;
    let cy = frame.y + frame.h / 2.0;
    Camera {
        x: cx - vw / zoom / 2.0,
        y: cy - vh / zoom / 2.0,
        zoom,
    }
}

/// Screen-space `clip-path: inset(top right bottom left)` distances hiding
/// everything outside `frame` under `cam`. Each inset is clamped at 0 —
/// float rounding must not produce a negative inset (which CSS rejects,
/// dropping the clip entirely).
#[must_use]
pub(super) fn clip_inset(frame: Bbox, cam: Camera, viewport: (f64, f64)) -> (f64, f64, f64, f64) {
    let (sx, sy) = viewport::world_to_screen(cam, frame.x, frame.y);
    let sw = frame.w * cam.zoom;
    let sh = frame.h * cam.zoom;
    let top = sy.max(0.0);
    let left = sx.max(0.0);
    let right = (viewport.0 - (sx + sw)).max(0.0);
    let bottom = (viewport.1 - (sy + sh)).max(0.0);
    (top, right, bottom, left)
}

/// The deck's playable slides: `frame_ids` resolved against the live
/// shapes, in deck order, ids that no longer resolve to a [`Shape::Frame`]
/// dropped. (The drawer keeps showing them as "missing" rows; playback
/// cannot — there is no bbox to fit.)
#[must_use]
pub(super) fn slide_frames(shapes: &[Shape], deck: &Deck) -> Vec<(String, Bbox)> {
    deck.frame_ids
        .iter()
        .filter_map(|fid| {
            shapes
                .iter()
                .find(|s| matches!(s, Shape::Frame { .. }) && s.id() == fid.as_str())
                .map(|s| (fid.clone(), Bbox::of_shape(s)))
        })
        .collect()
}

/// Advance (`forward`) or retreat the slide index, clamped at both ends —
/// clicking past the last slide stays on it (Esc is the exit, not an
/// accidental extra click). A `current` beyond `len` (the deck shrank under
/// a broadcast) clamps before stepping.
#[must_use]
pub(super) fn step_index(len: usize, current: usize, forward: bool) -> usize {
    if len == 0 {
        return 0;
    }
    let cur = current.min(len - 1);
    if forward {
        (cur + 1).min(len - 1)
    } else {
        cur.saturating_sub(1)
    }
}

// ---------------------------------------------------------------------------
// Overlay component.
// ---------------------------------------------------------------------------

/// Window inner size in CSS px, with the shell's SSR-safe fallback
/// (`state/viewport.rs` idiom — unreadable ⇒ a sane desktop size).
fn window_size() -> (f64, f64) {
    let Some(win) = web_sys::window() else {
        return (1280.0, 800.0);
    };
    let w = win
        .inner_width()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(1280.0);
    let h = win
        .inner_height()
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(800.0);
    (w, h)
}

/// Fullscreen deck playback (module doc). Mounted by the editor while its
/// `presenting` signal holds this deck's id; every keyboard/click surface
/// stops propagation so nothing reaches the editor's input plane below.
#[component]
pub(super) fn PresentOverlay(
    /// The deck to play, by id — resolved against the live document every
    /// render, so broadcast edits (reorder, frame moves) reflow the show.
    deck_id: String,
    /// Exit: Esc, the exit button, or the deck vanishing under a broadcast.
    on_close: Callback<()>,
) -> impl IntoView {
    let canvas = expect_context::<CanvasState>();
    let i18n = use_i18n();

    let viewport_size = RwSignal::new(window_size());
    let resize_handle = window_event_listener(leptos::ev::resize, move |_| {
        viewport_size.set(window_size());
    });

    let deck_key = StoredValue::new(deck_id);
    let slides = Memo::new(move |_| {
        canvas.doc.with(|d| {
            d.as_ref()
                .and_then(|d| {
                    let id = deck_key.get_value();
                    d.decks
                        .iter()
                        .find(|k| k.id == id)
                        .map(|k| slide_frames(&d.shapes, k))
                })
                .unwrap_or_default()
        })
    });
    let index = RwSignal::new(0usize);

    // A deck deleted — or emptied of live frames — under a broadcast closes
    // the show: there is nothing left to fit, and an overlay that eats every
    // key while showing a black void is a lock, not a viewer.
    Effect::new(move |_| {
        if slides.get().is_empty() {
            on_close.run(());
        }
    });

    let current = Memo::new(move |_| {
        let s = slides.get();
        if s.is_empty() {
            None
        } else {
            Some(s[index.get().min(s.len() - 1)].clone())
        }
    });
    let camera = Memo::new(move |_| {
        current
            .get()
            .map(|(_, b)| present_camera_for_frame(b, viewport_size.get()))
    });

    let advance = move |forward: bool| {
        let len = slides.with_untracked(Vec::len);
        index.update(|i| *i = step_index(len, *i, forward));
    };

    let key_handle = window_event_listener(
        leptos::ev::keydown,
        move |ev: web_sys::KeyboardEvent| match ev.key().as_str() {
            "Escape" => {
                ev.prevent_default();
                on_close.run(());
            }
            "ArrowRight" | "ArrowDown" | "PageDown" | " " => {
                ev.prevent_default();
                advance(true);
            }
            "ArrowLeft" | "ArrowUp" | "PageUp" => {
                ev.prevent_default();
                advance(false);
            }
            _ => {}
        },
    );
    on_cleanup(move || {
        key_handle.remove();
        resize_handle.remove();
    });

    // The same layer structure as the editor: SVG shapes under one world
    // transform, HTML frames on a CSS-transformed overlay above them.
    let sorted_ids = Memo::new(move |_| {
        canvas.doc.with(|d| {
            d.as_ref()
                .map(|d| super::editor::z_sorted_ids(&d.shapes))
                .unwrap_or_default()
        })
    });
    let shape_map = Memo::new(move |_| {
        canvas.doc.with(|d| {
            d.as_ref()
                .map(|d| super::editor::shapes_by_id(&d.shapes))
                .unwrap_or_default()
        })
    });

    // The stage: the canvas surface, clipped to the current frame — black
    // letterbox everywhere else. `display: none` while there is no slide
    // (the emptiness Effect above is already closing the overlay).
    let stage_style = move || {
        let (Some((_, frame)), Some(cam)) = (current.get(), camera.get()) else {
            return "display: none".to_string();
        };
        let (top, right, bottom, left) = clip_inset(frame, cam, viewport_size.get());
        format!("clip-path: inset({top}px {right}px {bottom}px {left}px)")
    };

    view! {
        <div
            class="fixed inset-0 z-50 bg-black overflow-hidden select-none cursor-pointer"
            style="touch-action: none"
            on:pointerdown=|ev: web_sys::PointerEvent| ev.stop_propagation()
            on:pointermove=|ev: web_sys::PointerEvent| ev.stop_propagation()
            on:pointerup=|ev: web_sys::PointerEvent| ev.stop_propagation()
            on:dblclick=|ev: web_sys::MouseEvent| ev.stop_propagation()
            on:wheel=|ev: web_sys::WheelEvent| ev.stop_propagation()
            on:click=move |ev: web_sys::MouseEvent| {
                ev.stop_propagation();
                advance(true);
            }
        >
            <div class="absolute inset-0 bg-surface" style=stage_style>
                <svg class="absolute inset-0 w-full h-full block">
                    <g transform=move || {
                        viewport::svg_transform(camera.get().unwrap_or_default())
                    }>
                        <For
                            each=move || sorted_ids.get()
                            key=Clone::clone
                            children=move |id: String| {
                                let shape =
                                    Memo::new(move |_| shape_map.with(|m| m.get(&id).cloned()));
                                view! { <ShapeView shape=shape /> }
                            }
                        />
                    </g>
                </svg>
                <div
                    class="absolute inset-0 pointer-events-none"
                    style=move || viewport::css_transform(camera.get().unwrap_or_default())
                >
                    <HtmlFrameOverlay />
                </div>
            </div>

            <button
                class="absolute top-4 right-4 px-3 py-1.5 rounded-lg bg-black/50 hover:bg-black/70 \
                       text-white/80 hover:text-white text-xs font-medium transition-colors"
                on:click=move |ev: web_sys::MouseEvent| {
                    ev.stop_propagation();
                    on_close.run(());
                }
            >
                {t!(i18n, canvas.present_exit)}
            </button>

            <div
                class="absolute bottom-5 left-1/2 -translate-x-1/2 flex items-center gap-3 \
                       px-3.5 py-2 rounded-full bg-black/50 cursor-default"
                on:click=|ev: web_sys::MouseEvent| ev.stop_propagation()
            >
                <span class="text-[11px] text-white/70 tabular-nums">
                    {move || {
                        let n = slides.get().len();
                        let cur = if n == 0 { 0 } else { index.get().min(n - 1) + 1 };
                        format!("{cur} / {n}")
                    }}
                </span>
                <div class="flex items-center gap-1.5">
                    {move || {
                        let n = slides.get().len();
                        let cur = if n == 0 { 0 } else { index.get().min(n - 1) };
                        (0..n)
                            .map(|i| {
                                let class = if i == cur {
                                    "w-2 h-2 rounded-full bg-white cursor-pointer"
                                } else {
                                    "w-2 h-2 rounded-full bg-white/30 hover:bg-white/60 \
                                     cursor-pointer transition-colors"
                                };
                                view! {
                                    <button
                                        class=class
                                        on:click=move |ev: web_sys::MouseEvent| {
                                            ev.stop_propagation();
                                            index.set(i);
                                        }
                                    ></button>
                                }
                            })
                            .collect_view()
                    }}
                </div>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(x: f64, y: f64, w: f64, h: f64) -> Bbox {
        Bbox { x, y, w, h }
    }

    /// Landscape frame in a square viewport: width is the binding axis
    /// (`zoom = vw/fw`), and the frame's world center lands on the
    /// viewport's center.
    #[test]
    fn fit_zoom_is_the_min_ratio_and_centers_the_frame() {
        let frame = bbox(100.0, 200.0, 400.0, 300.0);
        let cam = present_camera_for_frame(frame, (800.0, 800.0));
        assert!(
            (cam.zoom - 2.0).abs() < 1e-9,
            "min(800/400, 800/300) = 2, got {}",
            cam.zoom
        );
        let center = viewport::world_to_screen(cam, 300.0, 350.0);
        assert!(
            (center.0 - 400.0).abs() < 1e-9 && (center.1 - 400.0).abs() < 1e-9,
            "frame center must sit on the viewport center, got {center:?}"
        );
    }

    /// The fitted axis has zero inset; the letterboxed axis splits its slack
    /// evenly between its two sides.
    #[test]
    fn letterbox_is_symmetric_on_the_unfitted_axis() {
        let frame = bbox(100.0, 200.0, 400.0, 300.0);
        let viewport_px = (800.0, 800.0);
        let cam = present_camera_for_frame(frame, viewport_px);
        let (top, right, bottom, left) = clip_inset(frame, cam, viewport_px);
        assert!((left - 0.0).abs() < 1e-9 && (right - 0.0).abs() < 1e-9);
        assert!(
            (top - 100.0).abs() < 1e-9 && (bottom - 100.0).abs() < 1e-9,
            "800 viewport − 600 scaled frame = 200, split evenly; got top={top} bottom={bottom}"
        );
    }

    /// Portrait frame in a landscape viewport: height binds, the slack is
    /// horizontal.
    #[test]
    fn portrait_frame_in_landscape_viewport_fits_height() {
        let frame = bbox(0.0, 0.0, 100.0, 200.0);
        let viewport_px = (1000.0, 400.0);
        let cam = present_camera_for_frame(frame, viewport_px);
        assert!((cam.zoom - 2.0).abs() < 1e-9, "min(10, 2) = 2");
        let (top, right, bottom, left) = clip_inset(frame, cam, viewport_px);
        assert!((top - 0.0).abs() < 1e-9 && (bottom - 0.0).abs() < 1e-9);
        assert!(
            (left - 400.0).abs() < 1e-9 && (right - 400.0).abs() < 1e-9,
            "1000 − 200 scaled width = 800, split evenly; got left={left} right={right}"
        );
    }

    /// Zero-size frames and zero-size viewports produce a finite camera and
    /// non-negative insets — never NaN, never a negative CSS inset.
    #[test]
    fn degenerate_frame_and_viewport_stay_finite() {
        for (frame, vp) in [
            (bbox(10.0, 10.0, 0.0, 0.0), (800.0, 600.0)),
            (bbox(0.0, 0.0, 100.0, 50.0), (0.0, 0.0)),
            (bbox(0.0, 0.0, 0.0, 0.0), (0.0, 0.0)),
        ] {
            let cam = present_camera_for_frame(frame, vp);
            assert!(
                cam.x.is_finite() && cam.y.is_finite() && cam.zoom.is_finite() && cam.zoom > 0.0,
                "degenerate input must stay finite: {cam:?}"
            );
            let (t, r, b, l) = clip_inset(frame, cam, vp);
            assert!(
                t >= 0.0 && r >= 0.0 && b >= 0.0 && l >= 0.0,
                "insets must clamp at zero: {:?}",
                (t, r, b, l)
            );
        }
    }

    /// Slides resolve in deck order; ids that are gone or point at
    /// non-frames drop out of playback.
    #[test]
    fn slide_frames_resolves_deck_order_and_drops_dead_ids() {
        use aleph_protocol::canvas::{FracIndex, ShapeCommon, ShapeStyle};
        let common = |id: &str, x: f64| ShapeCommon {
            id: id.to_string(),
            x,
            y: 0.0,
            w: 160.0,
            h: 90.0,
            z: FracIndex::first(),
            parent_id: None,
        };
        let shapes = vec![
            Shape::Frame {
                common: common("a", 0.0),
                title: String::new(),
                aspect_locked: false,
            },
            Shape::Frame {
                common: common("b", 200.0),
                title: String::new(),
                aspect_locked: false,
            },
            Shape::Note {
                common: common("n", 400.0),
                style: ShapeStyle::default(),
                text: String::new(),
            },
        ];
        let deck = Deck {
            id: "d1".to_string(),
            title: "T".to_string(),
            frame_ids: ["b", "gone", "n", "a"]
                .iter()
                .map(ToString::to_string)
                .collect(),
        };
        let slides = slide_frames(&shapes, &deck);
        let got: Vec<(&str, f64)> = slides.iter().map(|(id, b)| (id.as_str(), b.x)).collect();
        assert_eq!(
            got,
            vec![("b", 200.0), ("a", 0.0)],
            "deck order, dead ids and non-frames dropped"
        );
    }

    /// The index clamps at both ends, survives an empty deck, and clamps a
    /// stale `current` before stepping.
    #[test]
    fn step_index_clamps_at_both_ends() {
        assert_eq!(step_index(3, 0, true), 1);
        assert_eq!(step_index(3, 2, true), 2, "clamped at the last slide");
        assert_eq!(step_index(3, 1, false), 0);
        assert_eq!(step_index(3, 0, false), 0, "clamped at the first slide");
        assert_eq!(step_index(0, 5, true), 0, "empty deck");
        assert_eq!(
            step_index(3, 9, false),
            1,
            "a stale index clamps to the end (2) before stepping back"
        );
    }
}
