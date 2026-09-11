//! The one place that decides which Markdown extensions assistant output is
//! parsed with.
//!
//! # Why this is a shared constant and not four literals
//!
//! The same assistant reply is parsed by four renderers — the Panel
//! (`components/markdown.rs` → HTML), the TUI (`tui/markdown.rs` → ratatui
//! lines), the CLI (`output/markdown.rs` → ANSI) and the session exporter
//! (`src/export/markdown.rs` → an HTML file). A construct only one of them
//! recognises does not degrade, it changes meaning: `- [ ] ship it` is a
//! checkbox in the Panel and the literal text `[ ]` everywhere the tasklist
//! flag is missing, and a GFM table is a grid in one place and a row of pipes
//! in another. The reader cannot tell which one is lying.
//!
//! That had already happened. As of 2026-09-11 the CLI parsed with
//! `STRIKETHROUGH | TABLES` while the Panel and the exporter used
//! `STRIKETHROUGH | TABLES | TASKLISTS`, and the exporter carried the comment
//! "Same extension set as the Panel renderer" — a claim about another
//! subsystem's behaviour, which is the form of duplication that silently
//! becomes false because nobody tells the quoted party it has been quoted
//! (判据 §1).
//!
//! # What is deliberately NOT this fact
//!
//! `src/builtin_tools/pdf_generate/{browser,native}_engine.rs` parse with
//! `Options::all()`, and `interfaces/webchat/src/memory_graph/
//! markdown_excerpt.rs` parses with none. Those are different questions —
//! "render a document the user asked for" and "pull a plain-text excerpt" —
//! not drifted copies of this one, and they must not be folded in here.

use pulldown_cmark::Options;

/// Markdown extensions every renderer of **assistant output** enables.
///
/// Deliberately not `Options::all()`: the fewer constructs the parser
/// recognises, the smaller the surface that model-authored text can reach.
/// Footnotes, definition lists, math and metadata blocks are all off because
/// no renderer paints them, and a construct that parses but does not paint
/// disappears from the output instead of showing as its source text.
///
/// # Adding a flag
///
/// Turning one on here turns it on in all four renderers at once, so the
/// construct must be *painted* in all four before it is added — otherwise the
/// one that only parses it drops it silently.
#[must_use]
pub fn markdown_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact set, spelled out, so widening it is a deliberate edit in two
    /// places rather than a one-character slip in one.
    ///
    /// # When this goes red
    ///
    /// Adding or removing any extension. That is the point: the flag set is
    /// read by four crates that cannot see each other's output, so it should
    /// not be possible to change it while thinking about only one of them.
    #[test]
    fn the_flag_set_is_exactly_these_three() {
        let opts = markdown_options();
        assert!(opts.contains(Options::ENABLE_STRIKETHROUGH));
        assert!(opts.contains(Options::ENABLE_TABLES));
        assert!(opts.contains(Options::ENABLE_TASKLISTS));
        assert_eq!(
            opts.iter().count(),
            3,
            "a flag was added without a renderer that paints it: {opts:?}"
        );
    }

    /// `Options::all()` is a different fact, and this says so out loud: if the
    /// two ever coincided, every caller could stop reading this module and the
    /// distinction the doc draws would quietly stop being true.
    #[test]
    fn the_assistant_set_is_narrower_than_everything() {
        assert_ne!(markdown_options(), Options::all());
    }
}
