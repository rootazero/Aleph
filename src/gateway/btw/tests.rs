use super::*;
use crate::routing::session_key::SessionKey;

#[test]
fn resolve_accepts_the_documented_spellings() {
    assert_eq!(
        BtwTurn::resolve("/btw what was that config file called?"),
        Some(BtwTurn {
            question: "what was that config file called?".into(),
            promote: false
        })
    );
    // Case-insensitive command, body case preserved verbatim for the model.
    assert_eq!(
        BtwTurn::resolve("/BTW Explain Async/Await").map(|b| b.question),
        Some("Explain Async/Await".into())
    );
    // Telegram's @botname suffix is tolerated.
    assert_eq!(
        BtwTurn::resolve("/btw@MyBot why?").map(|b| b.question),
        Some("why?".into())
    );
    // Newline separator.
    assert_eq!(
        BtwTurn::resolve("/btw\nnext line").map(|b| b.question),
        Some("next line".into())
    );
}

#[test]
fn resolve_rejects_non_btw_and_empty_bodies() {
    assert_eq!(BtwTurn::resolve("hello"), None);
    assert_eq!(BtwTurn::resolve("/help"), None);
    assert_eq!(BtwTurn::resolve("/btwlike this"), None);
    // An empty side question has nowhere to go.
    assert_eq!(BtwTurn::resolve("/btw"), None);
    assert_eq!(BtwTurn::resolve("/btw    "), None);
}

#[test]
fn resolve_recognises_the_promote_verb() {
    let b = BtwTurn::resolve("/btw promote").expect("promote parses");
    assert!(b.promote);
    assert!(b.question.is_empty());
    // "promote" as the first word of a real question is still promote —
    // documented and deliberate; ask "/btw please promote ..." to disambiguate.
    assert!(
        !BtwTurn::resolve("/btw what does promote mean?")
            .expect("q")
            .promote
    );
}

#[test]
fn the_side_key_is_derived_from_the_main_key_including_its_epoch() {
    let main = SessionKey::main("assistant");
    let bumped = main.with_epoch(1);

    let a = side_key_for(&main);
    let b = side_key_for(&main);
    let c = side_key_for(&bumped);

    // Deterministic: same main key, same side key. This is what gives the
    // side thread its memory.
    assert_eq!(a.to_key_string(), b.to_key_string());
    // Epoch-inclusive: /new bumps the epoch, so the side thread starts empty
    // by construction rather than by anyone remembering to clear it.
    assert_ne!(a.to_key_string(), c.to_key_string());
    assert!(matches!(a, SessionKey::Ephemeral { .. }));
    // Agent identity is preserved so partition/visibility predicates still work.
    assert_eq!(a.agent_id(), main.agent_id());
}
