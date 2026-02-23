//! Context Resuming Script
//!
//! JavaScript code to resume a frozen browser context by removing the freeze
//! overlay and restoring event handlers.

/// Get the JavaScript code for resuming a browser context
pub fn get_resume_context_script() -> &'static str {
    r#"
(function() {
    // Check if frozen
    if (!window.__aleph_frozen) {
        return { success: true, message: 'Not frozen' };
    }

    try {
        // Remove freeze overlay
        const overlay = document.getElementById('__aleph_freeze_overlay');
        if (overlay) {
            overlay.remove();
        }

        // Remove freeze indicator
        const indicators = document.querySelectorAll('div');
        indicators.forEach(el => {
            if (el.textContent === '⏸ Task Paused') {
                el.remove();
            }
        });

        // Restore original timer functions
        if (window.__aleph_original_setTimeout) {
            window.setTimeout = window.__aleph_original_setTimeout;
            delete window.__aleph_original_setTimeout;
        }
        if (window.__aleph_original_setInterval) {
            window.setInterval = window.__aleph_original_setInterval;
            delete window.__aleph_original_setInterval;
        }
        if (window.__aleph_original_requestAnimationFrame) {
            window.requestAnimationFrame = window.__aleph_original_requestAnimationFrame;
            delete window.__aleph_original_requestAnimationFrame;
        }

        // Remove event blockers
        if (window.__aleph_event_blocker) {
            ['click', 'mousedown', 'mouseup', 'mousemove', 'keydown', 'keyup',
             'keypress', 'touchstart', 'touchend', 'touchmove', 'wheel', 'scroll',
             'focus', 'blur', 'input', 'change', 'submit'].forEach(eventType => {
                document.removeEventListener(eventType, window.__aleph_event_blocker, true);
            });
            delete window.__aleph_event_blocker;
        }

        // Clear frozen timers
        if (window.__aleph_frozen_timers) {
            delete window.__aleph_frozen_timers;
        }

        // Mark as not frozen
        window.__aleph_frozen = false;

        return { success: true, message: 'Context resumed successfully' };
    } catch (error) {
        return { success: false, message: error.toString() };
    }
})();
    "#
}
