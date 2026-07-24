//! @-mention autocomplete for team chat. Typing '@' opens a roster picker
//! (avatar + name + dim id); selecting inserts the canonical `@<id>` token —
//! the backend (`teams/messages/mentions.rs`) resolves on agent_id, and names
//! are not unique, so the inserted token is always the id.

use super::agent_identity::agent_color_for_id;
use super::state::TeamMemberView;
use leptos::prelude::*;

/// Match a roster candidate against the text typed after '@'. Matches name OR
/// id, case-insensitive. Empty query matches everything.
#[must_use]
pub fn mention_matches(query: &str, name: &str, id: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    name.to_lowercase().contains(&q) || id.to_lowercase().contains(&q)
}

/// Given textarea text and the caret byte offset, return the active '@' query
/// (text from the '@' up to caret) if the caret is inside a mention token, else
/// None. A token is `@` preceded by start-or-whitespace, followed by
/// `[A-Za-z0-9_-]*` with no whitespace before the caret.
#[must_use]
pub fn active_mention_query(text: &str, caret: usize) -> Option<String> {
    let head = text.get(..caret)?;
    let at = head.rfind('@')?;
    if at > 0 {
        let prev = head[..at].chars().next_back()?;
        if !prev.is_whitespace() {
            return None;
        }
    }
    let token = &head[at + 1..];
    if token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Some(token.to_string())
    } else {
        None
    }
}

/// Given textarea text and the caret byte offset, return the byte offset of
/// the `@` that starts the active mention token, or `None` if not in a mention.
#[must_use]
pub fn mention_at_offset(text: &str, caret: usize) -> Option<usize> {
    let head = text.get(..caret)?;
    let at = head.rfind('@')?;
    if at > 0 {
        let prev = head[..at].chars().next_back()?;
        if !prev.is_whitespace() {
            return None;
        }
    }
    let token = &head[at + 1..];
    if token
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Some(at)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Leptos component (WASM only — the pure functions above are host-testable)
// ---------------------------------------------------------------------------

/// @-mention palette popup for team chat. Mirrors SlashPaletteView's structure:
/// signals drive visibility + selection, mousedown fires the callback keeping
/// textarea focus, and selection writes back via the same `input_text` signal.
#[component]
pub fn MentionPaletteView(
    /// Show/hide the palette.
    show: RwSignal<bool>,
    /// Filtered roster entries (already passed through `mention_matches`).
    members: RwSignal<Vec<TeamMemberView>>,
    /// Currently highlighted row index.
    selected_index: RwSignal<usize>,
    /// Callback fired when the user picks a row. Payload: the token to insert
    /// (e.g. `"@risk_analyst "` or `"@all "`).
    on_select: Callback<String>,
) -> impl IntoView {
    // Follow ↑/↓ selection: scroll the highlighted row into view when it
    // leaves the capped viewport.
    let list_ref = NodeRef::<leptos::html::Div>::new();
    Effect::new(move |_| {
        let idx = selected_index.get();
        let _ = members.get(); // re-run when the roster changes
        if let Some(el) = list_ref.get() {
            let el: &web_sys::HtmlElement = &el;
            crate::views::chat::list_scroll::reveal_selected_row(el, idx);
        }
    });

    view! {
        <Show when=move || show.get()>
            <div
                node_ref=list_ref
                class="glass animate-pop-in absolute bottom-full inset-x-0 mb-2 z-50
                       rounded-xl border border-border bg-surface-overlay/85 shadow-xl
                       max-h-[200px] overflow-y-auto"
            >
                // "@ everyone" fixed first row
                {move || {
                    let is_selected = selected_index.get() == 0;
                    let cls = if is_selected {
                        "w-full text-left px-3 py-2 flex items-center gap-2 text-sm \
                         transition-colors bg-primary/10 text-primary"
                    } else {
                        "w-full text-left px-3 py-2 flex items-center gap-2 text-sm \
                         transition-colors hover:bg-surface-sunken text-text-primary"
                    };
                    view! {
                        <button
                            data-pidx="0"
                            class=cls
                            on:mousedown=move |ev| {
                                ev.prevent_default(); // keep textarea focus
                                on_select.run("@all ".to_string());
                            }
                        >
                            // Avatar disc — neutral color for "all"
                            <span
                                class="w-6 h-6 rounded-full flex items-center justify-center \
                                       text-[10px] font-bold shrink-0"
                                style="background:#7c9cff;color:#fff"
                            >
                                "*"
                            </span>
                            <span class="font-medium shrink-0">"@ 所有人"</span>
                            <span class="text-text-tertiary text-[10px] truncate">"all"</span>
                        </button>
                    }
                }}
                // Roster rows
                <For
                    each=move || {
                        members
                            .get()
                            .into_iter()
                            .enumerate()
                            .collect::<Vec<_>>()
                    }
                    key=|(_, m)| m.agent_id.clone()
                    children=move |(idx, member)| {
                        // Offset by 1 for the fixed "all" row
                        let row_idx = idx + 1;
                        let id = member.agent_id.clone();
                        let name = member.name.clone();
                        let color = agent_color_for_id(&id);
                        let avatar: String = member
                            .emoji
                            .as_deref()
                            .filter(|e| !e.is_empty())
                            .map(String::from)
                            .unwrap_or_else(|| {
                                name.chars()
                                    .next()
                                    .map(|c| c.to_uppercase().to_string())
                                    .unwrap_or_else(|| "?".to_string())
                            });
                        let token = format!("@{id} ");
                        let pidx = row_idx.to_string();
                        view! {
                            <button
                                data-pidx=pidx
                                class=move || {
                                    let base = "w-full text-left px-3 py-2 flex items-center \
                                                gap-2 text-sm transition-colors";
                                    if selected_index.get() == row_idx {
                                        format!("{base} bg-primary/10 text-primary")
                                    } else {
                                        format!("{base} hover:bg-surface-sunken text-text-primary")
                                    }
                                }
                                on:mousedown=move |ev| {
                                    ev.prevent_default();
                                    on_select.run(token.clone());
                                }
                            >
                                // Avatar disc
                                <span
                                    class="w-6 h-6 rounded-full flex items-center justify-center \
                                           text-[10px] font-bold shrink-0"
                                    style=move || format!("background:{color};color:#fff")
                                >
                                    {avatar.clone()}
                                </span>
                                <span class="font-medium shrink-0">{name.clone()}</span>
                                <span class="text-text-tertiary text-[10px] truncate">{id.clone()}</span>
                            </button>
                        }
                    }
                />
            </div>
        </Show>
    }
}

/// Called by the composer's `on:input` handler. Updates the palette signals in
/// place: `show`, `members` (filtered), `selected_index`. Returns `Some(at_offset)`
/// when a mention is active so the composer can remember where to splice on commit.
pub fn update_mention_palette(
    text: &str,
    caret: usize,
    chat_team_id: Option<String>,
    all_members: &[TeamMemberView],
    show: RwSignal<bool>,
    members: RwSignal<Vec<TeamMemberView>>,
    selected_index: RwSignal<usize>,
) -> Option<usize> {
    // Only active in team mode
    if chat_team_id.is_none() {
        show.set(false);
        return None;
    }
    match active_mention_query(text, caret) {
        Some(query) => {
            let at = mention_at_offset(text, caret).unwrap_or(caret);
            let filtered: Vec<TeamMemberView> = all_members
                .iter()
                .filter(|m| mention_matches(&query, &m.name, &m.agent_id))
                .cloned()
                .collect();
            members.set(filtered);
            selected_index.set(0);
            show.set(true);
            Some(at)
        }
        None => {
            show.set(false);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_name_or_id_case_insensitive() {
        assert!(mention_matches("ris", "风险", "risk_analyst"));
        assert!(mention_matches("风", "风险", "risk_analyst"));
        assert!(mention_matches("", "x", "y"));
        assert!(!mention_matches("zzz", "风险", "risk_analyst"));
    }

    #[test]
    fn detects_active_query_at_caret() {
        assert_eq!(active_mention_query("hi @ris", 7), Some("ris".to_string()));
        assert_eq!(active_mention_query("@all", 4), Some("all".to_string()));
        assert_eq!(active_mention_query("email a@b", 9), None);
        assert_eq!(active_mention_query("done @ris now", 13), None);
    }
}
