# Glass-on-Zed

A fork of Zed (branch `glass`) that adds an integrated CEF-based web browser while
tracking upstream Zed with minimal divergence. Vocabulary below was pinned during the
migration-planning session (2026-07-10); the plan itself lives in
`docs/glass/migration-plan.md`.

## Language

### Browser

**Browser view**:
The single per-workspace entity that owns all browser tabs and their chrome, and
appears in the workspace as one pane item.
_Avoid_: browser pane, browser panel

**Browser tab**:
A web page open inside a browser view, shown in the browser view's internal tab strip.
Distinct from a pane tab — closing a browser tab never closes the pane item.
_Avoid_: page, webview tab

**Pane tab**:
A Zed workspace item's tab in a pane's tab bar. A whole browser view occupies exactly
one pane tab.
_Avoid_: editor tab (when contrasting with browser tabs)

**Omnibox**:
The combined address and search input of a browser view, with history-backed
suggestions.
_Avoid_: address bar, URL bar

**Browser chrome**:
All browser UI that is not page content: tab strip, omnibox, toolbar buttons,
bookmark bar, find overlay.
_Avoid_: shell (reserved for the app shell)

**New-tab page**:
The app-rendered (non-web) content shown in a browser tab with no URL.

**Incognito window**:
A browser context whose tabs, history, and downloads are excluded from persistence.
_Avoid_: private window

### Engine

**CEF subprocess**:
Any Chromium child process (renderer, GPU, utility) CEF spawns. On Linux it is the
main executable re-invoked with `--type=` arguments; on macOS it is a dedicated
helper binary.
_Avoid_: helper (except for the macOS helper binary specifically)

**Message pump**:
The GPUI foreground task that periodically calls CEF's `do_message_loop_work`,
driven by CEF's external-message-pump scheduling callbacks.

**Software OSR**:
Off-screen rendering where CEF hands over a CPU pixel buffer that is uploaded as a
GPUI image each frame. The cross-platform baseline path.

**Accelerated OSR**:
Off-screen rendering where CEF shares a GPU surface (IOSurface on macOS, dmabuf on
Linux, D3D shared handle on Windows) presented zero-copy. macOS-only today; Linux is
a possible future optimization.

**Frame presenter**:
The internal seam in the browser crate that turns CEF paint output into a GPUI
element, with one implementation per presentation path (software, IOSurface).

### Fork management

**Touch list**:
The enumerated, deliberately minimal set of upstream files this fork edits. Any edit
to an upstream file not on the list requires adding it to the list first.

**Additive crate**:
A new crate carrying fork functionality, chosen over modifying upstream crates so
upstream merges stay conflict-free.

**Upstream merge**:
The periodic `git merge` of upstream Zed `main` into `glass`. This fork never rebases
published history and keeps its own `main` as a clean upstream mirror.

### Deferred concepts (defined for reference, not currently built)

**Mode system**:
Glass's full-window switching between Browser, Editor, and Terminal modes via a
registry. Deferred; the browser integrates as a pane item instead.
