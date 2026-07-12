# CEF as the embedded browser engine

The integrated browser uses the Chromium Embedded Framework via the `cef-rs`
bindings (pinned at an exact tag, `cef-v150.0.0+150.0.10` since the post-M2
version bump; originally Glass's `cef-v145.6.1+145.0.28`), running in off-screen
rendering mode so page pixels are composited by GPUI like any other element. CEF is
the only production-quality engine that supports rendering into someone else's
compositor, which is what lets browser content live inside workspace panes, be
clipped and overlaid normally, and coexist with editor/terminal splits on every
platform; it also preserves the bulk of Glass's existing handler and input code.

## Considered options

- **Native webviews (wry / WKWebView / WebView2 / WebKitGTK)** — rejected: they are
  native child views overlaid on the window, invisible to GPUI's compositor, which
  breaks pane splitting, overlays, and cross-platform visual consistency.
- **Servo** — rejected: not production-ready for daily browsing.

## Consequences

CEF is a heavy dependency: ~200MB of per-platform binaries fetched by
`script/download-cef`, subprocess management, and per-platform packaging work. Stock
CEF builds lack H.264/AAC codecs (accepted; source builds with proprietary codecs are
documented but out of scope). The pinned Chromium version ages and must be bumped
deliberately after the initial migration (see migration plan, risk R6).
