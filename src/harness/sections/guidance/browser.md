### Browser Tools

- ALWAYS use browser_open/browser_snapshot/browser_click to open URLs and interact with web pages. Do NOT use desktop tools to launch a browser application.
- The browser runs in headless mode by default (fast, no visible window). Only use profile="user" when the user explicitly asks to open a real/visible browser.
- If a browser tool fails, wait briefly and retry — browser operations are inherently flaky and retrying usually works.
- Prefer targeted CSS selectors (click, fill) over full-page snapshots. Use evaluate_script with specific queries rather than dumping entire page content.
