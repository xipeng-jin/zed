# Design mode: cropped element screenshot pipeline research

Research for the design-mode element picker (map #56): when the user picks a
DOM element, how should we produce the cropped element screenshot PNG?
Two candidates: **(a)** crop the software-OSR frame we already receive from
CEF, or **(b)** ask the engine via CDP `Page.captureScreenshot` with a `clip`
rect. Compared against the reference implementation in orca
(`~/Projects/refs/orca`, Electron, feature called "grab").

Evidence tiers: **[verified]** = read in source on disk or fetched from the
official repo/spec as cited; **[documented]** = stated by official docs;
**[inferred]** = deduced, not directly confirmed; **[estimate]** = back-of-
envelope arithmetic.

## 1. TL;DR — recommendation

**Crop the OSR frame (option a).** One JS-bridge round trip hides the picker
overlay and re-measures the element rect + `window.innerWidth`; we wait for
the next `on_paint` frame (generation counter), copy the cropped rows out of
the retained frame on the foreground thread, and `cx.background_spawn` the
BGRA→RGBA swizzle + PNG encode with the `image` crate already in the
dependency tree. Scale mapping uses orca's empirical
`bitmap_width / innerWidth` trick, which captures device-scale-factor × page
zoom in one number.

Why not CDP: it is *available* — cef-rs 150 verifiably exposes
`execute_dev_tools_method` / `add_dev_tools_message_observer`, and the CEF
header states no DevTools front-end or remote-debugging session is required
([§3](#3-option-b-cdp-pagecapturescreenshot)) — but for this feature it buys
nothing and costs a lot: `clip` is in DIP, not CSS pixels, so the page-zoom
conversion problem remains identical ([§4](#4-sub-question-1-dpr-and-page-zoom-scaling));
`Page.captureScreenshot` under CEF *windowless* rendering has a history of
defects (crash with `fromSurface:false`, duplicated content with clip
`scale > 1` — CEF issues #2979/#3103); the result arrives as base64 over an
async observer that needs new message-id-correlation machinery with no
in-crate precedent; and it forces a compositor snapshot readback when we
already hold a device-resolution BGRA buffer of the exact visible viewport
in process memory. The prior picker research reached the same conclusion
from the other direction ("Element screenshots … or cropping the existing
software OSR frame … which needs no CDP at all",
`docs/glass/design-mode-picker-research.md`). Keep CDP in reserve for the one
thing option (a) cannot do: capturing content beyond the visible viewport
(`captureBeyondViewport`) or cross-origin iframe internals.

## 2. Option (a): crop the OSR frame

### What the engine hands us today

- CEF's `OnPaint` contract: "|buffer| will be |width|*|height|*4 bytes in
  size and represents a BGRA image with an upper-left origin", and "Pixel
  values passed to this method are scaled relative to view coordinates based
  on the value of CefScreenInfo.device_scale_factor returned from
  GetScreenInfo" — i.e. the buffer is **device pixels**, view size is DIP
  ([cef_render_handler.h](https://github.com/chromiumembedded/cef/blob/master/include/cef_render_handler.h))
  **[documented]**.
- Our handler reports the view rect in logical pixels and the scale factor
  via `screen_info` (`crates/browser/src/render_handler.rs:57-87`), and
  `on_paint` copies the full BGRA buffer (`byte_len = width * height * 4`)
  into `RenderState.frame` as a `SoftwareFrame`, then signals
  `TabBackendEvent::FrameReady` (`render_handler.rs:106-131`) **[verified]**.
- `SoftwareFrame` is documented at the seam as "a tightly-packed
  premultiplied BGRA buffer at physical (device) pixel resolution"
  (`crates/browser/src/tab_backend.rs:142-148`); `set_viewport` documents
  "The engine paints at `size * scale_factor` device pixels"
  (`tab_backend.rs:188-190`) **[verified]**.
- Frame flow: `CefTab::take_paint_output` *moves* the frame out of the
  shared `Arc<Mutex<RenderState>>` (`crates/browser/src/cef_tab.rs:510-516`);
  `BrowserTab::present_pending_frame` hands it to the presenter
  (`crates/browser/src/browser_tab.rs:519-524`); `SoftwarePresenter` wraps
  the owned `Vec<u8>` in an `image::ImageBuffer` inside an
  `Arc<RenderImage>` and **retains the latest full frame** in
  `SoftwarePresenter.latest` (`crates/browser/src/frame_presenter.rs:43-66,
  82`) **[verified]**.
- Alpha today: nothing touches it. The presenter uses `ImageBuffer` "only as
  a container: gpui treats `RenderImage` bytes as BGRA when uploading"
  (`frame_presenter.rs:58-66`). The CEF header only promises "BGRA"; our
  seam comment says premultiplied. For an ordinary opaque page the buffer's
  alpha channel is 255 everywhere, so premultiplied and straight coincide
  **[inferred]**. Recommendation: in the PNG path, force alpha to 255 during
  the swizzle — it is free, makes the premultiplication question moot, and
  avoids shipping translucent PNGs for pages with transparent backgrounds.

### Encoding

- The `image` crate is already a direct dependency of the browser crate
  (`crates/browser/Cargo.toml:30`, workspace pin `image = "0.25.1"` at
  `Cargo.toml:625`, resolving to 0.25.10 in `Cargo.lock`) **[verified]**.
  `image` 0.25 encodes PNG from RGBA8 via `RgbaImage`/`DynamicImage`
  `write_to(&mut cursor, ImageFormat::Png)` or
  `codecs::png::PngEncoder` ([docs.rs/image/0.25](https://docs.rs/image/0.25.10/image/codecs/png/struct.PngEncoder.html))
  **[documented]**.

### The capture sequence (proposed)

1. Payload arrives over the JS bridge with the element's
   `getBoundingClientRect` in CSS pixels (picker decision, ticket #57).
2. Run one more bridge script: hide the overlay, **re-measure** the element
   rect, and return `{rect, innerWidth}` (see [§7](#7-sub-question-4-where-the-crop-runs)
   and [§9](#9-sub-question-6-frame-freshness--race)).
3. Wait for the next `FrameReady`/presented frame (generation counter,
   [§5](#5-sub-question-2-overlay-exclusion)), with a timeout fallback.
4. On the foreground thread, compute `scale = frame_width / innerWidth`,
   map + clamp the rect to device pixels, memcpy the cropped rows out of the
   presenter's retained `Arc<RenderImage>`
   (`RenderImage::as_bytes`/`size` are public,
   `crates/gpui/src/assets.rs:72-87` **[verified]**).
5. Restore the overlay (bridge script, in a `finally`-equivalent that runs
   regardless of capture success — orca's pattern).
6. `cx.background_spawn`: BGRA→RGBA swizzle (alpha forced to 255), PNG
   encode, enforce the size budget, return the bytes to the view.

## 3. Option (b): CDP `Page.captureScreenshot`

### Availability — verified, the map's claim is true

- cef-rs 150 (the pinned engine binding: crate `cef` 150.0.0+150.0.10 from
  `git+https://github.com/tauri-apps/cef-rs?tag=cef-v150.0.0+150.0.10`,
  `Cargo.lock`) exposes on `ImplBrowserHost`:
  `send_dev_tools_message` (bindings
  `~/.cargo/git/checkouts/cef-rs-1a4e65a2c484707b/c73f792/cef/src/bindings/x86_64_unknown_linux_gnu.rs:12668`),
  `execute_dev_tools_method` (`:12670-12676`), and
  `add_dev_tools_message_observer` returning a `Registration` guard
  (`:12677-12681`), plus an implementable observer:
  `DevToolsMessageObserver` (`:3755`), callbacks `on_dev_tools_message` /
  `on_dev_tools_method_result` / `on_dev_tools_event` (`:3776-3801`), and a
  `wrap_dev_tools_message_observer!` macro (`:3814`) **[verified]**.
- No debug port needed: the CEF header states "Usage of the
  SendDevToolsMessage, ExecuteDevToolsMethod and AddDevToolsMessageObserver
  methods does not require an active DevTools front-end or remote-debugging
  session"
  ([cef_browser.h](https://github.com/chromiumembedded/cef/blob/master/include/cef_browser.h))
  **[documented]**. Our `remote-debugging-port=9222` is debug-build,
  env-gated only (`crates/browser/src/cef_instance.rs:205-214`)
  **[verified]** and irrelevant to this channel.
- Threading: these methods must be called on the CEF UI thread
  ([cef_browser.h](https://github.com/chromiumembedded/cef/blob/master/include/cef_browser.h)).
  Glass runs CEF with `external_message_pump = 1` driven by
  `do_message_loop_work` on the GPUI foreground thread
  (`crates/browser/src/cef_instance.rs:426, 508`), so the foreground thread
  *is* the CEF UI thread and can call them directly **[verified]**.

### The method itself

Per the official protocol docs
([Page.captureScreenshot](https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-captureScreenshot))
**[documented]**:

- Params: `format` ("Image compression format (defaults to png)":
  jpeg/png/webp), `quality` ("[0..100] (jpeg only)"), `clip` (a `Viewport`:
  "Capture the screenshot of a given region only"), `fromSurface`
  ("Capture the screenshot from the surface, rather than the view. Defaults
  to true", experimental), `captureBeyondViewport` ("Capture the screenshot
  beyond the viewport. Defaults to false", experimental),
  `optimizeForSpeed` ("Optimize image encoding for speed, not for resulting
  size (defaults to false)", experimental).
- Returns `data`: "Base64-encoded image data".
- `Viewport` fields x/y/width/height are documented as "device independent
  pixels (dip)" plus `scale`: "Page scale factor".

**Clip coordinate space in the implementation** — Chromium's
`content/browser/devtools/protocol/page_handler.cc` scales the clip offset
by the widget's device scale factor
(`modified_params.viewport_offset.Scale(widget_host_device_scale_factor)`)
and does **not** apply the page zoom factor; the screenshot is produced via
`widget_host->GetSnapshotFromBrowser(...)`, a compositor-surface snapshot
readback
([page_handler.cc](https://github.com/chromium/chromium/blob/main/content/browser/devtools/protocol/page_handler.cc))
**[verified against fetched source]**. Consequences:

- `clip` is DIP, and under page zoom DIP ≠ CSS px (DIP = CSS px ×
  zoom factor), so the caller must apply the same zoom correction option (a)
  needs — CDP does not make the scaling problem go away ([§4](#4-sub-question-1-dpr-and-page-zoom-scaling)).
- Every capture forces a fresh compositor snapshot + readback + PNG encode
  in the browser process, versus reusing a buffer we already hold
  **[verified]** (that it snapshots; the relative cost is **[inferred]**).

### Behavior notes

- The capture is of the page's compositor surface: no OS cursor is included
  (the cursor is not page content; in OSR there is no cursor in `on_paint`
  either — both paths are cursor-free) **[inferred]**; in-page DOM overlays
  **are** included — DOM content is DOM content — so the overlay-hide dance
  of [§5](#5-sub-question-2-overlay-exclusion) is required in both options.
- Known CEF-specific defects under windowless (OSR) mode:
  - "OSR: call Page.captureScreenshot causes crash in some cases" (with
    `fromSurface: false`) — CEF issue
    [#2979](https://bitbucket.org/chromiumembedded/cef/issues/2979/osr-call-pagecapturescreenshot-causes).
  - "OffScreen: Capture Screenshot with DevTools Protocol and Viewport
    Scale > 1 results in wrong image" (duplicated page content; absent in
    windowed mode; traced to `SetSize` being unimplemented in
    `CefRenderWidgetHostViewOSR`) — CEF issue
    [#3103](https://github.com/chromiumembedded/cef/issues/3103).
  Both are old (CEF 88 era) and the crash needs a non-default flag, but they
  show CDP screenshot under OSR is the less-exercised path in CEF, while
  `on_paint` is the path every OSR embedder exercises every frame
  **[documented]**.
- Plumbing cost: async result via `on_dev_tools_method_result` with
  message-id correlation, base64 decode of a potentially multi-megabyte
  string, `Registration` lifetime management — machinery with no in-crate
  precedent (same argument as the picker research made for extraction,
  `docs/glass/design-mode-picker-research.md`, "Why not as the primary
  channel").

### Where CDP stays relevant

`captureBeyondViewport: true` can capture the whole document, including
content outside the visible viewport — impossible from the OSR frame, which
only ever contains the visible viewport. If design mode later needs
"screenshot this element even though it's half scrolled off-screen" or
cross-origin iframe capture, that is the moment to build the CDP channel.
For v1, scroll-into-view before picking (or accepting viewport-clipped
crops, as orca does) is adequate.

## 4. Sub-question 1: DPR and page-zoom scaling

**The facts:**

- The OSR buffer is view size × `device_scale_factor` device pixels,
  regardless of page zoom
  ([cef_render_handler.h](https://github.com/chromiumembedded/cef/blob/master/include/cef_render_handler.h):
  pixel values "scaled relative to view coordinates based on …
  device_scale_factor") **[documented]**; our `screen_info` supplies GPUI's
  `window.scale_factor()` (`crates/browser/src/render_handler.rs:68-87`,
  `crates/browser/src/browser_view.rs:2217, 820-826`) **[verified]**. Page
  zoom re-lays-out content within the same physical surface.
- `window.innerWidth` "must return the viewport width including the size of
  a rendered scroll bar (if any)" in CSS pixels, and the spec distinguishes
  "page zoom which affects the size of the initial viewport" from the visual
  viewport scale factor
  ([CSSOM View](https://drafts.csswg.org/cssom-view/#dom-window-innerwidth))
  **[documented]**. So as page zoom increases, the viewport gets *smaller*
  in CSS px and `innerWidth` shrinks; `getBoundingClientRect` values live in
  this same zoomed CSS-px space.
- Therefore `frame_width_px / innerWidth = device_scale_factor ×
  page_zoom_factor` — one empirical number capturing both. This is exactly
  orca's derivation, with orca's own rationale: "Rather than using the
  primary display (which is wrong on multi-monitor setups with mixed DPI),
  we derive the scale factor empirically … This is correct regardless of
  which display the window is on"
  (`~/Projects/refs/orca/src/main/browser/browser-grab-screenshot.ts:59-70`)
  **[verified]**.
- The compositional alternative exists: cef-rs exposes
  `ImplBrowserHost::zoom_level()` / `set_zoom_level()` (bindings
  `x86_64_unknown_linux_gnu.rs:12613-12615`; UI-thread-only per
  [cef_browser.h](https://github.com/chromiumembedded/cef/blob/master/include/cef_browser.h))
  **[verified]**, and Chromium converts level→factor as
  `std::pow(kTextSizeMultiplierRatio, zoom_level)` with the ratio constant
  1.2 ("Change the zoom factor by 20% for each zoom level increase")
  ([third_party/blink/common/page/page_zoom.cc](https://github.com/chromium/chromium/blob/main/third_party/blink/common/page/page_zoom.cc))
  **[verified against fetched source]**. So
  `scale = scale_factor × 1.2^zoom_level` is computable Rust-side.

**Verdict: use the empirical `frame_width / innerWidth` trick.** Reasons:
(1) it is measured in the same coordinate system the rect was measured in,
at the same moment, by the same script — no cross-API composition to drift
(the 1.2 constant is a Chromium implementation detail, and glass currently
has no zoom plumbing at all — zero grep hits for zoom in
`crates/browser/src/*.rs` **[verified]**); (2) it needs no extra round trip:
the overlay-hide script already crosses the bridge and can return
`innerWidth` alongside the re-measured rect; (3) it is orca-proven in
production. Guard it like orca: reject `innerWidth <= 0` or non-finite
(`browser-grab-screenshot.ts:66-69`). One caveat orca tolerates and so can
we: a classic (non-overlay) scrollbar is included in `innerWidth` but also
occupies frame pixels, so the ratio stays correct; fractional zoom levels
give non-integer scale, handled by the rounding rules in
[§8](#8-sub-question-5-clamping--nan-guards).

**Rounding:** map CSS rect → device px with `x0 = floor(x·s)`,
`y0 = floor(y·s)`, `x1 = ceil((x+w)·s)`, `y1 = ceil((y+h)·s)`, then clamp
each to `[0, frame_dim]` and reject if the result is empty. Floor/ceil
(rather than orca's `Math.round` on width, `browser-grab-screenshot.ts:73-76`)
guarantees the element's painted edge pixels are included at fractional
scales; clamping after expansion keeps it in bounds.

## 5. Sub-question 2: overlay exclusion

The picker's hover highlight is in-page DOM (ticket #57 decision), so it
appears in the OSR frame *and* in any CDP screenshot alike. It must be
hidden for the capture.

**Orca's sequence** (`browser-grab-screenshot.ts:43-53`) **[verified]**:

1. `executeJavaScript(HIDE_...)` — sets `display:none` on the grab overlay
   host and on every `[data-orca-browser-annotation-overlay]` element,
   saving each prior inline display value in a `data-orca-previous-display`
   attribute (`:4-11`), with `.catch(() => {})`.
2. `image = await guest.capturePage()` inside `try`.
3. `executeJavaScript(RESTORE_...)` (`:13-20`) in a **`finally`**, also
   error-swallowed, "so the overlay is always restored even if capturePage()
   throws (e.g., guest destroyed mid-capture)" (`:44-47`).

Notably orca does **not** await a `requestAnimationFrame` or timeout between
hide and capture — it relies on Electron's `capturePage` producing a frame
composited after the style change has been committed **[verified — no rAF/
timeout exists in the file]**.

**Our OSR equivalent cannot skip the wait.** `take_paint_output` returns the
*most recent already-painted* frame (`cef_tab.rs:510-516`); capturing
immediately after the hide script would crop a frame that still shows the
overlay. Required sequencing:

1. Add a monotonically increasing frame generation counter to `RenderState`,
   bumped in `on_paint` (`render_handler.rs:106-131` is the single writer).
2. Bridge script hides the overlay (and returns rect + `innerWidth`).
3. Record `g0 = generation`; wait until `generation > g0` via the existing
   `FrameReady` event flow (`render_handler.rs:130` →
   `browser_tab.rs:314`) — the style change invalidates and repaints, so a
   new frame arrives without prodding. Add a timeout fallback (~250 ms) that
   captures the current frame anyway rather than failing: a stale-but-
   overlay-bearing screenshot on a pathological page beats no screenshot,
   and the timeout also covers pages that were fully idle (though a real
   `display:none` on a visible overlay always produces a paint) **[inferred]**.
4. Crop, then restore the overlay via the bridge in all exit paths
   (success, clamp-reject, timeout, tab-gone) — the Rust equivalent of
   orca's `finally`.

If the picker's UX tears the overlay down on click anyway (selection ends
the session), the hide step degenerates to "wait one generation after
teardown", same machinery.

## 6. Sub-question 3: size budget

**Orca's numbers** (`~/Projects/refs/orca/src/shared/browser-grab-types.ts`)
**[verified]**:

- `screenshotMaxBytes: 2 * 1024 * 1024` — "Hard byte budget for screenshot
  PNG data URL before we omit the screenshot" (`:188-189`). Enforced after
  encode: `if (pngBuffer.byteLength > GRAB_BUDGET.screenshotMaxBytes) return
  null` (`browser-grab-screenshot.ts:87-89`), with the explicit rationale
  "downscaling would add complexity for v1. Fail closed to 'no screenshot'
  rather than send an oversized payload" (`:85-86`).
- Everything else in the payload is budgeted too (text snippets 200 chars,
  html snippet 4096, etc., `browser-grab-types.ts:172-190`), so 2 MB
  dominates the payload size.

**Scale check for us [estimate]:** a full-viewport pick at 1600×1000 CSS px
on a 2× display is 3200×2000 = 6.4 Mpx = 25.6 MB raw BGRA. Screenshot-like
content (flat fills, text) typically PNG-compresses 5–20×, so most UI crops
land well under 2 MB, but a full-viewport crop of photographic content can
exceed it. A 2000×1500 device-px element crop is 3 Mpx = 12 MB raw.

**Recommendation:** adopt orca's contract: hard 2 MiB budget on the encoded
PNG, fail closed (payload ships with `screenshot: null` — orca's payload
type makes the screenshot nullable and non-fatal,
`browser-grab-types.ts:79-93`, `useGrabMode.ts:118-128`). Additionally clamp
the *input* before encoding: cap the crop at the frame bounds (automatic —
the OSR frame is only ever the visible viewport) and skip encoding when the
raw crop exceeds ~32 MB (protects the background thread from pathological
`innerWidth` skew). Do not downscale in v1, for orca's stated reason.

## 7. Sub-question 4: where the crop runs

**How the frame is shared today [verified]:**
`RenderState.frame` lives in `Arc<Mutex<RenderState>>` shared between the
CEF paint callback and the backend (`cef_tab.rs:118, 132-138`;
`render_handler.rs:19-26`). `take_paint_output` **moves** the
`SoftwareFrame` (a plain `{u32, u32, Vec<u8>}`, `tab_backend.rs:144-148` —
`Send` by construction) out of the mutex; the presenter then owns it inside
`Arc<RenderImage>` and retains the latest full frame across renders
(`frame_presenter.rs:44, 82`, drop discipline `:87-105`). So at any moment
after first paint, `SoftwarePresenter.latest` holds the current complete
frame, readable via the public `RenderImage::as_bytes(0)` / `size(0)`
(`crates/gpui/src/assets.rs:72-87`).

**Recommendation:** do the *crop copy* on the foreground thread and the
*swizzle + encode* on a background thread:

- Foreground: after the generation wait, borrow the presenter's retained
  frame (or take the pending `SoftwareFrame` before presentation), compute
  the clamped device-px rect, and memcpy `height` row-slices into a fresh
  `Vec<u8>`. A 2000×1500 crop is a 12 MB sequential copy — low
  single-digit milliseconds **[estimate]**, acceptable on the UI thread.
- Background: `cx.background_spawn` receives the owned crop `Vec` (plain
  `Send` data — no lock crosses threads), performs the BGRA→RGBA channel
  swap with alpha forced to 255, PNG-encodes via `image` 0.25, applies the
  2 MiB budget, and resolves back on the foreground. PNG encoding of a
  multi-megapixel image is tens of milliseconds **[estimate]** — too long
  for the frame budget, trivial for a background task.

Cloning the `Arc<RenderImage>` into the background task also works
(`RenderImage`'s pixel data is immutable), but copying only the crop keeps
the retained-frame drop discipline of `SoftwarePresenter` untouched and
minimizes the data crossing threads. Avoid holding the `RenderState` mutex
across any of this: it is contended by `on_paint` from the CEF pump.

## 8. Sub-question 5: clamping & NaN guards

**Orca's guards [verified]:**

- Screenshot side (`browser-grab-screenshot.ts:31-41`): every rect field is
  passed through `safeN` — `typeof n === 'number' && Number.isFinite(n) ? n
  : fallback(0)` — "so NaN cannot reach Electron's image.crop() and cause
  undefined behavior".
- Mapping (`:72-80`): `cropX = max(0, round(x·s))`, `cropY = max(0,
  round(y·s))`, `cropW = min(bitmapW − cropX, round(w·s))`, `cropH =
  min(bitmapH − cropY, round(h·s))`; reject when `cropW <= 0 || cropH <= 0`
  (negative widths from out-of-range x fall through to this check).
- Payload side, defense in depth (`browser-grab-payload.ts:51-52, 136-147`):
  `safeNum` (same finite check) and `safeRect` re-clamp every guest-supplied
  rect again in the main process, because "the guest payload is completely
  untrusted" (`:19-21`).
- Everything is wrapped in a global `try/catch` returning `null` — capture
  is always fail-closed (`browser-grab-screenshot.ts:101-105`).

**Rust-side equivalent (spec):** the rect and `innerWidth` arrive as JSON
numbers from page-controlled JS over the bridge — untrusted.

```text
fn clamp_capture_rect(rect: {x,y,w,h}: f64×4, inner_width: f64,
                      frame_w: u32, frame_h: u32) -> Option<DeviceRect>
- reject unless all of x, y, w, h, inner_width are f64::is_finite()
- reject unless inner_width > 0 and w > 0 and h > 0
- scale = frame_w as f64 / inner_width; reject unless scale.is_finite() && scale > 0
- x0 = (x·s).floor().clamp(0.0, frame_w as f64) as u32   (same for y0 vs frame_h)
- x1 = ((x+w)·s).ceil().clamp(x0 as f64, frame_w as f64) as u32  (same for y1)
- reject if x1 == x0 || y1 == y0  (empty after clamp)
```

`clamp` before the `as u32` cast keeps the float-to-int conversion in range
(Rust saturates float casts, but clamping first makes intent explicit and
handles the negative side). No indexing without these bounds: row copies use
`x0..x1` slices of rows `y0..y1`, all proven in-bounds by construction.

## 9. Sub-question 6: frame freshness / race

**Orca [verified]:** the rect is measured at click time inside the guest
payload (`getBoundingClientRect` at
`grab-guest-script.ts:686`, shipped as `rectViewport`, `:716-721`); the
renderer then makes a *separate* IPC round trip to capture
(`useGrabMode.ts:117-127` — after `awaitGrabSelection` resolves, it calls
`captureSelectionScreenshot({rect: result.payload.target.rectViewport})`,
handled at `ipc/browser.ts:414-437` → `browser-manager.ts:1793-1799`).
There is **no re-measurement** at capture time; the window between click and
capture is a few IPC hops, and any scroll/animation in that window shifts
the crop. Orca accepts this. (Its `isFixed` flag, `grab-guest-script.ts`,
travels in the payload but does not influence the screenshot.)

**Ours can be strictly better for free:** the overlay-hide script of
[§5](#5-sub-question-2-overlay-exclusion) already executes in the page at
capture time, and the picker's guest runtime holds the picked element
reference (orca does the same for `extractHover`,
`browser-manager.ts:1801-1815`). Re-measure `getBoundingClientRect` in that
same script and use *that* rect for the crop — the measurement and the
`innerWidth` sample are then atomically consistent with each other and only
one frame older than the pixels we crop. Residual race (page animates
between the re-measure and the next `on_paint`) is inherent to any
non-atomic capture and is the same one orca ships with; capture immediately
after the first post-hide frame to minimize it. If the element left the DOM
or scrolled fully out of the viewport, the re-measure returns an
empty/out-of-bounds rect and [§8](#8-sub-question-5-clamping--nan-guards)
fails the screenshot closed — correct behavior.

## 10. Appendix A: orca's exact behaviors (file:line)

All paths under `~/Projects/refs/orca/`.

| Behavior | Location |
|---|---|
| Hide-overlay script (display:none on `__orcaGrab.host` + annotation overlays, saves prior display in `data-orca-previous-display`) | `src/main/browser/browser-grab-screenshot.ts:4-11` |
| Restore-overlay script (restores saved display, removes attribute) | `browser-grab-screenshot.ts:13-20` |
| NaN/finite guard `safeN` on rect fields (why-comment: keep NaN out of `image.crop()`) | `browser-grab-screenshot.ts:31-41` |
| hide → `capturePage()` in `try` → restore in `finally`, both scripts `.catch(()=>{})`; no rAF/timeout between hide and capture | `browser-grab-screenshot.ts:47-53` |
| Full-surface capture, no rect passed to `capturePage` (crop done by hand afterwards) | `browser-grab-screenshot.ts:50` |
| Empty-image bail | `browser-grab-screenshot.ts:54-56` |
| Empirical scale: `scaleFactor = bitmapSize.width / (await 'window.innerWidth')`, reject `innerWidth <= 0`; multi-monitor rationale in comment | `browser-grab-screenshot.ts:58-70` |
| CSS→bitmap mapping: `round` + `max(0,·)` on x/y, `min(bitmap − origin, round(·))` on w/h; reject `<= 0` | `browser-grab-screenshot.ts:72-80` |
| Crop then `toPNG()` | `browser-grab-screenshot.ts:82-83` |
| 2 MB fail-closed check post-encode; "downscaling would add complexity for v1" | `browser-grab-screenshot.ts:85-89`, budget constant `src/shared/browser-grab-types.ts:188-189` |
| Reported width/height divided back to CSS px for payload consistency | `browser-grab-screenshot.ts:92-100` |
| Whole function `try/catch → null` (guest teardown mid-capture) | `browser-grab-screenshot.ts:101-105` |
| Main-side re-clamp of untrusted payload: `safeNum` finite check, `safeRect` | `src/main/browser/browser-grab-payload.ts:51-52, 136-147` |
| Rect measured at click: `getBoundingClientRect` → `rectViewport` (CSS px) + `rectPage` (+scroll); page block carries `innerWidth/innerHeight`, `devicePixelRatio` | `src/main/browser/grab-guest-script.ts:686, 690-697, 716-727` |
| Screenshot is a second IPC after selection resolves, using click-time `rectViewport`; failure non-fatal (`screenshot: null`) | `src/renderer/src/components/browser-pane/useGrabMode.ts:104-135`, `src/main/ipc/browser.ts:414-437`, `src/main/browser/browser-manager.ts:1793-1799` |
| Screenshot payload shape: `{mimeType:'image/png', dataUrl, width, height}` | `src/shared/browser-grab-types.ts:79-84` |

## 11. Appendix B: our seams (file:line)

All paths under `/home/xjin/Projects/zed/`.

| Seam | Location |
|---|---|
| `RenderState` (logical w/h + `scale_factor` + latest unconsumed `frame`) behind `Arc<Mutex<_>>` | `crates/browser/src/render_handler.rs:16-37`, `crates/browser/src/cef_tab.rs:118, 132-138` |
| `view_rect` (DIP) and `screen_info.device_scale_factor` fed to CEF | `crates/browser/src/render_handler.rs:57-87` |
| `on_paint`: BGRA copy (`w·h·4`), stores `SoftwareFrame`, emits `FrameReady` — the single place a generation counter goes | `crates/browser/src/render_handler.rs:106-131` |
| `SoftwareFrame` ("tightly-packed premultiplied BGRA … physical (device) pixel resolution") / `PaintOutput` | `crates/browser/src/tab_backend.rs:142-154` |
| `TabBackend::set_viewport` contract ("paints at `size * scale_factor` device pixels") / `take_paint_output` | `crates/browser/src/tab_backend.rs:188-190, 282`, impl `crates/browser/src/cef_tab.rs:276-286, 510-516` |
| Frame pump: `FrameReady` → `needs_notify` → render calls `present_pending_frame` + `render_frame` | `crates/browser/src/browser_tab.rs:264, 314, 519-528`, `crates/browser/src/browser_view.rs:2190-2196` |
| Presenter retains latest full frame as `Arc<RenderImage>`; BGRA passed through unconverted | `crates/browser/src/frame_presenter.rs:43-66, 82` |
| Public read access to retained pixels: `RenderImage::as_bytes` / `size` | `crates/gpui/src/assets.rs:72-87` |
| Content bounds + `window.scale_factor()` known Rust-side every draw | `crates/browser/src/browser_view.rs:816-826, 2215-2222` |
| CEF external message pump on GPUI foreground (foreground == CEF UI thread) | `crates/browser/src/cef_instance.rs:426, 500-530` |
| Debug-only remote-debugging port (not needed for in-process CDP) | `crates/browser/src/cef_instance.rs:205-214` |
| cef-rs CDP surface: `send_dev_tools_message` / `execute_dev_tools_method` / `add_dev_tools_message_observer`; observer + wrap macro; `zoom_level`/`set_zoom_level` | `~/.cargo/git/checkouts/cef-rs-1a4e65a2c484707b/c73f792/cef/src/bindings/x86_64_unknown_linux_gnu.rs:12668-12681, 3755, 3776-3801, 3814, 12613-12615` |
| Engine binding pin: crate `cef` 150.0.0+150.0.10, `tauri-apps/cef-rs` tag `cef-v150.0.0+150.0.10` | `Cargo.lock` (`name = "cef"` block), `crates/browser/Cargo.toml:48` |
| `image` crate: workspace `0.25.1` (locked 0.25.10), direct dep of browser crate | `Cargo.toml:625`, `crates/browser/Cargo.toml:30`, `Cargo.lock` |
| Prior decision context: picker overlay in-page JS + bridge; CDP verified available, reserved for screenshots/iframes | `docs/glass/design-mode-picker-research.md` (Option (c) section) |

## 12. External sources

- CDP `Page.captureScreenshot` + `Viewport`:
  <https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-captureScreenshot>
- CEF `cef_browser.h` (DevTools methods, no-front-end-required note, zoom
  level methods, UI-thread requirements):
  <https://github.com/chromiumembedded/cef/blob/master/include/cef_browser.h>
- CEF `cef_render_handler.h` (`GetViewRect` DIP, `OnPaint` BGRA/device-px
  contract):
  <https://github.com/chromiumembedded/cef/blob/master/include/cef_render_handler.h>
- Chromium screenshot implementation (clip × device-scale-factor, no zoom
  scaling; `GetSnapshotFromBrowser`):
  <https://github.com/chromium/chromium/blob/main/content/browser/devtools/protocol/page_handler.cc>
- Chromium zoom level↔factor (`1.2^level`):
  <https://github.com/chromium/chromium/blob/main/third_party/blink/common/page/page_zoom.cc>
- CSSOM View (`innerWidth` in CSS px incl. scrollbar; page zoom vs visual
  viewport): <https://drafts.csswg.org/cssom-view/#dom-window-innerwidth>
- CEF OSR × CDP screenshot defects:
  <https://bitbucket.org/chromiumembedded/cef/issues/2979/osr-call-pagecapturescreenshot-causes>,
  <https://github.com/chromiumembedded/cef/issues/3103>
