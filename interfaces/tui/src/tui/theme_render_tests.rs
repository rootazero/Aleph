//! Exercise actual cell styles, independently of terminal capability/global preferences.
use super::{app::AppState, render::render, theme};
use ratatui::{
    backend::TestBackend,
    buffer::{Buffer, Cell},
    Terminal,
};
use shared_ui_logic::transcript::SemanticColor;
use tui_textarea::TextArea;

fn find_text<'a>(buf: &'a Buffer, text: &str) -> &'a Cell {
    for y in 0..buf.area.height {
        let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
        if let Some(byte) = row.find(text) {
            let x = row[..byte].chars().count() as u16;
            return &buf[(x, y)];
        }
    }
    panic!("text {text:?} absent from rendered buffer");
}

#[test]
fn all_five_themes_reach_transcript_tool_composer_and_base_cells() {
    let root = tempfile::tempdir().unwrap();
    for name in ["amber", "tide", "jade", "iris", "paper"] {
        let palette = theme::load_named(root.path(), name, true).unwrap();
        let colors = theme::Theme::from_palette(palette.clone());
        theme::with_test_palette(palette.clone(), || {
            let mut state = AppState::new("theme-cells".into(), "model".into());
            state.add_user_message("USER_MARKER".into(), vec![]);
            state.append_assistant_content("# HEADING_MARKER\n\nBODY_MARKER");
            state.start_tool_execution(
                "test-tool".into(),
                "Read".into(),
                &serde_json::json!({"path":"sample.rs"}),
            );
            let mut term = Terminal::new(TestBackend::new(80, 30)).unwrap();
            term.draw(|f| render(f, &mut state, &TextArea::default()))
                .unwrap();
            let buf = term.backend().buffer();
            assert_eq!(
                find_text(buf, "HEADING_MARKER").fg,
                palette.color(SemanticColor::Accent),
                "{name}: heading"
            );
            assert_eq!(
                find_text(buf, "BODY_MARKER").fg,
                palette.color(SemanticColor::Fg),
                "{name}: body"
            );
            assert_eq!(
                find_text(buf, "USER_MARKER").bg,
                colors.user_bg,
                "{name}: user background"
            );
            assert_eq!(
                find_text(buf, "Read").bg,
                colors.surface,
                "{name}: tool surface"
            );
            let caret_y = (0..30).find(|y| buf[(0, *y)].symbol() == "❯").unwrap();
            assert_eq!(
                buf[(0, caret_y)].fg,
                palette.color(SemanticColor::Prompt),
                "{name}: caret"
            );
            assert_eq!(
                buf[(0, caret_y - 1)].fg,
                colors.border_focused,
                "{name}: rule"
            );
            assert_eq!(buf[(79, 29)].bg, colors.background, "{name}: status ground");
        });
    }
}

#[test]
fn paper_command_overlay_restores_background_after_clear() {
    use super::{
        app::PaletteState, command_tree::DisplayEntry,
        widgets::command_palette::render_command_palette,
    };
    let palette = theme::Palette::for_preset(theme::Preset::Paper, true);
    let colors = theme::Theme::from_palette(palette.clone());
    theme::with_test_palette(palette, || {
        let commands = PaletteState {
            input: "theme".into(),
            args: String::new(),
            selected: 0,
            namespace_stack: vec![],
            filtered: vec![DisplayEntry {
                label: "theme".into(),
                hint: "Switch theme".into(),
                is_namespace: false,
                full_command: "/theme".into(),
            }],
        };
        let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
        term.draw(|f| {
            let area = f.area();
            f.buffer_mut().set_style(
                area,
                ratatui::style::Style::default().bg(ratatui::style::Color::Black),
            );
            render_command_palette(f, &commands, ratatui::layout::Rect::new(0, 23, 80, 1));
        })
        .unwrap();
        let buf = term.backend().buffer();
        assert_eq!(find_text(buf, "Commands").bg, colors.background);
        assert_eq!(find_text(buf, "Switch theme").bg, colors.background);
        assert_eq!(buf[(59, 20)].bg, colors.background);
    });
}

#[test]
fn dark_theme_cursor_reverses_readable_body_colors() {
    use super::widgets::input_area::InputWidget;
    use ratatui::{layout::Rect, style::Modifier};
    for name in [
        theme::Preset::Amber,
        theme::Preset::Tide,
        theme::Preset::Jade,
        theme::Preset::Iris,
    ] {
        let palette = theme::Palette::for_preset(name, true);
        let colors = theme::Theme::from_palette(palette.clone());
        theme::with_test_palette(palette, || {
            let mut textarea = TextArea::from(["xy"]);
            textarea.move_cursor(tui_textarea::CursorMove::Head);
            let mut term = Terminal::new(TestBackend::new(80, 5)).unwrap();
            term.draw(|f| {
                InputWidget {
                    textarea: &textarea,
                    focused: true,
                }
                .render(f, Rect::new(0, 0, 80, 3))
            })
            .unwrap();
            let cell = find_text(term.backend().buffer(), "xy");
            assert!(cell.modifier.contains(Modifier::REVERSED));
            assert_eq!(cell.fg, colors.primary);
            assert_eq!(cell.bg, colors.background);
        });
    }
}
