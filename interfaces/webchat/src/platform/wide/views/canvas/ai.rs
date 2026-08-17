//! AI flows over the whiteboard — the Panel half of spec §4's three flows,
//! and deliberately the *dumb* half (R4/R7): every function here either edits
//! the document through the same ops funnel as a drag, or sends one
//! structured chat message into the current conversation. The model does the
//! actual reading (`canvas` get), generating (`image_generate`) and placing
//! (`canvas` insert_image); nothing in this file interprets a prompt.
//!
//! # The two flows this module owns
//!
//! 1. **AI image frame** ([`AiFramePanel`]): select an [`Shape::AiImageFrame`]
//!    (optionally together with `Image` shapes — those become the reference
//!    pick), type a prompt, hit Generate. The frame is re-upserted with
//!    `status: Pending` + the reference asset ids *before* the send, so every
//!    client watching the broadcast sees the badge flip; the chat message
//!    carries the reference images as attachments (fetched through
//!    `canvas.asset.get` — the same RPC the HTML frames use, because the
//!    capability byte route is for `<image href>`, not for reading bytes
//!    back).
//! 2. **Annotation regeneration** ([`AnnotateButton`]): select exactly one
//!    `Image` plus the annotation shapes drawn over it. The button composites
//!    an annotated PNG (`export.rs` rasterizer), stores it as a canvas asset,
//!    and sends original + annotated as two attachments with an instruction
//!    to place the regenerated image at `x + w + 40` — beside the original,
//!    never over it.
//!
//! # The message templates are wire contract, not copy
//!
//! [`generation_message`] / [`annotation_message`] name two tools by their
//! exact registry names — `canvas` and `image_generate` — and spell three
//! steps. Prose naming a tool is a second copy of that tool's name (§4.11
//! round-12): the unit tests below pin the spelling, and the server-side QA
//! suite (Task 20) resolves the names against the real tool table.
//!
//! # Why the sends mirror the voice path
//!
//! `ChatApi::send` here forwards the session's agent binding, project room,
//! model pick and dials exactly like `views/voice/mod.rs` — the lesson that
//! path learned: a send site that passes `None`s silently ignores the
//! session's agent/model/tier selections while every pill keeps showing them.
//! The dials go through `session_dials_for_send`, the one shared rule (the
//! `every_send_path_resolves_the_dials_through_the_shared_rule` guard scans
//! this file).

use aleph_protocol::canvas::{AiFrameStatus, CanvasDoc, CanvasOp, FracIndex, Shape, ShapeCommon};
use leptos::prelude::*;
use leptos::task::spawn_local;
use shared_ui_logic::state::session_dials_for_send;

use super::interaction::Bbox;
use super::{asset_ingest, export, ops};
use crate::api::canvas::CanvasApi;
use crate::api::chat::{ChatApi, ChatAttachment};
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use crate::state::canvas::CanvasState;
use crate::state::sessions::SessionMap;
use crate::views::chat::ChatState;

/// Default bbox for a freshly inserted AI image frame, world units. 3:2 —
/// roomy enough for the prompt excerpt and the status badge the SVG layer
/// draws (`shape_view::ai_frame_svg`).
pub(super) const AI_FRAME_SIZE: (f64, f64) = (360.0, 240.0);

/// How far right of the original an annotation-regenerated image is asked to
/// land: `x + w + 40` (spec §4 flow 3).
const ANNOTATION_GAP: f64 = 40.0;

// ---------------------------------------------------------------------------
// Pure selection predicates — unit-tested, zero DOM.
// ---------------------------------------------------------------------------

/// The AI image frame the panel operates on: exactly one
/// [`Shape::AiImageFrame`] among the selected shapes. Two selected frames
/// answer `None` — "which frame gets this prompt" must never be a guess.
#[must_use]
pub(super) fn selected_ai_frame(shapes: &[Shape], selection: &[String]) -> Option<Shape> {
    let mut found: Option<&Shape> = None;
    for shape in shapes {
        if matches!(shape, Shape::AiImageFrame { .. })
            && selection.iter().any(|id| id == shape.id())
        {
            if found.is_some() {
                return None;
            }
            found = Some(shape);
        }
    }
    found.cloned()
}

/// The reference pick: every selected [`Shape::Image`], as
/// `(shape_id, asset_id)` in document order. Selecting images *together
/// with* the frame is how references are chosen — there is no separate
/// picker state to drift from the selection.
#[must_use]
pub(super) fn selected_reference_images(
    shapes: &[Shape],
    selection: &[String],
) -> Vec<(String, String)> {
    shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Image {
                common, asset_id, ..
            } if selection.iter().any(|id| id == &common.id) => {
                Some((common.id.clone(), asset_id.clone()))
            }
            _ => None,
        })
        .collect()
}

/// The frame as it goes on the wire when Generate is pressed: the typed
/// prompt, the reference pick, and `status: Pending`. `None` for anything
/// that is not an AI image frame.
#[must_use]
pub(super) fn armed_frame(frame: &Shape, prompt: &str, refs: &[String]) -> Option<Shape> {
    match frame {
        Shape::AiImageFrame { common, .. } => Some(Shape::AiImageFrame {
            common: common.clone(),
            prompt: prompt.to_string(),
            reference_asset_ids: refs.to_vec(),
            status: AiFrameStatus::Pending,
        }),
        _ => None,
    }
}

/// The same frame with only its status changed — the failure path's op
/// (a send that never reached the model must not leave the badge on
/// "Generating…" forever).
#[must_use]
pub(super) fn frame_with_status(frame: &Shape, status: AiFrameStatus) -> Option<Shape> {
    match frame {
        Shape::AiImageFrame {
            common,
            prompt,
            reference_asset_ids,
            ..
        } => Some(Shape::AiImageFrame {
            common: common.clone(),
            prompt: prompt.clone(),
            reference_asset_ids: reference_asset_ids.clone(),
            status,
        }),
        _ => None,
    }
}

/// The annotation flow's precondition: the selection is exactly one
/// [`Shape::Image`] plus at least one other shape, every one of which
/// overlaps the image's bbox. Returns `(image, marks)` with the marks
/// z-sorted (id tie-break), i.e. in the order the composite draws them.
///
/// Non-overlapping extras fail the whole predicate rather than being
/// silently dropped: a user who selected a far-away note almost certainly
/// did not mean "annotate with everything except that one".
#[must_use]
pub(super) fn annotation_target(
    shapes: &[Shape],
    selection: &[String],
) -> Option<(Shape, Vec<Shape>)> {
    let selected: Vec<&Shape> = shapes
        .iter()
        .filter(|s| selection.iter().any(|id| id == s.id()))
        .collect();
    let mut image: Option<&Shape> = None;
    let mut marks: Vec<&Shape> = Vec::new();
    for shape in selected {
        if matches!(shape, Shape::Image { .. }) {
            if image.is_some() {
                return None; // two images: which one is "the original"?
            }
            image = Some(shape);
        } else {
            marks.push(shape);
        }
    }
    let image = image?;
    if marks.is_empty() {
        return None;
    }
    let image_bbox = Bbox::of_shape(image);
    if !marks
        .iter()
        .all(|m| Bbox::of_shape(m).intersects(&image_bbox))
    {
        return None;
    }
    let mut marks: Vec<Shape> = marks.into_iter().cloned().collect();
    marks.sort_by(|a, b| {
        a.common()
            .z
            .cmp(&b.common().z)
            .then_with(|| a.id().cmp(b.id()))
    });
    Some((image.clone(), marks))
}

/// Ops for inserting a fresh Draft AI image frame centered on `center` —
/// the panel's creation affordance (without it the whole flow is reachable
/// only through the model: capability wired ≠ capability delivered).
#[must_use]
pub(super) fn insert_frame_ops(
    doc: &CanvasDoc,
    center: (f64, f64),
    id: String,
) -> (Vec<CanvasOp>, Vec<CanvasOp>, String) {
    let top = doc.shapes.iter().map(|s| s.common().z.clone()).max();
    let (w, h) = AI_FRAME_SIZE;
    let shape = Shape::AiImageFrame {
        common: ShapeCommon {
            id: id.clone(),
            x: center.0 - w / 2.0,
            y: center.1 - h / 2.0,
            w,
            h,
            z: FracIndex::between(top.as_ref(), None),
            parent_id: None,
        },
        prompt: String::new(),
        reference_asset_ids: Vec::new(),
        status: AiFrameStatus::Draft,
    };
    let redo = vec![CanvasOp::UpsertShape { shape }];
    let undo = ops::invert(doc, &redo);
    (redo, undo, id)
}

// ---------------------------------------------------------------------------
// Message templates — model-facing English, exact tool names.
// ---------------------------------------------------------------------------

/// The structured chat message Generate sends. Names the `canvas` and
/// `image_generate` tools by their exact registry spellings, and spells the
/// three steps: read the frame, generate, place (which replaces the frame —
/// `insert_image` with a `frame_id` lands in the frame's box and deletes it,
/// `src/builtin_tools/canvas.rs::placement_of`).
#[must_use]
pub(super) fn generation_message(
    canvas_id: &str,
    frame_id: &str,
    prompt: &str,
    reference_count: usize,
) -> String {
    let refs_line = match reference_count {
        0 => String::new(),
        1 => "1 reference image is attached.\n".to_string(),
        n => format!("{n} reference images are attached.\n"),
    };
    format!(
        "[canvas] Generate an image for frame {frame_id} on canvas {canvas_id}.\n\
         Prompt: {prompt}\n\
         {refs_line}\
         Steps: \
         1) call `canvas` (action=\"get\", canvas_id=\"{canvas_id}\", detail=\"summary\") to read the frame. \
         2) generate the image with `image_generate`. \
         3) call `canvas` (action=\"insert_image\", canvas_id=\"{canvas_id}\", frame_id=\"{frame_id}\", \
         location=<the generated image file path or data: URL>) — this places the image in the \
         frame's box and replaces the frame."
    )
}

/// The annotation-regeneration message: two attachments (original, then the
/// annotated composite — also stored as a canvas asset so the model can
/// re-read it), and an explicit landing box at `x + w + 40` so the result
/// arrives beside the original instead of on top of it.
#[must_use]
pub(super) fn annotation_message(
    canvas_id: &str,
    image_id: &str,
    image_bbox: Bbox,
    annotated_asset_id: &str,
) -> String {
    let Bbox { x, y, w, h } = image_bbox;
    let target_x = x + w + ANNOTATION_GAP;
    format!(
        "[canvas] Regenerate the image {image_id} on canvas {canvas_id} following the user's \
         annotations.\n\
         Two images are attached: the original, then the original with the annotation marks drawn \
         on top (also stored as canvas asset {annotated_asset_id}). Produce a clean new image that \
         applies what the annotations ask for — the marks themselves must not appear in the \
         result.\n\
         Steps: \
         1) call `canvas` (action=\"get\", canvas_id=\"{canvas_id}\", detail=\"summary\") if you need \
         more context. \
         2) generate the new image with `image_generate`, using the attachments as reference. \
         3) call `canvas` (action=\"insert_image\", canvas_id=\"{canvas_id}\", x={target_x:.0}, \
         y={y:.0}, w={w:.0}, h={h:.0}, location=<the generated image>) to place it beside the \
         original."
    )
}

/// Decoded byte length of a base64 payload — `ChatAttachment.size` without
/// decoding (the payload goes on the wire as base64 anyway).
#[must_use]
fn base64_size_bytes(b64: &str) -> u64 {
    (b64.trim_end_matches('=').len() as u64) * 3 / 4
}

// ---------------------------------------------------------------------------
// Send plumbing shared by both flows.
// ---------------------------------------------------------------------------

/// Send one structured message into the current conversation, forwarding the
/// session's full context (agent binding, project room, model pick, dials) —
/// the voice-path shape, for the voice-path reason (module doc).
///
/// Every signal read is `try_*`: this runs past awaits, and the canvas (or
/// the whole editor) may have closed while an asset fetch was in flight.
async fn send_to_current_session(
    state: DashboardState,
    chat: ChatState,
    sessions: SessionMap,
    message: String,
    attachments: Vec<ChatAttachment>,
) -> Result<(), String> {
    let Some(sk) = chat.session_key.try_get_untracked() else {
        return Ok(()); // app tearing down: nothing to report, nowhere to report it
    };
    let Some(agent) = chat.agent_id.try_get_untracked() else {
        return Ok(());
    };
    let Some(room_project_id) = chat.room_project_id.try_get_untracked() else {
        return Ok(());
    };
    // A project-room send must not also carry a project_root — an explicit
    // root outranks the room's workspace binding (`ChatApi::send` doc).
    let project_root = if room_project_id.is_some() {
        None
    } else {
        chat.active_project_root.try_get_untracked().flatten()
    };
    let Some(model) = chat.selected_model.try_get_untracked() else {
        return Ok(());
    };
    let dials = session_dials_for_send(sk.is_some(), &chat.session_knobs());
    let resp = ChatApi::send(
        &state,
        &message,
        sk.as_deref(),
        attachments,
        agent.as_deref(),
        project_root.as_deref(),
        room_project_id.as_deref(),
        model.as_ref(),
        &dials,
        false,
    )
    .await?;
    // Route the run's stream frames to the conversation the send targeted,
    // and adopt a freshly minted session — both exactly what the voice path
    // does after its send.
    if let Some(conv) = sessions.active_conv() {
        sessions.bind_run(&resp.run_id, conv, Some(&resp.session_key));
    }
    chat.session_key.try_set(Some(resp.session_key));
    Ok(())
}

/// Re-upsert `frame_id` with `status` through the ops funnel — the failure
/// path's rollback (Pending → Failed when the send never went out).
fn set_frame_status(
    canvas: CanvasState,
    on_commit: Callback<(Vec<CanvasOp>, Vec<CanvasOp>)>,
    frame_id: &str,
    status: AiFrameStatus,
) {
    let pair = canvas
        .doc
        .try_with_untracked(|d| {
            d.as_ref().and_then(|d| {
                let current = d.shapes.iter().find(|s| s.id() == frame_id)?;
                let updated = frame_with_status(current, status)?;
                let redo = vec![CanvasOp::UpsertShape { shape: updated }];
                let undo = ops::invert(d, &redo);
                Some((redo, undo))
            })
        })
        .flatten();
    if let Some((redo, undo)) = pair {
        on_commit.run((redo, undo));
    }
}

// ---------------------------------------------------------------------------
// Components.
// ---------------------------------------------------------------------------

/// The AI image frame panel: an insert chip when no frame is selected, the
/// prompt/references/Generate panel when one is. Mounted by the editor in
/// its screen-space action cluster (top-right), which stops pointer
/// propagation the way the toolbar does.
#[component]
pub(super) fn AiFramePanel(
    /// The editor's optimistic commit funnel — same channel as every gesture.
    on_commit: Callback<(Vec<CanvasOp>, Vec<CanvasOp>)>,
    /// Inserts a fresh Draft frame at the viewport center (the editor owns
    /// the surface rect and camera needed to compute that point).
    insert_frame: Callback<()>,
) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();
    let i18n = use_i18n();

    let frame = Memo::new(move |_| {
        let sel = canvas.selection.get();
        canvas
            .doc
            .with(|d| d.as_ref().and_then(|d| selected_ai_frame(&d.shapes, &sel)))
    });
    let refs = Memo::new(move |_| {
        let sel = canvas.selection.get();
        canvas.doc.with(|d| {
            d.as_ref()
                .map(|d| selected_reference_images(&d.shapes, &sel))
                .unwrap_or_default()
        })
    });

    // Prompt draft, seeded from the frame's stored prompt once per frame
    // identity — re-seeding on every doc change would clobber typing with
    // each broadcast frame.
    let draft = RwSignal::new(String::new());
    let seeded_for: StoredValue<Option<String>> = StoredValue::new(None);
    Effect::new(move |_| {
        let Some(f) = frame.get() else {
            seeded_for.set_value(None);
            return;
        };
        let id = f.id().to_string();
        if seeded_for.with_value(|s| s.as_deref() == Some(id.as_str())) {
            return;
        }
        seeded_for.set_value(Some(id));
        if let Shape::AiImageFrame { prompt, .. } = &f {
            draft.set(prompt.clone());
        }
    });

    let busy = RwSignal::new(false);
    let generating = Memo::new(move |_| {
        matches!(
            frame.get(),
            Some(Shape::AiImageFrame {
                status: AiFrameStatus::Pending,
                ..
            })
        )
    });

    let on_generate = move |_| {
        if busy.get_untracked() {
            return;
        }
        let Some(fr) = frame.get_untracked() else {
            return;
        };
        let Some(cid) = canvas.open_canvas.get_untracked() else {
            return;
        };
        let prompt = draft.get_untracked().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let ref_ids: Vec<String> = refs
            .get_untracked()
            .into_iter()
            .map(|(_, asset_id)| asset_id)
            .collect();
        let Some(armed) = armed_frame(&fr, &prompt, &ref_ids) else {
            return;
        };
        let pair = canvas.doc.with_untracked(|d| {
            d.as_ref().map(|d| {
                let redo = vec![CanvasOp::UpsertShape {
                    shape: armed.clone(),
                }];
                let undo = ops::invert(d, &redo);
                (redo, undo)
            })
        });
        let Some((redo, undo)) = pair else {
            return;
        };
        busy.set(true);
        // Status flips to Pending on the wire before the chat send — every
        // client watching the broadcast sees the badge change immediately.
        on_commit.run((redo, undo));
        let frame_id = fr.id().to_string();
        let message = generation_message(&cid, &frame_id, &prompt, ref_ids.len());
        chat.push_user_message(&message);
        spawn_local(async move {
            let mut attachments = Vec::with_capacity(ref_ids.len());
            for asset_id in &ref_ids {
                match CanvasApi::asset_get(&state, &cid, asset_id).await {
                    Ok(asset) => attachments.push(ChatAttachment {
                        name: asset_id.clone(),
                        mime_type: asset.mime_type,
                        size: base64_size_bytes(&asset.data),
                        data_base64: asset.data,
                    }),
                    Err(e) => {
                        set_frame_status(canvas, on_commit, &frame_id, AiFrameStatus::Failed);
                        let _ = canvas.load_error.try_update(|v| {
                            *v = Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                                format!("Failed to load a reference image: {e}")
                            }));
                        });
                        let _ = busy.try_set(false);
                        return;
                    }
                }
            }
            if let Err(e) =
                send_to_current_session(state, chat, sessions, message, attachments).await
            {
                set_frame_status(canvas, on_commit, &frame_id, AiFrameStatus::Failed);
                let _ = canvas.load_error.try_update(|v| {
                    *v = Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to send the generation request: {e}")
                    }));
                });
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        {move || match frame.get() {
            None => view! {
                <button
                    class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border \
                           bg-surface-raised text-xs font-medium text-text-secondary \
                           hover:text-text-primary hover:border-primary/50 shadow-sm transition-colors"
                    on:click=move |_| insert_frame.run(())
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                        <rect x="3" y="3" width="18" height="18" rx="2" stroke-dasharray="4 3" />
                        <path d="M12 8v8M8 12h8" />
                    </svg>
                    {t!(i18n, canvas.ai_frame_insert)}
                </button>
            }
            .into_any(),
            Some(_) => view! {
                <div class="w-72 p-3 rounded-xl border border-border bg-surface-raised shadow-lg \
                            flex flex-col gap-2">
                    <div class="text-xs font-semibold text-text-primary">
                        {t!(i18n, canvas.ai_panel_title)}
                    </div>
                    <textarea
                        class="w-full h-20 px-2.5 py-2 rounded-lg bg-surface border border-border \
                               text-sm text-text-primary placeholder:text-text-tertiary resize-none \
                               focus:outline-none focus:border-primary"
                        placeholder=move || t_string!(i18n, canvas.ai_prompt_placeholder).to_string()
                        prop:value=move || draft.get()
                        on:input=move |ev| draft.set(event_target_value(&ev))
                    ></textarea>
                    <div class="text-xs text-text-tertiary">
                        {move || {
                            let n = refs.get().len();
                            if n == 0 {
                                view! { <span>{t!(i18n, canvas.ai_references_hint)}</span> }
                                    .into_any()
                            } else {
                                view! {
                                    <span>
                                        {n.to_string()} " " {t!(i18n, canvas.ai_references)}
                                    </span>
                                }
                                .into_any()
                            }
                        }}
                    </div>
                    <button
                        class="px-3 py-1.5 rounded-lg bg-primary hover:bg-primary-hover text-white \
                               text-xs font-medium transition-colors disabled:opacity-50"
                        prop:disabled=move || {
                            busy.get() || generating.get()
                                || draft.with(|d| d.trim().is_empty())
                        }
                        on:click=on_generate
                    >
                        {move || if generating.get() {
                            t!(i18n, canvas.ai_status_pending).into_any()
                        } else {
                            t!(i18n, canvas.ai_generate).into_any()
                        }}
                    </button>
                </div>
            }
            .into_any(),
        }}
    }
}

/// "Regenerate from annotations" — visible exactly when [`annotation_target`]
/// holds: one selected image plus overlapping marks.
#[component]
pub(super) fn AnnotateButton() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let canvas = expect_context::<CanvasState>();
    let chat = expect_context::<ChatState>();
    let sessions = expect_context::<SessionMap>();
    let i18n = use_i18n();

    let busy = RwSignal::new(false);
    let target = Memo::new(move |_| {
        let sel = canvas.selection.get();
        canvas
            .doc
            .with(|d| d.as_ref().and_then(|d| annotation_target(&d.shapes, &sel)))
    });

    let on_click = move |_| {
        if busy.get_untracked() {
            return;
        }
        let Some(cid) = canvas.open_canvas.get_untracked() else {
            return;
        };
        let Some((image, marks)) = target.get_untracked() else {
            return;
        };
        let Shape::Image {
            asset_id: image_asset_id,
            ..
        } = image.clone()
        else {
            return;
        };
        let all_shapes = canvas
            .doc
            .with_untracked(|d| d.as_ref().map(|d| d.shapes.clone()));
        let Some(all_shapes) = all_shapes else {
            return;
        };
        busy.set(true);
        spawn_local(async move {
            let report = |e: &str, frame: fn(&str) -> String| {
                let _ = canvas.load_error.try_update(|v| {
                    *v = Some(admin_refusal::settings_load_error(i18n, e, frame));
                });
                let _ = busy.try_set(false);
            };
            // 1. The original bytes — one fetch serves both the composite's
            //    embedded href and the first attachment.
            let original = match CanvasApi::asset_get(&state, &cid, &image_asset_id).await {
                Ok(asset) => asset,
                Err(e) => {
                    report(&e, |e| format!("Failed to load the original image: {e}"));
                    return;
                }
            };
            // 2. Composite: image first, marks in z-order on top, cropped to
            //    the image's bbox.
            let mut assets = std::collections::HashMap::new();
            assets.insert(
                image_asset_id.clone(),
                format!("data:{};base64,{}", original.mime_type, original.data),
            );
            let viewbox = Bbox::of_shape(&image);
            let mut layers = vec![image.clone()];
            layers.extend(marks.iter().cloned());
            let svg = export::export_svg(&layers, &all_shapes, &assets, viewbox, 0.0);
            let (pw, ph) = export::raster_dimensions(viewbox.w, viewbox.h);
            let png_data_url = match export::rasterize_svg_to_png(&svg, pw, ph).await {
                Ok(url) => url,
                Err(e) => {
                    report(&e, |e| format!("Failed to rasterize the annotation: {e}"));
                    return;
                }
            };
            let Some(png_b64) = asset_ingest::data_url_base64(&png_data_url) else {
                report("empty PNG", |e| {
                    format!("Failed to rasterize the annotation: {e}")
                });
                return;
            };
            // 3. Store the composite as a canvas asset (the message cites its
            //    id so the model can re-read it later).
            let annotated_id =
                match CanvasApi::asset_put(&state, &cid, "image/png", png_b64.to_string()).await {
                    Ok(id) => id,
                    Err(e) => {
                        let _ = canvas.load_error.try_update(|v| {
                            *v = Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                                format!("Failed to store the annotated image: {e}")
                            }));
                        });
                        let _ = busy.try_set(false);
                        return;
                    }
                };
            // 4. Send: original + annotated, and the x+w+40 landing box.
            let message = annotation_message(&cid, image.id(), viewbox, &annotated_id);
            chat.push_user_message(&message);
            let attachments = vec![
                ChatAttachment {
                    name: image_asset_id.clone(),
                    mime_type: original.mime_type.clone(),
                    size: base64_size_bytes(&original.data),
                    data_base64: original.data,
                },
                ChatAttachment {
                    name: format!("annotated-{annotated_id}"),
                    mime_type: "image/png".to_string(),
                    size: base64_size_bytes(png_b64),
                    data_base64: png_b64.to_string(),
                },
            ];
            if let Err(e) =
                send_to_current_session(state, chat, sessions, message, attachments).await
            {
                let _ = canvas.load_error.try_update(|v| {
                    *v = Some(admin_refusal::settings_write_error(i18n, &e, |e| {
                        format!("Failed to send the annotation request: {e}")
                    }));
                });
            }
            let _ = busy.try_set(false);
        });
    };

    view! {
        {move || {
            target.get().is_some().then(|| view! {
                <button
                    class="flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border \
                           bg-surface-raised text-xs font-medium text-text-secondary \
                           hover:text-text-primary hover:border-primary/50 shadow-sm \
                           transition-colors disabled:opacity-50"
                    prop:disabled=move || busy.get()
                    on:click=on_click
                >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                         stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M21 12a9 9 0 1 1-3-6.7" />
                        <polyline points="21 3 21 9 15 9" />
                    </svg>
                    {t!(i18n, canvas.annotate_regen)}
                </button>
            })
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn common(id: &str, x: f64, y: f64, w: f64, h: f64) -> ShapeCommon {
        ShapeCommon {
            id: id.to_string(),
            x,
            y,
            w,
            h,
            z: FracIndex::first(),
            parent_id: None,
        }
    }

    fn ai_frame(id: &str) -> Shape {
        Shape::AiImageFrame {
            common: common(id, 0.0, 0.0, 360.0, 240.0),
            prompt: "a red fox".to_string(),
            reference_asset_ids: Vec::new(),
            status: AiFrameStatus::Draft,
        }
    }

    fn image(id: &str, asset: &str, x: f64, y: f64, w: f64, h: f64) -> Shape {
        Shape::Image {
            common: common(id, x, y, w, h),
            asset_id: asset.to_string(),
            natural_w: 0.0,
            natural_h: 0.0,
        }
    }

    fn note(id: &str, x: f64, y: f64) -> Shape {
        Shape::Note {
            common: common(id, x, y, 50.0, 50.0),
            style: aleph_protocol::canvas::ShapeStyle::default(),
            text: String::new(),
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    /// The panel operates on exactly one frame — zero or two answer `None`.
    #[test]
    fn selected_ai_frame_requires_exactly_one_frame_in_the_selection() {
        let shapes = vec![ai_frame("f1"), ai_frame("f2"), note("n1", 0.0, 0.0)];
        assert!(selected_ai_frame(&shapes, &ids(&["n1"])).is_none());
        assert_eq!(
            selected_ai_frame(&shapes, &ids(&["f1", "n1"]))
                .expect("one frame selected")
                .id(),
            "f1"
        );
        assert!(
            selected_ai_frame(&shapes, &ids(&["f1", "f2"])).is_none(),
            "two selected frames must not guess which one gets the prompt"
        );
    }

    /// The reference pick is the selected Image shapes — nothing else, in
    /// document order.
    #[test]
    fn the_reference_pick_is_the_selected_image_shapes_alone() {
        let shapes = vec![
            image("i1", "aaa.png", 0.0, 0.0, 10.0, 10.0),
            note("n1", 0.0, 0.0),
            image("i2", "bbb.png", 20.0, 0.0, 10.0, 10.0),
            ai_frame("f1"),
        ];
        let refs = selected_reference_images(&shapes, &ids(&["i2", "f1", "i1", "n1"]));
        assert_eq!(
            refs,
            vec![
                ("i1".to_string(), "aaa.png".to_string()),
                ("i2".to_string(), "bbb.png".to_string()),
            ]
        );
        assert!(selected_reference_images(&shapes, &ids(&["f1", "n1"])).is_empty());
    }

    /// Generate arms the frame: prompt, references, and `Pending` — one op,
    /// so every watching client flips the badge together.
    #[test]
    fn armed_frame_sets_prompt_references_and_pending_status() {
        let armed = armed_frame(&ai_frame("f1"), "a blue whale", &["x.png".to_string()])
            .expect("an AI frame arms");
        let Shape::AiImageFrame {
            prompt,
            reference_asset_ids,
            status,
            ..
        } = armed
        else {
            panic!("arming must preserve the variant");
        };
        assert_eq!(prompt, "a blue whale");
        assert_eq!(reference_asset_ids, vec!["x.png".to_string()]);
        assert_eq!(status, AiFrameStatus::Pending);
        assert!(
            armed_frame(&note("n1", 0.0, 0.0), "p", &[]).is_none(),
            "only an AI frame can be armed"
        );
    }

    /// The failure path's status op keeps everything but the badge.
    #[test]
    fn frame_with_status_changes_only_the_status() {
        let updated =
            frame_with_status(&ai_frame("f1"), AiFrameStatus::Failed).expect("an AI frame updates");
        let Shape::AiImageFrame { prompt, status, .. } = updated else {
            panic!("variant preserved");
        };
        assert_eq!(prompt, "a red fox", "the prompt survives a status change");
        assert_eq!(status, AiFrameStatus::Failed);
    }

    /// One image + overlapping marks; two images, no marks, or a stray
    /// non-overlapping selection all refuse.
    #[test]
    fn annotation_target_requires_one_image_plus_overlapping_marks() {
        let img = image("i1", "orig.png", 0.0, 0.0, 100.0, 100.0);
        let on_it = note("m1", 40.0, 40.0);
        let far_away = note("m2", 500.0, 500.0);
        let second = image("i2", "other.png", 300.0, 0.0, 50.0, 50.0);
        let shapes = vec![img, on_it, far_away, second];

        let (image_out, marks) = annotation_target(&shapes, &ids(&["i1", "m1"]))
            .expect("one image plus an overlapping mark");
        assert_eq!(image_out.id(), "i1");
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].id(), "m1");

        assert!(
            annotation_target(&shapes, &ids(&["i1"])).is_none(),
            "an image with no marks has nothing to regenerate from"
        );
        assert!(
            annotation_target(&shapes, &ids(&["i1", "m2"])).is_none(),
            "a mark that does not touch the image fails the whole predicate"
        );
        assert!(
            annotation_target(&shapes, &ids(&["i1", "i2", "m1"])).is_none(),
            "two selected images must not guess which is the original"
        );
    }

    /// The insert affordance mints a Draft frame centered on the given point,
    /// with an inverse that removes it.
    #[test]
    fn insert_frame_ops_creates_a_centered_draft_frame_with_a_deleting_undo() {
        let doc = CanvasDoc {
            id: "cv-1".to_string(),
            title: "t".to_string(),
            owner_user_id: None,
            project_id: None,
            revision: 1,
            shapes: vec![note("n1", 0.0, 0.0)],
            decks: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let (redo, undo, id) = insert_frame_ops(&doc, (500.0, 300.0), "fid".to_string());
        assert_eq!(id, "fid");
        let [CanvasOp::UpsertShape { shape }] = redo.as_slice() else {
            panic!("one upsert expected");
        };
        let Shape::AiImageFrame { common, status, .. } = shape else {
            panic!("an AI frame is inserted");
        };
        assert_eq!(*status, AiFrameStatus::Draft);
        assert!((common.x + common.w / 2.0 - 500.0).abs() < 1e-9);
        assert!((common.y + common.h / 2.0 - 300.0).abs() < 1e-9);
        assert_eq!(
            undo,
            vec![CanvasOp::DeleteShape {
                id: "fid".to_string()
            }],
            "undoing a fresh insert is a delete"
        );
    }

    /// The generation message names the two tools by their exact registry
    /// spellings and spells the three steps — this template is wire contract
    /// (module doc), and the tool names in it are a second copy of the real
    /// names that only these assertions keep honest Panel-side.
    #[test]
    fn generation_message_names_the_real_tools_and_the_three_steps() {
        let msg = generation_message("cv-9", "frame-1", "a red fox", 2);
        assert!(msg.contains("`canvas`"), "{msg}");
        assert!(msg.contains("`image_generate`"), "{msg}");
        assert!(
            msg.contains("1)") && msg.contains("2)") && msg.contains("3)"),
            "{msg}"
        );
        assert!(msg.contains("action=\"get\""), "{msg}");
        assert!(msg.contains("detail=\"summary\""), "{msg}");
        assert!(msg.contains("action=\"insert_image\""), "{msg}");
        assert!(msg.contains("frame_id=\"frame-1\""), "{msg}");
        assert!(msg.contains("canvas_id=\"cv-9\""), "{msg}");
        assert!(msg.contains("Prompt: a red fox"), "{msg}");
        assert!(msg.contains("2 reference images are attached."), "{msg}");
        assert!(
            !generation_message("cv-9", "frame-1", "p", 0).contains("attached"),
            "no reference line when nothing is attached"
        );
    }

    /// The annotation message places the result at x + w + 40, beside the
    /// original — the numbers are computed here because the model cannot
    /// know the bbox.
    #[test]
    fn annotation_message_places_the_result_to_the_right_of_the_original() {
        let bbox = Bbox {
            x: 10.0,
            y: 5.0,
            w: 20.0,
            h: 30.0,
        };
        let msg = annotation_message("cv-9", "img-1", bbox, "anno.png");
        assert!(msg.contains("x=70"), "10 + 20 + 40 = 70: {msg}");
        assert!(
            msg.contains("y=5") && msg.contains("w=20") && msg.contains("h=30"),
            "{msg}"
        );
        assert!(
            msg.contains("`canvas`") && msg.contains("`image_generate`"),
            "{msg}"
        );
        assert!(
            msg.contains("anno.png"),
            "the stored composite is cited: {msg}"
        );
        assert!(
            msg.contains("1)") && msg.contains("2)") && msg.contains("3)"),
            "{msg}"
        );
    }

    /// Attachment sizes are the decoded byte counts.
    #[test]
    fn base64_size_reports_decoded_bytes() {
        // "AAAA" = 3 bytes, "AAA=" = 2, "AA==" = 1.
        assert_eq!(base64_size_bytes("AAAA"), 3);
        assert_eq!(base64_size_bytes("AAA="), 2);
        assert_eq!(base64_size_bytes("AA=="), 1);
        assert_eq!(base64_size_bytes(""), 0);
    }
}
