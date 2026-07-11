# Browser integrates as a pane item; Glass's mode system is deferred

The browser enters the workspace as a single per-workspace `BrowserView` pane item
containing its own internal browser tab strip (browser tabs are not workspace pane
tabs), openable and splittable like any other item. Glass's alternative — a
full-window "mode" system (Browser/Editor/Terminal) with the browser as the default
view on first launch — is deferred indefinitely, and browser-as-default is dropped
outright: this project is Zed-first with an integrated browser, not a browser that
embeds an editor.

Glass actually shipped both integrations simultaneously; we keep only the pane-item
path because it is the Zed-native shape, needs no workspace rendering hooks beyond
stock item registration, and avoids the mode system's coupling to the macOS native
toolbar we are not porting.

## Consequences

- Browser tabs and pane tabs are distinct concepts (see `CONTEXT.md`); keybindings
  are context-scoped to `BrowserView` so browser muscle-memory keys (ctrl-t, ctrl-w,
  ctrl-l, …) shadow Zed defaults only while the browser has focus, following the
  terminal's precedent.
- If the modes UX is revived post-M2, it must return as additive crates with a
  cross-platform switcher, not as a port of Glass's `workspace.rs` hooks.
