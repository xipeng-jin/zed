## Problem Statement

I use Zed as my editor and terminal, but my browsing happens in a separate application. The Glass project proved the product idea — a browser, editor, and terminal in one native window — but Glass is a hard fork that rewrote Zed internals and pinned a forked GPUI, so it fell irrecoverably behind upstream and only ever rendered web content on macOS. I develop on Linux.

I want the integrated browser rebuilt on my up-to-date Zed fork (`glass` branch), working on Linux first, in a way that lets the fork keep merging upstream Zed indefinitely with minimal conflict cost.

## Solution

Add an integrated web browser to the Zed fork as a new additive crate, powered by CEF (Chromium Embedded Framework) in off-screen rendering mode so page content is composited by GPUI like any other element. The browser appears in the workspace as a **browser view** — a single per-workspace pane item containing its own internal **browser tabs**, omnibox, bookmarks, and downloads — splittable next to editors and terminals.

Rendering uses **software OSR** behind a **frame presenter** seam (zero GPUI changes); the macOS **accelerated OSR** (zero-copy) path from Glass is re-added behind the same seam later. Divergence from upstream is controlled by the **additive crate** rule, an enumerated **touch list** of upstream-file edits, and periodic **upstream merges**. Delivery is three milestones: M1 engine-on-Linux, M2 daily-drivable browser UX, M3 macOS parity + extras.

## User Stories

### Browsing core

1. As a Zed user on Linux, I want to open a browser view inside my workspace, so that I can read documentation without leaving my editor.
2. As a Zed user, I want to navigate to a URL and see the page render inside a workspace pane, so that web content behaves like any other workspace content.
3. As a Zed user, I want to click, scroll, and type into web pages, so that the embedded browser is fully interactive, not a preview.
4. As a Zed user, I want back, forward, and reload controls, so that I can navigate without memorizing shortcuts.
5. As a Zed user, I want to split a browser view next to an editor and a terminal, so that I can code against live documentation or a running web app.
6. As a Zed user, I want the browser to keep working while I edit in another pane, so that pages keep loading and playing in the background.
7. As a Zed user, I want the application to start and quit cleanly with the browser engine running, so that the browser never hangs or crashes shutdown.

### Tabs

8. As a Zed user, I want multiple browser tabs inside one browser view, so that my web pages don't crowd the workspace pane tab bar.
9. As a Zed user, I want to open, close, and switch browser tabs, so that I can manage several pages at once.
10. As a Zed user, I want to pin browser tabs, so that important pages stay put and compact.
11. As a Zed user, I want to reopen the last closed browser tab, so that accidental closes are recoverable.
12. As a Zed user, I want each browser tab to show its favicon and title, so that I can identify pages at a glance.
13. As a Zed user, I want links that request a new tab or window to open as browser tabs in my browser view, so that pages can't scatter native windows across my desktop.

### Omnibox, history, bookmarks

14. As a Zed user, I want an omnibox that accepts both URLs and search queries, so that I don't have to decide which one I typed.
15. As a Zed user, I want omnibox suggestions drawn from my browsing history ranked by recency and frequency, so that returning to a page takes a few keystrokes.
16. As a Zed user, I want a new-tab page, so that a fresh tab gives me a starting point instead of a blank page.
17. As a Zed user, I want to bookmark the current page and see my bookmarks in the browser chrome, so that I can keep a stable set of references.
18. As a Zed user, I want to copy the current page URL with one action, so that sharing links is instant.

### Sessions and persistence

19. As a Zed user, I want my browser tabs (including pinned state and active tab) restored when I reopen the workspace, so that my browsing context survives restarts exactly like my editor tabs do.
20. As a Zed user, I want website logins and cookies to persist across restarts, so that I don't re-authenticate daily.
21. As a Zed user, I want my history, bookmarks, and download records saved automatically, so that nothing is lost on quit or crash.
22. As a Zed user, I want an incognito window whose tabs, history, and downloads are never persisted, so that I can browse without leaving traces in my profile.

### Web-app compatibility

23. As a Zed user, I want to sign in to Google, GitHub, and similar services, so that the embedded browser is usable for real work, not just static pages.
24. As a Zed user, I want OAuth login popups to open and complete, so that third-party sign-in flows work end to end.
25. As a Zed user, I want sites requesting camera/microphone and DRM (Widevine) to work, so that meetings and streaming sites function.
26. As a Zed user, I want file downloads saved to my Downloads folder with progress visible and name collisions resolved, so that downloading behaves like a normal browser.
27. As a Zed user, I want a right-click context menu with link/selection/edit actions rendered in the app's own style, so that browser menus feel native to the editor.
28. As a Zed user, I want find-in-page with match counts and next/previous, so that I can search long pages.
29. As a Zed user, I want to type non-Latin text via my input method (IME) into web forms with composition shown correctly, so that the browser is usable in my language.

### Commands, keys, settings

30. As a Zed user, I want browser muscle-memory shortcuts (new tab, close tab, focus omnibox, reload, find, cycle tabs, back/forward) to work while the browser has focus, so that the embedded browser matches every other browser I use.
31. As a Zed user, I want those shortcuts to apply only while a browser view is focused, so that my editor keybindings are untouched everywhere else.
32. As a Zed user, I want editor-level shortcuts (modifier-key chords) to take priority over the web page, so that pages can never swallow my Zed commands.
33. As a Zed user, I want browser actions available in the command palette, so that every browser capability is discoverable and rebindable.
34. As a Zed user, I want settings for default search engine, new-tab behavior, and download directory, so that basic browser behavior is configurable.
35. As a Zed user, I want to open Chromium DevTools for a tab, so that I can debug web apps I'm building.

### Cross-platform

36. As a Zed user on Linux, I want the full browser experience validated on Linux first, so that the platform I develop on is the best-supported one.
37. As a macOS user of the fork, I want the browser to reach parity later, including the zero-copy rendering path, so that macOS performance matches what Glass had.
38. As a Zed user on any platform, I want the browser architecture to avoid platform-specific assumptions in shared code, so that Windows support remains feasible later.

### Maintainer

39. As the fork maintainer, I want all browser functionality in additive crates with a short enumerated touch list of upstream edits, so that merging upstream Zed stays cheap forever.
40. As the fork maintainer, I want zero GPUI modifications unless proven necessary by a documented failure, so that the fork never re-creates Glass's GPUI divergence problem.
41. As the fork maintainer, I want CEF binaries fetched by a checksum-verified script at a pinned version, so that builds are reproducible and the engine supply chain is explicit.
42. As the fork maintainer, I want the CEF version bump to be a deliberate, separate task after the migration, so that port bugs and version-churn bugs are never confounded.
43. As the fork maintainer, I want the browser's model and UI logic covered by deterministic tests that run in CI without CEF, so that upstream merges and refactors are validated automatically.
44. As the fork maintainer, I want macOS-only work items explicitly flagged, so that I know what cannot be validated in the Linux development environment.

### Deferred-but-designed-for

45. As a Zed user, I want the app to be registrable as my OS default browser (later milestone), so that links from other applications open in my workspace.

## Implementation Decisions

All decisions below were confirmed interactively; the four load-bearing ones are recorded as ADRs 0001–0004 in the repo, and the full execution detail (milestone steps, touch list, source-map citations into the Glass reference checkouts, risk register) lives in the migration plan under `docs/glass/`.

- **Engine (ADR-0001):** CEF via the tauri-apps `cef-rs` bindings, in windowless (off-screen) rendering mode with an external message pump. Native webviews and Servo were rejected — only CEF composites into GPUI. The dependency is pinned at Glass's exact CEF/cef-rs version so ported handler code meets the API it was written against.
- **Rendering (ADR-0002):** software OSR first — CEF's paint callback delivers a pixel buffer that is uploaded as a GPUI image. Implemented entirely inside the browser crate behind a `FramePresenter` trait with per-path implementations (software everywhere; macOS IOSurface zero-copy in M3; Linux dmabuf only if measurement later demands it). Zero GPUI changes.
- **Divergence management (ADR-0003):** additive crates; enumerated touch list (workspace manifest, the app entry point for the CEF subprocess guard and init call, keymap/settings assets, one menu entry); periodic upstream merges, never rebases; the fork's main branch mirrors upstream.
- **Workspace model (ADR-0004):** one browser view per workspace as a pane item, holding an internal browser tab strip; browser tabs are not pane tabs. Glass's full-window mode system is deferred; browser-as-default-launch-view is dropped.
- **New module:** a single `browser` crate owning engine lifecycle (init, subprocess handling, message pump on the GPUI executor, shutdown), all CEF handlers (downloads, permissions incl. Widevine auto-accept, GPUI-rendered context menus, find, popup/OAuth lifecycle, display/load events, render-process bridge for text-input editability and theme color), the browser view and chrome, input translation, persistence, and the presenter seam. No helper binary on Linux (the main executable self-forks as the CEF subprocess); a macOS helper binary is added in M3.
- **Engine seam (new, the only new seam):** a tab-backend trait covering commands into the engine (navigate, reload, history traversal, key/mouse/IME injection, close, frame source) paired with the existing engine→app event stream. Two implementations: real CEF, and a scripted test stub.
- **Input routing:** three-way keystroke classification (app-first for modifier chords, IME route for editable fields, raw engine route otherwise), ported from Glass including its editability signal from the render process. Keycode translation to Windows virtual keys is reimplemented for Linux from key names rather than macOS hardware codes. A GPUI focused-input-context dispatch patch from the Glass gpui fork is a contingency only, adopted solely on demonstrated keystroke misrouting during M2, with evidence recorded.
- **Persistence:** JSON blobs in the existing key-value store under the same keys Glass used (tabs, pinned tabs, history, bookmarks, downloads); debounced saves with a synchronous flush on quit; history capped with LRU eviction and fuzzy recency/frequency ranking; web session state (cookies) persists via CEF's own on-disk profile. The browser view registers through the workspace's serializable-item mechanism so it restores with the workspace.
- **Command surface:** browser actions in a dedicated action namespace; a context-scoped keymap active only when a browser view is focused, shadowing conflicting Zed defaults (terminal precedent); minimal settings section (search engine, new-tab behavior, download directory).
- **Supply chain:** a new checksum-verifying download script fetches stock CEF from the public CDN into a conventional cache directory; proprietary-codec source builds are documented but out of scope (H.264/AAC gap accepted); a CEF/Chromium version bump is scheduled immediately after M2 as its own task (security staleness).
- **Milestones:** M1 gate — browse a real site interactively inside a Zed pane on Linux, split beside an editor and terminal, clean quit. M2 gate — daily-drivable on Linux; Google/GitHub logins including an OAuth popup succeed; sessions restore. M3 — macOS parity (zero-copy presenter, process-protocol patch, bundling/entitlements), DevTools, incognito, default-browser registration, theme-color tab tinting, swipe navigation.

## Testing Decisions

- **What makes a good test here:** drive external behavior through the highest seam — GPUI test contexts operating the browser view — and assert on user-visible outcomes (tab state, omnibox suggestions, persisted-and-restored sessions, which route a keystroke took), never on internal call sequences or CEF specifics.
- **One seam:** the tab-backend trait is the only test seam. Above it, everything (browser view, tab strip, omnibox + history ranking, bookmarks, session save/restore round-trips, input-routing classification, popup-redirect policy, workspace item behavior) is tested deterministically in CI with the scripted stub emitting engine events. Below it, real-CEF behavior (rendering, subprocesses, actual input delivery, OAuth windows, Wayland/X11 behavior) is validated manually against the milestone gates and explicitly not unit-tested.
- **Pure-logic tests:** Glass's existing unit tests (keystroke dispatch classification, URL-vs-search heuristics, disposition mapping, page-chrome parsing) are ported with the code they test; they need no seam.
- **Prior art:** GPUI `TestAppContext`/`VisualTestContext` tests as used by the terminal view and workspace item tests in this repo; the key-value persistence round-trip style used by existing serializable items. Per repo test guidance, timeouts and pumping use GPUI executor timers, not ad-hoc async timers.
- **Modules tested:** the `browser` crate's model and UI layers; the touch-list integration points are exercised by one smoke test that registers the item and opens a browser view in a test workspace.

## Out of Scope

- iOS support in any form (hard constraint), including the Glass gpui fork's iOS/Apple-mobile abstractions.
- Wholesale adoption of the Glass gpui fork; the native macOS control suite (SF-Symbol image views, native buttons/menus/sidebar) is rejected and replaced with the stock component library.
- Glass's mode system (full-window Browser/Editor/Terminal switching), native macOS toolbar/titlebar, native sidebar, and the associated workspace rewrites — deferred or excluded per ADR-0004/0003.
- Glass's `service_hub`/`app_runtime` layers (App Store Connect tooling, run-project layer), Glass themes/branding/app identity, sidebar thread navigator, terminal session manager.
- Windows enablement (architecture stays Windows-shaped; no scheduled work).
- Proprietary-codec CEF builds; accelerated (dmabuf) Linux rendering; Chromium sandbox hardening (`no_sandbox` accepted initially) — each documented for later.
- OS default-browser registration before M3.

## Further Notes

- Authoritative companion documents in-repo: `docs/glass/migration-plan.md` (execution contract with citations and the complete touch list), root `CONTEXT.md` (glossary — this spec uses its terms), `docs/adr/0001`–`0004`.
- Reference checkouts for porting: `~/Projects/refs/Glass` and `~/Projects/refs/gpui` (read at the rev Glass pins, `3790fca` — the fork's HEAD has deleted the native-controls system Glass uses).
- Known risks tracked in the plan: CEF under Wayland runs via XWayland (validate popups there), disabled Chromium sandbox, software-OSR performance ceiling, cef-rs API churn, Chromium staleness, upstream drift during the build (merge upstream at least weekly).
- Implementation must not begin from this spec alone for M1 step details — the migration plan's §7 numbered steps are the executable breakdown.
