//! Owned operation and successful-reply values for the native browser vocabulary.
//!
//! The declaration below is the sole method/variant/feature mapping. Feature
//! helpers describe tags only; they confer no permission or runtime capability.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::effect::EffectVerification;
use super::recording::{RecordStartOptions, RecordingReceipt, RecordingStatus};
use super::routes::{NewRouteRule, RouteRuleInfo, RouteScope};
use super::types::{
    ActionTarget, CookieOp, EmulateOptions, HistoryNav, PresentedSnapshot, ScreenshotOpts,
    ScreenshotOutput, ScrollDirection, SnapshotOutput, TabId, TabLine, WaitCondition,
};

macro_rules! declare_browser_operations {
    (
        $(
            $method:ident => $variant:ident {
                $($field:ident: $field_type:ty),* $(,)?
            } -> $return_type:ty;
        )+
    ) => {
        /// Owned inputs preserving the native method's parameter names and values.
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        pub enum BrowserOperation {
            $( $variant { $($field: $field_type),* }, )+
        }

        /// Complete successful return values, including unit and optional values.
        #[derive(Debug, Clone, Serialize, Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        pub enum BrowserReply {
            $( $variant { value: $return_type }, )+
        }

        /// A method tag, without authorization or measured-capability semantics.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum BrowserFeature {
            $( $variant, )+
        }

        impl BrowserFeature {
            /// All method tags in native declaration order.
            pub const ALL: &'static [Self] = &[$( Self::$variant, )+];

            /// The exact native method identifier corresponding to this tag.
            pub const fn method_name(self) -> &'static str {
                match self {
                    $( Self::$variant => stringify!($method), )+
                }
            }
        }

        impl BrowserOperation {
            /// The method tag of these inputs; this does not grant execution.
            pub fn feature(&self) -> BrowserFeature {
                match self {
                    $( Self::$variant { .. } => BrowserFeature::$variant, )+
                }
            }
        }

        impl BrowserReply {
            /// The method tag of this value; this does not verify an effect.
            pub fn feature(&self) -> BrowserFeature {
                match self {
                    $( Self::$variant { .. } => BrowserFeature::$variant, )+
                }
            }
        }
    };
}

// Derived from BrowserBackend in src/browser/backend.rs, excluding as_any.
// Only borrowed inputs become owned; Result's error remains outside the reply.
declare_browser_operations! {
    open_tab => OpenTab { url: String } -> TabId;
    close_tab => CloseTab { tab_id: String } -> ();
    list_tabs => ListTabs {} -> Vec<TabLine>;
    navigate => Navigate { tab_id: String, url: String } -> ();
    click => Click { tab_id: String, target: ActionTarget } -> ();
    type_text => TypeText { tab_id: String, target: ActionTarget, text: String } -> ();
    fill => Fill { tab_id: String, target: ActionTarget, value: String } -> ();
    hover => Hover { tab_id: String, target: ActionTarget } -> ();
    scroll => Scroll { tab_id: String, target: ActionTarget, direction: ScrollDirection } -> ();
    screenshot => Screenshot { tab_id: String, opts: ScreenshotOpts } -> ScreenshotOutput;
    snapshot => Snapshot { tab_id: String } -> SnapshotOutput;
    snapshot_presented => SnapshotPresented { tab_id: String, _max_chars: usize } -> PresentedSnapshot;
    evaluate => Evaluate { tab_id: String, js: String } -> String;
    select => Select { tab_id: String, target: ActionTarget, value: String } -> ();
    press_key => PressKey { tab_id: String, key: String } -> ();
    history => History { tab_id: String, nav: HistoryNav } -> ();
    dblclick => Dblclick { tab_id: String, target: ActionTarget } -> ();
    wait_for => WaitFor { tab_id: String, condition: WaitCondition, timeout_ms: u64 } -> bool;
    console_messages => ConsoleMessages { tab_id: String } -> String;
    network_log => NetworkLog { tab_id: String } -> String;
    route_add => RouteAdd { tab_id: String, rule: NewRouteRule } -> RouteRuleInfo;
    route_list => RouteList {} -> Vec<RouteRuleInfo>;
    route_remove => RouteRemove { rule_id: String } -> RouteRuleInfo;
    route_clear => RouteClear { tab_id: String, scope: RouteScope } -> usize;
    record_start => RecordStart { tab_id: String, options: RecordStartOptions } -> RecordingStatus;
    record_stop => RecordStop { tab_id: String, recording_id: Option<String> } -> RecordingReceipt;
    record_status => RecordStatus { tab_id: String } -> Option<RecordingStatus>;
    pdf => Pdf { _tab_id: String, _output_path: PathBuf } -> ();
    switch_tab => SwitchTab { tab_id: String } -> ();
    handle_dialog => HandleDialog { tab_id: String, action: String, prompt_text: Option<String> } -> ();
    drag => Drag { tab_id: String, from: ActionTarget, to: ActionTarget } -> ();
    upload => Upload { tab_id: String, target: Option<ActionTarget>, paths: Vec<String> } -> ();
    resize => Resize { tab_id: String, width: u32, height: u32 } -> ();
    emulate => Emulate { tab_id: String, opts: EmulateOptions } -> ();
    save_state => SaveState { _path: PathBuf } -> ();
    load_state => LoadState { _path: PathBuf } -> ();
    cookies => Cookies { _op: CookieOp } -> String;
    fill_form => FillForm { tab_id: String, fields: Vec<(ActionTarget, String)> } -> usize;
    last_effect_verification => LastEffectVerification {} -> Option<EffectVerification>;
}
