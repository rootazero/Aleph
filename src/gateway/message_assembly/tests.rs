use super::MessageAssembler;

#[test]
fn deltas_accumulate_into_snapshot() {
    let mut a = MessageAssembler::new();
    let v1 = a.push_text_delta("Hello ");
    let v2 = a.push_text_delta("world");
    assert_eq!(format!("{v1}{v2}"), "Hello world");
    assert_eq!(
        a.snapshot(),
        "Hello world",
        "snapshot mirrors the streamed slices"
    );
}

#[test]
fn inline_think_stripped_from_visible_across_deltas() {
    let mut a = MessageAssembler::new();
    let v1 = a.push_text_delta("answer <thi");
    let v2 = a.push_text_delta("nk>secret</think> done");
    assert_eq!(format!("{v1}{v2}"), "answer  done");
    assert_eq!(
        a.snapshot(),
        "answer  done",
        "stripped inline think never reaches the snapshot"
    );
}

#[test]
fn think_only_turn_yields_no_visible_text() {
    let mut a = MessageAssembler::new();
    a.push_text_delta("<think>only thinking</think>");
    assert_eq!(
        a.snapshot(),
        "",
        "pure-reasoning turn streams nothing visible"
    );
}

#[test]
fn chunk_index_is_monotonic() {
    let mut a = MessageAssembler::new();
    assert_eq!(a.next_chunk_index(), 0);
    assert_eq!(a.next_chunk_index(), 1);
    assert_eq!(a.next_chunk_index(), 2);
}

#[test]
fn reset_iteration_clears_visible_but_keeps_chunk_index() {
    let mut a = MessageAssembler::new();
    a.push_text_delta("first iter");
    assert_eq!(a.next_chunk_index(), 0);
    assert_eq!(a.next_chunk_index(), 1);
    a.reset_iteration();
    assert_eq!(a.snapshot(), "", "visible cleared at iteration boundary");
    assert_eq!(
        a.next_chunk_index(),
        2,
        "chunk index stays monotonic across reset_iteration"
    );
}
