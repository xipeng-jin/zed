# Design-mode element picker: architecture research (ticket #57)

Research for wayfinder ticket #57 (part of #56, design mode): how design mode
arms the element picker and gets the clicked element's data back to Rust.
Options compared: (a) in-page JS overlay (orca's "grab" shape), (b) Rust-side
click interception, (c) CDP. All claims below are cited to code read on
2026-07-17; facts marked *(inference)* are reasoned, not verified by execution.

## TL;DR — recommendation

**Adopt (a), the in-page JS overlay, with one deliberate deviation from orca:
the click itself is intercepted Rust-side (option (b)'s mechanism).**
Concretely:

- **Arm/disarm**: `frame.execute_java_script(...)` on the main frame from the
  browser process — no browser→renderer process message, no new
  render-process-handler surface for activation.
- **Hover feedback**: entirely in-page (orca's shadow-root overlay + highlight
  box). Nothing streams to Rust during hover.
- **Click**: while armed, `browser_view.rs::handle_mouse_down` does **not**
  forward the mouse-down to CEF; instead it issues a `finalize` script at the
  click position. This guarantees the page never receives the click — the
  guarantee orca explicitly wanted and could not get from Electron (see
  [orca's own comment](#what-orcas-experience-says) below).
- **Payload return**: the established render-process bridge pattern —
  a native V8 function installed at `on_context_created` calls
  `frame.send_process_message(ProcessId::BROWSER, …)` →
  `client.rs::on_process_message_received` → new
  `TabBackendEvent::ElementPicked` → `browser_view.rs::drain_engine_events` —
  exactly the `zed.page_chrome` / `zed.text_input_state` precedent.
- **Seam extension (minimal)**: two commands
  (`set_element_picker_armed(bool)`, `finalize_element_pick(position)`) and one
  event (`ElementPicked(Option<ElementPickPayload>)`).

Option (c) is **more viable than the ticket assumed** — cef-rs exposes CEF's
in-process CDP channel (`execute_dev_tools_method` /
`add_dev_tools_message_observer`), which needs neither `ZED_CEF_DEBUG` nor the
TCP port — but it is still the wrong primary channel for this feature (see
[Option (c)](#option-c-cdp)). Keep it in reserve for element screenshots.

## What exists today (verified)

**The seam.** `TabBackend` is the trait through which all engine communication
flows; "nothing above it may touch CEF"
(`crates/browser/src/tab_backend.rs:156-165`). Commands are trait methods
(`tab_backend.rs:165-283`); engine state returns through the
`TabBackendEvent` enum (`tab_backend.rs:96-140`), sent from handler threads
over an mpsc channel (`tab_backend.rs:285-302`) and drained on the foreground
thread in `browser_view.rs::drain_engine_events` (`browser_view.rs:721-814`).
Per-tab state lives **above** the seam: "tab state lives above the seam in
`browser_view.rs`" (`crates/browser/src/cef_tab.rs:1-3`).

**The one-render-process-handler constraint.** Quoting
`crates/browser/src/page_chrome.rs:8-10` verbatim:

> The render-process handler here also owns the focused-node editability
> signal (`text_input.rs`), because an engine app registers exactly one
> render-process handler.

The single handler is registered app-wide in
`crates/browser/src/cef_instance.rs:224-226`, with the comment (lines 221-223)
"Runs in the render subprocess (the same App is passed to `execute_process`)".
Any picker code that must run at context-creation time therefore extends
`PageChromeRenderProcessHandler` (`page_chrome.rs:616-696`), not a new handler.

**The render→browser payload precedent (twice-proven).** At
`on_context_created`, the handler installs a native V8 function on the page's
global (`__zedReportChromeColor`, `page_chrome.rs:636-659`) and evaluates an
observer script (`page_chrome.rs:661-676`). The V8 function packs a JSON
string into a `ProcessMessage` named `zed.page_chrome` and sends it with
`frame.send_process_message(ProcessId::BROWSER, …)`
(`page_chrome.rs:574-602`). The browser process receives it in
`client.rs::on_process_message_received` (`crates/browser/src/client.rs:159-187`),
decodes it (`page_chrome.rs:534-548`), and forwards a `TabBackendEvent`.
`text_input.rs` uses the same message plumbing under the name
`zed.text_input_state` (`crates/browser/src/text_input.rs:155-176`).

**Input today.** All page input is forwarded by `browser_view.rs`:
`handle_mouse_down` (`browser_view.rs:1404-1420`) forwards to
`tab.send_mouse_down(...)`, which calls `host.send_mouse_click_event`
(`cef_tab.rs:300-313`). Keystrokes are already classified and routed Rust-side
(`browser_view.rs:1496`, `:2325`), so an armed-mode Esc-to-cancel can be
handled entirely in Rust.

**CDP today.** Debug-only: `ZED_CEF_DEBUG` sets `remote-debugging-port=9222`
under `#[cfg(debug_assertions)]` (`crates/browser/src/cef_instance.rs:205-214`).

## What orca's experience says

Orca (Electron; the feature is called "grab") is the reference shape.

**The injected overlay** (`refs/orca/src/main/browser/grab-guest-script.ts`):
a full-viewport host `div` with `pointer-events:all` and a crosshair cursor,
appended to `documentElement` with `z-index:2147483647`
(grab-guest-script.ts:741-744), holding a **closed shadow root** for the
highlight box and hover label (`:746-762`). Hover hit-testing temporarily sets
the host to `pointer-events:none`, calls `document.elementFromPoint`, and
restores it (`:804-812`). Extraction (selector building, accessibility,
computed-style subset, React fiber component names and `_debugSource` file
paths, secret redaction, hard text/HTML budgets) all happens in-page
(`:685-734`), with a second validation/clamp pass in the main process
(`browser-grab-session-controller.ts:151`, `browser-grab-payload.ts`).
Anti-tamper: arm unconditionally tears down any pre-existing
`window.__orcaGrab` because "a malicious guest page could predefine
window.__orcaGrab with a fake extractPayload function"
(grab-guest-script.ts:53-63).

**The lifecycle** (`refs/orca/src/main/browser/browser-grab-session-controller.ts`):
one active op per tab (`:97-105`), a 120 s hard timeout (`:22`, `:188-195`),
cancel on **main-frame** navigation only — "Subframe navigations (e.g., iframe
ads loading) should not spuriously cancel the grab" (`:171-182`) — and cancel
on webContents destruction (`:184-186`). Payload kinds are a discriminated
union `selected | context-selected | cancelled | error`
(`shared/browser-grab-types.ts:105-112`).

**The documented compromise that decides our architecture**
(browser-grab-session-controller.ts:80-89, comment on `awaitGrabSelection`):

> Why the click is handled in-guest rather than via main-side interception:
> Electron's `before-input-event` only fires for keyboard events, not mouse
> events on guest webContents. The design doc anticipated a main-owned
> interceptor, but the spike showed this API gap. The fallback … is to let the
> guest overlay's full-viewport hit-catcher consume the click. … This is not a
> perfect guarantee (capture-phase listeners on window may still fire), but it
> covers the vast majority of sites.

Orca's in-guest click was a **fallback for a platform limitation we do not
have**. In our OSR architecture every mouse event passes through
`browser_view.rs::handle_mouse_down` before CEF ever sees it — we have the
main-owned interceptor orca's design doc wanted. Gating it while armed makes
click suppression a hard guarantee instead of "covers the vast majority of
sites".

**A second orca difference that matters:** orca's payload rides the return
value of Electron's `executeJavaScript(...)` Promise
(browser-grab-session-controller.ts:130-169). **CEF's
`execute_java_script` returns nothing** — it is fire-and-forget (see next
section). So under CEF, *whichever* option arms the picker, the payload must
come back over the process-message bridge. This is why option (a) and option
(b) share their return path here, unlike in Electron.

## Per-option analysis

### Option (a): in-page JS overlay (orca's shape)

- **Mechanics.** Bridge function installed once per context by the existing
  render-process handler (extend `page_chrome.rs::on_context_created`,
  honoring the one-handler constraint); arm/teardown/finalize scripts injected
  on demand via `execute_java_script`; hover highlight rendered in-page inside
  a closed shadow root; payload JSON returned via the bridge →
  `zed.element_pick` process message → `TabBackendEvent`.
- **Lifecycle.** Arm on entering design mode; hover feedback is purely
  in-page (zero IPC per mousemove); click extracts and reports; disarm tears
  the overlay down. Armed state lives above the seam in `BrowserView`, which
  also owns the timeout (orca uses 120 s) and Esc-to-cancel (keys are already
  Rust-routed, `browser_view.rs:1496`).
- **Navigation resilience.** The overlay and its listeners die with the old
  V8 context; the **bridge auto-heals** because `on_context_created` runs for
  every new context (`page_chrome.rs:622-677` — the same reason theme-color
  observing survives navigation). Since armed state lives Rust-side, the
  existing `AddressChanged`/`LoadingStateChanged` events
  (`tab_backend.rs:103-111`) let `BrowserView` either cancel the pick (orca's
  choice, recommended) or re-arm after load. No new events needed for this.
- **Seam impact.** Smallest of the three: two commands, one event (below).
- **Weaknesses.** Runs in the page's main world (CEF injects into the same
  world where `on_context_created`'s global lives; there is no isolated-world
  API on `execute_java_script`) — a hostile page can fight the overlay, delete
  the bridge, or feed fake payloads. Orca accepts and documents the same
  (grab-guest-script.ts:53-63); mitigations: unconditional teardown-on-arm,
  serde-side validation and clamping (the `parse_page_chrome_payload` pattern,
  `page_chrome.rs:36-47`), and a Rust-side timeout. Cross-origin iframes are
  out of reach — `elementFromPoint` in the main frame resolves to the
  `<iframe>` element itself *(inference; orca has the same limit and does not
  special-case it)*.

### Option (b): Rust-side interception

- **Mechanics.** `browser_view.rs::handle_mouse_down` checks an armed flag
  and, instead of `tab.send_mouse_down(...)`, runs an on-demand JS query at
  the click position. Coordinates line up: input positions are "logical
  pixels relative to the tab's content origin" (`tab_backend.rs:162-164`,
  computed at `browser_view.rs:1414`), which equal CSS pixels at default page
  zoom *(inference)*.
- **Fatal flaw as a complete option.** There is no hover feedback. Since
  `execute_java_script` has no return value, streaming hover would mean an
  injected script per mousemove plus a process message per result — an IPC
  round-trip per pointer event against a 60 fps OSR pipeline. The moment you
  add an in-page overlay to fix hover, you have rebuilt option (a) and (b)
  stops being a distinct architecture. Also, with the mouse-down swallowed
  Rust-side, an in-page hit-catcher never sees the click anyway — the two
  click paths are mutually exclusive, so the choice is per-leg, not per-option.
- **What survives into the recommendation.** The click gate itself: it is
  ~5 lines in an existing function, it survives navigation trivially (it is
  Rust state), and it upgrades orca's "vast majority of sites" click
  suppression to a guarantee. Combined with a `finalize`-at-coordinates script
  (orca has exactly this shape: `FINALIZE_SCRIPT`,
  grab-guest-script.ts:925-940), it replaces the overlay's
  `click`/`contextmenu` capture handlers (grab-guest-script.ts:847-920)
  entirely.

### Option (c): CDP

- **Correction to the ticket's framing.** CDP does **not** require the
  debug-only TCP port. CEF has an in-process CDP channel on the browser host,
  and cef-rs 150 exposes all of it:
  `ImplBrowserHost::send_dev_tools_message` / `execute_dev_tools_method` /
  `add_dev_tools_message_observer`
  (`~/.cargo/git/checkouts/cef-rs-1a4e65a2c484707b/c73f792/cef/src/bindings/x86_64_unknown_linux_gnu.rs:12668-12677`),
  plus an implementable `DevToolsMessageObserver` with a
  `wrap_dev_tools_message_observer!` macro (same file, `:3755`, `:3814`,
  callbacks `on_dev_tools_message` / `on_dev_tools_method_result` /
  `on_dev_tools_event`, `:3776-3801`). The CEF header states: "Usage of the
  SendDevToolsMessage, ExecuteDevToolsMethod and AddDevToolsMessageObserver
  functions does not require an active DevTools front-end or remote-debugging
  session"
  (`…/c73f792/sys/src/bindings/x86_64_unknown_linux_gnu.rs:8582-8583`; method
  variant `:8591`, observer `:8600`).
- **What it would look like.** `Overlay.enable` + `DOM.enable` +
  `Overlay.setInspectMode("searchForNode", …)` to arm with native
  DevTools-style hover highlighting; the `Overlay.inspectNodeRequested` event
  delivers a `backendNodeId` on click; then `DOM.describeNode`,
  `DOM.getOuterHTML`, `CSS.getComputedStyleForNode`, `DOM.getBoxModel` to
  assemble the payload *(protocol knowledge; not exercised against this
  build)*.
- **Why not as the primary channel.**
  1. **Payload assembly is the hard part, and CDP makes it harder.** The
     valuable fields orca extracts — readable selector paths, React component
     names and `_debugSource` file:line (grab-guest-script.ts:651-682),
     redaction-aware attribute filtering, nearby-text context — have no CDP
     domain; you would end up calling `Runtime.evaluate` with injected JS,
     i.e. option (a) over a clunkier transport, with N async round-trips per
     click instead of one message.
  2. **New machinery with no in-crate precedent**: message-id correlation,
     per-browser observer registration/`Registration` lifetime, JSON protocol
     structs — versus the third use of an existing, tested bridge pattern.
  3. **Unverified rendering under OSR**: whether `Overlay.setInspectMode`'s
     highlight paints into `on_paint` frames in windowless mode is untested
     here (the existing DevTools window works, migration-plan.md:522-528, but
     that is a separate native window). Would need a spike before betting the
     feature's UX on it.
  4. **Session-state caveat** from the same header: "any modification of
     global browser state by one session may not be reflected in the UI of
     other sessions" (sys bindings `:8582`) — shared-state interference with
     an open DevTools window is plausible.
- **Where CDP *is* right.** Element screenshots (orca deliberately captures
  screenshots in the main process, not in-page —
  `browser-grab-screenshot.ts`; our equivalent is `Page.captureScreenshot`
  with a clip rect, or cropping the existing software OSR frame in
  `frame_presenter.rs`, which needs no CDP at all *(inference)*), and
  potentially cross-origin-iframe picking later.

## The activation channel (required decision #1)

Three candidates for browser→renderer "arm/disarm", with verdicts:

1. **`send_process_message(ProcessId::RENDERER, …)` — real, verified, not
   chosen.** CEF process messages are direction-agnostic: the header on
   `_cef_frame_t::send_process_message` says "Send a message to the specified
   |target_process|. … Message delivery is not guaranteed in all cases (for
   example, if the browser is closing, navigating, or if the target process
   crashes)" (`…/c73f792/sys/src/bindings/x86_64_unknown_linux_gnu.rs:6863-6864`).
   The renderer-side receiver exists: `_cef_render_process_handler_t::
   on_process_message_received` — "Called when a new message is received from
   a different process" (sys `:13858-13859`), surfaced by cef-rs as
   `ImplRenderProcessHandler::on_process_message_received`
   (cef `x86_64_unknown_linux_gnu.rs:32608`) inside the same
   `wrap_render_process_handler!` macro `page_chrome.rs` already uses, and
   `ProcessId::RENDERER` is exported (cef `:47502`). **Why not:** the handler
   would receive the message and then still have to locate the frame's V8
   context and eval the arm script — everything `execute_java_script` already
   does internally, plus hand-written routing in the one shared
   render-process handler. Same semantics, strictly more code. Keep this
   channel in reserve for a future need that genuinely requires render-process
   state (e.g. `visit_dom`, which "can only be called from the render
   process", sys `:6852`).

2. **`frame.execute_java_script` — chosen.** Verified on the safe `Frame`
   wrapper (cef `x86_64_unknown_linux_gnu.rs:7593-7598`; header doc sys
   `:6811-6812`: "Execute a string of JavaScript code in this frame"). Called
   from the browser process on the UI thread — the same thread discipline as
   every `with_focused_frame` edit command in `cef_tab.rs:170-176`, `:441-467`.
   One call, no new handler surface, delivery to the frame's main world
   handled by CEF. The scripts are self-arming/self-tearing (orca's
   `ARM_SCRIPT`/`TEARDOWN_SCRIPT` idempotency shape,
   grab-guest-script.ts:49-63, :964-976), so a dropped injection during
   navigation degrades to "picker not armed", which the Rust-side timeout
   already covers.

3. **A flag consulted at `on_context_created` — rejected.** The render
   process is a separate OS process (`cef_instance.rs:221-223`); a
   browser-process flag is simply not visible there. Making it visible needs
   an IPC (back to #1) or process-launch state (`extra_info`/command line),
   which cannot express per-tab, mid-session arm/disarm. What *does* belong at
   `on_context_created` is the **payload bridge**: installing
   `__zedReportElementPick` unconditionally on every main-frame context
   (exactly like `__zedReportChromeColor`, `page_chrome.rs:636-659`) is cheap,
   auto-heals across navigation, and is the only way page JS can reach
   `send_process_message` at all.

## Recommended round-trip lifecycle (required decision #2)

```
enter design mode
  └─ BrowserView.set picker_armed = true (state above the seam)
  └─ tab.set_element_picker_armed(true)
        └─ main_frame.execute_java_script(ARM_SCRIPT)        [overlay + hover installs]
hover
  └─ forwarded mouse-moves as today; highlight tracked entirely in-page
        (host overlay with pointer-events:all also suppresses page :hover)
click (left or right) while armed
  └─ handle_mouse_down: do NOT forward to engine
  └─ tab.finalize_element_pick(position)
        └─ main_frame.execute_java_script(FINALIZE_SCRIPT(x, y))
              └─ elementFromPoint(x,y) → extractPayload(el)
              └─ window.__zedReportElementPick(json)          [bridge]
                    └─ frame.send_process_message(BROWSER, "zed.element_pick")
  └─ client.rs on_process_message_received → ElementPicked event
  └─ drain_engine_events → BrowserView resolves the pick, disarms (or re-arms)
Esc while armed
  └─ handled Rust-side (keys already routed) → disarm
disarm / exit design mode
  └─ tab.set_element_picker_armed(false)
        └─ main_frame.execute_java_script(TEARDOWN_SCRIPT)
main-frame navigation while armed  (AddressChanged / LoadingStateChanged)
  └─ BrowserView cancels the pick (orca's policy, controller.ts:171-182);
     old overlay died with the context, bridge reinstalls via on_context_created
tab closed / engine gone
  └─ armed state dropped with BrowserView tab state; nothing to clean up in-page
no payload within timeout (delivery not guaranteed while navigating, sys:6863)
  └─ BrowserView timeout cancels the pick (orca: 120 s, controller.ts:22)
```

**Incognito**: unaffected by the choice — the render-process handler and its
bridge are app-global across request contexts; the seam commands are per-tab
(`cef_tab.rs:94-113` creates incognito tabs through the same `CefTab`).
**Popups**: native popup windows share the *opener's* event sender
(`client.rs:206-218` `build_for_popup`), so a popup page calling the bridge
would inject `ElementPicked` into the opener tab's stream. `BrowserView` must
drop `ElementPicked` events whenever that tab is not armed (also the defense
against a hostile page calling the bridge spontaneously). Popups take no other
part in design mode — they have no `browser_view` input path to gate.

## Recommended `TabBackend` seam extension (required decision #3)

Minimal set, matching existing naming (`set_focus`/`set_hidden` for state
pushes, `tab_backend.rs:193-198`; `find`/`stop_finding` for operations with
event-carried results, `:242-247`; `PageChromeChanged(Option<…>)` for
payload-or-nothing events, `:137-139`):

```rust
// tab_backend.rs — commands (Rust → engine)
/// Install or tear down the in-page element-picker overlay in the main
/// frame. Armed state lives above the seam; the backend is stateless.
fn set_element_picker_armed(&mut self, armed: bool);

/// Extract the element at `position` (logical px, content-origin-relative,
/// same space as send_mouse_down) and report it as an ElementPicked event.
/// Only meaningful while the picker is armed.
fn finalize_element_pick(&mut self, position: Point<Pixels>);

// tab_backend.rs — event (engine → Rust)
/// The armed picker resolved a click: the extracted element payload, or
/// `None` when extraction failed or nothing was at the point.
ElementPicked(Option<ElementPickPayload>),
```

- `ElementPickPayload` lives in a new `element_picker.rs` alongside its arm/
  finalize/teardown scripts and serde parsing — the `page_chrome.rs` file
  shape (payload type + parser + `#[cfg(feature = "cef")] mod engine`).
  Payload fields: start from orca's `BrowserGrabPayload`
  (`browser-grab-types.ts:87-93` — page context, target with
  selector/paths/attributes/accessibility/rects/computed styles/React
  metadata, nearby text, ancestor path) minus the screenshot (deferred;
  crop the OSR frame or use CDP later). Enforce orca's budgets and redaction
  in the script *and* re-clamp in serde (orca does both:
  grab-guest-script.ts:66-102, controller.ts:151).
- Process-message name `zed.element_pick`, bridge name
  `__zedReportElementPick` (following `zed.page_chrome` /
  `__zedReportChromeColor`, `page_chrome.rs:253-254`, and
  `zed.text_input_state`, `text_input.rs:155`).
- No arm/disarm/cancel events: cancellation (Esc, navigation, timeout, tab
  close) is decided where the state lives, in `BrowserView`, reusing existing
  events. Orca's `cancelled`/`error` result kinds collapse into Rust-side
  state transitions plus the `None` payload.
- `stub_tab_backend.rs` implements both commands as recorded no-ops and can
  script `ElementPicked` events, giving deterministic tests through
  `simulate_message_pump` like every other event (`browser_view.rs:718-720`).
- Right-click-for-menu (orca's `context-selected`,
  grab-guest-script.ts:886-906): if wanted, it is a Rust-side distinction —
  `handle_mouse_down` knows the button before calling
  `finalize_element_pick` — so it needs **no** extra seam surface beyond
  `BrowserView` remembering which button triggered the finalize. Do not add a
  payload flag for it.

## Risks and open questions

1. **Main-world tampering** (shared with orca, accepted): a hostile page can
   delete `__zedReportElementPick`, remove the overlay host, or send forged
   payloads. Mitigations: unconditional teardown-on-arm
   (grab-guest-script.ts:53-63), drop `ElementPicked` when not armed, serde
   validation + budget clamps, pick timeout. Forged payloads while armed
   remain possible; the payload is display/context data, not a capability, so
   the blast radius is prompt-content pollution *(inference)*.
2. **`finalize` vs hover-state race**: finalizing by click coordinates
   (`elementFromPoint(x, y)`) rather than "current hovered element" makes the
   result exact even if the last forwarded mousemove lagged. The overlay must
   hide its host before hit-testing, as the hover path already does
   (grab-guest-script.ts:804-812).
3. **Page zoom / CSS-pixel mismatch**: the logical-px == CSS-px assumption
   breaks if per-tab zoom ships later; the finalize script can divide by the
   page's effective zoom at that point. Flagged, not blocking.
4. **Cross-origin iframes**: picker resolves the `<iframe>` element, not its
   contents (same as orca). If this matters for design mode, the CDP channel
   (now known to be available in-process) is the escalation path.
5. **Message delivery during navigation is best-effort** (sys `:6863-6864`):
   the arm script or the payload can be silently dropped if a navigation
   races the click. Covered by the navigation-cancel policy + timeout; do not
   build an ACK protocol for v1.
6. **Unverified**: CDP `Overlay.setInspectMode` highlight rendering under
   windowless OSR (only relevant if option (c) is ever revisited); exact
   `execute_java_script` behavior when called before the first load completes
   (irrelevant to this design — arming is a user action on a loaded page).

## Sources

- Zed glass: `crates/browser/src/{page_chrome,client,browser_view,tab_backend,cef_tab,cef_instance,text_input}.rs`; `docs/adr/0003-fork-divergence-management.md`; `docs/glass/migration-plan.md`.
- orca: `src/main/browser/{grab-guest-script,browser-grab-session-controller,browser-grab-payload,browser-grab-screenshot}.ts`; `src/shared/browser-grab-types.ts`.
- cef-rs (git checkout `c73f792`, tag `cef-v150.0.0+150.0.10`):
  `cef/src/bindings/x86_64_unknown_linux_gnu.rs` (safe API),
  `sys/src/bindings/x86_64_unknown_linux_gnu.rs` (CEF header docs), at
  `~/.cargo/git/checkouts/cef-rs-1a4e65a2c484707b/c73f792/`.
