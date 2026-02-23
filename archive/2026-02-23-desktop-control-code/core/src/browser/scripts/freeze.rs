//! Context Freezing Script
//!
//! JavaScript code to freeze a browser context by blocking user interactions
//! and pausing JavaScript execution.

/// Get the JavaScript code for freezing a browser context
pub fn get_freeze_context_script() -> &'static str {
    r#"
(function() {
    // Check if already frozen
    if (window.__aleph_frozen) {
        return { success: true, message: 'Already frozen' };
    }

    try {
        // Create freeze overlay
        const overlay = document.createElement('div');
        overlay.id = '__aleph_freeze_overlay';
        overlay.style.cssText = `
            position: fixed;
            top: 0;
            left: 0;
            width: 100%;
            height: 100%;
            background: rgba(0, 0, 0, 0.1);
            z-index: 2147483647;
            cursor: not-allowed;
            pointer-events: all;
        `;

        // Add freeze indicator
        const indicator = document.createElement('div');
        indicator.style.cssText = `
            position: fixed;
            top: 20px;
            right: 20px;
            background: rgba(255, 165, 0, 0.9);
            color: white;
            padding: 10px 20px;
            border-radius: 5px;
            font-family: system-ui, -apple-system, sans-serif;
            font-size: 14px;
            font-weight: 500;
            z-index: 2147483647;
            box-shadow: 0 2px 10px rgba(0,0,0,0.3);
        `;
        indicator.textContent = '⏸ Task Paused';

        document.body.appendChild(overlay);
        document.body.appendChild(indicator);

        // Store timer IDs to clear them
        window.__aleph_frozen_timers = [];

        // Override setTimeout/setInterval to prevent new timers
        window.__aleph_original_setTimeout = window.setTimeout;
        window.__aleph_original_setInterval = window.setInterval;
        window.__aleph_original_requestAnimationFrame = window.requestAnimationFrame;

        window.setTimeout = function() {
            return 0;
        };
        window.setInterval = function() {
            return 0;
        };
        window.requestAnimationFrame = function() {
            return 0;
        };

        // Block all events
        const blockEvent = (e) => {
            e.stopPropagation();
            e.preventDefault();
            return false;
        };

        window.__aleph_event_blocker = blockEvent;

        ['click', 'mousedown', 'mouseup', 'mousemove', 'keydown', 'keyup',
         'keypress', 'touchstart', 'touchend', 'touchmove', 'wheel', 'scroll',
         'focus', 'blur', 'input', 'change', 'submit'].forEach(eventType => {
            document.addEventListener(eventType, blockEvent, true);
        });

        // Mark as frozen
        window.__aleph_frozen = true;

        return { success: true, message: 'Context frozen successfully' };
    } catch (error) {
        return { success: false, message: error.toString() };
    }
})();
    "#
}
