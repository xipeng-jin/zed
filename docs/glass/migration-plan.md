# Glass → Zed Fork Migration Plan

**Status:** Investigation complete, decisions confirmed, implementation not started.
**Target:** the `glass` branch of this repository (xipeng-jin/zed fork of zed-industries/zed).
**Date of investigation:** 2026-07-10, performed on Linux.

This document is the execution contract for rebuilding Glass's integrated browser on
top of current upstream Zed. It is written so an implementation agent can execute each
milestone without repeating the repository investigation. All decisions herein were
confirmed interactively with the project owner; the four load-bearing ones are recorded
as ADRs in `docs/adr/` (0001–0004).

Citation conventions:

- `Glass:<path>:<line>` → `~/Projects/refs/Glass` (Glass-HQ/Glass, `main` @ `fbc6b9aeaf`)
- `gpui-fork:<path>` → `~/Projects/refs/gpui` (Glass-HQ/gpui). Unless noted, paths refer
  to rev `3790fca` (the rev Glass pins), **not** the fork's HEAD — the fork's HEAD
  (`650aab5`) has already deleted the native-controls system Glass uses.
- `zed:<path>:<line>` → this repository, branch `glass`.

Findings below are **verified** against the checkouts unless explicitly marked
*(inferred)* or *(unverifiable on Linux)*.

---

## 1. Confirmed decisions (summary)

| # | Decision | Detail | Record |
|---|---|---|---|
| 1 | Engine | CEF via `cef-rs` (tauri-apps), off-screen rendering, same as Glass | ADR-0001 |
| 2 | Platform order | Linux first, macOS second, Windows deferred (never designed out) | §8 |
| 3 | Rendering | Software OSR first, behind a `FramePresenter` seam inside the browser crate; zero GPUI changes; accelerated dmabuf import only if measurement demands it | ADR-0002 |
| 4 | Divergence management | Additive crates only; enumerated touch-list of upstream-file edits (§6.3); periodic `git merge` of upstream into `glass`, never rebase; fork `main` stays clean upstream mirror | ADR-0003 |
| 5 | Workspace model | `BrowserView` as a pane `Item` with an internal browser tab strip; Glass's "modes" system deferred; "browser as default view on launch" dropped | ADR-0004 |
| 6 | Scope cut | Exclude `service_hub*`, `app_runtime*`, native NSToolbar/native sidebar and Glass's `workspace.rs`/`dock.rs` rewrites, `toast` crate, sidebar `threads_navigator`, `terminal_session_manager`, Glass themes/branding/app-identity. Default-browser OS registration deferred to M3 | §5 |
| 7 | Milestones | M1 engine-on-Linux → M2 daily-drivable UX → M3 macOS + extras | §7 |
| 8 | CEF supply | Stock Spotify-CDN binaries, sha-verified, at Glass's exact pin (CEF 145.0.28 / cef-rs tag `cef-v145.6.1+145.0.28`); H.264/AAC gap accepted; source-built codecs out of scope | §6.4 |
| 9 | Command surface | Context-scoped browser keymap that shadows Zed defaults when a BrowserView is focused (terminal precedent); minimal `browser` settings section | §7 M2 |
| 10 | GPUI policy | Port nothing from the gpui fork up front. One contingency patch (`d14fb11`, focused-input-context key dispatch) adopted only if stock dispatch proves broken during M2 IME work | §4 |

---

## 2. Repository state (verified 2026-07-10)

### 2.1 This repository (`~/Projects/zed`)

- Branch `glass` = `main` + one commit (`26f6897c46` "Add agent skills configuration");
  merge-base with `main` is `10504e3ce1`. Working tree clean. Effectively **current
  upstream Zed** — this is a rebuild, not a rebase of Glass.
- No crate named `browser`, `webview`, or `cef` exists (`ls crates/`) — the crate name
  `browser` is free.
- `workspace::register_serializable_item` exists (`zed:crates/workspace/src/workspace.rs:1086`)
  and is used by `terminal_view` (`zed:crates/terminal_view/src/terminal_view.rs:110`) —
  the stock mechanism for non-file items to serialize/restore.
- `mimalloc` is already an optional dependency upstream (`zed:crates/zed/Cargo.toml:159`).

### 2.2 Glass (`~/Projects/refs/Glass`)

- Glass-HQ/Glass, `main` @ `fbc6b9aeaf`. Fork of Zed maintained by one primary developer
  (`naaiyy`, ~400 commits). Tracks upstream via increasingly painful merge commits
  ("Merge upstream through 2d3f49e4b2", etc.).
- **Does not contain gpui.** All gpui-family crates (`gpui`, `gpui_macros`, `collections`,
  `util`, `sum_tree`, `media`, `http_client*`, `refineable`, `scheduler`, …) are git
  dependencies on Glass-HQ/gpui pinned at rev `3790fca` (`Glass:Cargo.toml:286-454`).
- Glass-original crates: `browser` (~11.9k LOC), `service_hub`/`service_hub_ui` (~10k),
  `app_runtime`/`app_runtime_ui` (~3k), `workspace_chrome` (442), `workspace_modes` (215),
  `toast` (503), plus `title_bar/native_toolbar/` (~1k) and heavy in-place edits to
  `workspace.rs`/`dock.rs`. Crates like `csv_preview`, `fuzzy_nucleo`, `git_graph`,
  `denoise`, `keymap_editor` are upstream-authored, **not** Glass work.

### 2.3 gpui fork (`~/Projects/refs/gpui`)

- Glass-HQ/gpui, standalone multi-crate workspace. Corresponds to upstream gpui of
  ~April 2026 (sync commits cite upstream zed commits through `9ef1afd6d5`; `gpui`
  version 0.2.2 matches upstream).
- The platform-crate split (`gpui_macos`/`gpui_linux`/…), the `scheduler` crate, and
  `gpui_shared_string` extraction all **exist in current upstream too** — they are
  upstream refactors the fork mirrored, not fork inventions.
- Fork HEAD (`650aab5`) has **deleted** the native-controls system ("phase-2 hosted
  architecture scaffold", commit `97eb0ad`) and pivoted to iOS work. Glass pins the
  older `3790fca` to keep the native controls. iOS support is entirely isolated in
  `gpui_ios`/`gpui_apple` platform crates and is **out of scope** (nothing in the core
  `gpui` crate references `target_os = "ios"`).

---

## 3. Browser architecture reference (the port-source map)

Glass's browser is `Glass:crates/browser` — 36 files, ~11,850 LOC, lib root
`src/browser.rs`, plus a `[[bin]] glass_helper`. It is **macOS-only in practice**:
zero `target_os = "linux"` references exist anywhere in the crate; every load-bearing
platform piece (framework loading, frame presentation, Obj-C patching, packaging) is
`#[cfg(target_os = "macos")]`. Windows has scaffolding only (runtime-staging scripts,
a keyboard-event type). There are **no TODO/FIXME markers**; tests are pure-logic unit
tests only (dispatch routing, URL parsing — `text_input.rs` 9, `input.rs` 4,
`page_chrome.rs` 4, `omnibox.rs` 3, `browser_view.rs` 3, `events.rs` 2).

### 3.1 CEF lifecycle

Two-phase init in `Glass:crates/browser/src/cef_instance.rs`:

1. **Subprocess phase** — `CefInstance::handle_subprocess()` (`cef_instance.rs:251`)
   runs as the *very first thing* in `main()`
   (`Glass:crates/zed/src/main.rs:183-187`, cfg'd macOS+Windows). Loads the CEF
   library (bundle-relative framework, else `CEF_PATH` env, else `~/.local/share/cef`;
   resolver at `cef_instance.rs:197-227`), calls `cef::execute_process`; if the return
   is `>= 0` this process *was* a CEF subprocess and exits immediately.
2. **Browser-process phase** — `CefInstance::initialize(cx)` (`cef_instance.rs:316`).
   Key settings (`cef_instance.rs:353-356,423-434`): `windowless_rendering_enabled=1`,
   **`external_message_pump=1`**, `no_sandbox=1`, cache at
   `paths::data_dir()/browser_cache`, `persist_session_cookies=1`. User agent spoofed
   to plain `Chrome/145.0.7632.75` because Google sign-in rejects the CEF UA
   (`cef_instance.rs:363-367`). Command-line switches applied in
   `on_before_command_line_processing` (`cef_instance.rs:112-172`): `no-startup-window`,
   `noerrdialogs`, `hide-crash-restore-bubble`, `disable-gpu-sandbox`,
   `autoplay-policy=no-user-gesture-required`, `ignore-gpu-blocklist`,
   `enable-gpu-rasterization`, `enable-zero-copy`, accelerated video decode,
   `enable-features=PlatformHEVCDecoderSupport,...`; macOS-only `use-angle=metal`.
   Debug: `GLASS_CEF_DEBUG` env → stderr logging + `remote-debugging-port=9222`.

**Message pump — platform-neutral, ports as-is.** CEF's
`on_schedule_message_pump_work(delay_ms)` folds the next-due time into an atomic
(`cef_instance.rs:74-77`). The driver is a plain GPUI foreground task:
`BrowserView::start_message_pump` (`Glass:crates/browser/src/browser_view/tabs.rs:362-393`)
loops `pump_messages()` (→ `cef::do_message_loop_work()`) when due, drains per-tab
event queues, then awaits `cx.background_executor().timer(...)` clamped to 500µs–1ms;
a ~30 Hz idle fallback is set after each pump. No platform run-loop hooks, no
scheduler-crate modifications — verified that all APIs used (`cx.spawn`,
`background_executor().timer()`) exist unchanged in current upstream.

**Shutdown** — `CefInstance::shutdown()` (`cef_instance.rs:492`): force-close all
browsers, pump 10×, `cef::shutdown()`. Registered via `cx.on_app_quit` in
`browser::init` (`Glass:crates/browser/src/browser.rs:193-195`).

**Helper binary** — `glass_helper` (`Glass:crates/browser/src/bin/glass_helper.rs`) is
the CEF subprocess executable *for macOS packaging only* (5 helper `.app` bundles: GPU,
Renderer, Plugin, Alerts). On non-macOS it exits 1 by design. On Linux, CEF subprocesses
are the main binary re-invoked with `--type=...` argv, which the early-`main()` guard
already handles — **no helper binary is needed on Linux**.

**macOS-only re-entrancy patch** — `Glass:crates/browser/src/macos_protocol.rs`
patches the Obj-C runtime so GPUI's `NSApplication` subclass conforms to CEF's
`CrAppProtocol`/`CefAppProtocol` (`isHandlingSendEvent` etc.). Required on macOS,
irrelevant elsewhere. *(unverifiable on Linux)*

### 3.2 Rendering

- Browser creation forces GPU shared-texture OSR: `WindowInfo { windowless_rendering_enabled: 1,
  shared_texture_enabled: 1 }`, `BrowserSettings { windowless_frame_rate: 60 }`
  (`Glass:crates/browser/src/tab.rs:285-294`).
- `on_accelerated_paint` (`Glass:crates/browser/src/render_handler.rs:119-164`), macOS
  only: wraps the CEF `shared_texture_io_surface` as a `CVPixelBuffer`, stores it,
  emits `BrowserEvent::FrameReady`. Presentation:
  `surface(frame).size_full().object_fit(ObjectFit::Fill)`
  (`Glass:crates/browser/src/browser_view/content.rs:503-506`).
- **On non-macOS every frame is dropped** (`render_handler.rs:160-163`) and
  `has_frame` is hard-coded false (`content.rs:368`); the software `on_paint` path is
  a dead warning stub (`render_handler.rs:166-177`). This is the single largest gap
  the rebuild must fill (M1).
- GPUI's `surface()` element in the fork is **byte-identical to upstream's**
  (both macOS/CVPixelBuffer-only) — the macOS path needs no gpui port.

### 3.3 Input

- **Three-way dispatch** (`Glass:crates/browser/src/text_input.rs:61-139`): each
  keystroke classifies as `App` (cmd/ctrl-modified → Zed actions win), `TextInput`
  (editable-field printable/IME → routed through the input handler), or `Browser`
  (raw CEF key event). Editability of the focused DOM node is reported by the render
  process via a `glass.text_input_state` process message
  (`text_input.rs:40-59`, received at `client.rs:170-177`).
- **Key events**: built as CEF `KeyEvent`s with Windows virtual-key codes.
  `keycodes.rs` maps *macOS hardware keycodes* → VK (`macos_keycode_to_windows_vk`)
  and key *names* → VK (`key_name_to_windows_vk`, portable). The macOS path also uses
  a fork-only `Keystroke.native_key_code` field that upstream gpui does not have —
  the Linux port must map from gpui key names / scancodes instead (M1 work item).
- **Mouse/scroll** (`Glass:crates/browser/src/input.rs:22-72`): coordinates offset by
  content bounds; `ScrollDelta::Lines` converted at 40px/line. `platform` modifier
  maps to Command on macOS, Ctrl elsewhere (`input.rs:284-293`).
- **IME**: `BrowserView` implements `EntityInputHandler`
  (`Glass:crates/browser/src/browser_view.rs:1106-1218`), installed via
  `window.handle_input(&focus_handle, ElementInputHandler::new(bounds, view), cx)`
  inside a `canvas` element (`content.rs:374-388`); composition forwarded to CEF via
  `ime_set_composition`/`ime_finish_composing_text` (`tab.rs:587-622`).
- **Native key suppression**: `OsrKeyboardHandler::on_pre_key_event` suppresses all
  OS-delivered key events unless the app explicitly sent them
  (`client.rs:36-63`) — only Glass-routed events reach the page. Popup windows use a
  permissive handler instead (`client.rs:74-98`).
- **Swipe navigation** (`browser_view/swipe.rs`): two-finger horizontal swipe →
  back/forward, driven by `ScrollWheelEvent.touch_phase`. Trackpad-dependent; M3.

### 3.4 Workspace integration (Glass's; ours differs per ADR-0004)

Glass integrates twice: as a full-window "mode" (`ModeId::BROWSER` registered with
`workspace_modes::ModeViewRegistry`, `Glass:crates/browser/src/browser.rs:255-302`;
browser is the default mode on first launch) *and* as a pane item
(`BrowserPaneItem`/`Item` impl, `Glass:crates/browser/src/browser_view.rs:1220-1332`,
tab icon Globe). One `BrowserView` per workspace holds an **internal browser tab
strip** (pinned tabs, favicons, sidebar list) — web pages are *not* individual Zed
pane tabs. Actions are defined in `actions!(browser, [...])`
(`browser_view.rs:45-76`): NewTab, CloseTab, ReopenClosedTab, Next/PreviousTab,
FocusOmnibox, Reload, GoBack/GoForward, OpenDevTools, Pin/UnpinTab, OpenBrowserPane,
BookmarkCurrentPage, CopyUrl, FindInPage, ToggleDownloadCenter, etc.
**Glass ships no browser keybindings at all** (verified: no browser/mode entries in
any `assets/keymaps/*.json`) — the command surface is green-field for us.

We keep: the `BrowserView`-with-internal-tabs model, the actions vocabulary, the
per-workspace-singleton pattern. We drop: the mode registry, mode default, native
toolbar/titlebar coupling (`ModeViewRegistry::set_titlebar_center_view` etc.).

### 3.5 Persistence

Everything is JSON blobs in Zed's existing KV store (`db::kvp`), no SQL schema.
Keys (`Glass:crates/browser/src/session.rs:7-11`): `browser_tabs`,
`browser_pinned_tabs`, `browser_history`, `browser_bookmarks`, `browser_downloads`.

- `SerializedTab { url, title, is_new_tab_page, is_pinned, favicon_url }`;
  restore recreates tabs lazily (`Glass:crates/browser/src/browser_view/session.rs:10-48`).
- History: in-memory, cap 2000 with LRU eviction, fuzzy search with
  recency/frequency/prefix bonuses (`Glass:crates/browser/src/history.rs:66-179`).
- Saves debounced 500ms; a single designated tab-owner view writes the global keys;
  synchronous flush on quit (`browser_view/session.rs:159-248`). Incognito windows
  skip both restore and save.
- Web session state (cookies, logins) persists via CEF's own on-disk profile
  (`browser_cache` dir + `persist_session_cookies=1`), not via Zed.

### 3.6 CEF handlers (all platform-neutral, port nearly as-is)

All wired in `Glass:crates/browser/src/client.rs:103-235`:

| Handler | Behavior | Notes |
|---|---|---|
| `download_handler.rs` | Auto-saves to `~/Downloads` (fallback `data_dir/browser_downloads`), de-dupes filenames `name (n).ext`, streams progress events | No save dialog; M2 settings adds dir override |
| `permission_handler.rs` | Grants all media-access requests; auto-accepts Widevine DRM prompts; other prompts → default deny | Deliberate for streaming sites |
| `context_menu_handler.rs` | Cancels CEF's native menu, extracts link/selection/edit context, emits event → GPUI renders the menu (`anchored`/`deferred`) | Must NOT override `on_before_context_menu` (CEF needs ≥1 model item to call `run_context_menu`) — documented trap |
| `life_span_handler.rs` | Popups: tab-like dispositions redirect into the app's tab flow; `NEW_POPUP` (OAuth/login windows) allowed as a **real native CEF window** with `windowless_rendering_enabled=0` | Native popup windows on Linux are X11 — see risk R3 |
| `find_handler.rs` | Match count/ordinal events for the find overlay | Trivial |
| `display_handler.rs` | Address/title/loading/favicon events; suppresses native fullscreen; contains a `CefStringList` clone-bug workaround (`std::mem::take`, `display_handler.rs:82-96`) | Keep the workaround |
| `request_handler.rs` | Mirrors popup logic for link-opens targeting tabs | Trivial |
| `page_chrome.rs` (render-process handler) | Injects a JS bridge that samples page top-edge color / `<meta theme-color>` for tab tinting, and reports focused-node editability | Editability signal is M2-critical (input routing); tinting is M3 |

### 3.7 Build/packaging (Glass's; ours per §6.4)

- CI downloads **stock CEF 145.0.28 from `https://cef-builds.spotifycdn.com`**,
  sha1-verified, cached at `~/.local/share/cef`, located via `CEF_PATH`
  (`Glass:.github/workflows/release_glass.yml:136-187`). **Verified live 2026-07-10**:
  the CDN serves CEF `145.0.28` as respin `145.0.28+g51162e8+chromium-145.0.7632.160`
  with `minimal` archives for `linux64`, `linuxarm64`, and `macosarm64` (sha1s in the
  CDN `index.json`). Same CEF API version as the `cef-rs` tag pin.
- `Glass:script/build-cef` builds CEF from source (~4–8h) solely for proprietary
  codecs (H.264/AAC). **Out of scope**; documented option only.
- macOS bundling (`Glass:script/bundle-mac-cef`): framework copy + 5 helper app
  bundles + ad-hoc signing; entitlements add `allow-jit`,
  `allow-unsigned-executable-memory`, `disable-library-validation` (CEF requirements).
  All M3, *(unverifiable on Linux)*.
- `mimalloc` enabled in release to fix a CEF allocator crash
  (Glass commit `70d08691aa`) — upstream already has the optional dep
  (`zed:crates/zed/Cargo.toml:159`); validate necessity during M3 macOS work.

---

## 4. GPUI policy (constraint: minimal divergence)

Verified conclusion: **no gpui changes are required to start, and possibly none ever.**

- The fork's `surface()` element, `scheduler` crate, platform-crate split, and
  `gpui_shared_string` extraction are already present in current upstream (or
  upstream is ahead).
- The entire `native_*` control suite (24 elements: `native_image_view`,
  `native_icon_button`, `native_menu_button`/`show_native_popup_menu`,
  `native_tracking_view`, `native_sidebar`, …) is AppKit-hosted and **early-returns
  into a no-op wherever there is no NSView** (`gpui-fork:crates/gpui/src/elements/native_image_view.rs:369-371`;
  `raw_native_view_ptr()` returns null on Linux/Windows/Web). The fork's own HEAD has
  deleted them. **Disposition: reject; replace every call site with stock `ui`-crate
  components** (`IconButton`, `img()`, `svg()`, `ContextMenu`, `on_hover`). Glass's
  browser uses them at ~30 call sites (omnibox, tab strip, bookmarks, toolbar,
  content placeholder); Glass's own non-macOS fallback chrome
  (`browser_view.rs:1495-1516` renders a regular GPUI tab strip/toolbar) is the
  closer starting point for our cross-platform chrome.
- **One contingency patch**: fork commit `d14fb11` "Preserve focused input contexts
  in key dispatch" — adds `Frame.focused_input_contexts` + `*_with_context_stack`
  dispatch (~220 platform-neutral LOC in `key_dispatch.rs`/`window.rs`) so keystrokes
  resolve against the context captured when the focused element painted. Verified
  **absent from current upstream** (`grep focused_input_contexts zed:crates/gpui/src/` → none).
  The browser consumes it implicitly through `set_input_handler`. **Policy: build M1/M2
  against stock dispatch; adopt (and ideally upstream) this patch only if key/IME
  routing over the CEF content element demonstrably misroutes.** If adopted, it goes
  on the touch-list with a dedicated evaluation note.
- The fork-only `Keystroke.native_key_code` field is *not* ported; the Linux keycode
  mapping (M1) works from gpui key names/scancodes instead.

---

## 5. Feature inventory and dispositions

Classification legend: **migrate** (port, adapting to current APIs) · **reimplement**
(preserve intent, new code) · **defer** (explicitly later, not scheduled) ·
**exclude** (will not do).

| Feature | Where in Glass | Disposition | Class per brief | Notes |
|---|---|---|---|---|
| CEF lifecycle, pump, shutdown | `browser/src/cef_instance.rs`, `tabs.rs` | **Migrate** (M1) | Platform-independent | Add Linux library-loading path |
| Software frame presentation | *(does not exist)* | **Reimplement** (M1) | New work | `on_paint` → `RenderImage`; §7 M1 |
| macOS zero-copy presentation | `render_handler.rs`, `content.rs` | **Migrate** (M3) | macOS-specific, required for browser on macOS | Behind `FramePresenter` seam |
| Input routing/dispatch classification | `text_input.rs`, `input.rs`, `browser_view/input.rs` | **Migrate** (M1 basic, M2 IME) | Platform-independent concept; keycode tables need Linux reimplementation | |
| CEF handlers (§3.6) | `client.rs` + 8 handler files | **Migrate** (M1/M2) | Platform-independent | Near verbatim |
| `macos_protocol.rs` Obj-C patch | `browser/src/macos_protocol.rs` | **Migrate** (M3) | macOS-specific, required internally | cfg-gated |
| BrowserView + internal tab strip, omnibox, new-tab page, bookmarks, chrome | `browser_view*.rs`, `omnibox.rs`, `bookmarks.rs`, `new_tab_page.rs`, `toolbar.rs` | **Reimplement UI, migrate logic** (M1 minimal, M2 full) | Concept platform-independent; UI was macOS-native | Replace all `native_*` with `ui` components |
| Session/history/downloads persistence | `session.rs`, `history.rs`, `browser_view/session.rs` | **Migrate** (M2) | Platform-independent | KV JSON model kept |
| Popup/OAuth windows | `life_span_handler.rs` | **Migrate** (M2) | Platform-independent logic | Linux native-window behavior needs validation (R3) |
| Page-chrome tinting | `page_chrome.rs` (color part) | **Migrate** (M3) | Platform-independent | Editability part is M2 |
| Swipe navigation | `browser_view/swipe.rs` | **Migrate** (M3) | Trackpad/macOS in practice | |
| Default-browser OS registration | `zed/main.rs` `[default-browser]` + Info.plist URL schemes | **Reimplement** (M3) | Per-platform | Linux: `.desktop` `x-scheme-handler`; macOS unvalidatable here |
| Workspace modes (Browser/Editor/Terminal) | `workspace_modes`, `workspace_chrome`, `workspace.rs` hooks | **Defer** (post-M3 opt-in) | Concept portable; Glass impl entangled with NSToolbar | ADR-0004 |
| Browser-as-default-launch-view | `workspace_modes.rs:57` | **Exclude** | Product decision | |
| Native NSToolbar titlebar | `title_bar/native_toolbar/` (~1k LOC) | **Exclude** | macOS-specific, optional | Biggest merge-tax item |
| Native sidebar (NSSplitViewController) + `workspace.rs`/`dock.rs` rewrites | `workspace/src/*` | **Exclude** | macOS-specific, optional | |
| `toast` crate extraction | `crates/toast` | **Exclude** | Obsolete for us | Our browser crate depends on `workspace`; use upstream `StatusToast` |
| `app_runtime` / `app_runtime_ui` | own crates | **Exclude** (revisit on demand) | Mostly Apple-specific | Upstream tasks/debugger cover intent |
| `service_hub` / `service_hub_ui` | own crates (~10k) | **Exclude** | Apple release tooling | |
| Sidebar `threads_navigator`, `terminal_session_manager` | `sidebar/`, `workspace/` | **Exclude** | Belongs to deferred modes UX | |
| Collaboration/calling | restored-upstream code | **Nothing to do** | Already in upstream | |
| Glass themes, logos, bundle identity | `assets/themes/glass/`, resources | **Exclude** | Product decision: stay Zed-branded | |
| Terminal changes | *(none exist)* | **Nothing to do** | Glass uses stock terminal | |
| iOS anything | gpui fork `gpui_ios`/`gpui_apple`, `hosts/ios` | **Exclude** (hard constraint) | | No shared change identified as necessary |
| gpui `native_*` controls | gpui fork elements | **Exclude** | macOS/iOS-native | Replaced at ui level, §4 |
| gpui `d14fb11` dispatch patch | gpui fork | **Defer** (contingency, M2 gate) | Platform-neutral gpui extension | §4 |

---

## 6. Target architecture in this repository

### 6.1 New crates (additive; no upstream file owns any of this logic)

- **`crates/browser`** — everything: CEF instance/lifecycle, handlers, tabs,
  `BrowserView` + chrome, persistence, input, presenter seam. Mirrors Glass's crate
  name and internal file layout where the file survives the port (eases side-by-side
  reference), lib root `src/browser.rs` per repo convention. Internal seam:
  `FramePresenter` trait with `SoftwarePresenter` (M1, all platforms) and
  `IoSurfacePresenter` (M3, macOS). Depends on `workspace`, `ui`, `db`, `settings`,
  `menu`, `zed_actions` — all standard.
- No other new crates. (If the modes UX is ever revived, it returns as additive
  crates then.)

### 6.2 Helper binary

None on Linux (self-fork via early-`main()` guard). The macOS helper
(`glass_helper` equivalent, named for Zed) is added in M3 as a `[[bin]]` inside
`crates/browser`, exactly as Glass does.

### 6.3 The upstream-file touch-list (complete; keep it this small)

| File | Edit | Milestone |
|---|---|---|
| `Cargo.toml` (workspace) | Add `crates/browser` member + `browser` and `cef` workspace deps | M1 |
| `crates/zed/Cargo.toml` | Add `browser` dep | M1 |
| `crates/zed/src/main.rs` | (a) `browser::handle_cef_subprocess()` as first statement of `main()`, cfg'd for linux+macos+windows; (b) `browser::init(cx)` in the existing init block | M1 |
| `crates/zed/src/zed.rs` | Add `browser` to `test_action_namespaces`' expected list — the test enumerates every registered action namespace, so the browser actions (M1) break it without this one-line entry. *(Added retroactively during ticket #9, when the zed-crate suite was first run against the branch.)* | M1 |
| `assets/keymaps/default-linux.json`, `default-macos.json` | Context-scoped `BrowserView` bindings | M2 |
| `assets/settings/default.json` | `browser` settings section defaults | M2 |
| `crates/settings_content/src/settings_content.rs` (+ new `browser` content module) | Register the `browser` settings schema — upstream centralizes settings-content structs in this crate (see its `terminal` module) | M2 |
| `crates/zed/src/zed/app_menus.rs` | One "Open Browser" menu entry | M2 |
| *(contingent)* `crates/gpui/src/key_dispatch.rs`, `window.rs` | `d14fb11` port, only if M2 IME evaluation demands it | M2 |
| *(M3)* `script/bundle-mac*`, entitlements | CEF framework/helper bundling | M3 |

Everything else (including `script/download-cef`, `docs/glass/`, the browser crate)
is a new file. Any implementation step that wants to edit an upstream file not on
this list must first justify adding it here.

### 6.4 CEF supply chain

- New `script/download-cef`: platform-aware (linux64/linuxarm64/macosarm64/macosx64)
  download of the **minimal distribution** of CEF `145.0.28` from the Spotify CDN,
  sha1-verified, extracted to `~/.local/share/cef` (override via `CEF_PATH`) —
  conventions copied from `Glass:.github/workflows/release_glass.yml:136-187`.
- Cargo dep pinned to Glass's exact tag:
  `cef = { git = "https://github.com/tauri-apps/cef-rs", tag = "cef-v145.6.1+145.0.28", default-features = false }`.
  Rationale: every ported handler was written against this API surface; version bumps
  become deliberate follow-up tasks, never confounded with port bugs.
- Known accepted gap: stock builds lack H.264/AAC. Document in user-facing docs.
- Known risk: pinned Chromium goes stale (security). Revisit the pin immediately
  after M2 (see R6).

---

## 7. Milestones

### M1 — Engine on Linux

Gate: **browse a real site (navigate, scroll, click, type) inside a Zed pane on
Linux, split next to an editor and a terminal, quit cleanly.**

1. `script/download-cef` + docs; verify CEF 145.0.28 Linux minimal distribution
   downloads and matches sha1. Establish how `cef-dll-sys` locates CEF at build and
   runtime on Linux (build-time `CEF_PATH`; runtime library + `icudtl.dat`/`*.pak`
   resource paths next to the executable — expect a `stage-linux-cef-runtime`
   equivalent of Glass's Windows staging script).
2. Workspace plumbing per touch-list; empty `browser` crate compiling on all three
   platforms (non-Linux/macOS = stubs).
3. Port `cef_instance.rs`: subprocess phase (extend to Linux self-fork — main binary
   handles `--type=` argv), init phase (drop `use-angle=metal` on Linux; keep
   `external_message_pump=1`; **disable `shared_texture_enabled` on Linux** — request
   software OSR), shutdown, `GLASS_CEF_DEBUG`-equivalent debug env (rename `ZED_CEF_DEBUG`).
4. Port the message pump verbatim (`tabs.rs:362-393` pattern).
5. Implement `SoftwarePresenter`: `on_paint` → copy BGRA into an `Arc<RenderImage>`
   — **no pixel conversion**: `RenderImage` is natively BGRA
   (`zed:crates/gpui/src/assets.rs:42-49`) — swap on `FrameReady`, paint with
   `img()`. Watch for sprite-atlas
   churn from per-frame image ids; if it leaks/thrashes, reuse ids or throttle —
   measure before touching gpui.
6. Port `client.rs` + the trivially-neutral handlers needed to boot: load, display,
   render, life_span (minimal: allow default, no popup redirect yet).
   *Deviation (2026-07-11, ticket #5):* the minimal life-span handler **cancels**
   popups instead of allowing CEF's default. The default would create the popup
   with the opener tab's client, so its off-screen paints would corrupt the
   opener's render state. The full popup routing (tab redirect + native windows
   with a separate client) restores popup support in M2 step 6 (ticket #15).
7. Input, minimal: mouse click/move/wheel with bounds offset; key events via a new
   Linux mapping (gpui `Keystroke` key names → `key_name_to_windows_vk`, which is
   already portable; add scancode fallback only if names prove insufficient).
   `platform`→Ctrl mapping.
8. Minimal `BrowserView`: single tab, URL-entry omnibox (plain editor line),
   back/forward/reload buttons (stock `IconButton`), `Item` impl (Globe icon),
   `OpenBrowserPane`-style workspace action registered in `browser::init`.
9. Quit-path validation: CEF shutdown races GPUI teardown — verify no hang/crash on
   `cx.on_app_quit`.

### M2 — Daily-drivable browser UX

Gate: **usable as a daily browser on Linux; Gmail and GitHub login flows work
(including an OAuth popup); sessions restore across restart.**

1. Internal tab strip (start from Glass's non-macOS fallback chrome,
   `browser_view.rs:1495-1516`, re-skinned with `ui` components), pinned tabs,
   reopen-closed-tab, next/prev tab.
2. New-tab page; omnibox with history fuzzy suggestions (`history.rs` port);
   bookmarks + bookmark bar (`bookmarks.rs` port, `native_*` call sites replaced).
3. Persistence: `session.rs` + `browser_view/session.rs` port (same KV keys),
   restore-on-open, save-on-quit, `register_serializable_item::<BrowserView>` so the
   item itself restores with the workspace (follow `TerminalView`'s implementation).
4. Downloads: handler port + a minimal download-status UI (`ToggleDownloadCenter`).
5. Find-in-page (handler + overlay), GPUI context menus (port, including the
   `on_before_context_menu` trap note), permissions handler, favicon pipeline.
6. Popup/OAuth: `life_span_handler` full port. Validate native CEF popup windows on
   Linux/X11 and under Wayland/XWayland (R3).
7. Text input/IME: `EntityInputHandler` + dispatch-classification port
   (`text_input.rs` has 9 unit tests — port them). **Evaluation gate for `d14fb11`**:
   exercise keystrokes while CEF content is focused with editor splits present; adopt
   the gpui patch only on demonstrated misrouting, and record the evidence in this doc.
8. Keymap (context `BrowserView`, shadowing set: `ctrl-t`, `ctrl-w`, `ctrl-shift-t`,
   `ctrl-l`, `ctrl-r`, `ctrl-f`, `ctrl-tab`/`ctrl-shift-tab`, `alt-left`/`alt-right`)
   + `browser` settings section (search engine, new-tab behavior, download dir).
9. UA spoofing carried over (Google sign-in); re-verify it is still needed.

### M3 — Platform breadth + extras

Gate: soft — items are independent.

- macOS *(all unvalidatable on Linux; needs a macOS machine)*: `IoSurfacePresenter`
  (port `render_handler.rs` accelerated path + `surface()` presentation),
  `macos_protocol.rs`, helper binary + `bundle-mac` CEF additions + entitlements
  (Zed-branded naming, not `dev.glass.*`), `use-angle=metal` switch, mimalloc
  necessity check, swipe navigation.
- DevTools toggle (`OpenDevTools` — CEF built-in window), incognito windows
  (persistence skips already modeled), theme-color tab tinting (`page_chrome.rs`
  color half), default-browser registration (Linux `.desktop` + `xdg-settings`;
  macOS deferred to a macOS session).
- Windows: explicitly unscheduled; keep `FramePresenter` and keycode layers
  Windows-shaped (Glass's `stage-windows-cef-runtime.ps1` is the reference when the
  time comes).

---

## 8. Validation register — what cannot be verified on Linux

| Item | Needs |
|---|---|
| IOSurface/CVPixelBuffer zero-copy path, `surface()` rendering | macOS |
| Obj-C `CefAppProtocol` patch behavior | macOS |
| Helper-app bundling, signing, entitlements, notarization | macOS |
| mimalloc/CEF allocator interaction | macOS (crash was observed there) |
| Trackpad swipe (touch-phase scroll events) | macOS hardware |
| macOS default-browser registration | macOS |
| Everything Windows | Windows |
| Widevine/DRM streaming actually playing | any platform + codec caveat (H.264 gap) |

## 9. Risks

- **R1 — CEF-on-Wayland.** CEF/Chromium on Linux is X11-first; under a Wayland
  session it runs via XWayland, and native popup windows (OAuth) may behave oddly.
  Ozone/Wayland flags exist but are not part of the plan. Mitigation: validate on
  both; accept XWayland. *(inferred from Chromium platform status; verify in M1)*
- **R2 — Sandbox disabled.** Glass runs `no_sandbox=1` everywhere. On Linux the
  Chromium sandbox needs user namespaces or a SUID helper; keeping `no_sandbox` is
  the pragmatic M1 choice but is a real security tradeoff for a daily-driver browser.
  Revisit after M2 (decision principles rank correctness first, security eighth —
  documented deliberately).
- **R3 — Native popup windows on Linux.** The OAuth path creates non-OSR CEF windows;
  unowned by GPUI. Focus/stacking behavior needs empirical validation (M2 step 6).
- **R4 — Software-OSR performance.** Full-pane 60fps repaints (video, heavy
  animation) may tax the CPU copy + upload. The presenter seam exists precisely so a
  dmabuf path can be added; do not pre-build it.
- **R5 — `cef-rs` churn.** The bindings move fast and the pinned tag ages. Contained
  by the pin; budget for a bump after M2 (R6).
- **R6 — Chromium staleness = security exposure.** CEF 145 will be months old by M2.
  Schedule a CEF/cef-rs upgrade task immediately post-M2, treating handler-API churn
  as its own change set.
- **R7 — Key dispatch/IME correctness.** The one place a gpui patch might be forced
  (`d14fb11`). Contained by the M2 evaluation gate.
- **R8 — Upstream drift during the build.** Zed main moves daily; merge upstream into
  `glass` at least weekly during M1/M2 so conflicts stay small (ADR-0003 workflow).

## 10. Open (deliberately unscheduled) questions

- Revive the modes UX (full-window Browser/Editor/Terminal switching) as additive
  crates with a cross-platform switcher? Decide after M2, with the browser in daily use.
- Accelerated Linux path (dmabuf import into blade) — only if R4 materializes.
- Windows enablement.
- Upstreaming candidates: the `d14fb11` dispatch fix (if adopted) and any generic
  "embed an externally-rendered surface" element, both of which would shrink the fork.
