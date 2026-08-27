//! Parse UI-TARS-style action scripts into a sequence of [`DesktopBatchAction`].
//!
//! UI-TARS-finetuned VLMs emit one or more lines of:
//!
//! ```text
//! Thought: I want to click the OK button.
//! Action: click(start_box='(500,500)')
//! ```
//!
//! or multi-action variants (Doubao 1.5 prompts allow a `;`-joined chain):
//!
//! ```text
//! Action: type(content='hello\n'); wait()
//! ```
//!
//! This parser is intentionally tolerant — it accepts the four box formats
//! UI-TARS uses (`(x,y)`, `[x1,y1,x2,y2]`, `<point>x y</point>`,
//! `<bbox>x1 y1 x2 y2</bbox>`), strips `Thought:` lines, and emits one
//! [`DesktopBatchAction`] per recognized call. Coordinates pass through in
//! the caller-declared [`coord_space`](super::types::DesktopArgs::coord_space)
//! — typically `"normalized"` for UI-TARS-trained models — and are rescaled
//! to pixels by [`super::coord_resolve`] at dispatch time.
//!
//! Anything the parser cannot interpret becomes a [`ScriptParseError`], not a
//! silent skip — letting bugs hide behind the parser would defeat the point.

use super::types::DesktopBatchAction;

/// Errors that surface while parsing a UI-TARS action script.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ScriptParseError {
    #[error("empty action script")]
    Empty,
    #[error("unrecognised action: {0}")]
    UnknownAction(String),
    #[error("malformed call: {0}")]
    MalformedCall(String),
    #[error("missing required argument '{arg}' for action '{action}'")]
    MissingArg { action: String, arg: String },
    #[error("invalid coordinate in '{0}'")]
    BadCoord(String),
}

/// Parse an action script into a sequence of batch sub-actions.
///
/// The parser only translates syntax; it does **not** rescale coordinates.
/// All emitted actions inherit the caller's `coord_space` / `coord_factors`
/// via the dispatcher's later normalization pass.
pub fn parse_action_script(script: &str) -> Result<Vec<DesktopBatchAction>, ScriptParseError> {
    let stripped: Vec<&str> = script
        .lines()
        .filter_map(|raw| {
            let line = raw.trim();
            if line.is_empty() {
                return None;
            }
            // Drop "Thought:" preamble lines — they carry no action.
            if line.to_ascii_lowercase().starts_with("thought:") {
                return None;
            }
            // Strip leading "Action:" tag (case-insensitive).
            let payload = line
                .strip_prefix("Action:")
                .or_else(|| line.strip_prefix("action:"))
                .or_else(|| line.strip_prefix("ACTION:"))
                .unwrap_or(line)
                .trim();
            if payload.is_empty() {
                None
            } else {
                Some(payload)
            }
        })
        .collect();

    if stripped.is_empty() {
        return Err(ScriptParseError::Empty);
    }

    let mut out = Vec::new();
    for line in stripped {
        for call in split_call_chain(line) {
            out.push(parse_single_call(call)?);
        }
    }

    if out.is_empty() {
        return Err(ScriptParseError::Empty);
    }
    Ok(out)
}

/// Split a chain like `type(content='hi'); wait(); click(start_box='(1,2)')`
/// while respecting quoted strings.
fn split_call_chain(input: &str) -> Vec<&str> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut depth_paren = 0i32;
    let mut prev_escape = false;

    for (i, &b) in bytes.iter().enumerate() {
        let escaped = prev_escape;
        prev_escape = !escaped && b == b'\\';
        match b {
            b'\'' if !in_double && !escaped => in_single = !in_single,
            b'"' if !in_single && !escaped => in_double = !in_double,
            b'(' if !in_single && !in_double => depth_paren += 1,
            b')' if !in_single && !in_double => depth_paren -= 1,
            b';' if !in_single && !in_double && depth_paren == 0 => {
                let chunk = input[start..i].trim();
                if !chunk.is_empty() {
                    out.push(chunk);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = input[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

fn parse_single_call(call: &str) -> Result<DesktopBatchAction, ScriptParseError> {
    let open = call
        .find('(')
        .ok_or_else(|| ScriptParseError::MalformedCall(call.into()))?;
    if !call.ends_with(')') {
        return Err(ScriptParseError::MalformedCall(call.into()));
    }
    let name = call[..open].trim().to_ascii_lowercase();
    let body = &call[open + 1..call.len() - 1];

    let kwargs = parse_kwargs(body)?;
    build_action(&name, &kwargs)
}

#[derive(Debug)]
struct Kwargs<'a> {
    items: Vec<(String, &'a str)>,
}

impl Kwargs<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.items
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| *v)
    }
}

/// Tokenize a kwargs body `key='value', key2='v2'` with quote-aware splitting.
///
/// Quoted values keep internal commas, brackets and parens intact. Unquoted
/// values (rare — e.g. bare numbers) are split on top-level `,`.
fn parse_kwargs(body: &str) -> Result<Kwargs<'_>, ScriptParseError> {
    let body = body.trim();
    if body.is_empty() {
        return Ok(Kwargs { items: vec![] });
    }

    let bytes = body.as_bytes();
    let mut items = Vec::new();
    let mut start = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_escape = false;
    let mut chunks = Vec::new();

    for (i, &b) in bytes.iter().enumerate() {
        let escaped = prev_escape;
        prev_escape = !escaped && b == b'\\';
        match b {
            b'\'' if !in_double && !escaped => in_single = !in_single,
            b'"' if !in_single && !escaped => in_double = !in_double,
            b',' if !in_single && !in_double => {
                let chunk = body[start..i].trim();
                if !chunk.is_empty() {
                    chunks.push(chunk);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = body[start..].trim();
    if !tail.is_empty() {
        chunks.push(tail);
    }

    for chunk in chunks {
        let eq = chunk
            .find('=')
            .ok_or_else(|| ScriptParseError::MalformedCall(chunk.into()))?;
        let key = chunk[..eq].trim().to_string();
        let raw_val = chunk[eq + 1..].trim();
        let val = strip_quotes(raw_val);
        items.push((key, val));
    }
    Ok(Kwargs { items })
}

fn strip_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && ((bytes.first() == Some(&b'\'') && bytes.last() == Some(&b'\''))
            || (bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn build_action(name: &str, kw: &Kwargs<'_>) -> Result<DesktopBatchAction, ScriptParseError> {
    let mut action = blank_batch_action();
    match name {
        // ── Click family ────────────────────────────────────────
        "click" | "left_click" | "left_single" | "tap" => {
            let (x, y) = require_box_center(kw, "start_box", "click")?;
            action.action = "click".into();
            action.x = Some(x);
            action.y = Some(y);
        }
        "left_double" | "double_click" | "dblclick" => {
            let (x, y) = require_box_center(kw, "start_box", "double_click")?;
            action.action = "double_click".into();
            action.x = Some(x);
            action.y = Some(y);
        }
        "right_single" | "right_click" => {
            let (x, y) = require_box_center(kw, "start_box", "click")?;
            action.action = "click".into();
            action.x = Some(x);
            action.y = Some(y);
            action.button = Some(super::types::MouseButton::Right);
        }
        "middle_click" | "middle_single" => {
            let (x, y) = require_box_center(kw, "start_box", "click")?;
            action.action = "click".into();
            action.x = Some(x);
            action.y = Some(y);
            action.button = Some(super::types::MouseButton::Middle);
        }

        // ── Drag ────────────────────────────────────────────────
        "drag" | "left_click_drag" | "select" => {
            let (sx, sy) = require_box_center(kw, "start_box", "drag")?;
            let (ex, ey) = require_box_center(kw, "end_box", "drag")?;
            action.action = "drag".into();
            action.start_x = Some(sx);
            action.start_y = Some(sy);
            action.end_x = Some(ex);
            action.end_y = Some(ey);
        }
        "hover" | "move_to" => {
            let (x, y) = require_box_center(kw, "start_box", "hover")?;
            action.action = "hover".into();
            action.x = Some(x);
            action.y = Some(y);
        }

        // ── Keyboard ────────────────────────────────────────────
        "type" => {
            let content = kw
                .get("content")
                .ok_or_else(|| ScriptParseError::MissingArg {
                    action: "type".into(),
                    arg: "content".into(),
                })?;
            action.action = "type_text".into();
            action.text = Some(unescape(content));
        }
        "hotkey" => {
            let key = kw.get("key").ok_or_else(|| ScriptParseError::MissingArg {
                action: "hotkey".into(),
                arg: "key".into(),
            })?;
            action.action = "key_combo".into();
            action.keys = Some(split_hotkey(key));
        }
        // UI-TARS `press`/`release`: hold a key (or chord) down, release it
        // later — distinct from `hotkey` which presses and releases atomically.
        "press" | "key_down" => {
            let key = kw.get("key").ok_or_else(|| ScriptParseError::MissingArg {
                action: "press".into(),
                arg: "key".into(),
            })?;
            action.action = "key_button".into();
            action.keys = Some(split_hotkey(key));
            action.press_action = Some("press".into());
        }
        "release" | "key_up" => {
            let key = kw.get("key").ok_or_else(|| ScriptParseError::MissingArg {
                action: "release".into(),
                arg: "key".into(),
            })?;
            action.action = "key_button".into();
            action.keys = Some(split_hotkey(key));
            action.press_action = Some("release".into());
        }

        // ── Scroll ──────────────────────────────────────────────
        "scroll" => {
            let direction = kw.get("direction").unwrap_or("down");
            let (delta_x, delta_y) = match direction.to_ascii_lowercase().as_str() {
                "up" => (0.0, -500.0),
                "down" => (0.0, 500.0),
                "left" => (-500.0, 0.0),
                "right" => (500.0, 0.0),
                other => return Err(ScriptParseError::BadCoord(format!("scroll dir {other}"))),
            };
            action.action = "scroll".into();
            action.delta_x = Some(delta_x);
            action.delta_y = Some(delta_y);
            if let Some(b) = kw.get("start_box") {
                if let Some((x, y)) = parse_box_center(b) {
                    action.x = Some(x);
                    action.y = Some(y);
                }
            }
        }

        // ── Loop-control verbs (model emits, host honours) ─────
        "wait" => {
            action.action = "wait_visual".into();
            // No timeout_ms — caller-default 5000.
        }
        "finished" => {
            action.action = "finished".into();
            // UI-TARS `finished(content='...')` carries the model's answer —
            // keep it so the host can surface it instead of dropping it.
            if let Some(c) = kw.get("content") {
                action.text = Some(unescape(c));
            }
        }
        "call_user" => {
            action.action = "call_user".into();
            if let Some(c) = kw.get("content") {
                action.text = Some(unescape(c));
            }
        }

        unknown => return Err(ScriptParseError::UnknownAction(unknown.into())),
    }
    Ok(action)
}

const fn blank_batch_action() -> DesktopBatchAction {
    DesktopBatchAction {
        action: String::new(),
        region: None,
        image_base64: None,
        x: None,
        y: None,
        button: None,
        text: None,
        keys: None,
        bundle_id: None,
        app: None,
        window_id: None,
        start_x: None,
        start_y: None,
        end_x: None,
        end_y: None,
        delta_x: None,
        delta_y: None,
        width: None,
        height: None,
        duration_ms: None,
        press_action: None,
        duration: None,
        fps: None,
        with_audio: None,
        display_id: None,
        format: None,
        quality: None,
        max_width: None,
        max_height: None,
        timeout_ms: None,
        coord_space: None,
        coord_factors: None,
        describe: None,
        role: None,
        element_title: None,
        ax_action_name: None,
        pid: None,
        observe: None,
        // The UI-TARS action script has no syntax for it: `force` is an explicit
        // escape hatch the model opts into per call, never a script default.
        force: None,
        // No script verb maps to a postcondition check; verify_state is issued
        // as its own JSON action, never expanded from a UI-TARS line.
        expect: Vec::new(),
        stable_samples: None,
        // A token is minted by ax_snapshot output the model reads as JSON; the
        // UI-TARS script syntax has no way to carry one.
        element: None,
    }
}

/// Split a hotkey string like `"ctrl shift a"` or `"ctrl+c"` into individual
/// key tokens. Shared by the `hotkey`, `press`, and `release` verbs.
fn split_hotkey(key: &str) -> Vec<String> {
    key.split_whitespace()
        .flat_map(|s| s.split('+'))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn require_box_center(
    kw: &Kwargs<'_>,
    field: &str,
    action_name: &str,
) -> Result<(f64, f64), ScriptParseError> {
    let raw = kw.get(field).ok_or_else(|| ScriptParseError::MissingArg {
        action: action_name.into(),
        arg: field.into(),
    })?;
    parse_box_center(raw).ok_or_else(|| ScriptParseError::BadCoord(raw.into()))
}

/// Accept all four UI-TARS box formats and return the centre point.
pub fn parse_box_center(raw: &str) -> Option<(f64, f64)> {
    // UI-TARS 1.5 / Doubao-1.5-UI-TARS wrap coordinates in
    // `<|box_start|>(x,y)<|box_end|>` special tokens. Strip them before
    // format detection — the upstream parser does the same as its first step.
    let cleaned;
    let raw = if raw.contains("<|box_start|>") || raw.contains("<|box_end|>") {
        cleaned = raw.replace("<|box_start|>", "").replace("<|box_end|>", "");
        cleaned.trim()
    } else {
        raw.trim()
    };

    // <point>x y</point>
    if let Some(inner) = raw
        .strip_prefix("<point>")
        .and_then(|s| s.strip_suffix("</point>"))
    {
        let nums = parse_floats(inner);
        if nums.len() >= 2 {
            return Some((
                *nums.first().expect("invariant: length checked above"),
                *nums.get(1).expect("invariant: length checked above"),
            ));
        }
        return None;
    }
    // <bbox>x1 y1 x2 y2</bbox>
    if let Some(inner) = raw
        .strip_prefix("<bbox>")
        .and_then(|s| s.strip_suffix("</bbox>"))
    {
        let nums = parse_floats(inner);
        if nums.len() >= 4 {
            return Some((
                (*nums.first().expect("invariant: length checked above")
                    + *nums.get(2).expect("invariant: length checked above"))
                    / 2.0,
                (*nums.get(1).expect("invariant: length checked above")
                    + *nums.get(3).expect("invariant: length checked above"))
                    / 2.0,
            ));
        }
        return None;
    }

    // (x, y) or [x, y] or [x1, y1, x2, y2]
    let trimmed = raw.trim_matches(|c| c == '(' || c == ')' || c == '[' || c == ']');
    let nums = parse_floats(trimmed);
    match nums.len() {
        2 => Some((
            *nums.first().expect("invariant: length matched above"),
            *nums.get(1).expect("invariant: length matched above"),
        )),
        4 => Some((
            (*nums.first().expect("invariant: length matched above")
                + *nums.get(2).expect("invariant: length matched above"))
                / 2.0,
            (*nums.get(1).expect("invariant: length matched above")
                + *nums.get(3).expect("invariant: length matched above"))
                / 2.0,
        )),
        _ => None,
    }
}

fn parse_floats(s: &str) -> Vec<f64> {
    s.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .filter_map(|t| {
            t.trim_matches(|c: char| c == '<' || c == '>')
                .parse::<f64>()
                .ok()
        })
        .collect()
}

/// Minimal escape handling — translates `\n` / `\t` / `\\` / `\'` / `\"`.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some('"') => out.push('"'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_with_tuple_box() {
        let parsed = parse_action_script("Action: click(start_box='(500, 500)')").unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].action, "click");
        assert_eq!(parsed[0].x, Some(500.0));
        assert_eq!(parsed[0].y, Some(500.0));
    }

    #[test]
    fn click_with_bbox() {
        let parsed = parse_action_script("click(start_box='[100,200,300,400]')").unwrap();
        assert_eq!(parsed[0].x, Some(200.0));
        assert_eq!(parsed[0].y, Some(300.0));
    }

    #[test]
    fn click_with_point_tag() {
        let parsed = parse_action_script("click(start_box='<point>250 750</point>')").unwrap();
        assert_eq!(parsed[0].x, Some(250.0));
        assert_eq!(parsed[0].y, Some(750.0));
    }

    #[test]
    fn click_with_bbox_tag() {
        let parsed =
            parse_action_script("click(start_box='<bbox>100 200 300 400</bbox>')").unwrap();
        assert_eq!(parsed[0].x, Some(200.0));
        assert_eq!(parsed[0].y, Some(300.0));
    }

    #[test]
    fn click_with_box_token_markers() {
        // UI-TARS 1.5 / Doubao emit <|box_start|>…<|box_end|> around the tuple.
        let parsed =
            parse_action_script("click(start_box='<|box_start|>(500,500)<|box_end|>')").unwrap();
        assert_eq!(parsed[0].action, "click");
        assert_eq!(parsed[0].x, Some(500.0));
        assert_eq!(parsed[0].y, Some(500.0));

        // Markers around a bbox form resolve to the centre too.
        let bbox =
            parse_action_script("click(start_box='<|box_start|>[100,200,300,400]<|box_end|>')")
                .unwrap();
        assert_eq!(bbox[0].x, Some(200.0));
        assert_eq!(bbox[0].y, Some(300.0));
    }

    #[test]
    fn finished_and_call_user_capture_content() {
        let fin = parse_action_script(r"finished(content='all done\nok')").unwrap();
        assert_eq!(fin[0].action, "finished");
        assert_eq!(fin[0].text.as_deref(), Some("all done\nok"));

        let call = parse_action_script("call_user(content='need a password')").unwrap();
        assert_eq!(call[0].action, "call_user");
        assert_eq!(call[0].text.as_deref(), Some("need a password"));

        // Bare forms still parse with no content.
        let bare = parse_action_script("finished()").unwrap();
        assert_eq!(bare[0].action, "finished");
        assert_eq!(bare[0].text, None);
    }

    #[test]
    fn type_with_escaped_newline() {
        let parsed = parse_action_script(r"Action: type(content='hello\nworld')").unwrap();
        assert_eq!(parsed[0].action, "type_text");
        assert_eq!(parsed[0].text.as_deref(), Some("hello\nworld"));
    }

    #[test]
    fn hotkey_splits_into_keys() {
        let parsed = parse_action_script("hotkey(key='ctrl shift a')").unwrap();
        assert_eq!(parsed[0].action, "key_combo");
        assert_eq!(
            parsed[0].keys.as_deref(),
            Some(vec!["ctrl".into(), "shift".into(), "a".into()].as_slice())
        );
    }

    #[test]
    fn drag_requires_both_boxes() {
        let parsed = parse_action_script("drag(start_box='(10,20)', end_box='(100,200)')").unwrap();
        assert_eq!(parsed[0].action, "drag");
        assert_eq!(parsed[0].start_x, Some(10.0));
        assert_eq!(parsed[0].end_y, Some(200.0));

        let err = parse_action_script("drag(start_box='(10,20)')").unwrap_err();
        assert!(matches!(err, ScriptParseError::MissingArg { .. }));
    }

    #[test]
    fn middle_click_maps_to_click_with_middle_button() {
        let parsed = parse_action_script("middle_click(start_box='(5,6)')").unwrap();
        assert_eq!(parsed[0].action, "click");
        assert_eq!(parsed[0].x, Some(5.0));
        assert!(matches!(
            parsed[0].button,
            Some(super::super::types::MouseButton::Middle)
        ));
    }

    #[test]
    fn press_and_release_map_to_key_button() {
        let press = parse_action_script("press(key='ctrl')").unwrap();
        assert_eq!(press[0].action, "key_button");
        assert_eq!(
            press[0].keys.as_deref(),
            Some(["ctrl".to_string()].as_slice())
        );
        assert_eq!(press[0].press_action.as_deref(), Some("press"));

        let release = parse_action_script("release(key='shift a')").unwrap();
        assert_eq!(release[0].action, "key_button");
        assert_eq!(
            release[0].keys.as_deref(),
            Some(["shift".to_string(), "a".to_string()].as_slice())
        );
        assert_eq!(release[0].press_action.as_deref(), Some("release"));
    }

    #[test]
    fn hotkey_plus_separator_splits_into_keys() {
        // `ctrl+c` was previously a single broken token; now splits cleanly.
        let parsed = parse_action_script("hotkey(key='ctrl+c')").unwrap();
        assert_eq!(
            parsed[0].keys.as_deref(),
            Some(["ctrl".to_string(), "c".to_string()].as_slice())
        );
    }

    #[test]
    fn scroll_directions_map_to_deltas() {
        let down = parse_action_script("scroll(direction='down')").unwrap();
        assert_eq!(down[0].delta_y, Some(500.0));
        let up = parse_action_script("scroll(direction='up')").unwrap();
        assert_eq!(up[0].delta_y, Some(-500.0));
        let left = parse_action_script("scroll(direction='left')").unwrap();
        assert_eq!(left[0].delta_x, Some(-500.0));
    }

    #[test]
    fn loop_control_actions_pass_through() {
        let wait = parse_action_script("wait()").unwrap();
        assert_eq!(wait[0].action, "wait_visual");
        let finished = parse_action_script("finished()").unwrap();
        assert_eq!(finished[0].action, "finished");
        let call = parse_action_script("call_user()").unwrap();
        assert_eq!(call[0].action, "call_user");
    }

    #[test]
    fn chained_calls_are_split() {
        let parsed =
            parse_action_script("type(content='hi'); wait(); click(start_box='(1,2)')").unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].action, "type_text");
        assert_eq!(parsed[1].action, "wait_visual");
        assert_eq!(parsed[2].action, "click");
    }

    #[test]
    fn thought_lines_are_ignored() {
        let script = "Thought: I should click the button.\nAction: click(start_box='(1,2)')";
        let parsed = parse_action_script(script).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].action, "click");
    }

    #[test]
    fn empty_script_errors() {
        assert!(matches!(
            parse_action_script("").unwrap_err(),
            ScriptParseError::Empty
        ));
        assert!(matches!(
            parse_action_script("Thought: nothing to do.").unwrap_err(),
            ScriptParseError::Empty
        ));
    }

    #[test]
    fn unknown_action_surfaces() {
        let err = parse_action_script("teleport(x=1, y=2)").unwrap_err();
        assert!(matches!(err, ScriptParseError::UnknownAction(_)));
    }

    #[test]
    fn semicolon_inside_quotes_does_not_split() {
        let parsed = parse_action_script(r"type(content='a;b;c')").unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].text.as_deref(), Some("a;b;c"));
    }
}
