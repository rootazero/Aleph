//! Room settings → which channel conversations this room lives in.
//!
//! Its own file rather than a fifth section inside `settings.rs`: that file
//! already carries the tab body, four sections and its own test module, and
//! folding this in would cross the 200-400 band in the same edit that touches
//! it. Deliberately no line count here — the one that used to be quoted drifted
//! twice, and a rationale that names no number cannot go stale.
//!
//! ## Why this section does not hide its controls behind `is_owner`
//!
//! `settings.rs`'s workspace and archive sections render read-only for a
//! non-owner, softening rather than removing the control (its `RosterSection`
//! has two sites that still vanish — that file's module doc names them as the
//! unfinished half). This one must not gate at all, for two separate reasons:
//!
//! 1. `projects.channel.bind` / `.unbind` are **admin**-gated
//!    (`method_admin.rs`), not owner-gated. A room's owner who is not an org
//!    admin fails; an org admin who is not the owner passes. `is_owner` is
//!    therefore wrong in both directions here, and a hidden control that the
//!    server would have accepted is a feature the user never learns exists.
//! 2. The Panel deliberately holds no client-side role predicate at all —
//!    `DashboardState::is_operator()` was deleted on 2026-08-07 because a role
//!    captured at `connect` is stale in both directions once
//!    `handlers::users::restamp_live_connections` re-stamps a live connection.
//!    `admin_refusal`'s own doc says it plainly: do not use the refusal to
//!    pre-emptively hide a surface, because that is the same gate under a new
//!    name.
//!
//! So: render, call, and report what the server said. **Both** directions go
//! through `admin_refusal` — classifying only the read half is what left ~20
//! settings pages explaining their load failure politely and then answering a
//! Save with the raw protocol string, telling the user their action failed
//! rather than that they lack permission.
//!
//! ## 频道绑定区（中文）
//!
//! 绑定 / 解绑是 **admin** 闸（不是房主闸），所以这一区刻意不按 `is_owner`
//! 隐藏控件——房主未必是管理员，管理员未必是房主，两个方向都会判错。读写两侧
//! 的拒绝都经 `admin_refusal` 分类，只接读那一半会让同一个判决对用户讲两个
//! 故事。

use aleph_protocol::projects::{
    BindingPeerKind, ChannelBindResult, ChannelBindingRow, ChannelUnbindResult, RescopeOutcome,
    UNBIND_KEEPS_TRANSCRIPT_NOTICE,
};
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::projects::ProjectsApi;
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n, I18nCtx};

/// Classify a failed WRITE, once, for both buttons.
///
/// A local wrapper rather than `admin_refusal::settings_write_error` inline at
/// each site, and the reason is mechanical: the crate guard
/// `no_error_signal_is_fed_an_unclassified_error` finds writes by locating
/// `.set(Some(` **on a single line**, and inside a `view!` macro this deep,
/// rustfmt splits the receiver, `.set(`, `Some(` and the classifier call across
/// six lines. The guard then never sees the site at all — so the file sits in
/// its blind spot whether or not the code is correct, and the next author
/// editing this section gets no protection. Measured: feeding the raw `e` here
/// left the guard green.
///
/// Collapsing the call to one line restores coverage, and routing it through a
/// local function is the shape the guard already recognises
/// (`calls_a_local_classifier`, written for `cluster.rs::fleet_error_label`) —
/// so this is coverage regained without an exemption.
///
/// 本地分类器包装：crate 级守卫按**单行** `.set(Some(` 定位写入点，而在
/// `view!` 深层缩进里 rustfmt 会把它拆成六行，于是守卫看不见这个文件——不管
/// 代码对不对。收成一行并走本地分类器，是守卫本来就认得的形状。
fn write_failure(i18n: I18nCtx, err: &str) -> String {
    admin_refusal::settings_write_error(i18n, err, str::to_string)
}

/// The localized label for one peer kind.
///
/// Exhaustive on purpose: a third variant added to the wire enum is a compile
/// error here, which is how the picker cannot silently stop offering one.
fn kind_label(i18n: I18nCtx, kind: BindingPeerKind) -> String {
    match kind {
        BindingPeerKind::Group => t_string!(i18n, project_room.channel_kind_group).to_string(),
        BindingPeerKind::Thread => t_string!(i18n, project_room.channel_kind_thread).to_string(),
    }
}

/// What a bind did to the conversation's existing transcript.
///
/// Three sentences, and the third is not a rewording of the second.
/// [`RescopeOutcome::NothingToMove`] is "the store answered, and it found no
/// session row"; [`RescopeOutcome::Unknown`] is "the store did not answer".
/// Rendering the second as the first is the Panel inventing a result the
/// server never gave it — the same class as reading a refused call as an empty
/// answer, which `admin_refusal` exists to stop.
///
/// The wording also avoids "nobody has spoken in that conversation yet". That
/// is an *interpretation* of "no row found", true today and false the moment
/// somebody narrows the search — and an interpretation is a claim this receipt
/// cannot support.
fn rescope_sentence(i18n: I18nCtx, outcome: RescopeOutcome) -> String {
    match outcome {
        RescopeOutcome::Moved => t_string!(i18n, project_room.channel_moved).to_string(),
        RescopeOutcome::NothingToMove => {
            t_string!(i18n, project_room.channel_nothing_to_move).to_string()
        }
        RescopeOutcome::Unknown => {
            t_string!(i18n, project_room.channel_unknown_rescope).to_string()
        }
    }
}

/// The lines shown after a successful bind.
fn bind_receipt(i18n: I18nCtx, result: &ChannelBindResult) -> Vec<String> {
    vec![
        format!(
            "{} {}:{}",
            t_string!(i18n, project_room.channel_bound),
            result.binding.channel_id,
            result.binding.peer_id
        ),
        rescope_sentence(i18n, result.rescoped_session),
    ]
}

/// The lines shown after an unbind.
///
/// The second line is [`UNBIND_KEEPS_TRANSCRIPT_NOTICE`], **imported, not
/// typed**. It has to be byte-identical here, in `aleph projects channel
/// unbind`, and in the server-side doc; three copies of one sentence is three
/// authors, and this repo's most-recorded defect is one fact with two
/// statements where only one gets updated. Precedent: `ADMIN_REQUIRED_MESSAGE`,
/// which this module's neighbour matches on for the same reason.
///
/// It is therefore also the one string here that is **not** localized. That is
/// a deliberate trade, not an oversight: a translated copy is a second author
/// by construction, and the sentence's job is to correct an assumption an
/// operator makes about what `unbind` did — being wrong about that costs more
/// than being English.
///
/// It appears only when something was actually released. On `unbound: false`
/// there was nothing bound, so there is no transcript decision to explain and
/// saying it anyway would describe an event that did not happen.
fn unbind_receipt(i18n: I18nCtx, result: &ChannelUnbindResult) -> Vec<String> {
    if result.unbound {
        vec![
            t_string!(i18n, project_room.channel_released).to_string(),
            UNBIND_KEEPS_TRANSCRIPT_NOTICE.to_string(),
        ]
    } else {
        vec![t_string!(i18n, project_room.channel_unbind_noop).to_string()]
    }
}

#[component]
#[must_use]
pub fn ChannelBindingsSection(project_id: String, dash: DashboardState) -> impl IntoView {
    let i18n = use_i18n();
    let id = StoredValue::new(project_id);

    let bindings: RwSignal<Vec<ChannelBindingRow>> = RwSignal::new(Vec::new());
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    let receipt: RwSignal<Vec<String>> = RwSignal::new(Vec::new());

    let channel_id = RwSignal::new(String::new());
    let peer_id = RwSignal::new(String::new());
    let label = RwSignal::new(String::new());
    let kind = RwSignal::new(BindingPeerKind::Group);

    // The READ path is classified too. A member may open this page, and
    // `projects.channel.list` is open to members — but a transport failure and
    // a refusal are still two different things, and the frame is the caller's
    // because only the caller knows what was being attempted.
    let refresh: Callback<()> = Callback::new(move |()| {
        let project = id.get_value();
        spawn_local(async move {
            match ProjectsApi::channel_list(&dash, &project).await {
                Ok(rows) => {
                    error.set(None);
                    bindings.set(rows);
                }
                Err(e) => error.set(Some(admin_refusal::settings_load_error(i18n, &e, |e| {
                    format!("{}: {e}", t_string!(i18n, project_room.channel_bindings))
                }))),
            }
        });
    });
    refresh.run(());

    view! {
        <section>
            <h3 class="text-xs font-medium text-text-tertiary uppercase tracking-wider mb-2">
                {t!(i18n, project_room.channel_bindings)}
            </h3>

            <Show when=move || error.get().is_some()>
                <div class="mb-2 px-3 py-2 rounded-md bg-danger/10 text-danger text-sm">
                    {move || error.get().unwrap_or_default()}
                </div>
            </Show>

            <Show when=move || !receipt.get().is_empty()>
                <div class="mb-2 px-3 py-2 rounded-md bg-surface-sunken text-text-secondary text-sm space-y-1">
                    {move || {
                        receipt
                            .get()
                            .into_iter()
                            .map(|line| view! { <p>{line}</p> })
                            .collect_view()
                    }}
                </div>
            </Show>

            {move || {
                let rows = bindings.get();
                if rows.is_empty() {
                    view! {
                        <p class="text-sm text-text-tertiary">
                            {t!(i18n, project_room.channel_bindings_empty)}
                        </p>
                    }
                        .into_any()
                } else {
                    view! {
                        <ul class="space-y-1">
                            {rows
                                .into_iter()
                                .map(|row| {
                                    let row_channel = StoredValue::new(row.channel_id.clone());
                                    let row_peer = StoredValue::new(row.peer_id.clone());
                                    let row_kind = row.peer_kind;
                                    // The label is the operator's own spelling
                                    // of the conversation; the key components
                                    // are normalized before storage, so this is
                                    // the only place their original wording
                                    // survives. Falling back to the peer id
                                    // rather than to an empty cell keeps the
                                    // row addressable.
                                    let shown = row
                                        .label
                                        .clone()
                                        .unwrap_or_else(|| row.peer_id.clone());
                                    // `StoredValue` (Copy) and a plain `bool`,
                                    // not the `Option<String>`: `Show`'s
                                    // `when` and its children are two closures
                                    // that must both be re-callable (`Fn`), and
                                    // a moved-out `String` makes the ancestor
                                    // `FnOnce`. Same reason `settings.rs`
                                    // documents for every id it threads into a
                                    // handler.
                                    let has_bound_by = row.bound_by.is_some();
                                    let bound_by =
                                        StoredValue::new(row.bound_by.clone().unwrap_or_default());
                                    view! {
                                        <li class="flex items-center justify-between px-3 py-1.5 rounded-md bg-surface-sunken">
                                            <span class="min-w-0 flex-1">
                                                <span class="text-sm text-text-primary">{shown}</span>
                                                <span class="ml-2 text-[10px] uppercase tracking-wide text-text-tertiary">
                                                    {row.channel_id.clone()}
                                                    " · "
                                                    {kind_label(i18n, row_kind)}
                                                </span>
                                                <Show when=move || has_bound_by>
                                                    <span class="ml-2 text-[10px] text-text-tertiary">
                                                        {t!(i18n, project_room.channel_bound_by)}
                                                        " "
                                                        {move || bound_by.get_value()}
                                                    </span>
                                                </Show>
                                            </span>
                                            <button
                                                type="button"
                                                class="text-text-tertiary hover:text-danger text-xs px-1"
                                                on:click=move |_| {
                                                    let channel = row_channel.get_value();
                                                    let peer = row_peer.get_value();
                                                    spawn_local(async move {
                                                        match ProjectsApi::channel_unbind(
                                                                &dash,
                                                                &channel,
                                                                row_kind,
                                                                &peer,
                                                            )
                                                            .await
                                                        {
                                                            Ok(result) => {
                                                                error.set(None);
                                                                receipt.set(unbind_receipt(i18n, &result));
                                                                refresh.run(());
                                                            }
                                                            Err(e) => {
                                                                receipt.set(Vec::new());
                                                                error.set(Some(write_failure(i18n, &e)));
                                                            }
                                                        }
                                                    });
                                                }
                                            >
                                                {t!(i18n, project_room.unbind)}
                                            </button>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                        .into_any()
                }
            }}

            <div class="mt-3 flex flex-wrap items-center gap-2">
                <input
                    type="text"
                    class="flex-1 min-w-0 px-2 py-1.5 rounded-md bg-surface-sunken border border-border text-sm text-text-primary focus:outline-none focus:border-primary/60"
                    placeholder=move || t_string!(i18n, project_room.channel_id_placeholder).to_string()
                    prop:value=move || channel_id.get()
                    on:input=move |ev| channel_id.set(event_target_value(&ev))
                />
                <input
                    type="text"
                    class="flex-1 min-w-0 px-2 py-1.5 rounded-md bg-surface-sunken border border-border text-sm text-text-primary focus:outline-none focus:border-primary/60"
                    placeholder=move || t_string!(i18n, project_room.channel_peer_placeholder).to_string()
                    prop:value=move || peer_id.get()
                    on:input=move |ev| peer_id.set(event_target_value(&ev))
                />
                <select
                    class="px-2 py-1.5 rounded-md bg-surface-sunken border border-border text-sm text-text-primary"
                    on:change=move |ev| {
                        // The option VALUE is the wire spelling and the option
                        // TEXT is the localized label. Sending back what the
                        // user saw instead of the key is how a decorated label
                        // stops matching anything server-side.
                        //
                        // A value that does not parse leaves the signal alone
                        // rather than defaulting: the option list is built from
                        // `ALL`, so there is no such value, and picking one
                        // would mean binding a kind nobody chose.
                        if let Ok(parsed) = event_target_value(&ev).parse::<BindingPeerKind>() {
                            kind.set(parsed);
                        }
                    }
                >
                    {BindingPeerKind::ALL
                        .into_iter()
                        .map(|k| {
                            view! {
                                <option value=k.as_str()>{kind_label(i18n, k)}</option>
                            }
                        })
                        .collect_view()}
                </select>
                <input
                    type="text"
                    class="flex-1 min-w-0 px-2 py-1.5 rounded-md bg-surface-sunken border border-border text-sm text-text-primary focus:outline-none focus:border-primary/60"
                    placeholder=move || t_string!(i18n, project_room.channel_label_placeholder).to_string()
                    prop:value=move || label.get()
                    on:input=move |ev| label.set(event_target_value(&ev))
                />
                <button
                    type="button"
                    class="px-3 py-1.5 rounded-md text-sm bg-primary/15 text-primary hover:bg-primary/25"
                    on:click=move |_| {
                        let project = id.get_value();
                        let channel = channel_id.get_untracked().trim().to_string();
                        let peer = peer_id.get_untracked().trim().to_string();
                        let text = label.get_untracked().trim().to_string();
                        let chosen = kind.get_untracked();
                        if channel.is_empty() || peer.is_empty() {
                            return;
                        }
                        spawn_local(async move {
                            let named = if text.is_empty() { None } else { Some(text.as_str()) };
                            match ProjectsApi::channel_bind(
                                    &dash,
                                    &project,
                                    &channel,
                                    chosen,
                                    &peer,
                                    named,
                                )
                                .await
                            {
                                Ok(result) => {
                                    error.set(None);
                                    receipt.set(bind_receipt(i18n, &result));
                                    channel_id.set(String::new());
                                    peer_id.set(String::new());
                                    label.set(String::new());
                                    refresh.run(());
                                }
                                Err(e) => {
                                    receipt.set(Vec::new());
                                    error.set(Some(write_failure(i18n, &e)));
                                }
                            }
                        });
                    }
                >
                    {t!(i18n, project_room.channel_bind)}
                </button>
            </div>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EN: &str = include_str!("../../../locales/en.json");
    const ZH: &str = include_str!("../../../locales/zh.json");

    fn room_copy(src: &str) -> serde_json::Value {
        let all: serde_json::Value = serde_json::from_str(src).expect("locale file is JSON");
        all["project_room"].clone()
    }

    fn sentence(src: &str, key: &str) -> String {
        room_copy(src)[key]
            .as_str()
            .unwrap_or_else(|| panic!("locale file is missing project_room.{key}"))
            .to_string()
    }

    /// The three rescope outcomes must read as three different answers, in
    /// every language.
    ///
    /// Asserted against the locale files rather than by reading
    /// [`rescope_sentence`], because the failure this guards is two arms
    /// pointing at keys whose *values* happen to be the same sentence — which
    /// compiles, renders, and looks deliberate. `Unknown` collapsing into
    /// `NothingToMove` is the specific one: the server did not answer, and
    /// telling the user it found nothing is a fact the Panel never received.
    #[test]
    fn the_three_rescope_outcomes_read_as_three_different_answers() {
        for (lang, src) in [("en", EN), ("zh", ZH)] {
            let mut seen = std::collections::BTreeSet::new();
            for key in [
                "channel_moved",
                "channel_nothing_to_move",
                "channel_unknown_rescope",
            ] {
                let text = sentence(src, key);
                assert!(
                    !text.trim().is_empty(),
                    "{lang}: project_room.{key} is empty"
                );
                assert!(
                    seen.insert(text.clone()),
                    "{lang}: project_room.{key} reuses another outcome's sentence \
                     ({text:?}) — the Panel would be reporting an answer the server \
                     did not give"
                );
            }
        }
    }

    /// `Unknown` must not claim a search happened.
    ///
    /// The English copy is checked by phrase; the Chinese by the fact that it
    /// is not the other sentence, which the test above already establishes.
    /// Phrase-matching a translation would be a rule about one wording rather
    /// than about the claim.
    #[test]
    fn the_unknown_outcome_does_not_claim_the_store_answered() {
        let unknown = sentence(EN, "channel_unknown_rescope").to_lowercase();
        for forbidden in [
            "no session was found",
            "nothing was moved",
            "nobody has spoken",
        ] {
            assert!(
                !unknown.contains(forbidden),
                "the Unknown sentence says {forbidden:?}, which asserts the store \
                 answered — it did not: {unknown:?}"
            );
        }
        let nothing = sentence(EN, "channel_nothing_to_move").to_lowercase();
        assert!(
            !nothing.contains("nobody has spoken"),
            "the server reported \"no row found\"; \"nobody has spoken\" is an \
             inference layered on top of it: {nothing:?}"
        );
    }

    /// Every key this section reads exists in BOTH locale files.
    ///
    /// `leptos_i18n` resolves `t!` at compile time, so a key missing from the
    /// *primary* locale is a build error — but a key present in `en` and
    /// absent from `zh` is not necessarily caught here by construction, and
    /// this section adds thirteen at once.
    #[test]
    fn every_key_this_section_reads_is_translated_in_both_languages() {
        for key in [
            "channel_bindings",
            "channel_bindings_empty",
            "channel_bind",
            "channel_bound",
            "channel_bound_by",
            "channel_released",
            "channel_unbind_noop",
            "channel_moved",
            "channel_nothing_to_move",
            "channel_unknown_rescope",
            "channel_kind_group",
            "channel_kind_thread",
            "channel_id_placeholder",
            "channel_peer_placeholder",
            "channel_label_placeholder",
            "unbind",
        ] {
            for (lang, src) in [("en", EN), ("zh", ZH)] {
                assert!(
                    room_copy(src)[key].is_string(),
                    "{lang}: project_room.{key} is missing"
                );
            }
        }
    }

    /// The unbind notice is the shared constant, and it still says the thing
    /// it exists to say.
    ///
    /// Byte equality is the whole assertion: this sentence has to be identical
    /// on the Panel, in `aleph projects channel unbind`, and in the
    /// server-side doc.
    #[test]
    fn the_unbind_notice_is_the_shared_constant_and_not_a_locale_key() {
        assert!(
            !room_copy(EN)
                .as_object()
                .expect("project_room is an object")
                .keys()
                .any(|k| k.contains("keeps_transcript")),
            "the notice must not gain a locale key: a translated copy is a second \
             author of a sentence that must be byte-identical on three surfaces"
        );
        assert!(
            UNBIND_KEEPS_TRANSCRIPT_NOTICE.contains("does not move"),
            "the notice must still say the history does NOT come back — that is the \
             assumption it exists to correct"
        );
    }
}
