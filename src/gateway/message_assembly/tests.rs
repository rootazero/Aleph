use super::MessageAssembler;

#[test]
fn snapshot_equals_finalized_answer_the_antidrift_invariant() {
    let mut a = MessageAssembler::new();
    let v1 = a.push_text_delta("Hello ");
    let v2 = a.push_text_delta("world");
    assert_eq!(format!("{v1}{v2}"), "Hello world");
    let snap = a.snapshot().to_string();
    let final_ans = a.finalize().answer.unwrap();
    assert_eq!(snap, final_ans, "live snapshot must equal terminal answer");
    assert_eq!(final_ans, "Hello world");
}

#[test]
fn inline_think_stripped_from_visible_across_deltas() {
    let mut a = MessageAssembler::new();
    let v1 = a.push_text_delta("answer <thi");
    let v2 = a.push_text_delta("nk>secret</think> done");
    assert_eq!(format!("{v1}{v2}"), "answer  done");
    assert_eq!(a.finalize().answer.as_deref(), Some("answer  done"));
}

#[test]
fn reasoning_deltas_route_to_reasoning_not_answer() {
    let mut a = MessageAssembler::new();
    a.push_text_delta("visible");
    a.push_reasoning_delta("step 1 ");
    a.push_reasoning_delta("step 2");
    let m = a.finalize();
    assert_eq!(m.answer.as_deref(), Some("visible"));
    assert_eq!(m.reasoning.as_deref(), Some("step 1 step 2"));
}

#[test]
fn think_only_turn_yields_no_answer() {
    let mut a = MessageAssembler::new();
    a.push_text_delta("<think>only thinking</think>");
    let m = a.finalize();
    assert_eq!(m.answer, None, "pure-reasoning turn delivers nothing");
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

#[test]
fn finalize_uses_shared_sanitizer_for_task_complete_marker() {
    let mut a = MessageAssembler::new();
    a.push_text_delta("done <task-complete/>");
    // The self-closing marker is caught by the shared final sanitizer,
    // not the streaming scrubber.
    assert_eq!(a.finalize().answer.as_deref(), Some("done"));
}
