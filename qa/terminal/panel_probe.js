// Machine-checkable probes for `qa/terminal/run.sh panel`.
//
// Paste one expression into chrome-devtools-mcp `evaluate_script` (or
// claude-in-chrome `javascript_tool`). Every probe returns a VALUE that an
// assertion can be written against — never "the click happened". The terminal
// screen is a `<canvas>` (`views/terminal/mod.rs`), so the cursor and the
// screen text are pixel questions, not DOM ones, and the probes say so
// instead of pretending a DOM assertion exists.
//
// Anchors used, all already in the shipped markup:
//   [data-terminal-view]  the terminal panel root
//   [data-terminal-tabs]  the tab strip
//   canvas                the screen
//   [aria-selected]       which tab is selected

globalThis.qaTerm = {
  // --- tabs -------------------------------------------------------------
  //
  // `tabs.rs::title_prefers_osc_then_program_then_shell` is the unit test.
  // On metal the claim is: a session SPAWNED as `sh` that then runs an agent
  // must show the AGENT, not `sh` — the same falsifier `stage_identify` uses,
  // because a tab reading `sh` is what the phase-1 defect looked like.
  tabs() {
    const strip = document.querySelector("[data-terminal-tabs]");
    if (!strip) return { error: "no [data-terminal-tabs] — is the terminal panel open?" };
    return Array.from(strip.querySelectorAll("button[aria-selected]")).map((b) => ({
      title: b.querySelector("span:last-child")?.textContent ?? "",
      program: b.getAttribute("title") ?? "",
      selected: b.getAttribute("aria-selected") === "true",
    }));
  },

  // --- row click --------------------------------------------------------
  //
  // `agent_panel.rs::agent_row_click_selects_the_session_and_switches_mode`
  // proves the helper and greps its own source for the `on:click`. Neither
  // half can see a click that reaches no handler in a real build, which is
  // what this reports: route AFTER the click, plus which tab ended selected.
  route() {
    return {
      path: location.pathname + location.search + location.hash,
      terminalPanelMounted: !!document.querySelector("[data-terminal-view]"),
      selectedTab: (this.tabs() || []).find?.((t) => t.selected) ?? null,
    };
  },

  // --- cursor visibility -------------------------------------------------
  //
  // `session.rs::cursor_visible_false_is_stored_and_render_skips_the_cursor`
  // stops at the model. `render.rs::cursor_rect` returns `None` when hidden,
  // so on metal the question is whether a block of pixels disappeared.
  //
  // Sample the whole canvas and count non-background pixels. Call once with
  // the cursor showing, once after `printf '\033[?25l'`, and compare: the
  // count must DROP. Comparing to a literal would be a number nobody can
  // maintain across fonts and DPI (判据 §18).
  inkCount() {
    const c = document.querySelector("[data-terminal-view] canvas");
    if (!c) return { error: "no canvas under [data-terminal-view]" };
    const ctx = c.getContext("2d", { willReadFrequently: true });
    if (!ctx) return { error: "no 2d context" };
    const { data, width, height } = ctx.getImageData(0, 0, c.width, c.height);
    // The background is the MOST COMMON colour, not pixel (0,0).
    //
    // (0,0) was the first version and it is wrong in the one case this probe
    // exists for: the top-left cell is exactly where a cursor sits on a fresh
    // screen, and where the first character lands. Sampling it makes the ink
    // the background and the background the ink — the count INVERTS, and an
    // inverted count still moves when you hide the cursor, so the assertion
    // "hidden < before" would have gone on passing in the wrong direction.
    // Caught by driving this function over a synthetic ImageData whose ink
    // was deliberately at the origin (7 ink pixels of 100 were reported as
    // 93).
    const counts = new Map();
    for (let i = 0; i < data.length; i += 4) {
      // 5 bits per channel: exact equality would make anti-aliased text its
      // own colour a thousand times over and no colour a majority.
      const key = ((data[i] >> 3) << 10) | ((data[i + 1] >> 3) << 5) | (data[i + 2] >> 3);
      counts.set(key, (counts.get(key) || 0) + 1);
    }
    let bgKey = 0;
    let best = -1;
    for (const [k, n] of counts) {
      if (n > best) {
        best = n;
        bgKey = k;
      }
    }
    const bg = [((bgKey >> 10) & 31) << 3, ((bgKey >> 5) & 31) << 3, (bgKey & 31) << 3];
    let ink = 0;
    for (let i = 0; i < data.length; i += 4) {
      if (
        Math.abs(data[i] - bg[0]) > 12 ||
        Math.abs(data[i + 1] - bg[1]) > 12 ||
        Math.abs(data[i + 2] - bg[2]) > 12
      ) {
        ink++;
      }
    }
    return {
      ink,
      width,
      height,
      bg,
      bgShare: best / (data.length / 4),
      cssWidth: c.clientWidth,
      cssHeight: c.clientHeight,
    };
  },

  // --- paste -------------------------------------------------------------
  //
  // `keymap.rs::cmd_v_and_ctrl_shift_v_are_left_to_the_browser_ctrl_v_is_0x16`
  // is the unit test, and it is the one that most needs metal: "left to the
  // browser" is a claim ABOUT THE BROWSER, and no unit test has one.
  //
  // This does not synthesise the paste — a synthetic ClipboardEvent proves
  // nothing about whether the real one is preventDefault()ed. Drive the real
  // Cmd+V from the MCP client; this only reports what reached the screen, by
  // ink delta, plus whether a paste handler is even registered.
  pasteSurface() {
    const view = document.querySelector("[data-terminal-view]");
    if (!view) return { error: "no [data-terminal-view]" };
    return {
      focused: document.activeElement?.tagName ?? null,
      focusInsideTerminal: view.contains(document.activeElement),
      ink: this.inkCount().ink,
    };
  },
};
"qaTerm ready: " + Object.keys(globalThis.qaTerm).join(", ");
