export function describe() {
  return {
    key: { plugin_id: "community.fixture", tab_id: "verified" },
    label: "Verified community fixture",
    icon: "layout",
    order: 10,
    entry: "./tabs/community.fixture/1.0.0/verified.mjs",
    api_major: 1,
    trust: "trusted",
    permissions: [],
    status: "enabled",
  };
}

export function mount(sidebar, content) {
  sidebar.textContent = "community fixture sidebar";
  content.textContent = "community fixture content";
  let disposed = false;
  return {
    unmount() {
      if (disposed) return;
      disposed = true;
      sidebar.textContent = "";
      content.textContent = "";
    },
  };
}
