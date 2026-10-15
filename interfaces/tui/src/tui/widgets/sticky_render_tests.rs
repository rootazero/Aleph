//! Integration coverage for the painted chip, measured jump and hit regions.
use super::*;
use ratatui::{backend::TestBackend, Terminal};

fn draw(state: &mut AppState, width: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, 10)).unwrap();
    terminal
        .draw(|frame| render_chat_area(frame, state, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..10)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn compact_chip_uses_current_layout_and_disappears_after_jump() {
    let mut state = AppState::new("s".into(), "model".into());
    state.add_user_message(
        "请解释这一段很长的消息，它必须只占很小的单行空间。".repeat(4),
        vec![],
    );
    for i in 0..40 {
        state.add_system_message(format!("output {i}"));
    }
    let rows = draw(&mut state, 80);
    assert!(rows[1].contains("... [click to jump]"), "{}", rows[1]);
    let jump = (1..79)
        .find_map(|x| {
            state
                .regions
                .hit(x, 1)
                .filter(|r| r.kind == RegionKind::JumpToUser)
                .and_then(|r| r.jump.clone())
        })
        .expect("painted actionable region");
    // The label wins even though its shield covers the same row.
    state.request_sticky_jump(jump);
    let rows = draw(&mut state, 80);
    assert!(!rows[1].contains("[click to jump]"));
    assert!(rows[1].contains("You"));
    assert!(rows[2].replace(' ', "").contains("请解释"), "{rows:?}");
    // Subsequent output and a narrower viewport keep the original user anchor.
    state.add_system_message("more output".into());
    let rows = draw(&mut state, 40);
    assert!(rows[1].contains("You"));
    assert!(rows[2].replace(' ', "").contains("请解释"), "{rows:?}");
    assert!(!rows[1].contains("[click to jump]"));
    state.scroll_to_bottom();
    assert!(draw(&mut state, 80)[1].contains("[click to jump]"));
}

#[test]
fn narrow_viewport_hides_bar_without_leaving_a_click_target() {
    let mut state = AppState::new("s".into(), "model".into());
    state.add_user_message("question".into(), vec![]);
    for i in 0..30 {
        state.add_system_message(format!("output {i}"));
    }
    let rows = draw(&mut state, 16);
    assert!(!rows[1].contains("[click to jump]"));
    assert!((1..15).all(|x| state
        .regions
        .hit(x, 1)
        .is_none_or(|r| r.kind != RegionKind::JumpToUser)));
}
