// The transcript's first entry: who you are talking to, on what, about which
// folder.
//
// `ℵ Aleph 26.9.1 · claude-opus-5 · ~/proj (main)`
//
// Not stored in `AppState::messages`. It is derived at paint time from the
// state it describes, because every part of it can change after the entry
// would have been created: the model arrives with the session snapshot, and
// `/session` can move this screen to a conversation scoped to another folder.
// A stored first entry would be a second copy of those facts, going stale the
// moment either moved (判据 §1).

use std::path::Path;

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::compact::fit_line;
use crate::tui::theme::theme;

/// The version of the binary the user launched, from the workspace `VERSION`
/// file via this crate's `build.rs` — not `CARGO_PKG_VERSION`, which is the
/// hand-synced copy.
pub const VERSION: &str = env!("ALEPH_VERSION");

const SEP: &str = " \u{b7} "; // ·

// Hollow slanted outline lettering (figlet `slant` strokes, kerned to one
// cell). ASCII only, so every glyph is exactly one cell on every terminal.
const ALEPH_LOGO: [&str; 6] = [
    r"           __                 __  ",
    r"  ____ _  / / ___    ____    / /_ ",
    r" / __ `/ / / / _ \  / __ \  / __ \",
    r"/ /_/ / / / /  __/ / /_/ / / / / /",
    r"\__,_/  /_/ \___/ / .___/ /_/ /_/ ",
    r"                 /_/              ",
];

// Which letter (0..=4, a-l-e-p-h) owns each cell of `ALEPH_LOGO`; `.` is
// blank. Same shape as the logo, pinned by a test, so a colour can never land
// on the neighbouring letter's stroke.
const ALEPH_LOGO_MASK: [&str; 6] = [
    "...........11.................44..",
    "..0000.0..1.1.222....3333....4.44.",
    ".0.00.00.1.1.2.2.2..3.33.3..4.44.4",
    "0.000.0.1.1.2..222.3.333.3.4.4.4.4",
    "000000..111.22222.3.33333.444.444.",
    ".................333..............",
];

const LOGO_WIDTH: usize = 34;

/// One hue per letter, warm to cool, like a spectrum across the word.
const LOGO_HUES: [((u8, u8, u8), ratatui::style::Color); 5] = {
    use ratatui::style::Color;
    [
        ((0xff, 0x5f, 0x56), Color::Red),
        ((0xff, 0xa5, 0x3c), Color::LightRed),
        ((0xff, 0xe3, 0x5a), Color::Yellow),
        ((0x6e, 0xe0, 0x5a), Color::Green),
        ((0x8c, 0x64, 0xff), Color::Magenta),
    ]
};

/// Narrowest frame that shows logo │ guide side by side.
const WIDE_MIN: usize = 73;

fn logo_row(row: usize) -> Line<'static> {
    let palette = crate::tui::theme::palette();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut owner = None;
    for (ch, tag) in ALEPH_LOGO[row].chars().zip(ALEPH_LOGO_MASK[row].chars()) {
        let next = tag.to_digit(10).map(|d| d as usize);
        if next != owner && !run.is_empty() {
            spans.push(logo_span(std::mem::take(&mut run), owner, &palette));
        }
        owner = next;
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(logo_span(run, owner, &palette));
    }
    Line::from(spans)
}

fn logo_span(
    text: String,
    owner: Option<usize>,
    palette: &crate::tui::theme::Palette,
) -> Span<'static> {
    match owner {
        Some(i) => {
            let (rgb, fallback) = LOGO_HUES[i];
            Span::styled(
                text,
                Style::default()
                    .fg(palette.brand(rgb, fallback))
                    .add_modifier(Modifier::BOLD),
            )
        }
        None => Span::raw(text),
    }
}

/// Fit `line` to exactly `width` cells, padding with blanks.
fn pad_line(line: Line<'static>, width: usize) -> Line<'static> {
    let mut line = fit_line(line, width);
    let pad = width.saturating_sub(line.width());
    if pad > 0 {
        line.spans.push(Span::raw(" ".repeat(pad)));
    }
    line
}

/// Build the header line.
///
/// Every argument after the model is optional and *absent* rather than
/// guessed: a conversation scoped to no folder shows no folder, and a folder
/// this machine cannot read shows no branch. The alternative — printing this
/// terminal's own `cwd` — would be the header stating a fact about a
/// different machine's filesystem (判据 §1, the copy that describes another
/// subsystem's behaviour).
#[must_use]
pub fn header_line(
    version: &str,
    model: &str,
    project_root: Option<&str>,
    branch: Option<&str>,
) -> Line<'static> {
    let colors = theme();
    let dim = Style::default().fg(colors.muted);
    let mut spans = vec![
        Span::styled(
            "\u{2135} ".to_string(), // ℵ
            Style::default()
                .fg(colors.user)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("Aleph {version}"),
            Style::default()
                .fg(colors.heading)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(SEP.to_string(), dim),
        Span::styled(model.to_string(), Style::default().fg(colors.tool_name)),
    ];
    if let Some(root) = project_root {
        spans.push(Span::styled(SEP.to_string(), dim));
        spans.push(Span::styled(
            abbreviate_home(root, home_dir().as_deref()),
            dim,
        ));
        if let Some(b) = branch {
            spans.push(Span::styled(format!(" ({b})"), dim));
        }
    }
    Line::from(spans)
}

/// Empty conversations get a welcome surface; history keeps the one-line identity.
/// Widths are terminal cells, not font sizes. All facts still come from the caller.
#[must_use]
pub fn header_lines(
    version: &str,
    model: &str,
    project_root: Option<&str>,
    branch: Option<&str>,
    width: u16,
    welcome: bool,
) -> Vec<Line<'static>> {
    let width = usize::from(width);
    let identity = header_line(version, model, project_root, branch);
    if !welcome {
        return vec![fit_line(identity, width)];
    }
    let colors = theme();
    let title = Style::default()
        .fg(colors.heading)
        .add_modifier(Modifier::BOLD);
    let accent = Style::default()
        .fg(colors.user)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(colors.muted);
    let command = Style::default().fg(colors.tool_name);
    let mut lines = vec![hero_frame_line(
        Line::from(Span::styled(format!("─ Aleph {version} "), title)),
        width,
        "╭",
        "╮",
        "─",
    )];
    let mut body = Vec::with_capacity(8);
    if width >= WIDE_MIN {
        // Left: logo, then the identity split over two left-aligned rows.
        // Right: the guide. A vertical rule between them, as in Pi's hero.
        let mut left: Vec<Line<'static>> = (0..ALEPH_LOGO.len()).map(logo_row).collect();
        left.push(Line::from(vec![
            Span::styled("\u{2135} ", accent),
            Span::styled(model.to_string(), command),
        ]));
        left.push(match project_root {
            Some(root) => {
                let mut spans = vec![Span::styled(
                    abbreviate_home(root, home_dir().as_deref()),
                    muted,
                )];
                if let Some(b) = branch {
                    spans.push(Span::styled(format!(" ({b})"), muted));
                }
                Line::from(spans)
            }
            None => Line::default(),
        });
        // inner = width - 2 edges; content = 1 lead pad + left + " │ " + right + 1 pad.
        let inner = width - 2;
        let left_width = LOGO_WIDTH + 2;
        let right_width = inner - 1 - left_width - 3 - 1;
        let rule = Style::default().fg(colors.muted);
        let key_line = |key: &'static str, description: &'static str| {
            Line::from(vec![
                Span::styled(key, command),
                Span::raw(" ".repeat(10usize.saturating_sub(key.width()))),
                Span::styled(description, muted),
            ])
        };
        let right = [
            Line::from(Span::styled("Welcome to Aleph", title)),
            Line::from(Span::styled("Ask a question to begin", muted)),
            Line::from(Span::styled("\u{2500}".repeat(right_width.min(30)), rule)),
            Line::from(Span::styled("Commands", title)),
            key_line("/help", "Commands & shortcuts"),
            key_line("/session", "Browse conversations"),
            key_line("/agents", "Background agents"),
            Line::default(),
        ];
        for (l, r) in left.into_iter().zip(right) {
            let mut spans = pad_line(l, left_width).spans;
            spans.push(Span::styled(" \u{2502} ", rule));
            spans.extend(pad_line(r, right_width).spans);
            body.push(Line::from(spans));
        }
        for mut line in body {
            line.spans.insert(0, Span::raw(" "));
            lines.push(hero_frame_line(line, width, "│", "│", " "));
        }
        lines.push(hero_frame_line(Line::default(), width, "╰", "╯", "─"));
        return lines;
    } else {
        body.push(Line::from(Span::styled("ℵ Welcome to Aleph", title)));
        body.push(Line::from(Span::styled("Ask a question to begin", muted)));
        for (key, description) in [
            ("/help", "Commands & shortcuts"),
            ("/session", "Conversations"),
            ("/agents", "Background agents"),
        ] {
            let mut spans = vec![Span::styled(key, command)];
            if width >= 32 {
                spans.push(Span::styled(format!("  {description}"), muted));
            }
            body.push(Line::from(spans));
        }
    }
    // Narrow: eight rows including identity and borders. Wide: ten rows.
    // chat_area adds a separator and gates the hero on a twelve-row viewport.
    body.push(identity);
    for mut line in body {
        line.spans.insert(0, Span::raw(" "));
        lines.push(hero_frame_line(line, width, "│", "│", " "));
    }
    lines.push(hero_frame_line(Line::default(), width, "╰", "╯", "─"));
    lines
}

/// Fit the contents before adding edges, so truncation never eats the right edge.
/// All frame glyphs occupy one terminal cell; even widths zero and one are safe.
fn hero_frame_line(
    content: Line<'static>,
    width: usize,
    left: &'static str,
    right: &'static str,
    fill: &'static str,
) -> Line<'static> {
    let border = Style::default().fg(theme().muted);
    if width == 0 {
        return Line::default();
    }
    let mut spans = vec![Span::styled(left, border)];
    if width > 1 {
        let inner_width = width - 2;
        let content = fit_line(content, inner_width);
        let padding = inner_width.saturating_sub(content.width());
        spans.extend(content.spans);
        spans.push(Span::styled(fill.repeat(padding), border));
        spans.push(Span::styled(right, border));
    }
    Line::from(spans)
}

/// `C:\Users\me\proj` → `~\proj`. Cosmetic only: a path that does not start
/// with the home directory is printed as it came.
///
/// `home` is a parameter rather than an ambient read so its test does not have
/// to set a process-wide environment variable — which in this test binary
/// would race every other test that reads one.
fn abbreviate_home(path: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    if home.is_empty() || !path.starts_with(home) {
        return path.to_string();
    }
    let rest = &path[home.len()..];
    if rest.is_empty() {
        return "~".to_string();
    }
    // Only when the match ends on a separator — `/home/meagain` must not
    // become `~again`.
    if rest.starts_with('/') || rest.starts_with('\\') {
        return format!("~{rest}");
    }
    path.to_string()
}

/// This machine's home directory, as a string to compare a path prefix
/// against.
///
/// Delegates to [`aleph_protocol::paths::home_dir`] rather than reading the
/// variables here. B6 wrote its own `HOME`-then-`USERPROFILE` version, which
/// was the shared one minus its `HOMEDRIVE` + `HOMEPATH` arm — a copy that is
/// born weaker than its original and agrees with it on every machine except
/// the ones the missing arm exists for (判据 §1).
fn home_dir() -> Option<String> {
    aleph_protocol::paths::home_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|h| !h.is_empty())
}

/// The git branch checked out at `root`, read from this machine's filesystem.
///
/// # Why this is allowed to be wrong, and how it fails when it is
///
/// `project_root` is the server's answer — the folder the conversation is
/// scoped to. The branch is not on the wire at all, and no RPC reports it, so
/// the only way to know it is to read the path here. When the gateway runs on
/// this machine (loopback, which is the default and the overwhelming case)
/// that is the same filesystem and the answer is exact. When it does not, the
/// path almost always fails to resolve and the branch is simply absent — the
/// fail-closed direction. The residual: a remote gateway whose `project_root`
/// happens to name a path that *also* exists here would show this machine's
/// branch for it. Left standing rather than gated on a loopback check the
/// client has no accessor for, and recorded here so the next reader does not
/// have to re-derive it.
///
/// Returns the branch name, or the short commit for a detached HEAD. `None`
/// for anything it cannot read — never a placeholder, and never the string
/// `HEAD`.
#[must_use]
pub fn git_branch_of(root: &str) -> Option<String> {
    let git = Path::new(root).join(".git");
    // A worktree (and a submodule) has `.git` as a FILE holding
    // `gitdir: <path>`. This repository is itself a worktree, so the case is
    // not exotic.
    let head = if git.is_dir() {
        git.join("HEAD")
    } else {
        let pointer = std::fs::read_to_string(&git).ok()?;
        let dir = pointer.trim().strip_prefix("gitdir:")?.trim();
        Path::new(dir).join("HEAD")
    };
    let contents = std::fs::read_to_string(head).ok()?;
    let contents = contents.trim();
    if let Some(reference) = contents.strip_prefix("ref:") {
        let name = reference.trim().rsplit('/').next()?;
        return (!name.is_empty()).then(|| name.to_string());
    }
    // Detached HEAD: the raw object id. Short form, because the long one is
    // 40 columns of a header that has other things to say.
    let short: String = contents.chars().take(7).collect();
    (short.len() == 7 && short.chars().all(|c| c.is_ascii_hexdigit())).then_some(short)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn logo_and_mask_share_one_shape() {
        for (row, mask) in ALEPH_LOGO.iter().zip(ALEPH_LOGO_MASK) {
            assert_eq!(row.chars().count(), LOGO_WIDTH, "{row}");
            assert_eq!(mask.chars().count(), LOGO_WIDTH, "{mask}");
            assert!(row.is_ascii());
            for (ch, tag) in row.chars().zip(mask.chars()) {
                assert_eq!(ch == ' ', tag == '.', "{row} / {mask}");
            }
        }
    }

    #[test]
    fn wide_hero_fills_every_cell_with_borders_logo_and_real_commands() {
        for width in [73, 80, 120, 240] {
            let lines = header_lines(
                "26.9.1",
                "model",
                Some("/w/proj"),
                Some("main"),
                width,
                true,
            );
            assert_eq!(lines.len(), 10);
            for line in &lines {
                assert_eq!(line.width(), usize::from(width), "{}", text(line));
                assert_eq!(text(line).width(), usize::from(width));
            }
            let rows = lines.iter().map(text).collect::<Vec<_>>();
            assert_eq!(
                rows[0],
                format!("╭─ Aleph 26.9.1 {}╮", "─".repeat(usize::from(width) - 17))
            );
            assert_eq!(rows[9], format!("╰{}╯", "─".repeat(usize::from(width) - 2)));
            for (row, logo) in rows[1..7].iter().zip(ALEPH_LOGO) {
                assert!(row.starts_with(&format!("│ {logo}")), "{row}");
                assert!(row.ends_with('│'));
            }
            // Left column text is left-aligned, and a rule splits the columns
            // at the same cell on every body row.
            assert!(rows[7].starts_with("│ ℵ model "), "{}", rows[7]);
            assert!(rows[8].starts_with("│ /w/proj (main) "), "{}", rows[8]);
            let rule_at = 2 + LOGO_WIDTH + 2 + 1;
            for row in &rows[1..9] {
                assert_eq!(row.chars().nth(rule_at), Some('│'), "{row}");
            }
            let rendered = rows.join(
                "
",
            );
            for expected in [
                "Welcome to Aleph",
                "Ask a question to begin",
                "Commands",
                "/help     Commands & shortcuts",
                "/session  Browse conversations",
                "/agents   Background agents",
            ] {
                assert!(
                    rendered.contains(expected),
                    "missing {expected}: {rendered}"
                );
            }
            assert!(!rendered.contains("skills"));
        }
    }

    #[test]
    fn narrow_hero_is_cell_safe_for_every_width_and_unicode_identity() {
        for width in 0..73 {
            let lines = header_lines(
                "版本26.9.1🦀",
                "模型e\u{301}🦀",
                Some("/项目/界面"),
                Some("分支"),
                width,
                true,
            );
            assert_eq!(lines.len(), 8);
            for (index, line) in lines.iter().enumerate() {
                let row = text(line);
                assert_eq!(line.width(), usize::from(width), "width {width}: {row}");
                assert_eq!(row.width(), usize::from(width), "width {width}: {row}");
                if width >= 2 {
                    let (left, right) = match index {
                        0 => ('╭', '╮'),
                        7 => ('╰', '╯'),
                        _ => ('│', '│'),
                    };
                    assert!(row.starts_with(left) && row.ends_with(right), "{row}");
                }
            }
            if width >= 21 {
                let rendered = lines.iter().map(text).collect::<Vec<_>>().join("\n");
                for expected in ["Welcome to Aleph", "/help", "/session", "/agents"] {
                    assert!(rendered.contains(expected), "width {width}: {rendered}");
                }
            }
        }
    }

    #[test]
    fn existing_conversations_keep_one_compact_identity_line() {
        let lines = header_lines("26.9.1", "model", Some("/w/proj"), Some("main"), 80, false);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            text(&lines[0]),
            text(&header_line(
                "26.9.1",
                "model",
                Some("/w/proj"),
                Some("main")
            ))
        );
        assert!(!text(&lines[0]).contains("Welcome"));
    }

    /// The full shape, and the one thing about it that is easy to get
    /// wrong: the version is the one `build.rs` derived from `VERSION`, not
    /// `CARGO_PKG_VERSION`.
    #[test]
    fn the_header_names_the_app_the_model_and_the_folder() {
        let line = header_line("26.9.1", "claude-opus-5", Some("/w/proj"), Some("main"));
        assert_eq!(
            text(&line),
            "\u{2135} Aleph 26.9.1 \u{b7} claude-opus-5 \u{b7} /w/proj (main)"
        );
    }

    /// A conversation scoped to no folder says nothing about a folder. It
    /// must NOT fall back to this terminal's own working directory: on a
    /// remote gateway that is a different machine's filesystem, and the
    /// reader has no way to tell the two apart.
    #[test]
    fn an_unscoped_conversation_shows_no_folder_and_no_branch() {
        let line = header_line("26.9.1", "m", None, Some("main"));
        assert_eq!(text(&line), "\u{2135} Aleph 26.9.1 \u{b7} m");
        let unreadable = header_line("26.9.1", "m", Some("/w/proj"), None);
        assert_eq!(
            text(&unreadable),
            "\u{2135} Aleph 26.9.1 \u{b7} m \u{b7} /w/proj"
        );
    }

    #[test]
    fn the_version_comes_from_the_version_file_not_the_cargo_copy() {
        // Both are CalVer strings, so a wrong wiring would still *look*
        // right; what pins it is that this is the file the release process
        // writes.
        // Anchor the path on the package manifest dir so the test works
        // regardless of the cwd cargo happens to invoke it from.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let from_file = std::fs::read_to_string(format!("{}/../../VERSION", manifest_dir))
            .expect("workspace VERSION file");
        assert_eq!(VERSION, from_file.trim());
    }

    #[test]
    fn home_is_abbreviated_only_on_a_component_boundary() {
        let home = Some("/home/me");
        assert_eq!(abbreviate_home("/home/me/proj", home), "~/proj");
        assert_eq!(abbreviate_home("/home/me", home), "~");
        // The trap: a sibling directory whose name merely starts with the
        // home path.
        assert_eq!(
            abbreviate_home("/home/meagain/proj", home),
            "/home/meagain/proj"
        );
        assert_eq!(abbreviate_home("/elsewhere", home), "/elsewhere");
        // No home known: print what came, rather than nothing.
        assert_eq!(abbreviate_home("/home/me/proj", None), "/home/me/proj");
        assert_eq!(abbreviate_home("/home/me/proj", Some("")), "/home/me/proj");
        // Windows separators take the same boundary rule.
        assert_eq!(
            abbreviate_home(r"C:\Users\me\proj", Some(r"C:\Users\me")),
            r"~\proj"
        );
    }

    /// The branch of the repository this test is compiled in. It is a
    /// worktree, so this also exercises the `.git`-is-a-file path — the one
    /// a `.git/HEAD`-only implementation gets wrong.
    #[test]
    fn a_worktrees_branch_is_readable_through_its_gitdir_pointer() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let branch = git_branch_of(root);
        assert!(
            branch.is_some(),
            "this crate's own repository root must resolve a branch: {root}"
        );
        let branch = branch.unwrap_or_default();
        assert_ne!(branch, "HEAD", "`ref: refs/heads/x` must yield `x`");
        assert!(!branch.contains('/'), "only the leaf name: {branch}");
    }

    #[test]
    fn a_folder_that_is_not_a_repository_has_no_branch() {
        assert_eq!(git_branch_of(env!("CARGO_MANIFEST_DIR")), None);
        assert_eq!(git_branch_of("/definitely/not/here"), None);
    }
}
