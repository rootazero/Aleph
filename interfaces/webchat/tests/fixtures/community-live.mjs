// Test-only single-file module; admission still uses its real manifest and SRI.
globalThis.__livePanelImports = (globalThis.__livePanelImports || 0) + 1;
export function describe() {
  return {
    key: { plugin_id: 'community-live', tab_id: 'overview' },
    label: 'Overview', icon: 'layout', order: 0,
    entry: './tabs/community-live/1.0.0/ui/panel.mjs',
    api_major: 1, trust: 'trusted', permissions: [], status: 'enabled',
  };
}
export function mount(sidebar, content) {
  globalThis.__livePanelMounts = (globalThis.__livePanelMounts || 0) + 1;
  sidebar.textContent = 'Live community sidebar';
  content.textContent = 'Live Gateway community content';
  let disposed = false;
  return { unmount() {
    if (disposed) {
      globalThis.__livePanelDuplicateCleanup = (globalThis.__livePanelDuplicateCleanup || 0) + 1;
      throw Error('duplicate cleanup');
    }
    disposed = true;
    globalThis.__livePanelUnmounts = (globalThis.__livePanelUnmounts || 0) + 1;
    sidebar.textContent = ''; content.textContent = '';
    if (globalThis.__livePanelThrowOnUnmount) throw Error('intentional cleanup failure');
  } };
}
