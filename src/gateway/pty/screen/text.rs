//! Visible-region text export — the input side of agent detection.
//!
//! Deliberately NOT a "tail N lines" reader: the detection manifest matches
//! the chrome an agent is painting right now, not its scrollback. The row
//! count is derived from the grid, so a resize moves the window with it.

use super::Screen;

impl Screen {
    /// The whole visible region as text, one line per row, no scrollback.
    #[must_use]
    pub fn visible_text(&self) -> String {
        let rows = self.grid.rows();
        (0..rows)
            .map(|r| self.grid.row_text(r))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use crate::gateway::pty::screen::Screen;

    /// 取的是「当前可视区」，不是一个魔数行数。resize 之后
    /// 范围必须跟着变——边界与它比较的东西共用同一处派生（判据 §12）。
    #[test]
    fn visible_text_follows_the_grid_height_across_resize() {
        let mut s = Screen::new(3, 10);
        s.feed(b"a\r\nb\r\nc");
        assert_eq!(s.visible_text().split('\n').count(), 3);

        s.resize(5, 10);
        assert_eq!(s.visible_text().split('\n').count(), 5);
    }
}
