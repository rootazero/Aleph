//! Project room Memory tab (P3, spec §6.4) — the notes this room has written.
//!
//! ## Which partition, and why this one is safe to name
//! A room's memory lives in the composed partition `{agent}__{project_id}`
//! (`memory::project_scope::scoped_agent_id`). `memory.listFacts` resolves a
//! caller-supplied id through `memory_scope::read_partitions`, which passes an
//! ALREADY-COMPOSED id through verbatim — recomposing it would produce a ghost
//! partition, and rewriting it to the caller's own would turn a refusal into a
//! silent substitution. The visibility gate runs on the id the caller actually
//! sent, before composition, so naming a room the caller is not in is refused
//! there rather than here.
//!
//! ## Why there is no curated (MEMORY.md) section
//! `memory.curated.list` deliberately REFUSES a composed id, and its refusal
//! is shaped as an empty store — byte-identical to a room that has written
//! nothing. So a curated panel here could not tell "this room has no hot-tier
//! memory" from "you may not read it", and would render the same confident
//! empty list either way. The hot tier is reachable from inside a room turn,
//! where the server composes the scope itself. Showing nothing is honest;
//! showing an empty list would not be.
//!
//! ## The agent
//! Resolved by [`super::room_agent_id`] — the same rule `RoomChat` uses to
//! open the room's session, not a second one. A room whose chat runs under
//! one agent while this tab reads another would show memory nobody in the
//! room wrote.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::api::memory::{CompressedFact, MemoryApi};
use crate::api::projects::ProjectInfo;
use crate::context::DashboardState;
use crate::i18n::{t, use_i18n, Locale};
use leptos_i18n::I18nContext;

/// How many notes the tab lists. A room browse, not an audit surface — the
/// memory view proper is where you page through everything.
const ROOM_NOTE_LIMIT: usize = 100;

/// The partition id for a room: the base agent composed with the project id.
///
/// Project ids already carry the `p-` family prefix the composition keys on,
/// so this is a join and not a re-spelling. Kept as a named function with a
/// test rather than an inline `format!` because a wrong partition id here
/// does not fail — it silently reads an empty one.
#[must_use]
pub(crate) fn room_partition(agent_id: &str, project_id: &str) -> String {
    format!("{agent_id}__{project_id}")
}

fn load_failure(i18n: I18nContext<Locale>, err: &str) -> String {
    crate::components::admin_refusal::settings_load_error(i18n, err, |e| e.to_string())
}

#[component]
#[must_use]
pub fn MemoryTab(project: ProjectInfo) -> impl IntoView {
    let i18n = use_i18n();
    let dash = expect_context::<DashboardState>();
    // Snapshot the context handle BEFORE the async block: reading context
    // after a suspension point is the disposed-read shape the crate guard
    // rejects, and `ChatState` is Copy so carrying it costs nothing.
    let chat = use_context::<crate::platform::wide::views::chat::state::ChatState>();

    let notes: RwSignal<Vec<CompressedFact>> = RwSignal::new(Vec::new());
    let total: RwSignal<Option<u64>> = RwSignal::new(None);
    let error: RwSignal<Option<String>> = RwSignal::new(None);
    // `None` until an answer arrives, so an unloaded tab never renders the
    // empty-state copy — "this room has written nothing" is a claim, and it
    // must not be made before anyone has looked.
    let loaded = RwSignal::new(false);

    let project_id = StoredValue::new(project.id.clone());

    Effect::new(move |_| {
        if !dash.is_connected.get() {
            return;
        }
        let pid = project_id.get_value();
        spawn_local(async move {
            let agent = super::room_agent_id(&dash, chat).await;
            let partition = room_partition(&agent, &pid);
            match MemoryApi::list_facts(&dash, &partition, ROOM_NOTE_LIMIT, 0).await {
                Ok((rows, count)) => {
                    notes.set(rows);
                    total.set(count);
                    error.set(None);
                }
                Err(e) => error.set(Some(load_failure(i18n, &e))),
            }
            loaded.set(true);
        });
    });

    view! {
        <div class="flex-1 overflow-auto px-6 py-4 space-y-3">
            <Show when=move || error.get().is_some()>
                <div class="rounded-md bg-danger-subtle px-3 py-2 text-xs text-danger">
                    {move || error.get().unwrap_or_default()}
                </div>
            </Show>

            <Show when=move || total.get().is_some()>
                <p class="text-xs text-text-tertiary">
                    {move || total.get().map(|t| t.to_string()).unwrap_or_default()}
                    " "
                    {t!(i18n, project_room.memory_note_count_suffix)}
                </p>
            </Show>

            <Show
                when=move || !notes.get().is_empty()
                fallback=move || view! {
                    <Show when=move || loaded.get() && error.get().is_none()>
                        <p class="text-sm text-text-tertiary">
                            {t!(i18n, project_room.memory_empty)}
                        </p>
                    </Show>
                }
            >
                <ul class="space-y-1">
                    <For each=move || notes.get() key=|n: &CompressedFact| n.id.clone() let:note>
                        <li class="rounded-md bg-surface-raised px-3 py-2">
                            <div class="truncate text-sm">{note.content.clone()}</div>
                            <div class="mt-0.5 flex flex-wrap gap-1.5 text-[11px] text-text-tertiary">
                                <span>{note.category.clone()}</span>
                                {note.tags.iter().map(|tag| {
                                    view! { <span class="rounded-full bg-surface-sunken px-1.5">{tag.clone()}</span> }
                                }).collect_view()}
                            </div>
                        </li>
                    </For>
                </ul>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project id already carries the `p-` family prefix the partition
    /// composition keys on, so this is a join — not a second spelling of the
    /// scope rule. A wrong id here reads an empty partition rather than
    /// failing, which is why it is worth pinning.
    #[test]
    fn a_room_partition_is_the_agent_joined_to_the_project_id() {
        assert_eq!(room_partition("main", "p-x7f2"), "main__p-x7f2");
    }

    /// The separator is exactly two underscores; a single one would name a
    /// different (and almost certainly absent) partition.
    #[test]
    fn the_separator_is_the_composed_id_separator() {
        let composed = room_partition("researcher", "p-a");
        assert!(composed.starts_with("researcher__"));
        assert_eq!(composed.matches("__").count(), 1);
    }
}
