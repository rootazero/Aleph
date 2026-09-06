# T0 results — spec §11 U1–U6 and U9, measured 2026-09-06

Produced by `probes/t0-run.sh` → `probes/t0-report.mjs`. Every number below is a probe's
own output; the raw JSON stays in the session scratchpad (evidence README: raw outputs are
not committed). Re-run: `bash probes/t0-run.sh`.

| U | verdict |
|---|---|
| U1 | confirms R14: `--port 0` yields no ownable endpoint (banner said "ws://127.0.0.1:0/devtools/browser", which names port 0 — not a port at all), while an Aleph-picked port (52432) answers /json/version 200 and its listener pid IS the launched pid. The banner is evidence, never an endpoint (it also did name the requested port on the fixed-port run) ⇒ §6.2 keeps only «Aleph allocates the port + verifies ownership». |
| U2 | obscura: `display:none` ⇒ a box IS returned, content quad [0,0,0,0,0,0,0,0] (4 distinct quads across the four not-laid-out cases) — a getBoxModel failure is NOT a visibility signal on this engine; the interim fetcher must read `computed.display_none`. · chrome: `display:none` ⇒ honest failure, error text "Could not compute box model." (only some fail). |
| U3 | SAME-ORIGIN: obscura same-origin (load confirmed via contentDocument.title="T0 child frame"): pierce:true does NOT carry iframe content (0 contentDocument(s), child `#probe` absent, 88 nodes) — confirmed via obscura source (read-only, /Volumes/TBU4/Github/obscura): DOM.getDocument's handler (obscura-cdp/src/domains/dom.rs:96-103) reads only `depth`, never `pierce` — unimplemented, not merely unhonoured. serialize_node (dom.rs:454-520) walks a single DomTree with no contentDocument or frame-crossing branch, sourced from Page::with_dom -> ObscuraJsRuntime::with_dom -> state.dom (obscura-js/src/runtime.rs:3311-3312), the TOP-LEVEL page's own document only — even though a child iframe genuinely has its own separate DOM tree internally (FrameRealm, obscura-js/src/frame.rs:34). No code path branches on origin, so same-origin and cross-origin behave identically: neither is reachable from DOM.getDocument. \|\| CROSS-ORIGIN: NOT APPLICABLE on obscura: confirmed via source that obscura has no out-of-process (or even per-frame-target) concept at all — Target.setAutoAttach is a literal Ok({}) no-op that never registers anything (obscura-cdp/src/domains/target.rs:240), and Target.getTargets / Target.attachedToTarget only ever represent top-level "page" targets (target.rs:47-64, 100-140); an iframe — same-origin or cross-origin — is never its own CDP target, because every frame shares one V8 isolate by construction ("Staying in one isolate is what lets same-origin frames share objects with their parent", obscura-js/src/frame.rs:20-21). There is no cross-origin/same-origin distinction in obscura's process model to measure, so this half is retired as not-applicable rather than left dangling as unmeasured. · SAME-ORIGIN: chrome same-origin (in-process child, load confirmed via contentDocument.title="T0 child frame"): pierce:true DOES carry iframe content (1 contentDocument(s), child `#probe` present, 101 nodes, 3 carry a frameId) ⇒ the interim fetcher CAN flatten a same-process child from this one call. \|\| CROSS-ORIGIN: chrome cross-origin OOPIF (load confirmed via the child's own session, document.title="T0 child frame"): pierce:true does NOT carry iframe content (0 contentDocument(s), child `#probe` absent, 87 nodes) ⇒ the interim fetcher must attach a session per frame for cross-origin children. |
| U4 | Chrome same-origin (in-process) child: documents[1] bounds are FRAME-LOCAL — #probe reads [30,40,150,20] against a frame-local expectation of [30,40,150,20] ⇒ `fetch_chromium`'s same-process path must add the parent iframe's own rect as the child's offset. · confirmed: the parent session's own captureSnapshot sees only its own document (contentDocumentIndex is empty for the iframe's owner node, i.e. nothing on the parent side points at the child at all). The child's OWN captureSnapshot (its own session) returns its own document with #probe at [30,40,150,20] — exactly frame-local, matching the static page. Join key: parent iframe node's `frameId` = "69F3C410D5C580F9AAD324EC395F5E1D", OOPIF target's `targetId` = "69F3C410D5C580F9AAD324EC395F5E1D", child's own document `frameId` = "69F3C410D5C580F9AAD324EC395F5E1D" — all three are the SAME value ⇒ `fetch_chromium` places a child by matching the parent DOM node's `frameId` (from DOM.getDocument, since DOMSnapshot's own node has no such field) against the auto-attached child target's `targetId`, which is also the child's own document `frameId`. `fetch_chromium` must therefore: (1) enumerate iframe sub-targets via Target.setAutoAttach, (2) capture each child session's own DOMSnapshot separately, (3) place each child's frame-local bounds by adding the OWNER `<iframe>` element's own rect (from the PARENT's snapshot, looked up by this join key) as the offset — the same arithmetic as the same-origin path, just sourced from two separate captures instead of one. |
| U5 | obscura: Input.dispatchMouseEvent takes VIEWPORT coordinates (hit at y=785 with scrollY=1548, miss at y=2333) ⇒ `ActionTarget::Coordinates` (page coords) converts by subtracting scroll. · chrome: Input.dispatchMouseEvent takes VIEWPORT coordinates (hit at y=785 with scrollY=1556, miss at y=2341) ⇒ `ActionTarget::Coordinates` (page coords) converts by subtracting scroll. |
| U6 | obscura: 8/8 cookies came back; sameSite kept for [t0_lax,t0_strict,t0_none_insecure]; `expires` round-trips; fields present on a returned cookie: ["name","value","domain","path","expires","size","httpOnly","secure","session","sameSite","sameParty","sourceScheme","sourcePort","priority"]. · chrome: 7/8 cookies came back; sameSite kept for [t0_lax,t0_strict]; `expires` round-trips; fields present on a returned cookie: ["name","value","domain","path","expires","size","httpOnly","secure","session","priority","sourceScheme","sourcePort"]. |
| U7 | deferred to Task 14 (`ALEPH_QA_DRIVER=playwright_cli` baseline, then `=cdp`). |
| U8 | deferred to Task 20 (upstream `domsnapshot.rs` diff + `render-repros`). |
| U9 | obscura: only-a-in-p,a-in-td,span-in-block-have-boxes (no box for [a-in-body]); all three sources agree on every placement; HN: hn-title-link box {"x":130,"y":11,"w":83,"h":15,"scrollX":0,"scrollY":0}, hn-first-story box {"x":136,"y":44,"w":136,"h":15,"scrollX":0,"scrollY":0}. · chrome: all-inline-have-boxes; all three sources agree on every placement; HN: hn-title-link box {"x":130,"y":12,"w":98,"h":16,"scrollX":0,"scrollY":0}, hn-first-story box {"x":139,"y":44,"w":156,"h":16,"scrollX":0,"scrollY":0}. |

## capture-chrome

```json
{
  "engine": "chrome",
  "wsUrl": "ws://127.0.0.1:52783/devtools/browser/a20ed41f-e159-4ee6-a5f1-f25f1515d40c",
  "files": [
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Browser.getVersion.json",
      "bytes": 301
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Target.getTargets.json",
      "bytes": 1943
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Target.createTarget.json",
      "bytes": 53
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Target.closeTarget.json",
      "bytes": 22
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Page.navigate.json",
      "bytes": 125
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Page.getFrameTree.json",
      "bytes": 1319
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Page.getLayoutMetrics.json",
      "bytes": 750
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Page.getNavigationHistory.json",
      "bytes": 408
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOM.getDocument.json",
      "bytes": 51235
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOM.querySelector.json",
      "bytes": 19
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOM.querySelector.miss.json",
      "bytes": 18
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOM.querySelectorAll.json",
      "bytes": 38
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOM.describeNode.json",
      "bytes": 260
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOM.getBoxModel.json",
      "bytes": 713
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOM.resolveNode.json",
      "bytes": 180
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOM.getBoxModel.hidden.json",
      "bytes": 87
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOM.getBoxModel.badnode.json",
      "bytes": 93
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Runtime.evaluate.json",
      "bytes": 83
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Runtime.evaluate.throws.json",
      "bytes": 759
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Runtime.callFunctionOn.json",
      "bytes": 62
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Network.getAllCookies.json",
      "bytes": 329
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Network.getCookies.json",
      "bytes": 329
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-DOMSnapshot.captureSnapshot.json",
      "bytes": 45048
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Page.captureScreenshot.json",
      "bytes": 301
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-Page.printToPDF.json",
      "bytes": 1445
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/chrome-void.json",
      "bytes": 894
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/src/browser/engine/fixtures/t0-support-matrix.json",
      "bytes": 10024
    }
  ],
  "totalBytes": 116838,
  "oversize": [],
  "unsupported": [
    [
      "DOM.getBoxModel.hidden",
      {
        "protocol": "Could not compute box model.",
        "effect": null
      }
    ],
    [
      "DOM.getBoxModel.badnode",
      {
        "protocol": "No node found for given backend id",
        "effect": null
      }
    ]
  ],
  "reportsSuccessButDoesNothing": [
    "Input.dispatchTouchEvent",
    "Input.dispatchDragEvent"
  ],
  "verdict": "chrome: 27 fixtures, 116838 bytes total, 2 method(s) not answered: {\"DOM.getBoxModel.hidden\":{\"protocol\":\"Could not compute box model.\",\"effect\":null},\"DOM.getBoxModel.badnode\":{\"protocol\":\"No node found for given backend id\",\"effect\":null}}"
}
```

## capture-obscura

```json
{
  "engine": "obscura",
  "wsUrl": "ws://127.0.0.1:55108/devtools/browser",
  "files": [
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/obscura-Browser.getVersion.json",
      "bytes": 270
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/obscura-Target.getTargets.json",
      "bytes": 271
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/obscura-Page.navigate.json",
      "bytes": 87
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/obscura-DOM.getDocument.json",
      "bytes": 34808
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/obscura-DOM.getBoxModel.json",
      "bytes": 497
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/obscura-Runtime.evaluate.json",
      "bytes": 85
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/crates/aleph-cdp/tests/fixtures/obscura-Network.getAllCookies.json",
      "bytes": 380
    },
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/src/browser/engine/fixtures/t0-support-matrix.json",
      "bytes": 10062
    }
  ],
  "totalBytes": 46460,
  "oversize": [],
  "unsupported": [
    [
      "Page.bringToFront",
      {
        "protocol": "Unknown Page method: bringToFront",
        "effect": null
      }
    ],
    [
      "DOM.setFileInputFiles",
      {
        "protocol": "DOM.setFileInputFiles is disabled. Restart with `obscura serve --allow-file-access` to enable local file uploads.",
        "effect": null
      }
    ],
    [
      "Network.emulateNetworkConditions",
      {
        "protocol": "Unknown Network method: emulateNetworkConditions",
        "effect": null
      }
    ],
    [
      "Page.javascriptDialogOpening",
      {
        "protocol": "no event within 4s (wait expired, not a measured absence)",
        "effect": null
      }
    ],
    [
      "Page.handleJavaScriptDialog",
      {
        "protocol": "not reachable: no dialog event",
        "effect": null
      }
    ],
    [
      "Input.dispatchDragEvent",
      {
        "protocol": "Unknown Input method: dispatchDragEvent",
        "effect": false
      }
    ]
  ],
  "reportsSuccessButDoesNothing": [
    "Input.dispatchTouchEvent",
    "DOM.setAttributeValue",
    "DOM.removeNode"
  ],
  "verdict": "obscura: 8 fixtures, 46460 bytes total, 6 method(s) not answered: {\"Page.bringToFront\":{\"protocol\":\"Unknown Page method: bringToFront\",\"effect\":null},\"DOM.setFileInputFiles\":{\"protocol\":\"DOM.setFileInputFiles is disabled. Restart with `obscura serve --allow-file-access` to enable local file uploads.\",\"effect\":null},\"Network.emulateNetworkConditions\":{\"protocol\":\"Unknown Network method: emulateNetworkConditions\",\"effect\":null},\"Page.javascriptDialogOpening\":{\"protocol\":\"no event within 4s (wait expired, not a measured absence)\",\"effect\":null},\"Page.handleJavaScriptDialog\":{\"protocol\":\"not reachable: no dialog event\",\"effect\":null},\"Input.dispatchDragEvent\":{\"protocol\":\"Unknown Input method: dispatchDragEvent\",\"effect\":false}}"
}
```

## hn-chromium

```json
{
  "engine": "chromium",
  "url": "https://news.ycombinator.com/",
  "capturedAt": "2026-09-06T12:41:37.942Z",
  "files": [
    {
      "path": "/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/browser-dual-engine/src/browser/page_state/fixtures/hn-chromium.domsnapshot.json",
      "bytes": 798060
    }
  ],
  "title": "Hacker News",
  "documents": 1,
  "layoutNodes": 1292,
  "totalBytes": 798060,
  "oversize": [],
  "verdict": "chromium HN capture: 1 document(s), 1292 layout nodes, 798060 bytes"
}
```

## u1

```json
{
  "u": "U1",
  "question": "obscura --port 0: is there ever an endpoint we can own?",
  "port_zero": {
    "argv": [
      "/private/tmp/claude-502/-Volumes-TBU4-Workspace-Aleph/cfd2843f-43bd-4fe1-9d69-577277c06fcb/scratchpad/obscura-bin/obscura",
      "serve",
      "--port",
      "0",
      "--storage-dir",
      "/var/folders/qt/d007hr751cl59b3_8sptd8fr0000gp/T/t0-obs-ql8C3v",
      "--allow-private-network"
    ],
    "exit": null,
    "wsUrl": null,
    "banner": {
      "url": "ws://127.0.0.1:0/devtools/browser",
      "port": 0
    },
    "stdout": "\n   ____  _                              \n  / __ \\| |                             \n | |  | | |__  ___  ___ _   _ _ __ __ _ \n | |  | | '_ \\/ __|/ __| | | | '__/ _` |\n | |__| | |_) \\__ \\ (__| |_| | | | (_| |\n  \\____/|_.__/|___/\\___|\\__,_|_|  \\__,_|\n                   \n  Headless Browser v0.2.2\n  CDP server: ws://127.0.0.1:0/devtools/browser\n\n",
    "stderr": "",
    "announcedPort": 0,
    "bannerNamesAUsablePort": false,
    "jsonVersion": null,
    "listenerPids": [],
    "ownedByLaunchedPid": false,
    "usableEndpoint": false
  },
  "fixed_port": {
    "port": 52432,
    "pid": 8133,
    "wsUrl": "ws://127.0.0.1:52432/devtools/browser",
    "exit": null,
    "banner": {
      "url": "ws://127.0.0.1:52432/devtools/browser",
      "port": 52432
    },
    "stdout": "\n   ____  _                              \n  / __ \\| |                             \n | |  | | |__  ___  ___ _   _ _ __ __ _ \n | |  | | '_ \\/ __|/ __| | | | '__/ _` |\n | |__| | |_) \\__ \\ (__| |_| | | | (_| |\n  \\____/|_.__/|___/\\___|\\__,_|_|  \\__,_|\n                   \n  Headless Browser v0.2.2\n  CDP server: ws://127.0.0.1:52432/devtools/browser\n\n",
    "jsonVersion": {
      "status": 200,
      "body": {
        "Browser": "Chrome/145.0.0.0",
        "Protocol-Version": "1.3",
        "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36",
        "V8-Version": "14.5.0.0",
        "WebKit-Version": "537.36",
        "webSocketDebuggerUrl": "ws://127.0.0.1:52432/devtools/browser"
      }
    },
    "listenerPids": [
      8133
    ],
    "ownedByLaunchedPid": true,
    "usableEndpoint": true,
    "bannerAgreesWithTheRequestedPort": true
  },
  "verdict": "confirms R14: `--port 0` yields no ownable endpoint (banner said \"ws://127.0.0.1:0/devtools/browser\", which names port 0 — not a port at all), while an Aleph-picked port (52432) answers /json/version 200 and its listener pid IS the launched pid. The banner is evidence, never an endpoint (it also did name the requested port on the fixed-port run) ⇒ §6.2 keeps only «Aleph allocates the port + verifies ownership»."
}
```

## u2-chrome

```json
{
  "u": "U2",
  "engine": "chrome",
  "families": [
    "127.0.0.1",
    "::1"
  ],
  "cases": {
    "#go": {
      "nodeId": 20,
      "backendNodeId": 23,
      "clientRect": {
        "x": 213,
        "y": 169,
        "w": 34,
        "h": 21,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          220.609375,
          172.453125,
          238.40625,
          172.453125,
          238.40625,
          187.453125,
          220.609375,
          187.453125
        ],
        "padding": [
          214.609375,
          171.453125,
          244.40625,
          171.453125,
          244.40625,
          188.453125,
          214.609375,
          188.453125
        ],
        "border": [
          212.609375,
          169.453125,
          246.40625,
          169.453125,
          246.40625,
          190.453125,
          212.609375,
          190.453125
        ],
        "margin": [
          212.609375,
          169.453125,
          246.40625,
          169.453125,
          246.40625,
          190.453125,
          212.609375,
          190.453125
        ],
        "width": 34,
        "height": 21
      },
      "error": null
    },
    "#hidden-none": {
      "nodeId": 85,
      "backendNodeId": 37,
      "clientRect": {
        "x": 0,
        "y": 0,
        "w": 0,
        "h": 0,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": null,
      "error": "Could not compute box model."
    },
    "#hidden-vis": {
      "nodeId": 137,
      "backendNodeId": 39,
      "clientRect": {
        "x": 0,
        "y": 192,
        "w": 1280,
        "h": 22,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          0,
          192.453125,
          1280,
          192.453125,
          1280,
          214.84375,
          0,
          214.84375
        ],
        "padding": [
          0,
          192.453125,
          1280,
          192.453125,
          1280,
          214.84375,
          0,
          214.84375
        ],
        "border": [
          0,
          192.453125,
          1280,
          192.453125,
          1280,
          214.84375,
          0,
          214.84375
        ],
        "margin": [
          0,
          192.453125,
          1280,
          192.453125,
          1280,
          214.84375,
          0,
          214.84375
        ],
        "width": 1280,
        "height": 22
      },
      "error": null
    },
    "#zero-size": {
      "nodeId": 189,
      "backendNodeId": 41,
      "clientRect": {
        "x": 0,
        "y": 215,
        "w": 0,
        "h": 0,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          0,
          214.84375,
          0,
          214.84375,
          0,
          214.84375,
          0,
          214.84375
        ],
        "padding": [
          0,
          214.84375,
          0,
          214.84375,
          0,
          214.84375,
          0,
          214.84375
        ],
        "border": [
          0,
          214.84375,
          0,
          214.84375,
          0,
          214.84375,
          0,
          214.84375
        ],
        "margin": [
          0,
          214.84375,
          0,
          214.84375,
          0,
          214.84375,
          0,
          214.84375
        ],
        "width": 0,
        "height": 0
      },
      "error": null
    },
    "#offscreen": {
      "nodeId": 241,
      "backendNodeId": 43,
      "clientRect": {
        "x": -9999,
        "y": 0,
        "w": 96,
        "h": 22,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          -9999,
          0,
          -9903.234375,
          0,
          -9903.234375,
          22.390625,
          -9999,
          22.390625
        ],
        "padding": [
          -9999,
          0,
          -9903.234375,
          0,
          -9903.234375,
          22.390625,
          -9999,
          22.390625
        ],
        "border": [
          -9999,
          0,
          -9903.234375,
          0,
          -9903.234375,
          22.390625,
          -9999,
          22.390625
        ],
        "margin": [
          -9999,
          0,
          -9903.234375,
          0,
          -9903.234375,
          22.390625,
          -9999,
          22.390625
        ],
        "width": 96,
        "height": 22
      },
      "error": null
    },
    "#opacity-zero": {
      "nodeId": 293,
      "backendNodeId": 45,
      "clientRect": {
        "x": 0,
        "y": 215,
        "w": 1280,
        "h": 22,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          0,
          214.84375,
          1280,
          214.84375,
          1280,
          237.234375,
          0,
          237.234375
        ],
        "padding": [
          0,
          214.84375,
          1280,
          214.84375,
          1280,
          237.234375,
          0,
          237.234375
        ],
        "border": [
          0,
          214.84375,
          1280,
          214.84375,
          1280,
          237.234375,
          0,
          237.234375
        ],
        "margin": [
          0,
          214.84375,
          1280,
          214.84375,
          1280,
          237.234375,
          0,
          237.234375
        ],
        "width": 1280,
        "height": 22
      },
      "error": null
    }
  },
  "distinct_hidden_quads": 3,
  "hidden_all_errored": false,
  "verdict": "chrome: `display:none` ⇒ honest failure, error text \"Could not compute box model.\" (only some fail)."
}
```

## u2-obscura

```json
{
  "u": "U2",
  "engine": "obscura",
  "families": [
    "127.0.0.1",
    "::1"
  ],
  "cases": {
    "#go": {
      "nodeId": 38,
      "backendNodeId": 38,
      "clientRect": {
        "x": 236,
        "y": 149,
        "w": 31,
        "h": 25,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          236,
          149,
          267,
          149,
          267,
          174,
          236,
          174
        ],
        "padding": [
          236,
          149,
          267,
          149,
          267,
          174,
          236,
          174
        ],
        "border": [
          236,
          149,
          267,
          149,
          267,
          174,
          236,
          174
        ],
        "margin": [
          236,
          149,
          267,
          149,
          267,
          174,
          236,
          174
        ],
        "width": 31,
        "height": 25
      },
      "error": null
    },
    "#hidden-none": {
      "nodeId": 53,
      "backendNodeId": 53,
      "clientRect": {
        "x": 0,
        "y": 0,
        "w": 0,
        "h": 0,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0
        ],
        "padding": [
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0
        ],
        "border": [
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0
        ],
        "margin": [
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0
        ],
        "width": 0,
        "height": 0
      },
      "error": null
    },
    "#hidden-vis": {
      "nodeId": 56,
      "backendNodeId": 56,
      "clientRect": {
        "x": 0,
        "y": 185,
        "w": 1280,
        "h": 23,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          0,
          185,
          1280,
          185,
          1280,
          208,
          0,
          208
        ],
        "padding": [
          0,
          185,
          1280,
          185,
          1280,
          208,
          0,
          208
        ],
        "border": [
          0,
          185,
          1280,
          185,
          1280,
          208,
          0,
          208
        ],
        "margin": [
          0,
          185,
          1280,
          185,
          1280,
          208,
          0,
          208
        ],
        "width": 1280,
        "height": 23
      },
      "error": null
    },
    "#zero-size": {
      "nodeId": 59,
      "backendNodeId": 59,
      "clientRect": {
        "x": 0,
        "y": 207,
        "w": 0,
        "h": 0,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          0,
          207,
          0,
          207,
          0,
          207,
          0,
          207
        ],
        "padding": [
          0,
          207,
          0,
          207,
          0,
          207,
          0,
          207
        ],
        "border": [
          0,
          207,
          0,
          207,
          0,
          207,
          0,
          207
        ],
        "margin": [
          0,
          207,
          0,
          207,
          0,
          207,
          0,
          207
        ],
        "width": 0,
        "height": 0
      },
      "error": null
    },
    "#offscreen": {
      "nodeId": 62,
      "backendNodeId": 62,
      "clientRect": {
        "x": -9999,
        "y": 0,
        "w": 96,
        "h": 22,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          -9999,
          0,
          -9903,
          0,
          -9903,
          22,
          -9999,
          22
        ],
        "padding": [
          -9999,
          0,
          -9903,
          0,
          -9903,
          22,
          -9999,
          22
        ],
        "border": [
          -9999,
          0,
          -9903,
          0,
          -9903,
          22,
          -9999,
          22
        ],
        "margin": [
          -9999,
          0,
          -9903,
          0,
          -9903,
          22,
          -9999,
          22
        ],
        "width": 96,
        "height": 22
      },
      "error": null
    },
    "#opacity-zero": {
      "nodeId": 65,
      "backendNodeId": 65,
      "clientRect": {
        "x": 0,
        "y": 207,
        "w": 1280,
        "h": 22,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "content": [
          0,
          207,
          1280,
          207,
          1280,
          229,
          0,
          229
        ],
        "padding": [
          0,
          207,
          1280,
          207,
          1280,
          229,
          0,
          229
        ],
        "border": [
          0,
          207,
          1280,
          207,
          1280,
          229,
          0,
          229
        ],
        "margin": [
          0,
          207,
          1280,
          207,
          1280,
          229,
          0,
          229
        ],
        "width": 1280,
        "height": 22
      },
      "error": null
    }
  },
  "distinct_hidden_quads": 4,
  "hidden_all_errored": false,
  "verdict": "obscura: `display:none` ⇒ a box IS returned, content quad [0,0,0,0,0,0,0,0] (4 distinct quads across the four not-laid-out cases) — a getBoxModel failure is NOT a visibility signal on this engine; the interim fetcher must read `computed.display_none`."
}
```

## u3-chrome

```json
{
  "u": "U3",
  "engine": "chrome",
  "sameOrigin": {
    "childUrl": "http://127.0.0.1:18999/t0-frame.html",
    "loadCheck": {
      "src": "http://127.0.0.1:18999/t0-frame.html",
      "contentDocumentTitle": "T0 child frame",
      "accessError": null
    },
    "childLoaded": true,
    "ok": true,
    "nodes": 101,
    "contentDocuments": 1,
    "shadowRoots": 7,
    "nodesWithFrameId": 3,
    "sawChildProbe": true,
    "sawChildLink": true,
    "verdict": "chrome same-origin (in-process child, load confirmed via contentDocument.title=\"T0 child frame\"): pierce:true DOES carry iframe content (1 contentDocument(s), child `#probe` present, 101 nodes, 3 carry a frameId) ⇒ the interim fetcher CAN flatten a same-process child from this one call."
  },
  "crossOrigin": {
    "childUrl": "http://localhost:19001/t0-frame.html",
    "childSessionId": "26A6BC62A8E5AA03A7917B69E337F956",
    "childOwnSessionTitle": "T0 child frame",
    "childLoaded": true,
    "frameHostCosmetic": "{\"src\":\"http://localhost:19001/t0-frame.html\",\"w\":400}",
    "ok": true,
    "nodes": 87,
    "contentDocuments": 0,
    "shadowRoots": 7,
    "nodesWithFrameId": 2,
    "sawChildProbe": false,
    "verdict": "chrome cross-origin OOPIF (load confirmed via the child's own session, document.title=\"T0 child frame\"): pierce:true does NOT carry iframe content (0 contentDocument(s), child `#probe` absent, 87 nodes) ⇒ the interim fetcher must attach a session per frame for cross-origin children."
  },
  "verdict": "SAME-ORIGIN: chrome same-origin (in-process child, load confirmed via contentDocument.title=\"T0 child frame\"): pierce:true DOES carry iframe content (1 contentDocument(s), child `#probe` present, 101 nodes, 3 carry a frameId) ⇒ the interim fetcher CAN flatten a same-process child from this one call. || CROSS-ORIGIN: chrome cross-origin OOPIF (load confirmed via the child's own session, document.title=\"T0 child frame\"): pierce:true does NOT carry iframe content (0 contentDocument(s), child `#probe` absent, 87 nodes) ⇒ the interim fetcher must attach a session per frame for cross-origin children."
}
```

## u3-obscura

```json
{
  "u": "U3",
  "engine": "obscura",
  "sameOrigin": {
    "childUrl": "http://127.0.0.1:18999/t0-frame.html",
    "loadCheck": {
      "src": "http://127.0.0.1:18999/t0-frame.html",
      "contentDocumentTitle": "T0 child frame",
      "accessError": null
    },
    "childLoaded": true,
    "ok": true,
    "nodes": 88,
    "contentDocuments": 0,
    "shadowRoots": 0,
    "nodesWithFrameId": 0,
    "sawChildProbe": false,
    "sawChildLink": false,
    "verdict": "obscura same-origin (load confirmed via contentDocument.title=\"T0 child frame\"): pierce:true does NOT carry iframe content (0 contentDocument(s), child `#probe` absent, 88 nodes) — confirmed via obscura source (read-only, /Volumes/TBU4/Github/obscura): DOM.getDocument's handler (obscura-cdp/src/domains/dom.rs:96-103) reads only `depth`, never `pierce` — unimplemented, not merely unhonoured. serialize_node (dom.rs:454-520) walks a single DomTree with no contentDocument or frame-crossing branch, sourced from Page::with_dom -> ObscuraJsRuntime::with_dom -> state.dom (obscura-js/src/runtime.rs:3311-3312), the TOP-LEVEL page's own document only — even though a child iframe genuinely has its own separate DOM tree internally (FrameRealm, obscura-js/src/frame.rs:34). No code path branches on origin, so same-origin and cross-origin behave identically: neither is reachable from DOM.getDocument."
  },
  "crossOrigin": {
    "childUrl": "http://localhost:19001/t0-frame.html",
    "childSessionId": null,
    "childOwnSessionTitle": null,
    "childLoaded": null,
    "frameHostCosmetic": "{\"src\":\"http://localhost:19001/t0-frame.html\",\"w\":400}",
    "ok": true,
    "nodes": 88,
    "contentDocuments": 0,
    "shadowRoots": 0,
    "nodesWithFrameId": 0,
    "sawChildProbe": false,
    "verdict": "NOT APPLICABLE on obscura: confirmed via source that obscura has no out-of-process (or even per-frame-target) concept at all — Target.setAutoAttach is a literal Ok({}) no-op that never registers anything (obscura-cdp/src/domains/target.rs:240), and Target.getTargets / Target.attachedToTarget only ever represent top-level \"page\" targets (target.rs:47-64, 100-140); an iframe — same-origin or cross-origin — is never its own CDP target, because every frame shares one V8 isolate by construction (\"Staying in one isolate is what lets same-origin frames share objects with their parent\", obscura-js/src/frame.rs:20-21). There is no cross-origin/same-origin distinction in obscura's process model to measure, so this half is retired as not-applicable rather than left dangling as unmeasured."
  },
  "verdict": "SAME-ORIGIN: obscura same-origin (load confirmed via contentDocument.title=\"T0 child frame\"): pierce:true does NOT carry iframe content (0 contentDocument(s), child `#probe` absent, 88 nodes) — confirmed via obscura source (read-only, /Volumes/TBU4/Github/obscura): DOM.getDocument's handler (obscura-cdp/src/domains/dom.rs:96-103) reads only `depth`, never `pierce` — unimplemented, not merely unhonoured. serialize_node (dom.rs:454-520) walks a single DomTree with no contentDocument or frame-crossing branch, sourced from Page::with_dom -> ObscuraJsRuntime::with_dom -> state.dom (obscura-js/src/runtime.rs:3311-3312), the TOP-LEVEL page's own document only — even though a child iframe genuinely has its own separate DOM tree internally (FrameRealm, obscura-js/src/frame.rs:34). No code path branches on origin, so same-origin and cross-origin behave identically: neither is reachable from DOM.getDocument. || CROSS-ORIGIN: NOT APPLICABLE on obscura: confirmed via source that obscura has no out-of-process (or even per-frame-target) concept at all — Target.setAutoAttach is a literal Ok({}) no-op that never registers anything (obscura-cdp/src/domains/target.rs:240), and Target.getTargets / Target.attachedToTarget only ever represent top-level \"page\" targets (target.rs:47-64, 100-140); an iframe — same-origin or cross-origin — is never its own CDP target, because every frame shares one V8 isolate by construction (\"Staying in one isolate is what lets same-origin frames share objects with their parent\", obscura-js/src/frame.rs:20-21). There is no cross-origin/same-origin distinction in obscura's process model to measure, so this half is retired as not-applicable rather than left dangling as unmeasured."
}
```

## u4

```json
{
  "u": "U4",
  "engine": "chrome",
  "flags": [
    "--site-per-process"
  ],
  "sameOrigin": {
    "childUrl": "http://127.0.0.1:18999/t0-frame.html",
    "iframeRectInParent": {
      "x": 200,
      "y": 120,
      "w": 400,
      "h": 200
    },
    "ok": true,
    "documentCount": 2,
    "probeBoundsInChildDoc": [
      30,
      40,
      150,
      20
    ],
    "expected_frame_local": [
      30,
      40,
      150,
      20
    ],
    "expected_page_absolute": [
      230,
      160,
      150,
      20
    ],
    "verdict": "Chrome same-origin (in-process) child: documents[1] bounds are FRAME-LOCAL — #probe reads [30,40,150,20] against a frame-local expectation of [30,40,150,20] ⇒ `fetch_chromium`'s same-process path must add the parent iframe's own rect as the child's offset."
  },
  "oopif": {
    "parentUrl": "http://127.0.0.1:18999/t0-page.html",
    "childUrl": "http://localhost:19001/t0-frame.html",
    "crossOrigin": true,
    "iframeRectInParent": {
      "x": 200,
      "y": 120,
      "w": 400,
      "h": 200
    },
    "childSessionId": "13E43998383237B44AC4CE27CE41BBBA",
    "childTargetId": "69F3C410D5C580F9AAD324EC395F5E1D",
    "parentIframeNodeFrameId": "69F3C410D5C580F9AAD324EC395F5E1D",
    "parentDocumentCount": 1,
    "parentDocumentFrameId": "A9A5F4A04C3B9665BD5344A0C8AA1E73",
    "parentContentDocumentIndexEmpty": true,
    "childDocumentCount": 1,
    "childDocumentFrameId": "69F3C410D5C580F9AAD324EC395F5E1D",
    "probeBoundsInChildOwnSnapshot": [
      30,
      40,
      150,
      20
    ],
    "joinKey": {
      "parentIframeNode_frameId": "69F3C410D5C580F9AAD324EC395F5E1D",
      "childTarget_targetId": "69F3C410D5C580F9AAD324EC395F5E1D",
      "childDocument_frameId": "69F3C410D5C580F9AAD324EC395F5E1D",
      "iframeNodeFrameId_equals_childTargetId": true,
      "iframeNodeFrameId_equals_childDocumentFrameId": true,
      "childTargetId_equals_childDocumentFrameId": true
    },
    "verdict": "confirmed: the parent session's own captureSnapshot sees only its own document (contentDocumentIndex is empty for the iframe's owner node, i.e. nothing on the parent side points at the child at all). The child's OWN captureSnapshot (its own session) returns its own document with #probe at [30,40,150,20] — exactly frame-local, matching the static page. Join key: parent iframe node's `frameId` = \"69F3C410D5C580F9AAD324EC395F5E1D\", OOPIF target's `targetId` = \"69F3C410D5C580F9AAD324EC395F5E1D\", child's own document `frameId` = \"69F3C410D5C580F9AAD324EC395F5E1D\" — all three are the SAME value ⇒ `fetch_chromium` places a child by matching the parent DOM node's `frameId` (from DOM.getDocument, since DOMSnapshot's own node has no such field) against the auto-attached child target's `targetId`, which is also the child's own document `frameId`. `fetch_chromium` must therefore: (1) enumerate iframe sub-targets via Target.setAutoAttach, (2) capture each child session's own DOMSnapshot separately, (3) place each child's frame-local bounds by adding the OWNER `<iframe>` element's own rect (from the PARENT's snapshot, looked up by this join key) as the offset — the same arithmetic as the same-origin path, just sourced from two separate captures instead of one."
  },
  "origins": {
    "parent": "http://127.0.0.1:18999",
    "child": "http://localhost:19001"
  },
  "verdict": "Chrome same-origin (in-process) child: documents[1] bounds are FRAME-LOCAL — #probe reads [30,40,150,20] against a frame-local expectation of [30,40,150,20] ⇒ `fetch_chromium`'s same-process path must add the parent iframe's own rect as the child's offset. · confirmed: the parent session's own captureSnapshot sees only its own document (contentDocumentIndex is empty for the iframe's owner node, i.e. nothing on the parent side points at the child at all). The child's OWN captureSnapshot (its own session) returns its own document with #probe at [30,40,150,20] — exactly frame-local, matching the static page. Join key: parent iframe node's `frameId` = \"69F3C410D5C580F9AAD324EC395F5E1D\", OOPIF target's `targetId` = \"69F3C410D5C580F9AAD324EC395F5E1D\", child's own document `frameId` = \"69F3C410D5C580F9AAD324EC395F5E1D\" — all three are the SAME value ⇒ `fetch_chromium` places a child by matching the parent DOM node's `frameId` (from DOM.getDocument, since DOMSnapshot's own node has no such field) against the auto-attached child target's `targetId`, which is also the child's own document `frameId`. `fetch_chromium` must therefore: (1) enumerate iframe sub-targets via Target.setAutoAttach, (2) capture each child session's own DOMSnapshot separately, (3) place each child's frame-local bounds by adding the OWNER `<iframe>` element's own rect (from the PARENT's snapshot, looked up by this join key) as the offset — the same arithmetic as the same-origin path, just sourced from two separate captures instead of one."
}
```

## u5-chrome

```json
{
  "u": "U5",
  "engine": "chrome",
  "scrollResult": "1556",
  "target": {
    "selector": "#deep-target",
    "nodeId": 49,
    "backendNodeId": 51,
    "clientRect": {
      "x": 0,
      "y": 770,
      "w": 120,
      "h": 30,
      "scrollX": 0,
      "scrollY": 1556
    }
  },
  "scrolledTo": 1556,
  "viewportPoint": {
    "x": 60,
    "y": 785
  },
  "pagePoint": {
    "x": 60,
    "y": 2341
  },
  "clickAtViewportPoint": "[{\"id\":\"deep-target\",\"clientX\":60,\"clientY\":785,\"pageX\":60,\"pageY\":2341,\"scrollY\":1556}]",
  "clickAtPagePoint": "[]",
  "verdict": "chrome: Input.dispatchMouseEvent takes VIEWPORT coordinates (hit at y=785 with scrollY=1556, miss at y=2341) ⇒ `ActionTarget::Coordinates` (page coords) converts by subtracting scroll."
}
```

## u5-obscura

```json
{
  "u": "U5",
  "engine": "obscura",
  "scrollResult": "1548",
  "target": {
    "selector": "#deep-target",
    "nodeId": 79,
    "backendNodeId": 79,
    "clientRect": {
      "x": 0,
      "y": 770,
      "w": 120,
      "h": 30,
      "scrollX": 0,
      "scrollY": 1548
    }
  },
  "scrolledTo": 1548,
  "viewportPoint": {
    "x": 60,
    "y": 785
  },
  "pagePoint": {
    "x": 60,
    "y": 2333
  },
  "clickAtViewportPoint": "[{\"id\":\"deep-target\",\"clientX\":60,\"clientY\":785,\"scrollY\":1548}]",
  "clickAtPagePoint": "[]",
  "verdict": "obscura: Input.dispatchMouseEvent takes VIEWPORT coordinates (hit at y=785 with scrollY=1548, miss at y=2333) ⇒ `ActionTarget::Coordinates` (page coords) converts by subtracting scroll."
}
```

## u6-chrome

```json
{
  "u": "U6",
  "engine": "chrome",
  "sent": [
    {
      "name": "t0_plain",
      "value": "v1",
      "url": "http://127.0.0.1:18999/t0-page.html"
    },
    {
      "name": "t0_lax",
      "value": "v2",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "sameSite": "Lax"
    },
    {
      "name": "t0_strict",
      "value": "v3",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "sameSite": "Strict"
    },
    {
      "name": "t0_expires",
      "value": "v4",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "expires": 1788702052
    },
    {
      "name": "t0_httponly",
      "value": "v5",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "httpOnly": true
    },
    {
      "name": "t0_path",
      "value": "v6",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "path": "/sub"
    },
    {
      "name": "t0_domainpath",
      "value": "v7",
      "domain": "127.0.0.1",
      "path": "/"
    },
    {
      "name": "t0_none_insecure",
      "value": "v8",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "sameSite": "None"
    }
  ],
  "expiresSent": 1788702052,
  "setCookies": {
    "ok": true,
    "result": {}
  },
  "getAllOk": true,
  "received": [
    {
      "name": "t0_path",
      "value": "v6",
      "domain": "127.0.0.1",
      "path": "/sub",
      "expires": -1,
      "size": 9,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "priority": "Medium",
      "sourceScheme": "NonSecure",
      "sourcePort": 80
    },
    {
      "name": "t0_plain",
      "value": "v1",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 10,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "priority": "Medium",
      "sourceScheme": "NonSecure",
      "sourcePort": 80
    },
    {
      "name": "t0_lax",
      "value": "v2",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 8,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "sameSite": "Lax",
      "priority": "Medium",
      "sourceScheme": "NonSecure",
      "sourcePort": 80
    },
    {
      "name": "t0_strict",
      "value": "v3",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 11,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "sameSite": "Strict",
      "priority": "Medium",
      "sourceScheme": "NonSecure",
      "sourcePort": 80
    },
    {
      "name": "t0_expires",
      "value": "v4",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": 1788702052,
      "size": 12,
      "httpOnly": false,
      "secure": false,
      "session": false,
      "priority": "Medium",
      "sourceScheme": "NonSecure",
      "sourcePort": 80
    },
    {
      "name": "t0_httponly",
      "value": "v5",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 13,
      "httpOnly": true,
      "secure": false,
      "session": true,
      "priority": "Medium",
      "sourceScheme": "NonSecure",
      "sourcePort": 80
    },
    {
      "name": "t0_domainpath",
      "value": "v7",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 15,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "priority": "Medium",
      "sourceScheme": "NonSecure",
      "sourcePort": 80
    }
  ],
  "perCookie": [
    {
      "name": "t0_plain",
      "present": true,
      "value": "v1",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSiteKept": null,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "priority",
        "sourceScheme",
        "sourcePort"
      ]
    },
    {
      "name": "t0_lax",
      "present": true,
      "value": "v2",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSite": "Lax",
      "sameSiteKept": true,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "priority",
        "sourceScheme",
        "sourcePort"
      ]
    },
    {
      "name": "t0_strict",
      "present": true,
      "value": "v3",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSite": "Strict",
      "sameSiteKept": true,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "priority",
        "sourceScheme",
        "sourcePort"
      ]
    },
    {
      "name": "t0_expires",
      "present": true,
      "value": "v4",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": 1788702052,
      "httpOnly": false,
      "secure": false,
      "sameSiteKept": null,
      "expiresKept": true,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "priority",
        "sourceScheme",
        "sourcePort"
      ]
    },
    {
      "name": "t0_httponly",
      "present": true,
      "value": "v5",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": true,
      "secure": false,
      "sameSiteKept": null,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "priority",
        "sourceScheme",
        "sourcePort"
      ]
    },
    {
      "name": "t0_path",
      "present": true,
      "value": "v6",
      "domain": "127.0.0.1",
      "path": "/sub",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSiteKept": null,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "priority",
        "sourceScheme",
        "sourcePort"
      ]
    },
    {
      "name": "t0_domainpath",
      "present": true,
      "value": "v7",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSiteKept": null,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "priority",
        "sourceScheme",
        "sourcePort"
      ]
    },
    {
      "name": "t0_none_insecure",
      "present": false
    }
  ],
  "verdict": "chrome: 7/8 cookies came back; sameSite kept for [t0_lax,t0_strict]; `expires` round-trips; fields present on a returned cookie: [\"name\",\"value\",\"domain\",\"path\",\"expires\",\"size\",\"httpOnly\",\"secure\",\"session\",\"priority\",\"sourceScheme\",\"sourcePort\"]."
}
```

## u6-obscura

```json
{
  "u": "U6",
  "engine": "obscura",
  "sent": [
    {
      "name": "t0_plain",
      "value": "v1",
      "url": "http://127.0.0.1:18999/t0-page.html"
    },
    {
      "name": "t0_lax",
      "value": "v2",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "sameSite": "Lax"
    },
    {
      "name": "t0_strict",
      "value": "v3",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "sameSite": "Strict"
    },
    {
      "name": "t0_expires",
      "value": "v4",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "expires": 1788702053
    },
    {
      "name": "t0_httponly",
      "value": "v5",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "httpOnly": true
    },
    {
      "name": "t0_path",
      "value": "v6",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "path": "/sub"
    },
    {
      "name": "t0_domainpath",
      "value": "v7",
      "domain": "127.0.0.1",
      "path": "/"
    },
    {
      "name": "t0_none_insecure",
      "value": "v8",
      "url": "http://127.0.0.1:18999/t0-page.html",
      "sameSite": "None"
    }
  ],
  "expiresSent": 1788702053,
  "setCookies": {
    "ok": true,
    "result": {}
  },
  "getAllOk": true,
  "received": [
    {
      "name": "t0_plain",
      "value": "v1",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 10,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "sameSite": "Lax",
      "sameParty": false,
      "sourceScheme": "NonSecure",
      "sourcePort": 80,
      "priority": "Medium"
    },
    {
      "name": "t0_none_insecure",
      "value": "v8",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 18,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "sameSite": "None",
      "sameParty": false,
      "sourceScheme": "NonSecure",
      "sourcePort": 80,
      "priority": "Medium"
    },
    {
      "name": "t0_strict",
      "value": "v3",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 11,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "sameSite": "Strict",
      "sameParty": false,
      "sourceScheme": "NonSecure",
      "sourcePort": 80,
      "priority": "Medium"
    },
    {
      "name": "t0_path",
      "value": "v6",
      "domain": "127.0.0.1",
      "path": "/sub",
      "expires": -1,
      "size": 9,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "sameSite": "Lax",
      "sameParty": false,
      "sourceScheme": "NonSecure",
      "sourcePort": 80,
      "priority": "Medium"
    },
    {
      "name": "t0_expires",
      "value": "v4",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": 1788702053,
      "size": 12,
      "httpOnly": false,
      "secure": false,
      "session": false,
      "sameSite": "Lax",
      "sameParty": false,
      "sourceScheme": "NonSecure",
      "sourcePort": 80,
      "priority": "Medium"
    },
    {
      "name": "t0_lax",
      "value": "v2",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 8,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "sameSite": "Lax",
      "sameParty": false,
      "sourceScheme": "NonSecure",
      "sourcePort": 80,
      "priority": "Medium"
    },
    {
      "name": "t0_httponly",
      "value": "v5",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 13,
      "httpOnly": true,
      "secure": false,
      "session": true,
      "sameSite": "Lax",
      "sameParty": false,
      "sourceScheme": "NonSecure",
      "sourcePort": 80,
      "priority": "Medium"
    },
    {
      "name": "t0_domainpath",
      "value": "v7",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "size": 15,
      "httpOnly": false,
      "secure": false,
      "session": true,
      "sameSite": "Lax",
      "sameParty": false,
      "sourceScheme": "NonSecure",
      "sourcePort": 80,
      "priority": "Medium"
    }
  ],
  "perCookie": [
    {
      "name": "t0_plain",
      "present": true,
      "value": "v1",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSite": "Lax",
      "sameSiteKept": null,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "sameParty",
        "sourceScheme",
        "sourcePort",
        "priority"
      ]
    },
    {
      "name": "t0_lax",
      "present": true,
      "value": "v2",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSite": "Lax",
      "sameSiteKept": true,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "sameParty",
        "sourceScheme",
        "sourcePort",
        "priority"
      ]
    },
    {
      "name": "t0_strict",
      "present": true,
      "value": "v3",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSite": "Strict",
      "sameSiteKept": true,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "sameParty",
        "sourceScheme",
        "sourcePort",
        "priority"
      ]
    },
    {
      "name": "t0_expires",
      "present": true,
      "value": "v4",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": 1788702053,
      "httpOnly": false,
      "secure": false,
      "sameSite": "Lax",
      "sameSiteKept": null,
      "expiresKept": true,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "sameParty",
        "sourceScheme",
        "sourcePort",
        "priority"
      ]
    },
    {
      "name": "t0_httponly",
      "present": true,
      "value": "v5",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": true,
      "secure": false,
      "sameSite": "Lax",
      "sameSiteKept": null,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "sameParty",
        "sourceScheme",
        "sourcePort",
        "priority"
      ]
    },
    {
      "name": "t0_path",
      "present": true,
      "value": "v6",
      "domain": "127.0.0.1",
      "path": "/sub",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSite": "Lax",
      "sameSiteKept": null,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "sameParty",
        "sourceScheme",
        "sourcePort",
        "priority"
      ]
    },
    {
      "name": "t0_domainpath",
      "present": true,
      "value": "v7",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSite": "Lax",
      "sameSiteKept": null,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "sameParty",
        "sourceScheme",
        "sourcePort",
        "priority"
      ]
    },
    {
      "name": "t0_none_insecure",
      "present": true,
      "value": "v8",
      "domain": "127.0.0.1",
      "path": "/",
      "expires": -1,
      "httpOnly": false,
      "secure": false,
      "sameSite": "None",
      "sameSiteKept": true,
      "expiresKept": null,
      "keys": [
        "name",
        "value",
        "domain",
        "path",
        "expires",
        "size",
        "httpOnly",
        "secure",
        "session",
        "sameSite",
        "sameParty",
        "sourceScheme",
        "sourcePort",
        "priority"
      ]
    }
  ],
  "verdict": "obscura: 8/8 cookies came back; sameSite kept for [t0_lax,t0_strict,t0_none_insecure]; `expires` round-trips; fields present on a returned cookie: [\"name\",\"value\",\"domain\",\"path\",\"expires\",\"size\",\"httpOnly\",\"secure\",\"session\",\"sameSite\",\"sameParty\",\"sourceScheme\",\"sourcePort\",\"priority\"]."
}
```

## u9-chrome

```json
{
  "u": "U9",
  "engine": "chrome",
  "localUrl": "http://127.0.0.1:18999/t0-inline.html",
  "hnUrl": "https://news.ycombinator.com/",
  "local": [
    {
      "name": "a-in-body",
      "selector": "#a-in-body",
      "nodeId": 6,
      "backendNodeId": 7,
      "clientRect": {
        "x": 0,
        "y": 2,
        "w": 135,
        "h": 18,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "width": 135,
        "height": 18,
        "content": [
          0,
          2,
          135.171875,
          2,
          135.171875,
          20,
          0,
          20
        ]
      },
      "boxModelHasBox": true,
      "contentQuads": [
        [
          0,
          2,
          135.171875,
          2,
          135.171875,
          20,
          0,
          20
        ]
      ],
      "contentQuadsHasBox": true,
      "clientRectHasBox": true,
      "sourcesAgree": true
    },
    {
      "name": "a-in-p",
      "selector": "#a-in-p",
      "nodeId": 21,
      "backendNodeId": 12,
      "clientRect": {
        "x": 0,
        "y": 40,
        "w": 161,
        "h": 18,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "width": 161,
        "height": 18,
        "content": [
          0,
          40.390625,
          161,
          40.390625,
          161,
          58.390625,
          0,
          58.390625
        ]
      },
      "boxModelHasBox": true,
      "contentQuads": [
        [
          0,
          40.390625,
          161,
          40.390625,
          161,
          58.390625,
          0,
          58.390625
        ]
      ],
      "contentQuadsHasBox": true,
      "clientRectHasBox": true,
      "sourcesAgree": true
    },
    {
      "name": "a-in-td",
      "selector": "#a-in-td",
      "nodeId": 36,
      "backendNodeId": 17,
      "clientRect": {
        "x": 3,
        "y": 82,
        "w": 151,
        "h": 18,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "width": 151,
        "height": 18,
        "content": [
          3,
          81.78125,
          154.203125,
          81.78125,
          154.203125,
          99.78125,
          3,
          99.78125
        ]
      },
      "boxModelHasBox": true,
      "contentQuads": [
        [
          3,
          81.78125,
          154.203125,
          81.78125,
          154.203125,
          99.78125,
          3,
          99.78125
        ]
      ],
      "contentQuadsHasBox": true,
      "clientRectHasBox": true,
      "sourcesAgree": true
    },
    {
      "name": "span-in-block",
      "selector": "#span-in-block",
      "nodeId": 48,
      "backendNodeId": 19,
      "clientRect": {
        "x": 0,
        "y": 107,
        "w": 215,
        "h": 18,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "width": 215,
        "height": 18,
        "content": [
          0,
          107.171875,
          215.234375,
          107.171875,
          215.234375,
          125.171875,
          0,
          125.171875
        ]
      },
      "boxModelHasBox": true,
      "contentQuads": [
        [
          0,
          107.171875,
          215.234375,
          107.171875,
          215.234375,
          125.171875,
          0,
          125.171875
        ]
      ],
      "contentQuadsHasBox": true,
      "clientRectHasBox": true,
      "sourcesAgree": true
    }
  ],
  "hn": {
    "reachable": true,
    "title": "Hacker News",
    "elements": [
      {
        "name": "hn-title-link",
        "selector": "a[href=\"news\"]",
        "nodeId": 42,
        "backendNodeId": 51,
        "clientRect": {
          "x": 130,
          "y": 12,
          "w": 98,
          "h": 16,
          "scrollX": 0,
          "scrollY": 0
        },
        "boxModel": {
          "width": 98,
          "height": 16,
          "content": [
            129.796875,
            12,
            227.796875,
            12,
            227.796875,
            28,
            129.796875,
            28
          ]
        },
        "boxModelHasBox": true,
        "contentQuads": [
          [
            129.796875,
            12,
            227.796875,
            12,
            227.796875,
            28,
            129.796875,
            28
          ]
        ],
        "contentQuadsHasBox": true,
        "clientRectHasBox": true,
        "sourcesAgree": true
      },
      {
        "name": "hn-first-story",
        "selector": "span.titleline a",
        "nodeId": 155,
        "backendNodeId": 152,
        "clientRect": {
          "x": 139,
          "y": 44,
          "w": 156,
          "h": 16,
          "scrollX": 0,
          "scrollY": 0
        },
        "boxModel": {
          "width": 156,
          "height": 16,
          "content": [
            138.609375,
            43.5,
            294.40625,
            43.5,
            294.40625,
            59.5,
            138.609375,
            59.5
          ]
        },
        "boxModelHasBox": true,
        "contentQuads": [
          [
            138.609375,
            43.5,
            294.40625,
            43.5,
            294.40625,
            59.5,
            138.609375,
            59.5
          ]
        ],
        "contentQuadsHasBox": true,
        "clientRectHasBox": true,
        "sourcesAgree": true
      }
    ]
  },
  "placementsWithBox": [
    "a-in-body",
    "a-in-p",
    "a-in-td",
    "span-in-block"
  ],
  "placementsWithoutBox": [],
  "disagreeing": [],
  "shape": "all-inline-have-boxes",
  "verdict": "chrome: all-inline-have-boxes; all three sources agree on every placement; HN: hn-title-link box {\"x\":130,\"y\":12,\"w\":98,\"h\":16,\"scrollX\":0,\"scrollY\":0}, hn-first-story box {\"x\":139,\"y\":44,\"w\":156,\"h\":16,\"scrollX\":0,\"scrollY\":0}."
}
```

## u9-obscura

```json
{
  "u": "U9",
  "engine": "obscura",
  "localUrl": "http://127.0.0.1:18999/t0-inline.html",
  "hnUrl": "https://news.ycombinator.com/",
  "local": [
    {
      "name": "a-in-body",
      "selector": "#a-in-body",
      "nodeId": 13,
      "backendNodeId": 13,
      "clientRect": {
        "x": 0,
        "y": 0,
        "w": 0,
        "h": 0,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "width": 0,
        "height": 0,
        "content": [
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0
        ]
      },
      "boxModelHasBox": false,
      "contentQuads": [
        [
          0,
          0,
          0,
          0,
          0,
          0,
          0,
          0
        ]
      ],
      "contentQuadsHasBox": false,
      "clientRectHasBox": false,
      "sourcesAgree": true
    },
    {
      "name": "a-in-p",
      "selector": "#a-in-p",
      "nodeId": 17,
      "backendNodeId": 17,
      "clientRect": {
        "x": 0,
        "y": 41,
        "w": 161,
        "h": 17,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "width": 160.9921875,
        "height": 17,
        "content": [
          0,
          40.74687576293945,
          160.9921875,
          40.74687576293945,
          160.9921875,
          57.74687576293945,
          0,
          57.74687576293945
        ]
      },
      "boxModelHasBox": true,
      "contentQuads": [
        [
          0,
          40.74687576293945,
          160.9921875,
          40.74687576293945,
          160.9921875,
          57.74687576293945,
          0,
          57.74687576293945
        ]
      ],
      "contentQuadsHasBox": true,
      "clientRectHasBox": true,
      "sourcesAgree": true
    },
    {
      "name": "a-in-td",
      "selector": "#a-in-td",
      "nodeId": 24,
      "backendNodeId": 24,
      "clientRect": {
        "x": 3,
        "y": 83,
        "w": 151,
        "h": 17,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "width": 151.1953125,
        "height": 17,
        "content": [
          3,
          82.74687194824219,
          154.1953125,
          82.74687194824219,
          154.1953125,
          99.7468719482422,
          3,
          99.7468719482422
        ]
      },
      "boxModelHasBox": true,
      "contentQuads": [
        [
          3,
          82.74687194824219,
          154.1953125,
          82.74687194824219,
          154.1953125,
          99.7468719482422,
          3,
          99.7468719482422
        ]
      ],
      "contentQuadsHasBox": true,
      "clientRectHasBox": true,
      "sourcesAgree": true
    },
    {
      "name": "span-in-block",
      "selector": "#span-in-block",
      "nodeId": 28,
      "backendNodeId": 28,
      "clientRect": {
        "x": 0,
        "y": 108,
        "w": 215,
        "h": 17,
        "scrollX": 0,
        "scrollY": 0
      },
      "boxModel": {
        "width": 215.234375,
        "height": 17,
        "content": [
          0,
          107.7468719482422,
          215.234375,
          107.7468719482422,
          215.234375,
          124.7468719482422,
          0,
          124.7468719482422
        ]
      },
      "boxModelHasBox": true,
      "contentQuads": [
        [
          0,
          107.7468719482422,
          215.234375,
          107.7468719482422,
          215.234375,
          124.7468719482422,
          0,
          124.7468719482422
        ]
      ],
      "contentQuadsHasBox": true,
      "clientRectHasBox": true,
      "sourcesAgree": true
    }
  ],
  "hn": {
    "reachable": true,
    "title": "Hacker News",
    "elements": [
      {
        "name": "hn-title-link",
        "selector": "a[href=\"news\"]",
        "nodeId": 25,
        "backendNodeId": 25,
        "clientRect": {
          "x": 130,
          "y": 11,
          "w": 83,
          "h": 15,
          "scrollX": 0,
          "scrollY": 0
        },
        "boxModel": {
          "width": 82.99357604980469,
          "height": 15,
          "content": [
            130,
            10.623241424560549,
            212.9935760498047,
            10.623241424560549,
            212.9935760498047,
            25.623241424560547,
            130,
            25.623241424560547
          ]
        },
        "boxModelHasBox": true,
        "contentQuads": [
          [
            130,
            10.623241424560549,
            212.9935760498047,
            10.623241424560549,
            212.9935760498047,
            25.623241424560547,
            130,
            25.623241424560547
          ]
        ],
        "contentQuadsHasBox": true,
        "clientRectHasBox": true,
        "sourcesAgree": true
      },
      {
        "name": "hn-first-story",
        "selector": "span.titleline a",
        "nodeId": 66,
        "backendNodeId": 66,
        "clientRect": {
          "x": 136,
          "y": 44,
          "w": 136,
          "h": 15,
          "scrollX": 0,
          "scrollY": 0
        },
        "boxModel": {
          "width": 135.58457946777344,
          "height": 15,
          "content": [
            136,
            44.121238708496094,
            271.58457946777344,
            44.121238708496094,
            271.58457946777344,
            59.121238708496094,
            136,
            59.121238708496094
          ]
        },
        "boxModelHasBox": true,
        "contentQuads": [
          [
            136,
            44.121238708496094,
            271.58457946777344,
            44.121238708496094,
            271.58457946777344,
            59.121238708496094,
            136,
            59.121238708496094
          ]
        ],
        "contentQuadsHasBox": true,
        "clientRectHasBox": true,
        "sourcesAgree": true
      }
    ]
  },
  "placementsWithBox": [
    "a-in-p",
    "a-in-td",
    "span-in-block"
  ],
  "placementsWithoutBox": [
    "a-in-body"
  ],
  "disagreeing": [],
  "shape": "only-a-in-p,a-in-td,span-in-block-have-boxes",
  "verdict": "obscura: only-a-in-p,a-in-td,span-in-block-have-boxes (no box for [a-in-body]); all three sources agree on every placement; HN: hn-title-link box {\"x\":130,\"y\":11,\"w\":83,\"h\":15,\"scrollX\":0,\"scrollY\":0}, hn-first-story box {\"x\":136,\"y\":44,\"w\":136,\"h\":15,\"scrollX\":0,\"scrollY\":0}."
}
```
