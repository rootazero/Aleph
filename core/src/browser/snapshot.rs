//! ARIA snapshot extraction from web pages via CDP.
//!
//! Evaluates a JavaScript snippet that walks the document's DOM tree,
//! identifies elements with semantic ARIA roles (explicit or implicit from
//! tag names), and returns a structured [`AriaSnapshot`] with positional
//! data suitable for AI-driven browser interaction.

use chromiumoxide::Page;

use super::error::BrowserError;
use super::types::{AriaElement, AriaSnapshot, ElementRect};

/// JavaScript that extracts the ARIA accessibility tree from the current page.
///
/// The script walks `document.body` recursively, skipping hidden elements,
/// iframes, and zero-size nodes. Each semantically relevant element produces
/// an [`AriaElement`]-shaped object with a stable `ref_id` (e.g. `"button[0]"`),
/// its accessible name, value, interaction state, bounding rect, and children.
const ARIA_SNAPSHOT_JS: &str = r#"
(() => {
  // Tag-name to implicit ARIA role mapping.
  const TAG_ROLE_MAP = {
    'A':        'link',
    'BUTTON':   'button',
    'SELECT':   'combobox',
    'TEXTAREA': 'textbox',
    'IMG':      'img',
    'NAV':      'navigation',
    'MAIN':     'main',
    'ASIDE':    'complementary',
    'FORM':     'form',
    'TABLE':    'table',
    'LI':       'listitem',
    'UL':       'list',
    'OL':       'list',
    'H1':       'heading',
    'H2':       'heading',
    'H3':       'heading',
    'H4':       'heading',
    'H5':       'heading',
    'H6':       'heading',
  };

  // Resolve role for <input> based on its type attribute.
  function inputRole(el) {
    const t = (el.getAttribute('type') || 'text').toLowerCase();
    switch (t) {
      case 'checkbox': return 'checkbox';
      case 'radio':    return 'radio';
      case 'submit':
      case 'reset':
      case 'button':
      case 'image':    return 'button';
      case 'range':    return 'slider';
      default:         return 'textbox';
    }
  }

  // Determine the ARIA role for an element (explicit > implicit).
  function getRole(el) {
    const explicit = el.getAttribute('role');
    if (explicit) return explicit.trim().split(/\s+/)[0];

    const tag = el.tagName;
    if (tag === 'INPUT') return inputRole(el);
    return TAG_ROLE_MAP[tag] || null;
  }

  // Compute the accessible name for an element.
  function getName(el) {
    // 1. aria-label
    const ariaLabel = el.getAttribute('aria-label');
    if (ariaLabel) return ariaLabel.trim();

    // 2. aria-labelledby
    const labelledBy = el.getAttribute('aria-labelledby');
    if (labelledBy) {
      const parts = labelledBy.split(/\s+/).map(id => {
        const refEl = document.getElementById(id);
        return refEl ? refEl.textContent.trim() : '';
      }).filter(Boolean);
      if (parts.length) return parts.join(' ');
    }

    // 3. <label> association (for form controls)
    if (el.id) {
      const label = document.querySelector('label[for="' + CSS.escape(el.id) + '"]');
      if (label) return label.textContent.trim();
    }
    // Wrapped in <label>
    const parentLabel = el.closest('label');
    if (parentLabel && parentLabel !== el) {
      // Get label text excluding the control's own text
      const clone = parentLabel.cloneNode(true);
      const controls = clone.querySelectorAll('input,select,textarea');
      controls.forEach(c => c.remove());
      const labelText = clone.textContent.trim();
      if (labelText) return labelText;
    }

    // 4. alt (images), placeholder, title
    const alt = el.getAttribute('alt');
    if (alt) return alt.trim();

    const placeholder = el.getAttribute('placeholder');
    if (placeholder) return placeholder.trim();

    const title = el.getAttribute('title');
    if (title) return title.trim();

    // 5. Direct text content (only for elements that typically derive
    //    their name from content: buttons, links, headings, listitems).
    const tag = el.tagName;
    const role = getRole(el);
    const contentRoles = ['button', 'link', 'heading', 'listitem', 'tab', 'menuitem'];
    if (contentRoles.includes(role)) {
      const text = el.textContent.trim();
      if (text && text.length <= 500) return text;
      if (text) return text.substring(0, 500);
    }

    return null;
  }

  // Collect interaction/state attributes.
  function getState(el) {
    const states = [];
    if (document.activeElement === el) states.push('focused');
    if (el.disabled || el.getAttribute('aria-disabled') === 'true') states.push('disabled');
    // checked
    const checked = el.getAttribute('aria-checked') || (el.checked !== undefined ? String(el.checked) : null);
    if (checked === 'true') states.push('checked');
    if (checked === 'mixed') states.push('mixed');
    // expanded
    const expanded = el.getAttribute('aria-expanded');
    if (expanded === 'true') states.push('expanded');
    if (expanded === 'false') states.push('collapsed');
    // selected
    if (el.getAttribute('aria-selected') === 'true' || el.selected) states.push('selected');
    // hidden (should not normally appear, since we skip hidden, but just in case)
    if (el.getAttribute('aria-hidden') === 'true') states.push('hidden');
    // required
    if (el.required || el.getAttribute('aria-required') === 'true') states.push('required');
    // readonly
    if (el.readOnly || el.getAttribute('aria-readonly') === 'true') states.push('readonly');
    return states;
  }

  // Get element value for form controls.
  function getValue(el) {
    if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT') {
      return el.value || null;
    }
    const ariaValue = el.getAttribute('aria-valuenow');
    if (ariaValue !== null) return ariaValue;
    return null;
  }

  // Counter per role for generating ref_ids.
  const roleCounts = {};
  function nextRefId(role) {
    if (!(role in roleCounts)) roleCounts[role] = 0;
    const id = role + '[' + roleCounts[role] + ']';
    roleCounts[role]++;
    return id;
  }

  // Check if element is visible.
  function isVisible(el) {
    if (!el.offsetParent && el.tagName !== 'BODY' && el.tagName !== 'HTML') {
      // offsetParent is null for hidden elements (display:none) and fixed/body.
      const style = window.getComputedStyle(el);
      if (style.display === 'none') return false;
      if (style.visibility === 'hidden') return false;
    }
    const style = window.getComputedStyle(el);
    if (style.display === 'none') return false;
    if (style.visibility === 'hidden') return false;
    if (parseFloat(style.opacity) === 0) return false;
    return true;
  }

  let focusedRef = null;

  // Recursively walk the DOM.
  function walk(el) {
    // Skip non-element nodes, iframes, SVG internals, and hidden elements.
    if (el.nodeType !== 1) return null;
    if (el.tagName === 'IFRAME' || el.tagName === 'FRAME') return null;
    if (el.tagName === 'SCRIPT' || el.tagName === 'STYLE' || el.tagName === 'NOSCRIPT') return null;

    // Skip SVG child elements (we keep <svg> itself if it has a role).
    if (el instanceof SVGElement && el.tagName !== 'svg') return null;

    if (!isVisible(el)) return null;

    const role = getRole(el);

    // Walk children regardless of whether this element has a role.
    const children = [];
    for (const child of el.children) {
      const result = walk(child);
      if (result) children.push(result);
    }

    // If this element has no role, bubble children up.
    if (!role) {
      if (children.length === 1) return children[0];
      if (children.length > 1) {
        // Return a synthetic group only if there are multiple children.
        return { _bubble: children };
      }
      return null;
    }

    // Get bounds.
    const rect = el.getBoundingClientRect();
    // Skip zero-size elements (but keep elements with role even if small).
    if (rect.width === 0 && rect.height === 0) {
      // Still return children if they exist.
      if (children.length === 1) return children[0];
      if (children.length > 1) return { _bubble: children };
      return null;
    }

    const refId = nextRefId(role);
    const name = getName(el);
    const value = getValue(el);
    const state = getState(el);

    // Flatten any bubbled children.
    const flatChildren = [];
    for (const c of children) {
      if (c._bubble) {
        flatChildren.push(...c._bubble);
      } else {
        flatChildren.push(c);
      }
    }

    const entry = {
      ref_id: refId,
      role: role,
      name: name || null,
      value: value || null,
      state: state,
      bounds: {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
      },
      children: flatChildren,
    };

    if (document.activeElement === el) {
      focusedRef = refId;
    }

    return entry;
  }

  // Walk the body.
  const root = document.body;
  if (!root) {
    return {
      elements: [],
      page_title: document.title || null,
      page_url: location.href,
      focused_ref: null,
    };
  }

  const result = walk(root);

  // Collect top-level elements.
  let elements = [];
  if (result) {
    if (result._bubble) {
      elements = result._bubble;
    } else {
      elements = [result];
    }
  }

  return {
    elements: elements,
    page_title: document.title || null,
    page_url: location.href,
    focused_ref: focusedRef,
  };
})()
"#;

/// Evaluate the ARIA snapshot JavaScript on a page and return structured data.
///
/// The returned [`AriaSnapshot`] contains a tree of [`AriaElement`]s with
/// stable `ref_id` values that can be used for targeting click/type actions.
pub async fn take_aria_snapshot(page: &Page) -> Result<AriaSnapshot, BrowserError> {
    let result = page
        .evaluate(ARIA_SNAPSHOT_JS)
        .await
        .map_err(|e| BrowserError::EvalError(e.to_string()))?;

    let value: serde_json::Value = result
        .into_value()
        .unwrap_or(serde_json::Value::Null);

    serde_json::from_value(value).map_err(|e| {
        BrowserError::EvalError(format!("Failed to deserialize ARIA snapshot: {e}"))
    })
}

/// Resolve a `ref_id` (e.g. `"button[0]"`) to the center point of its
/// bounding rectangle within the given snapshot.
///
/// Returns `None` if the `ref_id` is not found or the element has no bounds.
pub fn resolve_ref_to_point(snapshot: &AriaSnapshot, ref_id: &str) -> Option<(f64, f64)> {
    fn find_in_elements<'a>(elements: &'a [AriaElement], ref_id: &str) -> Option<&'a ElementRect> {
        for el in elements {
            if el.ref_id == ref_id {
                return el.bounds.as_ref();
            }
            if let Some(bounds) = find_in_elements(&el.children, ref_id) {
                return Some(bounds);
            }
        }
        None
    }

    find_in_elements(&snapshot.elements, ref_id).map(|rect| {
        (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_element(ref_id: &str, role: &str, bounds: Option<ElementRect>) -> AriaElement {
        AriaElement {
            ref_id: ref_id.to_string(),
            role: role.to_string(),
            name: None,
            value: None,
            state: vec![],
            bounds,
            children: vec![],
        }
    }

    #[test]
    fn test_resolve_ref_to_point() {
        let snapshot = AriaSnapshot {
            elements: vec![
                make_element(
                    "button[0]",
                    "button",
                    Some(ElementRect {
                        x: 100.0,
                        y: 200.0,
                        width: 80.0,
                        height: 40.0,
                    }),
                ),
                make_element(
                    "textbox[0]",
                    "textbox",
                    Some(ElementRect {
                        x: 50.0,
                        y: 300.0,
                        width: 200.0,
                        height: 30.0,
                    }),
                ),
            ],
            page_title: Some("Test Page".to_string()),
            page_url: Some("https://example.com".to_string()),
            focused_ref: None,
        };

        // button[0]: center = (100 + 80/2, 200 + 40/2) = (140, 220)
        let point = resolve_ref_to_point(&snapshot, "button[0]");
        assert_eq!(point, Some((140.0, 220.0)));

        // textbox[0]: center = (50 + 200/2, 300 + 30/2) = (150, 315)
        let point = resolve_ref_to_point(&snapshot, "textbox[0]");
        assert_eq!(point, Some((150.0, 315.0)));
    }

    #[test]
    fn test_resolve_ref_to_point_nested() {
        // Verify search recurses into children.
        let snapshot = AriaSnapshot {
            elements: vec![AriaElement {
                ref_id: "navigation[0]".to_string(),
                role: "navigation".to_string(),
                name: None,
                value: None,
                state: vec![],
                bounds: Some(ElementRect {
                    x: 0.0,
                    y: 0.0,
                    width: 800.0,
                    height: 60.0,
                }),
                children: vec![make_element(
                    "link[0]",
                    "link",
                    Some(ElementRect {
                        x: 10.0,
                        y: 10.0,
                        width: 60.0,
                        height: 20.0,
                    }),
                )],
            }],
            page_title: None,
            page_url: None,
            focused_ref: None,
        };

        // link[0]: center = (10 + 60/2, 10 + 20/2) = (40, 20)
        let point = resolve_ref_to_point(&snapshot, "link[0]");
        assert_eq!(point, Some((40.0, 20.0)));
    }

    #[test]
    fn test_resolve_ref_not_found() {
        let snapshot = AriaSnapshot {
            elements: vec![],
            page_title: None,
            page_url: None,
            focused_ref: None,
        };

        assert_eq!(resolve_ref_to_point(&snapshot, "button[99]"), None);
    }

    #[test]
    fn test_resolve_ref_no_bounds() {
        let snapshot = AriaSnapshot {
            elements: vec![make_element("img[0]", "img", None)],
            page_title: None,
            page_url: None,
            focused_ref: None,
        };

        // Element exists but has no bounds.
        assert_eq!(resolve_ref_to_point(&snapshot, "img[0]"), None);
    }
}
