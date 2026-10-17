use aleph_protocol::hook::*;
use serde_json::{json, Value};

const SESSION: &str = "agent:main:fixture";
// A TurnId-shaped fixture fact, independent of any run_id.
const TURN: &str = "0192a193-4c13-7000-8000-000000000005";

fn envelope(event: Value) -> Value {
    json!({"version":HOOK_CONTRACT_VERSION,"session_id":SESSION,"turn_id":TURN,"slot":"fixture.slot","event":event})
}

fn pair(call: &str, result: &str) -> Value {
    json!({"call_id":call,"result_message_id":result})
}

fn compact() -> Value {
    envelope(json!({"event":"compaction","data":{
        "messages":[
            {"id":"agent:main:fixture:1","role":"user","text":"question","tool_call_id":null},
            {"id":"agent:main:fixture:2","role":"assistant","text":"working","tool_call_id":null},
            {"id":"agent:main:fixture:3","role":"system","text":"session fact","tool_call_id":null},
            {"id":"agent:main:fixture:4","role":"tool_call","text":"read","tool_call_id":"call-a"},
            {"id":"agent:main:fixture:5","role":"tool_result","text":"read result","tool_call_id":"call-a"},
            {"id":"agent:main:fixture:6","role":"tool_call","text":"write","tool_call_id":"call-b"},
            {"id":"agent:main:fixture:7","role":"tool_error","text":"write error","tool_call_id":"call-b"},
            {"id":"agent:main:fixture:8","role":"tool_call","text":"parked orphan","tool_call_id":"orphan"},
            {"id":"agent:main:fixture:9","role":"tool_result","text":"call outside span","tool_call_id":"outside"}
        ],
        "required_pairs":[pair("call-a","agent:main:fixture:5"),pair("call-b","agent:main:fixture:7")],
        "required_session_markers":["opaque::{TURN}|do not parse", " session-removed / reset:✓ "]
    }}))
}

fn requests() -> Vec<Value> {
    vec![
        envelope(
            json!({"event":"tool.pre","data":{"call_id":"call-a","tool":"file_read","input":{"path":"changed"},"original_input":{"path":"original"}}}),
        ),
        envelope(
            json!({"event":"tool.post","data":{"call_id":"call-a","tool":"file_read","result":{"text":"read result"}}}),
        ),
        envelope(
            json!({"event":"prompt","data":{"layers":[{"id":"system","text":"pinned","replaceability":"pinned"},{"id":"history","text":"replaceable","replaceability":"replaceable"}]}}),
        ),
        compact(),
        envelope(json!({"event":"continue","data":{"iteration":1,"limit":3}})),
        envelope(
            json!({"event":"ingest_filter","data":{"records":[{"id":"r1","content":{"text":"first"}},{"id":"r2","content":{"text":"second"}}]}}),
        ),
        envelope(
            json!({"event":"observer","data":{"lifecycle":{"turn_end":{"context":{"session_id":SESSION,"turn_id":TURN,"reason":"run-finished","outcome":"success"}}}}}),
        ),
    ]
}

fn responses() -> Vec<Value> {
    vec![
        json!({"event":"tool.pre","data":{"action":{"action":"continue","updated_input":{"path":"updated"}}}}),
        json!({"event":"tool.post","data":{"replacement":{"text":"filtered"}}}),
        json!({"event":"prompt","data":{"append":["appended"],"replace":[{"replaceable_layer_id":"history","text":"shortened"}]}}),
        json!({"event":"compaction","data":{"summary":{"text":"summary chosen by hook","preserved_pairs":[pair("call-b","agent:main:fixture:7"),pair("call-a","agent:main:fixture:5")],"session_markers":[" session-removed / reset:✓ ","opaque::{TURN}|do not parse"]}}}),
        json!({"event":"continue","data":{"request":true,"reason":"verify remaining work"}}),
        json!({"event":"ingest_filter","data":{"keep_ids":["r2"]}}),
        json!({"event":"observer","data":{"ack":{"records":["observed only"]}}}),
    ]
}

fn response_envelope(response: Value) -> Value {
    json!({"version":HOOK_CONTRACT_VERSION,"response":response})
}

fn request_ok(value: &Value) -> HookEnvelope {
    let text = value.to_string();
    let from_helper = decode_hook_v1(&text).unwrap();
    assert_eq!(
        serde_json::from_str::<HookEnvelope>(&text).unwrap(),
        from_helper
    );
    assert_eq!(
        serde_json::from_value::<HookEnvelope>(value.clone()).unwrap(),
        from_helper
    );
    assert_eq!(serde_json::to_value(&from_helper).unwrap(), *value);
    validate_envelope(&from_helper).unwrap();
    from_helper
}

fn request_bad(value: &Value) {
    let text = value.to_string();
    assert!(decode_hook_v1(&text).is_err(), "helper accepted {value}");
    assert!(
        serde_json::from_str::<HookEnvelope>(&text).is_err(),
        "from_str accepted {value}"
    );
    assert!(
        serde_json::from_value::<HookEnvelope>(value.clone()).is_err(),
        "from_value accepted {value}"
    );
}

fn response_ok(value: &Value) -> HookResponseEnvelope {
    let text = value.to_string();
    let from_helper = decode_hook_response_v1(&text).unwrap();
    assert_eq!(
        serde_json::from_str::<HookResponseEnvelope>(&text).unwrap(),
        from_helper
    );
    assert_eq!(
        serde_json::from_value::<HookResponseEnvelope>(value.clone()).unwrap(),
        from_helper
    );
    assert_eq!(serde_json::to_value(&from_helper).unwrap(), *value);
    from_helper
}

fn response_bad(value: &Value) {
    let text = value.to_string();
    assert!(
        decode_hook_response_v1(&text).is_err(),
        "helper accepted {value}"
    );
    assert!(
        serde_json::from_str::<HookResponseEnvelope>(&text).is_err(),
        "from_str accepted {value}"
    );
    assert!(
        serde_json::from_value::<HookResponseEnvelope>(value.clone()).is_err(),
        "from_value accepted {value}"
    );
}

fn input(value: &Value) -> HookInput {
    request_ok(value).event
}

fn response(value: Value) -> HookResponse {
    response_ok(&response_envelope(value)).response
}

#[test]
fn hook_v1_golden_all_events() {
    let tags = [
        HookEventTag::ToolPre,
        HookEventTag::ToolPost,
        HookEventTag::Prompt,
        HookEventTag::Compaction,
        HookEventTag::Continue,
        HookEventTag::IngestFilter,
        HookEventTag::Observer,
    ];
    let inputs = requests();
    let outputs = responses();
    assert_eq!(inputs.len(), 7);
    for (i, value) in inputs.iter().enumerate() {
        let decoded = request_ok(value);
        // Equality against the literal fixture above pins every field, including original_input.
        assert_eq!(decoded.version, HOOK_CONTRACT_VERSION);
        assert_eq!(decoded.session_id, SESSION);
        assert_eq!(decoded.turn_id.as_deref(), Some(TURN));
        assert_eq!(decoded.slot, "fixture.slot");
        assert_eq!(decoded.event.event_tag(), tags[i]);
        assert_eq!(
            serde_json::to_value(tags[i]).unwrap(),
            value["event"]["event"]
        );
        let output = response(outputs[i].clone());
        validate_response(&decoded.event, &output).unwrap();
    }
    for action in [
        json!({"action":"deny","reason":"denied by hook"}),
        json!({"action":"ask","reason":"approval requested"}),
    ] {
        let out = response(json!({"event":"tool.pre","data":{"action":action}}));
        validate_response(&input(&inputs[0]), &out).unwrap();
    }
}

fn lifecycles() -> Vec<Value> {
    vec![
        json!({"session_start":{"context":{"session_id":SESSION}}}),
        json!({"session_end":{"context":{"session_id":SESSION,"reason":"session-removed","outcome":"reset"}}}),
        json!({"turn_start":{"context":{"session_id":SESSION,"turn_id":TURN}}}),
        json!({"turn_end":{"context":{"session_id":SESSION,"turn_id":TURN,"reason":"run-finished","outcome":"success"}}}),
    ]
}

#[test]
fn hook_v1_lifecycle_matrix() {
    for (i, lifecycle) in lifecycles().into_iter().enumerate() {
        let mut value = envelope(json!({"event":"observer","data":{"lifecycle":lifecycle}}));
        if i < 2 {
            value["turn_id"] = Value::Null;
        }
        let good = request_ok(&value);
        assert_eq!(good.turn_id.is_some(), i >= 2);
        let mut bad = value.clone();
        bad["turn_id"] = if i < 2 { json!(TURN) } else { Value::Null };
        request_bad(&bad);
        if i >= 2 {
            bad["turn_id"] = json!("");
            request_bad(&bad);
        }
        let tag = value["event"]["data"]["lifecycle"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone();
        for session in ["", "different-session"] {
            let mut bad = value.clone();
            bad["event"]["data"]["lifecycle"][&tag]["context"]["session_id"] = json!(session);
            request_bad(&bad);
        }
        if i >= 2 {
            for turn in ["", "different-turn"] {
                let mut bad = value.clone();
                bad["event"]["data"]["lifecycle"][&tag]["context"]["turn_id"] = json!(turn);
                request_bad(&bad);
            }
        }
        if i == 1 || i == 3 {
            for field in ["reason", "outcome"] {
                let mut bad = value.clone();
                bad["event"]["data"]["lifecycle"][&tag]["context"][field] = json!("");
                request_bad(&bad);
                bad["event"]["data"]["lifecycle"][&tag]["context"]
                    .as_object_mut()
                    .unwrap()
                    .remove(field);
                request_bad(&bad);
            }
        }
        let mut bad_rust = good.clone();
        bad_rust.session_id.clear();
        assert!(validate_envelope(&bad_rust).is_err());
        bad_rust = good;
        bad_rust.version += 1;
        assert!(matches!(
            validate_envelope(&bad_rust),
            Err(HookWireError::UnsupportedVersion(_))
        ));
    }
    for value in requests().into_iter().take(6) {
        for field in ["session_id", "slot", "turn_id"] {
            let mut bad = value.clone();
            bad[field] = json!("");
            request_bad(&bad);
            bad.as_object_mut().unwrap().remove(field);
            request_bad(&bad);
        }
    }
}

#[test]
fn hook_v1_options_absent_or_null_serialize_null() {
    let mut start = envelope(json!({"event":"observer","data":{"lifecycle":lifecycles()[0]}}));
    start.as_object_mut().unwrap().remove("turn_id");
    let decoded: HookEnvelope = serde_json::from_value(start.clone()).unwrap();
    start["turn_id"] = Value::Null;
    assert_eq!(decode_hook_v1(&start.to_string()).unwrap(), decoded);
    assert_eq!(serde_json::to_value(decoded).unwrap(), start);
    let mut compact = compact();
    compact["event"]["data"]["messages"][0]
        .as_object_mut()
        .unwrap()
        .remove("tool_call_id");
    let decoded = decode_hook_v1(&compact.to_string()).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap()["event"]["data"]["messages"][0]["tool_call_id"],
        Value::Null
    );
    for (event, field) in [
        (
            json!({"event":"tool.pre","data":{"action":{"action":"continue"}}}),
            "/response/data/action/updated_input",
        ),
        (
            json!({"event":"tool.post","data":{}}),
            "/response/data/replacement",
        ),
        (
            json!({"event":"continue","data":{"request":false}}),
            "/response/data/reason",
        ),
    ] {
        let value = response_envelope(event);
        let decoded = decode_hook_response_v1(&value.to_string()).unwrap();
        let serialized = serde_json::to_value(decoded).unwrap();
        assert_eq!(serialized.pointer(field), Some(&Value::Null));
        response_ok(&serialized);
    }
}

// Visit all structural objects, excluding arbitrary business Value payloads.
fn object_paths(value: &Value, path: String, paths: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            paths.push(path.clone());
            for (key, child) in object {
                if [
                    "input",
                    "original_input",
                    "result",
                    "content",
                    "replacement",
                    "updated_input",
                ]
                .contains(&key.as_str())
                {
                    continue;
                }
                object_paths(child, format!("{path}/{key}"), paths);
            }
        }
        Value::Array(array) => {
            for (i, child) in array.iter().enumerate() {
                object_paths(child, format!("{path}/{i}"), paths);
            }
        }
        _ => {}
    }
}

#[test]
fn hook_v1_unknowns_reject() {
    for mut value in requests() {
        value["version"] = json!(HOOK_CONTRACT_VERSION + 1);
        request_bad(&value);
        assert_eq!(
            decode_hook_v1(&value.to_string()),
            Err(HookWireError::UnsupportedVersion(HOOK_CONTRACT_VERSION + 1))
        );
    }
    for output in responses() {
        let mut value = response_envelope(output);
        value["version"] = json!(HOOK_CONTRACT_VERSION + 1);
        response_bad(&value);
        assert_eq!(
            decode_hook_response_v1(&value.to_string()),
            Err(HookWireError::UnsupportedVersion(HOOK_CONTRACT_VERSION + 1))
        );
    }
    let mut all_requests = requests();
    for (i, lifecycle) in lifecycles().into_iter().enumerate() {
        let mut value = envelope(json!({"event":"observer","data":{"lifecycle":lifecycle}}));
        if i < 2 {
            value["turn_id"] = Value::Null;
        }
        all_requests.push(value);
    }
    for value in all_requests {
        let mut paths = Vec::new();
        object_paths(&value, String::new(), &mut paths);
        for path in paths {
            let mut bad = value.clone();
            bad.pointer_mut(&path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("future_control".into(), json!(true));
            request_bad(&bad);
        }
        let mut bad = value;
        bad["event"]["event"] = json!("future_event");
        request_bad(&bad);
    }
    let mut all_responses = responses();
    for action in [
        json!({"action":"deny","reason":"no"}),
        json!({"action":"ask","reason":"please"}),
    ] {
        all_responses.push(json!({"event":"tool.pre","data":{"action":action}}));
    }
    for output in all_responses {
        let value = response_envelope(output);
        let mut paths = Vec::new();
        object_paths(&value, String::new(), &mut paths);
        for path in paths {
            let mut bad = value.clone();
            bad.pointer_mut(&path)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("metadata".into(), json!({"lease":"forged"}));
            response_bad(&bad);
            let body = &bad["response"];
            if !path.is_empty() {
                assert!(serde_json::from_value::<HookResponse>(body.clone()).is_err());
                assert!(serde_json::from_str::<HookResponse>(&body.to_string()).is_err());
            }
        }
        let mut bad = value;
        bad["response"]["event"] = json!("future_event");
        response_bad(&bad);
    }
    let mut bad = requests()[0].clone();
    bad["event"]["data"]
        .as_object_mut()
        .unwrap()
        .remove("original_input");
    request_bad(&bad);
    bad = requests()[6].clone();
    bad["event"]["data"]["lifecycle"] =
        json!({"future_lifecycle":{"context":{"session_id":SESSION}}});
    request_bad(&bad);
    bad = requests()[2].clone();
    bad["event"]["data"]["layers"][0]["replaceability"] = json!("future");
    request_bad(&bad);
    response_bad(&response_envelope(
        json!({"event":"tool.pre","data":{"action":{"action":"future"}}}),
    ));
}

#[test]
fn hook_v1_business_json_survives() {
    let business = json!({"custom":{"x":1},"metadata":{"served_by":"business","backend":{"generation":9,"lease":"business"},"safety":[null,true,3,{"event":"arbitrary","version":99}]}});
    for (i, fields) in [
        (0, vec!["input", "original_input"]),
        (1, vec!["result"]),
        (5, vec!["content"]),
    ] {
        let mut value = requests()[i].clone();
        for field in fields {
            if i == 5 {
                value["event"]["data"]["records"][0][field] = business.clone();
            } else {
                value["event"]["data"][field] = business.clone();
            }
        }
        let decoded = request_ok(&value);
        match decoded.event {
            HookInput::ToolPre {
                input,
                original_input,
                ..
            } => {
                assert_eq!(input, business);
                assert_eq!(original_input, business);
            }
            HookInput::ToolPost { result, .. } => assert_eq!(result, business),
            HookInput::IngestFilter { records } => assert_eq!(records[0].content, business),
            _ => unreachable!(),
        }
    }
    for output in [
        json!({"event":"tool.pre","data":{"action":{"action":"continue","updated_input":business}}}),
        json!({"event":"tool.post","data":{"replacement":business}}),
    ] {
        response_ok(&response_envelope(output));
    }
    let decoded_input = match input(&envelope(
        json!({"event":"tool.pre","data":{"call_id":"c","tool":"t","input":{"custom":{"x":1}},"original_input":null}}),
    )) {
        HookInput::ToolPre { input, .. } => input,
        _ => unreachable!(),
    };
    assert_eq!(decoded_input, json!({"custom":{"x":1}}));
}

#[test]
fn compaction_facts_reject_missing_forged_duplicate() {
    let original = compact();
    let good_input = input(&original);
    let good_output = response(responses()[3].clone());
    validate_response(&good_input, &good_output).unwrap();
    for pairs in [
        json!([]),
        json!([pair("call-a", "agent:main:fixture:5")]),
        json!([
            pair("call-a", "agent:main:fixture:5"),
            pair("call-a", "agent:main:fixture:5")
        ]),
        json!([
            pair("call-a", "agent:main:fixture:5"),
            pair("call-b", "agent:main:fixture:5")
        ]),
        json!([
            pair("call-a", "agent:main:fixture:5"),
            pair("call-b", "unknown")
        ]),
        json!([
            pair("call-a", "agent:main:fixture:5"),
            pair("unknown", "agent:main:fixture:7")
        ]),
        json!([
            pair("call-a", "agent:main:fixture:7"),
            pair("call-b", "agent:main:fixture:5")
        ]),
        json!([
            pair("call-a", "agent:main:fixture:4"),
            pair("call-b", "agent:main:fixture:7")
        ]),
        json!([
            pair("call-a", "agent:main:fixture:5"),
            pair("call-b", "agent:main:fixture:7"),
            pair("orphan", "agent:main:fixture:8")
        ]),
        json!([
            pair("call-a", "agent:main:fixture:5"),
            pair("call-b", "agent:main:fixture:7"),
            pair("outside", "agent:main:fixture:9")
        ]),
    ] {
        let mut bad = original.clone();
        bad["event"]["data"]["required_pairs"] = pairs.clone();
        request_bad(&bad);
        let bad_input: HookInput = serde_json::from_value(bad["event"].clone()).unwrap();
        assert!(validate_response(&bad_input, &good_output).is_err());
        let mut bad_output = responses()[3].clone();
        bad_output["data"]["summary"]["preserved_pairs"] = pairs;
        assert!(validate_response(&good_input, &response(bad_output)).is_err());
    }
    for i in [0, 3, 4, 6] {
        let mut bad = original.clone();
        let row = bad["event"]["data"]["messages"][i].clone();
        bad["event"]["data"]["messages"]
            .as_array_mut()
            .unwrap()
            .push(row);
        request_bad(&bad);
    }
    // Distinct row IDs must not hide ambiguous call or result correlations.
    for i in [3, 4, 6] {
        let mut bad = original.clone();
        let mut row = bad["event"]["data"]["messages"][i].clone();
        row["id"] = json!("agent:main:fixture:99");
        bad["event"]["data"]["messages"]
            .as_array_mut()
            .unwrap()
            .push(row);
        request_bad(&bad);
    }
    for role in ["tool", "future", ""] {
        let mut bad = original.clone();
        bad["event"]["data"]["messages"][0]["role"] = json!(role);
        request_bad(&bad);
    }
    for call in [Value::Null, json!("")] {
        let mut bad = original.clone();
        bad["event"]["data"]["messages"][3]["tool_call_id"] = call;
        request_bad(&bad);
    }
    let mut no_results = original.clone();
    no_results["event"]["data"]["messages"]
        .as_array_mut()
        .unwrap()
        .retain(|r| !["tool_result", "tool_error"].contains(&r["role"].as_str().unwrap()));
    no_results["event"]["data"]["required_pairs"] = json!([]);
    request_ok(&no_results); // Actual orphan calls yield no manufactured pairs.
    let mut more = original.clone();
    more["event"]["data"]["messages"].as_array_mut().unwrap().extend([
        json!({"id":"agent:main:fixture:10","role":"tool_call","text":"extra","tool_call_id":"c"}),
        json!({"id":"agent:main:fixture:11","role":"tool_result","text":"extra result","tool_call_id":"c"}),
    ]);
    request_bad(&more); // Declared complete set omitted a third real pair.
    more["event"]["data"]["required_pairs"]
        .as_array_mut()
        .unwrap()
        .push(pair("c", "agent:main:fixture:11"));
    request_ok(&more);
}

#[test]
fn opaque_markers_preserve_exact_complete_set() {
    let original = compact();
    let good = input(&original);
    let markers = &original["event"]["data"]["required_session_markers"];
    for bad_markers in [
        json!([]),
        json!([markers[0]]),
        json!([markers[0], markers[0]]),
        json!([markers[0], "forged"]),
        json!([markers[0], "session-removed / reset:✓"]),
    ] {
        let mut out = responses()[3].clone();
        out["data"]["summary"]["session_markers"] = bad_markers;
        assert!(validate_response(&good, &response(out)).is_err());
    }
    let mut bad = original.clone();
    bad["event"]["data"]["required_session_markers"] = json!([markers[0], markers[0]]);
    request_bad(&bad);
    // Opaque means even empty strings are exact facts, not parsed IDs.
    let mut with_empty = original;
    with_empty["event"]["data"]["required_session_markers"] = json!([""]);
    let mut out = responses()[3].clone();
    out["data"]["summary"]["session_markers"] = json!([""]);
    validate_response(&input(&with_empty), &response(out)).unwrap();
}

#[test]
fn prompt_replace_ids_reject() {
    let good = input(&requests()[2]);
    validate_response(&good, &response(responses()[2].clone())).unwrap();
    for ids in [vec!["system"], vec!["unknown"], vec!["history", "history"]] {
        let replace: Vec<_> = ids
            .into_iter()
            .map(|id| json!({"replaceable_layer_id":id,"text":"changed"}))
            .collect();
        assert!(validate_response(
            &good,
            &response(json!({"event":"prompt","data":{"append":["text"],"replace":replace}}))
        )
        .is_err());
    }
    let mut bad = requests()[2].clone();
    bad["event"]["data"]["layers"][1]["id"] = json!("system");
    request_bad(&bad);
}

#[test]
fn prompt_ambiguous_input_ids_fail_closed_in_both_orders() {
    let output = HookResponse::Prompt {
        append: vec!["additional context".into()],
        replace: vec![ReplaceLayerRequest {
            replaceable_layer_id: "shared".into(),
            text: "replacement".into(),
        }],
    };
    for replaceabilities in [
        [Replaceability::Pinned, Replaceability::Replaceable],
        [Replaceability::Replaceable, Replaceability::Pinned],
    ] {
        let layers = replaceabilities
            .into_iter()
            .map(|replaceability| PromptLayerView {
                id: "shared".into(),
                text: "same arbitrary text in either layer".into(),
                replaceability,
            })
            .collect();
        // Exercise direct Rust input: decode must not be the only duplicate guard.
        let ambiguous = HookInput::Prompt { layers };
        assert_eq!(
            validate_response(&ambiguous, &output),
            Err(HookWireError::Malformed("duplicate layer id".into()))
        );
        request_bad(&envelope(serde_json::to_value(&ambiguous).unwrap()));
    }
    // Pinnedness comes only from replaceability, never the layer's text.
    let mut layer = PromptLayerView {
        id: "shared".into(),
        text: "Pinned system policy".into(),
        replaceability: Replaceability::Replaceable,
    };
    validate_response(
        &HookInput::Prompt {
            layers: vec![layer.clone()],
        },
        &output,
    )
    .unwrap();
    layer.text = "ordinary replaceable context".into();
    layer.replaceability = Replaceability::Pinned;
    assert!(matches!(
        validate_response(
            &HookInput::Prompt {
                layers: vec![layer]
            },
            &output
        ),
        Err(HookWireError::InvalidResponse(_))
    ));
}

#[test]
fn ingest_subset_unique_and_continue_limit() {
    let ingest = input(&requests()[5]);
    for ids in [json!([]), json!(["r1"]), json!(["r2", "r1"])] {
        validate_response(
            &ingest,
            &response(json!({"event":"ingest_filter","data":{"keep_ids":ids}})),
        )
        .unwrap();
    }
    for ids in [json!(["unknown"]), json!(["r1", "r1"])] {
        assert!(validate_response(
            &ingest,
            &response(json!({"event":"ingest_filter","data":{"keep_ids":ids}}))
        )
        .is_err());
    }
    let mut bad = requests()[5].clone();
    bad["event"]["data"]["records"][1]["id"] = json!("r1");
    request_bad(&bad);
    for iteration in [2, 3, 4] {
        let event = HookInput::Continue {
            iteration,
            limit: 3,
        };
        assert_eq!(
            validate_response(
                &event,
                &HookResponse::Continue {
                    request: true,
                    reason: Some("more".into())
                }
            )
            .is_ok(),
            iteration < 3
        );
        validate_response(
            &event,
            &HookResponse::Continue {
                request: false,
                reason: None,
            },
        )
        .unwrap();
    }
    assert!(validate_response(
        &HookInput::Continue {
            iteration: 0,
            limit: 0
        },
        &HookResponse::Continue {
            request: true,
            reason: None
        }
    )
    .is_err());
}

#[test]
fn hook_v1_effect_boundary() {
    let ins: Vec<_> = requests().iter().map(input).collect();
    let outs: Vec<_> = responses().into_iter().map(response).collect();
    for (i, request) in ins.iter().enumerate() {
        for (j, output) in outs.iter().enumerate() {
            assert_eq!(
                validate_response(request, output).is_ok(),
                i == j,
                "event boundary {i}/{j}"
            );
        }
    }
    for field in [
        "metadata",
        "served_by",
        "backend",
        "generation",
        "lease",
        "safety",
        "control",
        "continue",
    ] {
        let mut out = responses()[6].clone();
        out["data"]["ack"][field] = json!(true);
        response_bad(&response_envelope(out));
        let mut out = responses()[1].clone();
        out["data"][field] = json!(true);
        response_bad(&response_envelope(out));
    }
}
