//! Accessibility Tree Walker Script
//!
//! JavaScript code to extract accessibility information from a web page.

/// Get the JavaScript code for extracting accessibility tree
pub fn get_accessibility_tree_script() -> &'static str {
    r#"
(function() {
    const nodes = [];
    const walk = (node, depth) => {
        if (depth > 10) return; // Limit depth

        const role = node.getAttribute?.('role') || node.tagName?.toLowerCase() || '';
        const ariaLabel = node.getAttribute?.('aria-label') || '';
        const innerText = node.innerText?.slice(0, 100) || '';

        const interactive = ['a', 'button', 'input', 'select', 'textarea'].includes(node.tagName?.toLowerCase()) ||
            node.getAttribute?.('onclick') ||
            node.getAttribute?.('role') === 'button' ||
            node.getAttribute?.('role') === 'link';

        if (role || ariaLabel || (interactive && innerText)) {
            nodes.push({
                role: role || node.tagName?.toLowerCase() || 'generic',
                name: ariaLabel || innerText.trim().slice(0, 50),
                value: node.value || null,
                depth: depth,
                interactive: interactive,
                tagName: node.tagName?.toLowerCase() || '',
            });
        }

        if (nodes.length < 500) { // Limit total nodes
            for (const child of node.children || []) {
                walk(child, depth + 1);
            }
        }
    };
    walk(document.body, 0);
    return nodes;
})()
    "#
}
