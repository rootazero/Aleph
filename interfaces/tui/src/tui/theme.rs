use ratatui::style::Color;

pub struct Theme {
    pub user: Color,
    pub assistant: Color,
    pub system: Color,
    pub tool_running: Color,
    pub tool_success: Color,
    pub tool_failed: Color,
    pub tool_name: Color,
    pub tool_param: Color,
    pub code_bg: Color,
    pub code_block_border: Color,
    pub heading: Color,
    pub link: Color,
    pub quote: Color,
    pub border: Color,
    pub border_focused: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub connected: Color,
    pub disconnected: Color,
    pub primary: Color,
    pub muted: Color,
    pub reasoning: Color,
    pub error: Color,
    pub warning: Color,
}

/// Braille spinner frames shared by every "still working" indicator (status
/// bar run spinner, in-block tool spinner). Keeping one source of truth stops
/// the bars from drifting out of phase and halves the frame-table footprint.
///
/// Indexed by `tick % FRAMES.len()`; ten steps makes the cycle fit a 50 ms
/// TUI tick into a 500 ms rotation without leaning on `Duration` arithmetic.
pub const SPINNER_FRAMES: &[&str] = &[
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

pub const DEFAULT_THEME: Theme = Theme {
    user: Color::Blue,
    assistant: Color::Green,
    system: Color::Yellow,
    tool_running: Color::Yellow,
    tool_success: Color::Green,
    tool_failed: Color::Red,
    tool_name: Color::Cyan,
    tool_param: Color::DarkGray,
    code_bg: Color::DarkGray,
    code_block_border: Color::Gray,
    heading: Color::White,
    link: Color::Blue,
    quote: Color::DarkGray,
    border: Color::Gray,
    border_focused: Color::White,
    status_bg: Color::DarkGray,
    status_fg: Color::White,
    connected: Color::Green,
    disconnected: Color::Red,
    primary: Color::White,
    muted: Color::DarkGray,
    reasoning: Color::DarkGray,
    error: Color::Red,
    warning: Color::Yellow,
};
