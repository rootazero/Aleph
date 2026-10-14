// Gate lib-dead (test-only) methods with #[cfg(test)]; fix stale dead-code
// allowances on the production LiveViewerBackend.
const fs = require('fs');
function edit(file, pairs) {
  let s = fs.readFileSync(file, 'utf8');
  const crlf = s.includes('\r\n');
  s = s.replace(/\r\n/g, '\n');
  for (const [a, b] of pairs) {
    const n = s.split(a).length - 1;
    if (n !== 1) throw new Error(`${file}: ${n} matches for ${JSON.stringify(a.slice(0, 60))}`);
    s = s.replace(a, b);
  }
  fs.writeFileSync(file, crlf ? s.replace(/\n/g, '\r\n') : s);
  console.log('edited', file);
}
const T = '    #[cfg(test)] // only tests read this; production has no caller\n';
edit('src/browser/cdp_backend/operation.rs', [
  ['    pub(crate) fn target_id(&self) -> &str {', T + '    pub(crate) fn target_id(&self) -> &str {'],
  ['    pub(crate) fn kind(&self) -> BrowserOperationKind {', T + '    pub(crate) fn kind(&self) -> BrowserOperationKind {'],
  ['    pub(crate) fn engine(&self) -> Engine {', T + '    pub(crate) fn engine(&self) -> Engine {'],
  ['    pub(crate) fn engine_pid(&self) -> u32 {', T + '    pub(crate) fn engine_pid(&self) -> u32 {'],
]);
edit('src/browser/live/mod.rs', [
  ['    pub(crate) async fn attach_target(', T + '    pub(crate) async fn attach_target('],
  ['    pub(crate) async fn close_target(&self, target_id: &str)', T + '    pub(crate) async fn close_target(&self, target_id: &str)'],
  ['    pub(crate) async fn dispatch_human_input(', '    // Test-only: Panel/TUI viewers are read-only; no production input path.\n    #[cfg(test)]\n    pub(crate) async fn dispatch_human_input('],
  ['    #[allow(dead_code)] // consumed by the Task 6 boot wiring (LiveViewerBackend)\n', ''],
]);
edit('src/browser/live/control.rs', [
  ['    pub(crate) fn pending_count(&self) -> usize {', T + '    pub(crate) fn pending_count(&self) -> usize {'],
  ['    pub(crate) fn revoke_human_permission(', T + '    pub(crate) fn revoke_human_permission('],
]);
edit('src/browser/live/stream_hub.rs', [
  ['    #[must_use]\n    pub fn skipped(&self) -> u64 {', '    #[cfg(test)] // only tests read this; production has no caller\n    #[must_use]\n    pub fn skipped(&self) -> u64 {'],
]);
edit('src/browser/live/viewer_backend.rs', [
  ['//! implementation owns an [`LiveProfileOwner`].', '//! implementation owns an [`super::LiveProfileOwner`].'],
  ['use super::{AttachmentLease, LiveProfileOwner};', 'use super::AttachmentLease;'],
  ['/// Production backend over a [`LiveProfileOwner`].\n///\n/// Constructed by the boot wiring that owns the browser engine handle\n/// (a later task); until then nothing in production builds one, so the\n/// struct keeps its dead-code allowance.\n#[allow(dead_code)]\n', '/// Production backend over a [`super::LiveProfileOwner`].\n///\n/// Constructed by `gateway::server::browser_live_runtime` when an explicit,\n/// valid `[gateway] browser_viewer_origin` installs the browser-live runtime.\n'],
  ['#[allow(dead_code)] // constructed by the Task 6 boot wiring\nimpl LiveViewerBackend {', 'impl LiveViewerBackend {'],
  ['    use super::super::tests::fixture;\n    use super::*;\n', '    use super::super::tests::fixture;\n    use super::super::LiveProfileOwner;\n    use super::*;\n'],
]);
