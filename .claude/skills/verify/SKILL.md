---
name: verify
description: Build, launch, and drive the dev Zed build (glass branch, CEF browser) headlessly for end-to-end verification on this Linux machine
---

# Verifying Zed (glass branch) end-to-end

## Build

```bash
CEF_PATH=~/.local/share/cef cargo build -p zed   # cef feature is on via crates/zed/Cargo.toml
```

## Harness (nested Xwayland, isolated data dir)

Each long-lived process gets its own `run_in_background` Bash task with `exec`:

1. `exec Xwayland :99 -geometry 1280x800 -decorate` (remove stale `/tmp/.X99-lock` first)
2. Optional local test server (browser flows): `python3 -m http.server`-style script on 127.0.0.1
3. ```bash
   exec env -u WAYLAND_DISPLAY DISPLAY=:99 \
     CEF_PATH=$HOME/.local/share/cef LD_LIBRARY_PATH=$HOME/.local/share/cef \
     VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json \
     ZED_CEF_DEBUG=1 target/debug/zed --user-data-dir /tmp/<short>/ud
   ```
   - `VK_DRIVER_FILES` (llvmpipe) avoids a RADV swapchain deadlock under nested Xwayland.
   - `--user-data-dir` isolates config (`<dir>/config/settings.json`), db (`<dir>/db/0-dev/db.sqlite`), CEF profile. Short path (Unix socket 108-byte cap). Fresh dir shows onboarding — `ctrl+enter` dismisses.
   - `--foreground` is not accepted alongside `--user-data-dir`.
   - `ZED_CEF_DEBUG=1` exposes CDP on `127.0.0.1:9222` for page-state assertions (`/json` + `Runtime.evaluate` via python `websockets`).

## Drive

Reusable python-xlib XTEST driver + CDP helper live in the session scratchpad pattern
(`drive.py`: geom/shot/key/type/click/palette/clearmods; `cdp.py <url-substring> <js>`).
Key traps:

- Multiple X windows carry WM_CLASS "zed"; pick the **largest viewable** one (a 10x10 unmapped helper window appears first).
- `set_input_focus` before every key send (no WM on :99); XTEST typing lands only after clicking INTO the target element.
- Screenshots: ImageMagick `import -display :99 -window <id>`; poke a redraw (palette open+esc) after relaunch before capturing.
- Browser session/history/bookmarks/downloads persist as JSON blobs in `kv_store` (`browser_tabs` etc.) — inspect/plant with sqlite3; ctrl+q flushes synchronously.
- Navigate a tab off a download URL before quitting, or restore re-triggers the download.

Full trap list: the `glass-gui-validation-harness` memory.
