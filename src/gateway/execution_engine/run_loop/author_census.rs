//! Source-level census over the producers of `AUTHOR_USER_KEY`.
//!
//! `run_loop::with_request_scope`'s doc names TWO origin sites for this key.
//! Before 2026-08-28 only one of them existed, and the doc was the only
//! external reference to the missing wire — grepping the key's name found the
//! comment that vouched for the absent producer, not the absence.
//!
//! This census makes that sentence self-enforcing: it first proves the run
//! loop really does seed `CURRENT_ROOM_AUTHOR` from the key, then requires
//! every named origin site to actually write it.

#[cfg(test)]
mod tests {
    /// Every file that must stamp `AUTHOR_USER_KEY`, and the function whose
    /// body has to contain the write. Named, not globbed: a producer that
    /// stops stamping must fail by name.
    const ORIGIN_SITES: &[(&str, &str, &str)] = &[
        (
            "src/gateway/handlers/agent.rs",
            include_str!("../../handlers/agent.rs"),
            "build_run_request",
        ),
        (
            "src/gateway/inbound_router/executor.rs",
            include_str!("../../inbound_router/executor.rs"),
            "execute_for_context_inner",
        ),
    ];

    /// Everything before the first *inline* `#[cfg(test)] mod <name> { ... }`
    /// block. A bare declaration (`#[cfg(test)] mod tests;`, pointing at a
    /// sibling file like `tests.rs`) is production code — more production
    /// code follows it — and must NOT be mistaken for the boundary: doing so
    /// silently discards everything after the first such declaration,
    /// including real producers.
    fn production_prefix(src: &str) -> String {
        // CRLF-safe: the repo is checked out with CRLF on Windows, so a
        // separator anchored to "\n#[cfg(test)]\n" never matches and the whole
        // file (tests included) would be scanned.
        let normalized = src.replace('\r', "");
        const MARKER: &str = "#[cfg(test)]";
        let mut search_from = 0usize;
        loop {
            let Some(rel) = normalized[search_from..].find(MARKER) else {
                return normalized;
            };
            let at = search_from + rel;
            let after = normalized[at + MARKER.len()..].trim_start();
            let is_inline_module = match after.strip_prefix("mod ") {
                Some(tail) => match (tail.find('{'), tail.find(';')) {
                    (Some(brace), Some(semi)) => brace < semi,
                    (Some(_), None) => true,
                    _ => false,
                },
                None => false,
            };
            if is_inline_module {
                return normalized[..at].to_string();
            }
            search_from = at + MARKER.len();
        }
    }

    /// Drops every `//`/`///`/`//!` comment line. A doc comment naming a
    /// symbol is not evidence the symbol is actually written anywhere — see
    /// this module's own doc for the defect this exists to stop from
    /// recurring: the ONLY reason `AUTHOR_USER_KEY`'s absent producer went
    /// unnoticed for as long as it did was that a doc comment vouching for it
    /// was the sole hit when grepping the key's name.
    fn strip_comment_lines(src: &str) -> String {
        src.lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_run_loop_seeds_the_room_author_from_the_author_key() {
        let prod = strip_comment_lines(&production_prefix(include_str!("mod.rs")));
        assert!(
            prod.contains("AUTHOR_USER_KEY"),
            "run_loop must read AUTHOR_USER_KEY — without this the census below \
             would be requiring producers for a key nobody consumes"
        );
        assert!(
            prod.contains("with_room_author") || prod.contains("CURRENT_ROOM_AUTHOR"),
            "run_loop must seed the room-author task-local from that key"
        );
    }

    #[test]
    fn every_named_origin_site_stamps_the_author_key() {
        let mut checked = 0usize;
        for (path, src, function) in ORIGIN_SITES {
            let prod = production_prefix(src);
            assert!(
                prod.contains(function),
                "{path}: the census names `{function}` but that function is not in \
                 the production half of the file — the census input rotted"
            );
            assert!(
                strip_comment_lines(&prod).contains("AUTHOR_USER_KEY"),
                "{path}: `run_loop::with_request_scope`'s doc names this file as an \
                 origin site for AUTHOR_USER_KEY, but nothing here stamps it. Either \
                 stamp it, or delete the claim from that doc — a doc comment naming \
                 a producer is not that producer."
            );
            checked += 1;
        }
        assert_eq!(
            checked,
            ORIGIN_SITES.len(),
            "the census must have inspected every origin site"
        );
    }
}
