//! Integration coverage for the painted chip, measured jump and hit regions.
use super::*;
use ratatui::{backend::TestBackend, Terminal};

fn draw_height(state: &mut AppState, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| render_chat_area(frame, state, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

fn draw(state: &mut AppState, width: u16) -> Vec<String> {
    draw_height(state, width, 10)
}

#[test]
fn startup_notice_keeps_welcome_until_real_transcript_history_exists() {
    let mut state = AppState::new("s".into(), "model".into());
    state.add_system_message("startup notice".into());
    // The wide hero needs a twelve-row viewport (ten rows plus separator).
    let rows = draw_height(&mut state, 80, 14);
    assert!(rows.iter().any(|row| row.contains("Welcome to Aleph")));
    assert!(rows.iter().any(|row| row.contains("startup notice")));

    state.add_user_message("hello".into(), vec![]);
    let rows = draw(&mut state, 80);
    assert!(!rows.iter().any(|row| row.contains("Welcome to Aleph")));
}

#[test]
fn welcome_falls_back_to_compact_header_in_a_short_viewport() {
    let mut state = AppState::new("s".into(), "model".into());
    state.add_system_message("startup notice".into());
    let rows = draw_height(&mut state, 80, 4);
    assert!(!rows.iter().any(|row| row.contains("Welcome to Aleph")));
    assert!(rows[0].contains("Aleph"));
}

#[test]
fn compact_chip_uses_current_layout_and_stays_visible_after_jump() {
    let mut state = AppState::new("s".into(), "model".into());
    state.add_user_message(
        "请解释这一段很长的消息，它必须只占很小的单行空间。".repeat(4),
        vec![],
    );
    for i in 0..40 {
        state.add_system_message(format!("output {i}"));
    }
    let rows = draw(&mut state, 80);
    assert!(rows[0].contains("... [click to jump]"), "{}", rows[0]);
    let jump = (1..79)
        .find_map(|x| {
            state
                .regions
                .hit(x, 0)
                .filter(|r| r.kind == RegionKind::JumpToUser)
                .and_then(|r| r.jump.clone())
        })
        .expect("painted actionable region");
    state.request_sticky_jump(jump);
    let rows = draw(&mut state, 80);
    assert!(rows[0].contains("[click to jump]"));
    assert!(!rows[0].contains("You"));
    assert!(rows[1].replace(' ', "").contains("请解释"), "{rows:?}");
    state.add_system_message("more output".into());
    let rows = draw(&mut state, 40);
    assert!(!rows[0].contains("You"));
    assert!(rows[1].replace(' ', "").contains("请解释"), "{rows:?}");
    assert!(rows[0].contains("[click to jump]"));
    state.scroll_to_bottom();
    assert!(draw(&mut state, 80)[0].contains("[click to jump]"));
}

#[test]
fn narrow_viewport_hides_bar_without_leaving_a_click_target() {
    let mut state = AppState::new("s".into(), "model".into());
    state.add_user_message("question".into(), vec![]);
    for i in 0..30 {
        state.add_system_message(format!("output {i}"));
    }
    let rows = draw(&mut state, 16);
    assert!(!rows[0].contains("[click to jump]"));
    assert!((1..15).all(|x| state
        .regions
        .hit(x, 0)
        .is_none_or(|r| r.kind != RegionKind::JumpToUser)));
}

#[test]
fn transcript_has_no_chat_box_or_title() {
    let mut state = AppState::new("s".into(), "model".into());
    state.add_user_message("hello".into(), vec![]);
    let rows = draw(&mut state, 80);
    for row in rows {
        assert!(!row.contains(" Chat "));
        assert!(!row
            .chars()
            .any(|ch| ['┌', '┐', '└', '┘', '│'].contains(&ch)));
    }
}
