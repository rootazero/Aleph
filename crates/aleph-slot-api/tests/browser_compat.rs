use aleph_slot_api::browser::BrowserManagerContract;

fn accepts_browser_manager_contract(_: &dyn BrowserManagerContract) {}

#[test]
fn browser_manager_contract_is_public() {
    let _consumer: fn(&dyn BrowserManagerContract) = accepts_browser_manager_contract;
}

#[path = "support/browser_inventory.rs"]
mod browser_inventory;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use aleph_slot_api::browser::*;
use serde_json::{json, Value};

#[test]
fn browser_schema_inventory_complete() {
    let (native, rows) = browser_inventory::census();
    assert_eq!(
        native.len(),
        39,
        "38 async methods plus the synchronous getter"
    );
    assert_eq!(rows.len(), native.len());
    let features = BrowserFeature::ALL;
    assert_eq!(features.len(), rows.len(), "one emitted feature per row");
    let mut emitted = BTreeMap::new();
    let mut tags = BTreeSet::new();
    for feature in features {
        assert!(emitted
            .insert(feature.method_name(), format!("{feature:?}"))
            .is_none());
        assert!(tags.insert(
            serde_json::to_value(feature)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        ));
    }
    assert_eq!(
        emitted.keys().copied().collect::<BTreeSet<_>>(),
        native.keys().map(String::as_str).collect()
    );
    for row in &rows {
        assert_eq!(emitted[row.method.as_str()], row.variant);
    }
    // The typed fixtures below consume BOTH generated enums; a missing or
    // duplicated operation/reply arm cannot hide behind a feature-only census.
    let fixtures = browser_fixtures();
    assert_eq!(fixtures.len(), rows.len());
    let mut operations = BTreeSet::new();
    let mut replies = BTreeSet::new();
    for fixture in fixtures {
        assert!(operations.insert(fixture.operation.feature().method_name()));
        assert!(replies.insert(fixture.reply.feature().method_name()));
    }
    let expected: BTreeSet<_> = native.keys().map(String::as_str).collect();
    assert_eq!(operations, expected);
    assert_eq!(replies, expected);
}

struct Fixture {
    operation: BrowserOperation,
    reply: BrowserReply,
    operation_json: Value,
    reply_json: Value,
}

// Literal fixtures are test data, never the inventory authority. This macro
// only packages typed values and independently written JSON payloads.
macro_rules! fixture {
    ($variant:ident { $($field:ident: $input:expr),* $(,)? } => $tag:literal,
        $fields:expr; $value:expr => $($golden:tt)+) => {{
        let mut operation_json = $fields;
        operation_json.as_object_mut().unwrap().insert("type".into(), json!($tag));
        Fixture {
            operation: BrowserOperation::$variant { $($field: $input),* },
            reply: BrowserReply::$variant { value: $value },
            operation_json,
            reply_json: json!({"type": $tag, "value": $($golden)+}),
        }
    }};
}

fn ref_target() -> ActionTarget {
    ActionTarget::Ref {
        ref_id: "e42-雪".into(),
    }
}

fn coordinate_target() -> ActionTarget {
    ActionTarget::Coordinates { x: 12.5, y: -3.25 }
}

fn snapshot_fixture() -> SnapshotOutput {
    SnapshotOutput {
        snapshot_text: "Page 雪\n[ref=e42] button Save".into(),
        page_url: Some("https://example.test/雪?q=1".into()),
        page_title: Some("Title 雪".into()),
        ref_count: 7,
        state_json: Some(
            json!({"nodes": [{"ref": "e42", "checked": false, "x": 12.5}], "revision": 9}),
        ),
    }
}

fn snapshot_golden() -> Value {
    json!({
        "snapshot_text": "Page 雪\n[ref=e42] button Save",
        "page_url": "https://example.test/雪?q=1", "page_title": "Title 雪", "ref_count": 7,
        "state_json": {"nodes": [{"ref": "e42", "checked": false, "x": 12.5}], "revision": 9}
    })
}

fn route_info_fixture() -> RouteRuleInfo {
    RouteRuleInfo {
        id: "route-17".into(),
        url_contains: "/api/雪".into(),
        method: Some("PATCH".into()),
        kind_label: "mock",
        scope: RouteScope::Profile,
        hits: 23,
        active: false,
        note: Some("retired after hits 雪".into()),
    }
}

fn route_info_golden() -> Value {
    json!({"id":"route-17", "url_contains":"/api/雪", "method":"PATCH", "kind_label":"mock",
        "scope":"profile", "hits":23, "active":false, "note":"retired after hits 雪"})
}

fn recording_status_fixture() -> RecordingStatus {
    RecordingStatus {
        recording_id: "rec-雪-3".into(),
        path: PathBuf::from("/tmp/session 雪.mp4"),
        elapsed_ms: 2345,
        captured_frames: 17,
        dropped_frames: 3,
        ffmpeg_source: "ffmpeg on PATH".into(),
    }
}

fn recording_status_golden() -> Value {
    json!({"recording_id":"rec-雪-3", "path":"/tmp/session 雪.mp4", "elapsed_ms":2345,
        "captured_frames":17, "dropped_frames":3, "ffmpeg_source":"ffmpeg on PATH"})
}

fn browser_fixtures() -> Vec<Fixture> {
    vec![
        fixture!(OpenTab { url: "https://example.test/new?雪=1".into() } => "open_tab",
            json!({"url":"https://example.test/new?雪=1"}); "new-tab-17".into() => "new-tab-17"),
        fixture!(CloseTab { tab_id: "close-17".into() } => "close_tab",
            json!({"tab_id":"close-17"}); () => null),
        fixture!(ListTabs {} => "list_tabs", json!({});
            vec![TabLine { id:"tab-a".into(), url:"https://example.test/a".into(), selected:true },
                TabLine { id:"tab-b".into(), url:"https://example.test/b".into(), selected:false }]
            => [{"id":"tab-a","url":"https://example.test/a","selected":true},
                {"id":"tab-b","url":"https://example.test/b","selected":false}]),
        fixture!(Navigate { tab_id:"tab-7".into(), url:"https://example.test/next?q=雪".into() } => "navigate",
            json!({"tab_id":"tab-7","url":"https://example.test/next?q=雪"}); () => null),
        fixture!(Click { tab_id:"tab-7".into(), target:ref_target() } => "click",
            json!({"tab_id":"tab-7","target":{"type":"ref","ref_id":"e42-雪"}}); () => null),
        fixture!(TypeText { tab_id:"tab-7".into(), target:coordinate_target(), text:"line1\n\"雪\"".into() } => "type_text",
            json!({"tab_id":"tab-7","target":{"type":"coordinates","x":12.5,"y":-3.25},"text":"line1\n\"雪\""}); () => null),
        fixture!(Fill { tab_id:"tab-7".into(), target:ref_target(), value:"value 雪\n2".into() } => "fill",
            json!({"tab_id":"tab-7","target":{"type":"ref","ref_id":"e42-雪"},"value":"value 雪\n2"}); () => null),
        fixture!(Hover { tab_id:"tab-7".into(), target:coordinate_target() } => "hover",
            json!({"tab_id":"tab-7","target":{"type":"coordinates","x":12.5,"y":-3.25}}); () => null),
        fixture!(Scroll { tab_id:"tab-7".into(), target:ref_target(), direction:ScrollDirection::Left } => "scroll",
            json!({"tab_id":"tab-7","target":{"type":"ref","ref_id":"e42-雪"},"direction":"left"}); () => null),
        fixture!(Screenshot { tab_id:"tab-7".into(), opts:ScreenshotOpts { full_page:true, format:"jpeg".into() } } => "screenshot",
            json!({"tab_id":"tab-7","opts":{"full_page":true,"format":"jpeg"}});
            ScreenshotOutput { png_bytes:vec![137,80,78,71,13,10,26,10,0,255,42] } => {"png_bytes":[137,80,78,71,13,10,26,10,0,255,42]}),
        fixture!(Snapshot { tab_id:"tab-7".into() } => "snapshot", json!({"tab_id":"tab-7"});
            snapshot_fixture() => snapshot_golden()),
        fixture!(SnapshotPresented { tab_id:"tab-7".into(), _max_chars:37 } => "snapshot_presented",
            json!({"tab_id":"tab-7","_max_chars":37});
            PresentedSnapshot { snap:snapshot_fixture(), text:"bounded 雪…".into(), truncated:true,
                full_text:Some("Page 雪\n[ref=e42] button Save\nfull retained".into()) }
            => {"snap":snapshot_golden(),"text":"bounded 雪…","truncated":true,
                "full_text":"Page 雪\n[ref=e42] button Save\nfull retained"}),
        fixture!(Evaluate { tab_id:"tab-7".into(), js:"({text:'雪', n:2}).text".into() } => "evaluate",
            json!({"tab_id":"tab-7","js":"({text:'雪', n:2}).text"}); "result 雪\n42".into() => "result 雪\n42"),
        fixture!(Select { tab_id:"tab-7".into(), target:ref_target(), value:"option-雪".into() } => "select",
            json!({"tab_id":"tab-7","target":{"type":"ref","ref_id":"e42-雪"},"value":"option-雪"}); () => null),
        fixture!(PressKey { tab_id:"tab-7".into(), key:"Control+Shift+K".into() } => "press_key",
            json!({"tab_id":"tab-7","key":"Control+Shift+K"}); () => null),
        fixture!(History { tab_id:"tab-7".into(), nav:HistoryNav::Forward } => "history",
            json!({"tab_id":"tab-7","nav":"forward"}); () => null),
        fixture!(Dblclick { tab_id:"tab-7".into(), target:coordinate_target() } => "dblclick",
            json!({"tab_id":"tab-7","target":{"type":"coordinates","x":12.5,"y":-3.25}}); () => null),
        fixture!(WaitFor { tab_id:"tab-7".into(), condition:WaitCondition::SelectorGone(".loading 雪".into()), timeout_ms:1234 } => "wait_for",
            json!({"tab_id":"tab-7","condition":{"type":"selector_gone","value":".loading 雪"},"timeout_ms":1234}); false => false),
        fixture!(ConsoleMessages { tab_id:"tab-7".into() } => "console_messages", json!({"tab_id":"tab-7"});
            "[warn] 雪\nsecond message".into() => "[warn] 雪\nsecond message"),
        fixture!(NetworkLog { tab_id:"tab-7".into() } => "network_log", json!({"tab_id":"tab-7"});
            "PATCH /api/雪 -> 503\nGET /asset -> 200".into() => "PATCH /api/雪 -> 503\nGET /asset -> 200"),
        fixture!(RouteAdd { tab_id:"tab-7".into(), rule:NewRouteRule {
            url_contains:"/api/雪".into(), method:Some("PATCH".into()),
            kind:RouteKind::Mock { status:207, headers:vec![("X-Test".into(),"雪-value".into()), ("Content-Type".into(),"application/octet-stream".into())],
                body:vec![0,255,13,10,42] }, scope:RouteScope::Profile, note:Some("mock note 雪".into()) } } => "route_add",
            json!({"tab_id":"tab-7","rule":{"url_contains":"/api/雪","method":"PATCH","kind":{"type":"mock","status":207,
                "headers":[["X-Test","雪-value"],["Content-Type","application/octet-stream"]],"body":[0,255,13,10,42]},
                "scope":"profile","note":"mock note 雪"}}); route_info_fixture() => route_info_golden()),
        fixture!(RouteList {} => "route_list", json!({}); vec![route_info_fixture(), RouteRuleInfo {
            id:"route-abort".into(), url_contains:"/deny".into(), method:Some("DELETE".into()), kind_label:"abort",
            scope:RouteScope::Tab, hits:9, active:true, note:Some("abort note".into()) }]
            => [route_info_golden(), {"id":"route-abort","url_contains":"/deny","method":"DELETE","kind_label":"abort",
                "scope":"tab","hits":9,"active":true,"note":"abort note"}]),
        fixture!(RouteRemove { rule_id:"route-17".into() } => "route_remove", json!({"rule_id":"route-17"});
            route_info_fixture() => route_info_golden()),
        fixture!(RouteClear { tab_id:"tab-7".into(), scope:RouteScope::Tab } => "route_clear",
            json!({"tab_id":"tab-7","scope":"tab"}); 5 => 5),
        fixture!(RecordStart { tab_id:"tab-7".into(), options:RecordStartOptions {
            path:Some(PathBuf::from("/tmp/session 雪.mp4")), fps_cap:23, format:RecordingFormat::Mp4, quality:67, ffmpeg:None } } => "record_start",
            json!({"tab_id":"tab-7","options":{"path":"/tmp/session 雪.mp4","fps_cap":23,"format":"mp4","quality":67}});
            recording_status_fixture() => recording_status_golden()),
        fixture!(RecordStop { tab_id:"tab-7".into(), recording_id:None } => "record_stop",
            json!({"tab_id":"tab-7","recording_id":null}); RecordingReceipt {
                recording_id:"rec-雪-3".into(), path:PathBuf::from("/tmp/session 雪.mp4"), duration_ms:6789,
                captured_frames:101, encoded_frames:93, dropped_frames:8, encoder_exit:"signal 9".into(),
                size_bytes:654321, complete:false, truncation_reason:Some("frame loss 雪".into()) }
            => {"recording_id":"rec-雪-3","path":"/tmp/session 雪.mp4","duration_ms":6789,"captured_frames":101,
                "encoded_frames":93,"dropped_frames":8,"encoder_exit":"signal 9","size_bytes":654321,"complete":false,
                "truncation_reason":"frame loss 雪"}),
        fixture!(RecordStatus { tab_id:"tab-7".into() } => "record_status", json!({"tab_id":"tab-7"}); None => null),
        fixture!(Pdf { _tab_id:"pdf-tab".into(), _output_path:PathBuf::from("/tmp/report 雪.pdf") } => "pdf",
            json!({"_tab_id":"pdf-tab","_output_path":"/tmp/report 雪.pdf"}); () => null),
        fixture!(SwitchTab { tab_id:"tab-a".into() } => "switch_tab", json!({"tab_id":"tab-a"}); () => null),
        fixture!(HandleDialog { tab_id:"tab-7".into(), action:"raw-action:accept 雪".into(), prompt_text:Some("prompt 雪\nanswer".into()) } => "handle_dialog",
            json!({"tab_id":"tab-7","action":"raw-action:accept 雪","prompt_text":"prompt 雪\nanswer"}); () => null),
        fixture!(Drag { tab_id:"tab-7".into(), from:ref_target(), to:coordinate_target() } => "drag",
            json!({"tab_id":"tab-7","from":{"type":"ref","ref_id":"e42-雪"},"to":{"type":"coordinates","x":12.5,"y":-3.25}}); () => null),
        fixture!(Upload { tab_id:"tab-7".into(), target:Some(ref_target()), paths:vec!["/tmp/file 雪.txt".into(),"/tmp/other.bin".into()] } => "upload",
            json!({"tab_id":"tab-7","target":{"type":"ref","ref_id":"e42-雪"},"paths":["/tmp/file 雪.txt","/tmp/other.bin"]}); () => null),
        fixture!(Resize { tab_id:"tab-7".into(), width:1365, height:769 } => "resize",
            json!({"tab_id":"tab-7","width":1365,"height":769}); () => null),
        fixture!(Emulate { tab_id:"tab-7".into(), opts:EmulateOptions {
            color_scheme:Some(ColorScheme::Dark), geolocation:Some(Geolocation { latitude:35.125, longitude:-120.75 }),
            network_condition:Some(NetworkCondition::Slow3g), cpu_throttle:Some(3.5),
            extra_http_headers:Some(BTreeMap::from([("X-Test".into(),"header 雪".into()),("X-Other".into(),"two".into())])),
            user_agent:Some("Aleph/test 雪".into()) } } => "emulate",
            json!({"tab_id":"tab-7","opts":{"color_scheme":"dark","geolocation":{"latitude":35.125,"longitude":-120.75},
                "network_condition":"slow3g","cpu_throttle":3.5,"extra_http_headers":{"X-Test":"header 雪","X-Other":"two"},
                "user_agent":"Aleph/test 雪"}}); () => null),
        fixture!(SaveState { _path:PathBuf::from("/tmp/saved 雪.json") } => "save_state",
            json!({"_path":"/tmp/saved 雪.json"}); () => null),
        fixture!(LoadState { _path:PathBuf::from("/tmp/restore 雪.json") } => "load_state",
            json!({"_path":"/tmp/restore 雪.json"}); () => null),
        fixture!(Cookies { _op:CookieOp::Set { name:"session 雪".into(), value:"value=雪".into(), domain:Some("example.test".into()),
            path:Some("/account".into()), expires:Some(1_750_000_123), http_only:Some(true), secure:Some(false), same_site:Some(SameSite::Lax) } } => "cookies",
            json!({"_op":{"type":"set","name":"session 雪","value":"value=雪","domain":"example.test","path":"/account", "expires":1750000123,
                "http_only":true,"secure":false,"same_site":"lax"}}); "stored session 雪".into() => "stored session 雪"),
        fixture!(FillForm { tab_id:"tab-7".into(), fields:vec![(ref_target(),"first 雪".into()),(coordinate_target(),"second\nvalue".into())] } => "fill_form",
            json!({"tab_id":"tab-7","fields":[[{"type":"ref","ref_id":"e42-雪"},"first 雪"],
                [{"type":"coordinates","x":12.5,"y":-3.25},"second\nvalue"]]}); 2 => 2),
        fixture!(LastEffectVerification {} => "last_effect_verification", json!({});
            Some(EffectVerification::Skipped(Engine::Chromium)) => {"state":"skipped","engine":"chromium"}),
    ]
}

#[test]
fn browser_all_delegates_map_losslessly() {
    let (native, _) = browser_inventory::census();
    let fixtures = browser_fixtures();
    assert_eq!(fixtures.len(), 39);
    let mut seen = BTreeSet::new();
    for fixture in fixtures {
        let method = fixture.operation.feature().method_name();
        assert!(seen.insert(method), "duplicate fixture {method}");
        assert!(
            native.contains_key(method),
            "fixture outside native source {method}"
        );
        assert_eq!(fixture.operation.feature(), fixture.reply.feature());
        validate_browser_reply(&fixture.operation, &fixture.reply).unwrap();
        let operation_json = serde_json::to_value(&fixture.operation).unwrap();
        let reply_json = serde_json::to_value(&fixture.reply).unwrap();
        assert_eq!(
            operation_json, fixture.operation_json,
            "operation golden {method}"
        );
        assert_eq!(reply_json, fixture.reply_json, "reply golden {method}");
        assert_eq!(
            operation_json["type"],
            serde_json::to_value(fixture.operation.feature()).unwrap()
        );
        let operation: BrowserOperation = serde_json::from_value(operation_json.clone()).unwrap();
        let reply: BrowserReply = serde_json::from_value(reply_json.clone()).unwrap();
        validate_browser_reply(&operation, &reply).unwrap();
        assert_eq!(
            serde_json::to_value(&operation).unwrap(),
            operation_json,
            "operation roundtrip {method}"
        );
        assert_eq!(
            serde_json::to_value(&reply).unwrap(),
            reply_json,
            "reply roundtrip {method}"
        );
        assert_typed_values(&operation, &reply);
    }
    assert_eq!(seen, native.keys().map(String::as_str).collect());
}

fn assert_typed_values(operation: &BrowserOperation, reply: &BrowserReply) {
    match (operation, reply) {
        (BrowserOperation::OpenTab { url }, BrowserReply::OpenTab { value }) => {
            assert_eq!(url, "https://example.test/new?雪=1");
            let _: &TabId = value;
            assert_eq!(value, "new-tab-17");
        }
        (_, BrowserReply::ListTabs { value }) => {
            assert_eq!(value.len(), 2);
            assert!(value[0].selected);
            assert!(!value[1].selected);
            assert_eq!(value[0].id, "tab-a");
            assert_eq!(value[1].url, "https://example.test/b");
        }
        (
            BrowserOperation::Click {
                target: ActionTarget::Ref { ref_id },
                ..
            },
            _,
        ) => assert_eq!(ref_id, "e42-雪"),
        (
            BrowserOperation::TypeText {
                target: ActionTarget::Coordinates { x, y },
                text,
                ..
            },
            _,
        ) => {
            assert_eq!((*x, *y), (12.5, -3.25));
            assert_eq!(text, "line1\n\"雪\"");
        }
        (BrowserOperation::Screenshot { opts, .. }, BrowserReply::Screenshot { value }) => {
            assert!(opts.full_page);
            assert_eq!(opts.format, "jpeg");
            assert_eq!(
                value.png_bytes,
                [137, 80, 78, 71, 13, 10, 26, 10, 0, 255, 42]
            );
        }
        (_, BrowserReply::Snapshot { value }) => {
            assert_eq!(value.ref_count, 7);
            assert_eq!(value.page_title.as_deref(), Some("Title 雪"));
            assert_eq!(
                value.state_json.as_ref().unwrap()["nodes"][0]["checked"],
                false
            );
        }
        (
            BrowserOperation::SnapshotPresented { _max_chars, .. },
            BrowserReply::SnapshotPresented { value },
        ) => {
            assert_eq!(*_max_chars, 37);
            assert!(value.truncated);
            assert_eq!(value.text, "bounded 雪…");
            assert_eq!(
                value.full_text.as_deref(),
                Some("Page 雪\n[ref=e42] button Save\nfull retained")
            );
            assert_eq!(value.snap.state_json.as_ref().unwrap()["revision"], 9);
        }
        (BrowserOperation::Evaluate { js, .. }, BrowserReply::Evaluate { value }) => {
            assert_eq!(js, "({text:'雪', n:2}).text");
            assert_eq!(value, "result 雪\n42");
        }
        (
            BrowserOperation::WaitFor {
                condition,
                timeout_ms,
                ..
            },
            BrowserReply::WaitFor { value },
        ) => {
            assert_eq!(
                condition,
                &WaitCondition::SelectorGone(".loading 雪".into())
            );
            assert_eq!(*timeout_ms, 1234);
            assert!(!value);
        }
        (_, BrowserReply::ConsoleMessages { value }) => {
            assert_eq!(value, "[warn] 雪\nsecond message")
        }
        (_, BrowserReply::NetworkLog { value }) => {
            assert_eq!(value, "PATCH /api/雪 -> 503\nGET /asset -> 200")
        }
        (BrowserOperation::RouteAdd { rule, .. }, BrowserReply::RouteAdd { value }) => {
            assert_eq!(rule.scope, RouteScope::Profile);
            assert_eq!(rule.note.as_deref(), Some("mock note 雪"));
            let RouteKind::Mock {
                status,
                headers,
                body,
            } = &rule.kind
            else {
                panic!("mock material lost")
            };
            assert_eq!(*status, 207);
            assert_eq!(headers[0], ("X-Test".into(), "雪-value".into()));
            assert_eq!(body, &[0, 255, 13, 10, 42]);
            assert_eq!(value, &route_info_fixture());
        }
        (_, BrowserReply::RouteList { value }) => {
            assert_eq!(value[0], route_info_fixture());
            assert_eq!(value[1].kind_label, "abort");
            assert_eq!(value[1].hits, 9);
            assert!(value[1].active);
        }
        (_, BrowserReply::RouteRemove { value }) => {
            assert_eq!(value.hits, 23);
            assert!(!value.active);
        }
        (_, BrowserReply::RouteClear { value }) => assert_eq!(*value, 5),
        (BrowserOperation::RecordStart { options, .. }, BrowserReply::RecordStart { value }) => {
            assert_eq!(
                options.path.as_deref(),
                Some(std::path::Path::new("/tmp/session 雪.mp4"))
            );
            assert_eq!((options.fps_cap, options.quality), (23, 67));
            assert_eq!(options.format, RecordingFormat::Mp4);
            assert!(options.ffmpeg.is_none());
            assert_eq!(value, &recording_status_fixture());
        }
        (BrowserOperation::RecordStop { recording_id, .. }, BrowserReply::RecordStop { value }) => {
            assert!(recording_id.is_none());
            assert!(!value.complete);
            assert_eq!(value.truncation_reason.as_deref(), Some("frame loss 雪"));
            assert_eq!((value.duration_ms, value.size_bytes), (6789, 654321));
            assert_eq!(
                (
                    value.captured_frames,
                    value.encoded_frames,
                    value.dropped_frames
                ),
                (101, 93, 8)
            );
            assert_eq!(value.encoder_exit, "signal 9");
            assert_eq!(value.recording_id, "rec-雪-3");
            assert_eq!(value.path, PathBuf::from("/tmp/session 雪.mp4"));
        }
        (_, BrowserReply::RecordStatus { value }) => assert!(value.is_none()),
        (
            BrowserOperation::HandleDialog {
                action,
                prompt_text,
                ..
            },
            _,
        ) => {
            assert_eq!(action, "raw-action:accept 雪");
            assert_eq!(prompt_text.as_deref(), Some("prompt 雪\nanswer"));
        }
        (BrowserOperation::Upload { target, paths, .. }, _) => {
            assert!(matches!(target, Some(ActionTarget::Ref { ref_id }) if ref_id == "e42-雪"));
            assert_eq!(paths, &["/tmp/file 雪.txt", "/tmp/other.bin"]);
        }
        (BrowserOperation::Emulate { opts, .. }, _) => {
            assert_eq!(opts.color_scheme, Some(ColorScheme::Dark));
            assert_eq!(
                opts.geolocation,
                Some(Geolocation {
                    latitude: 35.125,
                    longitude: -120.75
                })
            );
            assert_eq!(opts.network_condition, Some(NetworkCondition::Slow3g));
            assert_eq!(opts.cpu_throttle, Some(3.5));
            assert_eq!(
                opts.extra_http_headers.as_ref().unwrap()["X-Test"],
                "header 雪"
            );
            assert_eq!(opts.user_agent.as_deref(), Some("Aleph/test 雪"));
        }
        (
            BrowserOperation::Cookies {
                _op:
                    CookieOp::Set {
                        expires,
                        http_only,
                        secure,
                        same_site,
                        ..
                    },
            },
            BrowserReply::Cookies { value },
        ) => {
            assert_eq!(*expires, Some(1_750_000_123)); // Unix seconds, not milliseconds.
            assert_eq!(
                (*http_only, *secure, *same_site),
                (Some(true), Some(false), Some(SameSite::Lax))
            );
            assert_eq!(value, "stored session 雪");
        }
        (BrowserOperation::FillForm { fields, .. }, BrowserReply::FillForm { value }) => {
            assert_eq!(fields.len(), 2);
            assert_eq!(*value, fields.len());
            assert!(matches!(&fields[0].0, ActionTarget::Ref { ref_id } if ref_id == "e42-雪"));
            assert!(
                matches!(&fields[1].0, ActionTarget::Coordinates { x,y } if *x == 12.5 && *y == -3.25)
            );
            assert_eq!(fields[0].1, "first 雪");
            assert_eq!(fields[1].1, "second\nvalue");
        }
        (_, BrowserReply::LastEffectVerification { value }) => {
            assert_eq!(value, &Some(EffectVerification::Skipped(Engine::Chromium)));
        }
        _ => {} // Remaining unit replies and inputs are checked by full JSON goldens.
    }
}

#[test]
fn browser_optional_upload_dialog_and_recording_values_roundtrip() {
    // Additional optional-value branches stay outside the one-per-method census.
    let uploads = [
        (None, Value::Null),
        (
            Some(coordinate_target()),
            json!({"type":"coordinates","x":12.5,"y":-3.25}),
        ),
    ];
    for (target, target_json) in uploads {
        let operation = BrowserOperation::Upload {
            tab_id: "optional-tab".into(),
            target,
            paths: vec!["/tmp/optional 雪.txt".into()],
        };
        let golden = json!({"type":"upload","tab_id":"optional-tab","target":target_json,"paths":["/tmp/optional 雪.txt"]});
        assert_eq!(serde_json::to_value(&operation).unwrap(), golden);
        let decoded: BrowserOperation = serde_json::from_value(golden.clone()).unwrap();
        validate_browser_reply(&decoded, &BrowserReply::Upload { value: () }).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), golden);
        let BrowserOperation::Upload {
            target: actual,
            paths,
            ..
        } = decoded
        else {
            panic!("wrong operation")
        };
        assert_eq!(paths, ["/tmp/optional 雪.txt"]);
        assert_eq!(serde_json::to_value(actual).unwrap(), target_json);
    }
    let dialog = BrowserOperation::HandleDialog {
        tab_id: "optional-tab".into(),
        action: "dismiss".into(),
        prompt_text: None,
    };
    let dialog_golden = json!({"type":"handle_dialog","tab_id":"optional-tab","action":"dismiss","prompt_text":null});
    assert_eq!(serde_json::to_value(&dialog).unwrap(), dialog_golden);
    let decoded: BrowserOperation = serde_json::from_value(dialog_golden.clone()).unwrap();
    assert!(matches!(
        &decoded,
        BrowserOperation::HandleDialog {
            prompt_text: None,
            ..
        }
    ));
    assert_eq!(serde_json::to_value(decoded).unwrap(), dialog_golden);

    let stop = BrowserOperation::RecordStop {
        tab_id: "optional-tab".into(),
        recording_id: Some("rec-by-id-雪".into()),
    };
    let stop_golden =
        json!({"type":"record_stop","tab_id":"optional-tab","recording_id":"rec-by-id-雪"});
    assert_eq!(serde_json::to_value(&stop).unwrap(), stop_golden);
    let decoded: BrowserOperation = serde_json::from_value(stop_golden.clone()).unwrap();
    assert!(
        matches!(&decoded, BrowserOperation::RecordStop { recording_id:Some(id), .. } if id == "rec-by-id-雪")
    );
    assert_eq!(serde_json::to_value(decoded).unwrap(), stop_golden);
    let status = BrowserReply::RecordStatus {
        value: Some(recording_status_fixture()),
    };
    let status_golden = json!({"type":"record_status","value":recording_status_golden()});
    assert_eq!(serde_json::to_value(&status).unwrap(), status_golden);
    let decoded: BrowserReply = serde_json::from_value(status_golden.clone()).unwrap();
    validate_browser_reply(
        &BrowserOperation::RecordStatus {
            tab_id: "optional-tab".into(),
        },
        &decoded,
    )
    .unwrap();
    assert!(
        matches!(&decoded, BrowserReply::RecordStatus { value:Some(actual) } if actual == &recording_status_fixture())
    );
    assert_eq!(serde_json::to_value(decoded).unwrap(), status_golden);
}

#[test]
fn browser_reply_pairing_and_unknown_tags_fail_closed() {
    let operation = BrowserOperation::OpenTab {
        url: "https://example.test".into(),
    };
    let wrong = BrowserReply::CloseTab { value: () };
    assert!(validate_browser_reply(&operation, &wrong).is_err());
    assert!(serde_json::from_value::<BrowserOperation>(
        json!({"type":"future_unknown_op","url":"x"})
    )
    .is_err());
    assert!(serde_json::from_value::<BrowserReply>(
        json!({"type":"future_unknown_reply","value":null})
    )
    .is_err());
    assert!(serde_json::from_value::<BrowserFeature>(json!("future_unknown_feature")).is_err());
}

#[test]
fn browser_effect_none_and_all_three_states_roundtrip_without_claiming_verification() {
    let operation = BrowserOperation::LastEffectVerification {};
    let cases = [
        (None, Value::Null),
        (
            Some(EffectVerification::Verified),
            json!({"state":"verified"}),
        ),
        (Some(EffectVerification::Failed), json!({"state":"failed"})),
        (
            Some(EffectVerification::Skipped(Engine::Obscura)),
            json!({"state":"skipped","engine":"obscura"}),
        ),
    ];
    for (value, expected) in cases {
        let reply = BrowserReply::LastEffectVerification {
            value: value.clone(),
        };
        let golden = json!({"type":"last_effect_verification","value":expected});
        assert_eq!(serde_json::to_value(&reply).unwrap(), golden);
        let decoded: BrowserReply = serde_json::from_value(golden.clone()).unwrap();
        validate_browser_reply(&operation, &decoded).unwrap();
        let BrowserReply::LastEffectVerification { value: actual } = &decoded else {
            panic!("wrong reply")
        };
        assert_eq!(actual, &value);
        assert_eq!(serde_json::to_value(&decoded).unwrap(), golden);
    }
}

#[test]
fn browser_route_rule_info_static_labels_roundtrip_from_owned_json_and_reject_unknown() {
    for (label, scope, hits, active) in [("mock", "profile", 37, false), ("abort", "tab", 91, true)]
    {
        let golden = json!({"id":"rule-owned-雪","url_contains":"/owned/雪","method":"PUT","kind_label":label,
            "scope":scope,"hits":hits,"active":active,"note":"all fields 雪"});
        // Owned String input cannot satisfy a borrowed 'static Deserialize bound.
        let input = golden.to_string();
        let decoded: RouteRuleInfo = serde_json::from_str(&input).unwrap();
        drop(input);
        let _: &'static str = decoded.kind_label;
        assert_eq!(decoded.kind_label, label);
        assert_eq!(decoded.id, "rule-owned-雪");
        assert_eq!(decoded.url_contains, "/owned/雪");
        assert_eq!(decoded.method.as_deref(), Some("PUT"));
        assert_eq!(decoded.note.as_deref(), Some("all fields 雪"));
        assert_eq!(decoded.hits, hits);
        assert_eq!(decoded.active, active);
        assert_eq!(
            decoded.scope,
            if scope == "tab" {
                RouteScope::Tab
            } else {
                RouteScope::Profile
            }
        );
        assert_eq!(serde_json::to_value(&decoded).unwrap(), golden);
        let mut bad = golden;
        bad["kind_label"] = json!("future-unknown-kind");
        assert!(serde_json::from_value::<RouteRuleInfo>(bad).is_err());
    }
}

#[test]
fn browser_record_start_wire_preserves_normal_fields_and_rejects_explicit_ffmpeg_even_null() {
    let golden = json!({"path":"/tmp/normal 雪.mp4","fps_cap":19,"format":"mp4","quality":61});
    let options: RecordStartOptions = serde_json::from_value(golden.clone()).unwrap();
    assert_eq!(options.path, Some(PathBuf::from("/tmp/normal 雪.mp4")));
    assert_eq!((options.fps_cap, options.quality), (19, 61));
    assert_eq!(options.format, RecordingFormat::Mp4);
    assert!(options.ffmpeg.is_none());
    assert_eq!(serde_json::to_value(&options).unwrap(), golden);
    let request = BrowserRequest {
        profile: "profile-雪".into(),
        operation: BrowserOperation::RecordStart {
            tab_id: "tab-7".into(),
            options,
        },
    };
    validate_browser_request(&request).unwrap();
    let request_json = serde_json::to_value(&request).unwrap();
    assert_eq!(
        request_json,
        json!({"profile":"profile-雪","operation":{"type":"record_start","tab_id":"tab-7","options":golden}})
    );
    let decoded: BrowserRequest = serde_json::from_value(request_json.clone()).unwrap();
    validate_browser_request(&decoded).unwrap();
    assert_eq!(serde_json::to_value(&decoded).unwrap(), request_json);
    for forbidden in [json!("/tmp/injected-encoder"), Value::Null] {
        let mut bad = golden.clone();
        bad["ffmpeg"] = forbidden.clone();
        assert!(serde_json::from_value::<RecordStartOptions>(bad).is_err());
        let mut bad_request = request_json.clone();
        bad_request["operation"]["options"]["ffmpeg"] = forbidden;
        assert!(serde_json::from_value::<BrowserRequest>(bad_request).is_err());
    }
    let second: RecordStartOptions = serde_json::from_value(golden.clone()).unwrap();
    assert_eq!(serde_json::to_value(second).unwrap(), golden);
}

// C2 uses private in-memory actors and the real pure guards. These helpers
// neither authorize native operations nor stand in for a production adapter.
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use aleph_protocol::browser_live::{
    BrowserTargetBinding, ConversationBinding, IssuerIdentity, ViewerBinding,
};
use aleph_slot_api::error::AlephError;
use aleph_slot_api::tool::RefusedBy;
use async_trait::async_trait;

async fn call_browser(
    actor: &dyn BrowserManagerContract,
    request: &BrowserRequest,
) -> Result<BrowserReply, BrowserContractError> {
    validate_browser_request(request)?;
    validate_browser_claim(&[request.operation.feature()], &actor.capabilities())?;
    let reply = actor.execute(request).await?;
    validate_browser_reply(&request.operation, &reply)?;
    Ok(reply)
}

#[derive(Default)]
struct BrowserWakeCounter(AtomicUsize);

impl Wake for BrowserWakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

// All non-cancellation actor branches below must complete on their first poll.
// A surprising Pending fails immediately instead of depending on a timer.
fn browser_ready<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(BrowserWakeCounter::default()));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("expected an immediately ready browser call"),
    }
}

#[test]
fn browser_claim_requires_every_feature() {
    // Catches an any-feature check, an early success, or skipped endpoints.
    use BrowserFeature::{Navigate, OpenTab, Snapshot};
    let offered = BrowserCapabilities {
        supported: vec![OpenTab],
    };
    assert!(matches!(
        validate_browser_claim(&[OpenTab, Snapshot], &offered),
        Err(BrowserContractError::Unsupported(Snapshot))
    ));
    validate_browser_claim(&[OpenTab], &offered).unwrap();
    for required in [
        vec![Snapshot, OpenTab, OpenTab],
        vec![OpenTab, Snapshot, OpenTab],
        vec![OpenTab, OpenTab, Snapshot],
        vec![OpenTab, Snapshot, Snapshot],
    ] {
        assert!(matches!(
            validate_browser_claim(&required, &offered),
            Err(BrowserContractError::Unsupported(Snapshot))
        ));
    }
    let empty = BrowserCapabilities { supported: vec![] };
    validate_browser_claim(&[], &empty).unwrap();
    validate_browser_claim(&[], &offered).unwrap();
    assert!(matches!(
        validate_browser_claim(&[Snapshot], &empty),
        Err(BrowserContractError::Unsupported(Snapshot))
    ));
    validate_browser_claim(&[OpenTab, OpenTab], &offered).unwrap();
    let duplicates = BrowserCapabilities {
        supported: vec![OpenTab, OpenTab],
    };
    validate_browser_claim(&[OpenTab, OpenTab], &duplicates).unwrap();
    assert!(matches!(
        validate_browser_claim(&[OpenTab, Snapshot], &duplicates),
        Err(BrowserContractError::Unsupported(Snapshot))
    ));
    assert!(matches!(
        validate_browser_claim(&[OpenTab, Snapshot, Navigate], &offered),
        Err(BrowserContractError::Unsupported(Snapshot))
    ));
}

struct ScriptedBrowser {
    outcome: Mutex<Option<Result<BrowserReply, BrowserContractError>>>,
    entered: AtomicUsize,
}

#[async_trait]
impl BrowserManagerContract for ScriptedBrowser {
    fn capabilities(&self) -> BrowserCapabilities {
        BrowserCapabilities {
            supported: vec![BrowserFeature::Navigate],
        }
    }

    async fn execute(
        &self,
        request: &BrowserRequest,
    ) -> Result<BrowserReply, BrowserContractError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.profile, "c2-profile-雪");
        assert!(matches!(
            &request.operation,
            BrowserOperation::Navigate { tab_id, url }
                if tab_id == "typed-tab" && url == "https://example.test/new"
        ));
        self.outcome.lock().unwrap().take().expect("one execution")
    }
}

fn typed_navigation_request() -> BrowserRequest {
    BrowserRequest {
        profile: "c2-profile-雪".into(),
        operation: BrowserOperation::Navigate {
            tab_id: "typed-tab".into(),
            url: "https://example.test/new".into(),
        },
    }
}

fn browser_error_through_execute(error: BrowserContractError) -> BrowserContractError {
    let actor = ScriptedBrowser {
        outcome: Mutex::new(Some(Err(error))),
        entered: AtomicUsize::new(0),
    };
    let request = typed_navigation_request();
    let future = call_browser(&actor, &request);
    assert_eq!(actor.entered.load(Ordering::SeqCst), 0);
    let error = browser_ready(future).unwrap_err();
    assert_eq!(actor.entered.load(Ordering::SeqCst), 1);
    assert!(actor.outcome.lock().unwrap().is_none());
    error
}

#[test]
fn browser_typed_permission_never_ordinary() {
    // Catches erasure/string-based relabeling of typed failures in the async
    // path. P0 has no production fallback classifier to exercise here.
    for by in [
        RefusedBy::Hook,
        RefusedBy::HookFailed,
        RefusedBy::Person,
        RefusedBy::NobodyAsked,
    ] {
        let error = browser_error_through_execute(BrowserContractError::Refused {
            by,
            reason: "declined 雪\nNavigationFailed; ActionFailed; permission denied; cancelled"
                .into(),
        });
        match error {
            BrowserContractError::Refused { by: actual, reason } => {
                assert_eq!(actual, by);
                assert_eq!(
                    reason,
                    "declined 雪\nNavigationFailed; ActionFailed; permission denied; cancelled"
                );
            }
            other => panic!("typed refusal was changed: {other:?}"),
        }
    }
    let error = browser_error_through_execute(
        AlephError::PermissionDenied {
            message: "OS grant absent 雪\nActionFailed: navigation was cancelled".into(),
            suggestion: Some("Grant Accessibility; keep the human hold 雪".into()),
        }
        .into(),
    );
    match error {
        BrowserContractError::Aleph(AlephError::PermissionDenied {
            message,
            suggestion,
        }) => {
            assert_eq!(
                message,
                "OS grant absent 雪\nActionFailed: navigation was cancelled"
            );
            assert_eq!(
                suggestion.as_deref(),
                Some("Grant Accessibility; keep the human hold 雪")
            );
        }
        other => panic!("canonical permission error was changed: {other:?}"),
    }
    assert!(matches!(
        browser_error_through_execute(AlephError::Cancelled.into()),
        BrowserContractError::Aleph(AlephError::Cancelled)
    ));

    let error = browser_error_through_execute(
        BrowserError::NavigationFailed(
            "NavigationFailed 雪\nActionFailed: permission denied; cancelled; human hold; effect not delivered".into(),
        ).into(),
    );
    match error {
        BrowserContractError::Native(BrowserError::NavigationFailed(detail)) => assert_eq!(
            detail,
            "NavigationFailed 雪\nActionFailed: permission denied; cancelled; human hold; effect not delivered"
        ),
        other => panic!("native navigation error was changed: {other:?}"),
    }
    let error = browser_error_through_execute(
        BrowserError::ActionFailed(
            "ActionFailed 雪\nNavigationFailed: permission denied; cancelled; human hold; effect not delivered".into(),
        ).into(),
    );
    match error {
        BrowserContractError::Native(BrowserError::ActionFailed(detail)) => assert_eq!(
            detail,
            "ActionFailed 雪\nNavigationFailed: permission denied; cancelled; human hold; effect not delivered"
        ),
        other => panic!("native action error was changed: {other:?}"),
    }
    let error = browser_error_through_execute(
        BrowserError::EffectNotDelivered {
            verb: "browser_click",
            detail: "probe 雪\nNavigationFailed / ActionFailed / permission denied / cancelled"
                .into(),
        }
        .into(),
    );
    match error {
        BrowserContractError::Native(BrowserError::EffectNotDelivered { verb, detail }) => {
            assert_eq!(verb, "browser_click");
            assert_eq!(
                detail,
                "probe 雪\nNavigationFailed / ActionFailed / permission denied / cancelled"
            );
        }
        other => panic!("native effect error was changed: {other:?}"),
    }
    let error = browser_error_through_execute(
        BrowserError::HumanHasControl {
            binding: Box::new(ViewerBinding {
                principal: IssuerIdentity::User {
                    user_id: "human-雪".into(),
                },
                task: ConversationBinding {
                    session_key: "conversation-雪".into(),
                    run_id: "run-17".into(),
                },
                target: BrowserTargetBinding {
                    profile: "c2-profile-雪".into(),
                    browser_session_id: "browser-session-23".into(),
                    target_id: "held-target-42".into(),
                },
            }),
        }
        .into(),
    );
    match error {
        BrowserContractError::Native(BrowserError::HumanHasControl { binding }) => {
            assert_eq!(
                binding.principal,
                IssuerIdentity::User {
                    user_id: "human-雪".into()
                }
            );
            assert_eq!(binding.task.session_key, "conversation-雪");
            assert_eq!(binding.task.run_id, "run-17");
            assert_eq!(binding.target.profile, "c2-profile-雪");
            assert_eq!(binding.target.browser_session_id, "browser-session-23");
            assert_eq!(binding.target.target_id, "held-target-42");
        }
        other => panic!("native human hold was changed: {other:?}"),
    }
}

#[test]
fn browser_private_sequence_rejects_mismatched_reply() {
    // Catches omission of the final production reply guard after execution.
    let actor = ScriptedBrowser {
        outcome: Mutex::new(Some(Ok(BrowserReply::OpenTab {
            value: "wrong-tag-tab".into(),
        }))),
        entered: AtomicUsize::new(0),
    };
    let request = typed_navigation_request();
    let error = browser_ready(call_browser(&actor, &request)).unwrap_err();
    assert_eq!(actor.entered.load(Ordering::SeqCst), 1);
    match error {
        BrowserContractError::Aleph(AlephError::Validation(detail)) => {
            assert_eq!(
                detail,
                "browser reply mismatch: expected navigate, actual open_tab"
            );
        }
        other => panic!("mismatched reply was not rejected: {other:?}"),
    }
}

#[derive(Default)]
struct NavigationGate {
    released: bool,
    polls: usize,
    waker: Option<Waker>,
}

struct NavigationWait<'a>(&'a Mutex<NavigationGate>);

impl Future for NavigationWait<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let mut gate = self.0.lock().unwrap();
        gate.polls += 1;
        if gate.released {
            Poll::Ready(())
        } else {
            gate.waker = Some(context.waker().clone());
            Poll::Pending
        }
    }
}

struct NavigationCleanup<'a>(&'a AtomicUsize);

impl Drop for NavigationCleanup<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct MemoryBrowser {
    supported: Vec<BrowserFeature>,
    tabs: Mutex<Vec<TabLine>>,
    entered: AtomicUsize,
    claim_reads: AtomicUsize,
    navigation_writes: AtomicUsize,
    cleanups: AtomicUsize,
    navigation_gate: Option<Mutex<NavigationGate>>,
    recorded_options: Mutex<Option<RecordStartOptions>>,
}

impl MemoryBrowser {
    fn new(supported: Vec<BrowserFeature>) -> Self {
        Self {
            supported,
            tabs: Mutex::new(vec![]),
            entered: AtomicUsize::new(0),
            claim_reads: AtomicUsize::new(0),
            navigation_writes: AtomicUsize::new(0),
            cleanups: AtomicUsize::new(0),
            navigation_gate: None,
            recorded_options: Mutex::new(None),
        }
    }
}

#[async_trait]
impl BrowserManagerContract for MemoryBrowser {
    fn capabilities(&self) -> BrowserCapabilities {
        self.claim_reads.fetch_add(1, Ordering::SeqCst);
        BrowserCapabilities {
            supported: self.supported.clone(),
        }
    }

    async fn execute(
        &self,
        request: &BrowserRequest,
    ) -> Result<BrowserReply, BrowserContractError> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.profile, "c2-profile-雪");
        match &request.operation {
            BrowserOperation::OpenTab { url } => {
                let mut tabs = self.tabs.lock().unwrap();
                let id = format!("memory-tab-{}", tabs.len() + 1);
                tabs.push(TabLine {
                    id: id.clone(),
                    url: url.clone(),
                    selected: true,
                });
                Ok(BrowserReply::OpenTab { value: id })
            }
            BrowserOperation::Navigate { tab_id, url } => {
                // This local must remain in the async frame across Pending.
                let _cleanup = self
                    .navigation_gate
                    .as_ref()
                    .map(|_| NavigationCleanup(&self.cleanups));
                if let Some(gate) = &self.navigation_gate {
                    NavigationWait(gate).await;
                }
                let mut tabs = self.tabs.lock().unwrap();
                let tab = tabs
                    .iter_mut()
                    .find(|tab| &tab.id == tab_id)
                    .ok_or_else(|| BrowserError::TabNotFound(tab_id.clone()))?;
                tab.url = url.clone();
                self.navigation_writes.fetch_add(1, Ordering::SeqCst);
                Ok(BrowserReply::Navigate { value: () })
            }
            BrowserOperation::ListTabs {} => Ok(BrowserReply::ListTabs {
                value: self.tabs.lock().unwrap().clone(),
            }),
            BrowserOperation::RecordStart { tab_id, options } => {
                assert!(self
                    .tabs
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|tab| &tab.id == tab_id));
                *self.recorded_options.lock().unwrap() = Some(options.clone());
                Ok(BrowserReply::RecordStart {
                    value: RecordingStatus {
                        recording_id: "memory-recording-1".into(),
                        path: options
                            .path
                            .clone()
                            .expect("explicit output path in this fixture"),
                        elapsed_ms: 0,
                        captured_frames: 0,
                        dropped_frames: 0,
                        ffmpeg_source: "in-memory contract mock".into(),
                    },
                })
            }
            _ => panic!("unexpected operation entered the in-memory actor"),
        }
    }
}

fn memory_request(operation: BrowserOperation) -> BrowserRequest {
    BrowserRequest {
        profile: "c2-profile-雪".into(),
        operation,
    }
}

fn memory_tabs(actor: &MemoryBrowser) -> Vec<TabLine> {
    let request = memory_request(BrowserOperation::ListTabs {});
    match browser_ready(call_browser(actor, &request)).unwrap() {
        BrowserReply::ListTabs { value } => value,
        other => panic!("wrong listing reply: {other:?}"),
    }
}

#[test]
fn browser_mock_effect_and_cancel() {
    // Catches omitted request/claim guards, lost navigation effects, and an
    // unpolled/dropped-too-early cancellation frame. No native effect claim.
    let actor = MemoryBrowser::new(vec![
        BrowserFeature::OpenTab,
        BrowserFeature::Navigate,
        BrowserFeature::ListTabs,
        BrowserFeature::RecordStart,
    ]);
    let open = memory_request(BrowserOperation::OpenTab {
        url: "https://example.test/old".into(),
    });
    let tab_id = match browser_ready(call_browser(&actor, &open)).unwrap() {
        BrowserReply::OpenTab { value } => value,
        other => panic!("wrong open reply: {other:?}"),
    };
    assert!(!tab_id.is_empty());
    let navigate = memory_request(BrowserOperation::Navigate {
        tab_id: tab_id.clone(),
        url: "https://example.test/new".into(),
    });
    assert!(matches!(
        browser_ready(call_browser(&actor, &navigate)).unwrap(),
        BrowserReply::Navigate { value: () }
    ));
    let tabs = memory_tabs(&actor);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].id, tab_id);
    assert_eq!(tabs[0].url, "https://example.test/new");
    assert!(tabs[0].selected);
    assert_eq!(actor.entered.load(Ordering::SeqCst), 3);
    assert_eq!(actor.claim_reads.load(Ordering::SeqCst), 3);
    assert_eq!(actor.navigation_writes.load(Ordering::SeqCst), 1);

    let snapshot = memory_request(BrowserOperation::Snapshot {
        tab_id: tab_id.clone(),
    });
    assert!(matches!(
        browser_ready(call_browser(&actor, &snapshot)),
        Err(BrowserContractError::Unsupported(BrowserFeature::Snapshot))
    ));
    assert_eq!(actor.entered.load(Ordering::SeqCst), 3);
    assert_eq!(actor.claim_reads.load(Ordering::SeqCst), 4);

    let forbidden = memory_request(BrowserOperation::RecordStart {
        tab_id: tab_id.clone(),
        options: RecordStartOptions {
            path: Some(PathBuf::from("/tmp/c2-explicit 雪.mp4")),
            fps_cap: 17,
            format: RecordingFormat::Mp4,
            quality: 63,
            ffmpeg: Some(PathBuf::from("/tmp/injected-encoder")),
        },
    });
    match browser_ready(call_browser(&actor, &forbidden)).unwrap_err() {
        BrowserContractError::Aleph(AlephError::Validation(detail)) => assert_eq!(
            detail,
            "record_start: ffmpeg override is not allowed through the browser contract"
        ),
        other => panic!("direct encoder override was not rejected: {other:?}"),
    }
    assert_eq!(actor.entered.load(Ordering::SeqCst), 3);
    assert_eq!(actor.claim_reads.load(Ordering::SeqCst), 4);
    assert!(actor.recorded_options.lock().unwrap().is_none());
    // Request validation also wins when the claim is missing.
    let no_claim = MemoryBrowser::new(vec![]);
    assert!(matches!(
        browser_ready(call_browser(&no_claim, &forbidden)),
        Err(BrowserContractError::Aleph(AlephError::Validation(_)))
    ));
    assert_eq!(no_claim.entered.load(Ordering::SeqCst), 0);
    assert_eq!(no_claim.claim_reads.load(Ordering::SeqCst), 0);

    let mut valid = forbidden.clone();
    let BrowserOperation::RecordStart { options, .. } = &mut valid.operation else {
        unreachable!()
    };
    options.ffmpeg = None;
    let valid_json = serde_json::to_value(&valid).unwrap();
    assert_eq!(
        valid_json,
        json!({"profile":"c2-profile-雪", "operation":{
            "type":"record_start", "tab_id":tab_id, "options":{
                "path":"/tmp/c2-explicit 雪.mp4", "fps_cap":17, "format":"mp4", "quality":63
            }
        }})
    );
    let valid: BrowserRequest = serde_json::from_value(valid_json.clone()).unwrap();
    assert_eq!(serde_json::to_value(&valid).unwrap(), valid_json);
    match browser_ready(call_browser(&actor, &valid)).unwrap() {
        BrowserReply::RecordStart { value } => {
            assert_eq!(value.recording_id, "memory-recording-1");
            assert_eq!(value.path, PathBuf::from("/tmp/c2-explicit 雪.mp4"));
            assert_eq!(
                (
                    value.elapsed_ms,
                    value.captured_frames,
                    value.dropped_frames
                ),
                (0, 0, 0)
            );
            assert_eq!(value.ffmpeg_source, "in-memory contract mock");
        }
        other => panic!("valid recording options were not accepted: {other:?}"),
    }
    let recorded = actor.recorded_options.lock().unwrap();
    let recorded = recorded.as_ref().unwrap();
    assert_eq!(
        recorded.path,
        Some(PathBuf::from("/tmp/c2-explicit 雪.mp4"))
    );
    assert_eq!((recorded.fps_cap, recorded.quality), (17, 63));
    assert_eq!(recorded.format, RecordingFormat::Mp4);
    assert!(recorded.ffmpeg.is_none());
    assert_eq!(actor.entered.load(Ordering::SeqCst), 4);
    assert_eq!(actor.claim_reads.load(Ordering::SeqCst), 5);

    let mut cancelling =
        MemoryBrowser::new(vec![BrowserFeature::Navigate, BrowserFeature::ListTabs]);
    cancelling.tabs.lock().unwrap().push(TabLine {
        id: "cancel-tab".into(),
        url: "https://example.test/before-cancel".into(),
        selected: true,
    });
    cancelling.navigation_gate = Some(Mutex::new(NavigationGate::default()));
    let request = memory_request(BrowserOperation::Navigate {
        tab_id: "cancel-tab".into(),
        url: "https://example.test/after-release".into(),
    });
    let wakes = Arc::new(BrowserWakeCounter::default());
    let waker = Waker::from(wakes.clone());
    let mut context = Context::from_waker(&waker);
    let mut pending = Box::pin(call_browser(&cancelling, &request));
    assert_eq!(cancelling.entered.load(Ordering::SeqCst), 0);
    assert_eq!(cancelling.cleanups.load(Ordering::SeqCst), 0);
    assert!(pending.as_mut().poll(&mut context).is_pending());
    assert_eq!(cancelling.entered.load(Ordering::SeqCst), 1);
    assert_eq!(cancelling.claim_reads.load(Ordering::SeqCst), 1);
    assert_eq!(cancelling.navigation_writes.load(Ordering::SeqCst), 0);
    assert_eq!(cancelling.cleanups.load(Ordering::SeqCst), 0);
    {
        let gate = cancelling.navigation_gate.as_ref().unwrap().lock().unwrap();
        assert_eq!(gate.polls, 1);
        assert!(!gate.released);
        assert!(gate.waker.as_ref().unwrap().will_wake(&waker));
    }
    assert_eq!(
        cancelling.tabs.lock().unwrap()[0].url,
        "https://example.test/before-cancel"
    );
    // Drop the owning Pin<Box<...>>, not merely a Pin<&mut ...>. This destroys
    // the actually suspended execute frame and its live RAII guard now.
    drop(pending);
    assert_eq!(cancelling.cleanups.load(Ordering::SeqCst), 1);
    let saved_waker = {
        let mut gate = cancelling.navigation_gate.as_ref().unwrap().lock().unwrap();
        gate.released = true;
        gate.waker.take().unwrap()
    };
    saved_waker.wake();
    assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
    let tabs = memory_tabs(&cancelling);
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].id, "cancel-tab");
    assert!(tabs[0].selected);
    assert_eq!(tabs[0].url, "https://example.test/before-cancel");
    assert_eq!(cancelling.entered.load(Ordering::SeqCst), 2);
    assert_eq!(cancelling.navigation_writes.load(Ordering::SeqCst), 0);
    assert_eq!(cancelling.cleanups.load(Ordering::SeqCst), 1);
    assert_eq!(
        cancelling
            .navigation_gate
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .polls,
        1
    );

    // Positive control: the same borrowed request can reach the mutation when
    // a NEW future is polled with the gate released; the sink is not a no-op.
    assert!(matches!(
        browser_ready(call_browser(&cancelling, &request)).unwrap(),
        BrowserReply::Navigate { value: () }
    ));
    assert_eq!(
        memory_tabs(&cancelling)[0].url,
        "https://example.test/after-release"
    );
    assert_eq!(cancelling.entered.load(Ordering::SeqCst), 4);
    assert_eq!(cancelling.claim_reads.load(Ordering::SeqCst), 4);
    assert_eq!(cancelling.navigation_writes.load(Ordering::SeqCst), 1);
    assert_eq!(cancelling.cleanups.load(Ordering::SeqCst), 2);
    assert_eq!(
        cancelling
            .navigation_gate
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .polls,
        2
    );
    assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
}
