//! Recover human-readable text from a `std::panic` payload.
//!
//! Every `catch_unwind` seam in the tree reports the panic to the model —
//! per-call at the tool dispatch chokepoint (`tools::scoped`), per-run around
//! a sub-agent harness (`agents::subagent_spawner`). A second copy of this
//! downcast ladder would be a second answer to "what did the panic say", so
//! the ladder lives here and both seams read it.

/// Pull a human-readable message out of a panic payload.
///
/// `panic!` boxes a `&'static str` for a literal message and a `String` for a
/// formatted one; a `panic_any` with any other type carries no text to
/// recover, so the caller gets a marker rather than a lie.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "panic (non-string payload)".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The ladder is exercised on directly-built payloads rather than by
    // panicking: the only way to keep a real panic quiet is `set_hook`, which
    // is process-global while libtest runs in parallel — two tests swapping it
    // can leave the silencing hook installed for every other test in the
    // binary. `real_panics_carry_the_payload_types_the_ladder_downcasts` below
    // pins the std behaviour these literals stand in for.

    #[test]
    fn recovers_a_literal_message() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("boom");
        assert_eq!(panic_message(&*payload), "boom");
    }

    #[test]
    fn recovers_an_owned_message() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("boom 7".to_string());
        assert_eq!(panic_message(&*payload), "boom 7");
    }

    #[test]
    fn marks_a_payload_that_carries_no_text() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(42u32);
        assert_eq!(panic_message(&*payload), "panic (non-string payload)");
    }

    #[test]
    fn real_panics_carry_the_payload_types_the_ladder_downcasts() {
        // Guards the assumption the ladder rests on: a literal message arrives
        // as `&'static str`, a formatted one as `String`. Prints two panics to
        // stderr; the assertions, not the output, say whether it passed.
        let literal = std::panic::catch_unwind(|| panic!("boom")).expect_err("must panic");
        assert_eq!(panic_message(&*literal), "boom");

        let n = 7;
        let formatted =
            std::panic::catch_unwind(move || panic!("boom {n}")).expect_err("must panic");
        assert_eq!(panic_message(&*formatted), "boom 7");
    }
}
