# Browser viewer safety spike evidence

Gate=Blocked(reason=CDP Fetch cannot reject WebSocket before connection; denied WS receiver counts: prototype_before_mock=1, prototype_mock_added=1, prototype_mock_removed=1)

## Environment

- Platform: win32 10.0.28000 x64
- Engine: Chrome/154.0.8037.98
- Executable: C:/Program Files/Google/Chrome/Application/chrome.exe
- Node: v24.13.0
- Profile: throwaway; removed=true
- Browser/tool installation or update: none
- External/private/metadata traffic: none; only 127.0.0.1/127.0.0.2 and isolated names mapped to them

## Command

`node D:\Workspace\Aleph-worktrees\panel-browser-workspace-20261007-202532-13256\.analysis\browser-viewer-safety-spike\probe.mjs`

## Cases and counts

```json
{
  "counts": {
    "baseline": {
      "allowed_top_level": 1,
      "allowed_redirect": 1,
      "allowed_subresource": 1,
      "allowed_iframe": 1,
      "allowed_fetch": 1,
      "allowed_hmr_ws": 1,
      "mock_network": 1,
      "denied_origin": 2,
      "denied_port": 1,
      "denied_protocol": 1,
      "denied_rebinding": 1,
      "denied_ws_origin": 1
    },
    "prototype_before_mock": {
      "allowed_top_level": 1,
      "allowed_redirect": 1,
      "allowed_subresource": 1,
      "allowed_iframe": 1,
      "allowed_fetch": 1,
      "allowed_hmr_ws": 1,
      "mock_network": 1,
      "denied_origin": 0,
      "denied_port": 0,
      "denied_protocol": 0,
      "denied_rebinding": 0,
      "denied_ws_origin": 1
    },
    "prototype_mock_added": {
      "allowed_top_level": 1,
      "allowed_redirect": 1,
      "allowed_subresource": 1,
      "allowed_iframe": 1,
      "allowed_fetch": 1,
      "allowed_hmr_ws": 1,
      "mock_network": 0,
      "denied_origin": 0,
      "denied_port": 0,
      "denied_protocol": 0,
      "denied_rebinding": 0,
      "denied_ws_origin": 1
    },
    "prototype_mock_removed": {
      "allowed_top_level": 1,
      "allowed_redirect": 1,
      "allowed_subresource": 1,
      "allowed_iframe": 1,
      "allowed_fetch": 1,
      "allowed_hmr_ws": 1,
      "mock_network": 1,
      "denied_origin": 0,
      "denied_port": 0,
      "denied_protocol": 0,
      "denied_rebinding": 0,
      "denied_ws_origin": 1
    }
  },
  "fetch": {
    "baseline": {
      "paused_total": 0,
      "paused_websocket": 0,
      "blocked": {},
      "fulfilled_mock": 0
    },
    "prototype_before_mock": {
      "paused_total": 13,
      "paused_websocket": 0,
      "blocked": {
        "origin": 2,
        "port": 1,
        "rebinding": 1,
        "protocol": 1
      },
      "fulfilled_mock": 0
    },
    "prototype_mock_added": {
      "paused_total": 13,
      "paused_websocket": 0,
      "blocked": {
        "origin": 2,
        "port": 1,
        "rebinding": 1,
        "protocol": 1
      },
      "fulfilled_mock": 1
    },
    "prototype_mock_removed": {
      "paused_total": 13,
      "paused_websocket": 0,
      "blocked": {
        "origin": 2,
        "port": 1,
        "rebinding": 1,
        "protocol": 1
      },
      "fulfilled_mock": 0
    }
  },
  "page_results": {
    "baseline": {
      "phase": "baseline",
      "fetch": "api-ok",
      "mock": "network",
      "ws": "open",
      "denied_ws": "open"
    },
    "prototype_before_mock": {
      "phase": "prototype_before_mock",
      "fetch": "api-ok",
      "mock": "network",
      "ws": "open",
      "denied_ws": "open"
    },
    "prototype_mock_added": {
      "phase": "prototype_mock_added",
      "fetch": "api-ok",
      "mock": "mock",
      "ws": "open",
      "denied_ws": "open"
    },
    "prototype_mock_removed": {
      "phase": "prototype_mock_removed",
      "fetch": "api-ok",
      "mock": "network",
      "ws": "open",
      "denied_ws": "open"
    }
  },
  "assertions": {
    "allowed_count_gt_zero": {
      "prototype_before_mock": {
        "top_level": true,
        "redirect": true,
        "subresource": true,
        "iframe": true,
        "fetch": true,
        "hmr_websocket": true
      },
      "prototype_mock_added": {
        "top_level": true,
        "redirect": true,
        "subresource": true,
        "iframe": true,
        "fetch": true,
        "hmr_websocket": true
      },
      "prototype_mock_removed": {
        "top_level": true,
        "redirect": true,
        "subresource": true,
        "iframe": true,
        "fetch": true,
        "hmr_websocket": true
      }
    },
    "denied_count_eq_zero": {
      "prototype_before_mock": {
        "origin": 0,
        "port": 0,
        "protocol": 0,
        "rebinding": 0,
        "websocket_origin": 1
      },
      "prototype_mock_added": {
        "origin": 0,
        "port": 0,
        "protocol": 0,
        "rebinding": 0,
        "websocket_origin": 1
      },
      "prototype_mock_removed": {
        "origin": 0,
        "port": 0,
        "protocol": 0,
        "rebinding": 0,
        "websocket_origin": 1
      }
    }
  }
}
```

## Interpretation

- Baseline is a positive receiver control: every allowed and would-be-denied self-owned loopback receiver was reachable without Fetch policy.
- Prototype phases apply one continuously enabled Fetch worker before mock add, with mock active, and after mock remove.
- Required assertions are `allowed_count > 0` and receiver-observed `denied_count == 0`; Fetch failure counts alone are not treated as isolation proof.
- CDP Fetch WebSocket answer: not proven safe; see exact gate reason and counts.

## Limitations

- CDP Fetch emitted no usable pre-connect pause for the denied WebSocket handshake; the self-owned loopback WS receiver observed it. HTTP Fetch results cannot stand in for WS safety.

## Cleanup

- Chromium process stopped: true
- Loopback servers closed: true
- Throwaway profile removed: true

No product source, spec, plan, ledger/progress, Cargo manifest, or lockfile was modified. No git commit was created.
