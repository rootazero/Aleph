// Provider/model picker widget: renders a floating overlay above the input
// area over the rows `providers.catalog` returned, with a filter line, a
// breadcrumb for the descended-into provider, and a selected-item indicator.
// Confirming a provider descends into its roster; confirming a model sends
// `/model <id>`.
//
// Rendering only. Every fact on screen is a field the server sent — this file
// chooses wording and colour, never which models exist or which row wins (R4).

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};

use aleph_protocol::providers::{AuthKind, CatalogEntry, ModelSource, RateCard, RosterModel};

use crate::tui::app::{PickerRow, ProviderPickerState};
use crate::tui::theme::DEFAULT_THEME;

/// Maximum number of visible items in the picker overlay.
const MAX_VISIBLE_ITEMS: u16 = 12;

/// Render the provider picker overlay. `area` is the input area's Rect — the
/// picker floats above it (same placement as the session picker).
pub fn render_provider_picker(frame: &mut Frame, picker: &ProviderPickerState, area: Rect) {
    let item_count = u16::try_from(picker.rows.len()).unwrap_or(u16::MAX);
    let visible_count = item_count.clamp(1, MAX_VISIBLE_ITEMS);
    // Height = visible items + 2 (borders) + 1 (filter line at top)
    let overlay_height = visible_count.saturating_add(3);

    let overlay_y = area.y.saturating_sub(overlay_height);
    // Wider than the session picker: a row carries an id, a provenance and a
    // lifecycle note, and truncating the lifecycle note is the one loss that
    // matters (it is what stops a retired id being picked).
    let overlay_width = area.width.min(78);
    let overlay_rect = Rect::new(area.x, overlay_y, overlay_width, overlay_height);

    frame.render_widget(Clear, overlay_rect);

    let open = picker.provider.and_then(|i| picker.entries.get(i));
    let title = open.map_or_else(
        || " Providers ".to_string(),
        |entry| format!(" Providers \u{25b8} {} ", entry.display_name),
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DEFAULT_THEME.border_focused))
        .title(title);

    let inner = block.inner(overlay_rect);
    frame.render_widget(block, overlay_rect);

    if inner.height < 2 {
        return;
    }

    // Filter line at the top, with the one binding that is not guessable.
    // Up/Down/Enter/Backspace are the picker conventions this overlay shares
    // with the session picker; Ctrl+R is new here, so an unadvertised key
    // would be an unusable one.
    let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let filter_line = Paragraph::new(Line::from(vec![
        Span::styled(
            format!("filter: {}", picker.input),
            Style::default().fg(DEFAULT_THEME.primary),
        ),
        Span::styled(
            refresh_hint(picker),
            Style::default().fg(DEFAULT_THEME.muted),
        ),
    ]));
    frame.render_widget(filter_line, filter_area);

    let list_area = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );

    if picker.rows.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            empty_message(open),
            Style::default().fg(DEFAULT_THEME.muted),
        )));
        frame.render_widget(empty, list_area);
        return;
    }

    let items: Vec<ListItem> = picker
        .rows
        .iter()
        .enumerate()
        .filter_map(|(row, entry)| {
            let is_selected = row == picker.selected;
            let indicator = if is_selected { "> " } else { "  " };
            let (text, deprecated) = match entry {
                PickerRow::Provider { index, matched } => {
                    let provider = picker.entries.get(*index)?;
                    (
                        provider_label(provider, *matched),
                        // The head of the roster is the id a bare `/model` would
                        // land on, so this is the same warning the entry-level
                        // `lifecycle` field used to carry — read off the list
                        // actually being offered rather than off a `default_model`
                        // the operator's ladder may have replaced.
                        provider
                            .roster
                            .first()
                            .is_some_and(|m| m.lifecycle.is_deprecated()),
                    )
                }
                PickerRow::Model { model } => (model_label(model), model.lifecycle.is_deprecated()),
            };
            let style = if is_selected {
                Style::default()
                    .fg(DEFAULT_THEME.primary)
                    .add_modifier(Modifier::BOLD)
            } else if deprecated {
                Style::default().fg(DEFAULT_THEME.warning)
            } else {
                Style::default().fg(DEFAULT_THEME.muted)
            };
            Some(ListItem::new(Line::from(Span::styled(
                format!("{indicator}{text}"),
                style,
            ))))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(picker.selected));

    frame.render_stateful_widget(List::new(items), list_area, &mut list_state);
}

/// Whether to advertise Ctrl+R, and for what.
///
/// Six presets publish no `/models` endpoint. The Panel hides its fetch button
/// for them and the CLI prints "no `/models` endpoint; list them by hand" — this
/// footer offered the key to everyone, so on those rows it advertised an action
/// whose only possible outcome was a failure message. The row it asks about is
/// the one Ctrl+R would actually act on, taken from the same state method the
/// handler calls, because "the hint and the action pick the same row" is not
/// something two independent index walks can promise.
fn refresh_hint(picker: &ProviderPickerState) -> String {
    match picker.refresh_target() {
        Some(target) if target.discoverable => "   ^R fetch models".to_string(),
        // Naming the vendor matters at the provider level, where the highlight
        // moves: "this one cannot" is only actionable if you know which one.
        Some(target) => format!("   ({} publishes no model list)", target.display_name),
        // No row to act on at all — a filter that matched nothing, say.
        None => String::new(),
    }
}

/// What an empty list means at this level.
///
/// A provider that ships no roster is not the same as a filter that matched
/// nothing, and the difference is actionable: the first needs a model id typed
/// by hand, the second needs a shorter filter.
fn empty_message(open: Option<&CatalogEntry>) -> String {
    match open {
        Some(entry) if entry.roster.is_empty() => format!(
            "  ({} publishes no model list — send /model <id> yourself)",
            entry.display_name
        ),
        Some(_) => "  (no matching models)".to_string(),
        None => "  (no matching providers)".to_string(),
    }
}

/// One provider row: who it is, whether it is usable, and how much it offers.
fn provider_label(entry: &CatalogEntry, matched: usize) -> String {
    let total = entry.roster.len();
    let models = if matched < total {
        format!("{matched}/{total} models")
    } else {
        format!("{total} models")
    };
    let mut notes = vec![credential_note(entry).to_string(), models];
    if entry.is_default {
        notes.insert(0, "default".to_string());
    }
    format!(
        "{:<22} {:<14} {}",
        truncate_chars(&entry.display_name, 22),
        truncate_chars(&entry.id, 14),
        notes.join("  ")
    )
}

/// Whether this provider can be used right now, and if not, how to link it.
///
/// `auth_kind` is the server's — it is why a client no longer needs its own
/// list of which vendors take a pasted key and which take a sign-in.
const fn credential_note(entry: &CatalogEntry) -> &'static str {
    if entry.verified {
        "verified"
    } else if entry.has_api_key {
        "key set"
    } else {
        match entry.auth_kind {
            AuthKind::OAuth => "needs sign-in",
            AuthKind::ApiKey => "needs key",
        }
    }
}

/// One model row: the id to send, what it costs, where it came from, and
/// whether the vendor has retired it.
///
/// The window and the price are the two facts that decide the pick, and they
/// used to reach no picker at all: the catalogue sent them for the provider's
/// *default* model on a row whose job is choosing a different one. Per-model
/// they are worth a column. Both are blank when the reference tables have no
/// row — the normal state for an id scraped off a live `/models` endpoint, and
/// the reason the cells are formatted by the contract rather than here (a `0`
/// invented locally is a claim the catalogue never made).
fn model_label(model: &RosterModel) -> String {
    let reference = reference_note(model);
    let mut label = format!(
        "{:<34} {:<16} {}",
        truncate_chars(&model.id, 34),
        reference,
        source_note(model.source)
    );
    if model.lifecycle.is_deprecated() {
        // The successor is the whole value of knowing it is deprecated — it is
        // the id the operator should use instead.
        label.push_str(&model.lifecycle.successor.as_ref().map_or_else(
            || "  DEPRECATED".to_string(),
            |s| format!("  DEPRECATED \u{2192} {s}"),
        ));
    } else if model.lifecycle.is_preview() {
        label.push_str("  preview");
    }
    label
}

/// The window and the price in one column, or an empty string when the
/// reference tables know neither.
///
/// Both halves come from the contract's own formatters, for the same reason the
/// lifecycle words do: `128K` and `$3/$15` are one number each, and a second
/// spelling invented in a terminal widget is a second answer.
fn reference_note(model: &RosterModel) -> String {
    let window = model
        .capabilities
        .as_ref()
        .map(|c| c.context_window_short());
    let price = model.cost.as_ref().and_then(RateCard::io_per_mtok_short);
    match (window, price) {
        (Some(w), Some(p)) => format!("{w} {p}"),
        (Some(w), None) => w,
        (None, Some(p)) => p,
        (None, None) => String::new(),
    }
}

/// Human wording for a model's provenance. The wire spelling is snake_case;
/// this is the same fact said out loud.
const fn source_note(source: ModelSource) -> &'static str {
    match source {
        ModelSource::PresetDefault => "preset default",
        ModelSource::PresetFallback => "preset",
        ModelSource::PresetAux => "preset aux",
        ModelSource::Configured => "configured",
        ModelSource::Discovered => "discovered",
    }
}

/// Cut on a char boundary, never a byte one (P7).
fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars()
        .take(max.saturating_sub(1))
        .chain(['…'])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::providers::{ModelCapabilities, ModelLifecycle, ModelStatus, RateBasis};

    fn model(id: &str, source: ModelSource) -> RosterModel {
        RosterModel::new(id, source)
    }

    #[test]
    fn max_visible_items_capped() {
        assert_eq!(MAX_VISIBLE_ITEMS, 12);
    }

    /// A retired id must say so, and must name what to use instead — the
    /// successor is the actionable half.
    #[test]
    fn a_deprecated_model_row_names_its_successor() {
        let mut retired = model("gpt-4", ModelSource::PresetFallback);
        retired.lifecycle = ModelLifecycle {
            status: ModelStatus::Deprecated,
            successor: Some("gpt-5.6".into()),
            note: None,
        };
        let label = model_label(&retired);
        assert!(label.contains("DEPRECATED"), "{label}");
        assert!(label.contains("gpt-5.6"), "{label}");

        let live = model("gpt-5.6", ModelSource::PresetDefault);
        assert!(!model_label(&live).contains("DEPRECATED"));
        assert!(model_label(&live).contains("preset default"));
    }

    /// "This provider has nothing to offer" and "your filter matched nothing"
    /// need different next actions, so they must not share a sentence.
    #[test]
    fn an_empty_roster_is_not_an_empty_filter() {
        assert!(empty_message(None).contains("providers"));

        let barren = crate::tui::app::sample_catalog_entry("relay", &[]);
        assert!(empty_message(Some(&barren)).contains("/model <id>"));

        let stocked = crate::tui::app::sample_catalog_entry("openai", &["gpt-5.6"]);
        assert!(empty_message(Some(&stocked)).contains("no matching models"));
    }

    /// A provider surfaced through one of its models says how many matched, so
    /// the count on screen is not a claim the roster is that small.
    #[test]
    fn a_narrowed_provider_row_shows_both_numbers() {
        let entry = crate::tui::app::sample_catalog_entry("openai", &["a", "b", "c"]);
        assert!(provider_label(&entry, 3).contains("3 models"));
        assert!(provider_label(&entry, 1).contains("1/3 models"));
    }

    /// An unlinked provider must say which act links it — the two are different
    /// buttons on every other surface, and the server is the one that knows.
    #[test]
    fn an_unlinked_provider_names_the_act_that_links_it() {
        let mut entry = crate::tui::app::sample_catalog_entry("openai", &["a"]);
        assert_eq!(credential_note(&entry), "needs key");
        entry.auth_kind = AuthKind::OAuth;
        assert_eq!(credential_note(&entry), "needs sign-in");
        entry.has_api_key = true;
        assert_eq!(credential_note(&entry), "key set");
        entry.verified = true;
        assert_eq!(credential_note(&entry), "verified");
    }

    #[test]
    fn truncation_lands_on_a_char_boundary() {
        assert_eq!(truncate_chars("abc", 8), "abc");
        assert_eq!(truncate_chars("日本語のモデル名", 4), "日本語…");
    }

    /// The two facts that decide the pick have to be on the row being picked.
    #[test]
    fn a_model_row_shows_the_window_and_the_price() {
        let mut m = model("claude-opus-4.6", ModelSource::Configured);
        m.capabilities = Some(ModelCapabilities {
            context_window: 1_000_000,
            max_output_tokens: 64_000,
            supports_vision: true,
            supports_tools: true,
            supports_reasoning: true,
        });
        m.cost = Some(RateCard {
            input_per_mtok: Some(5.0),
            output_per_mtok: Some(25.0),
            cache_read_per_mtok: None,
            cache_creation_per_mtok: None,
            reasoning_per_mtok: None,
            basis: RateBasis::Direct,
        });
        let label = model_label(&m);
        assert!(label.contains("1M"), "{label}");
        assert!(label.contains("$5/$25"), "{label}");
    }

    /// A scraped id has no curated row, and the cell must stay empty rather than
    /// claim a zero window or a free price.
    #[test]
    fn a_discovered_row_with_no_curated_data_says_nothing() {
        let m = model("some-relay-model", ModelSource::Discovered);
        assert_eq!(reference_note(&m), "");
        let label = model_label(&m);
        assert!(!label.contains('$'), "{label}");
        assert!(!label.contains('0'), "{label}");
        assert!(label.contains("discovered"), "{label}");
    }

    /// Ctrl+R must not be advertised on the six presets that publish no
    /// listing — the Panel hides its button for them and the CLI says so in
    /// words; this footer used to offer the key to everyone.
    #[test]
    fn the_footer_only_offers_a_fetch_where_one_can_work() {
        use crate::tui::app::{PickerRow, ProviderPickerState};

        let mut listing = crate::tui::app::sample_catalog_entry("openai", &["gpt-5.6"]);
        listing.discoverable = true;
        let mut silent = crate::tui::app::sample_catalog_entry("amazon-bedrock", &["claude"]);
        silent.discoverable = false;

        let picker = |entries: Vec<CatalogEntry>| ProviderPickerState {
            input: String::new(),
            entries,
            provider: None,
            rows: vec![PickerRow::Provider {
                index: 0,
                matched: 1,
            }],
            selected: 0,
        };

        assert!(refresh_hint(&picker(vec![listing])).contains("^R"));

        let hint = refresh_hint(&picker(vec![silent]));
        assert!(!hint.contains("^R"), "{hint}");
        // Naming the vendor is the actionable half at the provider level.
        assert!(hint.contains("AMAZON-BEDROCK"), "{hint}");
    }

    /// With nothing highlighted there is no row to talk about, so the footer
    /// says nothing rather than offering a key that resolves to no target.
    #[test]
    fn the_footer_is_silent_when_the_filter_matched_nothing() {
        use crate::tui::app::ProviderPickerState;

        let picker = ProviderPickerState {
            input: "zzz".to_string(),
            entries: vec![crate::tui::app::sample_catalog_entry("openai", &["a"])],
            provider: None,
            rows: Vec::new(),
            selected: 0,
        };
        assert_eq!(refresh_hint(&picker), "");
    }
}
