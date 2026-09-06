# cef-rs survey — the page-rendering path

Repo: `/Volumes/TBU4/Github/cef-rs` (tauri-apps/cef-rs), HEAD `71a3eba55bb46957b7bdd627ef7caf5c70acf4bb` "chore: fmt code", Thu Sep 3 2026.
Workspace version `151.8.1+151.3.24` (`Cargo.toml:9`) = cef-rs 151.8.1 wrapping CEF `151.3.24` (Chromium 151).
Read-only survey. No `cargo build` was run (the `cef-dll-sys` build.rs downloads the CEF binary distribution and drives cmake).

---

## 1. Layout, bindings, linking, build script

### Workspace members (`Cargo.toml:4-12`)

| Member | .rs files | LOC | What it is |
|---|---:|---:|---|
| `sys` (`cef-dll-sys`) | 11 | 288,608 | Raw bindgen output over CEF's C API, one checked-in file per target triple (~36k LOC each, `sys/src/bindings/*.rs`). Hand-written part is 105 lines (`sys/src/lib.rs`) + `sys/build.rs` (270 lines). |
| `cef` | 49 | 925,370 | Safe-ish wrapper. **All but ~6.5k LOC is generated**: `cef/src/bindings/<triple>.rs` (~59.5k each) + `cef/src/resources/<triple>.rs` (~59.6k each), 8 triples of each, checked in. Hand-written: `string.rs` 1422, `rc.rs` 380, `wrapper/message_router.rs` 1784, `wrapper/resource_manager.rs` 1155, `wrapper/message_router_utils.rs` 726, `osr_texture_import/*` 183+238+307+common+mod, `build_util/mac.rs` 342, `library_loader.rs` 45, `sandbox.rs`, `window_info.rs`, `args.rs`, `application_mac.rs`. |
| `download-cef` | 1 | 653 | CDN index client + archive download/extract (`download-cef/src/lib.rs`). |
| `export-cef-dir` | 1 | 136 | CLI that downloads+extracts a CEF dist into a chosen dir (`export-cef-dir/src/main.rs`). |
| `update-bindings` | 6 | 4,445 | Dev-time codegen: runs bindgen, then rewrites the raw tree into the `cef` crate's typed API (`update-bindings/src/{main,parse_tree,resources,upgrade,dirs}.rs`). `publish = false` (`update-bindings/Cargo.toml:3`). |
| `get-latest` | 1 | 235 | CI job: queries the CDN index for the newest stable CEF across all 8 targets, bumps the workspace version, runs git-cliff (`get-latest/src/main.rs:70-81`). |
| `examples/cefsimple` | 13 | 1,078 | Windowed (non-OSR) hello-world; the CEF upstream `cefsimple` ported. |
| `examples/osr` | 2 | 978 | **The off-screen-rendering example.** `src/main.rs` + `src/webrender.rs`. |
| `examples/tests_shared` | 23 | 2,399 | Port of CEF's `tests/shared` scaffolding: message-loop implementations, client app, resource utils. Not a test suite — a support library. |

### Bindings generation

Two-stage, and **the output is committed, not generated at build time**:

1. `update-bindings/src/upgrade.rs:52-81` runs `bindgen::Builder` over CEF's `cef_capi` headers with `.allowlist_type("cef_.*")`, `.allowlist_function("cef_.*")`, `.allowlist_item("CEF_API_VERSION(_.+)?")`, `CEF_VERSION`, `CHROME_VERSION`, `ID[CRS]_.+`, `default_enum_style(EnumVariation::Rust)`. Output → `sys/src/bindings/<triple>.rs`.
2. `update-bindings/src/parse_tree.rs:16-27` parses that file with `syn`, walks it, and emits the `cef` crate's wrapper types → `cef/src/bindings/<triple>.rs`. `resources.rs` does the same for the `ID[CRS]_` resource-id constants → `cef/src/resources/<triple>.rs`.
3. `update-bindings/src/main.rs:89-105` only copies over the destination when the content differs.

So the `cef` crate's entire public API surface is machine-generated from CEF's C API, per target triple, gated by `cfg(target_os/target_arch)` in `cef/src/bindings/mod.rs:1-39`. **This is why the crate has a per-CEF-version major version** — the API is the CEF API.

### Linking: static wrapper + dynamic libcef, NOT dlopen

`rg -n 'dlopen|libloading|link\(name|LoadLibrary|dlsym' sys/ cef/src/*.rs …` → the **only** hit outside changelogs is `cef/src/sandbox.rs:1: use libloading::Library;` (macOS sandbox helper lib). There is no `dlopen` of libcef itself.

`sys/build.rs` (`sys/build.rs:134-218`):
- Runs `cmake::Config::new(&cef_dir)` with the **Ninja** generator, `RelWithDebInfo`, building CEF's own `libcef_dll_wrapper` C++ static lib. **Ninja and a C++ toolchain are hard build requirements.**
- Pins the API version: reads `CEF_API_VERSION_LAST` out of `include/cef_api_versions.h` (`sys/build.rs:228-242`) and passes `CEF_COMPILER_DEFINES=CEF_API_VERSION=<n>`. The comment at `sys/build.rs:123-131` is the load-bearing one: without it "the macOS loader in `libcef_dll_dylib.cc` also resolves experimental entry points, and **loading any libcef but this exact build fails on the first one missing**." — i.e. the shipped libcef must match the build-time headers.
- Linux: `cargo::rustc-link-lib=dylib=cef` + copies the whole CEF dir and `locales/` next to the target binary (`sys/build.rs:162-164`, `copy_cef_runtime_files` at `:256-267`).
- Windows: links `static=libcef_dll_wrapper` + `dylib=libcef`, plus 14 Windows SDK libs (`sys/build.rs:171-202`), `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`.
- macOS: links `framework=AppKit` and `static=cef_dll_wrapper` — **but not libcef**. On macOS libcef is loaded at runtime from the framework bundle: `cef/src/library_loader.rs:26-36` calls `load_library(path)` (CEF's `cef_load_library`), resolving `../Frameworks/Chromium Embedded Framework.framework/Chromium Embedded Framework` for the main app or `../../../<framework>` for a helper (`library_loader.rs:11-23`). `Drop` calls `unload_library` (`:39-45`).

### Where the CEF distribution comes from

- CDN: `download-cef/src/lib.rs:131` `pub const DEFAULT_CDN_URL: &str = "https://cef-builds.spotifycdn.com";`, overridable via `CEF_DOWNLOAD_URL` (`:133-135`). Index at `{url}/index.json` (`:171`).
- **Only the `minimal` build is ever downloaded**: `CefVersion::minimal()` filters `f.file_type == "minimal"` (`download-cef/src/lib.rs:378-383`) and `download_archive_from` calls it unconditionally (`:253`). There is no code path that fetches the "standard"/"client" archive.
- Archive is `.tar.bz2`, SHA1-verified against the index (`:319-323`), extracted with `tar` + `BzDecoder` (`:489-490`). **No archive size is stated anywhere in the repo** — not found: `rg -in 'MB|GB|megabyte|size' download-cef/ README.md`.
- Cache resolution in `sys/build.rs:74-104`: `FLATPAK` → `/usr/lib`; else `CEF_PATH` (with a `$CEF_PATH/<cef_version>/cef_<os>_<arch>` versioned layout preferred, `sys/build.rs:83-85`); else download into `OUT_DIR`. An `archive.json` sidecar records name+sha1 and is version-checked (`check_archive_json`, `download-cef/src/lib.rs:100-121`).
- Extraction normalizes the layout to `cef_<os>_<arch>/` containing `Release/` contents, plus (non-macOS) the `Resources/` contents flattened in, plus `include/`, `cmake/`, `libcef_dll/`, `CMakeLists.txt`, `CREDITS.html` (`download-cef/src/lib.rs:506-544`).

---

## 2. Off-screen rendering (OSR) — the core question

### What `examples/osr` actually is

978 LOC across two files. Stack: **winit 0.30 + wgpu 30** (`examples/osr/Cargo.toml:15,20`), plus `pollster`, `bytemuck`, `env_logger`, and — declared but only used as a dependency, never as a runtime — `tokio = { version = "1", features = ["full"] }` (`examples/osr/Cargo.toml:19`). **`rg 'tokio' examples/osr/src/` → no hits.** The example does not use tokio at all; the dep is dead weight.

Presentation path: CEF frame → a `wgpu::BindGroup` stored in a **`thread_local! { pub static TEXTURE: RefCell<Option<wgpu::BindGroup>> }`** (`examples/osr/src/webrender.rs:408-410`) → each `RedrawRequested` a full-screen textured quad (2 triangles, `TriangleStrip`) is drawn to the winit surface (`examples/osr/src/main.rs:203-231`, geometry at `:435-478`, shader `shader.wgsl`). Backends are pinned per OS: dx12 / metal / vulkan (`examples/osr/src/main.rs:33-38`), surface format hard-coded `Bgra8Unorm` (`:62`).

**Honest scope warning:** this example is a *frame-pump demo, not a browser*. `rg -n 'popup|cursor|ime|send_mouse|send_key|send_touch' examples/osr/src/` returns **no functional hits** — no input injection, no popup compositing, no cursor, no IME, no `LoadHandler`, no navigation beyond one hard-coded URL. And that URL is a typo: `"https:://github.com"` (`examples/osr/src/main.rs:300`).

### The OSR contract as wired by the example

Enable flags, at browser creation (`examples/osr/src/main.rs:271-276`):
```rust
let window_info = WindowInfo {
    windowless_rendering_enabled: true as _,
    shared_texture_enabled: accelerated_osr as _,
    external_begin_frame_enabled: accelerated_osr as _,
    ..Default::default()
};
```
plus the process-wide `Settings { windowless_rendering_enabled: true, external_message_pump: true, .. }` (`examples/osr/src/main.rs:380-384`), and `BrowserSettings { windowless_frame_rate: 60, .. }` (`:286-289`).

`_cef_window_info_t` OSR-relevant fields (`sys/src/bindings/aarch64_apple_darwin.rs:17973-17993`):
- `windowless_rendering_enabled` — "No view will be created for the browser and all rendering will occur via the CefRenderHandler interface… In order to create windowless browsers the CefSettings.windowless_rendering_enabled value must be set to true. **Transparent painting is enabled by default** but can be disabled by setting CefBrowserSettings.background_color to an opaque value." (`:17983`)
- `shared_texture_enabled` — CEF's own doc string still says "**Currently only supported on Windows (D3D11)**" (`:17985`). That comment is **stale relative to the struct next to it**: the same header defines a macOS `shared_texture_io_surface` field and a Linux `planes[4]` variant, and cef-rs ships importers for all three. Treat the doc, not the capability, as the thing that is out of date — but verify on macOS before betting on it.
- `external_begin_frame_enabled` — "Set to true (1) to enable the ability to issue BeginFrame from the client application."
- `runtime_style` — "Alloy style will always be used if `windowless_rendering_enabled` is true". OSR forces the Alloy runtime, not Chrome-style runtime.

### `on_paint` (CPU path)

Trait signature (`cef/src/bindings/aarch64_apple_darwin.rs:23164-23173`):
```rust
fn on_paint(&self, browser: Option<&mut Browser>, type_: PaintElementType,
            dirty_rects: Option<&[Rect]>, buffer: *const u8,
            width: c_int, height: c_int) {}
```
CEF's own doc (`sys/src/bindings/aarch64_apple_darwin.rs:28371`), verbatim on the key points:
- "|buffer| contains the pixel data for **the whole image**" — **full buffer every call**, not a delta.
- "|dirtyRects| contains the set of rectangles **in pixel coordinates** that need to be repainted" — dirty rects are advisory; the buffer is whole.
- "|buffer| will be |width|\*|height|\*4 bytes in size and represents a **BGRA image with an upper-left origin**." Stride is implicitly `width*4`; there is no stride field. The example hard-codes `bytes_per_row: Some(4 * width)` (`examples/osr/src/webrender.rs:322`).
- "Pixel values … are scaled relative to view coordinates based on the value of `CefScreenInfo.device_scale_factor` returned from GetScreenInfo."
- "This function is **only called when** `cef_window_info::shared_texture_enabled` is set to **false**." — the two paths are mutually exclusive; you get one or the other, never both.
- `type_` (`PaintElementType` = `cef_paint_element_type_t`, `sys/…:19613-19616`) is `PET_VIEW = 0` or `PET_POPUP = 1`.

The example's `on_paint` (`examples/osr/src/webrender.rs:275-398`) **creates a brand-new wgpu texture, sampler, bind-group layout and bind group on every single frame** and uploads the whole buffer with `queue.write_texture`. That is demo-grade, not ship-grade. It also **ignores `type_` entirely** in the `on_paint` arm (it is named `_type_` at `:278`), so a popup repaint would overwrite the whole page texture.

### `on_accelerated_paint` (zero-copy path)

Trait signature (`cef/src/bindings/aarch64_apple_darwin.rs:23175-23182`):
```rust
fn on_accelerated_paint(&self, browser: Option<&mut Browser>, type_: PaintElementType,
                        dirty_rects: Option<&[Rect]>, info: Option<&AcceleratedPaintInfo>) {}
```
CEF's doc, the operationally load-bearing part (`sys/src/bindings/aarch64_apple_darwin.rs:28384`):
> "on Windows it is a HANDLE to a texture that can be opened with D3D11 OpenSharedResource1 or D3D12 OpenSharedHandle, on macOS it is an **IOSurface pointer**, and on Linux it contains several planes, each with an **fd** to the underlying system native buffer. The underlying implementation **uses a pool** to deliver frames. As a result, **the handle may differ every frame**… The handle's resource **cannot be cached and cannot be accessed outside of this callback**. It should be reopened each time this callback is executed and the contents should be **copied to a texture owned by the client application**. The contents of |info| will be released back to the pool after this callback returns."

That last sentence is the ship-blocker in the example: the example imports the shared texture into a wgpu texture and stashes the **bind group referring to the pooled texture** in `TEXTURE` (`examples/osr/src/webrender.rs:266-268`), then samples it on a later frame from `State::render` — i.e. after CEF has released it back to the pool. It never does the copy CEF's doc demands. **Do not copy this pattern.**

`AcceleratedPaintInfo` fields — three different structs, one per platform:

| Platform | struct fields | anchor |
|---|---|---|
| macOS | `size: usize`, **`shared_texture_io_surface: cef_shared_texture_handle_t`**, `format: cef_color_type_t`, `extra: cef_accelerated_paint_info_common_t`. Total 136 bytes. | `sys/src/bindings/aarch64_apple_darwin.rs:18024-18033` |
| Windows | `size`, **`shared_texture_handle`** ("The shared texture is instantiated **without a keyed mutex**"), `format`, `extra`. Total 136 bytes. | `sys/src/bindings/x86_64_pc_windows_msvc.rs:17953-17962` |
| Linux | `size`, **`planes: [cef_accelerated_paint_native_pixmap_plane_t; 4]`**, `plane_count: c_int`, `modifier: u64` ("could be used with EGL driver"), `format`, `extra`. Total 272 bytes. | `sys/src/bindings/x86_64_unknown_linux_gnu.rs:17730-17743` |

Linux plane = `{ stride: u32, offset: u64, size: u64, fd: c_int }` (`sys/src/bindings/x86_64_unknown_linux_gnu.rs:17697-17704`).

`cef_accelerated_paint_info_common_t` — the same on all three (`sys/src/bindings/aarch64_apple_darwin.rs:17861-17888`), 112 bytes:
`size`, `timestamp: u64` (µs since capture start), `coded_size: cef_size_t`, `visible_rect: cef_rect_t`, `content_rect: cef_rect_t`, `source_size: cef_size_t`, `capture_update_rect: cef_rect_t` ("the `dirty` area"), `region_capture_rect: cef_rect_t`, `capture_counter: u64` (incremental frame counter), plus four `has_*: u8` presence flags. **`timestamp` and `capture_counter` are exactly what a live-view pipeline needs for frame pacing and drop detection.**

`format` is `cef_color_type_t`; cef-rs maps only two values — `CEF_COLOR_TYPE_BGRA_8888 → Bgra8Unorm`, `CEF_COLOR_TYPE_RGBA_8888 → Rgba8Unorm`, everything else is `UnsupportedFormat` (`cef/src/osr_texture_import/common.rs:12-20`).

### `cef/src/osr_texture_import/` — the reusable part

Behind the non-default `accelerated_osr` feature (`cef/Cargo.toml:23-33`, pulls in `ash`, `wgpu`, `objc2-metal`, `objc2-io-surface`, `windows`, `libc`). This is **the most valuable code in the repo for a host** and it is in the library, not the example:
- `SharedTextureHandle::new(info)` picks the platform importer by `cfg`, else `Unsupported` (`cef/src/osr_texture_import/shared_texture_handle.rs:15-31`).
- macOS: `MTLDevice::newTextureWithDescriptor_iosurface_plane` on wgpu's Metal HAL device, then `create_texture_from_hal` (`cef/src/osr_texture_import/iosurface.rs:121-177`). Only works if wgpu picked the Metal backend (`:179-182`).
- Windows: tries **D3D12 first**, then Vulkan external memory, then CPU fallback (`cef/src/osr_texture_import/d3d11.rs:26-70`).
- Linux: Vulkan `VK_EXTERNAL_MEMORY` over the dmabuf fds; validates each fd with `libc::fcntl(fd, F_GETFD)` before trusting it (`cef/src/osr_texture_import/dmabuf.rs:58-77`).
- **Every path silently degrades to `texture::create_fallback`, which allocates an EMPTY texture and only logs `tracing::warn!`** (`cef/src/osr_texture_import/common.rs:38-69`). A host must treat that warn as a hard signal; otherwise "accelerated OSR works" and the viewer sees black.

### Frame clock: can the host drive it?

**Yes, and this is the single most important property for Aleph.** Two independent knobs:

1. `windowless_frame_rate` — `BrowserSettings.windowless_frame_rate` at creation, or `browser_host.set_windowless_frame_rate(n)` at runtime. CEF doc: "The maximum rate in fps that OnPaint will be called… **The minimum value is 1** and the default value is 30." (`sys/src/bindings/aarch64_apple_darwin.rs:18290`, setter doc at `:25612`, getter at `:25608` — "can only be called on the UI thread"). So the floor is 1 fps, **not 0**: you cannot stop paints entirely this way.
2. `send_external_begin_frame()` — "Issue a BeginFrame request to Chromium. Only valid when `cef_window_info::external_begin_frame_enabled` is set to true" (`sys/src/bindings/aarch64_apple_darwin.rs:25564-25565`). The example calls it once per `RedrawRequested` (`examples/osr/src/main.rs:321-325`), i.e. **the host owns the frame clock**. Combined with `was_hidden(true)` this is the "render only when a viewer is attached" primitive.

Related host methods, all `_cef_browser_host_t`, all OSR-only:
- `was_resized()` — "The browser will first call GetViewRect to get the new size and then call OnPaint asynchronously with the updated regions." (`:25551-25552`). The example calls it on `WindowEvent::Resized` after mutating a shared `Rc<RefCell<LogicalSize>>` that `view_rect` reads (`examples/osr/src/main.rs:329-338`).
- `was_hidden(hidden: c_int)` — "**Layouting and OnPaint notification will stop when the browser is hidden.**" (`:25553-25556`). This is the real "pause when nobody is watching" switch.
- `notify_screen_info_changed()` — re-queries `GetScreenInfo` + `GetRootScreenRect` + `GetViewRect`; "simulates moving or resizing the root window… or changing the properties of the current display" and pushes `window.devicePixelRatio` / `screenX/Y` / `outerWidth/Height` to the renderer (`:25557-25559`).
- `invalidate(type_)` — "The browser will call OnPaint asynchronously." (`:25560-25563`).
- `is_window_rendering_disabled()` (`:25547-25550`), `notify_move_or_resize_started()` ("only used on Windows and Linux", `:25605-25607`), `send_capture_lost_event()` (`:25602-25604`).

### Geometry callbacks

- `view_rect(browser, rect)` — "retrieve the view rectangle **in screen DIP coordinates**. This function must always provide a non-NULL rectangle." (`sys/…:28328`). Example returns the winit logical size and **silently leaves the rect untouched when width or height is 0** (`examples/osr/src/webrender.rs:134-143`) — CEF then reads whatever was in the struct.
- `root_screen_rect` — "root window rectangle in screen DIP coordinates. Return true if provided. If false the rectangle from GetViewRect will be used." (`sys/…:28320`). Not implemented by the example.
- `screen_point(view_x, view_y, &mut screen_x, &mut screen_y)` — "translation from view DIP coordinates to screen coordinates. **Windows/Linux should provide screen device (pixel) coordinates and MacOS should provide screen DIP coordinates.**" (`sys/…:28336`). The example **returns `false`** (`examples/osr/src/webrender.rs:157-166`) — which is why its popups/context menus would be mispositioned.
- `screen_info(browser, &mut ScreenInfo) -> c_int` — the example sets only `device_scale_factor` and returns true (`examples/osr/src/webrender.rs:145-155`). CEF's doc warns: "If the screen info rectangle is left NULL the rectangle from GetViewRect will be used. **If the rectangle is still NULL or invalid popups may not be drawn correctly.**" (`sys/…:28347`).

### Popups (dropdowns, `<select>`, autofill)

`on_popup_show(browser, show: c_int)` — "Called when the browser wants to show or hide the popup widget." (`sys/…:28355`). `on_popup_size(browser, rect: Option<&Rect>)` — "new location and size **in view coordinates**." (`sys/…:28363`).

**The osr example implements neither.** Compositing a popup is entirely on the host: you get a second stream of `on_paint`/`on_accelerated_paint` calls with `type_ == PET_POPUP` and must blit them at the rect from `on_popup_size` on top of the `PET_VIEW` layer. `rg 'PET_POPUP|on_popup' examples/` → only the generated `PaintElementType` definition. **not found: any popup compositing in this repo.**

### IME, cursor, drag, selection — declared, not demonstrated

All present on `ImplRenderHandler` with default no-op bodies, none implemented in the example:
`on_ime_composition_range_changed(browser, selected_range: Option<&Range>, character_bounds: Option<&[Rect]>)` (`cef/src/bindings/…:23213-23220`), `on_text_selection_changed`, `on_virtual_keyboard_requested(input_mode: TextInputMode)`, `start_dragging(drag_data, allowed_ops, x, y) -> c_int`, `update_drag_cursor`, `on_scroll_offset_changed(x: f64, y: f64)`, `touch_handle_size`, `on_touch_handle_state_changed`, `accessibility_handler`.

**Cursor changes are NOT on `RenderHandler`** — `on_cursor_change` lives on `DisplayHandler`; the osr example has no `DisplayHandler` at all. **not found: `rg 'on_cursor_change' examples/`.**

All 17 `_cef_render_handler_t` slots are wired unconditionally by the generated `init_methods` (`cef/src/bindings/aarch64_apple_darwin.rs:23246-23264`), so the C-side vtable is always fully populated regardless of what you override — the defaults just return `Default::default()`.

---

## 3. Input injection in OSR

All on `ImplBrowserHost` (`cef/src/bindings/aarch64_apple_darwin.rs:12646-12692`), forwarding to `_cef_browser_host_t` slots:

```rust
fn send_key_event(&self, event: Option<&KeyEvent>);
fn send_mouse_click_event(&self, event: Option<&MouseEvent>, type_: MouseButtonType,
                          mouse_up: c_int, click_count: c_int);
fn send_mouse_move_event(&self, event: Option<&MouseEvent>, mouse_leave: c_int);
fn send_mouse_wheel_event(&self, event: Option<&MouseEvent>, delta_x: c_int, delta_y: c_int);
fn send_touch_event(&self, event: Option<&TouchEvent>);
fn send_capture_lost_event(&self);
fn set_focus(&self, focus: c_int);                                    // :12535
fn ime_set_composition(&self, text: Option<&CefString>,
                       underlines: Option<&[CompositionUnderline]>,
                       replacement_range: Option<&Range>, selection_range: Option<&Range>);
fn ime_commit_text(&self, text: Option<&CefString>, replacement_range: Option<&Range>,
                   relative_cursor_pos: c_int);
fn ime_finish_composing_text(&self, keep_selection: c_int);
fn ime_cancel_composition(&self);
// drag: drag_target_drag_enter/over/leave/drop, drag_source_ended_at   :12693-12706
```

### Coordinate space

CEF's docs are explicit and consistent (`sys/src/bindings/aarch64_apple_darwin.rs:25571,25581,25589`): for click, move and wheel, "The |x| and |y| coordinates are **relative to the upper-left corner of the view**." `_cef_mouse_event_t` is `{ x: c_int, y: c_int, modifiers: u32 }` — 12 bytes, **no `size` field** (`sys/…:19515-19522`). `_cef_touch_event_t` uses `f32` x/y "relative to the left/top side of the view" plus `radius_x/y`, `rotation_angle`, `pressure`, `type_`, `pointer_type`, and an `id` that "must be unique per touch, can be any number except -1. **a maximum of 16 concurrent touches** will be tracked" (`sys/…:19560-19581`).

The DIP-vs-pixel question is **not answered by the mouse event itself** — it is resolved by `screen_info.device_scale_factor`. `view_rect` returns **screen DIP** (`sys/…:28328`), `on_paint`'s buffer is in **pixels** = DIP × device_scale_factor (`sys/…:28371`), and mouse events are in **view coordinates**, i.e. the same DIP space as `view_rect`. So: divide viewer pixel coordinates by the scale factor before injecting.

One trap worth quoting (`sys/…:25589`): "In order to **scroll inside select popups** with window rendering disabled `cef_render_handler_t::GetScreenPoint` should be implemented properly." — the osr example returns `false` from `screen_point` (`examples/osr/src/webrender.rs:157-166`), so popup scrolling is broken there.

### Key events

`_cef_key_event_t` (`sys/…:19936-19955`), 40 bytes: `size`, `type_: cef_key_event_type_t`, `modifiers: u32`, **`windows_key_code`** ("used by the DOM specification… sometimes it comes directly from the event (i.e. on Windows) and sometimes it's determined using a mapping function"), `native_key_code`, `is_system_key` ("always false on non-Windows"), `character: char16_t`, `unmodified_character: char16_t`, `focus_on_editable_field`.

**The Windows virtual-key-code requirement is the expensive part of OSR input on macOS and Linux**: a host must map its own key events to Windows VK codes. CEF's own `cefclient` ships that mapping; **cef-rs does not** — `rg -n 'windows_key_code' examples/` → no hits.

### Modifiers

`cef_event_flags_t` is a bitflag newtype: `EVENTFLAG_NONE=0`, `CAPS_LOCK_ON=1`, `SHIFT_DOWN=2`, `CONTROL_DOWN=4`, `ALT_DOWN=8`, `LEFT_MOUSE_BUTTON=16`, … (`sys/src/bindings/aarch64_apple_darwin.rs:19617-19623`).

### IME

`ime_set_composition` doc (`sys/…:25616`) — the full contract is quoted in the binding: "Blink has a special node (a composition node)… This function may be called multiple times as the composition changes… To cancel call ImeCancelComposition. To complete call either ImeCommitText or ImeFinishComposingText." And critically: "**The |replacement_range| value is only used on OS X**", and all four IME functions carry "**This function is only used when window rendering is disabled**". So IME is an OSR-exclusive surface — a windowed CEF handles it itself.

### How the osr example maps winit events → these calls

**It does not.** `examples/osr/src/main.rs:315-341` handles exactly three `WindowEvent` variants — `CloseRequested`, `RedrawRequested`, `Resized` — and falls through `_ => ()` for everything else. **There is zero input injection in this repo's OSR example.** not found: `rg 'send_mouse|send_key|send_touch|ime_' examples/`.

That is a real gap for anyone using this example as a template: the pixel path is demonstrated, the interaction path is not. The mapping work (winit `KeyEvent` → Windows VK code, scroll-delta units, click-count tracking, `mouse_leave`, focus, capture-lost) is all still ahead of you.

---

## 4. Process model + threading

### The host binary *is* the browser process

Both examples use the same entry-point shape (`examples/osr/src/main.rs:359-379`, `examples/cefsimple/src/shared/mod.rs:36-51`):

```rust
let args = Args::new();
let cmd = args.as_cmd_line().unwrap();
let is_browser_process = cmd.has_switch(Some(&"type".into())) != 1;
let ret = execute_process(Some(args.as_main_args()), Some(&mut app), null_mut());
if is_browser_process { assert!(ret == -1); }
else { /* sub-process: execute_process blocked and returned; just exit */ return 0.into(); }
```

CEF's contract (`sys/src/bindings/aarch64_apple_darwin.rs:30997`): "If called for the browser process (identified by **no \"type\" command-line value**) it will return immediately with a value of **-1**. If called for a recognized secondary process it will **block until the process should exit** and then return the process exit code."

**This is the fundamental collision with a tokio server.** `cef_execute_process` must run before anything else in `main`, and in a sub-process it never returns until that renderer/GPU process dies. Any Aleph binary that embeds CEF must either (a) branch on `--type` before touching tokio, or (b) ship a **separate helper executable** and point `CefSettings.browser_subprocess_path` at it.

Path knobs for (b) (`sys/…:18113-18118`):
- `browser_subprocess_path` — "If this value is empty on Windows or Linux then the main process executable will be used. **If this value is empty on macOS then a helper executable must exist at `Contents/Frameworks/<app> Helper.app/Contents/MacOS/<app> Helper`** in the top-level app bundle."
- `framework_dir_path` — "If empty then the framework must exist at `Contents/Frameworks/Chromium Embedded Framework.framework` in the top-level app bundle."
- `main_bundle_path` — defaults to the top-level app bundle.

All three are also settable as command-line switches (`framework-dir-path`, `browser-subprocess-path`, `main-bundle-path`).

### macOS bundling — handled by `bundle-cef-app`, and it is not a trivial layout

`cef/src/bin/bundle-cef-app/` + `cef/src/build_util/mac.rs`. `HELPERS` is a hard-coded list of **five** helper bundles (`cef/src/build_util/mac.rs:228-234`):
```rust
const HELPERS: &[&str] = &["Helper (GPU)", "Helper (Renderer)", "Helper (Plugin)", "Helper (Alerts)", "Helper"];
```
`bundle()` (`cef/src/build_util/mac.rs:61-100`) creates `<name>.app`, copies the whole `Chromium Embedded Framework.framework` into `Contents/Frameworks/` (`:78-82`), then creates **one `.app` per helper**, each with its own `Info.plist` and a copy of the *same* helper binary (`:83-93`). Info.plist details worth noting (`:170-202`): `LSEnvironment = { MallocNanoZone: "0" }`, `LSFileQuarantineEnabled: true`, `LSMinimumSystemVersion: "11.0"`, `LSUIElement: "1"` for helpers only, and usage-description strings for camera / microphone / bluetooth / WebAuthn.

The single helper binary is 28 lines (`examples/cefsimple/src/bin/cefsimple_helper.rs`): optional macOS sandbox init, `LibraryLoader::new(current_exe, helper=true)`, `api_hash`, `execute_process(args, None, null)`.

**A macOS host also has to subclass `NSApplication`.** `examples/cefsimple/src/mac/mod.rs:102-189` defines `SimpleApplication : NSApplication` conforming to `CefAppProtocol` / `CrAppProtocol` / `CrAppControlProtocol` (tracking `isHandlingSendEvent`), and overrides `terminate:` because "The default `-terminate:` implementation ends the process by calling exit(), and thus never leaves the main run loop. **This is unsuitable for Chromium since Chromium depends on leaving the main run loop to perform an orderly shutdown**" (`:126-162`). `setup_simple_application` asserts `NSApp` is that subclass and warns "If there was an invocation to NSApp prior to here, then the NSApp will not be a SimpleApplication" (`:204-216`).

For Aleph this is squarely **R1 territory**: `objc2`, `objc2-app-kit`, `NSApplication`, `NSTimer`, `NSRunLoop`. It cannot live in `src/`.

Note the OSR example **does not do any of this** — no `setup_simple_application`, only `LibraryLoader` (`examples/osr/src/main.rs:348-353`). It gets away with it because winit creates the NSApplication and OSR needs no CEF-owned NSView; whether that is actually correct on macOS is untested here.

### Message loop: three options, and which the OSR example picks

| Option | API | Constraint (quoted) |
|---|---|---|
| CEF owns the loop | `cef_run_message_loop()` / `cef_quit_message_loop()` | "should only be called on the main application thread and only if cef_initialize() is called with multi_threaded_message_loop = false. **This function will block**." (`sys/…:31026-31027`) |
| CEF on its own thread | `CefSettings.multi_threaded_message_loop` | "have the browser process message loop run in a separate thread… **This option is only supported on Windows and Linux.**" (`sys/…:18119-18120`) — **not available on macOS.** |
| Host pumps | `cef_do_message_loop_work()` + `external_message_pump` | "for cases where the CEF message loop must be integrated into an existing application message loop. **Use of this function is not recommended for most users**… care must be taken to balance performance against excessive CPU usage. It is recommended to enable external_message_pump… should only be called on the main application thread… **will not block**." (`sys/…:31022-31023`) |

`external_message_pump` doc adds (`sys/…:18121-18122`): "control browser process main (UI) thread message pump scheduling via `CefBrowserProcessHandler::OnScheduleMessagePumpWork()`… **Enabling this option is not recommended for most users**; leave it disabled and use either CefRunMessageLoop or multi_threaded_message_loop if possible."

**The osr example takes the third, explicitly-discouraged path** — `Settings { external_message_pump: true, .. }` (`examples/osr/src/main.rs:382`) plus a hand-rolled loop (`:400-410`):
```rust
loop {
    do_message_loop_work();
    let status = event_loop.pump_app_events(Some(Duration::ZERO), &mut app);
    if let PumpStatus::Exit(code) = status { break ... }
    sleep(Duration::from_millis(1000 / 17));   // ~59ms, i.e. ~17 Hz
}
```
That `sleep(1000/17)` is a **busy-poll at a fixed 17 Hz** with `ControlFlow::Poll` (`:397`), and — the tell — it declares `external_message_pump: true` but **never implements `on_schedule_message_pump_work`**. `rg 'on_schedule_message_pump_work' examples/osr/` → no hits. The `BrowserProcessHandler` it does install only implements `on_context_initialized` and `on_before_child_process_launch` (`examples/osr/src/webrender.rs:74-92`). So the whole point of `external_message_pump` — letting CEF tell you when work is due — is switched on and then ignored. **The 60 fps `windowless_frame_rate` cannot be achieved through a 17 Hz pump.**

`examples/tests_shared` **does** implement it properly (`examples/tests_shared/src/browser/main_message_loop_external_pump/mod.rs`): a `MainMessageLoopExternalPump` with `on_schedule_message_pump_work(delay)` → platform `set_timer` → `on_timer_timeout` → `do_work`, capped at `MAX_TIMER_DELAY = 1000/30` ms (`:31`), with a `reentrancy_detected` flag (`:57`). Platform backends: `NSTimer` on `NSRunLoopCommonModes` + `NSEventTrackingRunLoopMode` (`…/mac.rs:1-80`), Win32 timer, glib on Linux. **That is the file to port, not the osr example's loop.**

### Which thread do callbacks run on

`_cef_render_handler_t`'s own doc: "The functions of this structure **will be called on the UI thread**." (`sys/src/bindings/aarch64_apple_darwin.rs:28308`). Same for `_cef_print_handler_t` (`:28306`) and most client handlers.

`cef_thread_id_t` (`sys/…:19306-19323`):
- `TID_UI = 0` — "The main thread in the browser. This will be the same as the main application thread if CefInitialize() is called with multi_threaded_message_loop = false. **Do not perform blocking tasks on this thread.** This thread will outlive all other CEF threads."
- `TID_IO = 5` — "Used to process IPC and network messages. **Do not perform blocking tasks on this thread.**"
- Plus `TID_FILE_BACKGROUND/USER_VISIBLE/USER_BLOCKING`, `TID_PROCESS_LAUNCHER`, `TID_RENDERER`.

Cross-thread primitives: `cef_currently_on(thread_id) -> c_int` (`sys/…:29766`), `cef_post_task(thread_id, task)` (`sys/…:29770`), `cef_task_runner_get_for_thread` (`:29762`).

**Practical shape for a tokio host:** CEF's UI thread must be the process main thread (on macOS there is no alternative — `multi_threaded_message_loop` is Windows/Linux only). So tokio cannot own `main`; it has to be a runtime you enter/spawn onto, and every `on_paint` / `on_accelerated_paint` lands on the UI thread and must hand off to tokio via a channel without blocking. The `TEXTURE` thread-local in the osr example (`examples/osr/src/webrender.rs:408-410`) works **only because** the UI thread and the render thread are the same thread there; that pattern does not survive a tokio host.

### Reference counting / safety model

`cef/src/rc.rs` (380 LOC) implements `Rc`, `RcImpl<CefType, RustType>`, `RefGuard` over CEF's `cef_base_ref_counted_t`. Every wrapper is `Clone + Rc` and the generated `wrap_*!` macros build the vtable and the refcount plumbing (`cef/src/bindings/aarch64_apple_darwin.rs:23243`). Handlers must be `Clone` and are `&self` — so interior mutability (`RefCell`, `Mutex`) is mandatory, as the example shows (`examples/osr/src/webrender.rs:103,58`).

---

## 5. CDP from inside CEF — the decisive section for "one CDP-level interface"

**Answer up front: both channels exist, both are exposed safely by the `cef` crate, and the in-process one needs no websocket.**

### (a) The socket: a normal `--remote-debugging-port`

`CefSettings.remote_debugging_port: c_int` (`cef/src/bindings/aarch64_apple_darwin.rs:520`). CEF's doc (`sys/src/bindings/aarch64_apple_darwin.rs:18151`), verbatim on the parts that matter:
> "Set to a value **between 1024 and 65535** to enable remote debugging on the specified port. Also configurable using the **\"remote-debugging-port\" command-line switch**. Specifying **0 via the command-line switch will result in the selection of an ephemeral port and the port number will be printed as part of the WebSocket endpoint URL to stderr**. If a cache directory path is provided the port will also be written to the **`<cache-dir>/DevToolsActivePort`** file. Remote debugging can be accessed by loading the chrome://inspect page in Google Chrome. Port numbers 9222 and 9229 are discoverable by default."

So yes: an embedded CEF exposes the identical `/json/version`, `/json/list`, `ws://.../devtools/page/<id>` surface that Aleph's existing CDP client speaks to Chrome. **The `DevToolsActivePort` file and the ephemeral-port-to-stderr behaviour are exactly the two discovery mechanisms Aleph already deals with for Chrome** — same failure modes, same "Playwright injects its own port and yours loses" hazard, unchanged.

The osr example turns it on the crude way, as a command-line switch in `on_before_command_line_processing` (`examples/osr/src/webrender.rs:36-39`):
```rust
command_line.append_switch_with_value(Some(&"remote-debugging-port".into()), Some(&"9229".into()));
```
`rg 'dev_tools|DevTools|remote_debugging' examples/` → **that one line is the only DevTools-related code in every example.** There is no in-process CDP example anywhere in this repo.

### (b) The in-process channel: no websocket, no port

Three methods on `ImplBrowserHost` (`cef/src/bindings/aarch64_apple_darwin.rs:12609-12622`):
```rust
fn send_dev_tools_message(&self, message: Option<&[u8]>) -> c_int;
fn execute_dev_tools_method(&self, message_id: c_int, method: Option<&CefString>,
                            params: Option<&mut DictionaryValue>) -> c_int;
fn add_dev_tools_message_observer(&self, observer: Option<&mut DevToolsMessageObserver>)
    -> Option<Registration>;
```
Plus `show_dev_tools(window_info, client, settings, inspect_element_at)`, `close_dev_tools()`, `has_dev_tools() -> c_int` (`:12598-12608`). All are **safe Rust** on the generated `Browser`/`BrowserHost` wrappers — `send_dev_tools_message` takes `Option<&[u8]>` and the generated body passes pointer+len (`cef/src/bindings/…:13178-13188`).

CEF's own doc for `send_dev_tools_message` (`sys/src/bindings/aarch64_apple_darwin.rs:25507`), the load-bearing sentences:
> "|message| must be a **UTF8-encoded JSON dictionary that contains \"id\" (int), \"function\" (string) and \"params\" (dictionary, optional)** values. See the DevTools protocol documentation at https://chromedevtools.github.io/devtools-protocol/ … This function will return true (1) **if called on the UI thread** and the message was successfully submitted for validation… **Validation will be applied asynchronously and any messages that fail due to formatting errors or missing parameters may be discarded without notification.** Prefer ExecuteDevToolsMethod if a more structured approach to message formatting is desired.
> Every valid function call will result in an asynchronous function result or error message that references the sent message \"id\". **Event messages are received while notifications are enabled (for example, between function calls for \"Page.enable\" and \"Page.disable\").**
> **Usage of the SendDevToolsMessage, ExecuteDevToolsMethod and AddDevToolsMessageObserver functions does not require an active DevTools front-end or remote-debugging session. Other active DevTools sessions will continue to function independently.** However, any modification of global browser state by one session may not be reflected in the UI of other sessions.
> Communication with the DevTools front-end (when displayed) can be logged … by passing the `--devtools-protocol-log-file=<path>` command-line flag."

Two traps in that text worth flagging for a client implementation:
1. **"discarded without notification"** — a malformed message is a silent no-op that still returned `1`. Any Aleph CDP client over this transport needs its own per-`id` timeout; the transport will not tell you a message died.
2. **must be called on the UI thread** — returns 0 otherwise. From a tokio task you must `cef_post_task(TID_UI, …)` (`sys/…:29770`).

Note the wire vocabulary quirk: CEF's docs and its observer callbacks call the CDP method name **`"function"`**, not `"method"`. The `_cef_dev_tools_message_observer_t` doc says event dictionaries "include a **\"function\" (string)** value and optionally a \"params\" (dictionary) value" (`sys/…:22294`). Whether the JSON on the wire actually uses the key `method` (as real CDP does) and CEF's docs are just paraphrasing, or CEF genuinely renames it, is **not resolvable from this repo** — there is no example and no test. **This is the single thing to verify with a 20-line spike before designing around it**, because a wrong guess here breaks every message.

### `DevToolsMessageObserver` — the receive side

`ImplDevToolsMessageObserver` (`cef/src/bindings/aarch64_apple_darwin.rs:3716-3753`):
```rust
fn on_dev_tools_message(&self, browser: Option<&mut Browser>, message: Option<&[u8]>) -> c_int;
fn on_dev_tools_method_result(&self, browser: Option<&mut Browser>, message_id: c_int,
                              success: c_int, result: Option<&[u8]>);
fn on_dev_tools_event(&self, browser: Option<&mut Browser>, method: Option<&CefString>,
                      params: Option<&[u8]>);
fn on_dev_tools_agent_attached(&self, browser: Option<&mut Browser>);
fn on_dev_tools_agent_detached(&self, browser: Option<&mut Browser>);
```
with a `wrap_dev_tools_message_observer!` macro (`:3756`) exactly parallel to `wrap_render_handler!`.

`on_dev_tools_message` doc (`sys/…:22294`): "|message| is a UTF8-encoded JSON dictionary representing either a function result or an event. |message| is **only valid for the scope of this callback and should be copied**. Return true (1) if the message was handled or false (0) if the message should be further processed and passed to OnDevToolsMethodResult or OnDevToolsEvent." Result dicts carry `"id"` and either `"result"` or `"error"` (`{"code": int, "message": string}`). And a real perf note: "**some of which may exceed 1MB in size**" — which is precisely the `Page.captureScreenshot` / `Page.startScreencast` payload shape.

`add_dev_tools_message_observer` returns a `Registration`; "The observer will remain registered **until the returned Registration object is destroyed**" (`sys/…:25524`). Dropping the `Registration` silently unsubscribes — an easy accidental-`_` bug.

`execute_dev_tools_method` (`sys/…:25515`): "|message_id| is an incremental number that uniquely identifies the message (**pass 0 to have the next number assigned automatically**)… returns the assigned message ID if called on the UI thread and the message was successfully submitted for validation, **otherwise 0**." `params` is a `DictionaryValue`, i.e. you build a CEF dict rather than serializing JSON — awkward if Aleph's CDP client already produces `serde_json::Value`. **`send_dev_tools_message` with pre-serialized JSON bytes is the better fit for Aleph**, at the cost of the silent-discard hazard.

### Verdict for the dual-engine design

An Aleph CDP client could drive an embedded CEF **without a websocket** — the in-process channel is complete (send, structured-send, results, events, attach/detach) and needs neither a port nor a DevTools front-end. What it costs: a UI-thread hop per message, JSON copies at the boundary, and a transport that silently drops malformed messages. What it buys: no port allocation, no `DevToolsActivePort` race, no external process to lose, and no third party (Playwright) able to hijack the debugging port.

**But nothing in this repo demonstrates it.** Zero examples, zero tests. **not found: `rg 'send_dev_tools_message|add_dev_tools_message_observer' examples/ cef/src/wrapper/`.**

---

## 6. Browser/context features relevant to an agent engine

**Coverage answer first: essentially everything is wrapped as a safe Rust trait, not raw `sys`.** `rg -c '^pub trait Impl' cef/src/bindings/aarch64_apple_darwin.rs` → **154 traits**. The generator wraps every `_cef_*_t` interface, so "does the `cef` crate expose X safely" is nearly always yes; the real question is whether an *example* exists, and the answer there is nearly always no.

`ImplClient` (`cef/src/bindings/aarch64_apple_darwin.rs:27805-27880`) exposes 18 handler getters, all defaulting to `None`: `audio_handler`, `command_handler`, `context_menu_handler`, `dialog_handler`, `display_handler`, `download_handler`, `drag_handler`, `find_handler`, `focus_handler`, `frame_handler`, `permission_handler`, `jsdialog_handler`, `keyboard_handler`, `life_span_handler`, `load_handler`, `print_handler`, `render_handler`, `request_handler`.

### Per-browser isolation (cookie jars, profiles)

**Yes — two browsers in one process can have different cookie stores.** `browser_host_create_browser_sync(window_info, client, url, settings, extra_info, request_context)` takes a per-browser `RequestContext` (`cef/src/bindings/aarch64_apple_darwin.rs:57602-57609`); the async variant is at `:57534`. `request_context_create_context(settings, handler)` (`:57479-57482`).

`RequestContextSettings` (`cef/src/bindings/…:621-628`): `size`, **`cache_path`**, `persist_session_cookies`, `accept_language_list`, `cookieable_schemes_list`, `cookieable_schemes_exclude_defaults`.

CEF's doc on `RequestContextSettings.cache_path` (`sys/src/bindings/aarch64_apple_darwin.rs:18246`): "If this value is non-empty then it must be an absolute path that is either equal to or a child directory of `CefSettings.root_cache_path`. **If this value is empty then browsers will be created in \"incognito mode\" where in-memory caches are used** and no profile-specific data is persisted to disk. HTML5 databases such as localStorage will only persist across sessions if a cache path is specified. **To share the global browser cache and related configuration set this value to match the CefSettings.cache_path value.**"

And a hard operational constraint on `CefSettings.root_cache_path` (`sys/…:18129`): "**Multiple application instances writing to the same root_cache_path directory could result in data corruption. A process singleton lock based on the root_cache_path value is therefore used to protect against this.** This singleton behavior applies to all CEF-based applications using version 120 or newer… implement `CefBrowserProcessHandler::OnAlreadyRunningAppRelaunch`… **Failure to set the root_cache_path value correctly may result in startup crashes**." — Aleph already has a `flock`-based singleton on `~/.aleph/data/aleph.lock`; this would be a **second, independent singleton** with its own failure mode.

`ImplRequestContext` (`cef/src/bindings/…:11089-11178`) — the per-profile control surface, all safe: `is_same`, `is_sharing_with`, `is_global`, `cache_path`, **`cookie_manager(callback) -> Option<CookieManager>`**, `register_scheme_handler_factory`, `clear_scheme_handler_factories`, `clear_certificate_exceptions`, `clear_http_auth_credentials`, `close_all_connections`, `resolve_host`, `media_router`, `website_setting`/`set_website_setting`, `content_setting`/`set_content_setting`, `add_setting_observer`, **`clear_http_cache`**, plus preference access (`has_preference`, `preference`, `all_preferences`, `can_set_preference`). Global jar via `cookie_manager_get_global_manager(callback)` (`:57393`).

### Network interception

`ImplRequestHandler` (`cef/src/bindings/…:26751-26811`): `on_before_browse(browser, frame, request, user_gesture, is_redirect) -> c_int`, `on_open_urlfrom_tab`, **`resource_request_handler(browser, frame, request, is_navigation, is_download, request_initiator, disable_default_handling) -> Option<ResourceRequestHandler>`**, `auth_credentials(..., callback: &mut AuthCallback)`, `on_certificate_error(browser, cert_error, request_url, ssl_info, callback)`, and more.

The same `resource_request_handler` hook also hangs off `ImplRequestContextHandler` (`:28879-28890`) — i.e. **network policy can be attached per-profile, not just per-browser**. That is the right seam for an agent-level allow/deny list. `ImplResourceRequestHandler` is at `:25466`, with `ImplCookieAccessFilter` at `:26275`.

`ImplSchemeHandlerFactory` (`:33441`) + `register_scheme_handler_factory` gives custom schemes — useful for injecting agent-controlled resources without a local HTTP server.

### Dialogs, downloads, permissions

- `ImplJsdialogHandler` (`:20082-20114`): `on_jsdialog(browser, origin_url, dialog_type, message_text, default_prompt_text, callback, suppress_message) -> c_int`, `on_before_unload_dialog(...)`, `on_reset_dialog_state`, `on_dialog_closed`. The `suppress_message: Option<&mut c_int>` out-param is the "make alert() a no-op" switch an agent needs.
- `ImplDialogHandler` (`:17337-17355`): a single `on_file_dialog(...)` — file pickers.
- `ImplDownloadHandler` (`:18828-18859`): `can_download`, `on_before_download`, `on_download_updated`.
- `ImplPermissionHandler` (`:21854-21888`): `on_request_media_access_permission`, `on_show_permission_prompt`, `on_dismiss_permission_prompt`.
- `ImplLifeSpanHandler` (`:20682`) — `on_before_popup` (with a `no_javascript_access` out-param), `on_after_created`, `do_close`, `on_before_close`. This is how you keep `window.open` from spawning an unmanaged window.

### Load / display / focus / keyboard

- `ImplLoadHandler` (`:21386-21426`): `on_loading_state_change(is_loading, can_go_back, can_go_forward)`, `on_load_start(frame, transition_type)`, `on_load_end(frame, http_status_code)`, `on_load_error(error_code, error_text, failed_url)`. **This is a first-class navigation-lifecycle stream without CDP.**
- `ImplDisplayHandler` (`:17586-17662`): `on_address_change`, `on_title_change`, `on_favicon_urlchange`, `on_fullscreen_mode_change`, `on_tooltip`, `on_status_message`, **`on_console_message(level, message, source, line) -> c_int`**, `on_auto_resize`, `on_loading_progress_change(progress: f64)`, **`on_cursor_change(cursor, type_, custom_cursor_info) -> c_int`**, `on_media_access_change`, `on_contents_bounds_change`. Cursor and console live here, not on `RenderHandler`.
- `ImplFocusHandler` (`:19531`), `ImplKeyboardHandler` (`:20456`), `ImplContextMenuHandler` (`:16286`), `ImplCommandHandler` (`:14189`).

### PDF

`browser_host.print_to_pdf(path, settings: Option<&PdfPrintSettings>, callback: Option<&mut PdfPrintCallback>)` (`cef/src/bindings/…:12580-12586`) plus `print()` (`:12579`). Caveat in the doc (`sys/…:25464`): "**For PDF printing to work on Linux you must implement the `cef_print_handler_t::GetPdfPaperSize` function.**" `ImplPrintHandler` is at `:22729`; its C doc says "Implement this structure to handle printing **on Linux**" (`sys/…:28306`).

### Audio

`ImplAudioHandler` (`:13878-13911`): `audio_parameters`, `on_audio_stream_started`, **`on_audio_stream_packet`**, `on_audio_stream_stopped`, `on_audio_stream_error` — raw PCM out of the page. Nothing in Aleph needs this today, but it is the seam for capturing a meeting's audio without a virtual device.

### Command-line switches — yes, fully

`App::on_before_command_line_processing(process_type, command_line)` gives you the real `CefCommandLine` before Chromium parses it. The osr example uses it to add `no-startup-window`, `noerrdialogs`, `hide-crash-restore-bubble`, **`use-mock-keychain`**, `enable-logging=stderr`, and `remote-debugging-port=9229` (`examples/osr/src/webrender.rs:22-40`). `BrowserProcessHandler::on_before_child_process_launch` does the same for sub-processes; the example adds `disable-web-security`, `allow-running-insecure-content`, `disable-session-crashed-bubble`, `ignore-certificate-errors`, `ignore-ssl-errors` (`examples/osr/src/webrender.rs:79-90`) — note those are exactly the flags an automation host wants, and exactly the flags a security review will ask about.

`ImplCommandLine` (`:28502`) exposes `append_switch`, `append_switch_with_value`, `has_switch`, `switch_value`, etc. `CefSettings.command_line_args_disabled` (`:507`) can lock out external command-line configuration entirely — "Configuration can still be specified using CEF data structures or via `CefApp::OnBeforeCommandLineProcessing`" (`sys/…:18125`). **That is a genuinely useful hardening switch Chrome-as-external-process does not give you.**

### The hand-written `cef/src/wrapper/` helpers

`cef/src/wrapper/mod.rs:1-8` — the only non-generated runtime code besides `string`/`rc`/`osr_texture_import`:
- `message_router.rs` (1784 LOC) + `message_router_utils.rs` (726) — a port of CEF's `CefMessageRouterBrowserSide`/`RendererSide`: a JS `window.cefQuery({request, onSuccess, onFailure})` ⇄ Rust request/response bridge. **This is an alternative to CDP `Runtime.evaluate` for page↔host RPC**, and it is the biggest hand-written asset in the crate.
- `resource_manager.rs` (1155) — a port of CEF's `CefResourceManager`: serve URLs from directories, zip archives, or providers.
- `stream_resource_handler.rs`, `byte_read_handler.rs`, `zip_archive.rs`, `browser_info_map.rs`.

---

## 7. Shipping cost

### Archive naming and variant

`cef-rs` fetches **only the `minimal` build**: `CefVersion::minimal()` filters the CDN index for `file_type == "minimal"` (`download-cef/src/lib.rs:378-383`) and every download path calls it (`:253`, `:389`). There is no code path that fetches the `standard`/`client`/`debug` archives. Archive filename comes from the index's `name` field; a real one appears in a comment in the Windows manifest (`cef/src/build_util/win/cef-app.exe.manifest:4`):
```
https://cef-builds.spotifycdn.com/cef_binary_132.3.2+g4997b2f+chromium-132.0.6834.161_windows64.tar.bz2
```
Platform keys in the index: `macosarm64`, `macosx64`, `windows64`, `windowsarm64`, `windows32`, `linux64`, `linuxarm64`, `linuxarm` (`download-cef/src/lib.rs:154-163`).

**Archive size: NOT STATED anywhere in this repo.** not found: `rg -in 'MB|GB|megabyte|archive size' README.md download-cef/ export-cef-dir/ .github/` → zero hits. Any number you have for "~200 MB" comes from outside this repo and should be re-measured against the actual `index.json` `minimal` entry for the target version.

### What must ship next to the executable

The repo answers this by construction, per platform:

**Linux** (`sys/build.rs:159-165` + `cef/src/build_util/linux.rs:18-26`): copy the **entire** `cef_linux_<arch>` directory next to the binary, **plus** its `locales/` subdirectory. The extraction step already flattened CEF's `Release/` and `Resources/` into that one directory (`download-cef/src/lib.rs:517-527`), so that means `libcef.so`, `libEGL.so`, `libGLESv2.so`, `libvk_swiftshader.so`, `*.pak`, `icudtl.dat`, `v8_context_snapshot.bin`, `chrome-sandbox`, `vk_swiftshader_icd.json`, and `locales/*.pak` — as a directory copy, not an enumerated list. Requires `LD_LIBRARY_PATH` to include it (README, `export-cef-dir/README.md`).

**Windows** (`sys/build.rs:166-169` + `cef/src/build_util/win/mod.rs:19-27`): same directory copy plus `locales/`, but **excluding `.exe` files** (`:86-91`), plus a generated `<app>.exe.manifest` (`:44-47`, content at `cef/src/build_util/win/cef-app.exe.manifest` — Common-Controls v6 dependency, `asInvoker`, supportedOS GUIDs through Win10). With the `sandbox` feature the app is built as a **cdylib** and CEF's own `bootstrap.exe` is copied and renamed to `<app>.exe` (`:49-68`); the `.pdb` ships too. Requires `PATH` to include the dir.

**macOS** (`cef/src/build_util/mac.rs:61-100`): a real `.app`:
```
<App>.app/Contents/MacOS/<App>
<App>.app/Contents/Resources/…            (+ .icns, compiled .nib from .xib via `xcrun ibtool`)
<App>.app/Contents/Frameworks/Chromium Embedded Framework.framework/   (whole framework, recursive copy)
<App>.app/Contents/Frameworks/<App> Helper.app/
<App>.app/Contents/Frameworks/<App> Helper (GPU).app/
<App>.app/Contents/Frameworks/<App> Helper (Renderer).app/
<App>.app/Contents/Frameworks/<App> Helper (Plugin).app/
<App>.app/Contents/Frameworks/<App> Helper (Alerts).app/
```
The `.pak`s, `icudtl.dat` and `v8_context_snapshot.bin` are inside the framework on macOS — CEF's doc: "the `*.pak` files must be located in the module directory on Windows/Linux or the **app bundle Resources directory on MacOS**" and `locales_dir_path` "is **ignored on MacOS** where pack files are always loaded from the app bundle Resources directory" (`sys/src/bindings/aarch64_apple_darwin.rs:18147,18150`).

`sys/build.rs` **does not** copy anything on macOS — the comment says so explicitly: "On macOS it's more complicated so we'll **leave it to tools like tauri-cli** for now" (`sys/build.rs:160-161`, repeated `:168`).

### Runtime provisioning — can Aleph download CEF at install time?

**Partly, and less cleanly than for Chromium.** The pieces:
- `export-cef-dir` is a standalone CLI that downloads + extracts a CEF dist into any directory (`export-cef-dir/src/main.rs:44-135`), SHA1-verified, resumable-ish (it re-verifies an existing archive and re-downloads on mismatch, `download-cef/src/lib.rs:259-273`), with retry (`:331-348`). Its logic is a library (`download-cef`), so Aleph could vendor the same fetch into its runtime ledger.
- **But the CEF dist is a *build-time* dependency, not just a runtime one.** `sys/build.rs` needs the `include/` headers and `libcef_dll/` sources to compile `libcef_dll_wrapper` with cmake+Ninja (`sys/build.rs:134-145`), and it pins `CEF_API_VERSION` from those exact headers. The comment at `sys/build.rs:123-131` spells out the consequence: **"loading any libcef but this exact build fails on the first one missing"** entry point. So you cannot compile once and provision an arbitrary CEF later; the shipped libcef must match the CEF version the binary was compiled against.
- There is **no `dlopen`-with-fallback path**. On Linux/Windows libcef is a normal dynamic link resolved by the loader; on macOS `cef_load_library(path)` is called with a path resolved **relative to the executable inside the bundle** (`cef/src/library_loader.rs:11-23`), not from an arbitrary provisioned directory. Pointing it elsewhere would mean not using `LibraryLoader` and calling `sys::cef_load_library` yourself — possible (`cef/src/lib.rs:63` does exactly that in tests) but off the paved path.

**Verdict:** install-time provisioning is feasible in the "download a version-pinned blob next to the binary" sense (which is what Linux/Windows already are), but it is **version-locked to the build**, unlike Aleph's Chromium ledger where any recent Chrome will do. That is a materially different operational contract.

### Sandbox

- **macOS**: `cef/src/sandbox.rs` — `Sandbox::new()` `dlopen`s `../../../Chromium Embedded Framework.framework/Libraries/libcef_sandbox.dylib` (`:12-13`) and calls `cef_sandbox_initialize(argc, argv)` / `cef_sandbox_destroy` (`:38,57`). Used only in the helper binary, gated on the `sandbox` feature (`examples/cefsimple/src/bin/cefsimple_helper.rs:6-11`). **The `sandbox` feature is ON by default** (`cef/Cargo.toml:17`).
- **Windows**: the sandbox requires building the app as a **DLL** loaded by CEF's `bootstrap.exe` — `cefsimple`'s `main` literally refuses to run otherwise: `Err("Running in sandbox mode on Windows requires bootstrap.exe or bootstrapc.exe.")` (`examples/cefsimple/src/main.rs:24-27`). That is a **hard shape constraint on the host binary**, not a flag.
- **Disabling**: `CefSettings.no_sandbox` — "Set to true (1) to disable the sandbox for sub-processes… Also configurable using the **\"no-sandbox\"** command-line switch" (`sys/…:18111`). `cefsimple` sets `no_sandbox: !cfg!(feature = "sandbox")` (`examples/cefsimple/src/shared/mod.rs:56`). **The osr example sets neither** — it uses `Settings::default()` plus two OSR flags (`examples/osr/src/main.rs:380-384`), i.e. `no_sandbox: 0`, sandbox on.
- Linux: `chrome-sandbox` SUID binary ships in the dist directory; nothing in this repo configures it. **not found: `rg 'chrome-sandbox' .`**

### Codesigning / notarization

**Nothing. not found: `rg -in 'codesign|notariz|entitlement|hardened runtime' .` → zero hits.** `SECURITY.md` is a stub — it is the generic Tauri security policy and says "We will be adding contact information to this page very soon."

This is a real gap for a shipped macOS app: five helper `.app`s plus a framework each need signing with the right entitlements (Chromium needs `com.apple.security.cs.allow-unsigned-executable-memory` / `allow-jit`, and the helpers need distinct entitlement sets), and `bundle-cef-app` does none of it. Aleph's Tauri pipeline would have to own that entirely.

### Build-host requirements (easy to underestimate)

From `sys/build.rs` and CI (`.github/workflows/rust.yml`):
- **cmake + Ninja** on every build host (`sys/build.rs:136-137`).
- A **C++ toolchain** (MSVC on Windows with `CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded`, and `CMAKE_OBJECT_PATH_MAX=500` — a tell that the path lengths hit Windows limits, `sys/build.rs:190-191`).
- Linux CI installs `libglib2.0-dev` (`.github/workflows/rust.yml:29`).
- CI caches `~/.local/share/cef` keyed on `hashFiles('Cargo.toml')` and runs `export-cef-dir` on a miss — i.e. **every clean CI build downloads the CEF archive**.
- CI runs exactly `cargo fmt --check`, `cargo build`, `cargo test` on ubuntu/macos/windows-latest. **x86_64 only — no ARM Linux, no ARM Windows, no cross-compilation job**, despite bindings being committed for all 8 triples.

---

## 8. Maturity

### Cadence — active, but visibly slowing

`git log --since=2026-06-01 --oneline | wc -l` → **31 commits** in the last ~3 months. Total history 1110 commits since "Init commit" Mon May 22 2023.

Commits per month, most recent 16:

| Month | Commits |
|---|---:|
| 2025-06 | 50 |
| 2025-07 | 66 |
| 2025-08 | 45 |
| 2025-09 | 72 |
| 2025-10 | 92 |
| 2025-11 | 51 |
| 2025-12 | 47 |
| 2026-01 | 53 |
| 2026-02 | 39 |
| 2026-03 | 33 |
| 2026-04 | 47 |
| 2026-05 | 14 |
| 2026-06 | 8 |
| 2026-07 | 10 |
| 2026-08 | 8 |
| 2026-09 | 5 |

That is a step change in May 2026: from ~40-90/month for a year to **8-14/month for the last four months**. Read it honestly — the CEF-version treadmill is automated so the repo stays *current* without much human work, but feature velocity has dropped a lot.

Contributors (`git shortlog -sn --all`): **Bill Avery 587**, github-actions[bot] 200, csmoe 125, renovate[bot] 59, Gonzalo Ruiz 31, Wu Yu Wei 28, Lucas Nogueira 26, dependabot 18, then a long tail. **53% of all commits are one person; ~25% are bots.** Bus factor 1.

### Automation

- **release-plz** on push to `dev` (`.github/workflows/release-plz.yml`), publishing to crates.io with an org token. Config is one line: `repo_url` (`release-plz.toml`).
- **`get-latest`** runs on a **daily cron** (`.github/workflows/get-latest.yml:4-5`), queries the CDN index for the newest stable across all 8 targets, bumps the workspace version, regenerates the changelog with git-cliff, pushes a `get-latest` branch via the GraphQL `createCommitOnBranch` API, then chains into `update-bindings` and opens a PR. This is why the version is `151.8.1+151.3.24` and tracks CEF within days.
- `bindings-changed.yml` re-runs bindgen when `Cargo.toml`, `sys/wrapper.h`, or `update-bindings/**` change.
- renovate (`config:recommended`) + dependabot for GitHub Actions.

Recent commits confirm the pattern: `chore: update bindings (#459/#457/#454)`, `get latest (#451/#449/#448/#446/#444)`, `chore(deps): update wgpu to v30`, `chore(deps): update syn to v3`.

### Unsafe density

Generated code dominates: **2,495 `unsafe` occurrences in a single triple's `cef/src/bindings/*.rs`** — and there are 8 such files. That is inherent (every vtable thunk is `unsafe extern "C"`), and it is machine-generated from one generator, which is the mitigating factor.

Hand-written `unsafe` (`rg -c "unsafe" cef/src --glob '!bindings' --glob '!resources'`), highest last:
```
library_loader.rs                  1
osr_texture_import/common.rs       2
wrapper/byte_read_handler.rs       2
wrapper/stream_resource_handler.rs 2
sandbox.rs                         3
osr_texture_import/iosurface.rs    4
osr_texture_import/d3d11.rs        5
application_mac.rs                 7
osr_texture_import/dmabuf.rs       9
wrapper/message_router_utils.rs   11
rc.rs                             28
string.rs                         69
```
`string.rs` and `rc.rs` are the two files carrying the real audit burden — UTF-16 conversion and refcounting.

### Tests — thin

`rg '#\[test\]' -g '*.rs' .` → **16 total**, and 8 of those are one auto-generated test replicated per triple in `cef/src/resources/*.rs`. Real hand-written tests: 3 in `cef/src/string.rs`, 2 in `sys/src/lib.rs` (one of which just asserts `get_cef_dir()` is `Some`), 1 in `cef/src/bindings/mod.rs` (a `DictionaryValue` out-param round-trip, `:81-117`), 1 in `examples/tests_shared/src/browser/util_win.rs`. **There is no `tests/` directory anywhere** (`find . -type d -name tests` → empty).

`examples/tests_shared` is **not a test suite** — despite the name it is a port of CEF's `tests/shared/` *support library* (`examples/tests_shared/src/lib.rs` is three `pub mod` lines) containing the message-loop implementations, `ClientApp`, resource utils. Nothing runs it. Its most valuable content for a host is the external-pump implementation discussed in §4.

CI runs `cargo build` + `cargo test` on ubuntu/macos/windows-latest x86_64 only (`.github/workflows/rust.yml`), i.e. **the ARM Linux, ARM Windows and 32-bit Windows bindings are compiled by nobody.**

### TODO / unimplemented density

Very low in hand-written code. The complete list outside generated bindings:
- `sys/build.rs:106` — "TODO: far from ideal, but there's no other way to get the target dir" (cargo issue 9661).
- `sys/build.rs:217` — `unimplemented!("unknown target {os}")`.
- `cef/src/osr_texture_import/iosurface.rs:112` — `_ => unimplemented!()` on any pixel format that is not RGBA8/BGRA8. **This is a panic in the OSR hot path**, reachable if CEF ever hands you a different `cef_color_type_t`.
- `examples/cefsimple/src/shared/simple_handler/mod.rs:27` — `todo!("Implement platform_show_window for non-macOS platforms")`.
- `update-bindings/src/upgrade.rs:103` — `panic!("unsupported {v:?}")`.

### Tauri / wry integration

**There is none in this repo.** `rg 'tauri|wry'` outside Cargo metadata and changelogs yields exactly three substantive hits:
- `sys/build.rs:161` and `:168` — the same comment twice: "On macOS it's more complicated so we'll **leave it to tools like tauri-cli** for now."
- `cef/src/bin/bundle-cef-app/mac.rs:30` — default bundle identifier `apps.tauri.cef-rs.<name>`.

So cef-rs is a **tauri-apps-owned crate that Tauri does not yet use**. No wry backend, no Tauri webview integration, no shared window handle plumbing. **not found: `rg 'wry'` → zero hits outside Cargo.lock-adjacent metadata.**

---

## 9. Licenses

- **cef-rs itself**: `Apache-2.0 OR MIT` (`Cargo.toml:22`, `LICENSE-APACHE`, `LICENSE-MIT` — MIT © 2023 Wu Yuwei).
- **CEF**: BSD-3-Clause (not restated in this repo; the downloaded archive carries its own `LICENSE.txt`).
- **Chromium and its ~1000 third-party components**: the extraction step preserves **`CREDITS.html`** from the archive and moves it into the CEF dir (`download-cef/src/lib.rs:543-544`), and the changelog records "Include CREDITS.html in extracted cef dir" as a deliberate fix. **That file is the attribution artifact you must ship** — it is the aggregate third-party notice for the whole Chromium tree.
- **Codecs / DRM**: **not found: `rg -in 'widevine|proprietary|ffmpeg|h264|aac|drm' --glob '!*/bindings/*'` → zero relevant hits.** The Spotify CDN builds are the standard open-source CEF builds; they do **not** bundle Widevine, and proprietary-codec support (H.264/AAC) in those builds is whatever `chromiumembedded` compiled with, which this repo neither states nor controls. If Aleph needs to play H.264 video or DRM content in an embedded browser, **that is an unanswered question this repo cannot answer** and would require checking the specific CDN build's GN flags.
- The archive also ships `CMakeLists.txt`, `cmake/`, `include/`, `libcef_dll/` (CEF's own BSD-licensed C++ wrapper sources, compiled into your binary as `libcef_dll_wrapper`) — so **CEF's BSD-3 code is statically linked into the host binary**, not merely dynamically loaded.

---

## Where CEF could sit in Aleph's dual-engine design

Grounded against Aleph's actual browser layer: `src/browser/{backend,chromium_launch,chromium_resolve,playwright_cli_backend,profile,network_policy,discovery,tab_registry}.rs` and `desktop/{macos,windows,linux,shell}`. Live-view screencast is **not yet built** — `rg -l 'startScreencast|screencast' src/ interfaces/` → **no hits**, so item (b) compares CEF against a plan, not against shipped code.

1. **(a) As the Chromium escape hatch, CEF removes the two failure classes that hurt most.** No external process to lose, and no debugging port for a third party to steal — the in-process channel (§5) needs neither a port nor `DevToolsActivePort`. That directly retires the "Playwright injects its own random debugging port and yours loses" hazard recorded in `[[project-browser-live-view-design-round]]`, and the daemon-lifecycle problems behind `[[project-browser-plan1-launch-chain-execution]]`.

2. **(a, against) It replaces those with a worse dependency: CEF is a build-time dependency, not a runtime one.** `sys/build.rs:123-131` — the shipped libcef must match the headers it compiled against or "loading any libcef but this exact build fails on the first one missing." Aleph's Chromium ledger works because *any* recent Chrome will do; CEF is version-locked to the binary. You trade a discovery problem for a distribution problem.

3. **(a) The `RequestContext` model maps cleanly onto what Aleph already has.** `src/browser/profile.rs` ⇄ `RequestContextSettings.cache_path` (per-browser cookie jars in one process, §6); `src/browser/network_policy.rs` ⇄ `get_resource_request_handler` attachable **per profile** (`cef/src/bindings/…:28879`). Plus one thing Chrome-as-subprocess cannot give: `command_line_args_disabled` (`sys/…:18125`) locks out external command-line configuration entirely.

4. **(a, watch out) CEF adds a second process singleton.** `root_cache_path` has its own lock and "**Multiple application instances writing to the same root_cache_path could result in data corruption**" (`sys/…:18129`). Aleph already has a `flock` singleton on `~/.aleph/data/aleph.lock`; these two would be independent, with independent failure modes, and CEF's manifests as a startup crash.

5. **(b) For the human live view, OSR is genuinely better than screencast — but not in the example's form.** `on_accelerated_paint` hands you an IOSurface / D3D11 handle / DMA-BUF (§2) with a per-frame `timestamp` and `capture_counter` in `AcceleratedPaintInfo::extra` — no JPEG encode, no CDP round-trip. Against that: `Page.startScreencast` produces JPEG bytes that can be pushed to a **remote** Panel over the existing WebSocket. **A shared texture cannot leave the machine.** Aleph's live view has to work from a browser on another host, so OSR serves the desktop-app case only, and screencast (or a CPU `on_paint` → encode path) remains necessary for the Panel.

6. **(b) The osr example is a wrong template on two counts, both correctable but both real work.** It caches a bind group pointing at a **pooled texture CEF reclaims when the callback returns** (`examples/osr/src/webrender.rs:266-268` vs CEF's "cannot be cached and cannot be accessed outside of this callback", `sys/…:28384`), and it declares `external_message_pump: true` while never implementing `on_schedule_message_pump_work`, pumping instead at a fixed 17 Hz (`examples/osr/src/main.rs:400-410`) under a 60 fps setting. Port `examples/tests_shared/.../main_message_loop_external_pump/`, not the osr loop.

7. **(b) OSR gives Aleph the "render only when a viewer is attached" primitive it wants, for free.** `external_begin_frame_enabled` + `send_external_begin_frame()` puts the frame clock in the host (`sys/…:25564`), and `was_hidden(true)` stops layout and paints entirely (`sys/…:25553`). `windowless_frame_rate` bottoms out at 1 fps, so `was_hidden` — not the frame rate — is the real off switch.

8. **(b, cost) Everything around the pixels is unimplemented in this repo.** Popup compositing (`PET_POPUP` layer + `on_popup_size`), cursor (`DisplayHandler::on_cursor_change`), IME, and **all input injection** are absent from the example (§2, §3). Input in particular needs a winit/browser-event → **Windows virtual-key-code** mapping (`sys/…:19943`) that cef-rs does not ship. Budget this as its own milestone, not as "wire up the example."

9. **(c) Only the full desktop app can host CEF. The standalone `aleph-server` cannot, and `--headless` does not rescue it.** OSR *is* the headless-equivalent (no window is created), but the process still needs the GPU/renderer helper processes and, on macOS, an `NSApplication` on the main thread; on a headless Linux box you would need the full CEF dist plus GL/Vulkan userspace and would land on the CPU `on_paint` fallback (`cef/src/osr_texture_import/common.rs:38-69`) with an empty texture and only a `tracing::warn!` to tell you. Panel-only shell obviously cannot host it. **So CEF is a one-artifact-out-of-three capability** — which conflicts with R6 "一核多端, Rust Core is the only brain".

10. **(d) R1 placement is unusually invasive: CEF wants the process `main`, not a bridge.** `cef_execute_process` must run before anything else (`sys/…:30997`); on macOS `multi_threaded_message_loop` is unavailable (`sys/…:18119`) so CEF's UI thread **is** the main thread, and the host must subclass `NSApplication` (`examples/cefsimple/src/mac/mod.rs:102-189`). The `objc2` / `NSTimer` / `NSRunLoop` / helper-bundle work belongs in `desktop/macos` and `desktop/shell`, and the five-helper `.app` layout (`cef/src/build_util/mac.rs:228-234`) belongs in the Tauri bundling step. **But the part that cannot be pushed into a bridge is the ownership of `main` and the tokio-must-not-own-the-main-thread inversion.** That is not an R1 exception like `src/sandbox/*`; it is a different shape of constraint and deserves an explicit ruling before any code.

11. **(e) Memory and latency: same engine, so expect Chrome's numbers, plus OSR's own overhead.** CEF 151 is Chromium 151 — identical renderer, identical per-tab memory. Embedding *saves* one browser-process worth of RAM versus a separate Chrome, but adds: a compositor→shared-texture pool, a UI-thread hop per CDP message, and either a per-frame GPU import (accelerated) or a full `width*height*4` BGRA copy per frame (CPU path, `sys/…:28371` — 1080p ≈ 8.3 MB **per frame**, since the buffer is whole every time, not dirty-rect deltas). **Nothing in this repo measures any of it. not found: any benchmark, `rg -l 'criterion|bench' .` → none.**

12. **Top 3 risks.** (i) **Bus factor and velocity** — 53% of 1110 commits are one person, ~25% are bots, and monthly commits fell from ~50 to ~8 after May 2026 (§8); the CEF treadmill is automated, feature work is not, and there are **16 tests total, no `tests/` directory**. (ii) **The macOS shipping tail nobody has walked** — five helper bundles plus a framework, with **zero codesigning or notarization support in the repo** (`rg -in 'codesign|notariz|entitlement'` → nothing) and a stub `SECURITY.md`; Chromium's JIT entitlements are non-negotiable and entirely Aleph's problem. (iii) **The CDP-wire-format unknown** — CEF's docs call the CDP method key `"function"`, not `"method"` (`sys/…:22294`, `:25507`), there is no example and no test, and a wrong guess breaks every message. **A 20-line spike that sends one `Page.enable` and prints the raw observer bytes should gate any design decision that assumes Aleph's existing CDP client drops in unchanged.**
