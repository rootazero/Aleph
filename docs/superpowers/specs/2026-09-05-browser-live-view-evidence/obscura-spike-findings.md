# obscura spike findings (throwaway; 2026-09-05)

Binary: obscura v0.2.1 release (2026-08-23) aarch64-macos, sha256 verified against GitHub API digest.
Local source clone is 72c84ad (2026-09-04) — 12 days newer than the release.
Probes: probe.mjs (block/full), probe2.mjs, probe3.mjs; raw results in results-*.json; screenshots shot-{0,1,2}.png.
Network on this machine goes through a fake-ip TUN proxy (github.com -> 198.18.0.9); nav timings are network-dominated.

## Decisive
1. Targets are scoped per CDP connection (CdpContext per socket). A second connection sees Target.targetCreated
   broadcasts but Target.getTargets is [] and attachToTarget -> "Target not found", even with setDiscoverTargets
   and connecting first. => playwright-cli (conn 1) + Aleph observer (conn 2) is impossible. Only a single
   Aleph-owned connection can serve both tools and viewer.
2. Single V8 isolate: on github.com, Runtime.evaluate("1") RTT = 16.8 s right after load, 12.4 s after 8 s settle.
   The AX tree itself takes 18-39 ms once it holds the lock (the 13 s "AX time" in run 1 was lock wait).
   Any CDP command needing JS (evaluate, input dispatch, hit-test) stalls while page scripts run.

## Screencast (single session, 1280x800 JPEG q60)
- Activity-driven: 0 frames in 3 s on a static page; ~31 fps on an animated page.
- 28-33 KB/frame => ~1 MB/s at full motion. Static example.com: jpeg q30/60/90 = 32/35/41 KB, png 17 KB.
- Click -> next frame: 28, 13, 13, 14, 13 ms (local).
- Survives Page.navigate without restart (frames kept arriving across 3 navigations).

## Input
- keyDown+text types ASCII; `char` event inserts CJK ("ab中"); rawKeyDown+windowsVirtualKeyCode=8 deletes.
- mousePressed/Released clicks; mouseWheel scrolls (scrollY 600).
- Input.insertText: unknown in v0.2.1; present in source (input.rs, commit e4814b4 2026-09-04).
- No Page.javascriptDialogOpening / handleJavaScriptDialog; no drag events (source grep).

## AX tree (Accessibility.getFullAXTree)
- Every node carries backendDOMNodeId; ignored=0 (no pruning); HN 1305 / GitHub 5551 / Wikipedia 4004 nodes.
- Accessible name is NOT computed from content: HN links 0/229 have a name (children StaticText carry the text).
  Raw link node keys: nodeId, ignored, role, parentId, properties, childIds, backendDOMNodeId (no `name`).
  A snapshot generator must compute name-from-content itself.

## Hit-testing (for a future element picker)
- document.elementFromPoint via Runtime.evaluate works; DOM.getDocument + DOM.querySelector + DOM.getBoxModel works.

## Private-network floor
- Without --allow-private-network: Page.navigate to 127.0.0.1 -> error "Network error: Access to private/internal
  IP address 127.0.0.1 is not allowed"; page stays about:blank. Process-global flag.

## Runtime.evaluate is expression-only
- "1; 2" -> SyntaxError; "(1, 2)", IIFE, awaitPromise all fine. Chrome accepts multi-statement scripts.
  Aleph's wait_probe_func emits arrow functions (fine via callFunctionOn); browser_evaluate's free-form
  scripts need IIFE wrapping on this driver.

## Rendering fidelity (screenshots)
- HN, Wikipedia: near-Chrome. GitHub: recognizable but artifacts — nav labels double-painted ("Puull requests"),
  a phantom hover tooltip, empty commit-message column, README not rendered.

## Misc
- Browser.getVersion reports Chrome/145 with a Linux X11 UA even in the non-stealth build.
- Binary 94 MB (+86 MB obscura-worker, only for `scrape`). RSS not measured.
- Release asset host was DNS-blocked on this network; DoH-resolved --resolve download worked; digest verified.
