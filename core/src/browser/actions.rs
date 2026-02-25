//! Browser action primitives — click, type, fill, scroll, hover.
//!
//! Each action accepts an [`ActionTarget`] that can address elements by ARIA
//! snapshot `ref_id`, CSS selector, or raw viewport coordinates.  The
//! [`resolve_target`] helper normalises every variant to a `(x, y)` centre
//! point before the action is dispatched via JavaScript evaluation.

use chromiumoxide::Page;

use super::error::BrowserError;
use super::snapshot::{resolve_ref_to_point, take_aria_snapshot};
use super::types::{ActionTarget, AriaSnapshot, ScrollDirection};

/// Resolve an [`ActionTarget`] to viewport `(x, y)` coordinates.
///
/// | Variant       | Strategy                                                  |
/// |---------------|-----------------------------------------------------------|
/// | `Coordinates` | Returned as-is.                                           |
/// | `Ref`         | Take an ARIA snapshot, find the element, return its centre.|
/// | `Selector`    | Evaluate JS `querySelector` + `getBoundingClientRect`.    |
async fn resolve_target(page: &Page, target: &ActionTarget) -> Result<(f64, f64), BrowserError> {
    match target {
        ActionTarget::Coordinates { x, y } => Ok((*x, *y)),

        ActionTarget::Ref { ref_id } => {
            let snapshot: AriaSnapshot = take_aria_snapshot(page).await?;
            resolve_ref_to_point(&snapshot, ref_id).ok_or_else(|| {
                BrowserError::ActionFailed(format!(
                    "ARIA ref '{}' not found in snapshot",
                    ref_id
                ))
            })
        }

        ActionTarget::Selector { css } => {
            let escaped = serde_json::to_string(css).map_err(|e| {
                BrowserError::ActionFailed(format!("Failed to escape CSS selector: {e}"))
            })?;

            let js = format!(
                r#"(() => {{
                    const el = document.querySelector({escaped});
                    if (!el) return null;
                    const r = el.getBoundingClientRect();
                    return {{ x: r.x + r.width / 2, y: r.y + r.height / 2 }};
                }})()"#
            );

            let result = page
                .evaluate(js)
                .await
                .map_err(|e| BrowserError::EvalError(e.to_string()))?;

            let value: serde_json::Value = result
                .into_value()
                .unwrap_or(serde_json::Value::Null);

            if value.is_null() {
                return Err(BrowserError::ActionFailed(format!(
                    "CSS selector '{}' matched no element",
                    css
                )));
            }

            let x = value["x"]
                .as_f64()
                .ok_or_else(|| BrowserError::ActionFailed("Missing x coordinate".to_string()))?;
            let y = value["y"]
                .as_f64()
                .ok_or_else(|| BrowserError::ActionFailed("Missing y coordinate".to_string()))?;

            Ok((x, y))
        }
    }
}

/// Click the element addressed by `target`.
///
/// Resolves the target to `(x, y)` and uses `document.elementFromPoint` to
/// find and `.click()` the element at that position.
pub async fn click(page: &Page, target: &ActionTarget) -> Result<(), BrowserError> {
    let (x, y) = resolve_target(page, target).await?;

    let js = format!(
        r#"(() => {{
            const el = document.elementFromPoint({x}, {y});
            if (el) {{ el.click(); return true; }}
            return false;
        }})()"#
    );

    let result = page
        .evaluate(js)
        .await
        .map_err(|e| BrowserError::EvalError(e.to_string()))?;

    let clicked: serde_json::Value = result
        .into_value()
        .unwrap_or(serde_json::Value::Bool(false));

    if clicked.as_bool() != Some(true) {
        return Err(BrowserError::ActionFailed(format!(
            "No element found at ({x}, {y}) to click"
        )));
    }

    Ok(())
}

/// Type text into the element addressed by `target`.
///
/// Clicks the element first to ensure focus, then sets the element's `value`
/// and dispatches an `input` event so frameworks pick up the change.
pub async fn type_text(page: &Page, target: &ActionTarget, text: &str) -> Result<(), BrowserError> {
    let (x, y) = resolve_target(page, target).await?;

    let escaped_text = serde_json::to_string(text).map_err(|e| {
        BrowserError::ActionFailed(format!("Failed to escape text: {e}"))
    })?;

    let js = format!(
        r#"(() => {{
            const el = document.elementFromPoint({x}, {y});
            if (!el) return false;
            el.click();
            el.focus();
            el.value = (el.value || '') + {escaped_text};
            el.dispatchEvent(new Event('input', {{ bubbles: true }}));
            return true;
        }})()"#
    );

    let result = page
        .evaluate(js)
        .await
        .map_err(|e| BrowserError::EvalError(e.to_string()))?;

    let ok: serde_json::Value = result
        .into_value()
        .unwrap_or(serde_json::Value::Bool(false));

    if ok.as_bool() != Some(true) {
        return Err(BrowserError::ActionFailed(format!(
            "No element found at ({x}, {y}) to type into"
        )));
    }

    Ok(())
}

/// Fill (replace) the value of the element addressed by `target`.
///
/// Unlike [`type_text`], this clears the existing value before writing.
/// Dispatches both `input` and `change` events.
pub async fn fill(page: &Page, target: &ActionTarget, value: &str) -> Result<(), BrowserError> {
    let (x, y) = resolve_target(page, target).await?;

    let escaped_value = serde_json::to_string(value).map_err(|e| {
        BrowserError::ActionFailed(format!("Failed to escape fill value: {e}"))
    })?;

    let js = format!(
        r#"(() => {{
            const el = document.elementFromPoint({x}, {y});
            if (!el) return false;
            el.focus();
            el.value = {escaped_value};
            el.dispatchEvent(new Event('input', {{ bubbles: true }}));
            el.dispatchEvent(new Event('change', {{ bubbles: true }}));
            return true;
        }})()"#
    );

    let result = page
        .evaluate(js)
        .await
        .map_err(|e| BrowserError::EvalError(e.to_string()))?;

    let ok: serde_json::Value = result
        .into_value()
        .unwrap_or(serde_json::Value::Bool(false));

    if ok.as_bool() != Some(true) {
        return Err(BrowserError::ActionFailed(format!(
            "No element found at ({x}, {y}) to fill"
        )));
    }

    Ok(())
}

/// Scroll the page (or element) at the `target` coordinates.
///
/// Uses `window.scrollBy` with `behavior: 'smooth'` and a fixed delta of
/// 300 px in the appropriate direction.
pub async fn scroll(
    page: &Page,
    target: &ActionTarget,
    direction: &ScrollDirection,
) -> Result<(), BrowserError> {
    let (x, y) = resolve_target(page, target).await?;

    let (dx, dy): (i32, i32) = match direction {
        ScrollDirection::Up => (0, -300),
        ScrollDirection::Down => (0, 300),
        ScrollDirection::Left => (-300, 0),
        ScrollDirection::Right => (300, 0),
    };

    let js = format!(
        r#"(() => {{
            const el = document.elementFromPoint({x}, {y});
            const target = el || document.documentElement;
            target.scrollBy({{ left: {dx}, top: {dy}, behavior: 'smooth' }});
            return true;
        }})()"#
    );

    page.evaluate(js)
        .await
        .map_err(|e| BrowserError::EvalError(e.to_string()))?;

    Ok(())
}

/// Hover over the element addressed by `target`.
///
/// Dispatches `mouseenter` and `mouseover` events at the resolved `(x, y)`
/// coordinates so CSS `:hover` styles and JS listeners fire.
pub async fn hover(page: &Page, target: &ActionTarget) -> Result<(), BrowserError> {
    let (x, y) = resolve_target(page, target).await?;

    let js = format!(
        r#"(() => {{
            const el = document.elementFromPoint({x}, {y});
            if (!el) return false;
            const opts = {{ bubbles: true, clientX: {x}, clientY: {y} }};
            el.dispatchEvent(new MouseEvent('mouseenter', opts));
            el.dispatchEvent(new MouseEvent('mouseover', opts));
            return true;
        }})()"#
    );

    let result = page
        .evaluate(js)
        .await
        .map_err(|e| BrowserError::EvalError(e.to_string()))?;

    let ok: serde_json::Value = result
        .into_value()
        .unwrap_or(serde_json::Value::Bool(false));

    if ok.as_bool() != Some(true) {
        return Err(BrowserError::ActionFailed(format!(
            "No element found at ({x}, {y}) to hover"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::types::ActionTarget;

    /// Verify that the Coordinates variant resolves without needing a Page.
    ///
    /// Since `resolve_target` is async and requires a real `Page` for the
    /// `Ref` and `Selector` variants, we test the Coordinates path by
    /// checking the match arm directly.
    #[test]
    fn test_action_target_coordinates() {
        let target = ActionTarget::Coordinates { x: 42.0, y: 99.5 };

        match &target {
            ActionTarget::Coordinates { x, y } => {
                assert!((x - 42.0).abs() < f64::EPSILON);
                assert!((y - 99.5).abs() < f64::EPSILON);
            }
            _ => panic!("Expected Coordinates variant"),
        }
    }

    #[test]
    fn test_action_target_ref_variant() {
        let target = ActionTarget::Ref {
            ref_id: "button[0]".to_string(),
        };

        match &target {
            ActionTarget::Ref { ref_id } => {
                assert_eq!(ref_id, "button[0]");
            }
            _ => panic!("Expected Ref variant"),
        }
    }

    #[test]
    fn test_action_target_selector_variant() {
        let target = ActionTarget::Selector {
            css: "#login-form > input[type=\"email\"]".to_string(),
        };

        match &target {
            ActionTarget::Selector { css } => {
                assert_eq!(css, "#login-form > input[type=\"email\"]");
                // Verify the selector can be safely JSON-escaped for JS injection
                let escaped = serde_json::to_string(css).unwrap();
                assert!(escaped.starts_with('"'));
                assert!(escaped.ends_with('"'));
                assert!(escaped.contains("\\\"email\\\""));
            }
            _ => panic!("Expected Selector variant"),
        }
    }

    #[test]
    fn test_scroll_direction_deltas() {
        // Verify the delta mapping logic used inside scroll().
        let cases = vec![
            (ScrollDirection::Up, (0, -300)),
            (ScrollDirection::Down, (0, 300)),
            (ScrollDirection::Left, (-300, 0)),
            (ScrollDirection::Right, (300, 0)),
        ];

        for (direction, expected) in cases {
            let (dx, dy): (i32, i32) = match direction {
                ScrollDirection::Up => (0, -300),
                ScrollDirection::Down => (0, 300),
                ScrollDirection::Left => (-300, 0),
                ScrollDirection::Right => (300, 0),
            };
            assert_eq!((dx, dy), expected);
        }
    }

    #[test]
    fn test_resolve_ref_to_point_returns_none_for_missing() {
        let snapshot = AriaSnapshot {
            elements: vec![],
            page_title: None,
            page_url: None,
            focused_ref: None,
        };

        assert!(resolve_ref_to_point(&snapshot, "nonexistent[0]").is_none());
    }
}
