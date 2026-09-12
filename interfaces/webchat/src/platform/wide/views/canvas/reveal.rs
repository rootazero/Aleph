//! Playback — the `reveal` / `timeline` half of the wire contract, rendered
//! as CSS animations.
//!
//! # Mechanism (srt-whiteboard's two-phase reveal, no tween library)
//!
//! Every shape carrying a `Reveal` renders inside a `<g class="rv-…">`
//! ([`shape_class`]); its outline elements carry [`OUTLINE_CLASS`] (and
//! `pathLength="1"` when the mode is `Draw`), its non-outline content
//! [`BODY_CLASS`]. [`reveal_css`] turns one reveal into `@keyframes` + rules
//! that apply **only under a root carrying [`PLAY_CLASS`]** — so a document
//! that is not being played renders exactly as it always did, and a shape
//! without a reveal is simply visible.
//!
//! - `Draw`: the outline's `stroke-dashoffset` runs 1→0 over the first two
//!   thirds of `duration_ms` (the `pathLength="1"` normalisation is what
//!   makes a single dash of length 1 cover any path), then fill and body
//!   fade in over the last third.
//! - `Fade`: the group's opacity 0→1.
//! - `Wipe`: the group's `clip-path` from `inset(0 100% 0 0)` to `inset(0)`.
//!
//! Every animation is delayed by `start_ms` with `animation-fill-mode: both`
//! (hidden before its start, held after its end). [`PAUSED_CLASS`] on the
//! same root freezes every animation in place.
//!
//! # Restart
//!
//! CSS restarts an animation when its `animation-name` changes, so the
//! keyframe names carry a **play epoch**: bumping the epoch rewrites the
//! `<style>` and every shape starts over from t=0 without touching the DOM
//! nodes. The animated SVG export uses epoch 0 and puts [`PLAY_CLASS`] on
//! its root, so it plays on open.
//!
//! # What is not here
//!
//! No tween of arbitrary properties (spec §5); a `Dashed`/`Dotted` outline
//! draws on solid while [`PLAY_CLASS`] is up (the reveal's `stroke-dasharray`
//! wins over the presentation attribute) and regains its pattern when
//! playback ends.

use std::fmt::Write as _;

use aleph_protocol::canvas::{CanvasDoc, Ease, Reveal, RevealMode, Shape};
use leptos::prelude::*;

use super::sketch::fnv1a64;
use crate::i18n::{t, use_i18n};

/// Root class under which reveal animations apply.
pub(super) const PLAY_CLASS: &str = "rv-play";
/// Root class (alongside [`PLAY_CLASS`]) that freezes every animation.
pub(super) const PAUSED_CLASS: &str = "rv-paused";
/// Class of a shape's outline elements (the ones that draw on).
pub(super) const OUTLINE_CLASS: &str = "rv-outline";
/// Class of a shape's non-outline content (fades in after the outline).
pub(super) const BODY_CLASS: &str = "rv-body";

/// The CSS class naming one shape's reveal group: a CSS-safe rendering of
/// the id plus a hash of the original, so two ids that sanitise alike
/// still get distinct rules.
#[must_use]
pub(super) fn shape_class(shape_id: &str) -> String {
    let safe: String = shape_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("rv-{safe}-{:08x}", fnv1a64(shape_id) as u32)
}

/// `pathLength="1"` is needed only for the draw-on mode.
#[must_use]
pub(super) fn needs_path_length(reveal: Option<&Reveal>) -> bool {
    reveal.is_some_and(|r| r.mode == RevealMode::Draw)
}

/// CSS timing function for an ease.
#[must_use]
pub(super) fn ease_css(ease: Ease) -> &'static str {
    match ease {
        Ease::Linear => "linear",
        Ease::EaseOut => "cubic-bezier(0.22, 1, 0.36, 1)",
        Ease::EaseInOut => "cubic-bezier(0.65, 0, 0.35, 1)",
    }
}

/// Keyframes and rules for one shape's reveal, keyed by `epoch` (module
/// doc: the epoch in the animation name is the restart mechanism).
#[must_use]
pub(super) fn reveal_css(shape_id: &str, reveal: &Reveal, epoch: u32) -> String {
    let class = shape_class(shape_id);
    let key = format!("k{epoch}-{class}");
    let ease = ease_css(reveal.ease);
    let start = reveal.start_ms;
    let mut css = String::new();
    match reveal.mode {
        RevealMode::Draw => {
            // 2:1 — outline first, then fill and body.
            let outline_ms = reveal.duration_ms / 3 * 2;
            let body_ms = reveal.duration_ms - outline_ms;
            let body_start = start.saturating_add(outline_ms);
            let _ = write!(
                css,
                "@keyframes {key}-draw{{from{{stroke-dashoffset:1}}to{{stroke-dashoffset:0}}}}\
                 @keyframes {key}-fill{{from{{fill-opacity:0}}to{{fill-opacity:1}}}}\
                 @keyframes {key}-in{{from{{opacity:0}}to{{opacity:1}}}}\
                 .{PLAY_CLASS} .{class} .{OUTLINE_CLASS}{{stroke-dasharray:1;\
                 animation:{key}-draw {outline_ms}ms {ease} {start}ms both,\
                 {key}-fill {body_ms}ms {ease} {body_start}ms both}}\
                 .{PLAY_CLASS} .{class} .{BODY_CLASS}{{\
                 animation:{key}-in {body_ms}ms {ease} {body_start}ms both}}"
            );
        }
        RevealMode::Fade => {
            let _ = write!(
                css,
                "@keyframes {key}-in{{from{{opacity:0}}to{{opacity:1}}}}\
                 .{PLAY_CLASS} .{class}{{animation:{key}-in {}ms {ease} {start}ms both}}",
                reveal.duration_ms
            );
        }
        RevealMode::Wipe => {
            let _ = write!(
                css,
                "@keyframes {key}-wipe{{from{{clip-path:inset(0 100% 0 0)}}to{{clip-path:inset(0)}}}}\
                 .{PLAY_CLASS} .{class}{{animation:{key}-wipe {}ms {ease} {start}ms both}}",
                reveal.duration_ms
            );
        }
    }
    css
}

/// The whole document's playback stylesheet for `epoch`: every revealed
/// shape's rules plus the pause rule. Empty when nothing reveals — the
/// caller emits no `<style>` at all, so an un-animated document's markup is
/// byte-identical with or without playback support.
#[must_use]
pub(super) fn playback_css(doc: &CanvasDoc, epoch: u32) -> String {
    playback_css_for(&doc.shapes, epoch)
}

/// [`playback_css`] over an explicit shape list — what the export feeds
/// with the selected layers, so an exported subset animates exactly the
/// shapes it contains.
#[must_use]
pub(super) fn playback_css_for(shapes: &[Shape], epoch: u32) -> String {
    let mut css = String::new();
    for shape in shapes {
        let common = shape.common();
        if let Some(reveal) = &common.reveal {
            css.push_str(&reveal_css(&common.id, reveal, epoch));
        }
    }
    if !css.is_empty() {
        let _ = write!(
            css,
            ".{PLAY_CLASS}.{PAUSED_CLASS} *{{animation-play-state:paused}}"
        );
    }
    css
}

/// Playback length: `timeline.total_ms` when set, else the latest reveal
/// end; plus `hold_ms`. `0` when no shape reveals — there is nothing to
/// play, whatever the timeline says.
#[must_use]
pub(super) fn timeline_total_ms(doc: &CanvasDoc) -> u32 {
    let latest_end = doc
        .shapes
        .iter()
        .filter_map(|s| s.common().reveal.as_ref())
        .map(|r| r.start_ms.saturating_add(r.duration_ms))
        .max();
    let Some(latest_end) = latest_end else {
        return 0;
    };
    let timeline = doc.timeline.unwrap_or_default();
    timeline
        .total_ms
        .unwrap_or(latest_end)
        .saturating_add(timeline.hold_ms)
}

/// Playback state of one surface (the editor, or the presentation stage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Playback {
    #[default]
    Idle,
    Playing {
        /// Bumped on every (re)start — the animation-name key.
        epoch: u32,
        paused: bool,
    },
}

impl Playback {
    /// The next epoch: a fresh run from t=0.
    #[must_use]
    pub(super) fn restarted(self) -> Self {
        let epoch = match self {
            Self::Idle => 1,
            Self::Playing { epoch, .. } => epoch.wrapping_add(1),
        };
        Self::Playing {
            epoch,
            paused: false,
        }
    }

    #[must_use]
    fn toggled_pause(self) -> Self {
        match self {
            Self::Idle => Self::Idle,
            Self::Playing { epoch, paused } => Self::Playing {
                epoch,
                paused: !paused,
            },
        }
    }

    /// The root classes for this state (space-separated, possibly empty).
    #[must_use]
    pub(super) fn root_class(self) -> String {
        match self {
            Self::Idle => String::new(),
            Self::Playing { paused: false, .. } => PLAY_CLASS.to_string(),
            Self::Playing { paused: true, .. } => format!("{PLAY_CLASS} {PAUSED_CLASS}"),
        }
    }

    #[must_use]
    fn epoch(self) -> Option<u32> {
        match self {
            Self::Idle => None,
            Self::Playing { epoch, .. } => Some(epoch),
        }
    }
}

/// The document's playback `<style>` for the current epoch — nothing at all
/// while idle or when no shape reveals.
#[component]
pub(super) fn PlaybackStyle(
    doc: RwSignal<Option<CanvasDoc>>,
    playback: RwSignal<Playback>,
) -> impl IntoView {
    let css = Memo::new(move |_| {
        let epoch = playback.get().epoch()?;
        doc.with(|d| d.as_ref().map(|d| playback_css(d, epoch)))
            .filter(|css| !css.is_empty())
    });
    move || css.get().map(|css| view! { <style>{css}</style> })
}

/// Play / pause / resume for one surface. Hidden when the document has no
/// playback ([`timeline_total_ms`] = 0). Play restarts from t=0; the run
/// ends (root class dropped, static rendering restored) after the
/// timeline's length — a resume re-arms that timer for the full length,
/// which only ever holds the finished frame longer, never cuts it short.
#[component]
pub(super) fn PlaybackButton(
    doc: RwSignal<Option<CanvasDoc>>,
    playback: RwSignal<Playback>,
    /// Extra classes for the button (the two surfaces style it differently).
    #[prop(into)]
    class: String,
) -> impl IntoView {
    let i18n = use_i18n();
    let total_ms = Memo::new(move |_| doc.with(|d| d.as_ref().map_or(0, timeline_total_ms)));
    let timer: StoredValue<Option<leptos::leptos_dom::helpers::TimeoutHandle>, LocalStorage> =
        StoredValue::new_local(None);
    let clear_timer = move || {
        if let Some(h) = timer.try_update_value(Option::take).flatten() {
            h.clear();
        }
    };
    let arm_timer = move || {
        clear_timer();
        let ms = u64::from(total_ms.get_untracked());
        if let Ok(h) = leptos::leptos_dom::helpers::set_timeout_with_handle(
            move || {
                let _ = playback.try_set(Playback::Idle);
            },
            std::time::Duration::from_millis(ms),
        ) {
            timer.set_value(Some(h));
        }
    };
    on_cleanup(clear_timer);
    let on_play = move |ev: web_sys::MouseEvent| {
        ev.stop_propagation();
        playback.update(|p| *p = p.restarted());
        arm_timer();
    };
    let on_pause = move |ev: web_sys::MouseEvent| {
        ev.stop_propagation();
        playback.update(|p| *p = p.toggled_pause());
        match playback.get_untracked() {
            Playback::Playing { paused: true, .. } => clear_timer(),
            _ => arm_timer(),
        }
    };
    view! {
        {move || {
            (total_ms.get() > 0).then(|| {
                let class = class.clone();
                match playback.get() {
                    Playback::Idle => view! {
                        <button class=class on:click=on_play>
                            {t!(i18n, canvas.reveal_play)}
                        </button>
                    }
                    .into_any(),
                    Playback::Playing { paused, .. } => view! {
                        <button class=class on:click=on_pause>
                            {if paused {
                                t!(i18n, canvas.reveal_resume).into_any()
                            } else {
                                t!(i18n, canvas.reveal_pause).into_any()
                            }}
                        </button>
                    }
                    .into_any(),
                }
            })
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::canvas::{FracIndex, ShapeCommon, ShapeStyle, Timeline};

    fn shape(id: &str, reveal: Option<Reveal>) -> Shape {
        Shape::Note {
            common: ShapeCommon {
                id: id.to_string(),
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                z: FracIndex::first(),
                parent_id: None,
                reveal,
            },
            style: ShapeStyle::default(),
            text: String::new(),
        }
    }

    fn doc(shapes: Vec<Shape>, timeline: Option<Timeline>) -> CanvasDoc {
        CanvasDoc {
            id: "cv-1".to_string(),
            title: "t".to_string(),
            owner_user_id: None,
            project_id: None,
            revision: 1,
            shapes,
            decks: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
            timeline,
        }
    }

    fn reveal(start_ms: u32, duration_ms: u32, mode: RevealMode) -> Reveal {
        Reveal {
            start_ms,
            duration_ms,
            ease: Ease::EaseOut,
            mode,
        }
    }

    #[test]
    fn shape_class_is_css_safe_and_collision_resistant() {
        let a = shape_class("s 1/x");
        assert!(a.starts_with("rv-s_1_x-"), "{a}");
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_ne!(a, shape_class("s_1_x"), "same sanitised text, different id");
        assert_eq!(shape_class("s-1"), shape_class("s-1"));
    }

    /// Draw: outline dash-offset over the first two thirds, fill and body
    /// over the last third, both delayed by `start_ms`, epoch in the name.
    #[test]
    fn draw_reveal_is_two_phase_and_keyed_by_epoch() {
        let css = reveal_css("s-1", &reveal(500, 900, RevealMode::Draw), 3);
        let class = shape_class("s-1");
        assert!(
            css.contains(&format!("@keyframes k3-{class}-draw")),
            "{css}"
        );
        assert!(
            css.contains(&format!(
                ".rv-play .{class} .rv-outline{{stroke-dasharray:1;animation:k3-{class}-draw 600ms cubic-bezier(0.22, 1, 0.36, 1) 500ms both,k3-{class}-fill 300ms cubic-bezier(0.22, 1, 0.36, 1) 1100ms both}}"
            )),
            "{css}"
        );
        assert!(
            css.contains(&format!(
                ".rv-play .{class} .rv-body{{animation:k3-{class}-in 300ms"
            )),
            "{css}"
        );
        assert_ne!(
            css,
            reveal_css("s-1", &reveal(500, 900, RevealMode::Draw), 4),
            "a new epoch renames"
        );
    }

    #[test]
    fn fade_and_wipe_animate_the_group_and_eases_map_to_curves() {
        let fade = reveal_css("s-1", &reveal(0, 400, RevealMode::Fade), 0);
        let class = shape_class("s-1");
        assert!(
            fade.contains(&format!(".rv-play .{class}{{animation:k0-{class}-in 400ms")),
            "{fade}"
        );
        assert!(
            !fade.contains("rv-outline"),
            "fade touches no outline: {fade}"
        );
        let wipe = reveal_css(
            "s-1",
            &Reveal {
                ease: Ease::Linear,
                ..reveal(10, 20, RevealMode::Wipe)
            },
            0,
        );
        assert!(
            wipe.contains("inset(0 100% 0 0)") && wipe.contains("inset(0)"),
            "{wipe}"
        );
        assert!(wipe.contains("20ms linear 10ms both"), "{wipe}");
        assert_eq!(ease_css(Ease::EaseInOut), "cubic-bezier(0.65, 0, 0.35, 1)");
        assert!(needs_path_length(Some(&reveal(0, 1, RevealMode::Draw))));
        assert!(!needs_path_length(Some(&reveal(0, 1, RevealMode::Fade))));
        assert!(!needs_path_length(None));
    }

    /// The document stylesheet is empty without reveals — the exact
    /// condition under which the callers emit no `<style>` — and carries
    /// the pause rule once anything reveals.
    #[test]
    fn playback_css_is_empty_without_reveals_and_carries_the_pause_rule_with_them() {
        assert_eq!(playback_css(&doc(vec![shape("a", None)], None), 0), "");
        let css = playback_css(
            &doc(
                vec![
                    shape("a", Some(reveal(0, 100, RevealMode::Fade))),
                    shape("b", None),
                ],
                None,
            ),
            2,
        );
        assert!(css.contains(&shape_class("a")));
        assert!(!css.contains(&shape_class("b")));
        assert!(
            css.ends_with(".rv-play.rv-paused *{animation-play-state:paused}"),
            "{css}"
        );
    }

    #[test]
    fn timeline_total_is_the_latest_end_or_the_override_plus_hold_and_zero_without_reveals() {
        let none = doc(
            vec![shape("a", None)],
            Some(Timeline {
                total_ms: Some(5000),
                hold_ms: 100,
            }),
        );
        assert_eq!(
            timeline_total_ms(&none),
            0,
            "nothing reveals ⇒ nothing to play"
        );
        let two = vec![
            shape("a", Some(reveal(100, 200, RevealMode::Draw))),
            shape("b", Some(reveal(1000, 500, RevealMode::Fade))),
        ];
        assert_eq!(timeline_total_ms(&doc(two.clone(), None)), 1500);
        assert_eq!(
            timeline_total_ms(&doc(
                two.clone(),
                Some(Timeline {
                    total_ms: None,
                    hold_ms: 250
                })
            )),
            1750
        );
        assert_eq!(
            timeline_total_ms(&doc(
                two,
                Some(Timeline {
                    total_ms: Some(3000),
                    hold_ms: 250
                })
            )),
            3250
        );
    }

    #[test]
    fn playback_state_restarts_with_a_fresh_epoch_and_pauses_in_place() {
        let p = Playback::Idle.restarted();
        assert_eq!(
            p,
            Playback::Playing {
                epoch: 1,
                paused: false
            }
        );
        assert_eq!(
            p.restarted(),
            Playback::Playing {
                epoch: 2,
                paused: false
            }
        );
        let paused = p.toggled_pause();
        assert_eq!(
            paused,
            Playback::Playing {
                epoch: 1,
                paused: true
            }
        );
        assert_eq!(paused.root_class(), "rv-play rv-paused");
        assert_eq!(p.root_class(), "rv-play");
        assert_eq!(Playback::Idle.root_class(), "");
        assert_eq!(Playback::Idle.toggled_pause(), Playback::Idle);
        assert_eq!(
            paused.toggled_pause(),
            p,
            "resuming keeps the epoch — no restart"
        );
    }
}
