//! Live UI Automation probes.
//!
//! Every other test in this crate is pure: role mappings, escaping rules,
//! argument assembly. None of them can answer the one question that matters
//! about the accessibility limb — *does the tree walk actually come back with
//! properties on it?* The cached walk asks the provider to attach a property set
//! to each element and reads `Cached*` getters afterwards; a request that names
//! the wrong properties, or a provider that quietly serves none, produces a tree
//! of perfectly-shaped nodes that are all `AXUnknown` with no title, no bounds
//! and no pid. That is indistinguishable from a healthy walk of an empty
//! window at the type level, and pure tests cannot see it.
//!
//! These are `#[ignore]`d because they need a real interactive desktop session
//! with a foreground window — true on a developer machine, not on a headless
//! runner. Run them deliberately:
//!
//! ```text
//! cargo test -p aleph-desktop-windows --test uia_live -- --ignored --nocapture
//! ```

#![cfg(windows)]

use aleph_desktop::traits::AccessibilityCapability;
use aleph_desktop_windows::WindowsAccessibility;
use aleph_protocol::desktop_bridge::methods::ax::{AxElement, QueryFocusedParams, QueryTreeParams};

/// Count nodes and how many of them carry each property, depth-first.
#[derive(Default, Debug)]
struct Census {
    nodes: usize,
    with_role: usize,
    with_title: usize,
    with_bounds: usize,
    with_pid: usize,
    secure_answered: usize,
}

fn census(node: &AxElement, out: &mut Census) {
    out.nodes += 1;
    if node.role != "AXUnknown" {
        out.with_role += 1;
    }
    if node.title.as_ref().is_some_and(|t| !t.is_empty()) {
        out.with_title += 1;
    }
    if node.bounds.is_some() {
        out.with_bounds += 1;
    }
    if node.pid != 0 {
        out.with_pid += 1;
    }
    if node.secure.is_some() {
        out.secure_answered += 1;
    }
    for child in &node.children {
        census(child, out);
    }
}

#[tokio::test]
#[ignore = "needs an interactive desktop session with a foreground window"]
async fn the_cached_walk_returns_populated_nodes() {
    let ax = WindowsAccessibility::new();
    let started = std::time::Instant::now();
    let tree = ax
        .query_tree(QueryTreeParams {
            pid: None,
            max_depth: 8,
            // 0 lets the implementation apply its default node budget.
            max_nodes: 0,
        })
        .await
        .expect("query_tree failed")
        .element
        .expect("no tree for the foreground window");
    let elapsed = started.elapsed();

    let mut c = Census::default();
    census(&tree, &mut c);
    println!("walked {} nodes in {elapsed:?}: {c:?}", c.nodes);

    assert!(c.nodes > 1, "a real window has more than a root node");
    // The regression these guard: a cache request that names properties the
    // provider does not serve yields nodes with every field empty.
    assert!(
        c.with_pid * 2 >= c.nodes,
        "most nodes should carry a pid; got {}/{}",
        c.with_pid,
        c.nodes
    );
    assert!(
        c.with_role * 2 >= c.nodes,
        "most nodes should map to a known role; got {}/{}",
        c.with_role,
        c.nodes
    );
    assert!(c.with_title > 0, "some node should have a name");
    assert!(c.with_bounds > 0, "some node should have a rectangle");
    assert_eq!(
        c.secure_answered, c.nodes,
        "`secure` is decided for every node, never left unanswered"
    );
    // The wall-clock budget is 5s; a healthy cached walk is far under it.
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "walk took {elapsed:?}, which means the budget stopped it"
    );
}

#[tokio::test]
#[ignore = "needs an interactive desktop session with a focused control"]
async fn the_focused_element_is_enriched() {
    let ax = WindowsAccessibility::new();
    let focused = ax
        .query_focused(QueryFocusedParams::default())
        .await
        .expect("query_focused failed");
    let Some(el) = focused else {
        println!("nothing focused; skipping");
        return;
    };
    println!("focused: {el:?}");
    // `secure` is what the type_text pre-flight gate reads; it must always be a
    // decision, and `actions`/`settable` are what `enrich_resolved` adds for the
    // one element a call resolved.
    assert!(el.secure.is_some(), "`secure` must be decided");
    assert!(
        el.settable.is_some(),
        "the resolved element must be probed for settability"
    );
}

mod reference_fixture {
    use super::*;
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use aleph_desktop::DesktopError;
    use aleph_protocol::desktop_bridge::methods::ax::{
        AxMutation, AxReferenceAction, AxReferenceError, AxReferenceSnapshot, AxSnapshotRequest,
        NativeAxReference,
    };
    use windows::core::{w, HSTRING};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetDlgItem, GetMessageW,
        GetWindowLongPtrW, PostThreadMessageW, RegisterClassW, SetWindowLongPtrW, SetWindowTextW,
        TranslateMessage, GWLP_USERDATA, HMENU, MSG, WINDOW_EX_STYLE, WM_APP, WM_COMMAND, WM_QUIT,
        WNDCLASSW, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
    };

    const REPLACE_SECOND_WINDOW: u32 = WM_APP + 1;
    const PRESS_ID: usize = 101;
    const STATUS_ID: usize = 102;
    const EDIT_ID: usize = 103;
    const WINDOW_TITLE: &str = "Aleph UIA reference fixture";
    const BUTTON_TITLE: &str = "Count this press";

    struct Fixture {
        thread: u32,
        join: Option<JoinHandle<()>>,
        windows: [u64; 2],
        replaced: mpsc::Receiver<u64>,
    }

    impl Fixture {
        fn new() -> Self {
            let (started, ready) = mpsc::sync_channel(1);
            let (replacement, replaced) = mpsc::channel();
            let join = thread::spawn(move || fixture_thread(started, replacement));
            let (thread, windows) = ready
                .recv_timeout(Duration::from_secs(5))
                .expect("fixture start");
            Self {
                thread,
                join: Some(join),
                windows,
                replaced,
            }
        }

        fn pid(&self) -> i32 {
            std::process::id().try_into().expect("fixture PID fits i32")
        }

        fn counts(&self) -> [isize; 2] {
            // SAFETY: values come from the two fixture-owned windows.
            self.windows
                .map(|id| unsafe { GetWindowLongPtrW(hwnd(id), GWLP_USERDATA) })
        }

        fn recreate_second_window(&mut self) {
            // SAFETY: the fixture has created its message queue and accepts this command.
            unsafe { PostThreadMessageW(self.thread, REPLACE_SECOND_WINDOW, WPARAM(0), LPARAM(0)) }
                .expect("post recreation command");
            self.windows[1] = self
                .replaced
                .recv_timeout(Duration::from_secs(5))
                .expect("receive recreated window");
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // SAFETY: send quit only to the fixture's own message loop.
            if unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) }.is_ok() {
                if let Some(thread) = self.join.take() {
                    let _ = thread.join();
                }
            }
        }
    }

    fn window_id(hwnd: HWND) -> u64 {
        hwnd.0 as usize as u64
    }

    fn hwnd(id: u64) -> HWND {
        HWND(id as usize as *mut core::ffi::c_void)
    }

    fn fixture_thread(started: mpsc::SyncSender<(u32, [u64; 2])>, replacement: mpsc::Sender<u64>) {
        // SAFETY: registration, creation, message handling and destruction are
        // confined to this owning GUI thread.
        unsafe {
            let instance = GetModuleHandleW(None).expect("fixture module instance");
            let class = WNDCLASSW {
                hInstance: instance.into(),
                lpfnWndProc: Some(window_proc),
                lpszClassName: w!("AlephAxReferenceTestWindow"),
                ..Default::default()
            };
            assert_ne!(RegisterClassW(&class), 0, "register fixture class");
            let first = create_window(40);
            let mut second = create_window(420);
            started
                .send((GetCurrentThreadId(), [window_id(first), window_id(second)]))
                .expect("fixture receiver");
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                if msg.hwnd.0.is_null() && msg.message == REPLACE_SECOND_WINDOW {
                    DestroyWindow(second).expect("close selected window");
                    second = create_window(420);
                    replacement
                        .send(window_id(second))
                        .expect("fixture replacement receiver");
                } else {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            let _ = DestroyWindow(second);
            let _ = DestroyWindow(first);
        }
    }

    fn create_window(x: i32) -> HWND {
        // SAFETY: register and call this on the fixture's GUI thread only.
        unsafe {
            let instance = GetModuleHandleW(None).expect("fixture module instance");
            let root = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("AlephAxReferenceTestWindow"),
                &HSTRING::from(WINDOW_TITLE),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                x,
                100,
                320,
                200,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .expect("create fixture window");
            for (id, class, text, y) in [
                (PRESS_ID, w!("BUTTON"), BUTTON_TITLE, 20),
                (STATUS_ID, w!("STATIC"), "Presses: 0", 60),
                (EDIT_ID, w!("EDIT"), "initial", 100),
            ] {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    class,
                    &HSTRING::from(text),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                    20,
                    y,
                    220,
                    30,
                    Some(root),
                    Some(HMENU(id as *mut core::ffi::c_void)),
                    Some(instance.into()),
                    None,
                )
                .expect("create fixture control");
            }
            root
        }
    }

    extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: Win32 calls this procedure on the fixture window's GUI thread.
        unsafe {
            if message == WM_COMMAND && wparam.0 == PRESS_ID && lparam.0 != 0 {
                let count = GetWindowLongPtrW(hwnd, GWLP_USERDATA) + 1;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, count);
                if let Ok(status) = GetDlgItem(Some(hwnd), STATUS_ID as i32) {
                    let _ = SetWindowTextW(status, &HSTRING::from(format!("Presses: {count}")));
                }
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
    }

    fn request(pid: i32, window_id: Option<u64>) -> AxSnapshotRequest {
        AxSnapshotRequest {
            pid: Some(pid),
            window_id,
            max_depth: 8,
            max_nodes: 128,
            max_elements: 64,
        }
    }

    fn button(snapshot: &AxReferenceSnapshot) -> NativeAxReference {
        let row = snapshot
            .elements
            .iter()
            .find(|row| {
                row.element.role == "AXButton" && row.element.title.as_deref() == Some(BUTTON_TITLE)
            })
            .expect("fixture button in the AX snapshot");
        assert!(row
            .element
            .actions
            .as_ref()
            .is_some_and(|actions| actions.iter().any(|a| a == "AXPress")));
        row.reference.clone().expect("native button reference")
    }

    fn press(reference: NativeAxReference) -> AxReferenceAction {
        AxReferenceAction {
            reference,
            mutation: AxMutation::Perform {
                action: "AXPress".into(),
            },
        }
    }

    fn assert_error(error: DesktopError, expected: AxReferenceError) {
        assert!(
            matches!(error, DesktopError::AxReference { code, .. } if code == expected),
            "expected {expected:?}, got {error:?}"
        );
    }

    #[tokio::test]
    #[ignore = "needs a real Windows interactive desktop for the owned two-window fixture"]
    async fn uia_ref_selects_second_window_and_rejects_closed_window() {
        let mut fixture = Fixture::new();
        let ax = WindowsAccessibility::new();
        let pid = fixture.pid();
        let [first, second] = fixture.windows;
        let wrong_pid = ax
            .snapshot_references(request(pid + 1, Some(second)))
            .await
            .expect_err("a mismatching PID must refuse the exact HWND");
        assert_error(wrong_pid, AxReferenceError::WindowScopeUnsupported);
        let child =
            unsafe { GetDlgItem(Some(hwnd(second)), PRESS_ID as i32) }.expect("button HWND");
        let wrong_window = ax
            .snapshot_references(request(pid, Some(window_id(child))))
            .await
            .expect_err("a child HWND must not select another scope");
        assert_error(wrong_window, AxReferenceError::WindowScopeUnsupported);
        assert_eq!(fixture.counts(), [0, 0]);

        let first_snapshot = ax
            .snapshot_references(request(pid, Some(first)))
            .await
            .expect("first window");
        let second_snapshot = ax
            .snapshot_references(request(pid, Some(second)))
            .await
            .expect("second window");
        assert_eq!(
            (first_snapshot.app_pid, first_snapshot.window_id),
            (pid, Some(first))
        );
        assert_eq!(
            (second_snapshot.app_pid, second_snapshot.window_id),
            (pid, Some(second))
        );
        let pid_snapshot = ax
            .snapshot_references(request(pid, None))
            .await
            .expect("PID-only window");
        assert_eq!(pid_snapshot.app_pid, pid);
        assert!([Some(first), Some(second)].contains(&pid_snapshot.window_id));
        let first_button = button(&first_snapshot);
        let second_button = button(&second_snapshot);
        assert_ne!(first_button, second_button);

        let other_driver = WindowsAccessibility::new();
        let wrong_owner = other_driver
            .act_on_reference(press(second_button.clone()))
            .await
            .expect_err("a different driver must not consume this lease");
        assert_error(wrong_owner, AxReferenceError::StaleReference);
        assert_eq!(fixture.counts(), [0, 0]);

        let action = ax
            .act_on_reference(press(second_button.clone()))
            .await
            .expect("invoke the second button");
        assert!(action.performed);
        tokio::time::timeout(Duration::from_secs(3), async {
            while fixture.counts() == [0, 0] {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the fixture receives the invocation");
        assert_eq!(
            fixture.counts(),
            [0, 1],
            "only the selected control receives the command"
        );
        let readback = ax
            .snapshot_references(request(pid, Some(second)))
            .await
            .expect("independent second-window readback");
        assert!(readback
            .elements
            .iter()
            .any(|row| row.element.title.as_deref() == Some("Presses: 1")));

        let edit_ref = readback
            .elements
            .iter()
            .find(|row| row.element.role == "AXTextField")
            .and_then(|row| row.reference.clone())
            .expect("settable fixture edit reference");
        let value = "原生 UIA readback";
        let action = ax
            .act_on_reference(AxReferenceAction {
                reference: edit_ref,
                mutation: AxMutation::SetValue {
                    value: value.into(),
                },
            })
            .await
            .expect("set value in the selected window");
        assert_eq!(
            action.verification.expect("value readback").state,
            "verified"
        );
        let readback = ax
            .snapshot_references(request(pid, Some(second)))
            .await
            .expect("independent value readback");
        assert!(readback
            .elements
            .iter()
            .any(|row| row.element.value.as_deref() == Some(value)));
        let first_readback = ax
            .snapshot_references(request(pid, Some(first)))
            .await
            .expect("first window readback");
        assert!(first_readback
            .elements
            .iter()
            .any(|row| row.element.value.as_deref() == Some("initial")));

        fixture.recreate_second_window();
        let stale = ax
            .act_on_reference(press(second_button))
            .await
            .expect_err("the closed window's reference cannot move to the replacement");
        assert_error(stale, AxReferenceError::StaleReference);
        assert_eq!(fixture.counts(), [0, 0]);
        let replacement = ax
            .snapshot_references(request(pid, Some(fixture.windows[1])))
            .await
            .expect("replacement window has the same controls");
        button(&replacement);
        assert_eq!(fixture.counts(), [0, 0]);
    }
}
